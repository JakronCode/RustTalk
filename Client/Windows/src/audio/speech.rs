//! Spoken names for join/leave ("Alex joined"). Two kinds of voices:
//! - the speech voices installed in Windows (WinRT `SpeechSynthesizer`),
//! - the downloadable natural voices (see [`super::voice_pack`], [`super::neural`]).
//!
//! Both work offline. Synthesis runs on its own thread; finished clips are
//! cached per phrase and pushed into the speech lane of the [`SfxQueue`].

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};
use rusttalk_shared::audio_packet::SAMPLE_RATE;
use windows::core::HSTRING;
use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;

use super::resample::Resampler;
use super::sfx::{self, SfxQueue};
use super::{neural, voice_pack};

/// Peak level of a spoken clip (before the output volume).
const LEVEL: f32 = 0.3;
/// Names are cut to this many characters (nobody wants a novel read out).
const MAX_NAME_CHARS: usize = 24;
/// Clips kept in memory (a server's regulars); cleared when full.
const MAX_CACHED: usize = 128;
/// Clips kept on disk for the natural voices (about 200 KB each).
const MAX_DISK_CLIPS: usize = 500;
/// The speech model is unloaded after this long without work.
const MODEL_IDLE: Duration = Duration::from_secs(90);

/// An installed Windows voice.
#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    pub name: String,
    /// BCP-47 tag, e.g. "en-US" or "de-DE".
    pub language: String,
}

/// The installed voices (asked once).
pub fn voices() -> &'static [Voice] {
    static VOICES: OnceLock<Vec<Voice>> = OnceLock::new();
    VOICES.get_or_init(|| {
        let list = || -> windows::core::Result<Vec<Voice>> {
            let mut out = Vec::new();
            for v in SpeechSynthesizer::AllVoices()? {
                out.push(Voice { name: v.DisplayName()?.to_string(), language: v.Language()?.to_string() });
            }
            Ok(out)
        };
        list().unwrap_or_else(|e| {
            tracing::warn!("Speech voices unavailable: {e}");
            Vec::new()
        })
    })
}

struct Job {
    name: String,
    joined: bool,
    /// Voice id from the config: a Windows voice name, "neural:<preset>", or
    /// None for the Windows default voice.
    voice: Option<String>,
    /// Language of the sentence for natural voices ("en" / "de"); Windows
    /// voices speak their own language.
    lang: String,
    /// false = only make the clip now, so it is ready when it is needed.
    play: bool,
}

type ClipKey = (Option<String>, String, String, bool);

/// The loaded speech model with the voices used so far.
struct Neural {
    engine: neural::Engine,
    styles: HashMap<&'static str, neural::Style>,
}

/// Speaks names in the background.
pub struct Announcer {
    tx: mpsc::Sender<Job>,
}

impl Announcer {
    pub fn new(queue: SfxQueue) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("speech".into()).spawn(move || worker(rx, queue)).expect("speech thread");
        Self { tx }
    }

    /// Says "<name> joined" / "<name> left" after the chime. Names without
    /// anything pronounceable are skipped.
    pub fn announce(&self, name: &str, joined: bool, voice: Option<&str>, lang: &str) {
        self.send(name, joined, voice, lang, true);
    }

    /// Makes both clips for `name` ahead of time. Only natural voices need
    /// this (they take about a second per new name; Windows voices are instant).
    pub fn prepare(&self, name: &str, voice: Option<&str>, lang: &str) {
        if voice.is_some_and(|v| voice_pack::neural_id(v).is_some()) {
            self.send(name, true, voice, lang, false);
            self.send(name, false, voice, lang, false);
        }
    }

    fn send(&self, name: &str, joined: bool, voice: Option<&str>, lang: &str, play: bool) {
        let name = speakable(name);
        if !name.is_empty() {
            let _ = self.tx.send(Job { name, joined, voice: voice.map(str::to_owned), lang: lang.to_owned(), play });
        }
    }
}

fn worker(rx: mpsc::Receiver<Job>, queue: SfxQueue) {
    let mut cache: HashMap<ClipKey, Arc<Vec<f32>>> = HashMap::new();
    let mut pending: VecDeque<Job> = VecDeque::new();
    let mut model: Option<Neural> = None;
    loop {
        if pending.is_empty() {
            // The model needs a few hundred MB of memory: let it go when idle.
            let job = if model.is_some() {
                match rx.recv_timeout(MODEL_IDLE) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        model = None;
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            };
            pending.push_back(job);
        }
        pending.extend(rx.try_iter());
        // Something that should be heard now goes before clips made in advance.
        let next = pending.iter().position(|j| j.play).unwrap_or(0);
        let Some(job) = pending.remove(next) else { continue };

        let key = (job.voice.clone(), job.lang.clone(), job.name.clone(), job.joined);
        let clip = match cache.get(&key) {
            Some(c) => c.clone(),
            None => match make_clip(&job, &mut model) {
                Ok(c) => {
                    if cache.len() >= MAX_CACHED {
                        cache.clear();
                    }
                    let c = Arc::new(c);
                    cache.insert(key, c.clone());
                    c
                }
                Err(e) => {
                    tracing::warn!("Speech failed: {e:#}");
                    continue;
                }
            },
        };
        if job.play {
            sfx::play_speech(&queue, &clip);
        }
    }
}

