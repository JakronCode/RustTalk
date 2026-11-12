//! The downloadable "natural voices" pack: the Supertonic 3 speech model
//! (about 400 MB, from Hugging Face, pinned to one revision and checked by
//! SHA-256). The installed app downloads it on its own (can be turned off).
//!
//! The model is by Supertone Inc. under the BigScience OpenRAIL-M license; its
//! LICENSE (with the use restrictions) is downloaded into the pack folder too.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

/// Voice ids in the config start with this ("neural:F1").
pub const PREFIX: &str = "neural:";

/// The pack's preset voices: (id, label).
pub const VOICES: [(&str, &str); 10] = [
    ("F1", "Natural - female 1"),
    ("F2", "Natural - female 2"),
    ("F3", "Natural - female 3"),
    ("F4", "Natural - female 4"),
    ("F5", "Natural - female 5"),
    ("M1", "Natural - male 1"),
    ("M2", "Natural - male 2"),
    ("M3", "Natural - male 3"),
    ("M4", "Natural - male 4"),
    ("M5", "Natural - male 5"),
];

/// "neural:F1" -> "F1" (only for voices the pack really has).
pub fn neural_id(voice: &str) -> Option<&'static str> {
    let id = voice.strip_prefix(PREFIX)?;
    VOICES.iter().find(|(v, _)| *v == id).map(|(v, _)| *v)
}

/// Label for a voice id from the config.
pub fn label(voice: &str) -> String {
    match voice.strip_prefix(PREFIX) {
        Some(id) => VOICES.iter().find(|(v, _)| *v == id).map(|(_, l)| l.to_string()).unwrap_or_else(|| voice.to_string()),
        None => voice.to_string(),
    }
}

/// Model revision on Hugging Face (part of the clip cache key too).
pub const REVISION: &str = "3cadd1ee6394adea1bd021217a0e650ede09a323";
const BASE_URL: &str = "https://huggingface.co/Supertone/supertonic-3/resolve";
/// The model's license (OpenRAIL-M, use restrictions in Attachment A).
pub const LICENSE_URL: &str = "https://huggingface.co/Supertone/supertonic-3/blob/main/LICENSE";

struct PackFile {
    /// Path in the model repository.
    remote: &'static str,
    size: u64,
    sha256: &'static str,
}

impl PackFile {
    /// Local file name (the pack is one flat folder).
    fn name(&self) -> &'static str {
        self.remote.rsplit('/').next().unwrap_or(self.remote)
    }
}

const fn file(remote: &'static str, size: u64, sha256: &'static str) -> PackFile {
    PackFile { remote, size, sha256 }
}

const FILES: [PackFile; 17] = [
    file("onnx/tts.json", 8_253, "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09"),
    file("onnx/unicode_indexer.json", 277_676, "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f"),
    file("onnx/duration_predictor.onnx", 3_700_147, "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db"),
    file("onnx/text_encoder.onnx", 36_416_150, "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff"),
    file("onnx/vocoder.onnx", 101_424_195, "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba"),
    file("onnx/vector_estimator.onnx", 256_534_781, "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c"),
    file("voice_styles/F1.json", 292_046, "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2"),
    file("voice_styles/F2.json", 292_423, "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6"),
    file("voice_styles/F3.json", 290_794, "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab"),
    file("voice_styles/F4.json", 291_808, "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b"),
    file("voice_styles/F5.json", 291_479, "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d"),
    file("voice_styles/M1.json", 291_748, "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b"),
    file("voice_styles/M2.json", 292_055, "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50"),
    file("voice_styles/M3.json", 290_198, "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b"),
    file("voice_styles/M4.json", 291_522, "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303"),
    file("voice_styles/M5.json", 291_469, "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2"),
    file("LICENSE", 15_007, "0d944a9110fed9a9602d60e0423a272903e7bd21ab060490774efc77c2275e9f"),
];

/// Download size of the whole pack in bytes.
pub fn total_size() -> u64 {
    FILES.iter().map(|f| f.size).sum()
}

