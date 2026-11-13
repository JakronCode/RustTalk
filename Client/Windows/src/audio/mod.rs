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
pub mod share_source;
#[cfg(windows)]
#[path = "../windows_audio.rs"]
mod windows_audio;
#[cfg(target_os = "linux")]
#[path = "../linux/system_audio.rs"]
mod system_audio;
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
    /// Incoming streams seen by playback ([`mixer_key`] -> level), for the volume mixer.
    pub stream_meters: Mutex<HashMap<u64, StreamMeter>>,
    /// Single streams muted from the volume mixer ([`mixer_key`]s, session only).
    pub muted_streams: Mutex<HashSet<u64>>,
}

/// Last playback level of one incoming stream.
#[derive(Clone, Copy, Debug)]
pub struct StreamMeter {
    /// Peak of the last 20 ms after the stream's own volume (1.0 = full scale).
    pub level: f32,
    /// Last packet of this stream.
    pub heard: Instant,
}

/// Kind of an incoming audio stream: one mixer channel per user and kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamKind {
    Voice,
    /// Computer audio (screen share with sound, or "share audio" alone).
    Shared,
}

impl StreamKind {
    pub const ALL: [StreamKind; 2] = [StreamKind::Voice, StreamKind::Shared];

    pub fn key(self, speaker: u64) -> u64 {
        mixer_key(speaker, self == StreamKind::Shared)
    }
}

/// Meter entries older than this are dropped (the mixer row disappears earlier).
const METER_KEEP: std::time::Duration = std::time::Duration::from_secs(60);

impl AudioControl {
    /// Records the level of one mixed chunk; never blocks the audio thread.
    pub(crate) fn note_stream_levels(&self, levels: &[(u64, f32, Instant)]) {
        let Ok(mut meters) = self.stream_meters.try_lock() else { return };
        let now = Instant::now();
        for &(key, level, heard) in levels {
            meters.insert(key, StreamMeter { level, heard });
        }
        meters.retain(|_, m| now.duration_since(m.heard) < METER_KEEP);
    }

    /// Forgets meters and stream mutes (new voice server: client ids name other people).
    pub fn clear_streams(&self) {
        self.stream_meters.lock().unwrap().clear();
        self.muted_streams.lock().unwrap().clear();
    }

