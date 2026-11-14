//! Computer audio shared independently or alongside a screen.
//!
//! WASAPI loopback on Windows, or a PulseAudio output monitor on Linux
//! -> mono 48 kHz -> echo canceller against our own playback
//! (otherwise everyone would hear the other speakers again) -> Opus in music
//! mode -> UDP with `FLAG_SCREEN_AUDIO`. Silence is not sent. Receivers mix it
//! like a voice (per-user mute/volume apply), without the talking indicator.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate, Signal};
#[cfg(windows)]
use cpal::traits::DeviceTrait;
use ringbuf::traits::{Consumer, Observer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use rusttalk_shared::audio_packet::{seal_packet, AudioHeader, FLAG_SCREEN_AUDIO, FRAME_SAMPLES, SAMPLE_RATE};
use rusttalk_shared::secure::{PacketCipher, DIR_TO_SERVER};

use super::aec::{EchoCanceller, AEC_FRAME};
use super::encoder::rms;

/// Second copy of what we play, for the loopback echo canceller (`None` = not sharing).
pub type RenderTap = Arc<Mutex<Option<HeapProd<f32>>>>;

/// Below this level a frame counts as silence (~ -66 dBFS).
const SILENCE_RMS: f32 = 5e-4;
/// Keep sending this many silent frames after sound (fade-outs, short pauses).
const SILENCE_HANGOVER: u32 = 25; // 0.5 s

/// One captured stream. Its `stop` is set by whoever ends it (capture failure or app exit).
struct Source {
    id: u32,
    cons: HeapCons<f32>,
    stop: Arc<AtomicBool>,
    buf: [f32; FRAME_SAMPLES],
    have: usize,
    /// Mixed on the clock; re-primed (one frame of cushion) after an underrun.
    primed: bool,
    waited: u8,
}

/// More than this much audio piled up in one source is cut back to two frames.
const MAX_BACKLOG: usize = FRAME_SAMPLES * 6;
/// One mix tick.
const FRAME_PERIOD: Duration = Duration::from_millis(20);

impl Source {
    fn channel() -> (HeapProd<f32>, Self) {
        let (prod, cons) = HeapRb::<f32>::new(SAMPLE_RATE as usize * 2).split();
        (prod, Self { id: 0, cons, stop: Arc::new(AtomicBool::new(false)), buf: [0.0; FRAME_SAMPLES], have: 0, primed: false, waited: 0 })
    }

    /// A stopped source with less than a frame left is done; its tail is dropped.
    fn finished(&self) -> bool {
        self.stop.load(Ordering::Relaxed) && self.have < FRAME_SAMPLES && self.cons.is_empty()
    }

    /// Refills `buf`; true when this tick has a whole frame of this source to mix.
    fn next_frame(&mut self) -> bool {
        self.have += self.cons.pop_slice(&mut self.buf[self.have..]);
        let backlog = self.cons.occupied_len();
        if backlog > MAX_BACKLOG {
            self.cons.skip(backlog - FRAME_SAMPLES * 2);
            self.have += self.cons.pop_slice(&mut self.buf[self.have..]);
        }
        let full = self.have == FRAME_SAMPLES;
        if !full {
            self.primed = false;
            self.waited = 0;
            return false;
        }
        if !self.primed {
            self.waited = self.waited.saturating_add(1);
            self.primed = self.cons.occupied_len() >= FRAME_SAMPLES || self.waited > 2 || self.stop.load(Ordering::Relaxed);
        }
        self.primed
    }
}

/// When the last packet went out, for the "sound is flowing" indicator.
#[derive(Clone)]
struct Activity {
    started: Instant,
    /// Milliseconds since `started` plus one; 0 = nothing sent yet.
    last_sent: Arc<AtomicU64>,
}

impl Activity {
    fn new() -> Self {
        Self { started: Instant::now(), last_sent: Arc::default() }
    }

    fn note(&self) {
        self.last_sent.store(self.started.elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
    }

    fn recent(&self) -> bool {
        let last = self.last_sent.load(Ordering::Relaxed);
        last != 0 && (self.started.elapsed().as_millis() as u64 + 1).saturating_sub(last) < SOUND_LIT_MS
    }
}

/// Silence is not sent, so a packet in this window means sound.
pub const SOUND_LIT_MS: u64 = 400;

/// Running "share audio"; dropping it stops everything.
pub struct ScreenAudio {
    activity: Activity,
    source_ids: Vec<u32>,
    registered: Arc<Mutex<Vec<u32>>>,
    _stream: Option<super::DeviceStream>,
    capture_workers: Vec<std::thread::JoinHandle<()>>,
    source_stops: Vec<Arc<AtomicBool>>,
    worker: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    tap: RenderTap,
}

impl Drop for ScreenAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for stop in &self.source_stops { stop.store(true, Ordering::Relaxed); }
        *self.tap.lock().unwrap() = None;
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        for worker in self.capture_workers.drain(..) { let _ = worker.join(); }
    }
}

