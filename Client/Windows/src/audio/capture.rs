//! Mikrofon-Aufnahme via cpal: Mono-Mixdown + Resampling auf 48 kHz
//! -> Ring-Puffer (von dort holt sich der Encoder-Thread die Frames).

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapProd, HeapRb};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use super::encoder::rms;
use super::resample::Resampler;

/// Namen aller Eingabegeraete (fuer die Einstellungen).
pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

fn pick_input_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(want) = name {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(d) = devices.find(|d| d.name().map(|n| n == want).unwrap_or(false)) {
                return Ok(d);
            }
        }
        tracing::warn!("Eingabegeraet '{want}' nicht gefunden, nutze Standardgeraet");
    }
    host.default_input_device().context("No default input device found")
}

pub fn build_input_stream(
    device_name: Option<&str>,
    producer: HeapProd<f32>,
    level_sink: Option<Arc<AtomicU32>>,
    gain: Option<Arc<AtomicU32>>,
) -> Result<cpal::Stream> {
    let device = pick_input_device(device_name)?;
    let supported = device.default_input_config().context("default_input_config")?;
    let config = supported.config();
    tracing::info!(
        "Eingabe: {:?}, {} Hz, {} Kanaele, {:?}",
        device.name().unwrap_or_default(),
        config.sample_rate.0,
        config.channels,
        supported.sample_format()
    );
    let err_fn = |e| tracing::warn!("Input-Stream-Fehler: {e}");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_in::<f32>(&device, &config, |s| s, producer, level_sink, gain, err_fn)?,
        cpal::SampleFormat::I16 => {
            build_in::<i16>(&device, &config, |s| s as f32 / 32768.0, producer, level_sink, gain, err_fn)?
        }
        cpal::SampleFormat::U16 => {
            build_in::<u16>(&device, &config, |s| (s as f32 - 32768.0) / 32768.0, producer, level_sink, gain, err_fn)?
        }
        other => return Err(anyhow!("Unsupported sample format: {other:?}")),
    };
    Ok(stream)
}

/// Input stream for any device + format (loopback of an output device).
pub fn build_stream_for(device: &cpal::Device, supported: &cpal::SupportedStreamConfig, producer: HeapProd<f32>) -> Result<cpal::Stream> {
    let config = supported.config();
    let err_fn = |e| tracing::warn!("Loopback-Stream-Fehler: {e}");
    match supported.sample_format() {
        cpal::SampleFormat::F32 => build_in::<f32>(device, &config, |s| s, producer, None, None, err_fn),
        cpal::SampleFormat::I16 => build_in::<i16>(device, &config, |s| s as f32 / 32768.0, producer, None, None, err_fn),
        cpal::SampleFormat::U16 => build_in::<u16>(device, &config, |s| (s as f32 - 32768.0) / 32768.0, producer, None, None, err_fn),
        other => Err(anyhow!("Unsupported sample format: {other:?}")),
    }
}

/// Pegel-Messung ohne Server-Verbindung (Mikro-Test in den Einstellungen).
/// Hält einen Input-Stream offen und verwirft die Samples; der Pegel fließt
/// über die Level-Sink.
pub struct MicMonitor {
    stream: Option<cpal::Stream>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MicMonitor {
    /// `gain`: input volume handle, so the meter shows the level after the gain.
    pub fn start(device_name: Option<&str>, gain: Option<Arc<AtomicU32>>) -> Result<Self> {
        let rb = HeapRb::<f32>::new(9600); // 200 ms genuegen
        let (prod, mut cons) = rb.split();
        let level = Arc::new(AtomicU32::new(0));
        let stream = build_input_stream(device_name, prod, Some(level.clone()), gain)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::Builder::new()
            .name("mic-monitor".to_string())
            .spawn({
                let stop = stop.clone();
                move || {
                    while !stop.load(Ordering::Relaxed) {
                        while cons.try_pop().is_some() {}
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            })
            .context("Monitor-Thread starten")?;
        Ok(Self { stream: Some(stream), level, stop, worker: Some(worker) })
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for MicMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.stream = None;
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

fn build_in<T: cpal::SizedSample + Send + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    to_f32: fn(T) -> f32,
    mut producer: HeapProd<f32>,
    level_sink: Option<Arc<AtomicU32>>,
    gain: Option<Arc<AtomicU32>>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream> {
    let channels = config.channels as usize;
    let device_rate = config.sample_rate.0;
    let mut resampler = Resampler::to_internal(device_rate);
    let mut mono: Vec<f32> = Vec::with_capacity(8192);
    let mut resampled: Vec<f32> = Vec::with_capacity(8192);

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            mono.clear();
            if channels <= 1 {
                mono.extend(data.iter().map(|&s| to_f32(s)));
            } else {
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|&s| to_f32(s)).sum();
                    mono.push(sum / frame.len() as f32);
                }
            }
            // Manual input volume (before metering, so meter and VAD see the same signal).
            if let Some(g) = &gain {
                apply_gain(&mut mono, f32::from_bits(g.load(Ordering::Relaxed)));
            }
            // Pegel melden (geglaettet mit schnellem Anstieg, sanftem Abfall).
            if let Some(sink) = &level_sink {
                let level = rms(&mono);
                let prev = f32::from_bits(sink.load(Ordering::Relaxed));
                sink.store(level.max(prev * 0.85).to_bits(), Ordering::Relaxed);
            }
            resampled.clear();
            resampler.process(&mono, &mut resampled);
            for &s in &resampled {
                if producer.try_push(s).is_err() {
                    break; // Ring voll -> verwerfen (lieber Luecke als Latenz)
                }
            }
        },
        err_fn,
        None,
    )
    .context("build_input_stream")?;
    stream.play().context("Could not start the input stream")?;
    Ok(stream)
}

/// Multiplies the samples by `gain` and clamps to [-1, 1] (like a real ADC).
/// 1.0 is a no-op.
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if (gain - 1.0).abs() < 1e-4 {
        return;
    }
    for s in samples.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_scales_and_clamps() {
        let mut v = [0.1f32, -0.2, 0.6, -0.9];
        apply_gain(&mut v, 2.0);
        assert!((v[0] - 0.2).abs() < 1e-6);
        assert!((v[1] + 0.4).abs() < 1e-6);
        assert_eq!(v[2], 1.0, "clamped");
        assert_eq!(v[3], -1.0, "clamped");
        let mut w = [0.5f32; 4];
        apply_gain(&mut w, 1.0);
        assert_eq!(w, [0.5; 4], "unity is a no-op");
        apply_gain(&mut w, 0.0);
        assert_eq!(w, [0.0; 4], "zero mutes");
    }
}