/// Makes the clip for a job: with the natural voice if it is chosen and
/// installed, otherwise with a Windows voice.
fn make_clip(job: &Job, model: &mut Option<Neural>) -> anyhow::Result<Vec<f32>> {
    let preset = job.voice.as_deref().and_then(voice_pack::neural_id);
    let dir = voice_pack::dir();
    match preset {
        Some(preset) if voice_pack::installed_in(&dir) => {
            neural_clip(model, &dir, &voice_pack::cache_dir(), preset, &job.lang, &phrase(&job.name, job.joined, &job.lang))
        }
        // Natural voice chosen but the pack is gone: Windows default voice.
        Some(_) => windows_clip(&job.name, job.joined, None),
        None => windows_clip(&job.name, job.joined, job.voice.as_deref()),
    }
}

/// A clip from the natural voice. Finished clips are kept on disk, so the
/// model is only loaded for names that were never spoken before.
fn neural_clip(
    model: &mut Option<Neural>,
    dir: &std::path::Path,
    cache_dir: &std::path::Path,
    preset: &'static str,
    lang: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let id = format!("{}|{}|{preset}|{lang}|{text}", voice_pack::REVISION, neural::SETTINGS);
    let hash: String = Sha256::digest(id.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    let file = cache_dir.join(format!("{hash}.f32"));
    if let Ok(bytes) = std::fs::read(&file) {
        if !bytes.is_empty() && bytes.len() % 4 == 0 {
            return Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect());
        }
    }
    if model.is_none() {
        *model = Some(Neural { engine: neural::Engine::load(dir)?, styles: HashMap::new() });
    }
    let m = model.as_mut().expect("just loaded");
    if !m.styles.contains_key(preset) {
        m.styles.insert(preset, neural::Style::load(&dir.join(format!("{preset}.json")))?);
    }
    let pcm = m.engine.speak(text, lang, &m.styles[preset])?;
    let clip = polish(m.engine.sample_rate(), &pcm);
    anyhow::ensure!(!clip.is_empty(), "the voice stayed silent");

    // Best effort: without the file the clip is simply made again next time.
    if std::fs::create_dir_all(cache_dir).is_ok() {
        if std::fs::read_dir(cache_dir).is_ok_and(|d| d.count() >= MAX_DISK_CLIPS) {
            let _ = std::fs::remove_dir_all(cache_dir);
            let _ = std::fs::create_dir_all(cache_dir);
        }
        let bytes: Vec<u8> = clip.iter().flat_map(|s| s.to_le_bytes()).collect();
        let _ = std::fs::write(&file, bytes);
    }
    Ok(clip)
}

/// Reduces a username to what a voice can read: letters and digits, with
/// everything else ("xX_Dark-Lord_Xx", emoji) turned into word breaks.
pub fn speakable(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(MAX_NAME_CHARS).collect::<String>().trim_end().to_string()
}

/// The sentence in the voice's own language (an English voice reading German
/// words sounds wrong, and the other way round).
fn phrase(name: &str, joined: bool, language: &str) -> String {
    let german = language.to_ascii_lowercase().starts_with("de");
    match (german, joined) {
        (false, true) => format!("{name} joined"),
        (false, false) => format!("{name} left"),
        (true, true) => format!("{name} ist beigetreten"),
        (true, false) => format!("{name} ist gegangen"),
    }
}

/// A clip from a Windows voice (None = the default voice).
fn windows_clip(name: &str, joined: bool, voice: Option<&str>) -> anyhow::Result<Vec<f32>> {
    let synth = SpeechSynthesizer::new()?;
    if let Some(wanted) = voice {
        let found: Option<VoiceInformation> =
            SpeechSynthesizer::AllVoices()?.into_iter().find(|v| v.DisplayName().is_ok_and(|n| n.to_string() == *wanted));
        // A voice that was uninstalled meanwhile: fall back to the default.
        if let Some(v) = found {
            synth.SetVoice(&v)?;
        }
    }
    let language = synth.Voice()?.Language()?.to_string();
    let text = phrase(name, joined, &language);

    let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(text))?.join()?;
    let size = stream.Size()? as u32;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.join()?;
    let mut wav = vec![0u8; size as usize];
    reader.ReadBytes(&mut wav)?;

    let (rate, pcm) = parse_wav(&wav)?;
    Ok(polish(rate, &pcm))
}

