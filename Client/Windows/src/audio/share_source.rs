//! Explicit local capture selection; never persisted or sent over the network.
use std::sync::atomic::AtomicBool;

#[cfg(target_os = "linux")]
pub use super::system_audio::ApplicationSource;
#[cfg(windows)]
pub use super::windows_audio::ApplicationSource;
#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationSource;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AudioShareSource {
    #[default]
    System,
    Application(ApplicationSource),
}

#[derive(Clone, Debug)]
pub struct AudioApplication {
    pub name: String,
    pub source: ApplicationSource,
}

pub fn applications(stop: &AtomicBool) -> anyhow::Result<Vec<AudioApplication>> {
    #[cfg(target_os = "linux")]
    return super::system_audio::applications(stop);
    #[cfg(windows)]
    return super::windows_audio::applications(stop);
    #[cfg(not(any(windows, target_os = "linux")))]
    anyhow::bail!("Application audio capture is unavailable on this platform")
}
