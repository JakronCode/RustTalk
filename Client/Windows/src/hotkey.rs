//! Push-to-talk binding: config names -> keys / mouse buttons, plus the
//! global PTT poller (works while RustTalk is in the background).
//!
//! The poller asks the OS for the current key state every few ms
//! (`device_query`, `GetAsyncKeyState` on Windows). No keyboard hook: the
//! key still reaches other apps, no admin rights needed.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use device_query::{DeviceQuery, DeviceState, Keycode};
use eframe::egui;

use crate::audio::AudioControl;

/// What has to be held down to talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttBinding {
    Key(egui::Key),
    /// Side mouse button: 4 = back (XBUTTON1), 5 = forward (XBUTTON2).
    Mouse(u8),
}

/// Config name for a side mouse button ("Mouse4" / "Mouse5").
pub fn mouse_name(button: egui::PointerButton) -> Option<&'static str> {
    match button {
        egui::PointerButton::Extra1 => Some("Mouse4"),
        egui::PointerButton::Extra2 => Some("Mouse5"),
        _ => None,
    }
}

/// Parses `config.ptt_key` (egui key name, legacy uppercase names, "Mouse4/5").
pub fn parse_binding(name: &str) -> Option<PttBinding> {
    match name.trim().to_ascii_lowercase().as_str() {
        "mouse4" => Some(PttBinding::Mouse(4)),
        "mouse5" => Some(PttBinding::Mouse(5)),
        _ => key_from_name(name).map(PttBinding::Key),
    }
}

impl PttBinding {
    /// Held down in this egui frame (window focused).
    pub fn is_down_in(&self, input: &egui::InputState) -> bool {
        match self {
            PttBinding::Key(k) => input.key_down(*k),
            PttBinding::Mouse(4) => input.pointer.button_down(egui::PointerButton::Extra1),
            PttBinding::Mouse(5) => input.pointer.button_down(egui::PointerButton::Extra2),
            PttBinding::Mouse(_) => false,
        }
    }
}

/// egui key -> OS key code for the global poller (US key positions).
pub fn keycode_for(key: egui::Key) -> Option<Keycode> {
    use egui::Key as E;
    use Keycode as K;
    Some(match key {
        E::A => K::A,
        E::B => K::B,
        E::C => K::C,
        E::D => K::D,
        E::E => K::E,
        E::F => K::F,
        E::G => K::G,
        E::H => K::H,
        E::I => K::I,
        E::J => K::J,
        E::K => K::K,
        E::L => K::L,
        E::M => K::M,
        E::N => K::N,
        E::O => K::O,
        E::P => K::P,
        E::Q => K::Q,
        E::R => K::R,
        E::S => K::S,
        E::T => K::T,
        E::U => K::U,
        E::V => K::V,
        E::W => K::W,
        E::X => K::X,
        E::Y => K::Y,
        E::Z => K::Z,
        E::Num0 => K::Key0,
        E::Num1 => K::Key1,
        E::Num2 => K::Key2,
        E::Num3 => K::Key3,
        E::Num4 => K::Key4,
        E::Num5 => K::Key5,
        E::Num6 => K::Key6,
        E::Num7 => K::Key7,
        E::Num8 => K::Key8,
        E::Num9 => K::Key9,
        E::F1 => K::F1,
        E::F2 => K::F2,
        E::F3 => K::F3,
        E::F4 => K::F4,
        E::F5 => K::F5,
        E::F6 => K::F6,
        E::F7 => K::F7,
        E::F8 => K::F8,
        E::F9 => K::F9,
        E::F10 => K::F10,
        E::F11 => K::F11,
        E::F12 => K::F12,
        E::F13 => K::F13,
        E::F14 => K::F14,
        E::F15 => K::F15,
        E::F16 => K::F16,
        E::F17 => K::F17,
        E::F18 => K::F18,
        E::F19 => K::F19,
        E::F20 => K::F20,
        E::Space => K::Space,
        E::Enter => K::Enter,
        E::Tab => K::Tab,
        E::Escape => K::Escape,
        E::Backspace => K::Backspace,
        E::Insert => K::Insert,
        E::Delete => K::Delete,
        E::Home => K::Home,
        E::End => K::End,
        E::PageUp => K::PageUp,
        E::PageDown => K::PageDown,
        E::ArrowUp => K::Up,
        E::ArrowDown => K::Down,
        E::ArrowLeft => K::Left,
        E::ArrowRight => K::Right,
        E::Backtick => K::Grave,
        E::Minus => K::Minus,
        E::Equals => K::Equal,
        E::OpenBracket => K::LeftBracket,
        E::CloseBracket => K::RightBracket,
        E::Backslash => K::BackSlash,
        E::Semicolon => K::Semicolon,
        E::Quote => K::Apostrophe,
        E::Comma => K::Comma,
        E::Period => K::Dot,
        E::Slash => K::Slash,
        _ => return None,
    })
}

/// Shared with the poller: `Some` = global PTT on with this binding.
pub type GlobalBinding = Arc<Mutex<Option<PttBinding>>>;

