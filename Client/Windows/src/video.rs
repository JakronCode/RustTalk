//! Video-Capture: Kamera (nokhwa/Media Foundation) und Bildschirm/Fenster
//! (xcap) -> JPEG-Frames.
//!
//! MVP-Ansatz: MJPEG (~25 fps; Kamera max. 960x540, Bildschirm max.
//! 1280x720) ueber den TCP-Steuerkanal. Auf dem LAN ausreichend;
//! H.264/AV1-Streaming waere Phase 5.
//!
//! Jede Quelle laeuft in einem eigenen Thread und liefert Frames an eine
//! Senke `on_frame(jpeg, sending)`. `sending` ist `false`, solange die
//! Quelle nur fuer die lokale Vorschau laeuft (Kamera-Dialog) – die UI
//! zeigt den Frame dann nur lokal an und schickt nichts an den Server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Frame-Intervall: 25 fps.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(40);
/// Standardgroesse fuer `encode_jpeg` (nur Tests).
#[cfg(test)]
pub const MAX_W: u32 = 960;
#[cfg(test)]
pub const MAX_H: u32 = 540;

/// Gleichmaessiger Takt: laesst im Schnitt einen Frame pro `FRAME_INTERVAL`
/// durch, auch wenn die Quelle (z. B. Kamera mit 30 fps) etwas schneller ist.
struct Pacer {
    next: Instant,
}

impl Pacer {
    fn new() -> Self {
        Self { next: Instant::now() }
    }

    fn ready_at(&mut self, now: Instant) -> bool {
        // 5 ms Toleranz, damit 33-ms-Kameraframes nicht jedes 2. Mal knapp verfehlen.
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
    fn wait(&self) -> Duration {
        self.next.saturating_duration_since(Instant::now())
    }
}
/// Groesse der Vorschaubilder im Bildschirm/Fenster-Auswahldialog.
pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;

/// Laufende Capture-Quelle; Drop stoppt den Thread.
pub struct VideoCapture {
    stop: Arc<AtomicBool>,
    sending: Arc<AtomicBool>,
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
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Qualitaetsstufe fuer gesendete Streams (Einstellungen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VideoQuality {
    /// 720p-Bildschirm / 540p-Kamera: wenig CPU und Bandbreite (WLAN).
    Balanced,
    /// 1080p-Bildschirm / 720p-Kamera: scharf im Vollbild (LAN).
    #[default]
    High,
}

/// Kodier-Parameter einer Quelle.
#[derive(Debug, Clone, Copy)]
pub struct EncodeParams {
    pub max_w: u32,
    pub max_h: u32,
    pub quality: u8,
    /// Parallele Kodier-Threads (1080p braucht ~65 ms pro Bild -> 3 fuer 25 fps).
    pub workers: usize,
}

impl VideoQuality {
    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::Balanced => "Ausgewogen (720p, ~2,5 MB/s)",
            VideoQuality::High => "Hoch (1080p, ~6 MB/s)",
        }
    }

    pub fn screen(self) -> EncodeParams {
        match self {
            VideoQuality::Balanced => EncodeParams { max_w: 1280, max_h: 720, quality: 70, workers: 2 },
            VideoQuality::High => EncodeParams { max_w: 1920, max_h: 1080, quality: 75, workers: 3 },
        }
    }

