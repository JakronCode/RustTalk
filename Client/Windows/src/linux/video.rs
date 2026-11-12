//! Native Linux capture: V4L2 cameras and portal-authorized PipeWire screens.
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::SyncSender, Arc};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode,
};
use gst::prelude::*;
use futures_util::StreamExt;
use gst_video::prelude::*;
use gstreamer as gst;
use gstreamer_app::AppSink;
use gstreamer_video as gst_video;

use crate::video_common::{spawn_encoder, EncodeJob, EncodeParams, Pacer};
pub use crate::video_common::{
    CameraDevice, ShareEntry, ShareSource, VideoCapture, VideoOut, VideoQuality,
};

pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    gst::init()?;
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor.start().context("Could not discover cameras")?;
    let devices = monitor.devices();
    monitor.stop();
    let mut cameras: Vec<_> = devices
        .iter()
        .filter_map(|device| {
            let properties = device.properties()?;
            let path = properties
                .get::<String>("device.path")
                .or_else(|_| properties.get::<String>("api.v4l2.path"))
                .ok()?;
            let index = path.strip_prefix("/dev/video")?.parse().ok()?;
            Some(CameraDevice {
                index,
                name: format!("{} ({path})", device.display_name()),
            })
        })
        .collect();
    cameras.sort_by_key(|camera| camera.index);
    cameras.dedup_by_key(|camera| camera.index);
    Ok(cameras)
}

// Source selection is owned by the desktop portal; no windows are enumerated here.
pub fn list_share_sources() -> Vec<ShareEntry> {
    Vec::new()
}

/// Stops the pipeline even when capture, conversion, or encoding fails.
struct CapturePipeline(gst::Pipeline);
impl Drop for CapturePipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

fn capture_pipeline(source: &str, bounds: Option<EncodeParams>) -> Result<(CapturePipeline, AppSink)> {
    gst::init()?;
    // Scale before copying pixels into Rust; full-resolution thumbnailing is too slow for live video.
    let bounds = bounds.map(|p| format!(",width=(int)[1,{}],height=(int)[1,{}],pixel-aspect-ratio=(fraction)1/1", p.max_w, p.max_h)).unwrap_or_default();
    let pipeline = gst::parse::launch(&format!(
        "{source} ! videoconvertscale ! video/x-raw,format=RGB{bounds} ! appsink name=frames max-buffers=1 drop=true sync=false"
    ))?.downcast::<gst::Pipeline>().map_err(|_| anyhow::anyhow!("Invalid capture pipeline"))?;
    let sink = pipeline
        .by_name("frames")
        .context("Capture sink missing")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("Invalid capture sink"))?;
    let pipeline = CapturePipeline(pipeline);
    pipeline
        .0
        .set_state(gst::State::Playing)
        .context("Could not start capture")?;
    Ok((pipeline, sink))
}

fn sample_image(sample: &gst::Sample) -> Result<image::RgbImage> {
    let info = gst_video::VideoInfo::from_caps(sample.caps().context("Frame has no format")?)?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(
        sample.buffer().context("Frame has no buffer")?,
        &info,
    )?;
    let stride = usize::try_from(frame.plane_stride()[0]).context("Negative frame stride")?;
    let row_bytes = info.width() as usize * 3;
    let data = frame.plane_data(0)?;
    anyhow::ensure!(stride >= row_bytes, "Invalid RGB frame stride");
    let mut rgb = image::RgbImage::new(info.width(), info.height());
    for (row, output) in rgb.as_mut().chunks_exact_mut(row_bytes).enumerate() {
        let start = row.checked_mul(stride).context("Frame offset overflow")?;
        output.copy_from_slice(
            data.get(start..start + row_bytes)
                .context("Truncated RGB frame")?,
        );
    }
    Ok(rgb)
}

struct ScreenSending<'a>(Option<&'a AtomicBool>);
impl Drop for ScreenSending<'_> {
    fn drop(&mut self) {
        if let Some(sending) = self.0 { sending.store(false, Ordering::Relaxed); }
    }
}

