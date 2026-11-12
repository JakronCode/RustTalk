//! Windows active-audio-session discovery and process-tree loopback capture.
use std::collections::HashSet;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ringbuf::{traits::Producer, HeapProd};
use windows::core::{implement, Interface, Ref, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use windows::Win32::System::Variant::VT_BLOB;

use super::share_source::{AudioApplication, RawApplication};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationSource {
    pid: u32,
    created: u64,
}

struct Com;
impl Com {
    fn new() -> Result<Self> {
        // Each worker owns an MTA and releases every COM interface before uninitializing it.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn process(pid: u32) -> Result<(OwnedHandle, u64, String)> {
    anyhow::ensure!(
        pid != 0 && pid != std::process::id(),
        "Cannot share RustTalk or system processes"
    );
    // Keep the selected process handle open throughout capture to prevent PID reuse.
    let process = OwnedHandle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )?
    });
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let mut path = [0u16; 32768];
    let mut len = path.len() as u32;
    unsafe {
        GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)?;
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )?;
    }
    let path = String::from_utf16_lossy(&path[..len as usize]);
    let name = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("Application")
        .chars()
        .take(256)
        .collect();
    Ok((
        process,
        ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
        name,
    ))
}

pub fn applications(stop: &AtomicBool) -> Result<Vec<AudioApplication>> {
    let _com = Com::new()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let devices = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)? };
    let mut applications = Vec::new();
    let mut seen = HashSet::new();
    for i in 0..unsafe { devices.GetCount()? }.min(32) {
        anyhow::ensure!(!stop.load(Ordering::Relaxed), "Discovery stopped");
        let device = unsafe { devices.Item(i)? };
        let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) })
        else {
            continue;
        };
        let sessions = unsafe { manager.GetSessionEnumerator()? };
        for j in 0..unsafe { sessions.GetCount()? }.clamp(0, 512) {
            anyhow::ensure!(!stop.load(Ordering::Relaxed), "Discovery stopped");
            let Ok(session) = (unsafe { sessions.GetSession(j) }) else {
                continue;
            };
            if unsafe { session.GetState()? } != AudioSessionStateActive {
                continue;
            }
            let session: IAudioSessionControl2 = session.cast()?;
            let pid = unsafe { session.GetProcessId()? };
            if !seen.insert(pid) || applications.len() >= 128 {
                continue;
            }
            if let Ok((_process, created, name)) = process(pid) {
                applications.push(RawApplication {
                    app: name,
                    stream: None,
                    icon_hint: None,
                    source: ApplicationSource { pid, created },
                });
            }
        }
    }
    Ok(super::share_source::finish(applications))
}

static ACTIVATING: AtomicBool = AtomicBool::new(false);
struct ActivationLease;
impl Drop for ActivationLease {
    fn drop(&mut self) {
        ACTIVATING.store(false, Ordering::Relaxed);
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activation {
    ready: mpsc::SyncSender<()>,
    // Kept alive by the COM callback if cancellation wins before activation completes.
    params: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    _lease: ActivationLease,
}
impl IActivateAudioInterfaceCompletionHandler_Impl for Activation_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.ready.try_send(());
        Ok(())
    }
}

fn activate(pid: u32, stop: &AtomicBool) -> Result<IAudioClient> {
    anyhow::ensure!(
        ACTIVATING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok(),
        "Previous application audio activation is still finishing"
    );
    let (tx, rx) = mpsc::sync_channel(1);
    let activation = Activation {
        ready: tx,
        params: Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        }),
        _lease: ActivationLease,
    };
    let params = activation.params.as_ref() as *const AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8;
    // PROPVARIANT borrows the callback-owned blob; never PropVariantClear this borrowed allocation.
    let variant = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: params,
                    },
                },
                ..Default::default()
            }),
        },
    };
    let callback: IActivateAudioInterfaceCompletionHandler = activation.into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&variant),
            &callback,
        )
    }
    .context("Per-app capture requires Windows 10 build 20348 or newer")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        anyhow::ensure!(!stop.load(Ordering::Relaxed), "Audio capture stopped");
        anyhow::ensure!(
            Instant::now() < deadline,
            "Application audio activation timed out"
        );
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let mut result = windows::core::HRESULT(0);
    let mut interface = None;
    unsafe {
        operation.GetActivateResult(&mut result, &mut interface)?;
    }
    result.ok().context(
        "Application audio activation failed; Windows 10 build 20348 or newer is required",
    )?;
    Ok(interface
        .context("Audio activation returned no interface")?
        .cast()?)
}

