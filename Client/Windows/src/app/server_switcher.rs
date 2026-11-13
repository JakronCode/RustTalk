//! Server switcher: saved and live servers as rows at the top of the
//! server-name dropdown. A row selects (shows) a server; its icon connects or
//! disconnects it. Nothing here touches voice.

use super::session::{ServerRow, ServerStatus};
use super::sidebar::elide;
use super::theme;
use super::*;

/// Touch-sized row and icon hit area.
const ROW_H: f32 = 48.0;
const ACTION_W: f32 = 44.0;

/// Shown beside every connect action, before the user enrolls a server.
pub(super) const RECONNECT_NOTE: &str = "\"Connect and remember\" reconnects a saved server whenever RustTalk starts (chat only, never voice or camera) until you disconnect it. \"Connect once\" does not. A disconnected server sends no messages or notifications.";

/// Short help in the dropdown; the full consent text is in the remember modal.
const DROPDOWN_NOTE: &str = "Disconnect stops live notifications.";

/// Which connect route asked, so the answer repeats it exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RememberPrompt {
    pub key: String,
    /// The connect screen (address form) asked, else a switcher row.
    pub from_form: bool,
}

/// What the user did in the switcher; applied after the frame's drawing so the
/// sidebar never draws the new view with the old connection.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SwitchAction {
    Select(String),
    Connect(String),
    Disconnect(String),
    /// Open the connect form on a blank placeholder; nothing disconnects.
    Add,
}

impl RustTalkApp {
    pub(super) fn apply_switch(&mut self, action: SwitchAction) {
        match action {
            SwitchAction::Select(key) => self.select_server(&key),
            SwitchAction::Connect(key) => self.request_connect_row(&key),
            SwitchAction::Disconnect(key) => self.disconnect_server(&key),
            SwitchAction::Add => self.add_server(),
        }
    }

    /// Views a blank session with the connect form. The server shown so far
    /// stays connected (voice included); selecting any row leaves the form.
    pub(super) fn add_server(&mut self) {
        if self.srv.is_placeholder() {
            self.show_connect_form = true;
            return;
        }
        let id = self.next_session_id;
        self.next_session_id += 1;
        self.others.push(ServerSession::new(id, ""));
        self.view_session(id);
        self.show_connect_form = true;
    }

    /// Saved server that RustTalk would not reconnect at start yet: connecting
    /// asks first. Unsaved addresses are never remembered, so they connect once.
    fn needs_remember_question(&self, key: &str) -> bool {
        self.bookmark_for(key).is_some() && !self.config.auto_connect.contains(key)
    }

    /// Connect route of a switcher row.
    pub(super) fn request_connect_row(&mut self, key: &str) {
        if self.needs_remember_question(key) {
            self.remember_prompt = Some(RememberPrompt { key: key.to_string(), from_form: false });
        } else {
            self.connect_server(key, false); // not saved, or remembered already
        }
    }

    /// Connect route of the connect screen (address form, saved list, Enter).
    pub(super) fn request_connect(&mut self) {
        let key = net::normalize_addr(self.config.server_address.trim());
        if self.srv.cmd_tx.is_none() && self.needs_remember_question(&key) {
            self.remember_prompt = Some(RememberPrompt { key, from_form: true });
        } else {
            self.start_connect(false);
        }
    }

    /// The question: connect and remember, connect once, or cancel.
    pub(super) fn ui_remember_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.remember_prompt.clone() else { return };
        let Some(bm) = self.bookmark_for(&prompt.key).cloned() else {
            self.remember_prompt = None; // removed meanwhile
            return;
        };
        let name = bm.nickname().unwrap_or(&bm.address).to_string();
        let width = (ctx.screen_rect().width() - 48.0).clamp(160.0, 420.0);
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("remember_prompt"))
            .frame(egui::Frame::default().fill(theme::BG).corner_radius(10.0).inner_margin(egui::Margin::same(20)))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(egui::RichText::new(format!("Connect to {name}?")).strong().size(16.0).color(theme::HEADER));
                ui.add(egui::Label::new("Connect and remember: RustTalk connects to this server again every time it starts (chat only, never voice or camera), until you disconnect it.").wrap());
                ui.add(egui::Label::new("Connect once: connects now and nothing is remembered.").wrap());
                ui.add(egui::Label::new(egui::RichText::new("A certificate RustTalk does not know yet still asks for your approval first.").size(12.0).color(theme::MUTED)).wrap());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    ui.spacing_mut().button_padding = egui::vec2(12.0, 10.0); // touch-sized
                    if ui.add(egui::Button::new(egui::RichText::new("Connect and remember").strong().color(theme::on_accent())).fill(theme::accent())).clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Connect once").clicked() {
                        answer = Some(false);
                    }
                    if ui.button("Cancel").clicked() {
                        self.remember_prompt = None;
                    }
                });
            });
        if modal.should_close() {
            self.remember_prompt = None;
        }
        let Some(remember) = answer else { return };
        self.remember_prompt = None;
        if prompt.from_form {
            self.start_connect(remember);
        } else {
            self.connect_server(&prompt.key, remember);
        }
    }

    /// Server rows plus the reconnect note, for the top of the dropdown.
    /// `close` is set when the dropdown should close (a row was selected).
    pub(super) fn ui_server_rows(&self, ui: &mut egui::Ui, close: &mut bool) -> Option<SwitchAction> {
        let rows = self.server_rows();
        let mut action = None;
        let max_h = (ui.ctx().screen_rect().height() * 0.45).max(ROW_H * 2.0);
        egui::ScrollArea::vertical()
            .id_salt("server_rows")
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(egui::RichText::new("No servers yet.").size(12.5).color(theme::MUTED));
                }
                for row in &rows {
                    if let Some(a) = server_row(ui, row) {
                        *close |= matches!(a, SwitchAction::Select(_));
                        action = Some(a);
                    }
                }
            });
        if !self.srv.is_placeholder() && theme::menu_item(ui, icons::regular::PLUS, "Add server", false).clicked() {
            *close = true;
            action = Some(SwitchAction::Add);
        }
        ui.add(egui::Label::new(egui::RichText::new(DROPDOWN_NOTE).size(11.5).color(theme::MUTED)).wrap());
        action
    }
}

