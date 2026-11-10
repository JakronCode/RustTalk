//! Resampler fuer die Mikrofon-Aufnahme: beliebige Geraeterate -> 48 kHz.
//!
//! Gefensterter Sinc-Filter (polyphase Tabelle, lineare Interpolation
//! zwischen den Phasen). Frueher linear ohne Tiefpass: bei 16-kHz-Headsets
//! (Bluetooth) und 96-kHz-Interfaces entstanden Spiegelfrequenzen/Aliasing,
//! hoerbar als metallischer, kratziger Klang.
//!
//! Push-basiert: Eingangssamples (Mono, `in_rate`) rein, Ausgangssamples
//! (`out_rate`) werden an `out` angehaengt. Latenz: `half_taps` Eingangs-
//! samples (~0,3-1 ms).

use std::f64::consts::PI;

use rusttalk_shared::audio_packet::SAMPLE_RATE;

/// Halbe Filterlaenge (Taps je Seite) bei Upsampling.
const BASE_HALF_TAPS: usize = 16;
/// Phasen der Polyphasen-Tabelle.
const PHASES: usize = 256;
/// Grenzfrequenz relativ zur kleineren Nyquist-Frequenz (Rest = Uebergangsband).
const CUTOFF: f64 = 0.92;

#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    /// Eingangssamples pro Ausgangssample.
    step: f64,
    half_taps: usize,
    /// (PHASES + 1) Zeilen x (2 * half_taps) Koeffizienten.
    table: Vec<f32>,
    /// Eingangs-Historie; `pos` ist die Position des naechsten Outputs darin.
    buf: Vec<f32>,
    pos: f64,
}

