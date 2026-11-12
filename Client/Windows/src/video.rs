//! Video-Capture: Kamera (nokhwa/Media Foundation) und Bildschirm/Fenster
//! (xcap) -> H.264 (`video_codec`).
//!
//! Each source runs a capture thread and ONE encoder thread (H.264 frames
//! depend on each other, so they are encoded in order). The capture thread
//! hands over at most one picture at a time; a busy encoder drops frames
//! instead of building up latency. Frames go to a sink `on_frame(VideoOut)`:
//! the H.264 frame while `sending`, plus a preview picture for the camera
//! dialog.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
pub use crate::video_common::{CameraDevice, ShareEntry, ShareSource, VideoCapture, VideoOut, VideoQuality};
use crate::video_common::{spawn_encoder, EncodeJob, Pacer, FRAME_INTERVAL, THUMB_W, THUMB_H};

// ---------------------------------------------------------------------------
// Kamera
// ---------------------------------------------------------------------------

/// Alle Kameras (Media Foundation). Leere Liste, wenn keine da ist.
pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    use nokhwa::utils::{ApiBackend, CameraIndex};
    let infos = nokhwa::query(ApiBackend::MediaFoundation).context("Could not list cameras")?;
    Ok(infos
        .into_iter()
        .enumerate()
        .map(|(i, info)| CameraDevice {
            index: match info.index() {
                CameraIndex::Index(n) => *n,
                CameraIndex::String(_) => i as u32,
            },
            name: info.human_name(),
        })
        .collect())
}

/// Kamera oeffnen. Erst 720p/480p MJPEG mit 30 fps anfragen –
/// `AbsoluteHighestFrameRate` landet sonst gern bei Formaten, die nokhwa
/// nicht dekodieren kann oder die viel zu gross sind.
fn open_camera(index: u32) -> Result<nokhwa::Camera> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    };

    let attempts = [
        RequestedFormatType::Closest(CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30)),
        RequestedFormatType::Closest(CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30)),
        RequestedFormatType::Closest(CameraFormat::new_from(640, 480, FrameFormat::YUYV, 30)),
        RequestedFormatType::Closest(CameraFormat::new_from(640, 480, FrameFormat::NV12, 30)),
        RequestedFormatType::AbsoluteHighestFrameRate,
        RequestedFormatType::None,
    ];
    let mut last_err = None;
    for kind in attempts {
        let req = RequestedFormat::new::<RgbFormat>(kind);
        match nokhwa::Camera::new(CameraIndex::Index(index), req) {
            Ok(mut cam) => match cam.open_stream() {
                Ok(()) => {
                    tracing::info!("Kamera {index} offen: {}", cam.camera_format());
                    return Ok(cam);
                }
                Err(e) => last_err = Some(anyhow::anyhow!("Could not start the camera stream: {e}")),
            },
            Err(e) => last_err = Some(anyhow::anyhow!("Could not open the camera: {e}")),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("No camera found")))
}

/// Verstaendliche Meldung fuer Lese-Fehler der Kamera.
/// 0x800703E3 (ERROR_OPERATION_ABORTED) kommt von Windows selbst, wenn das
/// Geraet zwar gefunden wird, aber keinen Videostrom liefert – tritt auch mit
/// reinem Media Foundation ohne nokhwa auf (Blende zu, Kamera haengt, ...).
fn camera_read_error_text(err: &str) -> String {
    if err.contains("0x800703E3") {
        "The camera delivers no images (Windows: operation aborted). Check the privacy shutter / \
         camera switch, re-plug the camera or pick another one."
            .to_string()
    } else if err.contains("0x80070005") {
        "No access to the camera: enable Windows Settings > Privacy > Camera > \
         \"Let desktop apps access your camera\"."
            .to_string()
    } else {
        format!("The camera delivers no images: {err}")
    }
}

