//! Short notification sounds (join/leave/message/mute), generated in code –
//! no sound files. They are mixed into the playback stream via [`SfxQueue`].
//!
//! The tones are soft mallet/bell notes (a few harmonics that die away at
//! different speeds, notes ring into each other) instead of plain beeps.
//! Spoken names (see [`super::speech`]) run on a second lane, so a chime and
//! the name after it can overlap and a message blip never waits for speech.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rusttalk_shared::audio_packet::SAMPLE_RATE;

/// Pending sound samples (48 kHz mono), consumed by the output callback.
#[derive(Default)]
pub struct Queues {
    tones: VecDeque<f32>,
    speech: VecDeque<f32>,
}

impl Queues {
    pub fn clear(&mut self) {
        self.tones.clear();
        self.speech.clear();
    }
}

pub type SfxQueue = Arc<Mutex<Queues>>;

/// At most this much sound is queued (rapid events must not pile up).
const MAX_TONES: usize = SAMPLE_RATE as usize; // 1 s
const MAX_SPEECH: usize = 4 * SAMPLE_RATE as usize; // 4 s

/// Peak level of a notification tone (before the output volume).
const LEVEL: f32 = 0.2;

/// A spoken name starts this long after its chime (they overlap a little).
const SPEECH_LEAD_MS: f32 = 230.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Someone joined your voice channel: a rising fifth.
    Join,
    /// Someone left your voice channel: a falling fifth.
    Leave,
    /// New chat message: a short, bright double tap.
    Message,
    /// You were @mentioned: a rising three-note chord.
    Mention,
    /// You muted yourself: low, falling.
    Mute,
    /// Someone poked you: a short alarm-clock ring ("wake up!").
    Poke,
    /// You unmuted yourself: low, rising.
    Unmute,
}

#[cfg(test)]
impl Sound {
    pub const ALL: [Sound; 7] = [Sound::Join, Sound::Leave, Sound::Message, Sound::Mention, Sound::Mute, Sound::Unmute, Sound::Poke];
}

/// One note of a sound.
struct Note {
    freq: f32,
    /// Start within the sound (notes may overlap).
    start_ms: f32,
    /// Length until it has fully died away.
    len_ms: f32,
    gain: f32,
    /// Share of overtones: 0 = pure and dull, 1 = bright bell.
    bright: f32,
}

const fn note(freq: f32, start_ms: f32, len_ms: f32, gain: f32, bright: f32) -> Note {
    Note { freq, start_ms, len_ms, gain, bright }
}

// Note frequencies (equal temperament, A4 = 440 Hz).
const C4: f32 = 261.63;
const E4: f32 = 329.63;
const C5: f32 = 523.25;
const G5: f32 = 783.99;
const A5: f32 = 880.0;
const CS6: f32 = 1108.73;
const E6: f32 = 1318.51;

impl Sound {
    fn notes(self) -> Vec<Note> {
        match self {
            Sound::Join => vec![note(C5, 0.0, 300.0, 0.9, 0.7), note(G5, 95.0, 380.0, 1.0, 0.7)],
            Sound::Leave => vec![note(G5, 0.0, 300.0, 0.9, 0.7), note(C5, 95.0, 380.0, 1.0, 0.7)],
            Sound::Message => vec![note(A5, 0.0, 150.0, 0.8, 0.5), note(E6, 65.0, 240.0, 1.0, 0.5)],
            Sound::Mention => vec![note(A5, 0.0, 260.0, 0.8, 0.8), note(CS6, 80.0, 280.0, 0.9, 0.8), note(E6, 160.0, 340.0, 1.0, 0.8)],
            Sound::Mute => vec![note(E4, 0.0, 110.0, 1.0, 0.25), note(C4, 55.0, 150.0, 1.0, 0.25)],
            Sound::Unmute => vec![note(C4, 0.0, 110.0, 1.0, 0.25), note(E4, 55.0, 150.0, 1.0, 0.25)],
            // Two quick bursts of a ringing bell, like an alarm clock.
            Sound::Poke => (0..2)
                .flat_map(|burst| {
                    (0..5).map(move |i| {
                        let f = if i % 2 == 0 { E6 } else { CS6 };
                        note(f, burst as f32 * 470.0 + i as f32 * 62.0, 140.0, 1.0, 0.9)
                    })
                })
                .collect(),
        }
    }
}

/// Adds one mallet note to `out`: a soft attack, then every partial dies away
/// exponentially – the overtones faster than the fundamental, like a real bar.
fn add_note(out: &mut Vec<f32>, n: &Note) {
    let sr = SAMPLE_RATE as f32;
    let start = (sr * n.start_ms / 1000.0) as usize;
    let len = (sr * n.len_ms / 1000.0) as usize;
    if out.len() < start + len {
        out.resize(start + len, 0.0);
    }
    let attack = (sr * 0.004) as usize; // 4 ms: no click, still percussive
    let release = (sr * 0.02) as usize; // the tail ends exactly at zero
    // (harmonic, level, decay speed relative to the fundamental)
    let partials = [(1.0, 1.0, 1.0), (2.0, 0.42 * n.bright, 1.8), (3.0, 0.2 * n.bright, 2.8), (4.0, 0.08 * n.bright, 4.0)];
    // The fundamental falls to about -40 dB over the note's length.
    let decay = 4.6 / (n.len_ms / 1000.0);
    for i in 0..len {
        let t = i as f32 / sr;
        let mut env = 1.0;
        if i < attack {
            env *= 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / attack as f32).cos();
        }
        if i + release > len {
            env *= (len - i) as f32 / release as f32;
        }
        let mut v = 0.0;
        for (h, level, speed) in partials {
            v += level * (-decay * speed * t).exp() * (2.0 * std::f32::consts::PI * n.freq * h * t).sin();
        }
        out[start + i] += v * env * n.gain;
    }
}