/// Reads a PCM-16 WAV file: (sample rate, first channel as -1..1).
fn parse_wav(wav: &[u8]) -> anyhow::Result<(u32, Vec<f32>)> {
    anyhow::ensure!(wav.len() >= 12 && &wav[..4] == b"RIFF" && &wav[8..12] == b"WAVE", "not a WAV file");
    let mut fmt: Option<(u16, u32)> = None; // (channels, rate)
    let mut pos = 12;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let len = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &wav[pos + 8..(pos + 8 + len).min(wav.len())];
        if id == b"fmt " {
            anyhow::ensure!(body.len() >= 16, "short fmt chunk");
            let tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes([body[14], body[15]]);
            anyhow::ensure!(tag == 1 && bits == 16 && channels >= 1 && rate > 0, "unsupported WAV format (tag {tag}, {bits} bit)");
            fmt = Some((channels, rate));
        } else if id == b"data" {
            let (channels, rate) = fmt.ok_or_else(|| anyhow::anyhow!("WAV data before fmt"))?;
            let pcm = body
                .chunks_exact(2 * channels as usize)
                .map(|f| i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0)
                .collect();
            return Ok((rate, pcm));
        }
        pos += 8 + len + (len & 1); // chunks are word-aligned
    }
    anyhow::bail!("WAV without data")
}

