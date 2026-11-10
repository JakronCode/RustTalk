//! PTT/VAD-Gate + Opus-Encoder-Thread (Sendeseite).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate, Signal};
use nnnoiseless::DenoiseState;
use ringbuf::traits::Consumer;
use ringbuf::HeapCons;
use rusttalk_shared::audio_packet::{encode_to_vec, AudioHeader, FRAME_SAMPLES, SAMPLE_RATE};

use super::dsp::{soft_limit, Agc, DcBlocker};
use super::AudioControl;

/// RMS-Schwellen-Gate mit Hangover, damit Wortenden nicht abgehackt werden.
#[derive(Debug)]
pub struct VoiceGate {
    pub threshold: f32,
    hangover_frames: usize,
    cooldown: usize,
    active: bool,
}

impl VoiceGate {
    /// `hangover_frames`: so viele stille Frames werden noch gesendet
    /// (15 Frames a 20 ms = 300 ms).
    pub fn new(threshold: f32, hangover_frames: usize) -> Self {
        Self { threshold, hangover_frames, cooldown: 0, active: false }
    }

    /// Gibt true zurueck, wenn das Frame gesendet werden soll.
    pub fn process(&mut self, rms: f32) -> bool {
        if rms >= self.threshold {
            self.cooldown = self.hangover_frames;
            self.active = true;
        } else if self.cooldown > 0 {
            self.cooldown -= 1;
        } else {
            self.active = false;
        }
        self.active
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// RMS (Root Mean Square) eines Frames als einfacher Lautstaerkewert.
pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Opus fuer Sprache: 64 kbit/s (wie Discord-Standard), hoechste Komplexitaet,
/// Signal "Voice". `packet_loss_perc` ist noetig, damit Opus die In-Band-FEC
/// ueberhaupt erzeugt (ohne den Wert war `inband_fec` wirkungslos) – damit
/// kann der Empfaenger ein verlorenes Paket aus dem naechsten rekonstruieren.
pub fn configure_encoder(e: &mut Encoder) {
    let _ = e.set_bitrate(Bitrate::BitsPerSecond(64_000));
    let _ = e.set_complexity(10);
    let _ = e.set_signal(Signal::Voice);
    let _ = e.set_inband_fec(true);
    let _ = e.set_packet_loss_perc(10);
}

/// Encoder-Thread: 20-ms-Frames aus dem Ring-Puffer -> DC-Filter -> Denoise
/// -> Gate -> AGC/Limiter -> Opus -> UDP.
pub fn run(
    mut cons: HeapCons<f32>,
    udp: std::net::UdpSocket,
    token: u64,
    control: Arc<AudioControl>,
    stop: Arc<AtomicBool>,
) {
    let encoder = match Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip) {
        Ok(mut e) => {
            configure_encoder(&mut e);
            e
        }
        Err(e) => {
            tracing::error!("Opus-Encoder konnte nicht erstellt werden: {e}");
            return;
        }
    };

    // Registrierungs-Ping: der Server lernt so unsere UDP-Adresse.
    let _ = udp.send(&encode_to_vec(&AudioHeader::new(token, 0, 0), &[]));

    let mut gate = VoiceGate::new(control.vad_threshold(), 15);
    let mut frame = [0f32; FRAME_SAMPLES];
    let mut have = 0usize;
    let mut seq: u32 = 1;
    let mut timestamp: u32 = 0;
    let mut opus_buf = [0u8; 1024];

    // RNNoise-Denoiser (lazy, damit er nur bei Bedarf Speicher/CPU nutzt).
    const NS_FRAME: usize = DenoiseState::FRAME_SIZE; // 480 Samples = 10 ms @ 48 kHz
    let mut denoiser: Option<Box<DenoiseState<'static>>> = None;
    let mut denoiser_skip_first = false;
    let mut ns_in = [0f32; NS_FRAME];
    let mut ns_out = [0f32; NS_FRAME];
    let mut ns_frame = [0f32; FRAME_SAMPLES];
    let mut out = [0f32; FRAME_SAMPLES];
    let mut dc = DcBlocker::default();
    let mut agc = Agc::default();

    while !stop.load(Ordering::Relaxed) {
        while have < FRAME_SAMPLES {
            match cons.try_pop() {
                Some(s) => {
                    frame[have] = s;
                    have += 1;
                }
                None => break,
            }
        }
        if have < FRAME_SAMPLES {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        have = 0;

        // 0) Gleichanteil entfernen (billige Mikros haben oft DC-Offset).
        dc.process(&mut frame);

        // 1) KI-Rauschunterdrueckung (RNNoise). Laeuft immer, wenn aktiviert,
        //    damit das Modell einen lueckenlosen Stream sieht.
        let ns_on = control.noise_suppression.load(Ordering::Relaxed);
        let mut speech_prob = 0.0f32;
        if ns_on {
            if denoiser.is_none() {
                denoiser = Some(DenoiseState::new());
                denoiser_skip_first = true;
            }
            let d = denoiser.as_mut().expect("Denoiser");
            for chunk in 0..(FRAME_SAMPLES / NS_FRAME) {
                let s = chunk * NS_FRAME;
                for i in 0..NS_FRAME {
                    ns_in[i] = frame[s + i] * 32768.0; // RNNoise erwartet i16-Bereich
                }
                // Hoechste Sprachwahrscheinlichkeit beider 10-ms-Haelften.
                speech_prob = speech_prob.max(d.process_frame(&mut ns_out, &ns_in));
                for i in 0..NS_FRAME {
                    ns_frame[s + i] = ns_out[i] / 32768.0;
                }
            }
            if denoiser_skip_first {
                // Erster Output enthaelt Fade-in-Artefakte -> verwerfen.
                denoiser_skip_first = false;
                timestamp = timestamp.wrapping_add(FRAME_SAMPLES as u32);
                continue;
            }
        }
        let pcm: &[f32] = if ns_on { &ns_frame } else { &frame };

        // 2) Gate: Mute hat Vorrang, dann PTT, dann VAD (RMS oder RNNoise-Wahrscheinlichkeit).
        let level = rms(pcm);
        let vad_speech = control.voice_activation.load(Ordering::Relaxed) && {
            gate.threshold = control.vad_threshold();
            gate.process(level) || (ns_on && speech_prob > 0.6)
        };
        let send = !control.self_muted.load(Ordering::Relaxed)
            && (control.ptt_pressed.load(Ordering::Relaxed) || vad_speech);
        control.transmitting.store(send, Ordering::Relaxed);

        if send {
            // 3) Automatische Lautstaerke + weicher Limiter (nach dem Gate, damit
            //    die VAD-Schwelle weiter zum Pegelmesser in den Einstellungen passt).
            out.copy_from_slice(pcm);
            if control.auto_gain.load(Ordering::Relaxed) {
                let speech = if ns_on { speech_prob > 0.5 } else { level > control.vad_threshold() };
                agc.process(&mut out, level, speech);
                for s in out.iter_mut() {
                    *s = soft_limit(*s);
                }
            }
            match encoder.encode_float(&out[..], &mut opus_buf) {
                Ok(len) => {
                    let pkt = encode_to_vec(&AudioHeader::new(token, seq, timestamp), &opus_buf[..len]);
                    let _ = udp.send(&pkt);
                    seq = seq.wrapping_add(1);
                }
                Err(e) => tracing::warn!("Opus-Encode fehlgeschlagen: {e}"),
            }
        }
        timestamp = timestamp.wrapping_add(FRAME_SAMPLES as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_values() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 960]), 0.0);
        assert!((rms(&[0.5; 960]) - 0.5).abs() < 1e-6);
        assert!((rms(&[1.0, -1.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn gate_activates_above_threshold() {
        let mut gate = VoiceGate::new(0.05, 3);
        assert!(!gate.process(0.01), "leise = kein Senden");
        assert!(gate.process(0.10), "laut = Senden");
        assert!(gate.is_active());
    }

    #[test]
    fn gate_hangover_keeps_sending_then_stops() {
        let mut gate = VoiceGate::new(0.05, 3);
        assert!(gate.process(0.10)); // aktivieren
        assert!(gate.process(0.0), "Hangover 1");
        assert!(gate.process(0.0), "Hangover 2");
        assert!(gate.process(0.0), "Hangover 3");
        assert!(!gate.process(0.0), "nach Hangover: aus");
    }

    #[test]
    fn gate_reactivation_resets_hangover() {
        let mut gate = VoiceGate::new(0.05, 2);
        gate.process(0.10);
        gate.process(0.0); // cooldown 1
        gate.process(0.10); // reset
        assert!(gate.process(0.0));
        assert!(gate.process(0.0));
        assert!(!gate.process(0.0));
    }

    #[test]
    fn denoise_processes_frames() {
        // 10-ms-Frames (480 Samples @ 48 kHz), Werte im i16-Bereich.
        assert_eq!(DenoiseState::FRAME_SIZE, 480);
        let mut d = DenoiseState::new();
        let input = [2000.0f32; DenoiseState::FRAME_SIZE];
        let mut out = [0.0f32; DenoiseState::FRAME_SIZE];
        for _ in 0..5 {
            let prob = d.process_frame(&mut out, &input);
            assert!((0.0..=1.0).contains(&prob), "Wahrscheinlichkeit im Bereich");
        }
        assert!(out.iter().all(|x| x.is_finite()));
    }
}
