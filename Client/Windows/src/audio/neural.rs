//! Natural-sounding speech with the Supertonic 3 model (ONNX Runtime, CPU).
//!
//! Four networks run one after another: the duration predictor says how long
//! the sentence takes, the text encoder turns characters into features, the
//! vector estimator shapes noise into a compressed sound in a few steps, and
//! the vocoder turns that into a waveform. Text goes in as plain characters
//! (wrapped in a language tag) - no phoneme dictionary is needed.

use std::path::Path;

use anyhow::{bail, Context, Result};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;
use unicode_normalization::UnicodeNormalization;

/// Denoising steps: more = cleaner but slower (the model is made for 5-10).
const STEPS: usize = 10;
/// Speaking speed (1.0 = as trained; the model's own default is 1.05).
const SPEED: f32 = 1.05;
/// Changes whenever the settings above change what a clip sounds like
/// (part of the clip cache key).
pub const SETTINGS: &str = "steps10-speed1.05";
/// CPU threads per network (the voice chat itself must keep running smoothly).
const THREADS: usize = 4;

#[derive(serde::Deserialize)]
struct Config {
    ae: AeConfig,
    ttl: TtlConfig,
}
#[derive(serde::Deserialize)]
struct AeConfig {
    sample_rate: u32,
    base_chunk_size: usize,
}
#[derive(serde::Deserialize)]
struct TtlConfig {
    chunk_compress_factor: usize,
    latent_dim: usize,
}

#[derive(serde::Deserialize)]
struct StyleFile {
    style_ttl: StylePart,
    style_dp: StylePart,
}
#[derive(serde::Deserialize)]
struct StylePart {
    data: Vec<Vec<Vec<f32>>>,
    dims: Vec<usize>,
}

/// One preset voice (two embeddings: for the sound and for the timing).
pub struct Style {
    ttl: (Vec<usize>, Vec<f32>),
    dp: (Vec<usize>, Vec<f32>),
}

impl Style {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("Voice file {} missing", path.display()))?;
        let file: StyleFile = serde_json::from_str(&text).context("Voice file is damaged")?;
        let flat = |p: StylePart| -> Result<(Vec<usize>, Vec<f32>)> {
            let data: Vec<f32> = p.data.into_iter().flatten().flatten().collect();
            if p.dims.len() != 3 || p.dims.iter().product::<usize>() != data.len() {
                bail!("Voice file has the wrong shape");
            }
            Ok((p.dims, data))
        };
        Ok(Self { ttl: flat(file.style_ttl)?, dp: flat(file.style_dp)? })
    }
}

pub struct Engine {
    config: Config,
    /// Unicode code point -> token id (-1 = unknown to the model).
    indexer: Vec<i64>,
    duration: Session,
    text_encoder: Session,
    estimator: Session,
    vocoder: Session,
    // Dropped last: the sessions live inside this environment.
    _env: Environment,
}

fn ort_err<T>(e: ort::Error<T>) -> anyhow::Error {
    anyhow::anyhow!("ONNX Runtime: {e}")
}

