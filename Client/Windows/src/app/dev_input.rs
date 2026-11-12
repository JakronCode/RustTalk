//! Debug builds only: scripted UI input for tests and screenshots.
//!
//! `RUSTTALK_DEV_INPUT=<file>`: every frame the app looks at the file; lines
//! written into it are taken (the file is emptied) and played back as egui
//! input, one step per frame:
//!
//! ```text
//! click 550 402        # left click at window pixels (as in a screenshot of the client area)
//! dblclick 71 207
//! rclick 71 207
//! move 100 200
//! type Neuer Voice
//! key Enter            # Enter, Escape, Tab, Backspace, ArrowDown, ...
//! ```
//!
//! Unlike posted window messages this does not depend on where the real
//! mouse is, and nothing reaches other windows.

use std::collections::VecDeque;

use eframe::egui;

#[derive(Default)]
pub(super) struct DevInput {
    steps: VecDeque<Vec<egui::Event>>,
}

fn pos(ctx: &egui::Context, x: f32, y: f32) -> egui::Pos2 {
    egui::pos2(x, y) / ctx.pixels_per_point()
}

fn button(p: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos: p, button, pressed, modifiers: egui::Modifiers::NONE }
}

fn key(name: &str) -> Option<egui::Key> {
    egui::Key::from_name(name)
}

impl DevInput {
    /// Turns one script line into frames of events.
    fn parse(&mut self, ctx: &egui::Context, line: &str) {
        let line = line.trim();
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let nums: Vec<f32> = rest.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        let at = || nums.get(0..2).map(|n| pos(ctx, n[0], n[1]));
        let click = |p: egui::Pos2, b: egui::PointerButton| vec![vec![egui::Event::PointerMoved(p)], vec![button(p, b, true)], vec![button(p, b, false)]];
        let frames = match cmd {
            "move" => at().map(|p| vec![vec![egui::Event::PointerMoved(p)]]),
            "click" => at().map(|p| click(p, egui::PointerButton::Primary)),
            "rclick" => at().map(|p| click(p, egui::PointerButton::Secondary)),
            "dblclick" => at().map(|p| {
                let mut f = click(p, egui::PointerButton::Primary);
                f.push(vec![button(p, egui::PointerButton::Primary, true)]);
                f.push(vec![button(p, egui::PointerButton::Primary, false)]);
                f
            }),
            "type" => Some(vec![vec![egui::Event::Text(rest.to_string())]]),
            "key" => key(rest.trim()).map(|k| {
                let ev = |pressed| egui::Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
                vec![vec![ev(true)], vec![ev(false)]]
            }),
            "" => None,
            _ => {
                eprintln!("dev input: unknown line '{line}'");
                None
            }
        };
        self.steps.extend(frames.unwrap_or_default());
    }

    /// Called from `raw_input_hook`: takes new script lines, plays the next step.
    pub(super) fn feed(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(path) = std::env::var_os("RUSTTALK_DEV_INPUT") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.trim().is_empty() {
                    let _ = std::fs::write(&path, "");
                    for line in text.lines() {
                        self.parse(ctx, line);
                    }
                }
            }
        }
        if let Some(events) = self.steps.pop_front() {
            raw.events.extend(events);
            ctx.request_repaint();
        }
    }
}