fn capture_loop(source: &str, stop: &AtomicBool, jobs: SyncSender<EncodeJob>, params: EncodeParams, sending: Option<&AtomicBool>) -> Result<()> {
    let (pipeline, sink) = capture_pipeline(source, Some(params))?;
    // Declared after the pipeline: clear audio consent before pipeline teardown can wait.
    let _sending = ScreenSending(sending);
    let bus = pipeline.0.bus().context("Capture bus missing")?;
    let mut pacer = Pacer::new();
    let mut last_frame = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos]) {
            match message.view() {
                gst::MessageView::Error(e) => {
                    bail!("Capture failed: {} ({:?})", e.error(), e.debug())
                }
                _ => bail!("Capture ended: device disconnected or screen sharing stopped"),
            }
        }
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
            last_frame = Instant::now();
            if pacer.ready_at(last_frame) {
                let img = image::DynamicImage::ImageRgb8(sample_image(&sample)?);
                if stop.load(Ordering::Relaxed) { break; }
                // A Playing request may still be asynchronous; only a valid frame proves capture is live.
                if let Some(sending) = sending { sending.store(true, Ordering::Relaxed); }
                if stop.load(Ordering::Relaxed) {
                    if let Some(sending) = sending { sending.store(false, Ordering::Relaxed); }
                    break;
                }
                match jobs.try_send(EncodeJob { img }) {
                    Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
        } else if last_frame.elapsed() > Duration::from_secs(15) {
            bail!("Capture produced no frames for 15 seconds");
        }
    }
    Ok(())
}

pub fn start_camera<F, E>(
    index: u32,
    quality: VideoQuality,
    on_frame: F,
    mut on_error: E,
) -> Result<VideoCapture>
where
    F: FnMut(VideoOut) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    let cap = VideoCapture {
        stop: Arc::new(AtomicBool::new(false)),
        sending: Arc::new(AtomicBool::new(false)),
        keyframe: Arc::new(AtomicBool::new(false)),
    };
    let jobs = spawn_encoder(
        "video-cam",
        quality.camera(),
        false,
        true,
        cap.sending.clone(),
        cap.keyframe.clone(),
        on_frame,
    )?;
    let stop = cap.stop.clone();
    std::thread::Builder::new()
        .name("video-cam".into())
        .spawn(move || {
            if let Err(e) = capture_loop(
                &format!("v4l2src device=/dev/video{index} ! capsfilter caps=\"image/jpeg,width=(int)[1,{}],height=(int)[1,{}],framerate=(fraction)[1/1,30/1];video/x-raw,width=(int)[1,{}],height=(int)[1,{}],framerate=(fraction)[1/1,30/1]\" ! decodebin",
                    quality.camera().max_w, quality.camera().max_h, quality.camera().max_w, quality.camera().max_h),
                &stop,
                jobs,
                quality.camera(),
                None,
            ) {
                if !stop.load(Ordering::Relaxed) {
                    on_error(format!("Camera error: {e:#}"));
                }
            }
        })?;
    Ok(cap)
}

async fn until_stopped<T>(
    stop: &AtomicBool,
    future: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        result = future => Some(result),
        _ = async {
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        } => None,
    }
}

async fn stop_screen_capture(
    sending: &AtomicBool,
    capture_stop: &AtomicBool,
    cleanup: impl std::future::Future<Output = ()>,
) {
    capture_stop.store(true, Ordering::Relaxed);
    sending.store(false, Ordering::Relaxed);
    cleanup.await;
}

