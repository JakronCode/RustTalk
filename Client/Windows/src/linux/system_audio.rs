//! Immovable output-monitor capture through PulseAudio or pipewire-pulse.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::{Context as PulseContext, State as ContextState};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::stream::{FlagSet, PeekResult, State as StreamState, Stream};
use ringbuf::traits::Producer;
use ringbuf::HeapProd;

const TIMEOUT: Duration = Duration::from_secs(5);
const RECORD_FLAGS: FlagSet =
    FlagSet::from_bits_retain(FlagSet::DONT_MOVE.bits() | FlagSet::ADJUST_LATENCY.bits());

struct StopOnExit(Arc<AtomicBool>);
impl Drop for StopOnExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn discovery_ready(state: ContextState, stop: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        state == ContextState::Ready && !stop.load(Ordering::Relaxed),
        "System audio connection ended during discovery"
    );
    Ok(())
}

// Only nonblocking mainloop iterations: cancellation never waits for a server reply.
fn tick(mainloop: &mut Mainloop, stop: &AtomicBool, deadline: Instant) -> Result<()> {
    anyhow::ensure!(!stop.load(Ordering::Relaxed), "System audio stopped");
    anyhow::ensure!(Instant::now() < deadline, "System audio server timed out");
    if !matches!(mainloop.iterate(false), IterateResult::Success(_)) {
        bail!("System audio connection ended");
    }
    std::thread::sleep(Duration::from_millis(3));
    Ok(())
}

fn copy_samples(data: &[u8], producer: &mut HeapProd<f32>) -> Result<()> {
    anyhow::ensure!(
        data.len().is_multiple_of(4) && data.len() <= 48_000 * 4,
        "Invalid system audio buffer"
    );
    for bytes in data.as_chunks::<4>().0 {
        let sample = f32::from_le_bytes(*bytes);
        anyhow::ensure!(sample.is_finite(), "Invalid system audio sample");
        let _ = producer.try_push(sample); // bounded ring: drop overflow
    }
    Ok(())
}

fn capture(producer: &mut HeapProd<f32>, stop: &AtomicBool, server: Option<&str>) -> Result<()> {
    let mut mainloop = Mainloop::new().context("Could not create audio mainloop")?;
    let mut context = PulseContext::new(&mainloop, "RustTalk system audio")
        .context("Could not create PulseAudio context")?;
    context.connect(server, pulse::context::FlagSet::NOAUTOSPAWN, None)?;
    let deadline = Instant::now() + TIMEOUT;
    while context.get_state() != ContextState::Ready {
        anyhow::ensure!(
            !matches!(
                context.get_state(),
                ContextState::Failed | ContextState::Terminated
            ),
            "PulseAudio / pipewire-pulse unavailable"
        );
        tick(&mut mainloop, stop, deadline)?;
    }
    discovery_ready(context.get_state(), stop)?;
    // Resolve the actual sink's monitor, never the default microphone or a symbolic source.
    // shortcut: binding 2.30.1 leaks cancelled discovery callbacks; remove when Operation::from_raw is fixed upstream.
    let monitor = Rc::new(RefCell::new(None));
    let found = monitor.clone();
    let mut operation =
        context
            .introspect()
            .get_sink_info_by_name("@DEFAULT_SINK@", move |result| {
                if let ListResult::Item(info) = result {
                    *found.borrow_mut() = info
                        .monitor_source_name
                        .as_ref()
                        .map(|n| (info.index, n.to_string()));
                }
            });
    while operation.get_state() == pulse::operation::State::Running {
        if let Err(e) = tick(&mut mainloop, stop, deadline) {
            operation.cancel();
            return Err(e);
        }
    }
    discovery_ready(context.get_state(), stop)?;
    let (sink_index, monitor_name) = monitor
        .borrow_mut()
        .take()
        .context("Default output has no monitor")?;
    let verified = Rc::new(RefCell::new(false));
    let found = verified.clone();
    let mut operation =
        context
            .introspect()
            .get_source_info_by_name(&monitor_name, move |result| {
                if let ListResult::Item(info) = result {
                    *found.borrow_mut() = info.monitor_of_sink == Some(sink_index);
                }
            });
    while operation.get_state() == pulse::operation::State::Running {
        if let Err(e) = tick(&mut mainloop, stop, deadline) {
            operation.cancel();
            return Err(e);
        }
    }
    discovery_ready(context.get_state(), stop)?;
    anyhow::ensure!(
        *verified.borrow(),
        "Source is not an output monitor; microphone capture refused"
    );
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::F32le,
        channels: 1,
        rate: 48_000,
    };
    let mut stream = Stream::new(&mut context, "System audio", &spec, None)
        .context("Could not create monitor stream")?;
    // Server enforces immovability, including device removal. Buffered audio cannot change origin.
    let attr = pulse::def::BufferAttr {
        maxlength: 19_200,
        fragsize: 3_840,
        ..Default::default()
    };
    stream.connect_record(Some(&monitor_name), Some(&attr), RECORD_FLAGS)?;
    while stream.get_state() != StreamState::Ready {
        anyhow::ensure!(
            !matches!(
                stream.get_state(),
                StreamState::Failed | StreamState::Terminated
            ),
            "Monitor stream failed"
        );
        tick(&mut mainloop, stop, deadline)?;
    }
    anyhow::ensure!(
        stream.get_device_name().is_some_and(|n| n == monitor_name),
        "Unexpected recording source"
    );
    let mut deadline = Instant::now() + TIMEOUT;
    while !stop.load(Ordering::Relaxed) {
        anyhow::ensure!(
            stream.get_state() == StreamState::Ready && context.get_state() == ContextState::Ready,
            "System audio connection ended"
        );
        match stream.peek()? {
            PeekResult::Data(data) => {
                copy_samples(data, producer)?;
                stream.discard()?;
                deadline = Instant::now() + TIMEOUT;
            }
            PeekResult::Hole(_) => {
                stream.discard()?;
                deadline = Instant::now() + TIMEOUT;
            }
            PeekResult::Empty => {}
        }
        tick(&mut mainloop, stop, deadline)?;
    }
    // These queue local teardown; they do not drain, flush, or wait for acknowledgements.
    let _ = stream.disconnect();
    context.disconnect();
    Ok(())
}