impl Engine {
    /// Loads the four networks from `dir` (takes about a second).
    pub fn load(dir: &Path) -> Result<Self> {
        let config: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("tts.json")).context("tts.json missing")?)?;
        let indexer: Vec<i64> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("unicode_indexer.json")).context("unicode_indexer.json missing")?)?;
        let env = crate::onnx::env()?;
        let open = |file: &str| -> Result<Session> {
            let builder = Session::builder(&env).map_err(ort_err)?;
            let mut builder = builder.with_intra_threads(THREADS).map_err(ort_err)?;
            builder.commit_from_file(dir.join(file)).map_err(ort_err).with_context(|| file.to_string())
        };
        Ok(Self {
            duration: open("duration_predictor.onnx")?,
            text_encoder: open("text_encoder.onnx")?,
            estimator: open("vector_estimator.onnx")?,
            vocoder: open("vocoder.onnx")?,
            config,
            indexer,
            _env: env,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.ae.sample_rate
    }

    /// Speaks `text` in `lang` ("en", "de", ...). Returns mono samples at
    /// [`Self::sample_rate`]. The same text always gives the same audio.
    pub fn speak(&mut self, text: &str, lang: &str, style: &Style) -> Result<Vec<f32>> {
        let ids = self.tokens(text, lang);
        let n = ids.len();
        if n == 0 {
            bail!("nothing to say");
        }
        let ids = Tensor::from_array(([1, n], ids)).map_err(ort_err)?;
        let text_mask = Tensor::from_array(([1, 1, n], vec![1f32; n])).map_err(ort_err)?;
        let style_dp = Tensor::from_array((style.dp.0.clone(), style.dp.1.clone())).map_err(ort_err)?;
        let style_ttl = Tensor::from_array((style.ttl.0.clone(), style.ttl.1.clone())).map_err(ort_err)?;

        // 1. How long does it take to say this?
        let seconds = {
            let out = self
                .duration
                .run(ort::inputs! { "text_ids" => &ids, "style_dp" => &style_dp, "text_mask" => &text_mask })
                .map_err(ort_err)?;
            let (_, d) = out["duration"].try_extract_tensor::<f32>().map_err(ort_err)?;
            d.first().copied().unwrap_or(0.0) / SPEED
        };
        if !(0.05..=30.0).contains(&seconds) {
            bail!("implausible duration {seconds} s");
        }

        // 2. Characters -> features.
        let (emb_shape, emb) = {
            let out = self
                .text_encoder
                .run(ort::inputs! { "text_ids" => &ids, "style_ttl" => &style_ttl, "text_mask" => &text_mask })
                .map_err(ort_err)?;
            let (shape, data) = out["text_emb"].try_extract_tensor::<f32>().map_err(ort_err)?;
            (shape.iter().map(|d| *d as usize).collect::<Vec<_>>(), data.to_vec())
        };
        let text_emb = Tensor::from_array((emb_shape, emb)).map_err(ort_err)?;

        // 3. Noise -> compressed sound, step by step.
        let rate = self.config.ae.sample_rate as f32;
        let samples = (seconds * rate) as usize;
        let chunk = self.config.ae.base_chunk_size * self.config.ttl.chunk_compress_factor;
        let frames = samples.div_ceil(chunk).max(1);
        let dim = self.config.ttl.latent_dim * self.config.ttl.chunk_compress_factor;
        let mut rng = Gauss::seeded(text, lang);
        let mut latent: Vec<f32> = (0..dim * frames).map(|_| rng.next()).collect();
        let latent_mask = Tensor::from_array(([1, 1, frames], vec![1f32; frames])).map_err(ort_err)?;
        let total = Tensor::from_array(([1], vec![STEPS as f32])).map_err(ort_err)?;
        for step in 0..STEPS {
            let noisy = Tensor::from_array(([1, dim, frames], latent)).map_err(ort_err)?;
            let current = Tensor::from_array(([1], vec![step as f32])).map_err(ort_err)?;
            let out = self
                .estimator
                .run(ort::inputs! {
                    "noisy_latent" => &noisy,
                    "text_emb" => &text_emb,
                    "style_ttl" => &style_ttl,
                    "latent_mask" => &latent_mask,
                    "text_mask" => &text_mask,
                    "current_step" => &current,
                    "total_step" => &total
                })
                .map_err(ort_err)?;
            let (_, data) = out["denoised_latent"].try_extract_tensor::<f32>().map_err(ort_err)?;
            latent = data.to_vec();
        }

        // 4. Compressed sound -> waveform.
        let final_latent = Tensor::from_array(([1, dim, frames], latent)).map_err(ort_err)?;
        let out = self.vocoder.run(ort::inputs! { "latent" => &final_latent }).map_err(ort_err)?;
        let (_, wav) = out["wav_tts"].try_extract_tensor::<f32>().map_err(ort_err)?;
        Ok(wav[..samples.min(wav.len())].to_vec())
    }

    /// Text -> token ids, the way the model was trained: decomposed Unicode
    /// (ä = a + dots), a closing full stop, wrapped in language tags.
    fn tokens(&self, text: &str, lang: &str) -> Vec<i64> {
        let mut text: String = text.nfkd().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return Vec::new();
        }
        if !text.ends_with(['.', '!', '?']) {
            text.push('.');
        }
        format!("<{lang}>{text}</{lang}>").chars().map(|c| self.indexer.get(c as usize).copied().unwrap_or(-1)).collect()
    }
}

/// Small deterministic source of normally distributed numbers (xorshift +
/// Box-Muller), seeded from the text: a name sounds the same every time.
struct Gauss {
    state: u64,
    spare: Option<f32>,
}

impl Gauss {
    fn seeded(text: &str, lang: &str) -> Self {
        // FNV-1a
        let mut h: u64 = 0xcbf29ce484222325;
        for b in text.bytes().chain(lang.bytes()) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        Self { state: h | 1, spare: None }
    }

    /// Uniform in (0, 1].
    fn uniform(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        ((self.state >> 40) as f32 + 1.0) / (1u64 << 24) as f32
    }

    fn next(&mut self) -> f32 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let r = (-2.0 * self.uniform().ln()).sqrt();
        let a = 2.0 * std::f32::consts::PI * self.uniform();
        self.spare = Some(r * a.sin());
        r * a.cos()
    }
}
