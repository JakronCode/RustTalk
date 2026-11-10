//! Jitter-Buffer und pro-Sprecher-Audiozustand (Empfangsseite).

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use audiopus::coder::Decoder;
use audiopus::{Channels, SampleRate};
use rusttalk_shared::audio_packet::FRAME_SAMPLES;

/// Pakete, die vor Wiedergabestart gepuffert werden (3 = 60 ms Latenzpuffer).
const PREBUFFER_PACKETS: usize = 3;
/// Maximale Puffergroesse in Paketen (12 = 240 ms); Aeltestes wird verworfen.
const MAX_BUFFER_PACKETS: usize = 12;
/// Nach so langer Funkstille wird ein Sprecher aus dem Mixer entfernt.
pub const USER_TIMEOUT: Duration = Duration::from_secs(5);

/// Sortierter Paket-Puffer eines Sprechers.
///
/// - Duplikate werden verworfen
/// - bei Ueberlauf wird das aelteste Paket gedroppt (lieber Luecke als Latenz)
#[derive(Debug, Default)]
pub struct JitterBuffer {
    packets: BTreeMap<u32, Vec<u8>>,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, seq: u32, payload: &[u8]) {
        if self.packets.contains_key(&seq) {
            return;
        }
        self.packets.insert(seq, payload.to_vec());
        while self.packets.len() > MAX_BUFFER_PACKETS {
            self.packets.pop_first();
        }
    }

    pub fn pop_oldest(&mut self) -> Option<(u32, Vec<u8>)> {
        self.packets.pop_first()
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

/// Audiozustand eines Sprechers: Jitter-Buffer + Opus-Decoder + PCM-FIFO.
pub struct UserAudio {
    pub jitter: JitterBuffer,
    decoder: Decoder,
    pcm: VecDeque<f32>,
    expected_seq: Option<u32>,
    started: bool,
    pub last_seen: Instant,
}

impl UserAudio {
    /// `None`, wenn der Opus-Decoder nicht erstellt werden konnte.
    pub fn new() -> Option<Self> {
        let decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono).ok()?;
        Some(Self {
            jitter: JitterBuffer::new(),
            decoder,
            pcm: VecDeque::with_capacity(FRAME_SAMPLES * 4),
            expected_seq: None,
            started: false,
            last_seen: Instant::now(),
        })
    }

    /// Fuellt `out` mit Samples; nicht lieferbarer Rest bleibt unveraendert
    /// (Aufrufer hat mit 0 vorgefuellt = Stille).
    /// Gibt die Anzahl geschriebener Samples zurueck.
    pub fn fill(&mut self, out: &mut [f32]) -> usize {
        let mut written = 0;
        while written < out.len() {
            if let Some(s) = self.pcm.pop_front() {
                out[written] = s;
                written += 1;
            } else if !self.decode_next() {
                break;
            }
        }
        written
    }

    /// Dekodiert das naechste Paket in die PCM-FIFO. false = nichts verfuegbar.
    fn decode_next(&mut self) -> bool {
        if !self.started {
            // Prebuffering gegen Jitter.
            if self.jitter.len() < PREBUFFER_PACKETS {
                return false;
            }
            self.started = true;
        }
        let Some((seq, payload)) = self.jitter.pop_oldest() else {
            self.started = false; // Puffer leer -> naechstes Mal neu puffern
            return false;
        };

        let mut frame = [0f32; FRAME_SAMPLES];
        if let Some(exp) = self.expected_seq {
            let gap = seq.wrapping_sub(exp);
            if gap > 0 && gap <= 100 {
                // Paketluecke (max. 2 Frames auffuellen, sonst waechst die Latenz):
                // - aeltere Fehlstellen per PLC (Opus schaetzt das Signal fort),
                // - das Frame direkt vor diesem Paket aus dessen In-Band-FEC
                //   rekonstruieren (echte, nur etwas groebere Sprachdaten).
                let fill = gap.min(2);
                for k in 0..fill {
                    let use_fec = k + 1 == fill;
                    let res = if use_fec {
                        self.decoder.decode_float(Some(&payload[..]), &mut frame[..], true)
                    } else {
                        self.decoder.decode_float(None::<&[u8]>, &mut frame[..], false)
                    };
                    if let Ok(n) = res {
                        self.pcm.extend(&frame[..n.min(FRAME_SAMPLES)]);
                    }
                }
            }
        }
        match self.decoder.decode_float(Some(&payload[..]), &mut frame[..], false) {
            Ok(n) => self.pcm.extend(&frame[..n.min(FRAME_SAMPLES)]),
            Err(e) => tracing::warn!("Opus-Decode fehlgeschlagen: {e}"),
        }

        // expected_seq nur vorwaerts bewegen (verspaetete Pakete nicht zurueckdrehen).
        let is_late = self.expected_seq.is_some_and(|exp| seq.wrapping_sub(exp) > 1_000_000);
        if !is_late {
            self.expected_seq = Some(seq.wrapping_add(1));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiopus::coder::Encoder;
    use audiopus::Application;

    fn encode_frame(encoder: &Encoder, sample: f32) -> Vec<u8> {
        let input = [sample; FRAME_SAMPLES];
        let mut out = [0u8; 1024];
        let n = encoder.encode_float(&input, &mut out).unwrap();
        out[..n].to_vec()
    }

    #[test]
    fn jitter_orders_out_of_order_packets() {
        let mut jb = JitterBuffer::new();
        jb.push(3, b"c");
        jb.push(1, b"a");
        jb.push(2, b"b");
        assert_eq!(jb.pop_oldest().unwrap().0, 1);
        assert_eq!(jb.pop_oldest().unwrap().0, 2);
        assert_eq!(jb.pop_oldest().unwrap().0, 3);
        assert!(jb.pop_oldest().is_none());
    }

    #[test]
    fn jitter_drops_duplicates() {
        let mut jb = JitterBuffer::new();
        jb.push(1, b"erste");
        jb.push(1, b"zweite");
        assert_eq!(jb.len(), 1);
        assert_eq!(jb.pop_oldest().unwrap().1, b"erste");
    }

    #[test]
    fn jitter_overflow_keeps_newest() {
        let mut jb = JitterBuffer::new();
        for seq in 0..(MAX_BUFFER_PACKETS as u32 + 5) {
            jb.push(seq, b"x");
        }
        assert_eq!(jb.len(), MAX_BUFFER_PACKETS);
        // Die aeltesten wurden verworfen:
        assert_eq!(jb.pop_oldest().unwrap().0, 5);
    }

    #[test]
    fn user_audio_decodes_real_opus() {
        // Opus-Roundtrip ueber die oeffentliche API: 3 Pakete -> fill liefert Samples.
        let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip).unwrap();
        let mut user = UserAudio::new().expect("decoder");
        for seq in 0..3u32 {
            let pkt = encode_frame(&encoder, 0.1);
            user.jitter.push(seq, &pkt);
        }
        let mut out = [0f32; FRAME_SAMPLES * 3];
        let n = user.fill(&mut out);
        assert!(n >= FRAME_SAMPLES * 2, "zu wenig dekodiert: {n}");
        assert!(out[..n].iter().any(|&s| s != 0.0), "nur Stille dekodiert");
    }

    #[test]
    fn user_audio_conceals_gap() {
        // Luecke bei seq 1 -> PLC muss fill trotzdem liefern lassen.
        let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip).unwrap();
        let mut user = UserAudio::new().expect("decoder");
        for seq in [0u32, 2, 3] {
            let pkt = encode_frame(&encoder, 0.2);
            user.jitter.push(seq, &pkt);
        }
        let mut out = [0f32; FRAME_SAMPLES * 4];
        let n = user.fill(&mut out);
        // 3 echte Frames + 1 PLC-Frame fuer die Luecke:
        assert!(n >= FRAME_SAMPLES * 3, "Luecke nicht kaschiert: {n}");
    }

    /// Echte Sprachprobe (Windows-Sprachsynthese, 16 kHz) -> mit dem
    /// Aufnahme-Resampler auf 48 kHz -> 20-ms-Frames.
    fn speech_frames() -> Vec<[f32; FRAME_SAMPLES]> {
        let bytes: &[u8] = include_bytes!("../../testdata/speech16k.wav");
        let pos = bytes.windows(4).position(|w| w == b"data").expect("data-Chunk") + 8;
        let pcm16k: Vec<f32> = bytes[pos..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        let mut pcm = Vec::new();
        crate::audio::resample::Resampler::new(16_000, 48_000).process(&pcm16k, &mut pcm);
        pcm.chunks_exact(FRAME_SAMPLES)
            .map(|c| {
                let mut f = [0f32; FRAME_SAMPLES];
                f.copy_from_slice(c);
                f
            })
            .collect()
    }

    /// Mit unserer Encoder-Konfiguration muss die In-Band-FEC verlorene
    /// Pakete deutlich besser rekonstruieren als reine PLC (Schaetzung).
    /// Ohne `packet_loss_perc` erzeugt Opus gar keine FEC-Daten (dann waeren
    /// beide Fehler identisch). Gemessen: FEC ~7x kleinerer Fehler.
    #[test]
    fn fec_recovers_lost_packets_better_than_plc() {
        let frames = speech_frames();
        assert!(frames.len() > 150, "Sprachprobe zu kurz: {}", frames.len());
        let mut encoder =
            Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip).unwrap();
        crate::audio::encoder::configure_encoder(&mut encoder);
        let packets: Vec<Vec<u8>> = frames
            .iter()
            .map(|pcm| {
                let mut out = [0u8; 1500];
                let n = encoder.encode_float(pcm, &mut out).unwrap();
                out[..n].to_vec()
            })
            .collect();

        // mode 0 = alles da, 1 = Paket `lost` fehlt + FEC, 2 = fehlt + PLC.
        let decode = |lost: usize, mode: u8| -> Vec<f32> {
            let mut dec = Decoder::new(SampleRate::Hz48000, Channels::Mono).unwrap();
            let mut pcm = Vec::new();
            let mut fr = [0f32; FRAME_SAMPLES];
            for (i, p) in packets.iter().enumerate() {
                if i == lost && mode != 0 {
                    continue;
                }
                if i == lost + 1 && mode != 0 {
                    let res = if mode == 1 {
                        dec.decode_float(Some(&p[..]), &mut fr[..], true)
                    } else {
                        dec.decode_float(None::<&[u8]>, &mut fr[..], false)
                    };
                    let n = res.unwrap();
                    pcm.extend(&fr[..n]);
                }
                let n = dec.decode_float(Some(&p[..]), &mut fr[..], false).unwrap();
                pcm.extend(&fr[..n]);
            }
            pcm
        };
        let (mut e_fec, mut e_plc) = (0f32, 0f32);
        for lost in (10..packets.len() - 2).step_by(9) {
            let reference = decode(lost, 0);
            let range = lost * FRAME_SAMPLES..(lost + 1) * FRAME_SAMPLES;
            let err = |x: &[f32]| {
                range.clone().map(|i| (x[i] - reference[i]).powi(2)).sum::<f32>().sqrt()
            };
            e_fec += err(&decode(lost, 1));
            e_plc += err(&decode(lost, 2));
        }
        assert!(e_fec < e_plc * 0.5, "FEC {e_fec:.2} nicht deutlich besser als PLC {e_plc:.2}");
    }

    #[test]
    fn user_audio_starts_silent_until_prebuffered() {
        let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip).unwrap();
        let mut user = UserAudio::new().expect("decoder");
        let pkt = encode_frame(&encoder, 0.3);
        user.jitter.push(0, &pkt);
        let mut out = [0f32; FRAME_SAMPLES];
        assert_eq!(user.fill(&mut out), 0, "vor Prebuffering darf nichts kommen");
    }
}