/// Makes a raw clip ready for the mix: 48 kHz, silence trimmed off both ends,
/// a steady level and short fades.
fn polish(rate: u32, pcm: &[f32]) -> Vec<f32> {
    // Resample (plus a little silence so the filter's tail comes out too).
    let mut input = pcm.to_vec();
    input.extend(std::iter::repeat(0.0).take(rate as usize / 20));
    let mut out = Vec::with_capacity(input.len() * SAMPLE_RATE as usize / rate as usize + 64);
    Resampler::to_internal(rate).process(&input, &mut out);

    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak < 1e-4 {
        return Vec::new();
    }
    // Trim what is quieter than -40 dB of the peak, keeping 15 ms around it.
    let gate = peak * 0.01;
    let margin = SAMPLE_RATE as usize * 15 / 1000;
    let first = out.iter().position(|x| x.abs() > gate).unwrap_or(0).saturating_sub(margin);
    let last = (out.iter().rposition(|x| x.abs() > gate).unwrap_or(out.len() - 1) + margin).min(out.len() - 1);
    let mut clip = out[first..=last].to_vec();

    let gain = LEVEL / peak;
    let fade = (SAMPLE_RATE as usize * 8 / 1000).min(clip.len() / 2);
    let n = clip.len();
    for (i, s) in clip.iter_mut().enumerate() {
        let edge = i.min(n - 1 - i);
        let env = if edge < fade { edge as f32 / fade as f32 } else { 1.0 };
        *s *= gain * env;
    }
    clip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_speakable() {
        assert_eq!(speakable("Alex"), "Alex");
        assert_eq!(speakable("xX_Dark-Lord_Xx"), "xX Dark Lord Xx");
        assert_eq!(speakable("  Jörg.42 🎧 "), "Jörg 42");
        assert_eq!(speakable("___"), "");
        assert_eq!(speakable(&"a".repeat(100)).chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn phrase_follows_the_voice_language() {
        assert_eq!(phrase("Alex", true, "en-US"), "Alex joined");
        assert_eq!(phrase("Alex", false, "en-GB"), "Alex left");
        assert_eq!(phrase("Alex", true, "de-DE"), "Alex ist beigetreten");
        assert_eq!(phrase("Alex", false, "de-AT"), "Alex ist gegangen");
    }

    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"RIFF\0\0\0\0WAVEfmt ");
        b.extend(18u32.to_le_bytes()); // fmt with the optional cbSize field
        b.extend(1u16.to_le_bytes());
        b.extend(channels.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * 2 * channels as u32).to_le_bytes());
        b.extend((2 * channels).to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend(b"LIST");
        b.extend(3u32.to_le_bytes()); // odd length -> one pad byte
        b.extend([1, 2, 3, 0]);
        b.extend(b"data");
        b.extend((samples.len() as u32 * 2).to_le_bytes());
        for s in samples {
            b.extend(s.to_le_bytes());
        }
        b
    }

    #[test]
    fn wav_is_parsed() {
        let (rate, pcm) = parse_wav(&wav(16000, 1, &[0, 16384, -32768])).unwrap();
        assert_eq!(rate, 16000);
        assert_eq!(pcm, vec![0.0, 0.5, -1.0]);
        // Stereo: first channel only.
        let (_, pcm) = parse_wav(&wav(22050, 2, &[16384, 0, -16384, 0])).unwrap();
        assert_eq!(pcm, vec![0.5, -0.5]);
        assert!(parse_wav(b"nonsense").is_err());
        assert!(parse_wav(b"RIFF\0\0\0\0WAVE").is_err());
    }

    #[test]
    fn clip_is_trimmed_levelled_and_faded() {
        // 0.5 s silence, 0.25 s of a quiet 300 Hz tone, 0.5 s silence at 16 kHz.
        let rate = 16000;
        let mut pcm = vec![0.0f32; 8000];
        pcm.extend((0..4000).map(|i| 0.05 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / rate as f32).sin()));
        pcm.extend(vec![0.0f32; 8000]);
        let clip = polish(rate, &pcm);
        let ms = clip.len() as f32 / SAMPLE_RATE as f32 * 1000.0;
        assert!((250.0..=300.0).contains(&ms), "trimmed to the tone: {ms} ms");
        let peak = clip.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - LEVEL).abs() < 0.02, "levelled: {peak}");
        assert!(clip[0].abs() < 1e-3 && clip[clip.len() - 1].abs() < 1e-3, "faded");
        assert!(polish(rate, &vec![0.0; 1000]).is_empty(), "pure silence -> nothing");
    }

    /// Listening test with the real Windows voice:
    /// `cargo test -p rusttalk-client dump_speech -- --ignored`
    /// writes `target/sfx-preview/join_<voice>.wav` (chime + name) per voice.
    #[test]
    #[ignore]
    fn dump_speech() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sfx-preview");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!voices().is_empty(), "no Windows voice installed");
        for v in voices() {
            for (joined, sound) in [(true, sfx::Sound::Join), (false, sfx::Sound::Leave)] {
                let clip = windows_clip("Phil", joined, Some(&v.name)).unwrap();
                assert!(clip.len() > SAMPLE_RATE as usize / 5, "{}: {} samples", v.name, clip.len());
                // Mixed exactly like the output callback does it.
                let q: SfxQueue = Default::default();
                sfx::play(&q, sound);
                sfx::play_speech(&q, &clip);
                let mut mix = vec![0.0f32; clip.len() + SAMPLE_RATE as usize / 2];
                sfx::mix_into(&q, &mut mix);
                let file = format!("{}_{}.wav", if joined { "join" } else { "leave" }, v.name.replace(' ', "_")).to_lowercase();
                sfx::write_wav(&dir.join(file), &mix);
            }
        }
    }

    /// Listening test with the natural voices. Needs the voice pack in the
    /// folder named by `RUSTTALK_TEST_PACK_DIR`:
    /// `cargo test -p rusttalk-client dump_neural -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_neural() {
        let pack = std::path::PathBuf::from(std::env::var("RUSTTALK_TEST_PACK_DIR").expect("RUSTTALK_TEST_PACK_DIR"));
        assert!(voice_pack::installed_in(&pack), "voice pack incomplete in {}", pack.display());
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sfx-preview");
        let cache = out.join("clip-cache");
        let _ = std::fs::remove_dir_all(&cache);
        std::fs::create_dir_all(&out).unwrap();
        let mut model = None;
        for (preset, lang, name) in [("F1", "en", "Phil"), ("M1", "en", "Phil"), ("F2", "de", "Phil"), ("M2", "de", "Jörg"), ("M3", "en", "xX Dark Lord Xx")] {
            for (joined, sound) in [(true, sfx::Sound::Join), (false, sfx::Sound::Leave)] {
                let text = phrase(name, joined, lang);
                let t = std::time::Instant::now();
                let clip = neural_clip(&mut model, &pack, &cache, preset, lang, &text).unwrap();
                let made = t.elapsed();
                let secs = clip.len() as f32 / SAMPLE_RATE as f32;
                assert!((0.3..4.0).contains(&secs), "{text}: {secs} s");
                let peak = clip.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                assert!((peak - LEVEL).abs() < 0.02, "{text}: peak {peak}");
                // Second time: straight from the disk cache, identical, no model.
                let t = std::time::Instant::now();
                let again = neural_clip(&mut None, &pack, &cache, preset, lang, &text).unwrap();
                assert_eq!(clip, again);
                println!("{preset} {lang} {text:?}: {secs:.2} s audio, made in {made:?}, cached in {:?}", t.elapsed());

                let q: SfxQueue = Default::default();
                sfx::play(&q, sound);
                sfx::play_speech(&q, &clip);
                let mut mix = vec![0.0f32; clip.len() + SAMPLE_RATE as usize / 2];
                sfx::mix_into(&q, &mut mix);
                let file = format!("{}_natural_{preset}_{lang}_{}.wav", if joined { "join" } else { "leave" }, name.replace(' ', "_")).to_lowercase();
                sfx::write_wav(&out.join(file), &mix);
            }
        }
    }
}