/// Kamera-Thread. Startet im Vorschau-Modus (`sending = false`).
/// Hinweis: `Camera` ist nicht Send -> wird im Thread erzeugt. Fehler
/// gehen ueber `on_error` an die UI.
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
    let stop = Arc::new(AtomicBool::new(false));
    let sending = Arc::new(AtomicBool::new(false));
    let keyframe = Arc::new(AtomicBool::new(false));
    let jobs = spawn_encoder("video-cam", quality.camera(), false, true, sending.clone(), keyframe.clone(), on_frame)?;
    std::thread::Builder::new()
        .name("video-cam".to_string())
        .spawn({
            let stop = stop.clone();
            move || {
                use nokhwa::pixel_format::RgbFormat;

                let mut camera = match open_camera(index) {
                    Ok(c) => c,
                    Err(e) => {
                        on_error(format!("Camera error: {e:#}"));
                        return;
                    }
                };

                let mut pacer = Pacer::new();
                let mut failures = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    // frame() blockiert bis zum naechsten Kamera-Frame und
                    // leert so den Puffer; der Pacer laesst ~25 fps durch.
                    let frame = match camera.frame() {
                        Ok(f) => f,
                        Err(e) => {
                            failures += 1;
                            tracing::warn!("Kamera-Frame: {e}");
                            if failures >= 50 {
                                on_error(camera_read_error_text(&e.to_string()));
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                    };
                    failures = 0;
                    if !pacer.ready_at(Instant::now()) {
                        continue;
                    }
                    let img = match frame.decode_image::<RgbFormat>() {
                        Ok(i) => i,
                        Err(e) => {
                            tracing::warn!("Kamera-Decode: {e}");
                            continue;
                        }
                    };
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // Encoder busy -> drop the picture (no growing latency).
                    let _ = jobs.try_send(EncodeJob { img: image::DynamicImage::ImageRgb8(img) });
                }
                let _ = camera.stop_stream();
                // `jobs` faellt hier weg -> Encoder-Thread beendet sich.
            }
        })
        .context("Kamera-Thread starten")?;
    Ok(VideoCapture { stop, sending, keyframe })
}

// ---------------------------------------------------------------------------
// Bildschirm / Fenster
// ---------------------------------------------------------------------------

fn thumb(img: image::RgbaImage) -> image::RgbaImage {
    image::DynamicImage::ImageRgba8(img).thumbnail(THUMB_W, THUMB_H).to_rgba8()
}

/// Alle teilbaren Monitore und Fenster inkl. Vorschaubild.
/// Dauert je nach Fensteranzahl einige 100 ms -> im Hintergrund aufrufen.
pub fn list_share_sources() -> Vec<ShareEntry> {
    let mut out = Vec::new();

    match xcap::Monitor::all() {
        Ok(monitors) => {
            for (i, m) in monitors.iter().enumerate() {
                let Ok(id) = m.id() else { continue };
                let primary = m.is_primary().unwrap_or(false);
                let name = m
                    .friendly_name()
                    .ok()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("Screen {}", i + 1));
                let title = if primary { format!("{name} (main screen)") } else { name };
                let subtitle = format!(
                    "{} x {}",
                    m.width().unwrap_or(0),
                    m.height().unwrap_or(0)
                );
                out.push(ShareEntry {
                    source: ShareSource::Monitor(id),
                    title,
                    subtitle,
                    thumbnail: m.capture_image().ok().map(thumb),
                });
            }
        }
        Err(e) => tracing::warn!("Monitore abfragen: {e}"),
    }

    let own_pid = std::process::id();
    match xcap::Window::all() {
        Ok(windows) => {
            for w in windows {
                if w.pid().ok() == Some(own_pid) || w.is_minimized().unwrap_or(true) {
                    continue;
                }
                let title = w.title().unwrap_or_default();
                if title.trim().is_empty() || title == "Program Manager" {
                    continue;
                }
                if w.width().unwrap_or(0) < 120 || w.height().unwrap_or(0) < 80 {
                    continue;
                }
                let Ok(id) = w.id() else { continue };
                let app = w.app_name().unwrap_or_default();
                out.push(ShareEntry {
                    source: ShareSource::Window(id),
                    title,
                    subtitle: app,
                    thumbnail: w.capture_image().ok().map(thumb),
                });
            }
        }
        Err(e) => tracing::warn!("Fenster abfragen: {e}"),
    }
    out
}

/// Handle auf die Quelle; wird im Capture-Thread erzeugt.
enum Target {
    Monitor(xcap::Monitor),
    Window(xcap::Window),
}

fn find_target(source: ShareSource) -> Result<Target> {
    match source {
        ShareSource::Monitor(id) => xcap::Monitor::all()
            .context("Monitore abfragen")?
            .into_iter()
            .find(|m| m.id().ok() == Some(id))
            .map(Target::Monitor)
            .context("The screen is no longer available"),
        ShareSource::Window(id) => xcap::Window::all()
            .context("Fenster abfragen")?
            .into_iter()
            .find(|w| w.id().ok() == Some(id))
            .map(Target::Window)
            .context("The window was closed"),
    }
}

