//! Shared video types, pacing, and bounded H.264 encoder pipeline.
#[cfg(test)]
use crate::video_codec::fit_even;
use crate::video_codec::{fit_even_fast, H264Encoder};
use anyhow::{Context, Result};
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frame interval: 30 fps (`video_codec::FPS`).
pub const FRAME_INTERVAL: Duration = Duration::from_micros((1_000_000.0 / crate::video_codec::FPS) as u64);

/// Gleichmaessiger Takt: laesst im Schnitt einen Frame pro `FRAME_INTERVAL`
/// durch, auch wenn die Quelle (z. B. Kamera mit 30 fps) etwas schneller ist.
pub(crate) struct Pacer {
    next: Instant,
}

impl Pacer {
    pub(crate) fn new() -> Self {
        Self {
            next: Instant::now(),
        }
    }

    pub(crate) fn ready_at(&mut self, now: Instant) -> bool {
        // 5 ms tolerance so 30-fps camera frames (33 ms) are not missed by a hair.
        if now + Duration::from_millis(5) < self.next {
            return false;
        }
        self.next += FRAME_INTERVAL;
        if self.next + FRAME_INTERVAL < now {
            self.next = now; // nach Haenger nicht "nachholen"
        }
        true
    }

    /// Wie lange bis zum naechsten Frame (fuer Quellen ohne eigenen Takt).
    pub(crate) fn wait(&self) -> Duration {
        self.next.saturating_duration_since(Instant::now())
    }
}
/// Groesse der Vorschaubilder im Bildschirm/Fenster-Auswahldialog.
pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;

/// Laufende Capture-Quelle; Drop stoppt den Thread.
pub struct VideoCapture {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) sending: Arc<AtomicBool>,
    pub(crate) keyframe: Arc<AtomicBool>,
}

impl VideoCapture {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Frames ab jetzt an den Server senden (Vorschau -> live).
    pub fn set_sending(&self, on: bool) {
        self.sending.store(on, Ordering::Relaxed);
    }

    pub fn is_sending(&self) -> bool {
        self.sending.load(Ordering::Relaxed)
    }

    /// A viewer needs a full picture: the next frame is a keyframe.
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Qualitaetsstufe fuer gesendete Streams (Einstellungen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VideoQuality {
    /// 720p screen / 540p camera: little CPU and bandwidth (Wi-Fi).
    Balanced,
    /// 1080p screen / 720p camera: sharp in fullscreen (LAN).
    #[default]
    High,
    /// 1440p screen / 1080p camera: for big screens and fast connections.
    Best,
}

/// Kodier-Parameter einer Quelle.
#[derive(Debug, Clone, Copy)]
pub struct EncodeParams {
    pub max_w: u32,
    pub max_h: u32,
    /// H.264 bit rate (bits per second).
    pub bitrate: u32,
}

impl VideoQuality {
    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::Balanced => "Balanced (720p30, up to ~2.5 Mbit/s)",
            VideoQuality::High => "High (1080p30, up to ~6 Mbit/s)",
            VideoQuality::Best => "Best (1440p30, up to ~10 Mbit/s)",
        }
    }

    pub fn screen(self) -> EncodeParams {
        match self {
            VideoQuality::Balanced => EncodeParams { max_w: 1280, max_h: 720, bitrate: 2_500_000 },
            VideoQuality::High => EncodeParams { max_w: 1920, max_h: 1080, bitrate: 6_000_000 },
            VideoQuality::Best => EncodeParams { max_w: 2560, max_h: 1440, bitrate: 10_000_000 },
        }
    }

    pub fn camera(self) -> EncodeParams {
        match self {
            VideoQuality::Balanced => EncodeParams { max_w: 960, max_h: 540, bitrate: 1_200_000 },
            VideoQuality::High => EncodeParams { max_w: 1280, max_h: 720, bitrate: 2_500_000 },
            VideoQuality::Best => EncodeParams { max_w: 1920, max_h: 1080, bitrate: 5_000_000 },
        }
    }

