//! "Remove background" for the presenter's camera: person segmentation with
//! MediaPipe Selfie Segmentation (landscape, Apache-2.0, see
//! `assets/README.md`) on ONNX Runtime, CPU, one thread. Runs only on this
//! client, on pictures that are already decoded; nothing is sent.
//!
//! The model runs in its own thread: the decoder hands it a small copy of a
//! frame now and then and always applies the newest finished mask, so video
//! never waits for the model.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use eframe::egui;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::{Tensor, ValueType};
use sha2::{Digest, Sha256};

static MODEL: &[u8] = include_bytes!("../assets/selfie_segmentation_landscape.onnx");
const MODEL_SHA256: &str = "7a0adcfdb1715d3b0ff0f61486d8a181c33f4a208343a5abfccbc6540e872d24";
/// Model input size.
const W: usize = 256;
const H: usize = 144;
/// New masks at most this often; frames in between reuse the last one.
const MIN_INTERVAL: Duration = Duration::from_millis(50);
/// Weight of the newest mask against the previous one (less flicker).
const SMOOTH: f32 = 0.7;
/// Mask values between these fade from transparent to opaque (soft edge).
const EDGE: (f32, f32) = (0.3, 0.7);
/// Steps of the mask -> alpha table.
const LUT: usize = 1024;

fn ort_err<T>(e: ort::Error<T>) -> anyhow::Error {
    anyhow::anyhow!("ONNX Runtime: {e}")
}

/// The model itself (runs synchronously; used by the [`Cutout`] worker).
struct Segmenter {
    session: Session,
    input: String,
    output: String,
    channels_first: bool,
    // Dropped last: the session lives inside this environment.
    _env: Environment,
}

impl Segmenter {
    /// Loads the embedded model after checking its hash (fails closed).
    fn new() -> Result<Self> {
        if hex(&Sha256::digest(MODEL)) != MODEL_SHA256 {
            bail!("segmentation model checksum mismatch");
        }
        let env = crate::onnx::env()?;
        let mut builder = Session::builder(&env)
            .map_err(ort_err)?
            .with_intra_threads(1)
            .map_err(ort_err)?;
        let session = builder
            .commit_from_memory(MODEL)
            .map_err(ort_err)
            .context("segmentation model")?;
        let first = session.inputs().first().context("model has no input")?;
        let shape: Vec<i64> = match first.dtype() {
            ValueType::Tensor { shape, .. } => shape.iter().copied().collect(),
            _ => bail!("unexpected model input"),
        };
        let channels_first = match shape.as_slice() {
            [_, 3, _, _] => true,
            [_, _, _, 3] => false,
            other => bail!("unexpected model input shape {other:?}"),
        };
        let output = session
            .outputs()
            .first()
            .context("model has no output")?
            .name()
            .to_string();
        Ok(Self {
            input: first.name().to_string(),
            output,
            session,
            channels_first,
            _env: env,
        })
    }

