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
                // Paketluecke: bis zu 2 Frames per PLC kaschieren.
                for _ in 0..gap.min(2) {
                    if self.decoder.decode_float(None::<&[u8]>, &mut frame[..], false).is_ok() {
                        self.pcm.extend(frame);
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