    pub fn camera(self) -> EncodeParams {
        match self {
            VideoQuality::Balanced => EncodeParams { max_w: 960, max_h: 540, quality: 70, workers: 1 },
            VideoQuality::High => EncodeParams { max_w: 1280, max_h: 720, quality: 80, workers: 2 },
        }
    }
}

/// Auftrag an den Encoder-Pool (Sequenznummer haelt die Reihenfolge).
struct EncodeJob {
    seq: u64,
    img: image::DynamicImage,
}

/// Encoder-Pool: `workers` Threads kodieren parallel, ein Sammler-Thread
/// ruft `on_frame` in Aufnahme-Reihenfolge auf (verspaetete aeltere Bilder
/// werden verworfen). Die Queue fasst nur `workers` Auftraege; ist sie voll,
/// verwirft der Aufrufer das Bild (`try_send`) statt Latenz aufzubauen.
/// Alles endet, sobald der zurueckgegebene Sender gedroppt wird.
fn spawn_encoder_pool<F>(
    name: &str,
    params: EncodeParams,
    sending: Arc<AtomicBool>,
    mut on_frame: F,
) -> Result<std::sync::mpsc::SyncSender<EncodeJob>>
where
    F: FnMut(Vec<u8>, bool) + Send + 'static,
{
    let workers = params.workers.max(1);
    let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<EncodeJob>(workers);
    let job_rx = Arc::new(std::sync::Mutex::new(job_rx));
    let (out_tx, out_rx) = std::sync::mpsc::channel::<(u64, Vec<u8>)>();
    for i in 0..workers {
        let job_rx = job_rx.clone();
        let out_tx = out_tx.clone();
        std::thread::Builder::new()
            .name(format!("{name}-enc{i}"))
            .spawn(move || loop {
                let job = match job_rx.lock().unwrap().recv() {
                    Ok(j) => j,
                    Err(_) => break, // Aufnahme beendet
                };
                match encode_jpeg_q(job.img, params.max_w, params.max_h, params.quality) {
                    Ok(jpeg) => {
                        if out_tx.send((job.seq, jpeg)).is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!("JPEG-Encode: {e}"),
                }
            })
            .context("Kodier-Thread starten")?;
    }
    drop(out_tx);
    std::thread::Builder::new()
        .name(format!("{name}-out"))
        .spawn(move || {
            let mut last = 0u64;
            for (seq, jpeg) in out_rx {
                if seq <= last {
                    continue; // ein neueres Bild war schneller fertig
                }
                last = seq;
                on_frame(jpeg, sending.load(Ordering::Relaxed));
            }
        })
        .context("Sammler-Thread starten")?;
    Ok(job_tx)
}

/// JPEG-Kodierung mit Standardgroesse (Tests/Hilfen).
#[cfg(test)]
pub fn encode_jpeg(img: image::DynamicImage) -> Result<Vec<u8>> {
    encode_jpeg_max(img, MAX_W, MAX_H)
}

/// JPEG-Kodierung: DynamicImage -> Thumbnail -> JPEG (Qualitaet 60).
#[cfg(test)]
pub fn encode_jpeg_max(img: image::DynamicImage, max_w: u32, max_h: u32) -> Result<Vec<u8>> {
    encode_jpeg_q(img, max_w, max_h, 60)
}

/// Wie `encode_jpeg_max`, mit waehlbarer JPEG-Qualitaet (1-100).
pub fn encode_jpeg_q(img: image::DynamicImage, max_w: u32, max_h: u32, quality: u8) -> Result<Vec<u8>> {
    let img = if img.width() > max_w || img.height() > max_h {
        img.thumbnail(max_w, max_h)
    } else {
        img
    };
    // JPEG kennt keinen Alpha-Kanal -> immer RGB8 kodieren.
    let img = image::DynamicImage::ImageRgb8(img.to_rgb8());
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode_image(&img).context("JPEG-Encode")?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Kamera
// ---------------------------------------------------------------------------

/// Eine gefundene Kamera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    pub index: u32,
    pub name: String,
}

/// Alle Kameras (Media Foundation). Leere Liste, wenn keine da ist.
pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    use nokhwa::utils::{ApiBackend, CameraIndex};
    let infos = nokhwa::query(ApiBackend::MediaFoundation).context("Kameras abfragen")?;
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
                Err(e) => last_err = Some(anyhow::anyhow!("Kamera-Stream starten: {e}")),
            },
            Err(e) => last_err = Some(anyhow::anyhow!("Kamera oeffnen: {e}")),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Keine Kamera gefunden")))
}