struct Running(IAudioClient);
impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}
struct StopOnExit(Arc<AtomicBool>);
impl Drop for StopOnExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn capture(
    application: ApplicationSource,
    producer: &mut HeapProd<f32>,
    stop: &AtomicBool,
) -> Result<()> {
    let _com = Com::new()?;
    let (process, created, _) = process(application.pid)?;
    anyhow::ensure!(
        created == application.created,
        "Selected application exited; refresh sources"
    );
    let client = activate(application.pid, stop)?;
    anyhow::ensure!(!stop.load(Ordering::Relaxed), "Audio capture stopped");
    let format = WAVEFORMATEX {
        wFormatTag: 3,
        nChannels: 2,
        nSamplesPerSec: 48_000,
        nAvgBytesPerSec: 384_000,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            0,
            0,
            &format,
            None,
        )?;
    }
    let event = OwnedHandle(unsafe { CreateEventW(None, false, false, None)? });
    unsafe {
        client.SetEventHandle(event.0)?;
    }
    let capture: IAudioCaptureClient = unsafe { client.GetService()? };
    unsafe {
        client.Start()?;
    }
    let _running = Running(client);
    while !stop.load(Ordering::Relaxed) {
        // Bound each event wait so stopping or an exited application is noticed promptly.
        let wait = unsafe { WaitForSingleObject(event.0, 10) };
        anyhow::ensure!(
            wait == WAIT_OBJECT_0 || wait == WAIT_TIMEOUT,
            "Audio event wait failed"
        );
        if stop.load(Ordering::Relaxed) {
            break;
        }
        anyhow::ensure!(
            unsafe { WaitForSingleObject(process.0, 0) } == WAIT_TIMEOUT,
            "Selected application exited"
        );
        for _ in 0..16 {
            if stop.load(Ordering::Relaxed) || unsafe { capture.GetNextPacketSize()? } == 0 {
                break;
            }
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
            unsafe {
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            }
            let result = (|| -> Result<()> {
                anyhow::ensure!(frames <= 48_000, "Oversized application audio buffer");
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    for _ in 0..frames {
                        let _ = producer.try_push(0.0);
                    }
                } else {
                    anyhow::ensure!(
                        !data.is_null() || frames == 0,
                        "Missing application audio buffer"
                    );
                    if frames == 0 {
                        return Ok(());
                    }
                    // WASAPI owns this bounded buffer until ReleaseBuffer; copy without assuming alignment.
                    let bytes = unsafe { std::slice::from_raw_parts(data, frames as usize * 8) };
                    for frame in bytes.as_chunks::<8>().0 {
                        let left = f32::from_le_bytes(frame[..4].try_into()?);
                        let right = f32::from_le_bytes(frame[4..].try_into()?);
                        anyhow::ensure!(
                            left.is_finite() && right.is_finite(),
                            "Invalid application audio sample"
                        );
                        let _ = producer.try_push((left * 0.5 + right * 0.5).clamp(-1.0, 1.0));
                    }
                }
                Ok(())
            })();
            unsafe {
                capture.ReleaseBuffer(frames)?;
            }
            result?;
        }
    }
    Ok(())
}

pub fn start(
    application: ApplicationSource,
    mut producer: HeapProd<f32>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    Ok(std::thread::Builder::new()
        .name("application-audio".into())
        .spawn(move || {
            let _stop = StopOnExit(stop.clone());
            if capture(application, &mut producer, &stop).is_err() {
                tracing::warn!("Application audio capture stopped");
            }
        })?)
}
