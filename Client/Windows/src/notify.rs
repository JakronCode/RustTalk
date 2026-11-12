//! Windows toast notifications ("pop-ups") for new messages.
//!
//! Installed: shown as "RustTalk" with our icon (app id registered at
//! install). Dev/portable: Windows shows them under "PowerShell".

#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use tauri_winrt_notification::Toast;

#[cfg(windows)]
use crate::install::{self, Mode};

/// Shows a toast; `on_click` runs (on another thread) when it is clicked.
#[cfg(windows)]
pub fn show(title: String, body: String, on_click: impl Fn() + Send + 'static) {
    let _ = std::thread::Builder::new().name("toast".into()).spawn(move || {
        let app_id = if install::mode() == Mode::Installed {
            install::AUMID
        } else {
            Toast::POWERSHELL_APP_ID
        };
        let result = Toast::new(app_id)
            .title(&title)
            .text1(&body)
            .sound(None) // RustTalk plays its own sound
            .on_activated(move |_| {
                on_click();
                Ok(())
            })
            .show();
        match result {
            // Keep the thread (and with it the click handler) alive while the toast is visible.
            Ok(()) => std::thread::sleep(Duration::from_secs(10)),
            Err(e) => tracing::warn!("Toast konnte nicht angezeigt werden: {e}"),
        }
    });
}

#[cfg(target_os = "linux")]
pub fn show(title: String, body: String, on_click: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        match notify_rust::Notification::new()
            .appname("RustTalk")
            .summary(&title)
            .body(&body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"))
            .icon("org.rusttalk.RustTalk")
            .action("default", "Open channel")
            .hint(notify_rust::Hint::SuppressSound(true))
            .timeout(10_000)
            .show()
        {
            Ok(handle) => {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    runtime.block_on(async {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(15),
                            handle.wait_for_action_async(|action| {
                                if matches!(action, notify_rust::NotificationResponse::Default) {
                                    on_click();
                                }
                            }),
                        )
                        .await;
                        handle.close_async().await;
                    });
                }
            }
            Err(error) => tracing::warn!("Desktop notification: {error}"),
        }
    });
}

/// Title + body of a message toast. `preview = false` hides the text.
pub fn message_text(from: &str, channel: &str, text: &str, has_image: bool, preview: bool) -> (String, String) {
    let title = format!("{from}  ·  {channel}");
    let body = if !preview {
        "New message".to_string()
    } else if text.trim().is_empty() && has_image {
        "[image]".to_string()
    } else {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut s: String = flat.chars().take(140).collect();
        if flat.chars().count() > 140 {
            s.push_str("...");
        }
        s
    };
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "shows a real desktop notification"]
    #[cfg(target_os = "linux")]
    fn linux_notification_live() {
        let handle = notify_rust::Notification::new()
            .appname("RustTalk")
            .summary("RustTalk notifications work")
            .body("Wayland desktop notification test")
            .hint(notify_rust::Hint::SuppressSound(true))
            .timeout(3000)
            .show()
            .expect("desktop notification");
        std::thread::sleep(std::time::Duration::from_secs(3));
        handle.close();
    }

    #[test]
    fn toast_texts() {
        assert_eq!(
            message_text("anna", "general", "hi\nthere", false, true),
            ("anna  ·  general".into(), "hi there".into())
        );
        assert_eq!(message_text("anna", "general", "", true, true).1, "[image]");
        assert_eq!(message_text("anna", "general", "secret", false, false).1, "New message", "privacy");
        assert!(message_text("a", "b", &"x".repeat(300), false, true).1.ends_with("..."));
    }

    /// Shows a real toast (manual: `cargo test -p rusttalk-client toast_shows -- --ignored`).
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn toast_shows() {
        let app_id = Toast::POWERSHELL_APP_ID;
        Toast::new(app_id)
            .title("RustTalk test")
            .text1("Toast notifications work")
            .sound(None)
            .show()
            .expect("toast");
    }
}