/// Verstaendliche Meldung fuer Lese-Fehler der Kamera.
/// 0x800703E3 (ERROR_OPERATION_ABORTED) kommt von Windows selbst, wenn das
/// Geraet zwar gefunden wird, aber keinen Videostrom liefert – tritt auch mit
/// reinem Media Foundation ohne nokhwa auf (Blende zu, Kamera haengt, ...).
fn camera_read_error_text(err: &str) -> String {
    if err.contains("0x800703E3") {
        "Kamera liefert keine Bilder (Windows: Vorgang abgebrochen). Privacy-Blende/Kamera-Schalter pruefen, \
         Kamera neu einstecken oder andere Kamera waehlen."
            .to_string()
    } else if err.contains("0x80070005") {
        "Kein Zugriff auf die Kamera: Windows-Einstellungen > Datenschutz > Kamera > \
         \"Desktop-Apps den Zugriff erlauben\" einschalten."
            .to_string()
    } else {
        format!("Kamera liefert keine Bilder: {err}")
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
    F: FnMut(Vec<u8>, bool) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let sending = Arc::new(AtomicBool::new(false));
    let jobs = spawn_encoder_pool("video-cam", quality.camera(), sending.clone(), on_frame)?;
    std::thread::Builder::new()
        .name("video-cam".to_string())
        .spawn({
            let stop = stop.clone();
            move || {
                let mut seq = 0u64;
                use nokhwa::pixel_format::RgbFormat;

                let mut camera = match open_camera(index) {
                    Ok(c) => c,
                    Err(e) => {
                        on_error(format!("Kamera-Fehler: {e:#}"));
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
                    seq += 1;
                    // Pool voll -> Bild verwerfen (keine wachsende Latenz).
                    let _ = jobs.try_send(EncodeJob { seq, img: image::DynamicImage::ImageRgb8(img) });
                }
                let _ = camera.stop_stream();
                // `jobs` faellt hier weg -> Encoder-Pool beendet sich.
            }
        })
        .context("Kamera-Thread starten")?;
    Ok(VideoCapture { stop, sending })
}

// ---------------------------------------------------------------------------
// Bildschirm / Fenster
// ---------------------------------------------------------------------------

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
                    .unwrap_or_else(|| format!("Bildschirm {}", i + 1));
                let title = if primary { format!("{name} (Hauptbildschirm)") } else { name };
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
            .context("Bildschirm nicht mehr vorhanden"),
        ShareSource::Window(id) => xcap::Window::all()
            .context("Fenster abfragen")?
            .into_iter()
            .find(|w| w.id().ok() == Some(id))
            .map(Target::Window)
            .context("Fenster wurde geschlossen"),
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
    F: FnMut(Vec<u8>, bool) + Send + 'static,
    E: FnMut(String) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let sending = Arc::new(AtomicBool::new(true));

    // Pipeline: Aufnahme-Thread -> Encoder-Pool (parallel, Reihenfolge bleibt).
    let jobs = spawn_encoder_pool("video-screen", quality.screen(), sending.clone(), on_frame)?;

    std::thread::Builder::new()
        .name("video-screen".to_string())
        .spawn({
            let stop = stop.clone();
            move || {
                let mut seq = 0u64;
                let target = match find_target(source) {
                    Ok(t) => t,
                    Err(e) => {
                        on_error(format!("Bildschirm-Fehler: {e:#}"));
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
                            // Pool voll -> Kodierer haengen hinterher, Bild verwerfen.
                            seq += 1;
                            let _ = jobs.try_send(EncodeJob {
                                seq,
                                img: image::DynamicImage::ImageRgba8(img),
                            });
                        }
                        Err(e) => {
                            failures += 1;
                            tracing::warn!("Screen-Frame: {e}");
                            // ~2 s am Stueck keine Bilder (50 x 40 ms) -> Fenster weg o. ae.
                            if failures >= 50 {
                                on_error(format!("Teilen beendet: {e}"));
                                break;
                            }
                        }
                    }
                }
            }
        })
        .context("Screen-Thread starten")?;
    Ok(VideoCapture { stop, sending })
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

    #[test]
    fn screen_frames_use_larger_size() {
        let img = image::RgbaImage::from_pixel(3440, 1440, image::Rgba([1, 2, 3, 255]));
        let bytes =
            {
            let p = VideoQuality::High.screen();
            encode_jpeg_q(image::DynamicImage::ImageRgba8(img), p.max_w, p.max_h, p.quality).unwrap()
        };
        let d = image::load_from_memory(&bytes).unwrap();
        assert_eq!(d.width(), 1920);
        assert!(d.height() <= 1080);
        // Kleine Bilder werden nicht hochskaliert.
        let small = image::RgbaImage::from_pixel(200, 100, image::Rgba([1, 2, 3, 255]));
        let bytes = encode_jpeg_max(image::DynamicImage::ImageRgba8(small), 1280, 720).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 200);
    }

    #[test]
    fn pacer_yields_about_25_fps_from_30_fps_camera() {
        let start = Instant::now();
        let mut pacer = Pacer { next: start };
        // 3 s Kamera mit 30 fps (33,3 ms Abstand).
        let accepted = (0..90)
            .filter(|i| pacer.ready_at(start + Duration::from_micros(33_333 * i)))
            .count();
        assert!((70..=78).contains(&accepted), "~75 Frames erwartet, bekam {accepted}");
    }

    #[test]
    fn encoder_pool_keeps_order_and_shuts_down() {
        let params = EncodeParams { max_w: 64, max_h: 64, quality: 70, workers: 3 };
        let (tx, rx) = std::sync::mpsc::channel();
        let sending = Arc::new(AtomicBool::new(true));
        let jobs = spawn_encoder_pool("test", params, sending, move |jpeg, s| {
            let _ = tx.send((image::load_from_memory(&jpeg).unwrap(), s));
        })
        .unwrap();
        // Bildinhalt = Sequenznummer (Rotwert), damit die Reihenfolge pruefbar ist.
        for seq in 1..=20u64 {
            let img = image::RgbImage::from_pixel(32, 32, image::Rgb([(seq * 10) as u8, 0, 0]));
            jobs.send(EncodeJob { seq, img: image::DynamicImage::ImageRgb8(img) }).unwrap();
        }
        drop(jobs); // Pool muss sich danach beenden -> rx endet.
        let reds: Vec<u8> = rx.iter().map(|(img, s)| {
            assert!(s);
            img.to_rgb8().get_pixel(16, 16)[0]
        }).collect();
        assert!(!reds.is_empty());
        // Streng steigend (JPEG-Toleranz: Abstand 10 je Schritt).
        assert!(reds.windows(2).all(|w| w[1] > w[0]), "Reihenfolge: {reds:?}");
    }

    #[test]
    fn quality_levels_scale_up() {
        let (b, h) = (VideoQuality::Balanced.screen(), VideoQuality::High.screen());
        assert!(h.max_w > b.max_w && h.workers >= b.workers);
        assert_eq!(VideoQuality::default(), VideoQuality::High);
    }

    #[test]
    fn thumbnail_fits_picker_box() {
        let img = image::RgbaImage::from_pixel(2560, 1440, image::Rgba([1, 2, 3, 255]));
        let t = thumb(img);
        assert!(t.width() <= THUMB_W && t.height() <= THUMB_H);
    }

    #[test]
    fn camera_errors_are_readable() {
        assert!(camera_read_error_text("Could not capture frame (0x800703E3)").contains("Blende"));
        assert!(camera_read_error_text("denied (0x80070005)").contains("Datenschutz"));
        assert!(camera_read_error_text("boom").ends_with("boom"));
    }

    /// Echte Hardware: `cargo test -p rusttalk-client -- --ignored --nocapture hw_`
    #[test]
    #[ignore]
    fn hw_camera_delivers_frames() {
        let cams = list_cameras().expect("Kamera-Abfrage");
        println!("Kameras: {cams:?}");
        let cam = cams.first().expect("keine Kamera angeschlossen");
        let (tx, rx) = std::sync::mpsc::channel();
        let (etx, erx) = std::sync::mpsc::channel();
        let cap = start_camera(cam.index, VideoQuality::High, move |j, s| { let _ = tx.send((j, s)); }, move |e| { let _ = etx.send(e); })
            .unwrap();
        let (jpeg, sending) = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| {
            panic!("kein Kamera-Frame: {:?}", erx.try_recv())
        });
        assert!(!sending, "startet im Vorschau-Modus");
        let img = image::load_from_memory(&jpeg).unwrap();
        println!("Kamera-Frame {}x{}", img.width(), img.height());
        drop(cap);
    }

    /// Kodierzeit/Groesse verschiedener Stufen auf dem Hauptmonitor.
    #[test]
    #[ignore]
    fn hw_screen_quality_levels() {
        let m = xcap::Monitor::all().unwrap().into_iter().find(|m| m.is_primary().unwrap_or(false)).unwrap();
        let img = m.capture_image().unwrap();
        println!("Quelle {}x{}", img.width(), img.height());
        for (w, h, q) in [(1280, 720, 60), (1280, 720, 80), (1920, 1080, 70), (1920, 1080, 80), (2560, 1440, 75)] {
            let t = Instant::now();
            let mut size = 0;
            for _ in 0..5 {
                size = encode_jpeg_q(image::DynamicImage::ImageRgba8(img.clone()), w, h, q).unwrap().len();
            }
            println!("STUFE {w}x{h} q{q}: {:.1} ms, {} KB", t.elapsed().as_secs_f64() * 200.0, size / 1024);
        }
    }

    #[test]
    #[ignore]
    fn hw_share_sources_and_window_capture() {
        let list = list_share_sources();
        for e in &list {
            println!(
                "{:?} | {} | {} | thumb={:?}",
                e.source,
                e.title,
                e.subtitle,
                e.thumbnail.as_ref().map(|t| (t.width(), t.height()))
            );
        }
        let mon = list.iter().find(|e| matches!(e.source, ShareSource::Monitor(_))).expect("Monitor");
        let (tx, rx) = std::sync::mpsc::channel();
        let cap = start_screen(mon.source, VideoQuality::High, move |j, s| { let _ = tx.send((j, s)); }, |e| panic!("{e}")).unwrap();
        let (jpeg, sending) = rx.recv_timeout(Duration::from_secs(5)).expect("Screen-Frame");
        assert!(sending);
        assert!(image::load_from_memory(&jpeg).is_ok());
        // Einzelzeiten: Aufnahme vs. Verkleinern+JPEG.
        if let Ok(Target::Monitor(m)) = find_target(mon.source) {
            let t = Instant::now();
            let mut last = None;
            for _ in 0..10 {
                last = m.capture_image().ok();
            }
            let cap_ms = t.elapsed().as_secs_f64() * 100.0;
            let img = last.expect("Capture");
            let t = Instant::now();
            for _ in 0..10 {
                let _ = encode_jpeg_max(
                    image::DynamicImage::ImageRgba8(img.clone()),
                    1920,
                    1080,
                );
            }
            let enc_ms = t.elapsed().as_secs_f64() * 100.0;
            println!("Aufnahme {cap_ms:.1} ms, Kodierung {enc_ms:.1} ms pro Frame");
        }
        // Erreichte Bildrate ueber 3 s (aussagekraeftig nur im Release-Build).
        let t0 = Instant::now();
        let mut n = 0;
        let mut bytes = 0;
        while t0.elapsed() < Duration::from_secs(3) {
            if let Ok((j, _)) = rx.recv_timeout(Duration::from_millis(500)) {
                n += 1;
                bytes += j.len();
            }
        }
        println!(
            "Monitor '{}': {:.1} fps, {:.0} KB/Frame",
            mon.title,
            n as f64 / 3.0,
            bytes as f64 / n.max(1) as f64 / 1024.0
        );
        drop(cap);
        if let Some(win) = list.iter().find(|e| matches!(e.source, ShareSource::Window(_))) {
            let (tx, rx) = std::sync::mpsc::channel();
            let _cap = start_screen(win.source, VideoQuality::High, move |j, s| { let _ = tx.send((j, s)); }, |e| panic!("{e}")).unwrap();
            let (jpeg, _) = rx.recv_timeout(Duration::from_secs(5)).expect("Fenster-Frame");
            let img = image::load_from_memory(&jpeg).unwrap();
            println!("Fenster '{}' -> {}x{}", win.title, img.width(), img.height());
        }
    }

    #[test]
    fn sending_flag_toggles() {
        let cap = VideoCapture {
            stop: Arc::new(AtomicBool::new(false)),
            sending: Arc::new(AtomicBool::new(false)),
        };
        assert!(!cap.is_sending());
        cap.set_sending(true);
        assert!(cap.is_sending());
        let stop = cap.stop.clone();
        drop(cap);
        assert!(stop.load(Ordering::Relaxed), "Drop stoppt den Thread");
    }
}