    /// One model run on a [`downscale`]d picture: H x W person probabilities (0..1).
    fn infer(&mut self, hwc: &[f32]) -> Result<Vec<f32>> {
        if hwc.len() != 3 * W * H {
            bail!("wrong input size");
        }
        let (shape, input): ([usize; 4], Vec<f32>) = if self.channels_first {
            let mut chw = vec![0.0; hwc.len()];
            for (j, v) in hwc.iter().enumerate() {
                chw[(j % 3) * W * H + j / 3] = *v;
            }
            ([1, 3, H, W], chw)
        } else {
            ([1, H, W, 3], hwc.to_vec())
        };
        let tensor = Tensor::from_array((shape, input)).map_err(ort_err)?;
        let out = self
            .session
            .run(ort::inputs! { self.input.as_str() => &tensor })
            .map_err(ort_err)?;
        let (_, data) = out[self.output.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(ort_err)?;
        if data.len() != W * H {
            bail!("unexpected mask size {}", data.len());
        }
        Ok(data
            .iter()
            .map(|v| {
                if v.is_finite() {
                    v.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect())
    }
}

/// Background removal for one camera stream, owned by its decoder thread.
/// Dropping it ends the model thread.
pub struct Cutout {
    to_model: SyncSender<Vec<f32>>,
    mask: Arc<Mutex<Option<Arc<Vec<f32>>>>>,
    failed: Arc<AtomicBool>,
    last_sent: Option<Instant>,
    alpha: Box<[u8; LUT + 1]>,
}

impl Cutout {
    pub fn start() -> Self {
        let (to_model, frames) = sync_channel::<Vec<f32>>(1);
        let mask = Arc::new(Mutex::new(None::<Arc<Vec<f32>>>));
        let failed = Arc::new(AtomicBool::new(false));
        let (out, fail) = (mask.clone(), failed.clone());
        let spawned = std::thread::Builder::new()
            .name("segment".into())
            .spawn(move || {
                let mut seg = match Segmenter::new() {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("background removal unavailable: {e:#}");
                        return fail.store(true, Ordering::Relaxed);
                    }
                };
                let mut smooth: Vec<f32> = Vec::new();
                for input in frames {
                    match seg.infer(&input) {
                        Ok(new) => {
                            if smooth.len() == new.len() {
                                for (m, n) in smooth.iter_mut().zip(new) {
                                    *m = SMOOTH * n + (1.0 - SMOOTH) * *m;
                                }
                            } else {
                                smooth = new;
                            }
                            *out.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(Arc::new(smooth.clone()));
                        }
                        Err(e) => tracing::debug!("{e:#}"),
                    }
                }
            });
        if spawned.is_err() {
            failed.store(true, Ordering::Relaxed);
        }
        Self {
            to_model,
            mask,
            failed,
            last_sent: None,
            alpha: alpha_table(),
        }
    }

    /// Makes everything but the person in `img` transparent (once the first
    /// mask is there; until then the picture stays as it is).
    pub fn apply(&mut self, img: &mut egui::ColorImage) {
        let [w, h] = img.size;
        if w == 0 || h == 0 || img.pixels.len() != w * h || self.failed.load(Ordering::Relaxed) {
            return;
        }
        if self.last_sent.is_none_or(|t| t.elapsed() >= MIN_INTERVAL) {
            match self.to_model.try_send(downscale(img)) {
                // Busy: this frame is skipped, the next one tries again.
                Ok(()) | Err(TrySendError::Full(_)) => self.last_sent = Some(Instant::now()),
                Err(TrySendError::Disconnected(_)) => {
                    return self.failed.store(true, Ordering::Relaxed)
                }
            }
        }
        let mask = self.mask.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(mask) = mask {
            apply_mask(img, &mask, &self.alpha);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Mask value (0..1 in `LUT` steps) -> alpha with a smooth edge.
fn alpha_table() -> Box<[u8; LUT + 1]> {
    let mut t = Box::new([0u8; LUT + 1]);
    for (i, a) in t.iter_mut().enumerate() {
        let m = i as f32 / LUT as f32;
        let x = ((m - EDGE.0) / (EDGE.1 - EDGE.0)).clamp(0.0, 1.0);
        *a = (x * x * (3.0 - 2.0 * x) * 255.0).round() as u8;
    }
    t
}

/// Box-filtered W x H copy of `img`, interleaved RGB floats in 0..1 (the
/// model's rescale).
fn downscale(img: &egui::ColorImage) -> Vec<f32> {
    let [w, h] = img.size;
    let mut out = vec![0.0f32; 3 * W * H];
    for ty in 0..H {
        let y0 = ty * h / H;
        let y1 = ((ty + 1) * h / H).max(y0 + 1).min(h);
        for tx in 0..W {
            let x0 = tx * w / W;
            let x1 = ((tx + 1) * w / W).max(x0 + 1).min(w);
            let mut sum = [0u32; 3];
            for y in y0..y1 {
                for p in &img.pixels[y * w + x0..y * w + x1] {
                    sum[0] += p.r() as u32;
                    sum[1] += p.g() as u32;
                    sum[2] += p.b() as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as f32 * 255.0;
            let i = (ty * W + tx) * 3;
            for c in 0..3 {
                out[i + c] = sum[c] as f32 / n;
            }
        }
    }
    out
}

/// Bilinear upscale of the mask to the picture, soft edge, into alpha.
fn apply_mask(img: &mut egui::ColorImage, mask: &[f32], alpha: &[u8; LUT + 1]) {
    let [w, h] = img.size;
    if mask.len() != W * H || img.pixels.len() != w * h {
        return;
    }
    let sample = |n: usize, size: usize, i: usize| {
        let f = ((i as f32 + 0.5) * n as f32 / size as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let i0 = f as usize;
        (i0, (i0 + 1).min(n - 1), f - i0 as f32)
    };
    let cols: Vec<(usize, usize, f32)> = (0..w).map(|x| sample(W, w, x)).collect();
    for y in 0..h {
        let (y0, y1, dy) = sample(H, h, y);
        let (r0, r1) = (&mask[y0 * W..y0 * W + W], &mask[y1 * W..y1 * W + W]);
        let row = &mut img.pixels[y * w..y * w + w];
        for (p, &(x0, x1, dx)) in row.iter_mut().zip(&cols) {
            let top = r0[x0] + (r0[x1] - r0[x0]) * dx;
            let bottom = r1[x0] + (r1[x1] - r1[x0]) * dx;
            let m = top + (bottom - top) * dy;
            let a = alpha[((m * LUT as f32) as usize).min(LUT)];
            match a {
                255 => {}
                0 => *p = egui::Color32::TRANSPARENT,
                a => *p = egui::Color32::from_rgba_unmultiplied(p.r(), p.g(), p.b(), a),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light room, a skin-coloured head and a dark body.
    fn person(w: usize, h: usize) -> egui::ColorImage {
        let mut img = egui::ColorImage::new([w, h], egui::Color32::from_rgb(230, 230, 225));
        let (sx, sy) = (w as f32 / 640.0, h as f32 / 360.0);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 / sx, y as f32 / sy);
                let head = ((fx - 320.0) / 55.0).powi(2) + ((fy - 120.0) / 65.0).powi(2) <= 1.0;
                let body = ((fx - 320.0) / 160.0).powi(2) + ((fy - 380.0) / 170.0).powi(2) <= 1.0;
                if head {
                    img.pixels[y * w + x] = egui::Color32::from_rgb(224, 172, 138);
                } else if body {
                    img.pixels[y * w + x] = egui::Color32::from_rgb(40, 60, 90);
                }
            }
        }
        img
    }

    #[test]
    fn embedded_model_matches_its_pin() {
        assert_eq!(MODEL.len(), 462_338);
        assert_eq!(hex(&Sha256::digest(MODEL)), MODEL_SHA256);
    }

    #[test]
    fn mask_sets_alpha_and_keeps_color() {
        let table = alpha_table();
        let mut img = egui::ColorImage::new([64, 36], egui::Color32::from_rgb(200, 100, 50));
        let mut mask = vec![0.0f32; W * H];
        for y in 0..H {
            for x in W / 2..W {
                mask[y * W + x] = 1.0;
            }
        }
        apply_mask(&mut img, &mask, &table);
        let left = img.pixels[18 * 64 + 2];
        let right = img.pixels[18 * 64 + 61];
        assert_eq!(left.a(), 0);
        assert_eq!(right, egui::Color32::from_rgb(200, 100, 50));
        // Wrong mask size: picture untouched.
        let mut img2 = egui::ColorImage::new([4, 4], egui::Color32::WHITE);
        apply_mask(&mut img2, &[0.0; 3], &table);
        assert!(img2.pixels.iter().all(|p| p.a() == 255));
        // Soft edge: monotonic, 0 and 255 at the ends.
        assert_eq!((table[0], table[LUT]), (0, 255));
        assert!(table.windows(2).all(|p| p[0] <= p[1]));
    }

    #[test]
    fn downscale_averages_into_the_model_layout() {
        let img = egui::ColorImage::new([512, 288], egui::Color32::from_rgb(255, 0, 51));
        let hwc = downscale(&img);
        assert_eq!(hwc.len(), 3 * W * H);
        assert!((hwc[0] - 1.0).abs() < 1e-6 && hwc[1] == 0.0 && (hwc[2] - 0.2).abs() < 1e-6);
        // Pictures smaller than the model input still fill every cell.
        let tiny = egui::ColorImage::new([3, 2], egui::Color32::WHITE);
        assert!(downscale(&tiny).iter().all(|v| (*v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn model_runs_and_cutout_never_blocks_the_frame() {
        let mut seg = Segmenter::new().expect("model loads");
        let input = downscale(&person(640, 360));
        let _ = seg.infer(&input).expect("warm-up");
        let start = Instant::now();
        let runs = 10;
        for _ in 0..runs {
            let mask = seg.infer(&input).expect("inference");
            assert_eq!(mask.len(), W * H);
            assert!(mask.iter().all(|v| (0.0..=1.0).contains(v)));
        }
        let model = start.elapsed() / runs;

        // Per frame on the decoder thread: downscale + mask on a 720p picture.
        let mut cut = Cutout::start();
        let frame = person(1280, 720);
        let deadline = Instant::now() + Duration::from_secs(10);
        while cut.mask.lock().unwrap().is_none() && Instant::now() < deadline {
            let mut img = frame.clone();
            cut.apply(&mut img);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(cut.mask.lock().unwrap().is_some(), "worker produced a mask");
        let start = Instant::now();
        for _ in 0..runs {
            let mut img = frame.clone();
            cut.apply(&mut img);
        }
        let per_frame = start.elapsed() / runs;
        eprintln!("segmentation: model {model:?} per run (own thread), decoder thread {per_frame:?} per 720p frame");
    }
}