/// Folder for everything speech-related: `%APPDATA%\RustTalk\voices`
/// (also for the portable exe - 400 MB don't belong next to it).
fn voices_dir() -> PathBuf {
    crate::install::data_dir().unwrap_or_default().join("voices")
}

/// Default voice after the automatic download.
pub const DEFAULT_VOICE: &str = "neural:F1";

/// Language natural voices speak for a Windows language tag ("de-DE" -> "de").
pub fn language_for(tag: &str) -> &'static str {
    if tag.trim().to_ascii_lowercase().starts_with("de") {
        "de"
    } else {
        "en"
    }
}

/// "de" when Windows' first display language is German, else "en".
#[cfg(windows)]
pub fn system_language() -> &'static str {
    windows::System::UserProfile::GlobalizationPreferences::Languages()
        .ok()
        .and_then(|list| list.GetAt(0).ok())
        .map(|tag| language_for(&tag.to_string()))
        .unwrap_or("en")
}

#[cfg(not(windows))]
pub fn system_language() -> &'static str {
    ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .map(|tag| language_for(&tag))
        .unwrap_or("en")
}

/// Where the model lives.
pub fn dir() -> PathBuf {
    voices_dir().join("supertonic-3")
}

/// Where finished clips are kept (so a known name needs no model at all).
pub fn cache_dir() -> PathBuf {
    voices_dir().join("cache")
}

fn complete(dir: &Path, f: &PackFile) -> bool {
    std::fs::metadata(dir.join(f.name())).is_ok_and(|m| m.len() == f.size)
}

/// All files are there with the right size (contents were checked on download).
pub fn installed_in(dir: &Path) -> bool {
    FILES.iter().all(|f| complete(dir, f))
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Missing,
    Downloading { done: u64, total: u64 },
    Failed(String),
    Installed,
}

/// The pack's state, shared between the settings page and the download thread.
#[derive(Clone)]
pub struct Pack {
    state: Arc<Mutex<State>>,
}

impl Pack {
    pub fn new() -> Self {
        let state = if installed_in(&dir()) { State::Installed } else { State::Missing };
        Self { state: Arc::new(Mutex::new(state)) }
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Starts the download in the background (no-op while one is running).
    pub fn download(&self) {
        {
            let mut s = self.state.lock().unwrap();
            if matches!(*s, State::Downloading { .. } | State::Installed) {
                return;
            }
            *s = State::Downloading { done: 0, total: total_size() };
        }
        let state = self.state.clone();
        std::thread::Builder::new()
            .name("voice-pack".into())
            .spawn(move || {
                let result = download_all(&dir(), BASE_URL, &|done| {
                    *state.lock().unwrap() = State::Downloading { done, total: total_size() };
                });
                *state.lock().unwrap() = match result {
                    Ok(()) => State::Installed,
                    Err(e) => {
                        tracing::warn!("Voice download failed: {e:#}");
                        State::Failed(format!("{e:#}"))
                    }
                };
            })
            .expect("voice-pack thread");
    }

    /// Deletes the model and all cached clips.
    pub fn remove(&self) {
        let mut s = self.state.lock().unwrap();
        if matches!(*s, State::Downloading { .. }) {
            return;
        }
        let _ = std::fs::remove_dir_all(dir());
        let _ = std::fs::remove_dir_all(cache_dir());
        *s = State::Missing;
    }
}

/// Downloads every missing file into `dir`. Files that are already complete
/// are kept, so a broken download continues with the file it stopped at.
fn download_all(dir: &Path, base_url: &str, progress: &dyn Fn(u64)) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(60)).build();
    let mut done: u64 = FILES.iter().filter(|f| complete(dir, f)).map(|f| f.size).sum();
    progress(done);
    for f in FILES.iter().filter(|f| !complete(dir, f)) {
        let url = format!("{base_url}/{REVISION}/{}", f.remote);
        let part = dir.join(format!("{}.part", f.name()));
        let before = done;
        let result = download_file(&agent, &url, &part, f, &mut |n| {
            done += n;
            progress(done);
        });
        if let Err(e) = result {
            let _ = std::fs::remove_file(&part);
            progress(before);
            return Err(e.context(format!("Download of {} failed", f.name())));
        }
        std::fs::rename(&part, dir.join(f.name())).with_context(|| format!("Could not save {}", f.name()))?;
    }
    Ok(())
}

