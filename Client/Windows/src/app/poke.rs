//! Poke (like TeamSpeak): right-click a user -> "Poke..." -> optional short
//! message. The target hears a wake-up ring and gets a popup with the
//! message (plus a Windows notification while RustTalk is in the background).

use super::theme;
use super::*;
use rusttalk_shared::protocol::{ClientMessage, MAX_POKE_LEN};

/// "Poke ..." dialog being written.
pub(super) struct PokeDialog {
    target: u64,
    name: String,
    message: String,
    focus: bool,
}

/// A received poke shown as a popup until dismissed.
pub(super) struct PokeNote {
    from_id: u64,
    from: String,
    message: String,
    /// Local time (for the popup).
    at: u64,
}

/// Max. popups kept (older ones are dropped).
const MAX_INBOX: usize = 5;

impl RustTalkApp {
    /// Opens the dialog to poke `user`.
    pub(super) fn open_poke(&mut self, user: &UserInfo) {
        if user.id != self.my_id {
            self.poke_dialog = Some(PokeDialog { target: user.id, name: user.name.clone(), message: String::new(), focus: true });
        }
    }

    fn send_poke(&mut self, target: u64, name: &str, message: &str) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Raw(ClientMessage::Poke { target, message: message.trim().to_string() }));
            self.set_status(format!("You poked {name}"));
        }
    }

    /// Incoming poke: wake-up ring, popup, toast + taskbar flash if in the background.
    pub(super) fn receive_poke(&mut self, from_id: u64, from: String, message: String) {
        self.notify(Sound::Poke);
        if !self.window_focused() {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
            if self.config.desktop_notifications {
                let body = if message.is_empty() { "wants your attention".to_string() } else { message.clone() };
                let ctx = self.ctx.clone();
                crate::notify::show(format!("{from} poked you"), body, move || {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                });
            }
        }
        self.poke_inbox.push(PokeNote { from_id, from, message, at: Self::now() });
        if self.poke_inbox.len() > MAX_INBOX {
            self.poke_inbox.remove(0);
        }
    }

    /// The "Poke ..." dialog.
    pub(super) fn ui_poke_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.poke_dialog.take() else { return };
        let mut open = true;
        let mut done = false;
        egui::Window::new(format!("{}  Poke {}", icons::regular::HAND_WAVING, dlg.name))
            .id(egui::Id::new("poke_dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([380.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("{} hears a wake-up sound and gets a popup with your message.", dlg.name))
                        .color(theme::MUTED),
                );
                ui.add_space(4.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut dlg.message)
                        .hint_text("Message (optional), e.g. \"Come to Gaming!\"")
                        .char_limit(MAX_POKE_LEN)
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut dlg.focus) {
                    resp.request_focus();
                }
                ui.label(
                    egui::RichText::new(format!("{} / {MAX_POKE_LEN}", dlg.message.chars().count()))
                        .size(11.0)
                        .color(theme::MUTED),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button(ui, &format!("{}  Poke", icons::regular::HAND_WAVING)).clicked() || enter {
                        self.send_poke(dlg.target, &dlg.name, &dlg.message);
                        done = true;
                    }
                    if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                        done = true;
                    }
                });
            });
        if open && !done {
            self.poke_dialog = Some(dlg);
        }
    }

    /// Popup for the newest received poke (others wait behind it).
    pub(super) fn ui_poke_popup(&mut self, ctx: &egui::Context) {
        let Some(note) = self.poke_inbox.last() else { return };
        let (from_id, from, message, at) = (note.from_id, note.from.clone(), note.message.clone(), note.at);
        let waiting = self.poke_inbox.len() - 1;
        let mut close = false;
        let mut reply = false;
        egui::Window::new(format!("{}  {from} poked you", icons::regular::HAND_WAVING))
            .id(egui::Id::new("poke_popup"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
            .fixed_size([380.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    theme::draw_avatar(ui, &from, 40.0, false);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&from).strong().color(theme::HEADER));
                        ui.label(egui::RichText::new(format!("at {}", format_time(at))).size(11.5).color(theme::MUTED));
                    });
                });
                ui.add_space(4.0);
                let text = if message.is_empty() { "wants your attention!".to_string() } else { message.clone() };
                egui::Frame::default()
                    .fill(theme::INPUT)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(egui::RichText::new(text).size(15.0).color(theme::TEXT)).wrap());
                    });
                if waiting > 0 {
                    ui.label(egui::RichText::new(format!("+{waiting} more")).size(11.5).color(theme::MUTED));
                }
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button(ui, "OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                    let online = self.users.contains_key(&from_id);
                    if ui
                        .add_enabled(online, egui::Button::new(format!("{}  Poke Back", icons::regular::HAND_WAVING)).frame(false))
                        .clicked()
                    {
                        reply = true;
                        close = true;
                    }
                });
            });
        if close {
            self.poke_inbox.pop();
        }
        if reply {
            if let Some(user) = self.users.get(&from_id).cloned() {
                self.open_poke(&user);
            }
        }
    }
}
