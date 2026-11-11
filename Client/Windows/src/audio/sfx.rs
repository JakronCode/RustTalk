//! Short notification sounds (join/leave/message/mute), generated in code –
//! no sound files. They are mixed into the playback stream via [`SfxQueue`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rusttalk_shared::audio_packet::SAMPLE_RATE;

/// Pending sound samples (48 kHz mono), consumed by the output callback.
pub type SfxQueue = Arc<Mutex<VecDeque<f32>>>;

/// At most this much sound is queued (rapid events must not pile up).
const MAX_QUEUED: usize = SAMPLE_RATE as usize; // 1 s

/// Peak level of a notification tone (before the output volume).
const LEVEL: f32 = 0.18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Someone joined your voice channel: two rising notes.
    Join,
    /// Someone left your voice channel: two falling notes.
    Leave,
    /// New chat message: one short high blip.
    Message,
    /// You muted yourself: low, falling.
    Mute,
    /// You unmuted yourself: low, rising.
    Unmute,
}

impl Sound {
    /// Notes as (frequency Hz, duration ms).
    fn notes(self) -> &'static [(f32, f32)] {
        match self {
            Sound::Join => &[(660.0, 70.0), (880.0, 110.0)],
            Sound::Leave => &[(880.0, 70.0), (587.0, 110.0)],
            Sound::Message => &[(1046.0, 70.0)],
            Sound::Mute => &[(440.0, 50.0), (330.0, 70.0)],
            Sound::Unmute => &[(330.0, 50.0), (440.0, 70.0)],
        }
    }
}

/// Renders a sound to 48 kHz samples (sine notes with short fades, no clicks).
pub fn render(sound: Sound) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let mut out = Vec::new();
    for &(freq, ms) in sound.notes() {
        let n = (sr * ms / 1000.0) as usize;
        let fade = (sr * 0.008) as usize; // 8 ms in/out
        for i in 0..n {
            let env = if i < fade {
                i as f32 / fade as f32
            } else if i + fade > n {
                (n - i) as f32 / fade as f32
            } else {
                1.0
            };
            let phase = 2.0 * std::f32::consts::PI * freq * i as f32 / sr;
            out.push(phase.sin() * env * LEVEL);
        }
    }
    out
}

/// Queues a sound for playback (dropped if too much is already queued).
pub fn play(queue: &SfxQueue, sound: Sound) {
    let samples = render(sound);
    let mut q = queue.lock().unwrap();
    if q.len() + samples.len() <= MAX_QUEUED {
        q.extend(samples);
    }
}

/// Mixes queued sound into `chunk` (called from the output callback).
pub fn mix_into(queue: &SfxQueue, chunk: &mut [f32]) {
    let mut q = queue.lock().unwrap();
    for s in chunk.iter_mut() {
        match q.pop_front() {
            Some(x) => *s += x,
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_quiet_and_click_free() {
        for s in [Sound::Join, Sound::Leave, Sound::Message, Sound::Mute, Sound::Unmute] {
            let v = render(s);
            let ms = v.len() as f32 / SAMPLE_RATE as f32 * 1000.0;
            assert!((50.0..=250.0).contains(&ms), "{s:?}: {ms} ms");
            assert!(v.iter().all(|x| x.abs() <= LEVEL + 1e-6), "{s:?} too loud");
            assert!(v[0].abs() < 0.01 && v[v.len() - 1].abs() < 0.02, "{s:?} starts/ends quietly");
        }
        assert_ne!(render(Sound::Join), render(Sound::Leave));
    }

    #[test]
    fn queue_mixes_and_caps() {
        let q: SfxQueue = Arc::new(Mutex::new(VecDeque::new()));
        play(&q, Sound::Message);
        let len = q.lock().unwrap().len();
        let mut chunk = [0.0f32; 960];
        mix_into(&q, &mut chunk);
        assert_eq!(q.lock().unwrap().len(), len - 960, "one 20 ms chunk consumed");
        assert!(chunk.iter().any(|x| *x != 0.0));
        for _ in 0..100 {
            play(&q, Sound::Join);
        }
        assert!(q.lock().unwrap().len() <= MAX_QUEUED, "capped");
    }
}
