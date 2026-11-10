//! Kleine Sprach-DSP-Bausteine fuer die Sendeseite und den Mixer.
//!
//! Ziel: Auch mit billigen Mikros (zu leise, Gleichanteil, uebersteuert)
//! soll beim Gegenueber ein gleichmaessig lauter, sauberer Pegel ankommen.

/// Entfernt Gleichanteil/Rumpeln (einpoliger Hochpass, ~15 Hz @ 48 kHz).
/// Billige Mikros liefern oft einen DC-Offset, der RNNoise und den
/// Pegelmesser verfaelscht.
#[derive(Debug, Default)]
pub struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    const R: f32 = 0.998;

    pub fn process(&mut self, frame: &mut [f32]) {
        for s in frame {
            let y = *s - self.x1 + Self::R * self.y1;
            self.x1 = *s;
            self.y1 = y;
            *s = y;
        }
    }
}

/// Automatische Lautstaerke (AGC) fuer Sprache.
///
/// - Zielpegel ~ -20 dBFS RMS; Verstaerkung zwischen -12 dB und +16 dB.
/// - Passt sich nur waehrend Sprache an (sonst wuerde in Pausen das
///   Rauschen hochgezogen).
/// - Leiser werden geht schnell (kein Uebersteuern), lauter werden langsam
///   (~1 s), kein "Pumpen".
/// - Verstaerkung wird innerhalb eines Frames sanft gerampt (kein Knacken).
#[derive(Debug)]
pub struct Agc {
    gain: f32,
}

impl Default for Agc {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl Agc {
    pub const TARGET_RMS: f32 = 0.1; // -20 dBFS
    pub const MAX_GAIN: f32 = 6.3; // +16 dB
    pub const MIN_GAIN: f32 = 0.25; // -12 dB
    /// Unter diesem Pegel ist es kein brauchbares Sprachsignal -> nicht anpassen.
    const MIN_SPEECH_RMS: f32 = 0.003;

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// `rms`: Pegel des Frames vor der Verstaerkung; `speech`: Sprache erkannt.
    pub fn process(&mut self, frame: &mut [f32], rms: f32, speech: bool) {
        let old = self.gain;
        if speech && rms > Self::MIN_SPEECH_RMS {
            let desired = (Self::TARGET_RMS / rms).clamp(Self::MIN_GAIN, Self::MAX_GAIN);
            if desired < self.gain {
                self.gain += (desired - self.gain) * 0.5; // schnell runter
            } else {
                self.gain += (desired - self.gain) * 0.03; // langsam hoch (~0,7 s)
            }
        }
        let n = frame.len().max(1) as f32;
        for (k, s) in frame.iter_mut().enumerate() {
            let g = old + (self.gain - old) * (k as f32 + 1.0) / n;
            *s *= g;
        }
    }
}

/// Weicher Limiter: unterhalb von `THRESHOLD` unveraendert, darueber
/// sanft gegen 1.0 gebogen (statt hartem Abschneiden = Verzerrung).
pub fn soft_limit(x: f32) -> f32 {
    const THRESHOLD: f32 = 0.8;
    let a = x.abs();
    if a <= THRESHOLD {
        return x;
    }
    let over = (a - THRESHOLD) / (1.0 - THRESHOLD);
    let y = THRESHOLD + (1.0 - THRESHOLD) * over.tanh();
    y.copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn sine(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.05).sin()).collect()
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut dc = DcBlocker::default();
        let mut x: Vec<f32> = sine(48_000, 0.1).iter().map(|s| s + 0.3).collect();
        dc.process(&mut x);
        let tail = &x[24_000..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.005, "Gleichanteil uebrig: {mean}");
        assert!((rms(tail) - 0.1 / 2f32.sqrt()).abs() < 0.01, "Sprache bleibt");
    }

    #[test]
    fn agc_boosts_quiet_mic() {
        // Leises Mikro: RMS ~0.014 (-37 dBFS) -> nach 3 s deutlich lauter.
        let mut agc = Agc::default();
        let mut last = 0.0;
        for _ in 0..150 {
            let mut f = sine(960, 0.02);
            let level = rms(&f);
            agc.process(&mut f, level, true);
            last = rms(&f);
        }
        assert!(agc.gain() > 5.0, "Gain {}", agc.gain());
        assert!(last > 0.07, "Pegel nach AGC {last}");
    }

    #[test]
    fn agc_tames_hot_mic_quickly() {
        let mut agc = Agc::default();
        for _ in 0..10 {
            let mut f = sine(960, 0.9);
            let level = rms(&f);
            agc.process(&mut f, level, true);
        }
        assert!(agc.gain() < 0.3, "Gain {}", agc.gain());
    }

    #[test]
    fn agc_ignores_silence_and_noise() {
        // Pausen/Rauschen ohne Sprache duerfen die Verstaerkung nicht hochziehen.
        let mut agc = Agc::default();
        for _ in 0..200 {
            let mut f = sine(960, 0.002);
            let level = rms(&f);
            agc.process(&mut f, level, false);
        }
        assert_eq!(agc.gain(), 1.0);
    }

    #[test]
    fn soft_limit_is_transparent_below_threshold_and_bounded_above() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.8), -0.8);
        for x in [0.9f32, 1.5, 3.0, 100.0] {
            let y = soft_limit(x);
            assert!(y > 0.8 && y <= 1.0, "{x} -> {y}");
            assert_eq!(soft_limit(-x), -y);
        }
        // Monoton steigend.
        assert!(soft_limit(1.2) > soft_limit(1.0));
    }
}
