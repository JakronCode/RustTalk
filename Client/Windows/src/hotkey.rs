//! Push-to-talk binding: config names -> keys / mouse buttons, plus the
//! global PTT poller (works while RustTalk is in the background).
//!
//! The poller asks the OS for the current key state every few ms
//! (`device_query`, `GetAsyncKeyState` on Windows). No keyboard hook: the
//! key still reaches other apps, no admin rights needed.

#[cfg(windows)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use device_query::{DeviceQuery, DeviceState, Keycode};
use eframe::egui;

use crate::audio::AudioControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Key(egui::Key),
    Mouse(u8),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl From<egui::Modifiers> for Modifiers {
    fn from(value: egui::Modifiers) -> Self {
        Self {
            ctrl: value.ctrl,
            alt: value.alt,
            shift: value.shift,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PttBinding {
    pub button: Button,
    pub modifiers: Modifiers,
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
    let mut parts = name.trim().rsplitn(2, '+');
    let button = parts.next()?.trim();
    let mut binding = match button.to_ascii_lowercase().as_str() {
        "mouse4" => PttBinding::mouse(4),
        "mouse5" => PttBinding::mouse(5),
        _ => PttBinding::key(key_from_name(button)?),
    };
    if let Some(prefix) = parts.next() {
        for modifier in prefix.split('+') {
            let flag = match modifier.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut binding.modifiers.ctrl,
                "alt" => &mut binding.modifiers.alt,
                "shift" => &mut binding.modifiers.shift,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
    }
    Some(binding)
}

impl PttBinding {
    pub fn key(key: egui::Key) -> Self {
        Self {
            button: Button::Key(key),
            modifiers: Modifiers::default(),
        }
    }

    pub fn mouse(button: u8) -> Self {
        Self {
            button: Button::Mouse(button),
            modifiers: Modifiers::default(),
        }
    }

    pub fn name(self) -> String {
        let mut name = String::new();
        for (held, label) in [
            (self.modifiers.ctrl, "Ctrl+"),
            (self.modifiers.alt, "Alt+"),
            (self.modifiers.shift, "Shift+"),
        ] {
            if held {
                name.push_str(label);
            }
        }
        name.push_str(&match self.button {
            Button::Key(key) => key.name().to_string(),
            Button::Mouse(button) => format!("Mouse{button}"),
        });
        name
    }

    /// Exact modifiers prevent F8 from also firing for Ctrl+F8.
    pub fn is_down_in(&self, input: &egui::InputState) -> bool {
        self.modifiers == Modifiers::from(input.modifiers)
            && match self.button {
                Button::Key(k) => input.key_down(k),
                Button::Mouse(4) => input.pointer.button_down(egui::PointerButton::Extra1),
                Button::Mouse(5) => input.pointer.button_down(egui::PointerButton::Extra2),
                Button::Mouse(_) => false,
            }
    }
}

/// Capture the event's modifiers, not the state at the end of the frame.
pub fn recorded_binding(event: &egui::Event) -> Option<Option<String>> {
    let (mut binding, modifiers) = match event {
        egui::Event::Key {
            key: egui::Key::Escape,
            pressed: true,
            ..
        } => return Some(None),
        egui::Event::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } => (PttBinding::key(*key), *modifiers),
        egui::Event::PointerButton {
            button,
            pressed: true,
            modifiers,
            ..
        } => (parse_binding(mouse_name(*button)?)?, *modifiers),
        _ => return None,
    };
    binding.modifiers = modifiers.into();
    Some(Some(binding.name()))
}

/// egui key -> OS key code for the global poller (US key positions).
#[cfg(windows)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PushToTalk,
    Mute,
    Deafen,
}
impl Action {
    pub const ALL: [Self; 3] = [Self::PushToTalk, Self::Mute, Self::Deafen];
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::PushToTalk => "push-to-talk",
            Self::Mute => "toggle-mute",
            Self::Deafen => "toggle-deafen",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::PushToTalk => "Push to talk",
            Self::Mute => "Toggle mute",
            Self::Deafen => "Toggle deafen",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bindings {
    pub keys: [Option<PttBinding>; 3],
    pub generation: u64,
}
impl Bindings {
    pub fn set(&mut self, keys: [Option<PttBinding>; 3]) -> bool {
        if self.keys == keys {
            return false;
        }
        self.keys = keys;
        self.generation = self.generation.wrapping_add(1);
        true
    }
    pub fn is_empty(self) -> bool {
        self.keys.iter().all(Option::is_none)
    }
}
pub type GlobalBinding = Arc<Mutex<Bindings>>;
#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub action: Action,
    pub generation: u64,
}

/// Shared edge logic is tested on Linux too, even though Windows polls OS state.
#[cfg(any(windows, test))]
#[derive(Default)]
struct KeyEdges {
    generation: Option<u64>,
    down: [bool; 3],
}
#[cfg(any(windows, test))]
impl KeyEdges {
    fn poll(&mut self, bindings: Bindings, down: [bool; 3]) -> [bool; 3] {
        let mut pressed = [false; 3];
        if self.generation == Some(bindings.generation) {
            for i in 0..3 {
                pressed[i] = bindings.keys[i].is_some() && down[i] && !self.down[i];
            }
        }
        self.generation = Some(bindings.generation);
        self.down = down;
        pressed
    }
}

#[cfg(windows)]
pub fn spawn_global_ptt(
    control: Arc<AudioControl>,
) -> (GlobalBinding, std::sync::mpsc::Receiver<Event>) {
    let binding: GlobalBinding = Arc::new(Mutex::new(Bindings::default()));
    let shared = binding.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let _ = std::thread::Builder::new()
        .name("global-shortcuts".into())
        .spawn(move || {
            let state = DeviceState::new();
            let mut edges = KeyEdges::default();
            loop {
                let current = *shared.lock().unwrap();
                if current.is_empty() {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                let keys = state.get_keys();
                let mouse = state.get_mouse();
                let modifiers = Modifiers {
                    ctrl: keys.contains(&Keycode::LControl) || keys.contains(&Keycode::RControl),
                    alt: keys.contains(&Keycode::LAlt) || keys.contains(&Keycode::RAlt),
                    shift: keys.contains(&Keycode::LShift) || keys.contains(&Keycode::RShift),
                };
                let down = current.keys.map(|binding| {
                    binding.is_some_and(|binding| {
                        binding.modifiers == modifiers
                            && !keys.contains(&Keycode::LMeta)
                            && !keys.contains(&Keycode::RMeta)
                            && match binding.button {
                                Button::Key(k) => {
                                    keycode_for(k).is_some_and(|code| keys.contains(&code))
                                }
                                Button::Mouse(n) => mouse
                                    .button_pressed
                                    .get(n as usize)
                                    .copied()
                                    .unwrap_or(false),
                            }
                    })
                });
                let pressed = edges.poll(current, down);
                let active = shared.lock().unwrap();
                if *active == current {
                    if current.keys[0].is_some() {
                        control.ptt_pressed.store(down[0], Ordering::Relaxed);
                    }
                    for action in [Action::Mute, Action::Deafen] {
                        if pressed[action.index()] {
                            let _ = tx.try_send(Event {
                                action,
                                generation: current.generation,
                            });
                        }
                    }
                }
                drop(active);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    (binding, rx)
}

#[cfg(target_os = "linux")]
#[path = "linux/hotkey.rs"]
mod wayland;
#[cfg(target_os = "linux")]
pub use wayland::{shutdown_global_ptt, spawn_global_ptt, status};

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
    fn chords_record_roundtrip_and_match_exact_modifiers() {
        let event = egui::Event::Key {
            key: egui::Key::F8,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        assert_eq!(recorded_binding(&event), Some(Some("Ctrl+F8".into())));
        for name in [
            "F8",
            "Ctrl+F8",
            "Ctrl+Alt+Shift+F8",
            "Alt+Mouse5",
            "Shift+Plus",
        ] {
            let binding = parse_binding(name).unwrap();
            assert_eq!(parse_binding(&binding.name()), Some(binding));
        }
        assert_eq!(
            parse_binding("shift+control+F8"),
            parse_binding("Ctrl+Shift+F8")
        );
        for name in [
            "Ctrl+",
            "Ctrl+Ctrl+F8",
            "Unknown+F8",
            "Ctrl++F8",
            "Alt+Mouse9",
        ] {
            assert_eq!(parse_binding(name), None, "{name}");
        }
        let ctrl = parse_binding("Ctrl+F8").unwrap();
        let plain = parse_binding("F8").unwrap();
        let context = egui::Context::default();
        let raw = egui::RawInput {
            modifiers: egui::Modifiers::CTRL,
            events: vec![event],
            ..Default::default()
        };
        let _ = context.run(raw, |ctx| {
            ctx.input(|input| {
                assert!(ctrl.is_down_in(input));
                assert!(!plain.is_down_in(input));
            })
        });
        let raw = egui::RawInput {
            modifiers: egui::Modifiers::NONE,
            ..Default::default()
        };
        let _ = context.run(raw, |ctx| {
            ctx.input(|input| {
                assert!(!ctrl.is_down_in(input), "releasing Ctrl releases the chord");
                assert!(plain.is_down_in(input));
            })
        });
        let mouse = egui::Event::PointerButton {
            pos: egui::Pos2::ZERO,
            button: egui::PointerButton::Extra2,
            pressed: true,
            modifiers: egui::Modifiers::ALT,
        };
        assert_eq!(recorded_binding(&mouse), Some(Some("Alt+Mouse5".into())));
    }

    #[test]
    fn toggle_edges_fire_once_and_rebinding_does_not_toggle_held_keys() {
        let mut bindings = Bindings::default();
        bindings.set([
            None,
            Some(PttBinding::key(egui::Key::F10)),
            Some(PttBinding::mouse(5)),
        ]);
        let mut edges = KeyEdges::default();
        assert_eq!(edges.poll(bindings, [false; 3]), [false; 3]);
        assert_eq!(
            edges.poll(bindings, [false, true, false]),
            [false, true, false]
        );
        assert_eq!(
            edges.poll(bindings, [false, true, false]),
            [false; 3],
            "holding does not repeat"
        );
        assert_eq!(
            edges.poll(bindings, [false, false, true]),
            [false, false, true]
        );
        let old_generation = bindings.generation;
        bindings.set([None; 3]);
        assert_ne!(
            old_generation, bindings.generation,
            "queued events become stale on disconnect/recording"
        );
        assert_eq!(edges.poll(bindings, [false; 3]), [false; 3]);
        bindings.set([None, Some(PttBinding::key(egui::Key::F10)), None]);
        assert_eq!(
            edges.poll(bindings, [false, true, false]),
            [false; 3],
            "already-held key must be released first"
        );
        edges.poll(bindings, [false; 3]);
        assert_eq!(
            edges.poll(bindings, [false, true, false]),
            [false, true, false]
        );
    }

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
        for k in [
            egui::Key::V,
            egui::Key::Space,
            egui::Key::F8,
            egui::Key::Backtick,
            egui::Key::Num3,
        ] {
            assert_eq!(key_from_name(k.name()), Some(k), "{}", k.name());
        }
    }

    #[test]
    fn bindings_parse_keys_and_mouse_buttons() {
        assert_eq!(parse_binding("V"), Some(PttBinding::key(egui::Key::V)));
        assert_eq!(parse_binding("Mouse4"), Some(PttBinding::mouse(4)));
        assert_eq!(parse_binding("mouse5"), Some(PttBinding::mouse(5)));
        assert_eq!(parse_binding("Mouse9"), None);
        assert_eq!(mouse_name(egui::PointerButton::Extra1), Some("Mouse4"));
        assert_eq!(mouse_name(egui::PointerButton::Primary), None);
        for b in [egui::PointerButton::Extra1, egui::PointerButton::Extra2] {
            assert!(parse_binding(mouse_name(b).unwrap()).is_some(), "{b:?}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn os_key_and_mouse_state_can_be_queried() {
        // What the poller thread does every 10 ms (must not panic, mouse has buttons 1-5).
        let state = DeviceState::new();
        let _ = state.get_keys();
        assert!(
            state.get_mouse().button_pressed.len() >= 6,
            "index 4/5 = side buttons"
        );
    }

    #[test]
    #[cfg(windows)]
    fn common_ptt_keys_have_os_keycodes() {
        for k in [
            egui::Key::V,
            egui::Key::Space,
            egui::Key::F8,
            egui::Key::Backtick,
            egui::Key::Num3,
            egui::Key::PageUp,
        ] {
            assert!(keycode_for(k).is_some(), "{k:?}");
        }
        assert_eq!(keycode_for(egui::Key::Backtick), Some(Keycode::Grave));
        assert_eq!(keycode_for(egui::Key::Num3), Some(Keycode::Key3));
    }
}
