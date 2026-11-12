//! Hyprland's portal registers actions but does not assign their preferred keys.
//! Own one runtime binding, leaving the user's persistent desktop config alone.
use super::{Action, PttBinding};
use crate::hotkey::{Button, Modifiers};
use anyhow::{bail, Context, Result};
use std::time::Duration;

pub struct Binding {
    action: Action,
    key: String,
    owner: String,
    modifiers: Modifiers,
}

async fn command(args: &[&str]) -> Result<String> {
    let mut child = tokio::process::Command::new("hyprctl");
    child.args(args).kill_on_drop(true);
    // Host tools must use host libraries, not the AppImage's bundled C++ runtime.
    if std::env::var_os("APPIMAGE").is_some() {
        match std::env::var_os("RUSTTALK_ORIGINAL_LD_LIBRARY_PATH") {
            Some(value) if !value.is_empty() => {
                child.env("LD_LIBRARY_PATH", value);
            }
            _ => {
                child.env_remove("LD_LIBRARY_PATH");
            }
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(2), child.output())
        .await
        .context("Hyprland shortcut request timed out")??;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    anyhow::ensure!(output.status.success(), "Hyprland shortcut failed: {text}");
    Ok(text)
}

impl Binding {
    pub async fn install(action: Action, key: PttBinding, owner: &str) -> Result<Option<Self>> {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            return Ok(None);
        }
        let binding = Self {
            action,
            key: key_name(key)?,
            owner: owner.to_string(),
            modifiers: key.modifiers,
        };
        binding.apply(true).await?;
        Ok(Some(binding))
    }

    pub async fn refresh(&self) -> Result<bool> {
        self.apply(false).await
    }

    fn chord(&self) -> String {
        let mut parts = Vec::new();
        for (held, name) in [
            (self.modifiers.ctrl, "CTRL"),
            (self.modifiers.alt, "ALT"),
            (self.modifiers.shift, "SHIFT"),
        ] {
            if held {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join(" + ")
    }

    fn description(&self) -> String {
        format!("RustTalk managed {}", self.action.id())
    }
    fn variable(&self) -> String {
        format!("rusttalk_shortcut_{}", self.action.index())
    }

    async fn apply(&self, replace: bool) -> Result<bool> {
        let binds: serde_json::Value = serde_json::from_str(&command(&["-j", "binds"]).await?)?;
        let binds = binds.as_array().context("Invalid Hyprland shortcut list")?;
        let matching: Vec<_> = binds
            .iter()
            .filter(|b| {
                b["key"]
                    .as_str()
                    .is_some_and(|key| key.eq_ignore_ascii_case(&self.key))
                    && b["modmask"].as_u64() == Some(modmask(self.modifiers))
            })
            .collect();
        if matching
            .iter()
            .any(|b| b["description"].as_str() != Some(self.description().as_str()))
        {
            bail!(
                "{} is already a desktop shortcut. Choose another shortcut key",
                self.key
            );
        }
        if !replace && !matching.is_empty() {
            return Ok(false);
        }
        // All interpolated key characters are validated below; owner is a portal object path.
        anyhow::ensure!(
            self.owner
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '_'),
            "Invalid shortcut owner"
        );
        let variable = self.variable();
        let script = format!(
            "if {variable} then pcall(function() {variable}.handle:remove() end) end; {variable} = {{owner=\"{}\", handle=hl.bind(\"{}\", hl.dsp.global(\"org.rusttalk.RustTalk:{}\"), {{description=\"{}\", non_consuming=true}})}}",
            self.owner, self.chord(), self.action.id(), self.description()
        );
        let response = command(&["eval", &script]).await?;
        anyhow::ensure!(
            response.trim() == "ok",
            "Could not bind {} in Hyprland: {response}",
            self.key
        );
        Ok(true)
    }

    pub async fn remove(self) {
        let variable = self.variable();
        let script = format!(
            "if {variable} and {variable}.owner == \"{}\" then {variable}.handle:remove(); {variable} = nil end",
            self.owner
        );
        if let Err(error) = command(&["eval", &script]).await {
            tracing::warn!("Could not release RustTalk desktop shortcut: {error}");
        }
    }
}

pub fn key_name(binding: PttBinding) -> Result<String> {
    use eframe::egui::Key;
    let name = match binding.button {
        Button::Mouse(4) => "mouse:275",
        Button::Mouse(5) => "mouse:276",
        Button::Mouse(_) => bail!("Unsupported mouse button"),
        Button::Key(key) => match key {
            Key::Space => "space",
            Key::Enter => "Return",
            Key::Backspace => "BackSpace",
            Key::PageUp => "Page_Up",
            Key::PageDown => "Page_Down",
            Key::ArrowUp => "Up",
            Key::ArrowDown => "Down",
            Key::ArrowLeft => "Left",
            Key::ArrowRight => "Right",
            Key::Backtick => "grave",
            Key::Minus => "minus",
            Key::Equals => "equal",
            Key::OpenBracket => "bracketleft",
            Key::CloseBracket => "bracketright",
            Key::Backslash => "backslash",
            Key::Semicolon => "semicolon",
            Key::Quote => "apostrophe",
            Key::Comma => "comma",
            Key::Period => "period",
            Key::Slash => "slash",
            _ => key.name(),
        },
    };
    anyhow::ensure!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'),
        "Unsupported global key"
    );
    Ok(name.to_string())
}