async fn portal_capture(stop: Arc<AtomicBool>, sending: Arc<AtomicBool>, jobs: SyncSender<EncodeJob>, params: EncodeParams) -> Result<()> {
    let proxy = Screencast::new()
        .await
        .context("Screen sharing requires a working desktop portal")?;
    let session = proxy.create_session().await?;
    let result: Result<()> = async {
        let sources = proxy.available_source_types().await? & (SourceType::Monitor | SourceType::Window);
        anyhow::ensure!(!sources.is_empty(), "Desktop portal offers no screens or windows");
        let cursor = if proxy.available_cursor_modes().await?.contains(CursorMode::Embedded) {
            CursorMode::Embedded
        } else { CursorMode::Hidden };
        let Some(request) = until_stopped(&stop, proxy.select_sources(&session, cursor, sources, false, None, PersistMode::DoNot)).await else { return Ok(()) };
        request?.response()?;
        let Some(request) = until_stopped(&stop, proxy.start(&session, None)).await else { return Ok(()) };
        let response = request?.response()?;
        let node = response.streams().first().context("No screen or window selected")?.pipe_wire_node_id();
        let fd = proxy.open_pipe_wire_remote(&session).await?;
        let mut closed = session.receive_closed().await?;
        let capture_stop = Arc::new(AtomicBool::new(false));
        let worker_stop = capture_stop.clone();
        let worker_sending = sending.clone();
        let mut capture = tokio::task::spawn_blocking(move || {
            // Older PipeWire versions lack on-disconnect; also watch portal closure below.
            let probe = gst::ElementFactory::make("pipewiresrc").build()?;
            let disconnect = if probe.find_property("on-disconnect").is_some() { "on-disconnect=eos" } else { "" };
            let keepalive = if probe.find_property("keepalive-time").is_some() { "keepalive-time=40" } else { "" };
            let source = format!("pipewiresrc fd={} path={node} do-timestamp=true {keepalive} {disconnect} ! video/x-raw,width=(int)[1,16384],height=(int)[1,16384],pixel-aspect-ratio=(fraction)1/1", fd.as_raw_fd());
            capture_loop(&source, &worker_stop, jobs, params, Some(&worker_sending))
        });
        tokio::select! {
            result = &mut capture => { sending.store(false, Ordering::Relaxed); result??; }
            _ = closed.next() => {
                stop_screen_capture(&sending, &capture_stop, async { let _ = capture.await; }).await;
                bail!("Desktop stopped screen sharing");
            }
            _ = until_stopped(&stop, std::future::pending::<()>()) => {
                stop_screen_capture(&sending, &capture_stop, async { let _ = capture.await; }).await;
            }
        }
        Ok(())
    }.await;
    sending.store(false, Ordering::Relaxed);
    let _ = session.close().await;
    result
}