/// Blackman-Fenster auf [-1, 1].
fn blackman(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let t = (x + 1.0) / 2.0; // 0..1
    0.42 - 0.5 * (2.0 * PI * t).cos() + 0.08 * (4.0 * PI * t).cos()
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        debug_assert!(in_rate > 0 && out_rate > 0);
        let step = in_rate as f64 / out_rate as f64;
        // Beim Downsampling Grenzfrequenz auf die Ziel-Nyquist senken und den
        // Filter entsprechend verlaengern (gleiche Steilheit).
        let fc = CUTOFF * (out_rate as f64 / in_rate as f64).min(1.0);
        let half_taps = ((BASE_HALF_TAPS as f64) * (CUTOFF / fc)).ceil() as usize;
        let width = 2 * half_taps;

        let mut table = vec![0f32; (PHASES + 1) * width];
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let row = &mut table[p * width..(p + 1) * width];
            let mut sum = 0.0;
            for (j, c) in row.iter_mut().enumerate() {
                // Abstand Ausgabezeitpunkt -> Tap j (siehe `process`).
                let d = frac + half_taps as f64 - 1.0 - j as f64;
                let v = fc * sinc(fc * d) * blackman(d / half_taps as f64);
                *c = v as f32;
                sum += v;
            }
            // Jede Phase auf Gleichanteil 1 normieren -> DC exakt erhalten.
            if sum.abs() > 1e-12 {
                for c in row.iter_mut() {
                    *c = (*c as f64 / sum) as f32;
                }
            }
        }

        Self {
            in_rate,
            out_rate,
            step,
            half_taps,
            table,
            // Vorlauf mit Stille, damit von Anfang an volle Filterfenster da sind.
            buf: vec![0.0; half_taps],
            pos: (half_taps - 1) as f64,
        }
    }

    /// Resampler von beliebiger Rate auf die internen 48 kHz.
    pub fn to_internal(in_rate: u32) -> Self {
        Self::new(in_rate, SAMPLE_RATE)
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
        self.buf.extend_from_slice(input);
        let h = self.half_taps;
        let width = 2 * h;
        loop {
            let i = self.pos.floor() as usize;
            // Fenster i-h+1 ..= i+h muss komplett vorliegen.
            if i + h >= self.buf.len() {
                break;
            }
            let frac = self.pos - i as f64;
            let pf = frac * PHASES as f64;
            let p0 = (pf.floor() as usize).min(PHASES - 1);
            let w = (pf - p0 as f64) as f32;
            let row0 = &self.table[p0 * width..(p0 + 1) * width];
            let row1 = &self.table[(p0 + 1) * width..(p0 + 2) * width];
            let taps = &self.buf[i + 1 - h..=i + h];
            let mut acc = 0f32;
            for j in 0..width {
                let c = row0[j] + (row1[j] - row0[j]) * w;
                acc += taps[j] * c;
            }
            out.push(acc);
            self.pos += self.step;
        }
        // Verbrauchte Historie abschneiden (h Samples Rueckblick behalten).
        let keep_from = (self.pos.floor() as usize + 1).saturating_sub(h);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f64, secs: f64, amp: f32) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        (0..n).map(|i| amp * (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// In kleinen, ungleichen Bloecken einspeisen (wie cpal-Callbacks).
    fn run(r: &mut Resampler, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        for (k, chunk) in input.chunks(441 + 7).enumerate() {
            let split = (k * 37) % chunk.len().max(1);
            r.process(&chunk[..split], &mut out);
            r.process(&chunk[split..], &mut out);
        }
        out
    }

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
        let n = run(&mut r, &vec![0.0f32; 44_100]).len();
        assert!((47_950..=48_010).contains(&n), "unerwartete Samplezahl: {n}");
    }

    #[test]
    fn downsample_rate_ratio() {
        // 96000 -> 48000: Halbierung.
        let mut r = Resampler::new(96_000, 48_000);
        let n = run(&mut r, &vec![0.5f32; 96_000]).len();
        assert!((47_950..=48_010).contains(&n), "unerwartete Samplezahl: {n}");
    }

    #[test]
    fn constant_signal_stays_constant() {
        let mut r = Resampler::new(44_100, 48_000);
        let out = run(&mut r, &vec![0.25f32; 4410]);
        // Nach dem Einschwingen (Vorlauf mit Stille) exakt konstant.
        assert!(out[100..].iter().all(|&s| (s - 0.25).abs() < 1e-4));
    }

    #[test]
    fn passband_tone_keeps_amplitude() {
        // 16-kHz-Headset: 3-kHz-Sprachanteil muss unveraendert durchkommen.
        let mut r = Resampler::new(16_000, 48_000);
        let out = run(&mut r, &tone(16_000, 3_000.0, 1.0, 0.5));
        let level = rms(&out[1000..out.len() - 1000]);
        assert!((level - 0.5 / 2f32.sqrt()).abs() < 0.01, "Pegel {level}");
    }

    #[test]
    fn downsampling_suppresses_aliasing() {
        // 30 kHz liegt ueber der Ziel-Nyquist (24 kHz). Linear wuerde er als
        // 18 kHz fast ungedaempft zurueckgespiegelt; der Sinc-Filter muss ihn
        // um > 40 dB daempfen.
        let mut r = Resampler::new(96_000, 48_000);
        let out = run(&mut r, &tone(96_000, 30_000.0, 0.5, 0.5));
        let level = rms(&out[500..]);
        assert!(level < 0.5 * 0.01, "Alias-Pegel zu hoch: {level}");
    }

    #[test]
    fn upsampling_suppresses_images() {
        // 16 kHz -> 48 kHz mit 7-kHz-Ton: lineare Interpolation erzeugt
        // Spiegelbilder bei 9 kHz/23 kHz. Ausgang nach Abzug des Tons pruefen.
        let mut r = Resampler::new(16_000, 48_000);
        let out = run(&mut r, &tone(16_000, 7_000.0, 1.0, 0.5));
        // Bestes Fitting des 7-kHz-Tons (Phase unbekannt -> sin/cos-Projektion).
        let seg = &out[2000..out.len() - 2000];
        let (mut s, mut c) = (0f64, 0f64);
        for (k, &x) in seg.iter().enumerate() {
            let ph = 2.0 * PI * 7_000.0 * k as f64 / 48_000.0;
            s += x as f64 * ph.sin();
            c += x as f64 * ph.cos();
        }
        let n = seg.len() as f64;
        let (a, b) = (2.0 * s / n, 2.0 * c / n);
        let resid: Vec<f32> = seg
            .iter()
            .enumerate()
            .map(|(k, &x)| {
                let ph = 2.0 * PI * 7_000.0 * k as f64 / 48_000.0;
                x - (a * ph.sin() + b * ph.cos()) as f32
            })
            .collect();
        let level = rms(&resid);
        assert!(level < 0.5 * 0.02, "Spiegelfrequenzen zu laut: {level}");
    }
}