/// Starts the poller thread (runs for the app's lifetime, ~10 ms tick).
/// While the binding is `Some`, it alone sets `control.ptt_pressed`.
pub fn spawn_global_ptt(control: Arc<AudioControl>) -> GlobalBinding {
    let binding: GlobalBinding = Arc::new(Mutex::new(None));
    let shared = binding.clone();
    let _ = std::thread::Builder::new().name("global-ptt".into()).spawn(move || {
        let state = DeviceState::new();
        loop {
            let current = *shared.lock().unwrap();
            let Some(b) = current else {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            let down = match b {
                PttBinding::Key(k) => keycode_for(k).is_some_and(|code| state.get_keys().contains(&code)),
                PttBinding::Mouse(n) => state.get_mouse().button_pressed.get(n as usize).copied().unwrap_or(false),
            };
            control.ptt_pressed.store(down, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    binding
}

/// Maps a config string to an egui key.
pub fn key_from_name(name: &str) -> Option<egui::Key> {
    let n = name.trim();
    if n.chars().count() == 1 {
        let c = n.chars().next().unwrap().to_ascii_uppercase();
        return Some(match c {
            'A' => egui::Key::A,
            'B' => egui::Key::B,
            'C' => egui::Key::C,
            'D' => egui::Key::D,
            'E' => egui::Key::E,
            'F' => egui::Key::F,
            'G' => egui::Key::G,
            'H' => egui::Key::H,
            'I' => egui::Key::I,
            'J' => egui::Key::J,
            'K' => egui::Key::K,
            'L' => egui::Key::L,
            'M' => egui::Key::M,
            'N' => egui::Key::N,
            'O' => egui::Key::O,
            'P' => egui::Key::P,
            'Q' => egui::Key::Q,
            'R' => egui::Key::R,
            'S' => egui::Key::S,
            'T' => egui::Key::T,
            'U' => egui::Key::U,
            'V' => egui::Key::V,
            'W' => egui::Key::W,
            'X' => egui::Key::X,
            'Y' => egui::Key::Y,
            'Z' => egui::Key::Z,
            '0' => egui::Key::Num0,
            '1' => egui::Key::Num1,
            '2' => egui::Key::Num2,
            '3' => egui::Key::Num3,
            '4' => egui::Key::Num4,
            '5' => egui::Key::Num5,
            '6' => egui::Key::Num6,
            '7' => egui::Key::Num7,
            '8' => egui::Key::Num8,
            '9' => egui::Key::Num9,
            _ => return egui::Key::from_name(n),
        });
    }
    match n.to_ascii_uppercase().as_str() {
        "SPACE" => Some(egui::Key::Space),
        "TAB" => Some(egui::Key::Tab),
        "ENTER" => Some(egui::Key::Enter),
        "ESCAPE" => Some(egui::Key::Escape),
        "F1" => Some(egui::Key::F1),
        "F2" => Some(egui::Key::F2),
        "F3" => Some(egui::Key::F3),
        "F4" => Some(egui::Key::F4),
        "F5" => Some(egui::Key::F5),
        // Everything else by egui name (e.g. "F8", "Backtick", "PageUp").
        _ => egui::Key::from_name(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_letters() {
        assert_eq!(key_from_name("v"), Some(egui::Key::V));
        assert_eq!(key_from_name("A"), Some(egui::Key::A));
        assert_eq!(key_from_name("5"), Some(egui::Key::Num5));
        assert_eq!(key_from_name("SPACE"), Some(egui::Key::Space));
        assert_eq!(key_from_name("space"), Some(egui::Key::Space));
        assert_eq!(key_from_name("??"), None);
    }

    #[test]
    fn keybind_roundtrip_via_egui_names() {
        // What the settings store (key.name()) must map back to the key.
        for k in [egui::Key::V, egui::Key::Space, egui::Key::F8, egui::Key::Backtick, egui::Key::Num3] {
            assert_eq!(key_from_name(k.name()), Some(k), "{}", k.name());
        }
    }

    #[test]
    fn bindings_parse_keys_and_mouse_buttons() {
        assert_eq!(parse_binding("V"), Some(PttBinding::Key(egui::Key::V)));
        assert_eq!(parse_binding("Mouse4"), Some(PttBinding::Mouse(4)));
        assert_eq!(parse_binding("mouse5"), Some(PttBinding::Mouse(5)));
        assert_eq!(parse_binding("Mouse9"), None);
        assert_eq!(mouse_name(egui::PointerButton::Extra1), Some("Mouse4"));
        assert_eq!(mouse_name(egui::PointerButton::Primary), None);
        for b in [egui::PointerButton::Extra1, egui::PointerButton::Extra2] {
            assert!(parse_binding(mouse_name(b).unwrap()).is_some(), "{b:?}");
        }
    }

    #[test]
    fn os_key_and_mouse_state_can_be_queried() {
        // What the poller thread does every 10 ms (must not panic, mouse has buttons 1-5).
        let state = DeviceState::new();
        let _ = state.get_keys();
        assert!(state.get_mouse().button_pressed.len() >= 6, "index 4/5 = side buttons");
    }

    #[test]
    fn common_ptt_keys_have_os_keycodes() {
        for k in [egui::Key::V, egui::Key::Space, egui::Key::F8, egui::Key::Backtick, egui::Key::Num3, egui::Key::PageUp] {
            assert!(keycode_for(k).is_some(), "{k:?}");
        }
        assert_eq!(keycode_for(egui::Key::Backtick), Some(Keycode::Grave));
        assert_eq!(keycode_for(egui::Key::Num3), Some(Keycode::Key3));
    }
}
