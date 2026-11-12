use super::{Action, AudioControl, Bindings, Event, GlobalBinding, PttBinding};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[path = "hyprland.rs"]
mod hyprland;

static ACTIVE: AtomicBool = AtomicBool::new(false);

static STATUS: Mutex<String> = Mutex::new(String::new());
pub fn status() -> String {
    STATUS.lock().unwrap().clone()
}
fn set_status(value: impl Into<String>) {
    *STATUS.lock().unwrap() = value.into();
}

pub fn spawn_global_ptt(
    control: Arc<AudioControl>,
) -> (GlobalBinding, std::sync::mpsc::Receiver<Event>) {
    let binding: GlobalBinding = Arc::new(Mutex::new(Bindings::default()));
    let shared = binding.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            // Desktop entry monitors may not have indexed a first-launch entry yet.
            for attempt in 0..4 {
                match tokio::time::timeout(
                    Duration::from_secs(3),
                    ashpd::register_host_app("org.rusttalk.RustTalk".parse().unwrap()),
                )
                .await
                {
                    Ok(Ok(())) => break,
                    error if attempt == 3 => {
                        tracing::warn!("Could not register desktop application identity: {error:?}")
                    }
                    _ => tokio::time::sleep(Duration::from_millis(250)).await,
                }
            }
            loop {
                let current = {
                    let binding = shared.lock().unwrap();
                    if !binding.is_empty() {
                        ACTIVE.store(true, Ordering::Release);
                    }
                    *binding
                };
                if !current.is_empty() {
                    set_status("Waiting for desktop shortcut permission…");
                    if let Err(error) = run(&shared, &control, current, &tx).await {
                        set_status(format!(
                            "Global shortcut unavailable: {error}. Toggle off/on to retry."
                        ));
                        tracing::warn!("Wayland shortcut: {error:#}");
                    }
                    if current.keys[0].is_some() {
                        control.ptt_pressed.store(false, Ordering::Relaxed);
                    }
                    ACTIVE.store(false, Ordering::Release);
                    // Do not reopen a denied permission dialog every frame.
                    while *shared.lock().unwrap() == current {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
    });
    (binding, rx)
}

pub fn shutdown_global_ptt(binding: &GlobalBinding) {
    binding.lock().unwrap().set([None; 3]);
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while ACTIVE.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if ACTIVE.load(Ordering::Acquire) {
        tracing::warn!("Timed out closing desktop shortcut session");
    }
}

async fn run(
    shared: &GlobalBinding,
    control: &AudioControl,
    current: Bindings,
    tx: &std::sync::mpsc::SyncSender<Event>,
) -> anyhow::Result<()> {
    for i in 0..3 {
        for j in i + 1..3 {
            anyhow::ensure!(
                current.keys[i].is_none() || current.keys[i] != current.keys[j],
                "Shortcuts must use different keys"
            );
        }
    }
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session().await?;
    let session_path: String = serde_json::from_value(serde_json::to_value(&session)?)?;
    let mut desktop_bindings = Vec::new();
    let result = async {
        let mut activated = portal.receive_activated().await?;
        let mut deactivated = portal.receive_deactivated().await?;
        let mut closed = session.receive_closed().await?;
        let mut shortcuts = Vec::new();
        for action in Action::ALL {
            if let Some(key) = current.keys[action.index()] {
                let trigger = hyprland::portal_trigger(key)?;
                shortcuts.push(NewShortcut::new(action.id(), action.label()).preferred_trigger(Some(trigger.as_str())));
            }
        }
        let bind = portal.bind_shortcuts(&session, &shortcuts, None);
        tokio::pin!(bind);
        let response = loop {
            tokio::select! {
                result = &mut bind => break result?.response()?,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if *shared.lock().unwrap() != current { return Ok(()); }
                }
            }
        };
        let mut descriptions = Vec::new();
        for action in Action::ALL {
            if let Some(key) = current.keys[action.index()] {
                let shortcut = response
                    .shortcuts()
                    .iter()
                    .find(|s| s.id() == action.id())
                    .ok_or_else(|| anyhow::anyhow!("Desktop did not bind {}", action.label()))?;
                let binding = hyprland::Binding::install(action, key, &session_path).await?;
                let trigger = if binding.is_some() {
                    key.name()
                } else {
                    shortcut.trigger_description().to_string()
                };
                descriptions.push(format!(
                    "{}: {}",
                    action.label(),
                    if trigger.is_empty() { "desktop assigned" } else { &trigger }
                ));
                if let Some(binding) = binding {
                    desktop_bindings.push(binding);
                }
            }
        }
        if desktop_bindings.is_empty() {
            set_status(format!("Desktop shortcuts: {}", descriptions.join(", ")));
        } else {
            set_status("");
        }
        let mut last_ptt_event = (Duration::ZERO, false);
        let mut last_activation = [Duration::ZERO; 3];
        let maintain_bindings = async {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                for binding in &desktop_bindings {
                    if binding.refresh().await? && current.keys[0].is_some() {
                        control.ptt_pressed.store(false, Ordering::Relaxed);
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        tokio::pin!(maintain_bindings);
        loop {
            tokio::select! {
                event = activated.next() => {
                    let event = event.ok_or_else(|| anyhow::anyhow!("Desktop disconnected"))?;
                    let binding = shared.lock().unwrap();
                    if event.session_handle().as_str() != session_path || *binding != current { continue; }
                    let Some(action) = Action::ALL.into_iter().find(|a| a.id() == event.shortcut_id()) else { continue; };
                    if current.keys[action.index()].is_none() { continue; }
                    if action == Action::PushToTalk {
                        if accept_event(&mut last_ptt_event, event.timestamp(), true) { control.ptt_pressed.store(true, Ordering::Relaxed); }
                    } else if accept_activation(&mut last_activation[action.index()], event.timestamp()) {
                        // Portals emit Activated once per activation; release stream order must not lose a quick tap.
                        let _ = tx.try_send(Event { action, generation: current.generation });
                    }
                }
                event = deactivated.next() => {
                    let event = event.ok_or_else(|| anyhow::anyhow!("Desktop disconnected"))?;
                    if event.session_handle().as_str() == session_path && event.shortcut_id() == Action::PushToTalk.id()
                        && accept_event(&mut last_ptt_event, event.timestamp(), false) {
                        control.ptt_pressed.store(false, Ordering::Relaxed);
                    }
                }
                _ = closed.next() => anyhow::bail!("Desktop closed the shortcut session"),
                result = &mut maintain_bindings => { result?; }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if *shared.lock().unwrap() != current { return Ok(()); }
                }
            }
        }
    }
    .await;
    if current.keys[0].is_some() {
        control.ptt_pressed.store(false, Ordering::Relaxed);
    }
    for binding in desktop_bindings {
        binding.remove().await;
    }
    let _ = session.close().await;
    result
}

fn accept_activation(last: &mut Duration, timestamp: Duration) -> bool {
    if timestamp <= *last {
        return false;
    }
    *last = timestamp;
    true
}

// Separate portal signal streams can be polled out of order. Release wins ties.
fn accept_event(last: &mut (Duration, bool), timestamp: Duration, pressed: bool) -> bool {
    if timestamp < last.0 || (timestamp == last.0 && (pressed || !last.1)) {
        return false;
    }
    *last = (timestamp, pressed);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a running Wayland desktop and manual F8 press/release"]
    fn wayland_ptt_live() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .try_init();
        crate::install::refresh_installed();
        let control = Arc::new(AudioControl::new(false, 0.05));
        let (binding, _events) = spawn_global_ptt(control.clone());
        binding
            .lock()
            .unwrap()
            .set([Some(PttBinding::key(eframe::egui::Key::F8)), None, None]);
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        let mut pressed = false;
        let mut released = false;
        let mut previous_status = String::new();
        while std::time::Instant::now() < deadline {
            let now = status();
            if now != previous_status {
                println!("Portal: {now}");
                previous_status = now;
            }
            let down = control.ptt_pressed.load(Ordering::Relaxed);
            if down && !pressed {
                println!("F8 pressed");
                pressed = true;
            }
            if !down && pressed {
                println!("F8 released");
                released = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        shutdown_global_ptt(&binding);
        assert!(
            !ACTIVE.load(Ordering::Acquire),
            "Shortcut worker must finish cleanup"
        );
        println!("Portal: {}", status());
        assert!(
            pressed && released,
            "F8 must activate and deactivate through the desktop portal"
        );
    }

    #[test]
    fn toggle_activation_is_independent_of_release_stream_order() {
        let mut last = Duration::ZERO;
        assert!(accept_activation(&mut last, Duration::from_millis(10)));
        assert!(
            !accept_activation(&mut last, Duration::from_millis(10)),
            "duplicate activation"
        );
        assert!(
            !accept_activation(&mut last, Duration::from_millis(9)),
            "stale activation"
        );
        assert!(
            accept_activation(&mut last, Duration::from_millis(20)),
            "next press"
        );
    }

    #[test]
    fn release_before_buffered_press_does_not_reopen_microphone() {
        let mut last = (Duration::ZERO, false);
        assert!(accept_event(&mut last, Duration::from_millis(20), false));
        assert!(!accept_event(&mut last, Duration::from_millis(10), true));
        assert!(!accept_event(&mut last, Duration::from_millis(20), true));
        assert!(accept_event(&mut last, Duration::from_millis(30), true));
        assert!(accept_event(&mut last, Duration::from_millis(30), false));
    }
}