    /// Camera resolution to ask the device for (it may give less).
    #[allow(dead_code)] // used by the Windows camera path
    pub fn camera_request(self) -> (u32, u32) {
        match self {
            VideoQuality::Best => (1920, 1080),
            _ => (1280, 720),
        }
    }
}

/// What the encoder thread delivers per picture.
pub struct VideoOut {
    /// H.264 wire frame (only while sending).
    pub frame: Option<Vec<u8>>,
    /// Picture for the local camera preview (camera only).
    pub preview: Option<egui::ColorImage>,
}

/// Auftrag an den Encoder-Thread.
pub(crate) struct EncodeJob {
    pub(crate) img: image::DynamicImage,
}

/// Starts the encoder thread of a source. It ends when the returned sender
/// is dropped. The channel holds one picture: a busy encoder drops frames.
pub(crate) fn spawn_encoder<F>(
    name: &str,
    params: EncodeParams,
    screen: bool,
    preview: bool,
    sending: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    mut on_frame: F,
) -> Result<std::sync::mpsc::SyncSender<EncodeJob>>
where
    F: FnMut(VideoOut) + Send + 'static,
{
    let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<EncodeJob>(1);
    std::thread::Builder::new()
        .name(format!("{name}-enc"))
        .spawn(move || {
            let mut enc = H264Encoder::with_tuning(params.bitrate, screen, crate::video_codec::Tuning::stream(screen));
            let mut was_sending = false;
            for job in job_rx {
                // Fast SIMD scaling; RGB stays RGB, RGBA stays RGBA (screen pictures).
                let pic = fit_even_fast(job.img, params.max_w, params.max_h);
                let now_sending = sending.load(Ordering::Relaxed);
                // Asked for, or just went live: start with a full picture.
                if keyframe.swap(false, Ordering::Relaxed) || (now_sending && !was_sending) {
                    enc.request_keyframe();
                }
                was_sending = now_sending;
                let frame = if now_sending {
                    match enc.encode_image(&pic) {
                        Ok(f) => Some(f),
                        Err(e) => {
                            tracing::warn!("{e:#}");
                            None
                        }
                    }
                } else {
                    None
                };
                let size = [pic.width() as usize, pic.height() as usize];
                let preview = preview.then(|| match &pic {
                    image::DynamicImage::ImageRgba8(rgba) => egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    other => egui::ColorImage::from_rgb(size, other.to_rgb8().as_raw()),
                });
                if frame.is_some() || preview.is_some() {
                    on_frame(VideoOut { frame, preview });
                }
            }
        })
        .context("Kodier-Thread starten")?;
    Ok(job_tx)
}

/// Eine gefundene Kamera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    pub index: u32,
    pub name: String,
}

/// Was geteilt wird: ein ganzer Monitor oder ein einzelnes Fenster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShareSource {
    Monitor(u32),
    Window(u32),
}

/// Eintrag im Auswahl-Dialog (inkl. kleinem Vorschaubild).
pub struct ShareEntry {
    pub source: ShareSource,
    /// Fenstertitel bzw. Monitorname.
    pub title: String,
    /// Anwendungsname (Fenster) bzw. Aufloesung (Monitor).
    pub subtitle: String,
    pub thumbnail: Option<image::RgbaImage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pacer_keeps_30_fps_cameras_and_thins_faster_sources() {
        let start = Instant::now();
        // 3 s of a 30 fps camera (33.3 ms apart, a little jitter): every frame passes.
        let mut pacer = Pacer { next: start };
        let accepted = (0..90).filter(|i| pacer.ready_at(start + Duration::from_micros(33_333 * i + (i % 3) * 900))).count();
        assert!((86..=90).contains(&accepted), "~90 frames expected, got {accepted}");
        // A 60 fps source is thinned to ~30 fps.
        let mut pacer = Pacer { next: start };
        let accepted = (0..180).filter(|i| pacer.ready_at(start + Duration::from_micros(16_667 * i))).count();
        assert!((85..=95).contains(&accepted), "~90 of 180 expected, got {accepted}");
    }

