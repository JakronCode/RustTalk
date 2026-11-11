//! Wiedergabe via cpal: Mischt alle aktiven Sprecher (48 kHz Mono)
//! und gibt auf dem Ausgabegeraet aus (mit Resampling, falls noetig).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rusttalk_shared::audio_packet::{FRAME_SAMPLES, SAMPLE_RATE};

use super::jitter::USER_TIMEOUT;
use super::{AudioControl, LocalMuteSet, SharedMixer, UserVolumeMap};
use std::collections::VecDeque;

/// Namen aller Ausgabegeraete (fuer die Einstellungen).
pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

fn pick_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(want) = name {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(d) = devices.find(|d| d.name().map(|n| n == want).unwrap_or(false)) {
                return Ok(d);
            }
        }
        tracing::warn!("Ausgabegeraet '{want}' nicht gefunden, nutze Standardgeraet");
    }
    host.default_output_device().context("No default output device found")
}

/// Pull-Resampler: liest exakt so viele 48-kHz-Samples aus der FIFO,
/// wie fuer die gewuenschte Anzahl Ausgabeframes noetig sind.
///
/// Hintergrund: Der alte Push-Ansatz (alles resamplen + Rest verwerfen)
/// konsumierte pro Callback ~960 Samples zu viel -> Jitter-Buffer lief leer
/// -> hoerbares Stottern ("200 ms Ton, 200 ms Stille").
pub struct PullResampler {
    /// 48-kHz-Samples pro Output-Sample (z. B. 1.0 bei 48k, ~1.088 bei 44.1k).
    ratio: f32,
    /// Fractionale Position zwischen dem ersten und zweiten FIFO-Sample [0, 1).
    frac: f32,
}

impl PullResampler {
    pub fn new(device_rate: u32) -> Self {
        Self { ratio: SAMPLE_RATE as f32 / device_rate.max(1) as f32, frac: 0.0 }
    }

    /// Wie viele 48-kHz-Samples fuer `out_frames` Output-Frames gepuffert
    /// sein sollten (inkl. kleiner Reserve).
    pub fn needed_48k(&self, out_frames: usize) -> usize {
        (out_frames as f32 * self.ratio).ceil() as usize + 2
    }

    /// Liest `out.len()` Samples aus der 48-kHz-FIFO (Underrun -> Stille).
    pub fn pull(&mut self, fifo: &mut VecDeque<f32>, out: &mut [f32]) {
        for o in out.iter_mut() {
            let s = if fifo.len() >= 2 {
                let a = fifo[0];
                let b = fifo[1];
                a + (b - a) * self.frac
            } else if fifo.len() == 1 {
                fifo[0]
            } else {
                0.0
            };
            *o = s;
            self.frac += self.ratio;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                fifo.pop_front();
            }
        }
    }
}

/// Bevorzugt f32 @ 48 kHz; Fallback: Default-Config des Geraets.
fn pick_output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    if let Ok(mut configs) = device.supported_output_configs() {
        if let Some(range) = configs.find(|r| {
            r.sample_format() == cpal::SampleFormat::F32
                && r.min_sample_rate().0 <= SAMPLE_RATE
                && r.max_sample_rate().0 >= SAMPLE_RATE
        }) {
            return Ok(range.with_sample_rate(cpal::SampleRate(SAMPLE_RATE)));
        }
    }
    Ok(device.default_output_config()?)
}

pub fn build_output_stream(
    device_name: Option<&str>,
    mixer: SharedMixer,
    control: Arc<AudioControl>,
    local_muted: LocalMuteSet,
    user_volumes: UserVolumeMap,
) -> Result<cpal::Stream> {
    let device = pick_output_device(device_name)?;
    let supported = pick_output_config(&device)?;
    let config = supported.config();
    tracing::info!(
        "Ausgabe: {:?}, {} Hz, {} Kanaele, {:?}",
        device.name().unwrap_or_default(),
        config.sample_rate.0,
        config.channels,
        supported.sample_format()
    );
    let err_fn = |e| tracing::warn!("Output-Stream-Fehler: {e}");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_out::<f32>(&device, &config, |s| s, mixer, control, local_muted, user_volumes, err_fn)?,
        cpal::SampleFormat::I16 => {
            build_out::<i16>(&device, &config, |s| (s * 32767.0) as i16, mixer, control, local_muted, user_volumes, err_fn)?
        }
        cpal::SampleFormat::U16 => {
            build_out::<u16>(&device, &config, |s| (s * 32767.0 + 32768.0) as u16, mixer, control, local_muted, user_volumes, err_fn)?
        }
        other => return Err(anyhow!("Unsupported sample format: {other:?}")),
    };
    Ok(stream)
}