    /// Forgets the streams of one speaker who left.
    pub fn forget_speaker(&self, speaker: u64) {
        let keys = StreamKind::ALL.map(|k| k.key(speaker));
        self.stream_meters.lock().unwrap().retain(|k, _| !keys.contains(k));
        self.muted_streams.lock().unwrap().retain(|k| !keys.contains(k));
    }
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
            stream_meters: Mutex::new(HashMap::new()),
            muted_streams: Mutex::new(HashSet::new()),
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

/// Per-stream playback volume ([`mixer_key`] -> gain, 1.0 = 100 %, max 2.0).
/// Local only; missing entry = 100 %.
pub type UserVolumeMap = Arc<Mutex<HashMap<u64, f32>>>;

/// Allowed range for per-user and input volume.
pub const MAX_VOLUME: f32 = 2.0;

// Tests replace only device creation; lifecycle and network workers still run.
#[cfg(not(test))]
pub type DeviceStream = cpal::Stream;
#[cfg(test)]
pub enum DeviceStream {
    Hardware { _stream: cpal::Stream },
    Fake { _stream: test_devices::Stream },
}
#[cfg(test)]
pub(crate) mod test_devices;

/// Connection-owned transport. Keep it when voice stops: the cipher owns the nonce counter.
pub struct AudioSession {
    pub udp: UdpSocket,
    pub token: u64,
    pub cipher: Arc<rusttalk_shared::secure::PacketCipher>,
}

/// Laufende Audio-Pipeline. Beim Drop werden die Threads gestoppt.
pub struct AudioEngine {
    pub voice_channel: Option<u64>,
    input: Option<DeviceStream>,
    output: Option<DeviceStream>,
    workers: AudioWorkers,
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
    /// Starts playback and the receiver; capture exists only for a confirmed voice channel.
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
        voice_channel: Option<u64>,
    ) -> Result<Self> {
        let shared = Shared {
            control: control.clone(),
            talking: talking.clone(),
            local_muted: local_muted.clone(),
            user_volumes: user_volumes.clone(),
            sfx: sfx.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        // Any early return below stops the threads already spawned.
        let mut workers = AudioWorkers { stop: stop.clone(), handles: Vec::new() };
        // Bound now, not in the thread: a late-starting thread must not adopt a newer session.
        let session = control.link.session();
        let share_udp = udp.try_clone().context("UDP-Socket klonen")?;
        let render_tap: screen_audio::RenderTap = Arc::new(Mutex::new(None));
        let mixer: SharedMixer = Arc::new(Mutex::new(HashMap::new()));

        // UDP-Empfangsthread.
        let recv_udp = udp.try_clone().context("UDP-Socket klonen")?;
        recv_udp.set_read_timeout(Some(std::time::Duration::from_millis(50)))?;
        workers.handles.push(std::thread::Builder::new()
            .name("audio-recv".to_string())
            .spawn({
                let mixer = mixer.clone();
                let talking = talking.clone();
                let stop = stop.clone();
                let cipher = cipher.clone();
                let link = control.link.clone();
                move || recv_loop(recv_udp, token, mixer, talking, stop, cipher, link, session, voice_channel.is_some())
            })
            .context("Empfangsthread starten")?);

        let (render_prod, render_cons) = HeapRb::<f32>::new(SAMPLE_RATE as usize).split();
        // Output-only startup must never construct capture or an encoder.
        let input = if voice_channel.is_some() {
            let (prod, cons) = HeapRb::<f32>::new((SAMPLE_RATE * 2) as usize).split();
            let input = capture::build_input_stream(
                input_device,
                prod,
                Some(control.mic_level_sink()),
                Some(control.input_volume_handle()),
            )
            .context("Could not start audio input")?;
            workers.handles.push(
                std::thread::Builder::new()
                    .name("audio-enc".to_string())
                    .spawn({
                        let stop = stop.clone();
                        let control = control.clone();
                        let cipher = cipher.clone();
                        move || encoder::run(cons, udp, token, cipher, control, stop, render_cons)
                    })
                    .context("Encoder-Thread starten")?,
            );
            Some(input)
        } else {
            None
        };

        // Wiedergabe: Mixer -> Lautsprecher.
        let output = playback::build_output_stream(output_device, mixer, control, local_muted, user_volumes, sfx, render_prod, render_tap.clone())
            .context("Audio-Ausgabe konnte nicht gestartet werden")?;

        Ok(Self {
            voice_channel,
            input,
            output: Some(output),
            workers,
            udp: share_udp,
            token,
            cipher,
            render_tap,
            shared,
        })
    }

    /// Starts the pipeline again with other devices (same socket, token and
    /// key - the server notices nothing but a few ms without audio). The old
    /// threads stop first so only one encoder sends; on an error the caller
    /// restarts with the previous devices.
    pub fn restart(
        &mut self,
        input_device: Option<&str>,
        output_device: Option<&str>,
        voice_channel: Option<u64>,
    ) -> Result<()> {
        let udp = self.udp.try_clone().context("UDP-Socket klonen")?;
        let s = self.shared.clone();
        self.stop();
        let new = Self::start(
            udp,
            self.token,
            self.cipher.clone(),
            s.control,
            s.talking,
            s.local_muted,
            s.user_volumes,
            s.sfx,
            input_device,
            output_device,
            voice_channel,
        )?;
        *self = new; // drops the old streams
        Ok(())
    }

    /// Starts sending the computer audio (screen share with sound).
    #[cfg(test)]
    pub fn start_screen_audio(&self, output_device: Option<&str>) -> Result<screen_audio::ScreenAudio> {
        anyhow::ensure!(self.voice_channel.is_some(), "Join a voice channel before sharing audio");
        self.start_shared_audio(output_device, &share_source::AudioShareSource::System)
    }

    pub fn start_shared_audio(&self, output_device: Option<&str>, source: &share_source::AudioShareSource) -> Result<screen_audio::ScreenAudio> {
        anyhow::ensure!(self.voice_channel.is_some(), "Join a voice channel before sharing audio");
        let udp = self.udp.try_clone().context("UDP-Socket klonen")?;
        screen_audio::ScreenAudio::start_source(udp, self.token, self.cipher.clone(), self.shared.control.link.clone(), output_device, self.render_tap.clone(), source)
    }

    fn stop(&mut self) {
        self.workers.stop.store(true, Ordering::Relaxed);
        self.input = None; // release capture before waiting for the workers
        self.output = None;
        self.workers.join();
        self.shared.control.transmitting.store(false, Ordering::Relaxed);
        self.shared.control.mic_level.store(0, Ordering::Relaxed);
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Also joins on a partial startup failure, before another engine can use the socket.
struct AudioWorkers {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl AudioWorkers {
    fn join(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for AudioWorkers {
    fn drop(&mut self) {
        self.join();
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
#[allow(clippy::too_many_arguments)]
fn recv_loop(
    udp: UdpSocket,
    token: u64,
    mixer: SharedMixer,
    talking: TalkingMap,
    stop: Arc<AtomicBool>,
    cipher: Arc<rusttalk_shared::secure::PacketCipher>,
    link: Arc<link::LinkStats>,
    session: u64,
    in_voice: bool,
) {
    let mut buf = [0u8; rusttalk_shared::audio_packet::MAX_PACKET_LEN];
    let mut pinger = link::Pinger::default();
    // Drain on the receiver, not the UI thread. Under a flood stay fail-closed
    // until the socket is empty, yielding every bounded batch.
    if udp
        .set_read_timeout(Some(std::time::Duration::from_millis(1)))
        .is_err()
    {
        return;
    }
    let mut draining = true;
    let mut batch = 0;

    // Also ends when a newer session starts, even if nothing arrives (no orphaned pinger).
    while !stop.load(Ordering::Relaxed) && link.session() == session {
        if !draining {
            send_ping(&udp, token, &cipher, &link, &mut pinger);
        }
        batch += 1;
        if batch == 256 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            batch = 0;
        }
        match udp.recv(&mut buf) {
            Ok(n) => {
                if stop.load(Ordering::Relaxed) { break; }
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
                // Authenticate and advance replay state even when discarding.
                // shortcut: late packets after this drain have no channel epoch;
                // strict attribution across joins requires a protocol change.
                if draining || !in_voice {
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
                if draining {
                    if udp
                        .set_read_timeout(Some(std::time::Duration::from_millis(50)))
                        .is_err()
                    {
                        break;
                    }
                    draining = false;
                }
                continue; // Read-Timeout -> Stop-Flag pruefen
            }
            // Winsock discards oversized UDP datagrams and reports WSAEMSGSIZE.
            #[cfg(windows)]
            Err(e) if e.raw_os_error() == Some(10040) => continue,
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

#[cfg(test)]
mod session_tests {
    use super::*;

    /// Runs `recv_loop` on a socket that never gets an answer; returns its join handle.
    fn spawn_recv(link: Arc<link::LinkStats>, session: u64) -> std::thread::JoinHandle<()> {
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.connect(peer.local_addr().unwrap()).unwrap();
        udp.set_read_timeout(Some(std::time::Duration::from_millis(50))).unwrap();
        let cipher = Arc::new(rusttalk_shared::secure::PacketCipher::new(&[7u8; 32]).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || {
            let _peer = peer;
            recv_loop(
                udp,
                1,
                Arc::default(),
                Arc::default(),
                stop,
                cipher,
                link,
                session,
                true,
            )
        })
    }

    fn joins_soon(h: std::thread::JoinHandle<()>) -> bool {
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while !h.is_finished() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        h.join().is_ok()
    }

    #[test]
    fn voice_rejoin_keeps_nonce_and_replay_state_and_joins_receiver() {
        use rusttalk_shared::secure::{PacketCipher, DIR_TO_CLIENT};
        use std::time::Duration;
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.connect(peer.local_addr().unwrap()).unwrap();
        udp.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        peer.connect(udp.local_addr().unwrap()).unwrap();
        let transport = AudioSession { udp, token: 42, cipher: Arc::new(PacketCipher::new(&[7; 32]).unwrap()) };
        let server_cipher = PacketCipher::new(&[7; 32]).unwrap();
        let link = Arc::new(link::LinkStats::default());
        link.reset();
        let session = link.session();
        let packet = seal_packet(&AudioHeader::new(9, 1, 0), &server_cipher, DIR_TO_CLIENT, &[1]);
        let mut previous_counter = 0;
        for cycle in 0..2 {
            let stop = Arc::new(AtomicBool::new(false));
            let recv = transport.udp.try_clone().unwrap();
            let cipher = transport.cipher.clone();
            let stats = link.clone();
            let stopped = stop.clone();
            let worker = std::thread::spawn(move || recv_loop(
                recv, 42, Arc::default(), Arc::default(), stopped, cipher, stats, session, true,
            ));
            let workers = AudioWorkers { stop, handles: vec![worker] };
            let mut buf = [0; 2048];
            let n = peer.recv(&mut buf).unwrap(); // encrypted ping from each receiver
            let (header, _) = decode(&buf[..n]).unwrap();
            assert!(header.counter > previous_counter, "never reuse a nonce on rejoin");
            previous_counter = header.counter;
            peer.send(&packet).unwrap(); // first valid, then a replay after leaving voice
            if cycle == 1 {
                let mut forged = packet.clone();
                *forged.last_mut().unwrap() ^= 1;
                peer.send(&forged).unwrap();
                peer.send(&[0; 3]).unwrap(); // truncated
                let fresh = seal_packet(&AudioHeader::new(9, 2, 0), &server_cipher, DIR_TO_CLIENT, &[1]);
                peer.send(&fresh).unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while link.audio_received.load(Ordering::Relaxed) < cycle + 1 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let stopped = workers.stop.clone();
            drop(workers); // synchronous join, including on a failed engine startup
            assert!(stopped.load(Ordering::Relaxed));
            assert_eq!(link.audio_received.load(Ordering::Relaxed), cycle + 1, "replay and malformed packets rejected");
            assert_eq!(link.session(), session);
        }
        link.reset(); // reconnect is the only replay reset
        assert_eq!(link.accept(session, 3), None);
        assert_eq!(link.accept(link.session(), 1), Some(true));
    }

    #[test]
    fn inactive_receiver_and_join_drain_discard_unseen_audio_without_resetting_replay() {
        use rusttalk_shared::secure::{PacketCipher, DIR_TO_CLIENT};
        use std::time::Duration;

        let devices = test_devices::Devices::new();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.connect(peer.local_addr().unwrap()).unwrap();
        peer.connect(udp.local_addr().unwrap()).unwrap();
        let cipher = Arc::new(PacketCipher::new(&[7; 32]).unwrap());
        let server = PacketCipher::new(&[7; 32]).unwrap();
        let control = Arc::new(AudioControl::new(false, 0.02));
        let talking: TalkingMap = Arc::default();
        let start = |channel| {
            AudioEngine::start(
                udp.try_clone().unwrap(),
                42,
                cipher.clone(),
                control.clone(),
                talking.clone(),
                Arc::default(),
                Arc::default(),
                Arc::default(),
                None,
                None,
                channel,
            )
            .unwrap()
        };
        let ping = || {
            let mut buf = [0; 2048];
            loop {
                let n = peer.recv(&mut buf).unwrap();
                let (header, _) = decode(&buf[..n]).unwrap();
                if header.flags & FLAG_CONTROL != 0 {
                    return header;
                }
            }
        };
        let encoder = audiopus::coder::Encoder::new(
            audiopus::SampleRate::Hz48000,
            audiopus::Channels::Mono,
            audiopus::Application::Voip,
        )
        .unwrap();
        let mut opus = [0; 1275];
        let len = encoder
            .encode_float(
                &[0.0; rusttalk_shared::audio_packet::FRAME_SAMPLES],
                &mut opus,
            )
            .unwrap();
        let packet = |speaker| {
            seal_packet(
                &AudioHeader::new(speaker, 1, 0),
                &server,
                DIR_TO_CLIENT,
                &opus[..len],
            )
        };
        let wait = |condition: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !condition() {
                assert!(
                    Instant::now() < deadline,
                    "receiver did not process barrier"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        };

        let lobby = start(None);
        let first_ping = ping();
        assert_eq!(devices.0.borrow().input_starts, 0);
        let inactive = packet(10);
        peer.send(&inactive).unwrap();
        // A matched pong is a barrier after the inactive packet on this local socket.
        let mut header = AudioHeader::new(0, first_ping.sequence, 0);
        header.flags = FLAG_CONTROL;
        peer.send(&seal_packet(
            &header,
            &server,
            DIR_TO_CLIENT,
            &Control::Pong {
                audio_received: 0,
                audio_relayed: 0,
            }
            .encode(),
        ))
        .unwrap();
        wait(&|| !control.link.view(Instant::now()).0.is_empty());
        assert!(talking.lock().unwrap().is_empty());
        drop(lobby);

        // Packets queued during a transition (or a failed output start) must be drained too.
        let queued = packet(11);
        peer.send(&queued).unwrap();
        let valid_after_forgery = packet(12);
        let mut forged = valid_after_forgery.clone();
        *forged.last_mut().unwrap() ^= 1;
        peer.send(&forged).unwrap();
        peer.send(&[0; 3]).unwrap();
        peer.send(&[0; 2048]).unwrap();
        let voice = start(Some(2));
        let second_ping = ping(); // sent only once the startup drain finishes
        assert!(second_ping.counter > first_ping.counter);
        assert!(talking.lock().unwrap().is_empty());
        assert_eq!(control.link.audio_received.load(Ordering::Relaxed), 0);
        peer.send(&inactive).unwrap(); // inactive packet advanced the replay window
        peer.send(&queued).unwrap(); // so did the startup drain
        peer.send(&valid_after_forgery).unwrap(); // forged tag did not consume its counter
        wait(&|| talking.lock().unwrap().contains_key(&12));
        assert_eq!(talking.lock().unwrap().len(), 1);
        assert_eq!(control.link.audio_received.load(Ordering::Relaxed), 1);
        drop(voice);
        assert_eq!(devices.0.borrow().inputs, 0);
    }

    #[test]
    fn draining_a_large_queue_remains_stoppable() {
        use rusttalk_shared::secure::PacketCipher;
        use std::time::Duration;
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.connect(peer.local_addr().unwrap()).unwrap();
        peer.connect(udp.local_addr().unwrap()).unwrap();
        for _ in 0..512 {
            peer.send(&[0; 3]).unwrap();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let link = Arc::new(link::LinkStats::default());
        let session = link.session();
        let worker = std::thread::spawn(move || {
            recv_loop(
                udp,
                42,
                Arc::default(),
                Arc::default(),
                stopped,
                Arc::new(PacketCipher::new(&[7; 32]).unwrap()),
                link,
                session,
                true,
            )
        });
        std::thread::sleep(Duration::from_millis(1));
        stop.store(true, Ordering::Relaxed);
        assert!(joins_soon(worker));
    }

    #[test]
    fn receiver_stops_on_new_session_without_traffic() {
        let link = Arc::new(link::LinkStats::default());
        let h = spawn_recv(link.clone(), link.session());
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!h.is_finished(), "runs while its session is current");
        link.reset();
        assert!(joins_soon(h), "no orphaned pinger after a new session");
    }

    #[test]
    fn late_starting_receiver_keeps_its_old_session() {
        let link = Arc::new(link::LinkStats::default());
        let bound = link.session(); // bound at engine start
        link.reset(); // reconnect before the thread got scheduled
        let h = spawn_recv(link.clone(), bound);
        assert!(joins_soon(h), "must not adopt the newer session");
        assert_eq!(link.accept(link.session(), 1), Some(true), "new window untouched");
    }
}