pub fn start_screen<F, E>(
    _: ShareSource,
    quality: VideoQuality,
    on_frame: F,
    mut on_error: E,
) -> Result<VideoCapture>
where
    F: FnMut(VideoOut) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    gst::init()?;
    anyhow::ensure!(
        gst::ElementFactory::find("pipewiresrc").is_some(),
        "Install the GStreamer PipeWire plugin to share a screen"
    );
    let cap = VideoCapture {
        stop: Arc::new(AtomicBool::new(false)),
        sending: Arc::new(AtomicBool::new(false)),
        keyframe: Arc::new(AtomicBool::new(false)),
    };
    let jobs = spawn_encoder(
        "video-screen",
        quality.screen(),
        true,
        false,
        cap.sending.clone(),
        cap.keyframe.clone(),
        on_frame,
    )?;
    let stop = cap.stop.clone();
    let sending = cap.sending.clone();
    std::thread::Builder::new()
        .name("video-screen".into())
        .spawn(move || {
            let run = || -> Result<()> {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(portal_capture(stop.clone(), sending.clone(), jobs, quality.screen()))
            };
            let result = run();
            sending.store(false, Ordering::Relaxed);
            if let Err(e) = result {
                if !stop.load(Ordering::Relaxed) {
                    on_error(format!("Screen sharing stopped: {e:#}"));
                }
            }
        })?;
    Ok(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screen_without_frames_never_enables_audio() {
        gst::init().unwrap();
        let sending = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (jobs, _receiver) = std::sync::mpsc::sync_channel(1);
        let worker_stop = stop.clone();
        let worker_sending = sending.clone();
        let worker = std::thread::spawn(move || capture_loop(
            "videotestsrc is-live=true ! valve drop=true", &worker_stop, jobs,
            VideoQuality::Balanced.screen(), Some(&worker_sending)));
        std::thread::sleep(Duration::from_millis(200));
        let live_before_stop = sending.load(Ordering::Relaxed);
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap().unwrap();
        assert!(!live_before_stop);
        assert!(!sending.load(Ordering::Relaxed));
    }

    #[test]
    fn portal_stop_clears_sending_before_pending_cleanup() {
        use futures_util::FutureExt;
        let sending = AtomicBool::new(true);
        let stop = AtomicBool::new(false);
        assert!(stop_screen_capture(&sending, &stop, std::future::pending()).now_or_never().is_none());
        assert!(!sending.load(Ordering::Relaxed));
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn rgb_frames_respect_row_padding() {
        gst::init().unwrap();
        let (pipeline, sink) =
            capture_pipeline("videotestsrc num-buffers=1 ! video/x-raw,width=319,height=181", None)
                .unwrap();
        let sample = sink
            .try_pull_sample(gst::ClockTime::from_seconds(5))
            .expect("test frame");
        let image = sample_image(&sample).unwrap();
        assert_eq!(image.dimensions(), (319, 181));
        assert!(image.as_raw().iter().any(|v| *v != 0));
        drop(pipeline);
    }
    #[test]
    fn native_scaling_bounds_frames_and_preserves_aspect_ratio() {
        for (width, height) in [(3840, 2160), (3440, 1440), (200, 100)] {
            let source = format!("videotestsrc num-buffers=1 ! video/x-raw,width={width},height={height},pixel-aspect-ratio=1/1");
            let (_pipeline, sink) = capture_pipeline(&source, Some(VideoQuality::Balanced.screen())).unwrap();
            let sample = sink.try_pull_sample(gst::ClockTime::from_seconds(5)).unwrap();
            let image = sample_image(&sample).unwrap();
            assert!(image.width() <= 1280 && image.height() <= 720);
            let source_ratio = width as f64 / height as f64;
            let actual_ratio = image.width() as f64 / image.height() as f64;
            assert!((source_ratio - actual_ratio).abs() < 0.01, "{width}x{height} became {}x{}", image.width(), image.height());
            assert!(image.width() <= width && image.height() <= height, "small sources should not be enlarged");
        }
    }

    #[test]
    fn stopping_capture_releases_encoder() {
        let stop = Arc::new(AtomicBool::new(false));
        let sending = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let jobs = spawn_encoder(
            "capture-stop-test",
            VideoQuality::Balanced.camera(),
            false,
            true,
            sending,
            keyframe,
            move |out| {
                let _ = tx.send(out);
            },
        )
        .unwrap();
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            capture_loop(
                "videotestsrc is-live=true ! video/x-raw,width=320,height=180",
                &thread_stop,
                jobs,
                VideoQuality::Balanced.camera(),
                None,
            )
        });
        assert!(rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .preview
            .is_some());
        stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("Capture did not stop: {e}"),
            }
        }
        thread.join().unwrap().unwrap();
    }

    #[test]
    fn missing_camera_reports_error_and_stops() {
        let (tx, rx) = std::sync::mpsc::channel();
        let errors = tx.clone();
        let cap = start_camera(
            u32::MAX,
            VideoQuality::Balanced,
            move |_| {
                let _ = tx.send(false);
            },
            move |_| {
                let _ = errors.send(true);
            },
        )
        .unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "expected camera error, not a frame"
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
        drop(cap);
    }

    #[test]
    fn stop_cancels_portal_wait() {
        let stop = AtomicBool::new(true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(runtime
            .block_on(until_stopped(&stop, std::future::pending::<()>()))
            .is_none());
    }
    #[test]
    #[ignore = "requires a connected webcam"]
    fn hw_camera_previews_and_streams() {
        let camera = list_cameras()
            .unwrap()
            .into_iter()
            .next()
            .expect("Connect a webcam");
        let (tx, rx) = std::sync::mpsc::channel();
        let errors = tx.clone();
        let cap = start_camera(
            camera.index,
            VideoQuality::Balanced,
            move |out| {
                let _ = tx.send(Ok(out));
            },
            move |error| {
                let _ = errors.send(Err(error));
            },
        )
        .unwrap();
        let preview = rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
        assert!(preview.preview.is_some() && preview.frame.is_none());
        cap.set_sending(true);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .unwrap();
            if let Some(frame) = out.frame {
                let mut decoder = crate::video_codec::H264Decoder::new().unwrap();
                assert!(matches!(
                    decoder.decode(&frame).unwrap(),
                    crate::video_codec::Decoded::Picture(_)
                ));
                break;
            }
        }
        cap.stop();
    }
    #[test]
    #[ignore = "manual webcam frame-rate benchmark; camera must be free"]
    fn hw_camera_frame_rate() {
        let camera = list_cameras().unwrap().into_iter().next().expect("Connect a webcam");
        let (tx, rx) = std::sync::mpsc::channel();
        let errors = tx.clone();
        let cap = start_camera(camera.index, VideoQuality::High,
            move |out| { let _ = tx.send(Ok(out)); },
            move |error| { let _ = errors.send(Err(error)); }).unwrap();
        rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
        cap.set_sending(true);
        let start = Instant::now();
        let duration = Duration::from_secs(5);
        let mut frames = 0;
        while start.elapsed() < duration {
            match rx.recv_timeout(duration.saturating_sub(start.elapsed())) {
                Ok(Ok(out)) => frames += usize::from(out.frame.is_some()),
                Ok(Err(error)) => panic!("{error}"),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(error) => panic!("{error}"),
            }
        }
        cap.stop();
        println!("Webcam High, actual capture + resize + H.264 + preview: {:.1} fps ({frames} frames / 5 seconds)", frames as f64 / 5.0);
        assert!(frames > 0);
    }

    #[test]
    #[ignore = "opens the native screen selection dialog"]
    fn hw_portal_screen_streams() {
        let (tx, rx) = std::sync::mpsc::channel();
        let errors = tx.clone();
        let cap = start_screen(
            ShareSource::Monitor(0),
            VideoQuality::Balanced,
            move |out| {
                let _ = tx.send(Ok(out));
            },
            move |error| {
                let _ = errors.send(Err(error));
            },
        )
        .unwrap();
        let out = rx
            .recv_timeout(Duration::from_secs(90))
            .expect("Select a screen in the desktop dialog")
            .unwrap();
        let mut decoder = crate::video_codec::H264Decoder::new().unwrap();
        assert!(matches!(
            decoder.decode(&out.frame.unwrap()).unwrap(),
            crate::video_codec::Decoded::Picture(_)
        ));
        let start = Instant::now();
        let mut frames = 0;
        while start.elapsed() < Duration::from_secs(5) {
            match rx.recv_timeout(Duration::from_secs(5).saturating_sub(start.elapsed())) {
                Ok(Ok(out)) => frames += usize::from(out.frame.is_some()),
                Ok(Err(error)) => panic!("{error}"),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(error) => panic!("{error}"),
            }
        }
        println!("Actual portal capture + H.264: {:.1} fps", frames as f64 / 5.0);
        cap.stop();
    }
}

/// Packaging smoke check: catch missing dlopen plugins without a camera or desktop.
pub fn check_runtime() -> Result<()> {
    gst::init()?;
    for name in ["pipewiresrc", "v4l2src", "decodebin", "typefind", "jpegdec", "videoconvertscale", "appsink", "queue"] {
        gst::ElementFactory::make(name).build().with_context(|| format!("Missing capture element {name}"))?;
    }
    Ok(())
}