fn download_file(agent: &ureq::Agent, url: &str, part: &Path, f: &PackFile, progress: &mut dyn FnMut(u64)) -> Result<()> {
    // A generic User-Agent, nothing about the user.
    let resp = match agent.get(url).set("User-Agent", "RustTalk").call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => bail!("server returned HTTP {code}"),
        Err(e) => bail!("server not reachable: {e}"),
    };
    let mut reader = resp.into_reader();
    let mut out = std::fs::File::create(part)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > f.size {
            bail!("file is larger than expected");
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        progress(n as u64);
    }
    out.flush()?;
    if written != f.size {
        bail!("connection closed early ({written} of {} bytes)", f.size);
    }
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if hex != f.sha256 {
        bail!("checksum mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_windows_tag() {
        assert_eq!(language_for("de-DE"), "de");
        assert_eq!(language_for("de-AT"), "de");
        assert_eq!(language_for("en-US"), "en");
        assert_eq!(language_for("fr-FR"), "en", "other languages: English");
        assert_eq!(language_for(""), "en");
        assert!(["de", "en"].contains(&system_language()));
        assert!(neural_id(DEFAULT_VOICE).is_some(), "default voice exists");
    }

    #[test]
    fn voice_ids() {
        assert_eq!(neural_id("neural:F1"), Some("F1"));
        assert_eq!(neural_id("neural:M5"), Some("M5"));
        assert_eq!(neural_id("neural:X9"), None, "unknown preset");
        assert_eq!(neural_id("Microsoft Zira"), None);
        assert_eq!(label("neural:M2"), "Natural - male 2");
        assert_eq!(label("Microsoft Zira"), "Microsoft Zira");
    }

    #[test]
    fn pack_list_is_consistent() {
        let mut names: Vec<_> = FILES.iter().map(|f| f.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), FILES.len(), "flat names are unique");
        assert!(FILES.iter().all(|f| f.sha256.len() == 64 && f.size > 0));
        for (id, _) in VOICES {
            assert!(FILES.iter().any(|f| f.name() == format!("{id}.json")), "voice {id} is in the pack");
        }
        assert!((400_000_000..420_000_000).contains(&total_size()), "about 400 MB");
        assert!(FILES.iter().any(|f| f.name() == "LICENSE"), "the model license comes with the pack");
    }

    #[test]
    fn missing_pack_is_not_installed() {
        let dir = std::env::temp_dir().join(format!("rusttalk-pack-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!installed_in(&dir));
        // Right names but wrong sizes don't count either.
        for f in &FILES {
            std::fs::write(dir.join(f.name()), b"x").unwrap();
        }
        assert!(!installed_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_download_leaves_no_file() {
        // Nothing listens on this port: the first file fails at once.
        let dir = std::env::temp_dir().join(format!("rusttalk-pack-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seen = std::cell::Cell::new(u64::MAX);
        let err = download_all(&dir, "http://127.0.0.1:9", &|d| seen.set(d)).unwrap_err();
        assert!(format!("{err:#}").contains("tts.json"), "{err:#}");
        assert_eq!(seen.get(), 0, "progress back at zero");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no partial file left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real download (network). Files already in the folder are kept, so
    /// seeding it with the big ones makes this quick:
    /// `RUSTTALK_TEST_PACK_DIR=<dir> cargo test -p rusttalk-client download_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn download_real_pack() {
        let dir = PathBuf::from(std::env::var("RUSTTALK_TEST_PACK_DIR").expect("RUSTTALK_TEST_PACK_DIR"));
        let missing: u64 = FILES.iter().filter(|f| !complete(&dir, f)).map(|f| f.size).sum();
        println!("downloading {} MB", missing / 1_000_000);
        let last = std::cell::Cell::new(0);
        download_all(&dir, BASE_URL, &|d| last.set(d)).unwrap();
        assert_eq!(last.get(), total_size());
        assert!(installed_in(&dir));
        assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".part")));
    }
}