impl ScreenAudio {
    #[cfg(test)]
    pub fn simulate_failure(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Sound is going out right now.
    pub fn register(&self, ids: Vec<u32>) { *self.registered.lock().unwrap() = ids; }

    pub fn active_ids(&self) -> Vec<u32> {
        self.source_ids.iter().zip(&self.source_stops).filter(|(_, stop)| !stop.load(Ordering::Relaxed)).map(|(id, _)| *id).collect()
    }

    pub fn sending(&self) -> bool {
        self.activity.recent()
    }

    pub fn failed(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn start(udp: UdpSocket, token: u64, cipher: Arc<PacketCipher>, link: Arc<super::link::LinkStats>, output_device: Option<&str>, tap: RenderTap) -> Result<Self> {
        Self::start_source(udp, token, cipher, link, output_device, tap, &super::share_source::AudioShareSource::System)
    }

    pub fn start_source(udp: UdpSocket, token: u64, cipher: Arc<PacketCipher>, link: Arc<super::link::LinkStats>, output_device: Option<&str>, tap: RenderTap, source: &super::share_source::AudioShareSource) -> Result<Self> {
        use super::share_source::{AudioShareSource, MAX_MIX};
        #[cfg(test)]
        if let Some(stream) = super::test_devices::open(super::test_devices::Kind::System, output_device) {
            return Ok(Self { activity: Activity::new(), source_ids: vec![], registered: Arc::default(), _stream: Some(stream?), capture_workers: vec![], source_stops: vec![], worker: None, stop: Arc::new(AtomicBool::new(false)), tap });
        }
        #[cfg(not(windows))]
        let _ = output_device; // Linux captures the system default output, not a CPAL device.
        anyhow::ensure!(cfg!(any(windows, target_os = "linux")), "System audio capture is not supported on this platform");
        let separate = if let AudioShareSource::Separate(apps) = source { Some(apps.clone()) } else { None };
        let normalized = separate.as_ref().map(|apps| AudioShareSource::Applications(apps.iter().map(|(a, _)| a.clone()).collect()));
        let source = normalized.as_ref().unwrap_or(source);
        if let Some(apps) = &separate { anyhow::ensure!(rusttalk_shared::protocol::valid_audio_streams(&apps.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>()), "Invalid audio streams"); }
        if let AudioShareSource::Applications(apps) = source {
            anyhow::ensure!(!apps.is_empty() && apps.len() <= MAX_MIX, "Pick between 1 and {MAX_MIX} applications");
            anyhow::ensure!(apps.iter().enumerate().all(|(i, a)| !apps[..i].contains(a)), "Application selected twice");
        }
        let stop = Arc::new(AtomicBool::new(false));
        // Dropping `audio` on any early return stops the captures already running.
        let mut audio = Self { activity: Activity::new(), source_ids: vec![], registered: Arc::default(), _stream: None, capture_workers: vec![], source_stops: vec![], worker: None, stop: stop.clone(), tap: tap.clone() };
        let mut sources = Vec::new();
        #[cfg(windows)]
        match source {
            AudioShareSource::Applications(apps) => for application in apps {
                let (prod, src) = Source::channel();
                audio.source_stops.push(src.stop.clone());
                audio.capture_workers.push(super::windows_audio::start(application.clone(), prod, src.stop.clone())?);
                sources.push(src);
            },
            AudioShareSource::Separate(_) => unreachable!("normalized above"),
            AudioShareSource::System => {
                let (prod, src) = Source::channel();
                audio.source_stops.push(src.stop.clone());
                let device = super::playback::pick_output_device(output_device)?;
                let supported = device.default_output_config().context("Output device has no format")?;
                let stream = super::capture::build_stream_for(&device, &supported, prod, src.stop.clone())
                    .context("Could not record the computer audio")?;
                #[cfg(test)]
                let stream = super::DeviceStream::Hardware { _stream: stream };
                audio._stream = Some(stream);
                sources.push(src);
            }
        }
        #[cfg(target_os = "linux")]
        {
            let targets: Vec<Option<super::share_source::ApplicationSource>> = match source {
                AudioShareSource::Separate(_) => unreachable!("normalized above"),
                AudioShareSource::System => vec![None],
                AudioShareSource::Applications(apps) => apps.iter().cloned().map(Some).collect(),
            };
            for target in targets {
                let (prod, src) = Source::channel();
                audio.source_stops.push(src.stop.clone());
                audio.capture_workers.push(super::system_audio::start(prod, src.stop.clone(), target)?);
                sources.push(src);
            }
        }

        audio.source_ids = separate.as_ref().map(|apps| apps.iter().map(|(_, s)| s.id).collect()).unwrap_or_default();
        if let Some(apps) = &separate { for (source, (_, info)) in sources.iter_mut().zip(apps) { source.id = info.id; } }
        let (render_prod, render_cons) = HeapRb::<f32>::new(SAMPLE_RATE as usize).split();
        audio.worker = Some(std::thread::Builder::new()
            .name("screen-audio".into())
            .spawn({
                let stop = stop.clone();
                let activity = audio.activity.clone();
                let registered = audio.registered.clone();
                move || if separate.is_some() { encode_separate(sources, render_cons, udp, token, cipher, link, stop, activity, registered) } else { encode_loop(sources, render_cons, udp, token, cipher, link, stop, activity) }
            })
            .context("screen audio thread")?);
        *tap.lock().unwrap() = Some(render_prod);
        Ok(audio)
    }
}

/// One 20 ms tick: sums a frame of every source that has one (a stalled or silent source
/// contributes nothing) and clamps to full scale.
fn mix_tick(sources: &mut [Source], frame: &mut [f32; FRAME_SAMPLES]) {
    frame.fill(0.0);
    for s in sources.iter_mut() {
        if s.next_frame() {
            for (out, v) in frame.iter_mut().zip(&s.buf) { *out += v; }
            s.have = 0;
        }
    }
    for v in frame.iter_mut() { *v = v.clamp(-1.0, 1.0); }
}

fn encode_loop(
    mut sources: Vec<Source>,
    mut render: ringbuf::HeapCons<f32>,
    udp: UdpSocket,
    token: u64,
    cipher: Arc<PacketCipher>,
    link: Arc<super::link::LinkStats>,
    stop: Arc<AtomicBool>,
    activity: Activity,
) {
    let encoder = match Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Audio) {
        Ok(mut e) => {
            let _ = e.set_bitrate(Bitrate::BitsPerSecond(96_000));
            let _ = e.set_complexity(8);
            let _ = e.set_signal(Signal::Music);
            let _ = e.set_inband_fec(true);
            let _ = e.set_packet_loss_perc(10);
            e
        }
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            tracing::error!("Computer audio encoder failed: {e}");
            return;
        }
    };
    let mut aec = EchoCanceller::new();
    let mut frame = [0f32; FRAME_SAMPLES];
    let mut opus = [0u8; 1500];
    let (mut seq, mut ts, mut quiet) = (1u32, 0u32, SILENCE_HANGOVER);
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now < next {
            std::thread::sleep((next - now).min(Duration::from_millis(5)));
            continue;
        }
        next += FRAME_PERIOD;
        if now > next + Duration::from_millis(100) {
            next = now + FRAME_PERIOD; // fell behind: do not burst to catch up
        }
        // An app that closed leaves the mix; the last source ending ends the share.
        sources.retain(|s| !s.finished());
        if sources.is_empty() {
            stop.store(true, Ordering::Relaxed);
            break;
        }
        mix_tick(&mut sources, &mut frame);
        // Remove what RustTalk itself plays (the other people's voices).
        if let Some(a) = aec.as_mut() {
            for half in frame.chunks_mut(AEC_FRAME) {
                a.process_with_ring(half, &mut render);
            }
        }
        ts = ts.wrapping_add(FRAME_SAMPLES as u32);
        if rms(&frame) < SILENCE_RMS {
            quiet = quiet.saturating_add(1);
            if quiet > SILENCE_HANGOVER {
                continue;
            }
        } else {
            quiet = 0;
        }
        if let Ok(len) = encoder.encode_float(&frame[..], &mut opus) {
            let mut header = AudioHeader::new(token, seq, ts);
            header.flags |= FLAG_SCREEN_AUDIO;
            if udp
                .send(&seal_packet(&header, &cipher, DIR_TO_SERVER, &opus[..len]))
                .is_ok()
            {
                link.count_payload(
                    crate::bandwidth::OUT,
                    crate::bandwidth::Media::audio(&header),
                    len,
                );
            }
            link.audio_sent.fetch_add(1, Ordering::Relaxed);
            activity.note();
            seq = seq.wrapping_add(1);
        } else {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_separate(
    mut sources: Vec<Source>,
    mut render: HeapCons<f32>,
    udp: UdpSocket,
    token: u64,
    cipher: Arc<PacketCipher>,
    link: Arc<super::link::LinkStats>,
    stop: Arc<AtomicBool>,
    activity: Activity,
    registered: Arc<Mutex<Vec<u32>>>,
) {
    let mut encoders = std::collections::HashMap::new();
    for source in &sources {
        let Ok(mut encoder) = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Audio)
        else {
            stop.store(true, Ordering::Relaxed);
            return;
        };
        if encoder.set_bitrate(Bitrate::BitsPerSecond(64_000)).is_err() {
            stop.store(true, Ordering::Relaxed);
            return;
        }
        let _ = encoder.set_inband_fec(true);
        let _ = encoder.set_packet_loss_perc(10);
        encoders.insert(
            source.id,
            (encoder, EchoCanceller::new(), 1u32, 0u32, SILENCE_HANGOVER),
        );
    }
    let mut next = Instant::now();
    let mut opus = [0u8; 1400];
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now < next {
            std::thread::sleep((next - now).min(Duration::from_millis(5)));
            continue;
        }
        next = (next + FRAME_PERIOD).max(now);
        sources.retain(|s| !s.finished());
        if sources.is_empty() {
            stop.store(true, Ordering::Relaxed);
            break;
        }
        let backlog = render.occupied_len();
        if backlog > AEC_FRAME * 30 {
            render.skip(backlog - AEC_FRAME * 6);
        }
        let mut reference = [0.0; FRAME_SAMPLES];
        render.pop_slice(&mut reference);
        for source in &mut sources {
            let Some((encoder, aec, seq, ts, quiet)) = encoders.get_mut(&source.id) else {
                continue;
            };
            *ts = ts.wrapping_add(FRAME_SAMPLES as u32);
            let mut frame = [0.0; FRAME_SAMPLES];
            if source.next_frame() {
                frame = source.buf;
                source.have = 0;
            }
            if let Some(aec) = aec {
                for (half, reference) in
                    frame.chunks_mut(AEC_FRAME).zip(reference.chunks(AEC_FRAME))
                {
                    aec.process(half, reference);
                }
            }
            if rms(&frame) < SILENCE_RMS {
                *quiet = quiet.saturating_add(1);
                if *quiet > SILENCE_HANGOVER {
                    continue;
                }
            } else {
                *quiet = 0;
            }
            if !registered.lock().unwrap().contains(&source.id) {
                continue;
            }
            let Ok(len) = encoder.encode_float(&frame, &mut opus) else {
                stop.store(true, Ordering::Relaxed);
                return;
            };
            let mut header = AudioHeader::new(token, *seq, *ts);
            header.flags = FLAG_SCREEN_AUDIO;
            header.stream_id = source.id;
            if udp
                .send(&seal_packet(&header, &cipher, DIR_TO_SERVER, &opus[..len]))
                .is_ok()
            {
                link.count_payload(
                    crate::bandwidth::OUT,
                    crate::bandwidth::Media::audio(&header),
                    len,
                );
            }
            *seq = seq.wrapping_add(1);
            link.audio_sent.fetch_add(1, Ordering::Relaxed);
            activity.note();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_keys_separate_voice_and_screen_audio() {
        let (v, s) = (crate::audio::mixer_key(42, false), crate::audio::mixer_key(42, true));
        assert_ne!(v, s);
        assert_eq!(crate::audio::mixer_speaker(v), 42);
        assert_eq!(crate::audio::mixer_speaker(s), 42);
    }

    #[test]
    fn encoder_sends_separate_encrypted_audio_and_joins_on_stop() {
        use ringbuf::traits::Producer;
        use rusttalk_shared::audio_packet::{decode, open_packet};
        let devices = crate::audio::test_devices::Devices::new();
        let stream = crate::audio::test_devices::open(crate::audio::test_devices::Kind::System, None).unwrap().unwrap();
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.connect(receiver.local_addr().unwrap()).unwrap();
        let cipher = Arc::new(PacketCipher::new(&[7; 32]).unwrap());
        let voice = seal_packet(&AudioHeader::new(42, 1, 0), &cipher, DIR_TO_SERVER, b"voice");
        let (mut prod, source) = Source::channel();
        prod.push_slice(&[0.3; FRAME_SAMPLES * 3]);
        let (render_prod, render_cons) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        let tap = Arc::new(Mutex::new(Some(render_prod)));
        let stop = Arc::new(AtomicBool::new(false));
        let link = Arc::new(super::super::link::LinkStats::default());
        let worker = std::thread::spawn({
            let link = link.clone();
            let cipher = cipher.clone();
            let stop = stop.clone();
            move || {
                encode_loop(
                    vec![source],
                    render_cons,
                    sender,
                    42,
                    cipher,
                    link,
                    stop,
                    Activity::new(),
                )
            }
        });
        let audio = ScreenAudio { activity: Activity::new(), source_ids: vec![], registered: Arc::default(), _stream: Some(stream), capture_workers: vec![], source_stops: vec![], worker: Some(worker), stop: stop.clone(), tap: tap.clone() };
        let mut buf = [0; 1500];
        let n = receiver.recv(&mut buf).unwrap();
        let (header, _) = decode(&buf[..n]).unwrap();
        assert_eq!(header.flags, FLAG_SCREEN_AUDIO);
        assert_ne!(header.counter, decode(&voice).unwrap().0.counter);
        assert!(!open_packet(&header, &buf[..n], &cipher, DIR_TO_SERVER)
            .unwrap()
            .is_empty());
        let mut payload_bytes = open_packet(&header, &buf[..n], &cipher, DIR_TO_SERVER)
            .unwrap()
            .len();
        drop(audio);
        receiver.set_nonblocking(true).unwrap();
        while let Ok(n) = receiver.recv(&mut buf) {
            let (h, _) = decode(&buf[..n]).unwrap();
            payload_bytes += open_packet(&h, &buf[..n], &cipher, DIR_TO_SERVER)
                .unwrap()
                .len();
        }
        assert_eq!(
            link.traffic.lock().unwrap().snapshot()[crate::bandwidth::OUT],
            [0, 0, 0, payload_bytes as u64, 0]
        );
        assert!(stop.load(Ordering::Relaxed));
        assert!(tap.lock().unwrap().is_none());
        assert_eq!(devices.0.borrow().systems, 0);
    }

    #[test]
    fn application_encoders_have_independent_sequences_and_shared_nonces() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.connect(receiver.local_addr().unwrap()).unwrap();
        let (mut pa, mut a) = Source::channel();
        let (mut pb, mut b) = Source::channel();
        a.id = 7; b.id = 8;
        feed(&mut pa, 0.2, 5); feed(&mut pb, 0.4, 5);
        let (_reference, render) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        let cipher = Arc::new(PacketCipher::new(&[9; 32]).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn({
            let stop = stop.clone(); let cipher = cipher.clone();
            move || encode_separate(vec![a, b], render, sender, 5, cipher, Arc::default(), stop, Activity::new(), Arc::new(Mutex::new(vec![7, 8])))
        });
        let mut buf = [0; 1500];
        let mut counters = std::collections::HashSet::new();
        let mut sequences = std::collections::HashMap::new();
        for _ in 0..4 {
            let n = receiver.recv(&mut buf).unwrap();
            let h = rusttalk_shared::audio_packet::decode(&buf[..n]).unwrap().0;
            assert!(counters.insert(h.counter));
            assert!(rusttalk_shared::audio_packet::open_packet(&h, &buf[..n], &cipher, DIR_TO_SERVER).is_some());
            sequences.entry(h.stream_id).or_insert_with(Vec::new).push(h.sequence);
        }
        stop.store(true, Ordering::Relaxed); worker.join().unwrap();
        assert_eq!(sequences[&7], vec![1, 2]);
        assert_eq!(sequences[&8], vec![1, 2]);
    }

    fn feed(prod: &mut HeapProd<f32>, value: f32, frames: usize) {
        use ringbuf::traits::Producer;
        prod.push_slice(&vec![value; FRAME_SAMPLES * frames]);
    }

    #[test]
    fn sources_are_summed_clamped_and_a_stalled_one_does_not_block() {
        let (mut a_prod, a) = Source::channel();
        let (mut b_prod, b) = Source::channel();
        let mut sources = vec![a, b];
        let mut frame = [0.0; FRAME_SAMPLES];
        mix_tick(&mut sources, &mut frame);
        assert!(frame.iter().all(|v| *v == 0.0), "nothing buffered yet");
        feed(&mut a_prod, 0.25, 3);
        mix_tick(&mut sources, &mut frame);
        assert!(frame.iter().all(|v| *v == 0.25), "a silent source must not stall the mix");
        feed(&mut b_prod, 0.5, 3);
        mix_tick(&mut sources, &mut frame);
        assert!(frame.iter().all(|v| *v == 0.75));
        let (mut c_prod, c) = Source::channel();
        let (mut d_prod, d) = Source::channel();
        feed(&mut c_prod, 0.9, 3);
        feed(&mut d_prod, 0.9, 3);
        mix_tick(&mut [c, d], &mut frame);
        assert!(frame.iter().all(|v| *v == 1.0), "sum must not exceed full scale");
    }

    #[test]
    fn staggered_sources_give_one_mixed_frame_per_tick() {
        use ringbuf::traits::Producer;
        let (mut a_prod, a) = Source::channel();
        let (mut b_prod, b) = Source::channel();
        let mut sources = vec![a, b];
        let mut frame = [0.0; FRAME_SAMPLES];
        // b delivers 10 ms (half a frame) behind a, in 480-sample blocks.
        feed(&mut a_prod, 0.25, 2);
        b_prod.push_slice(&[0.5; FRAME_SAMPLES / 2]);
        mix_tick(&mut sources, &mut frame);
        assert!(frame.iter().all(|v| *v == 0.25), "b has no whole frame yet");
        b_prod.push_slice(&[0.5; FRAME_SAMPLES * 2]);
        mix_tick(&mut sources, &mut frame);
        assert!(frame.iter().all(|v| *v == 0.75), "both sources land in the same packet");
    }

    #[test]
    fn partial_delivery_never_shifts_a_source_and_backlog_is_bounded() {
        use ringbuf::traits::Producer;
        let (mut prod, source) = Source::channel();
        let mut sources = vec![source];
        let mut frame = [0.0; FRAME_SAMPLES];
        prod.push_slice(&[0.3; FRAME_SAMPLES - 1]);
        mix_tick(&mut sources, &mut frame);
        assert!(frame.iter().all(|v| *v == 0.0), "a partial frame waits");
        assert_eq!(sources[0].have, FRAME_SAMPLES - 1);
        feed(&mut prod, 0.3, 40);
        mix_tick(&mut sources, &mut frame);
        assert!(sources[0].cons.occupied_len() <= MAX_BACKLOG, "a burst is cut back");
    }

    #[test]
    fn a_stopped_source_with_a_partial_tail_ends_and_so_does_the_share() {
        use ringbuf::traits::Producer;
        let (mut prod, source) = Source::channel();
        let (_render_prod, render_cons) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        prod.push_slice(&[0.1; 10]);
        source.stop.store(true, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.connect("127.0.0.1:9").unwrap();
        let cipher = Arc::new(PacketCipher::new(&[7; 32]).unwrap());
        let worker = std::thread::spawn({
            let stop = stop.clone();
            move || encode_loop(vec![source], render_cons, sender, 1, cipher, Arc::new(super::super::link::LinkStats::default()), stop, Activity::new())
        });
        let start = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            assert!(start.elapsed() < Duration::from_secs(2), "sharing must end when its only source is gone");
            std::thread::sleep(Duration::from_millis(5));
        }
        worker.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a running local PulseAudio or pipewire-pulse server"]
    fn linux_system_audio_worker_stops() {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.connect(udp.local_addr().unwrap()).unwrap();
        let tap = Arc::new(Mutex::new(None));
        let audio = ScreenAudio::start(udp, 42, Arc::new(PacketCipher::new(&[7; 32]).unwrap()),
            Arc::new(super::super::link::LinkStats::default()), None, tap.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!audio.failed());
        let start = std::time::Instant::now();
        drop(audio);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(tap.lock().unwrap().is_none());
    }

    /// Opens the loopback recording of the default output device (plays nothing).
    /// Manual: `cargo test -p rusttalk-client loopback_opens -- --ignored`
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn loopback_opens() {
        let device = crate::audio::playback::pick_output_device(None).expect("output device");
        let supported = device.default_output_config().expect("format");
        let (prod, _cons) = HeapRb::<f32>::new(48_000).split();
        let stream = crate::audio::capture::build_stream_for(&device, &supported, prod, Arc::new(AtomicBool::new(false))).expect("loopback stream");
        std::thread::sleep(Duration::from_millis(300));
        drop(stream);
    }
}
