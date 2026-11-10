//! Audio-Engine: kapselt Capture, Encoder, UDP-Empfang und Wiedergabe.
//!
//! Threads/Streams:
//! - cpal-Input-Callback  -> Ring-Puffer
//! - Encoder-Thread       -> Ring-Puffer -> Gate -> Opus -> UDP
//! - UDP-Empfangsthread   -> Jitter-Buffer (SharedMixer)
//! - cpal-Output-Callback -> Mixer -> Lautsprecher

pub mod capture;
pub mod encoder;
pub mod jitter;
pub mod playback;
pub mod resample;

use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use ringbuf::traits::Split;
use ringbuf::HeapRb;
use rusttalk_shared::audio_packet::{decode, SAMPLE_RATE};

use self::jitter::UserAudio;

/// Geteilter Steuerzustand zwischen UI und Audio-Threads.
#[derive(Debug)]
pub struct AudioControl {
    /// Push-to-Talk-Taste gerade gedrueckt.
    pub ptt_pressed: AtomicBool,
    /// Sprachaktivierung statt PTT.
    pub voice_activation: AtomicBool,
    vad_threshold_bits: AtomicU32,
    /// Eigenes Mikro stumm.
    pub self_muted: AtomicBool,
    /// Eigene Wiedergabe stumm ("taub").
    pub self_deafened: AtomicBool,
    /// KI-Rauschunterdrueckung (RNNoise) vor dem Opus-Encoder.
    pub noise_suppression: AtomicBool,
    /// Aktueller Mikro-Pegel (RMS, geglaettet) – fuer die Pegel-Anzeige in den Einstellungen.
    mic_level: Arc<AtomicU32>,
    /// Wir senden gerade (Gate offen) – fuer den Selbst-Sprech-Indikator.
    pub transmitting: AtomicBool,
}

impl AudioControl {
    pub fn new(voice_activation: bool, vad_threshold: f32) -> Self {
        Self {
            ptt_pressed: AtomicBool::new(false),
            voice_activation: AtomicBool::new(voice_activation),
            vad_threshold_bits: AtomicU32::new(vad_threshold.to_bits()),
            self_muted: AtomicBool::new(false),
            self_deafened: AtomicBool::new(false),
            noise_suppression: AtomicBool::new(true),
            mic_level: Arc::new(AtomicU32::new(0)),
            transmitting: AtomicBool::new(false),
        }
    }

    pub fn mic_level(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Ordering::Relaxed))
    }

    pub fn mic_level_sink(&self) -> Arc<AtomicU32> {
        self.mic_level.clone()
    }

    pub fn vad_threshold(&self) -> f32 {
        f32::from_bits(self.vad_threshold_bits.load(Ordering::Relaxed))
    }

    pub fn set_vad_threshold(&self, value: f32) {
        self.vad_threshold_bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// speaker_id -> Zeitpunkt des letzten Audiopakets (fuer den Talking-Indikator).
pub type TalkingMap = Arc<Mutex<HashMap<u64, Instant>>>;

/// Mixer-State, geteilt zwischen UDP-Empfangsthread und Wiedergabe-Callback.
pub type SharedMixer = Arc<Mutex<HashMap<u64, UserAudio>>>;

/// Lokal stummgeschaltete Sprecher (nur clientseitig, kein Server-Eingriff).
pub type LocalMuteSet = Arc<Mutex<HashSet<u64>>>;

/// Laufende Audio-Pipeline. Beim Drop werden die Threads gestoppt.
pub struct AudioEngine {
    _input: cpal::Stream,
    _output: cpal::Stream,
    stop: Arc<AtomicBool>,
}

impl AudioEngine {
    /// Startet die komplette Pipeline.
    ///
    /// `udp`: verbundener UDP-Socket (connect() auf die Server-Adresse).
    /// `token`: UDP-Token aus dem Welcome (Identifikation beim Server).
    pub fn start(
        udp: UdpSocket,
        token: u64,
        control: Arc<AudioControl>,
        talking: TalkingMap,
        local_muted: LocalMuteSet,
        input_device: Option<&str>,
        output_device: Option<&str>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mixer: SharedMixer = Arc::new(Mutex::new(HashMap::new()));

        // UDP-Empfangsthread.
        let recv_udp = udp.try_clone().context("UDP-Socket klonen")?;
        recv_udp.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
        std::thread::Builder::new()
            .name("audio-recv".to_string())
            .spawn({
                let mixer = mixer.clone();
                let talking = talking.clone();
                let stop = stop.clone();
                move || recv_loop(recv_udp, mixer, talking, stop)
            })
            .context("Empfangsthread starten")?;

        // Capture: Mikro -> Ring-Puffer (mit Pegel-Sink fuer die UI).
        let rb = HeapRb::<f32>::new((SAMPLE_RATE * 2) as usize); // 2 s Puffer
        let (prod, cons) = rb.split();
        let input = capture::build_input_stream(input_device, prod, Some(control.mic_level_sink()))
            .context("Audio-Eingabe konnte nicht gestartet werden")?;

        // Encoder-Thread: Ring-Puffer -> Opus -> UDP.
        std::thread::Builder::new()
            .name("audio-enc".to_string())
            .spawn({
                let stop = stop.clone();
                let control = control.clone();
                move || encoder::run(cons, udp, token, control, stop)
            })
            .context("Encoder-Thread starten")?;

        // Wiedergabe: Mixer -> Lautsprecher.
        let output = playback::build_output_stream(output_device, mixer, control, local_muted)
            .context("Audio-Ausgabe konnte nicht gestartet werden")?;

        Ok(Self { _input: input, _output: output, stop })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Empfangsschleife: UDP-Pakete in die Jitter-Buffer der Sprecher einsortieren.
fn recv_loop(udp: UdpSocket, mixer: SharedMixer, talking: TalkingMap, stop: Arc<AtomicBool>) {
    let mut buf = [0u8; rusttalk_shared::audio_packet::MAX_PACKET_LEN];
    while !stop.load(Ordering::Relaxed) {
        match udp.recv(&mut buf) {
            Ok(n) => {
                let Some((header, payload)) = decode(&buf[..n]) else { continue };
                if payload.is_empty() {
                    continue; // Registrierungs-Ping o. ae.
                }
                let speaker = header.token; // Relay: Token-Feld = client_id des Sprechers
                {
                    let mut map = mixer.lock().unwrap();
                    if !map.contains_key(&speaker) {
                        match UserAudio::new() {
                            Some(u) => {
                                map.insert(speaker, u);
                            }
                            None => {
                                tracing::error!("Opus-Decoder fuer Sprecher {speaker} nicht erstellbar");
                                continue;
                            }
                        }
                    }
                    if let Some(user) = map.get_mut(&speaker) {
                        user.jitter.push(header.sequence, payload);
                        user.last_seen = Instant::now();
                    }
                }
                talking.lock().unwrap().insert(speaker, Instant::now());
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue; // Read-Timeout -> Stop-Flag pruefen
            }
            Err(e) => {
                tracing::warn!("UDP-Empfangsfehler: {e}");
                break;
            }
        }
    }
}
