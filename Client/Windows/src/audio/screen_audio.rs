//! Computer audio while sharing the screen ("Share audio").
//!
//! WASAPI loopback of the output device (`cpal` records an output device
//! as input) -> mono 48 kHz -> echo canceller against our own playback
//! (otherwise everyone would hear the other speakers again) -> Opus in music
//! mode -> UDP with `FLAG_SCREEN_AUDIO`. Silence is not sent. Receivers mix it
//! like a voice (per-user mute/volume apply), without the talking indicator.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate, Signal};
use cpal::traits::DeviceTrait;
use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapProd, HeapRb};
use rusttalk_shared::audio_packet::{encode_to_vec, AudioHeader, FLAG_SCREEN_AUDIO, FRAME_SAMPLES, SAMPLE_RATE};

use super::aec::{EchoCanceller, AEC_FRAME};
use super::encoder::rms;

/// Second copy of what we play, for the loopback echo canceller (`None` = not sharing).
pub type RenderTap = Arc<Mutex<Option<HeapProd<f32>>>>;

/// Below this level a frame counts as silence (~ -66 dBFS).
const SILENCE_RMS: f32 = 5e-4;
/// Keep sending this many silent frames after sound (fade-outs, short pauses).
const SILENCE_HANGOVER: u32 = 25; // 0.5 s

/// Running "share audio"; dropping it stops everything.
pub struct ScreenAudio {
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    tap: RenderTap,
}

impl Drop for ScreenAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        *self.tap.lock().unwrap() = None;
    }
}

impl ScreenAudio {
    pub fn start(udp: UdpSocket, token: u64, output_device: Option<&str>, tap: RenderTap) -> Result<Self> {
        let device = super::playback::pick_output_device(output_device)?;
        let supported = device.default_output_config().context("Output device has no format")?;
        let config = supported.config();
        tracing::info!("Loopback: {:?}, {} Hz, {} Kanaele", device.name().unwrap_or_default(), config.sample_rate.0, config.channels);
        let (prod, cons) = HeapRb::<f32>::new(SAMPLE_RATE as usize * 2).split();
        let stream = super::capture::build_stream_for(&device, &supported, prod).context("Could not record the computer audio")?;

        let (render_prod, render_cons) = HeapRb::<f32>::new(SAMPLE_RATE as usize).split();
        *tap.lock().unwrap() = Some(render_prod);

        let stop = Arc::new(AtomicBool::new(false));
        std::thread::Builder::new()
            .name("screen-audio".into())
            .spawn({
                let stop = stop.clone();
                move || encode_loop(cons, render_cons, udp, token, stop)
            })
            .context("screen audio thread")?;
        Ok(Self { _stream: stream, stop, tap })
    }
}

fn encode_loop(
    mut cons: ringbuf::HeapCons<f32>,
    mut render: ringbuf::HeapCons<f32>,
    udp: UdpSocket,
    token: u64,
    stop: Arc<AtomicBool>,
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
            tracing::error!("Opus (Bildschirmton) nicht erstellbar: {e}");
            return;
        }
    };
    let mut aec = EchoCanceller::new();
    let mut frame = [0f32; FRAME_SAMPLES];
    let mut have = 0usize;
    let mut opus = [0u8; 1500];
    let (mut seq, mut ts, mut quiet) = (1u32, 0u32, SILENCE_HANGOVER);
    while !stop.load(Ordering::Relaxed) {
        have += cons.pop_slice(&mut frame[have..]);
        if have < FRAME_SAMPLES {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }
        have = 0;
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
            let _ = udp.send(&encode_to_vec(&header, &opus[..len]));
            seq = seq.wrapping_add(1);
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

    /// Opens the loopback recording of the default output device (plays nothing).
    /// Manual: `cargo test -p rusttalk-client loopback_opens -- --ignored`
    #[test]
    #[ignore]
    fn loopback_opens() {
        let device = crate::audio::playback::pick_output_device(None).expect("output device");
        let supported = device.default_output_config().expect("format");
        let (prod, _cons) = HeapRb::<f32>::new(48_000).split();
        let stream = crate::audio::capture::build_stream_for(&device, &supported, prod).expect("loopback stream");
        std::thread::sleep(Duration::from_millis(300));
        drop(stream);
    }
}