fn modmask(modifiers: Modifiers) -> u64 {
    u64::from(modifiers.shift) | (u64::from(modifiers.ctrl) << 2) | (u64::from(modifiers.alt) << 3)
}

pub fn portal_trigger(binding: PttBinding) -> Result<String> {
    let mut trigger = String::new();
    for (held, name) in [
        (binding.modifiers.ctrl, "CTRL+"),
        (binding.modifiers.alt, "ALT+"),
        (binding.modifiers.shift, "SHIFT+"),
    ] {
        if held {
            trigger.push_str(name);
        }
    }
    trigger.push_str(&key_name(binding)?);
    Ok(trigger)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "creates and removes a temporary Hyprland binding"]
    fn hyprland_selected_key_lifecycle() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut bindings = Vec::new();
            for (action, key) in Action::ALL.into_iter().zip([
                eframe::egui::Key::F21,
                eframe::egui::Key::F22,
                eframe::egui::Key::F23,
            ]) {
                bindings.push(
                    Binding::install(action, PttBinding::key(key), "/rusttalk_test")
                        .await
                        .unwrap()
                        .unwrap(),
                );
            }
            let rows: serde_json::Value =
                serde_json::from_str(&command(&["-j", "binds"]).await.unwrap()).unwrap();
            let descriptions: Vec<_> = bindings.iter().map(Binding::description).collect();
            for binding in &bindings {
                assert!(rows
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b["description"] == binding.description() && b["key"] == binding.key));
            }
            assert!(Binding::install(
                Action::Mute,
                PttBinding::key(eframe::egui::Key::F21),
                "/rusttalk_conflict"
            )
            .await
            .is_err());
            let last = bindings.pop().unwrap();
            last.remove().await;
            let remaining: serde_json::Value =
                serde_json::from_str(&command(&["-j", "binds"]).await.unwrap()).unwrap();
            for binding in &bindings {
                assert!(remaining
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|b| b["description"] == binding.description()));
            }
            for binding in bindings {
                binding.remove().await;
            }
            let after: serde_json::Value =
                serde_json::from_str(&command(&["-j", "binds"]).await.unwrap()).unwrap();
            assert!(!after
                .as_array()
                .unwrap()
                .iter()
                .any(|b| ["F21", "F22", "F23"].iter().any(|key| b["key"] == *key)
                    && descriptions.iter().any(|d| b["description"] == d.as_str())));
        });
    }

    #[test]
    fn modifier_chords_use_desktop_syntax_and_masks() {
        let key = crate::hotkey::parse_binding("Ctrl+Alt+Shift+F8").unwrap();
        assert_eq!(portal_trigger(key).unwrap(), "CTRL+ALT+SHIFT+F8");
        let binding = Binding {
            action: Action::Mute,
            key: key_name(key).unwrap(),
            owner: "/test".into(),
            modifiers: key.modifiers,
        };
        assert_eq!(binding.chord(), "CTRL + ALT + SHIFT + F8");
        assert_eq!(modmask(binding.modifiers), 13);
        assert_eq!(modmask(Modifiers::default()), 0);
    }

    #[test]
    fn desktop_keys_match_recorded_keys() {
        use eframe::egui::Key;
        for (key, expected) in [
            (Key::F8, "F8"),
            (Key::V, "V"),
            (Key::Num3, "3"),
            (Key::Space, "space"),
            (Key::ArrowUp, "Up"),
        ] {
            assert_eq!(key_name(PttBinding::key(key)).unwrap(), expected);
        }
        assert_eq!(key_name(PttBinding::mouse(4)).unwrap(), "mouse:275");
        assert!(key_name(PttBinding::mouse(9)).is_err());
    }
}