    /// The encoder thread: preview only while not sending, keyframe when going
    /// live, H.264 frames decodable in order, thread ends with the sender.
    #[test]
    fn encoder_thread_previews_then_streams() {
        let params = EncodeParams {
            max_w: 320,
            max_h: 180,
            bitrate: 500_000,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let sending = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(false));
        let jobs = spawn_encoder(
            "test",
            params,
            false,
            true,
            sending.clone(),
            keyframe,
            move |out| {
                let _ = tx.send(out);
            },
        )
        .unwrap();
        let pic = |v: u8| {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                640,
                360,
                image::Rgb([v, 40, 200]),
            ))
        };
        jobs.send(EncodeJob { img: pic(10) }).unwrap();
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(first.frame.is_none(), "preview only");
        assert_eq!(
            first.preview.as_ref().unwrap().size,
            [320, 180],
            "scaled to the quality size"
        );
        sending.store(true, Ordering::Relaxed);
        let mut dec = crate::video_codec::H264Decoder::new().unwrap();
        let mut pictures = 0;
        for v in 0..5u8 {
            jobs.send(EncodeJob { img: pic(v * 40) }).unwrap();
            let out = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let frame = out.frame.expect("H.264 while sending");
            if let crate::video_codec::Decoded::Picture(img) = dec.decode(&frame).unwrap() {
                assert_eq!(img.size, [320, 180]);
                pictures += 1;
            }
        }
        assert!(
            pictures >= 4,
            "first live frame is a keyframe -> decodable right away ({pictures})"
        );
        drop(jobs);
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_err(),
            "thread ended"
        );
    }

    #[test]
    fn quality_levels_scale_up() {
        let (b, h, best) = (VideoQuality::Balanced.screen(), VideoQuality::High.screen(), VideoQuality::Best.screen());
        assert!(h.max_w > b.max_w && h.bitrate > b.bitrate);
        assert!(best.max_w > h.max_w && best.bitrate > h.bitrate);
        assert!(VideoQuality::Best.camera().max_w > VideoQuality::High.camera().max_w);
        assert_eq!(FRAME_INTERVAL, Duration::from_micros(33_333), "30 fps");
        assert_eq!(VideoQuality::default(), VideoQuality::High);
    }
    /// Compare with `cargo test --release -p rusttalk-client video_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual frame-processing benchmark"]
    fn video_frame_cost() {
        for (name, width, height, params, screen) in [
            ("camera", 2560, 1440, VideoQuality::High.camera(), false),
            ("screen", 3840, 2160, VideoQuality::High.screen(), true),
        ] {
            let source = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
                image::Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
            }));
            let mut encoder = H264Encoder::new(params.bitrate, screen);
            let mut resizing = Duration::ZERO;
            let mut encoding = Duration::ZERO;
            let mut previewing = Duration::ZERO;
            for _ in 0..8 {
                let start = Instant::now();
                let rgb = fit_even(source.clone(), params.max_w, params.max_h);
                resizing += start.elapsed();
                let start = Instant::now();
                assert!(encoder.encode(&rgb).unwrap().len() >= crate::video_codec::FRAME_HEADER);
                encoding += start.elapsed();
                if !screen {
                    let start = Instant::now();
                    let preview = egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw());
                    std::hint::black_box(preview);
                    previewing += start.elapsed();
                }
            }
            println!("{name}: resize={:.1} ms, encode={:.1} ms, preview={:.1} ms, processing ceiling={:.1} fps",
                resizing.as_secs_f64() * 125.0, encoding.as_secs_f64() * 125.0,
                previewing.as_secs_f64() * 125.0, 8.0 / (resizing + encoding + previewing).as_secs_f64());
        }
    }

}