/// Bildschirm/Fenster-Thread. Sendet sofort (`sending = true`).
pub fn start_screen<F, E>(
    source: ShareSource,
    quality: VideoQuality,
    on_frame: F,
    mut on_error: E,
) -> Result<VideoCapture>
where
    F: FnMut(VideoOut) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let sending = Arc::new(AtomicBool::new(true));
    let keyframe = Arc::new(AtomicBool::new(false));

    // Pipeline: Aufnahme-Thread -> H.264-Encoder-Thread.
    let jobs = spawn_encoder("video-screen", quality.screen(), true, false, sending.clone(), keyframe.clone(), on_frame)?;

    std::thread::Builder::new()
        .name("video-screen".to_string())
        .spawn({
            let stop = stop.clone();
            move || {
                let target = match find_target(source) {
                    Ok(t) => t,
                    Err(e) => {
                        on_error(format!("Screen share error: {e:#}"));
                        return;
                    }
                };
                let mut failures = 0u32;
                let mut pacer = Pacer::new();
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(pacer.wait());
                    pacer.ready_at(Instant::now());
                    let shot = match &target {
                        Target::Monitor(m) => m.capture_image(),
                        Target::Window(w) => {
                            if w.is_minimized().unwrap_or(false) {
                                std::thread::sleep(FRAME_INTERVAL);
                                continue;
                            }
                            w.capture_image()
                        }
                    };
                    match shot {
                        Ok(img) => {
                            failures = 0;
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            // Encoder busy -> drop the picture.
                            let _ = jobs.try_send(EncodeJob { img: image::DynamicImage::ImageRgba8(img) });
                        }
                        Err(e) => {
                            failures += 1;
                            tracing::warn!("Screen-Frame: {e}");
                            // ~2 s am Stueck keine Bilder (50 x 40 ms) -> Fenster weg o. ae.
                            if failures >= 50 {
                                on_error(format!("Screen sharing stopped: {e}"));
                                break;
                            }
                        }
                    }
                }
            }
        })
        .context("Screen-Thread starten")?;
    Ok(VideoCapture { stop, sending, keyframe })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_fits_picker_box() {
        let img = image::RgbaImage::from_pixel(2560, 1440, image::Rgba([1, 2, 3, 255]));
        let t = thumb(img);
        assert!(t.width() <= THUMB_W && t.height() <= THUMB_H);
    }

    #[test]
    fn camera_errors_are_readable() {
        assert!(camera_read_error_text("Could not capture frame (0x800703E3)").contains("shutter"));
        assert!(camera_read_error_text("denied (0x80070005)").contains("Privacy"));
        assert!(camera_read_error_text("boom").ends_with("boom"));
    }

    /// Echte Hardware: `cargo test -p rusttalk-client -- --ignored --nocapture hw_`
    #[test]
    #[ignore]
    fn hw_camera_delivers_frames() {
        let cams = list_cameras().expect("Kamera-Abfrage");
        let cam = cams.first().expect("keine Kamera angeschlossen");
        let (tx, rx) = std::sync::mpsc::channel();
        let cap = start_camera(cam.index, VideoQuality::High, move |o| { let _ = tx.send(o); }, |e| panic!("{e}")).unwrap();
        let out = rx.recv_timeout(Duration::from_secs(10)).expect("Kamera-Frame");
        assert!(out.frame.is_none() && out.preview.is_some(), "startet im Vorschau-Modus");
        drop(cap);
    }

    /// Bildrate und Datenrate beim Teilen des Hauptmonitors (Release-Build aussagekraeftig).
    #[test]
    #[ignore]
    fn hw_screen_h264_rate() {
        let list = list_share_sources();
        let mon = list.iter().find(|e| matches!(e.source, ShareSource::Monitor(_))).expect("Monitor");
        let (tx, rx) = std::sync::mpsc::channel();
        let cap = start_screen(mon.source, VideoQuality::High, move |o| { let _ = tx.send(o); }, |e| panic!("{e}")).unwrap();
        let t0 = Instant::now();
        let (mut n, mut bytes) = (0, 0);
        while t0.elapsed() < Duration::from_secs(3) {
            if let Ok(o) = rx.recv_timeout(Duration::from_millis(500)) {
                n += 1;
                bytes += o.frame.map(|f| f.len()).unwrap_or(0);
            }
        }
        println!("{:.1} fps, {:.2} Mbit/s", n as f64 / 3.0, bytes as f64 * 8.0 / 3.0 / 1e6);
        drop(cap);
    }

    #[test]
    fn sending_flag_and_keyframe_request() {
        let cap = VideoCapture {
            stop: Arc::new(AtomicBool::new(false)),
            sending: Arc::new(AtomicBool::new(false)),
            keyframe: Arc::new(AtomicBool::new(false)),
        };
        assert!(!cap.is_sending());
        cap.set_sending(true);
        assert!(cap.is_sending());
        cap.request_keyframe();
        assert!(cap.keyframe.load(Ordering::Relaxed));
        let stop = cap.stop.clone();
        drop(cap);
        assert!(stop.load(Ordering::Relaxed), "Drop stoppt den Thread");
    }
}
