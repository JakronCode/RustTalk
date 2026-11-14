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

/// Shown on a row that has unread messages.
const UNREAD_LABEL: &str = "New messages";

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
            self.remember_prompt = Some(RememberPrompt {
                key: key.to_string(),
                from_form: false,
            });
        } else {
            self.connect_server(key, false); // not saved, or remembered already
        }
    }

    /// Connect route of the connect screen (address form, saved list, Enter).
    pub(super) fn request_connect(&mut self) {
        let key = net::normalize_addr(self.config.server_address.trim());
        if self.srv.cmd_tx.is_none() && self.needs_remember_question(&key) {
            self.remember_prompt = Some(RememberPrompt {
                key,
                from_form: true,
            });
        } else {
            self.start_connect(false);
        }
    }

    /// The question: connect and remember, connect once, or cancel.
    pub(super) fn ui_remember_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.remember_prompt.clone() else {
            return;
        };
        if prompt.from_form && net::normalize_addr(self.config.server_address.trim()) != prompt.key
        {
            self.remember_prompt = None;
            return;
        }
        let Some(bm) = self.bookmark_for(&prompt.key).cloned() else {
            self.remember_prompt = None; // removed meanwhile
            return;
        };
        let name = bm.nickname().unwrap_or(&bm.address).to_string();
        let width = (ctx.screen_rect().width() - 48.0).clamp(160.0, 420.0);
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("remember_prompt"))
            .frame(egui::Frame::default().fill(theme::BG()).corner_radius(10.0).inner_margin(egui::Margin::same(20)))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(egui::RichText::new(format!("Connect to {name}?")).strong().size(16.0).color(theme::HEADER()));
                ui.add(egui::Label::new("Reconnect this server when RustTalk starts? Chat only, voice and camera stay off.").wrap());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    ui.spacing_mut().button_padding = egui::vec2(12.0, 10.0); // touch-sized
                    if ui.add(egui::Button::new(egui::RichText::new("Reconnect on start").strong().color(theme::on_accent())).fill(theme::accent()))
                        .on_hover_text("Connects this server every time RustTalk starts, until you disconnect it. Chat only, never voice or camera.")
                        .clicked()
                    {
                        answer = Some(true);
                    }
                    if ui.button("Just now").on_hover_text("Connects now only. The server stays in your saved list; it is not reconnected at start.").clicked() {
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
            if net::normalize_addr(self.config.server_address.trim()) != prompt.key {
                return;
            }
            self.start_connect(remember);
        } else {
            self.connect_server(&prompt.key, remember);
        }
    }

    /// Server rows and Add server, for the top of the dropdown.
    /// `close` is set when the dropdown should close (a row was selected).
    pub(super) fn ui_server_rows(
        &self,
        ui: &mut egui::Ui,
        close: &mut bool,
    ) -> Option<SwitchAction> {
        let rows = self.server_rows();
        let mut action = None;
        let max_h = (ui.ctx().screen_rect().height() * 0.45).max(ROW_H * 2.0);
        egui::ScrollArea::vertical()
            .id_salt("server_rows")
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(
                        egui::RichText::new("No servers yet.")
                            .size(12.5)
                            .color(theme::MUTED()),
                    );
                }
                for row in &rows {
                    if let Some(a) = server_row(ui, row) {
                        *close |= matches!(a, SwitchAction::Select(_));
                        action = Some(a);
                    }
                }
            });
        if !self.srv.is_placeholder()
            && theme::menu_item(ui, icons::regular::PLUS, "Add server", false).clicked()
        {
            *close = true;
            action = Some(SwitchAction::Add);
        }
        action
    }
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
        ServerStatus::Failed => (
            with("Needs attention, select to fix", &row.error),
            theme::RED,
        ),
        ServerStatus::Disconnected => ("Disconnected".into(), theme::MUTED()),
    }
}

/// Connect/disconnect label and tooltip for a row, from its status.
fn toggle_label(row: &ServerRow) -> (bool, String) {
    let live = matches!(
        row.status,
        ServerStatus::Connected
            | ServerStatus::Connecting
            | ServerStatus::Waiting
            | ServerStatus::Retrying
    );
    if live {
        (
            true,
            format!(
                "Disconnect from {}: no messages or notifications until you connect again",
                row.name
            ),
        )
    } else {
        (
            false,
            format!("Connect to {}: you appear online there", row.name),
        )
    }
}

fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
        .size()
        .x
}

fn server_row(ui: &mut egui::Ui, row: &ServerRow) -> Option<SwitchAction> {
    let (status, status_color) = status_text(row);
    let status_font = egui::FontId::proportional(11.5);
    let text_w = (ui.available_width() - ACTION_W - 36.0).max(24.0);
    let unread_w = if row.unread {
        text_width(ui, UNREAD_LABEL, &status_font) + 14.0
    } else {
        0.0
    };
    let wrap_unread = row.unread && text_width(ui, &status, &status_font) + unread_w > text_w;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            ROW_H + if wrap_unread { 16.0 } else { 0.0 },
        ),
        egui::Sense::hover(),
    );
    let (select_rect, action_rect) = rect.split_left_right_at_x(rect.right() - ACTION_W);
    let id = ui.id().with(("server_row", &row.key));
    let select = ui.interact(select_rect, id, egui::Sense::click());
    let toggle = ui.interact(action_rect, id.with("toggle"), egui::Sense::click());

    let (live, tip) = toggle_label(row);
    select.widget_info(|| {
        let mut label = format!("{}, {status}", row.name);
        if row.unread {
            label += ", new messages";
        }
        if row.voice {
            label += ", in voice";
        }
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, row.viewed, label)
    });
    toggle.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &tip));

    let p = ui.painter();
    if row.viewed {
        p.rect_filled(rect, 6.0, theme::SELECTED());
    } else if select.hovered() {
        p.rect_filled(rect, 6.0, theme::HOVER());
    }
    p.circle_filled(
        rect.left_center() + egui::vec2(16.0, 0.0),
        5.0,
        status_color,
    );

    // Voice icon on the name line; the status line keeps its full width for
    // the state and the plain "New messages" label.
    let mut name_w = (action_rect.left() - rect.left() - 36.0).max(24.0);
    if row.voice {
        p.text(
            egui::pos2(action_rect.left() - 12.0, rect.top() + 16.0),
            egui::Align2::CENTER_CENTER,
            icons::regular::SPEAKER_HIGH,
            egui::FontId::proportional(15.0),
            theme::GREEN,
        );
        name_w = (name_w - 22.0).max(24.0);
    }
    let name_font = egui::FontId::proportional(14.5);
    let name = elide(ui, &row.name, &name_font, name_w);
    let status_line = elide(
        ui,
        &status,
        &status_font,
        (text_w - if wrap_unread { 0.0 } else { unread_w }).max(24.0),
    );
    let status_w = text_width(ui, &status_line, &status_font);
    let p = ui.painter();
    p.text(
        rect.left_top() + egui::vec2(32.0, 8.0),
        egui::Align2::LEFT_TOP,
        name,
        name_font,
        if row.viewed {
            theme::HEADER()
        } else {
            theme::TEXT()
        },
    );
    p.text(
        rect.left_top() + egui::vec2(32.0, 27.0),
        egui::Align2::LEFT_TOP,
        status_line,
        status_font.clone(),
        status_color,
    );
    if row.unread {
        p.text(
            rect.left_top()
                + if wrap_unread {
                    egui::vec2(32.0, 43.0)
                } else {
                    egui::vec2(32.0 + status_w + 14.0, 27.0)
                },
            egui::Align2::LEFT_TOP,
            UNREAD_LABEL,
            status_font,
            theme::accent(),
        );
    }

    if toggle.hovered() {
        p.rect_filled(
            action_rect.shrink2(egui::vec2(4.0, 4.0)),
            6.0,
            theme::overlay(14),
        );
    }
    let (icon, color) = if live {
        (icons::regular::SIGN_OUT, theme::RED)
    } else {
        (icons::regular::SIGN_IN, theme::TEXT())
    };
    p.text(
        action_rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(18.0),
        color,
    );

    // Hover text names the whole row (long names are cut in the list).
    let select = select.on_hover_text(if row.voice {
        format!("{}\n{}\nYou are in voice here", row.name, row.address)
    } else {
        format!("{}\n{}", row.name, row.address)
    });
    let toggle = toggle
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(&tip);
    if toggle.clicked() {
        Some(if live {
            SwitchAction::Disconnect(row.key.clone())
        } else {
            SwitchAction::Connect(row.key.clone())
        })
    } else if select.clicked() {
        Some(SwitchAction::Select(row.key.clone()))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "server_switcher_tests.rs"]
mod tests;
