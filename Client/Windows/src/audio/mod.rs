//! Audio-Engine: kapselt Capture, Encoder, UDP-Empfang und Wiedergabe.
//!
//! Threads/Streams:
//! - cpal-Input-Callback  -> Ring-Puffer
//! - Encoder-Thread       -> Ring-Puffer -> Gate -> Opus -> UDP
//! - UDP-Empfangsthread   -> Jitter-Buffer (SharedMixer)
//! - cpal-Output-Callback -> Mixer -> Lautsprecher

pub mod aec;
pub mod capture;
pub mod dsp;
pub mod encoder;
pub mod jitter;
pub mod link;
pub mod neural;
pub mod playback;
pub mod resample;
pub mod screen_audio;
pub mod sfx;
pub mod speech;
pub mod voice_pack;

use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use ringbuf::traits::Split;
use ringbuf::HeapRb;
use rusttalk_shared::audio_packet::{decode, seal_packet, AudioHeader, Control, FLAG_CONTROL, SAMPLE_RATE};
use rusttalk_shared::secure::DIR_TO_SERVER;

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
    /// Automatische Lautstaerke (AGC + Limiter) auf der Sendeseite.
    pub auto_gain: AtomicBool,
    /// Echo cancellation (AEC3) on the microphone.
    pub echo_cancellation: AtomicBool,
    /// Playback volume (f32 bits, 1.0 = 100 %).
    output_volume_bits: AtomicU32,
    /// Manual microphone gain (f32 bits, 1.0 = 100 %), applied in the capture callback.
    input_volume: Arc<AtomicU32>,
    /// Aktueller Mikro-Pegel (RMS, geglaettet) – fuer die Pegel-Anzeige in den Einstellungen.
    mic_level: Arc<AtomicU32>,
    /// Wir senden gerade (Gate offen) – fuer den Selbst-Sprech-Indikator.
    pub transmitting: AtomicBool,
    /// Ping and packet-loss stats for the connection panel.
    pub link: Arc<link::LinkStats>,
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
            auto_gain: AtomicBool::new(true),
            echo_cancellation: AtomicBool::new(true),
            output_volume_bits: AtomicU32::new(1.0f32.to_bits()),
            input_volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            mic_level: Arc::new(AtomicU32::new(0)),
            transmitting: AtomicBool::new(false),
            link: Arc::new(link::LinkStats::default()),
        }
    }

    pub fn should_transmit(&self, vad_speech: bool) -> bool {
        !self.self_muted.load(Ordering::Relaxed)
            && !self.self_deafened.load(Ordering::Relaxed)
            && (self.ptt_pressed.load(Ordering::Relaxed) || vad_speech)
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

    pub fn output_volume(&self) -> f32 {
        f32::from_bits(self.output_volume_bits.load(Ordering::Relaxed))
    }

    pub fn set_output_volume(&self, value: f32) {
        self.output_volume_bits.store(value.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_input_volume(&self, value: f32) {
        self.input_volume.store(value.clamp(0.0, MAX_VOLUME).to_bits(), Ordering::Relaxed);
    }

    /// Shared handle for the capture callback (and the mic monitor).
    pub fn input_volume_handle(&self) -> Arc<AtomicU32> {
        self.input_volume.clone()
    }

    pub fn set_vad_threshold(&self, value: f32) {
        self.vad_threshold_bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// speaker_id -> Zeitpunkt des letzten Audiopakets (fuer den Talking-Indikator).
pub type TalkingMap = Arc<Mutex<HashMap<u64, Instant>>>;

/// Mixer key: one decoder per speaker and stream (voice / screen audio).
pub fn mixer_key(speaker: u64, screen: bool) -> u64 {
    (speaker << 1) | u64::from(screen)
}

/// Speaker of a mixer key.
pub fn mixer_speaker(key: u64) -> u64 {
    key >> 1
}

/// Mixer-State, geteilt zwischen UDP-Empfangsthread und Wiedergabe-Callback.
pub type SharedMixer = Arc<Mutex<HashMap<u64, UserAudio>>>;

/// Lokal stummgeschaltete Sprecher (nur clientseitig, kein Server-Eingriff).
pub type LocalMuteSet = Arc<Mutex<HashSet<u64>>>;

/// Per-speaker playback volume (client id -> gain, 1.0 = 100 %, max 2.0).
/// Local only; missing entry = 100 %.
pub type UserVolumeMap = Arc<Mutex<HashMap<u64, f32>>>;

/// Allowed range for per-user and input volume.
pub const MAX_VOLUME: f32 = 2.0;

/// Laufende Audio-Pipeline. Beim Drop werden die Threads gestoppt.
pub struct AudioEngine {
    _input: cpal::Stream,
    _output: cpal::Stream,
    stop: Arc<AtomicBool>,
    /// For "share audio": own UDP socket clone + token, playback tap.
    udp: UdpSocket,
    token: u64,
    cipher: Arc<rusttalk_shared::secure::PacketCipher>,
    render_tap: screen_audio::RenderTap,
    /// Everything else `start` needs, for [`AudioEngine::restart`].
    shared: Shared,
}

/// Handles the engine shares with the app (cheap clones).
#[derive(Clone)]
struct Shared {
    control: Arc<AudioControl>,
    talking: TalkingMap,
    local_muted: LocalMuteSet,
    user_volumes: UserVolumeMap,
    sfx: sfx::SfxQueue,
}

impl AudioEngine {
    /// Startet die komplette Pipeline.
    ///
    /// `udp`: verbundener UDP-Socket (connect() auf die Server-Adresse).
    /// `token`: UDP-Token aus dem Welcome (Identifikation beim Server).
    pub fn start(
        udp: UdpSocket,
        token: u64,
        cipher: Arc<rusttalk_shared::secure::PacketCipher>,
        control: Arc<AudioControl>,
        talking: TalkingMap,
        local_muted: LocalMuteSet,
        user_volumes: UserVolumeMap,
        sfx: sfx::SfxQueue,
        input_device: Option<&str>,
        output_device: Option<&str>,
    ) -> Result<Self> {
        let shared = Shared {
            control: control.clone(),
            talking: talking.clone(),
            local_muted: local_muted.clone(),
            user_volumes: user_volumes.clone(),
            sfx: sfx.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let share_udp = udp.try_clone().context("UDP-Socket klonen")?;
        let render_tap: screen_audio::RenderTap = Arc::new(Mutex::new(None));
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
                let cipher = cipher.clone();
                let link = control.link.clone();
                move || recv_loop(recv_udp, token, mixer, talking, stop, cipher, link)
            })
            .context("Empfangsthread starten")?;

        // Capture: Mikro -> Ring-Puffer (mit Pegel-Sink fuer die UI).
        let rb = HeapRb::<f32>::new((SAMPLE_RATE * 2) as usize); // 2 s Puffer
        let (prod, cons) = rb.split();
        // Render reference for the echo canceller: playback -> encoder thread (1 s).
        let (render_prod, render_cons) = HeapRb::<f32>::new(SAMPLE_RATE as usize).split();
        let input = capture::build_input_stream(input_device, prod, Some(control.mic_level_sink()), Some(control.input_volume_handle()))
            .context("Could not start audio input")?;

        // Encoder-Thread: Ring-Puffer -> Opus -> UDP.
        std::thread::Builder::new()
            .name("audio-enc".to_string())
            .spawn({
                let stop = stop.clone();
                let control = control.clone();
                let cipher = cipher.clone();
                move || encoder::run(cons, udp, token, cipher, control, stop, render_cons)
            })
            .context("Encoder-Thread starten")?;

        // Wiedergabe: Mixer -> Lautsprecher.
        let output = playback::build_output_stream(output_device, mixer, control, local_muted, user_volumes, sfx, render_prod, render_tap.clone())
            .context("Audio-Ausgabe konnte nicht gestartet werden")?;

        Ok(Self { _input: input, _output: output, stop, udp: share_udp, token, cipher, render_tap, shared })
    }

    /// Starts the pipeline again with other devices (same socket, token and
    /// key - the server notices nothing but a few ms without audio). The old
    /// threads stop first so only one encoder sends; on an error the caller
    /// restarts with the previous devices.
    pub fn restart(&mut self, input_device: Option<&str>, output_device: Option<&str>) -> Result<()> {
        let udp = self.udp.try_clone().context("UDP-Socket klonen")?;
        let s = self.shared.clone();
        self.stop();
        let new = Self::start(udp, self.token, self.cipher.clone(), s.control, s.talking, s.local_muted, s.user_volumes, s.sfx, input_device, output_device)?;
        *self = new; // drops the old streams
        Ok(())
    }

    /// Starts sending the computer audio (screen share with sound).
    pub fn start_screen_audio(&self, output_device: Option<&str>) -> Result<screen_audio::ScreenAudio> {
        let udp = self.udp.try_clone().context("UDP-Socket klonen")?;
        screen_audio::ScreenAudio::start(udp, self.token, self.cipher.clone(), self.shared.control.link.clone(), output_device, self.render_tap.clone())
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

/// Sends an encrypted ping every [`link::PING_INTERVAL`].
fn send_ping(udp: &UdpSocket, token: u64, cipher: &rusttalk_shared::secure::PacketCipher, link: &link::LinkStats, pinger: &mut link::Pinger) {
    let now = Instant::now();
    if !pinger.due(now) {
        return;
    }
    let id = link.next_ping_id();
    let mut header = AudioHeader::new(token, id, 0);
    header.flags = FLAG_CONTROL;
    // Snapshot before sending: audio sent after the ping is not in the server's count yet.
    pinger.sent(id, now, link.audio_sent.load(Ordering::Relaxed));
    let _ = udp.send(&seal_packet(&header, cipher, DIR_TO_SERVER, &Control::Ping.encode()));
}

/// Empfangsschleife: UDP-Pakete in die Jitter-Buffer der Sprecher einsortieren;
/// sends the pings and evaluates the pongs.
fn recv_loop(
    udp: UdpSocket,
    token: u64,
    mixer: SharedMixer,
    talking: TalkingMap,
    stop: Arc<AtomicBool>,
    cipher: Arc<rusttalk_shared::secure::PacketCipher>,
    link: Arc<link::LinkStats>,
) {
    let mut buf = [0u8; rusttalk_shared::audio_packet::MAX_PACKET_LEN];
    let mut pinger = link::Pinger::default();
    let session = link.session();
    while !stop.load(Ordering::Relaxed) {
        send_ping(&udp, token, &cipher, &link, &mut pinger);
        match udp.recv(&mut buf) {
            Ok(n) => {
                let Some((header, _)) = decode(&buf[..n]) else { continue };
                // Forged or foreign packets fail the tag check and are dropped.
                let Some(payload) = rusttalk_shared::audio_packet::open_packet(&header, &buf[..n], &cipher, rusttalk_shared::secure::DIR_TO_CLIENT) else {
                    continue;
                };
                match link.accept(session, header.counter) {
                    None => break, // a newer session took over
                    Some(false) => continue, // replayed
                    Some(true) => {}
                }
                if header.flags & FLAG_CONTROL != 0 {
                    if let (0, Some(Control::Pong { audio_received, audio_relayed })) = (header.token, Control::decode(&payload)) {
                        let received = link.audio_received.load(Ordering::Relaxed);
                        if let Some(sample) = pinger.pong(header.sequence, Instant::now(), audio_received, audio_relayed, received) {
                            link.record(session, sample);
                        }
                    }
                    continue;
                }
                let payload = payload.as_slice();
                if payload.is_empty() {
                    continue; // Registrierungs-Ping o. ae.
                }
                link.audio_received.fetch_add(1, Ordering::Relaxed);
                let speaker = header.token; // Relay: Token-Feld = client_id des Sprechers
                let screen = header.flags & rusttalk_shared::audio_packet::FLAG_SCREEN_AUDIO != 0;
                let key = mixer_key(speaker, screen);
                {
                    let mut map = mixer.lock().unwrap();
                    if !map.contains_key(&key) {
                        match UserAudio::new() {
                            Some(u) => {
                                map.insert(key, u);
                            }
                            None => {
                                tracing::error!("Opus-Decoder fuer Sprecher {speaker} nicht erstellbar");
                                continue;
                            }
                        }
                    }
                    if let Some(user) = map.get_mut(&key) {
                        user.jitter.push(header.sequence, payload);
                        user.last_seen = Instant::now();
                    }
                }
                if !screen {
                    talking.lock().unwrap().insert(speaker, Instant::now());
                }
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

#[cfg(test)]
mod shortcut_audio_tests {
    use super::*;
    #[test]
    fn deafen_blocks_transmission_even_when_mute_flag_is_cleared() {
        let control = AudioControl::new(false, 0.02);
        control.ptt_pressed.store(true, Ordering::Relaxed);
        assert!(control.should_transmit(false));
        control.self_deafened.store(true, Ordering::Relaxed);
        assert!(!control.should_transmit(false));
        assert!(!control.should_transmit(true));
        control.self_deafened.store(false, Ordering::Relaxed);
        control.self_muted.store(true, Ordering::Relaxed);
        assert!(!control.should_transmit(true));
    }
}
