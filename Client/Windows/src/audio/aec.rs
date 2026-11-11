//! Acoustic echo cancellation (WebRTC AEC3, pure Rust port `aec3`).
//!
//! Without headphones the microphone picks up what the speakers play;
//! everyone else then hears themselves. The playback callback hands every
//! mixed 48 kHz sample it plays to the encoder thread (lock-free ring, the
//! "render reference"); the encoder runs each 10 ms of microphone audio
//! through AEC3 together with the matching 10 ms of render audio, which
//! subtracts the echo. Noise suppression and gain control stay ours (RNNoise,
//! own AGC) - AEC3 here is high-pass + echo canceller only.

use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};
use ringbuf::traits::{Consumer, Observer};
use ringbuf::HeapCons;
use rusttalk_shared::audio_packet::SAMPLE_RATE;

/// 10 ms at 48 kHz.
pub const AEC_FRAME: usize = (SAMPLE_RATE / 100) as usize;
/// Typical speaker -> microphone delay of a PC (output buffer + input buffer).
const INITIAL_DELAY_MS: i32 = 60;
/// More render backlog than this (output started earlier than input, or a
/// hiccup) is dropped so the reference does not drift away from the capture.
const MAX_RENDER_BACKLOG: usize = AEC_FRAME * 30; // 300 ms

pub struct EchoCanceller {
    pipeline: LinearPipeline,
    render: [f32; AEC_FRAME],
    out: [f32; AEC_FRAME],
}

impl EchoCanceller {
    pub fn new() -> Option<Self> {
        let format = AudioFormat::ten_ms(SAMPLE_RATE, 1);
        let pipeline = linear::builder(format, format)
            .enable_noise_suppression(false)
            .enable_gain_controller2(false)
            .initial_delay_ms(INITIAL_DELAY_MS)
            .build()
            .map_err(|e| tracing::warn!("AEC3 konnte nicht erstellt werden: {e:?}"))
            .ok()?;
        Some(Self { pipeline, render: [0.0; AEC_FRAME], out: [0.0; AEC_FRAME] })
    }

    /// Processes 10 ms of capture in place. `render` = what was played at the
    /// same time (silence when nothing was played).
    pub fn process(&mut self, capture: &mut [f32], render: &[f32]) {
        debug_assert_eq!(capture.len(), AEC_FRAME);
        if self.pipeline.handle_render_frame(render).is_err() {
            return;
        }
        match self.pipeline.process_capture_frame(capture, &mut self.out) {
            Ok(true) => capture.copy_from_slice(&self.out),
            _ => {} // no output yet: keep the original
        }
    }

    /// Like [`process`](Self::process), taking the render reference from the
    /// playback ring (zeros when it is empty).
    pub fn process_with_ring(&mut self, capture: &mut [f32], ring: &mut HeapCons<f32>) {
        let backlog = ring.occupied_len();
        if backlog > MAX_RENDER_BACKLOG {
            ring.skip(backlog - AEC_FRAME * 6);
        }
        let n = ring.pop_slice(&mut self.render);
        self.render[n..].fill(0.0);
        let render = self.render;
        self.process(capture, &render);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Speech-like test signal: broadband noise (deterministic hash) with a
    /// syllable-rate envelope. (Pure sine tones are a known worst case for AEC.)
    fn far_end(i: usize) -> f32 {
        let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
        x ^= x >> 29;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 32;
        let noise = (x as u32) as f32 / u32::MAX as f32 * 2.0 - 1.0;
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = 0.3 + 0.7 * (2.0 * std::f32::consts::PI * 4.0 * t).sin().abs();
        0.25 * env * noise
    }

    /// Synthetic room: the microphone hears the speaker 40 ms later at half
    /// level. After a few seconds AEC3 must remove most of it.
    #[test]
    fn removes_a_delayed_echo() {
        let mut aec = EchoCanceller::new().expect("aec");
        let delay = SAMPLE_RATE as usize * 40 / 1000;
        let total = SAMPLE_RATE as usize * 8; // 8 s
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut i = 0;
        while i + AEC_FRAME <= total {
            let render: Vec<f32> = (i..i + AEC_FRAME).map(far_end).collect();
            let mut capture: Vec<f32> = (i..i + AEC_FRAME).map(|k| if k >= delay { 0.5 * far_end(k - delay) } else { 0.0 }).collect();
            let original = capture.clone();
            aec.process(&mut capture, &render);
            if i >= SAMPLE_RATE as usize * 5 {
                before.extend_from_slice(&original);
                after.extend_from_slice(&capture);
            }
            i += AEC_FRAME;
        }
        let (b, a) = (rms(&before), rms(&after));
        assert!(b > 0.01, "test signal present: {b}");
        // At least 15 dB less echo after convergence.
        assert!(a < b * 0.18, "echo not removed enough: before {b:.4}, after {a:.4} ({:.1} dB)", 20.0 * (a / b).log10());
    }

    /// Near-end speech without any playback must pass (no echo to remove).
    #[test]
    fn keeps_near_end_without_playback() {
        let mut aec = EchoCanceller::new().expect("aec");
        let silence = [0.0f32; AEC_FRAME];
        let mut kept = Vec::new();
        let mut orig = Vec::new();
        for f in 0..300 {
            let mut capture: Vec<f32> = (0..AEC_FRAME).map(|k| far_end(f * AEC_FRAME + k)).collect();
            let original = capture.clone();
            aec.process(&mut capture, &silence);
            if f > 100 {
                kept.extend_from_slice(&capture);
                orig.extend_from_slice(&original);
            }
        }
        let ratio = rms(&kept) / rms(&orig);
        assert!(ratio > 0.7, "near-end speech must survive: ratio {ratio:.2}");
    }
}