pub fn start(
    mut producer: HeapProd<f32>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    Ok(std::thread::Builder::new()
        .name("system-audio-capture".into())
        .spawn(move || {
            let _stop_on_exit = StopOnExit(stop.clone());
            if capture(&mut producer, &stop, None).is_err() {
                tracing::warn!("System audio capture stopped");
            }
        })?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::{
        traits::{Consumer, Split},
        HeapRb,
    };

    #[test]
    fn discovery_disconnect_refuses_next_stage_and_worker_exit_always_stops() {
        let stop = Arc::new(AtomicBool::new(false));
        // A callback may finish its lookup in the same iteration that the connection ends.
        assert!(discovery_ready(ContextState::Ready, &stop).is_ok());
        assert!(discovery_ready(ContextState::Failed, &stop).is_err());
        assert!(discovery_ready(ContextState::Terminated, &stop).is_err());
        let worker_stop = stop.clone();
        assert!(std::thread::spawn(move || {
            let _guard = StopOnExit(worker_stop);
            panic!("simulated binding panic");
        })
        .join()
        .is_err());
        assert!(stop.load(Ordering::Relaxed));
        assert!(discovery_ready(ContextState::Ready, &stop).is_err());
    }

    #[test]
    fn samples_are_bounded_and_invalid_buffers_rejected() {
        assert!(RECORD_FLAGS.contains(FlagSet::DONT_MOVE));
        let (mut prod, mut cons) = HeapRb::<f32>::new(2).split();
        copy_samples(&[0.25f32.to_le_bytes(); 3].concat(), &mut prod).unwrap();
        assert_eq!(cons.try_pop(), Some(0.25));
        assert_eq!(cons.try_pop(), Some(0.25));
        assert!(cons.try_pop().is_none());
        assert!(copy_samples(&[0; 3], &mut prod).is_err());
        assert!(copy_samples(&f32::NAN.to_le_bytes(), &mut prod).is_err());
        assert!(copy_samples(&vec![0; 48_000 * 4 + 4], &mut prod).is_err());
    }

    #[test]
    #[ignore = "requires local PulseAudio / pipewire-pulse, pactl, and two output monitors"]
    fn server_rejects_monitor_stream_moves() {
        use std::process::Command;
        fn list(kind: &str) -> Vec<serde_json::Value> {
            let output = Command::new("pactl")
                .args(["-f", "json", "list", kind])
                .output()
                .unwrap();
            assert!(output.status.success());
            serde_json::from_slice(&output.stdout).unwrap()
        }
        let (mut prod, _cons) = HeapRb::<f32>::new(48_000).split();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || capture(&mut prod, &worker_stop, None));
        let deadline = Instant::now() + Duration::from_secs(3);
        let process_id = std::process::id().to_string();
        let recording = loop {
            let recording = list("source-outputs").into_iter().find(|v| {
                v["properties"]["application.name"] == "RustTalk system audio"
                    && v["properties"]["application.process.id"].as_str()
                        == Some(process_id.as_str())
            });
            if let Some(recording) = recording {
                break recording;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        let other_monitor = list("sources")
            .into_iter()
            .find(|v| {
                v["properties"]["device.class"] == "monitor" && v["index"] != recording["source"]
            })
            .expect("second monitor");
        // Server must reject the move, so even a delayed consumer cannot receive other-source buffers.
        let result = Command::new("pactl")
            .args([
                "move-source-output",
                &recording["index"].to_string(),
                other_monitor["name"].as_str().unwrap(),
            ])
            .output()
            .unwrap();
        stop.store(true, Ordering::Relaxed);
        let _ = worker.join().unwrap();
        assert!(
            !result.status.success(),
            "recording stream must be immovable"
        );
    }

    #[test]
    fn stalled_server_cannot_block_stop() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pulse.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = format!("unix:{}", path.display());
        let (mut prod, mut cons) = HeapRb::<f32>::new(48_000).split();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || capture(&mut prod, &worker_stop, Some(&server)));
        let (_connection, _) = listener.accept().unwrap(); // accept, but never answer authentication
        let start = Instant::now();
        stop.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(cons.try_pop().is_none());
    }
}
