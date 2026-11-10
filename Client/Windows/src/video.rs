//! Video-Capture: Kamera (nokhwa) und Bildschirm (scrap) -> JPEG-Frames.
//!
//! MVP-Ansatz: MJPEG (~5 fps, max. 640x360) ueber den TCP-Steuerkanal.
//! Auf dem LAN voellig ausreichend; H.264/AV1-Streaming waere Phase 5.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Frame-Intervall: ~5 fps reichen fuers Teilen im LAN.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(200);
/// Zielgroesse (Thumbnail, Seitenverhaeltnis bleibt).
pub const MAX_W: u32 = 640;
pub const MAX_H: u32 = 360;

/// Laufende Capture-Quelle; Drop stoppt den Thread.
pub struct VideoCapture {
    stop: Arc<AtomicBool>,
}

impl VideoCapture {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// JPEG-Kodierung: DynamicImage -> Thumbnail -> JPEG (Qualitaet 60).
pub fn encode_jpeg(img: image::DynamicImage) -> Result<Vec<u8>> {
    let img = img.thumbnail(MAX_W, MAX_H);
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 60);
    enc.encode_image(&img).context("JPEG-Encode")?;
    Ok(out)
}

/// Kamera-Thread: Default-Kamera, Frames -> `on_frame` (JPEG-Bytes).
/// Hinweis: `Camera` ist nicht Send -> wird im Thread erzeugt. Fehler
/// gehen ueber `on_error` an die UI.
pub fn start_camera<F, E>(mut on_frame: F, mut on_error: E) -> Result<VideoCapture>
where
    F: FnMut(Vec<u8>) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::Builder::new()
        .name("video-cam".to_string())
        .spawn({
            let stop = stop.clone();
            move || {
                use nokhwa::pixel_format::RgbFormat;
                use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
                use nokhwa::Camera;

                let res = (|| -> Result<Camera> {
                    let req = RequestedFormat::new::<RgbFormat>(
                        RequestedFormatType::AbsoluteHighestFrameRate,
                    );
                    let mut cam =
                        Camera::new(CameraIndex::Index(0), req).context("Keine Kamera gefunden")?;
                    cam.open_stream().context("Kamera-Stream starten")?;
                    Ok(cam)
                })();
                let mut camera = match res {
                    Ok(c) => c,
                    Err(e) => {
                        on_error(format!("{e:#}"));
                        return;
                    }
                };

                let mut last = Instant::now() - FRAME_INTERVAL;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    if last.elapsed() < FRAME_INTERVAL {
                        continue;
                    }
                    last = Instant::now();
                    let frame = match camera.frame() {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!("Kamera-Frame: {e}");
                            continue;
                        }
                    };
                    let img = match frame.decode_image::<RgbFormat>() {
                        Ok(i) => i,
                        Err(e) => {
                            tracing::warn!("Kamera-Decode: {e}");
                            continue;
                        }
                    };
                    match encode_jpeg(image::DynamicImage::ImageRgb8(img)) {
                        Ok(jpeg) => on_frame(jpeg),
                        Err(e) => tracing::warn!("JPEG-Encode: {e}"),
                    }
                }
                let _ = camera.stop_stream();
            }
        })
        .context("Kamera-Thread starten")?;
    Ok(VideoCapture { stop })
}

/// Bildschirm-Thread: Primaer-Monitor, Frames -> `on_frame` (JPEG-Bytes).
/// Hinweis: `Capturer` enthaelt rohe DXGI-Zeiger (nicht Send) -> wird im
/// Thread erzeugt.
pub fn start_screen<F, E>(mut on_frame: F, mut on_error: E) -> Result<VideoCapture>
where
    F: FnMut(Vec<u8>) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::Builder::new()
        .name("video-screen".to_string())
        .spawn({
            let stop = stop.clone();
            move || {
                let res = (|| -> Result<scrap::Capturer> {
                    let display = scrap::Display::primary().context("Kein Display gefunden")?;
                    Ok(scrap::Capturer::new(display).context("Screen-Capturer starten")?)
                })();
                let mut capturer = match res {
                    Ok(c) => c,
                    Err(e) => {
                        on_error(format!("{e:#}"));
                        return;
                    }
                };
                let (w, h) = (capturer.width() as u32, capturer.height() as u32);

                let mut last = Instant::now() - FRAME_INTERVAL;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(30));
                    if last.elapsed() < FRAME_INTERVAL {
                        continue;
                    }
                    last = Instant::now();
                    match capturer.frame() {
                        Ok(frame) => {
                            // scrap liefert BGRA -> nach RGBA mischen.
                            let mut rgba = frame.to_vec();
                            for px in rgba.chunks_exact_mut(4) {
                                px.swap(0, 2);
                            }
                            let Some(img) =
                                image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(w, h, rgba)
                            else {
                                continue;
                            };
                            match encode_jpeg(image::DynamicImage::ImageRgba8(img)) {
                                Ok(jpeg) => on_frame(jpeg),
                                Err(e) => tracing::warn!("JPEG-Encode: {e}"),
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Kein neuer Frame (Desktop unveraendert) -> ueberspringen.
                        }
                        Err(e) => {
                            tracing::warn!("Screen-Frame: {e}");
                        }
                    }
                }
            }
        })
        .context("Screen-Thread starten")?;
    Ok(VideoCapture { stop })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_encode_produces_valid_jpeg() {
        let img = image::ImageBuffer::from_fn(128, 96, |x, y| {
            image::Rgba([(x * 2) as u8, (y * 2) as u8, 128, 255])
        });
        let bytes = encode_jpeg(image::DynamicImage::ImageRgba8(img)).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG-Magic");
        // Und dekodierbar:
        assert!(image::load_from_memory(&bytes).is_ok());
    }

    #[test]
    fn jpeg_thumbnail_bounds_size() {
        let img = image::ImageBuffer::from_pixel(1920, 1080, image::Rgba([10, 20, 30, 255]));
        let bytes = encode_jpeg(image::DynamicImage::ImageRgba8(img)).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.width() <= MAX_W && decoded.height() <= MAX_H);
        assert_eq!(decoded.width(), MAX_W, "Seitenverhaeltnis bleibt");
    }
}