fn build_out<T: cpal::SizedSample + Send + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    from_f32: fn(f32) -> T,
    mixer: SharedMixer,
    control: Arc<AudioControl>,
    local_muted: LocalMuteSet,
    user_volumes: UserVolumeMap,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream> {
    let channels = config.channels as usize;
    let device_rate = config.sample_rate.0;
    let mut puller = PullResampler::new(device_rate);
    // FIFO im 48-kHz-Mix-Format; wird just-in-time in 20-ms-Chunks gefuellt.
    let mut fifo: VecDeque<f32> = VecDeque::with_capacity(SAMPLE_RATE as usize);
    let mut chunk = [0f32; FRAME_SAMPLES];
    let mut tmp = [0f32; FRAME_SAMPLES];
    let mut mono_out: Vec<f32> = Vec::with_capacity(16384);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let frames = data.len() / channels.max(1);
            let needed = puller.needed_48k(frames);

            // FIFO auffuellen: in 20-ms-Chunks mischen (genau so viel wie noetig).
            while fifo.len() < needed {
                chunk.fill(0.0);
                if !control.self_deafened.load(Ordering::Relaxed) {
                    let mut map = mixer.lock().unwrap();
                    map.retain(|_, u| u.last_seen.elapsed() < USER_TIMEOUT);
                    let local = local_muted.lock().unwrap();
                    let volumes = user_volumes.lock().unwrap();
                    for (id, user) in map.iter_mut() {
                        if local.contains(id) {
                            continue; // lokal stummgeschaltet
                        }
                        tmp.fill(0.0);
                        let n = user.fill(&mut tmp);
                        // Per-user volume (local only, 1.0 = unchanged).
                        let gain = volumes.get(id).copied().unwrap_or(1.0);
                        for (m, t) in chunk[..n].iter_mut().zip(tmp[..n].iter()) {
                            *m += *t * gain;
                        }
                    }
                }
                // Weicher Limiter statt Hard-Clip: mehrere laute Sprecher
                // gleichzeitig verzerren nicht mehr.
                let vol = control.output_volume();
                for s in &mut chunk {
                    *s = super::dsp::soft_limit(*s * vol);
                }
                fifo.extend(chunk);
            }

            // Exakt `frames` Samples ziehen (kein Verwerfen mehr -> kein Stottern).
            mono_out.clear();
            mono_out.resize(frames, 0.0);
            puller.pull(&mut fifo, &mut mono_out);
            for (f, &s) in mono_out.iter().enumerate() {
                for c in 0..channels {
                    data[f * channels + c] = from_f32(s);
                }
            }
        },
        err_fn,
        None,
    )
    .context("build_output_stream")?;
    stream.play().context("Could not start the output stream")?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rate_consumes_exactly() {
        // 48k -> 48k: pull(480) verbraucht exakt 480 Samples (kein Drift!).
        let mut p = PullResampler::new(48_000);
        assert_eq!(p.needed_48k(480), 482);
        let mut fifo: VecDeque<f32> = (0..960).map(|i| i as f32).collect();
        let mut out = [0f32; 480];
        p.pull(&mut fifo, &mut out);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[479], 479.0);
        assert_eq!(fifo.len(), 480, "exakt 480 konsumiert");
        p.pull(&mut fifo, &mut out);
        assert_eq!(out[0], 480.0);
        assert_eq!(fifo.len(), 0);
    }

    #[test]
    fn downsample_output_rate_keeps_pitch() {
        // 44.1k Ausgabe: 44100 Output-Samples verbrauchen ~48000 48k-Samples.
        let mut p = PullResampler::new(44_100);
        let mut fifo: VecDeque<f32> = vec![0.25; 48_000].into_iter().collect();
        let mut out = vec![0f32; 44_100];
        p.pull(&mut fifo, &mut out);
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6), "konstant bleibt konstant");
        assert!(fifo.len() <= 3, "fast alles konsumiert: {}", fifo.len());
    }

    #[test]
    fn upsample_interpolates_monotonic() {
        // 96k Ausgabe (ratio 0.5): Rampe bleibt monoton steigend.
        let mut p = PullResampler::new(96_000);
        let mut fifo: VecDeque<f32> = (0..960).map(|i| i as f32).collect();
        let mut out = vec![0f32; 1920];
        p.pull(&mut fifo, &mut out);
        assert!(out.windows(2).all(|w| w[1] >= w[0] - 1e-6), "monoton");
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1919] - 959.0).abs() < 1.5, "Endwert ~959, ist {}", out[1919]);
    }

    #[test]
    fn underrun_yields_silence() {
        let mut p = PullResampler::new(48_000);
        let mut fifo: VecDeque<f32> = VecDeque::new();
        let mut out = [1.0f32; 480];
        p.pull(&mut fifo, &mut out);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn no_drift_over_many_callbacks() {
        // Der alte Bug: pro Callback wurden Reserve-Samples verworfen.
        // Jetzt: 100 Callbacks a 480 Frames verbrauchen exakt 48000 Samples.
        let mut p = PullResampler::new(48_000);
        let mut fifo: VecDeque<f32> = vec![0.0; 48_000].into_iter().collect();
        let mut out = [0f32; 480];
        for _ in 0..100 {
            while fifo.len() < p.needed_48k(480) {
                fifo.extend([0.0f32; FRAME_SAMPLES]);
            }
            p.pull(&mut fifo, &mut out);
        }
        assert!(fifo.len() < 48_000 + 2 * FRAME_SAMPLES, "FIFO laeuft nicht ueber");
        assert!(fifo.len() < 2 * FRAME_SAMPLES, "FIFO bleibt klein: {}", fifo.len());
    }
}