/// The consent text, wrapped to the current width.
pub(super) fn reconnect_note(ui: &mut egui::Ui) {
    ui.add(egui::Label::new(egui::RichText::new(RECONNECT_NOTE).size(11.5).color(theme::MUTED)).wrap());
}

fn status_text(row: &ServerRow) -> (String, egui::Color32) {
    let with = |label: &str, error: &Option<String>| match error {
        Some(e) => format!("{label}: {e}"),
        None => label.to_string(),
    };
    match row.status {
        ServerStatus::Connected => ("Connected".into(), theme::GREEN),
        ServerStatus::Connecting => ("Connecting...".into(), theme::YELLOW),
        ServerStatus::Waiting => ("Waiting to connect".into(), theme::YELLOW),
        ServerStatus::Retrying => (with("Retrying", &row.error), theme::YELLOW),
        ServerStatus::LimitReached => (with("Not connected", &row.error), theme::RED),
        ServerStatus::Failed => (with("Needs attention, select to fix", &row.error), theme::RED),
        ServerStatus::Disconnected => ("Disconnected".into(), theme::MUTED),
    }
}

/// Connect/disconnect label and tooltip for a row, from its status.
fn toggle_label(row: &ServerRow) -> (bool, String) {
    let live = matches!(
        row.status,
        ServerStatus::Connected | ServerStatus::Connecting | ServerStatus::Waiting | ServerStatus::Retrying
    );
    if live {
        (true, format!("Disconnect from {}: no messages or notifications until you connect again", row.name))
    } else {
        (false, format!("Connect to {}: you appear online there", row.name))
    }
}

fn server_row(ui: &mut egui::Ui, row: &ServerRow) -> Option<SwitchAction> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::hover());
    let (select_rect, action_rect) = rect.split_left_right_at_x(rect.right() - ACTION_W);
    let id = ui.id().with(("server_row", &row.key));
    let select = ui.interact(select_rect, id, egui::Sense::click());
    let toggle = ui.interact(action_rect, id.with("toggle"), egui::Sense::click());

    let (status, status_color) = status_text(row);
    let (live, tip) = toggle_label(row);
    select.widget_info(|| {
        let mut label = format!("{}, {status}", row.name);
        if row.unread {
            label += ", unread messages";
        }
        if row.voice {
            label += ", in voice";
        }
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, row.viewed, label)
    });
    toggle.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &tip));

    let p = ui.painter();
    if row.viewed {
        p.rect_filled(rect, 6.0, theme::SELECTED);
    } else if select.hovered() {
        p.rect_filled(rect, 6.0, theme::HOVER);
    }
    p.circle_filled(rect.left_center() + egui::vec2(16.0, 0.0), 5.0, status_color);

    // Markers sit left of the icon; names and status lines stop before them.
    let mut right = action_rect.left() - 4.0;
    if row.voice {
        p.text(egui::pos2(right - 8.0, rect.center().y), egui::Align2::CENTER_CENTER, icons::regular::SPEAKER_HIGH, egui::FontId::proportional(15.0), theme::GREEN);
        right -= 22.0;
    }
    if row.unread {
        p.circle_filled(egui::pos2(right - 6.0, rect.center().y), 5.0, theme::accent());
        right -= 18.0;
    }
    let text_w = (right - rect.left() - 36.0).max(24.0);
    let name_font = egui::FontId::proportional(14.5);
    let status_font = egui::FontId::proportional(11.5);
    let name = elide(ui, &row.name, &name_font, text_w);
    let status_line = elide(ui, &status, &status_font, text_w);
    let p = ui.painter();
    p.text(rect.left_top() + egui::vec2(32.0, 8.0), egui::Align2::LEFT_TOP, name, name_font, if row.viewed { theme::HEADER } else { theme::TEXT });
    p.text(rect.left_top() + egui::vec2(32.0, 27.0), egui::Align2::LEFT_TOP, status_line, status_font, status_color);

    if toggle.hovered() {
        p.rect_filled(action_rect.shrink2(egui::vec2(4.0, 4.0)), 6.0, egui::Color32::from_white_alpha(14));
    }
    let (icon, color) = if live { (icons::regular::SIGN_OUT, theme::RED) } else { (icons::regular::SIGN_IN, theme::TEXT) };
    p.text(action_rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(18.0), color);

    // Hover text names the whole row (long names are cut in the list).
    let select = select.on_hover_text(format!("{}\n{}", row.name, row.address));
    let toggle = toggle.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(&tip);
    if toggle.clicked() {
        Some(if live { SwitchAction::Disconnect(row.key.clone()) } else { SwitchAction::Connect(row.key.clone()) })
    } else if select.clicked() {
        Some(SwitchAction::Select(row.key.clone()))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "server_switcher_tests.rs"]
mod tests;