/// Renders a sound to 48 kHz samples, peak-normalised to [`LEVEL`].
pub fn render(sound: Sound) -> Vec<f32> {
    let mut out = Vec::new();
    for n in sound.notes() {
        add_note(&mut out, &n);
    }
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak > 0.0 {
        let g = LEVEL / peak;
        for s in &mut out {
            *s *= g;
        }
    }
    out
}

/// Queues a sound for playback (dropped if too much is already queued).
pub fn play(queue: &SfxQueue, sound: Sound) {
    let samples = render(sound);
    let mut q = queue.lock().unwrap();
    if q.tones.len() + samples.len() <= MAX_TONES {
        q.tones.extend(samples);
    }
}

/// Queues spoken audio (48 kHz mono). It starts shortly after the chime that
/// was queued with it; several announcements play one after another.
pub fn play_speech(queue: &SfxQueue, samples: &[f32]) {
    let mut q = queue.lock().unwrap();
    if q.speech.len() + samples.len() > MAX_SPEECH {
        return;
    }
    let lead = (SAMPLE_RATE as f32 * SPEECH_LEAD_MS / 1000.0) as usize;
    q.speech.extend(std::iter::repeat(0.0).take(lead));
    q.speech.extend(samples);
}

/// Mixes queued sound into `chunk` (called from the output callback).
pub fn mix_into(queue: &SfxQueue, chunk: &mut [f32]) {
    let mut q = queue.lock().unwrap();
    if q.tones.is_empty() && q.speech.is_empty() {
        return;
    }
    for s in chunk.iter_mut() {
        *s += q.tones.pop_front().unwrap_or(0.0) + q.speech.pop_front().unwrap_or(0.0);
    }
}

/// Writes 48 kHz mono samples as a 16-bit WAV file (for listening tests).
#[cfg(test)]
pub fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + samples.len() * 2);
    b.extend(b"RIFF");
    b.extend((36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes()); // PCM
    b.extend(1u16.to_le_bytes()); // mono
    b.extend(SAMPLE_RATE.to_le_bytes());
    b.extend((SAMPLE_RATE * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data_len.to_le_bytes());
    for s in samples {
        b.extend(((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_quiet_and_click_free() {
        for s in Sound::ALL {
            let v = render(s);
            let ms = v.len() as f32 / SAMPLE_RATE as f32 * 1000.0;
            // The poke ring is meant to wake you up, so it may ring a bit longer.
            let max_ms = if s == Sound::Poke { 1000.0 } else { 550.0 };
            assert!((150.0..=max_ms).contains(&ms), "{s:?}: {ms} ms");
            let peak = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!((peak - LEVEL).abs() < 1e-4, "{s:?} peak {peak}");
            assert!(v[0].abs() < 0.01 && v[v.len() - 1].abs() < 0.001, "{s:?} starts/ends quietly");
            // No jump between neighbouring samples (that would be a click).
            let step = v.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
            assert!(step < 0.08, "{s:?} jumps by {step}");
        }
        assert_ne!(render(Sound::Join), render(Sound::Leave));
        assert_ne!(render(Sound::Message), render(Sound::Mention));
    }

    #[test]
    fn queue_mixes_and_caps() {
        let q: SfxQueue = Default::default();
        play(&q, Sound::Message);
        let len = q.lock().unwrap().tones.len();
        let mut chunk = [0.0f32; 960];
        mix_into(&q, &mut chunk);
        assert_eq!(q.lock().unwrap().tones.len(), len - 960, "one 20 ms chunk consumed");
        assert!(chunk.iter().any(|x| *x != 0.0));
        for _ in 0..100 {
            play(&q, Sound::Join);
        }
        assert!(q.lock().unwrap().tones.len() <= MAX_TONES, "capped");
    }

    #[test]
    fn speech_runs_beside_tones_and_caps() {
        let q: SfxQueue = Default::default();
        play(&q, Sound::Join);
        play_speech(&q, &[0.5; 4800]);
        let lead = (SAMPLE_RATE as f32 * SPEECH_LEAD_MS / 1000.0) as usize;
        assert_eq!(q.lock().unwrap().speech.len(), lead + 4800);
        // The chime is audible at once, the speech only after the lead.
        let mut chunk = [0.0f32; 960];
        mix_into(&q, &mut chunk);
        assert!(chunk.iter().all(|x| x.abs() <= LEVEL + 1e-4), "speech not started yet");
        // Tones run out first; speech keeps playing.
        let mut rest = vec![0.0f32; 48000];
        mix_into(&q, &mut rest);
        assert!(rest[lead - 960 + 10] > 0.25, "speech after the lead");
        for _ in 0..100 {
            play_speech(&q, &[0.1; 48000]);
        }
        assert!(q.lock().unwrap().speech.len() <= MAX_SPEECH, "capped");
        q.lock().unwrap().clear();
        let mut silent = [0.0f32; 960];
        mix_into(&q, &mut silent);
        assert!(silent.iter().all(|x| *x == 0.0));
    }

    /// Listening test: `cargo test -p rusttalk-client dump_sounds -- --ignored`
    /// writes every sound to `target/sfx-preview/*.wav`.
    #[test]
    #[ignore]
    fn dump_sounds() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sfx-preview");
        std::fs::create_dir_all(&dir).unwrap();
        for s in Sound::ALL {
            write_wav(&dir.join(format!("{s:?}.wav").to_lowercase()), &render(s));
        }
    }
}
