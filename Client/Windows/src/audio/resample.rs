//! Einfacher linearer Resampler (MVP-Qualitaet, latenzarm, allocationfrei nach Warmup).
//!
//! Push-basiert: Eingangssamples (Mono, `in_rate`) rein,
//! Ausgangssamples (`out_rate`) werden an `out` angehaengt.

use rusttalk_shared::audio_packet::SAMPLE_RATE;

#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    accum: u32,
    prev: f32,
    has_prev: bool,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        debug_assert!(in_rate > 0 && out_rate > 0);
        Self { in_rate, out_rate, accum: 0, prev: 0.0, has_prev: false }
    }

    /// Resampler von beliebiger Rate auf die internen 48 kHz.
    pub fn to_internal(in_rate: u32) -> Self {
        Self::new(in_rate, SAMPLE_RATE)
    }

    /// Resampler von internen 48 kHz auf die Geraete-Rate.
    pub fn from_internal(out_rate: u32) -> Self {
        Self::new(SAMPLE_RATE, out_rate)
    }

    pub fn is_identity(&self) -> bool {
        self.in_rate == self.out_rate
    }

    /// Verarbeitet `input` und haengt Ergebnisse an `out` an (out wird nicht geleert).
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        for &s in input {
            if !self.has_prev {
                self.prev = s;
                self.has_prev = true;
                continue;
            }
            self.accum += self.out_rate;
            while self.accum >= self.in_rate {
                self.accum -= self.in_rate;
                let frac = self.accum as f32 / self.in_rate as f32;
                out.push(self.prev + (s - self.prev) * (1.0 - frac));
            }
            self.prev = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_passthrough() {
        let mut r = Resampler::new(48_000, 48_000);
        let input = [0.1, -0.2, 0.3, 0.0];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn upsample_rate_ratio() {
        // 44100 -> 48000: 44100 Input-Samples ergeben ~48000 Output-Samples.
        let mut r = Resampler::new(44_100, 48_000);
        let input = vec![0.0f32; 44_100];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let n = out.len();
        assert!((47_990..=48_010).contains(&n), "unerwartete Samplezahl: {n}");
    }

    #[test]
    fn downsample_rate_ratio() {
        // 96000 -> 48000: Halbierung.
        let mut r = Resampler::new(96_000, 48_000);
        let input = vec![0.5f32; 96_000];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let n = out.len();
        assert!((47_990..=48_010).contains(&n), "unerwartete Samplezahl: {n}");
    }

    #[test]
    fn constant_signal_stays_constant() {
        let mut r = Resampler::new(44_100, 48_000);
        let input = vec![0.25f32; 4410];
        let mut out = Vec::new();
        r.process(&input, &mut out);
        // Nach dem ersten Sample muss ein konstantes Signal konstant bleiben.
        assert!(out[1..].iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }
}
