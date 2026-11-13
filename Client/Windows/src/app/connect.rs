//! Connect screen (not connected): logo, saved servers, connect form.

use super::theme;
use super::*;

impl RustTalkApp {
    pub(super) fn ui_connect(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::DARK))
            .show(ctx, |ui| {
                if self.srv.join_error.is_some() {
                    ui.disable();
                }
                let avail = ui.available_size();
                let expanded = self.show_connect_form;
                let card_w = (if expanded { 760.0_f32 } else { 340.0_f32 }).min(avail.x - 32.0);
                ui.add_space(((avail.y - 530.0) / 2.0).max(16.0));
                ui.vertical_centered(|ui| {
                    logo::paint_logo(ui, 64.0);
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("RustTalk").size(30.0).strong().color(theme::HEADER));
                    ui.label(egui::RichText::new("Lightweight voice chat. Local first. No accounts.").color(theme::MUTED));
                });
                ui.add_space(20.0);

                ui.horizontal(|ui| {
                    ui.add_space(((avail.x - card_w) / 2.0).max(0.0));
                    egui::Frame::default()
                        .fill(theme::BG)
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::same(20))
                        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(110) })
                        .show(ui, |ui| {
                            ui.set_width(card_w - 40.0);
                            ui.horizontal_top(|ui| {
                                // --- Left: saved servers ---
                                ui.allocate_ui_with_layout(
                                    egui::vec2(if expanded { 240.0 } else { card_w - 40.0 }, 360.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        if self.saved_servers(ui) {
                                            self.start_connect();
                                        }
                                    },
                                );
                                if expanded {
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(8.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(ui.available_width(), 360.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| self.connect_form(ui),
                                    );
                                }
                            });
                        });
                });

                if self.srv.cert_check.as_ref().is_some_and(|check| !check.matches_address(&self.config.server_address)) {
                    self.srv.cert_check = None;
                    self.srv.join_error = None;
                    self.set_status("Server address changed. Connect again to verify its identity.");
                }
                if !self.status.is_empty() && self.srv.join_error.is_none() {
                    ui.add_space(14.0);
                    ui.vertical_centered(|ui| {
                        let color = if self.status_is_error { theme::RED } else { theme::MUTED };
                        ui.label(egui::RichText::new(&self.status).color(color));
                    });
                }
            });
        self.update_banner_overlay(ctx);
        if self.srv.join_error.is_some() {
            let size = ctx.screen_rect().size();
            let width = (size.x - 96.0).clamp(160.0, 640.0);
            let response = egui::Modal::new(egui::Id::new("join_error_dialog"))
                .frame(egui::Frame::default().fill(theme::BG).corner_radius(10.0))
                .backdrop_color(egui::Color32::from_black_alpha(160))
                .show(ctx, |ui| {
                    // A modal Area otherwise retains its small default height and clips the scroll viewport.
                    ui.set_width(width + 48.0);
                    ui.set_max_height((size.y - 64.0).max(80.0));
                    egui::ScrollArea::vertical()
                        .id_salt("join_error_dialog_scroll")
                        .max_height((size.y - 64.0).max(80.0))
                        .show(ui, |ui| self.join_error_panel(ui, width));
                });
            if response.should_close() {
                self.srv.join_error = None;
                self.srv.cert_check = None;
                self.set_status("");
            }
        }
    }

    fn saved_servers(&mut self, ui: &mut egui::Ui) -> bool {
        let mut connect = false;
        theme::caption(ui, "Saved servers");
        ui.add_space(4.0);
        let bookmarks = self.config.bookmarks.clone();
        let mut remove_idx: Option<usize> = None;
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, false]).show(ui, |ui| {
            if bookmarks.is_empty() {
                ui.label(
                    egui::RichText::new("No saved servers yet.")
                        .size(12.5)
                        .color(theme::MUTED),
                );
            }
            for (i, bm) in bookmarks.iter().enumerate() {
                let selected = self.config.server_address == bm.address;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
                let resp = ui.interact(rect, ui.id().with(("saved_server", &bm.address)), egui::Sense::click());
                let p = ui.painter();
                p.rect_filled(rect, 8.0, if selected { theme::SELECTED } else if resp.hovered() { theme::HOVER } else { egui::Color32::TRANSPARENT });
                let icon_r = egui::Rect::from_center_size(rect.left_center() + egui::vec2(22.0, 0.0), egui::vec2(30.0, 30.0));
                p.rect_filled(icon_r, 15.0, theme::avatar_color(&bm.address));
                p.text(icon_r.center(), egui::Align2::CENTER_CENTER, icons::regular::HARD_DRIVES, egui::FontId::proportional(15.0), egui::Color32::WHITE);
                let text_w = rect.width() - 52.0;
                match bm.nickname() {
                    Some(nick) => {
                        paint_clipped(ui, rect.left_top() + egui::vec2(46.0, 7.0), nick, 14.0, theme::HEADER, text_w);
                        paint_clipped(ui, rect.left_top() + egui::vec2(46.0, 25.0), &bm.address, 11.5, theme::MUTED, text_w);
                    }
                    None => paint_clipped(ui, rect.left_top() + egui::vec2(46.0, 13.0), &bm.address, 14.0, theme::HEADER, text_w),
                }
                if resp.clicked() || resp.double_clicked() {
                    self.select_saved_server(bm);
                    self.show_connect_form = false;
                }
                if resp.double_clicked() && self.srv.cmd_tx.is_none() {
                    connect = true;
                }
                resp.on_hover_text(format!("{}\nClick = select  -  double click = connect  -  right click = edit or remove", bm.address))
                    .context_menu(|ui| {
                        ui.set_width(160.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if theme::menu_item(ui, icons::regular::PENCIL_SIMPLE, "Edit", false).clicked() {
                            self.select_saved_server(bm);
                            self.editing_bookmark = Some(bm.address.clone());
                            self.show_connect_form = true;
                            ui.close_menu();
                        }
                        theme::menu_separator(ui);
                        if theme::menu_item(ui, icons::regular::TRASH, "Remove", true).clicked() {
                            remove_idx = Some(i);
                            ui.close_menu();
                        }
                    });
            }
            ui.add_space(6.0);
            if theme::ghost_button(
                ui,
                egui::RichText::new(format!("{}  Add / connect custom server", icons::regular::PLUS))
                    .color(theme::MUTED),
            ).clicked() {
                self.show_connect_form = true;
                self.server_nickname.clear();
                self.config.server_address.clear();
                self.config.server_password = None;
                self.config.invite = None;
                self.editing_bookmark = None;
            }
        });
        if let Some(i) = remove_idx {
            // A removed server is no longer reconnected at startup.
            let removed = self.config.bookmarks.remove(i);
            self.config.auto_connect.remove(&crate::net::normalize_addr(removed.address.trim()));
            self.config.save();
        }
        if !self.show_connect_form {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::ghost_button_sized(ui, format!("{}  Settings", icons::regular::GEAR_SIX), egui::vec2(0.0, 32.0)).clicked() {
                    self.open_settings(SettingsPage::Voice);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let selected = self.config.bookmarks.iter()
                        .find(|bm| bm.address == self.config.server_address).cloned();
                    let enabled = selected.is_some() && self.srv.cmd_tx.is_none();
                    if ui.add_enabled_ui(enabled, |ui| theme::primary_button(ui, "Connect")).inner.clicked() {
                        if let Some(bm) = selected {
                            self.select_saved_server(&bm);
                            connect = true;
                        }
                    }
                });
            });
        }
        connect
    }

    fn update_saved_server(&mut self) -> Result<(), &'static str> {
        let addr = self.config.server_address.trim().to_string();
        if addr.is_empty() {
            return Err("Enter a server address before saving.");
        }
        let editing = self.editing_bookmark.as_ref().and_then(|address|
            self.config.bookmarks.iter().position(|b| &b.address == address));
        if self.editing_bookmark.is_some() && editing.is_none() {
            return Err("This saved server was removed. Select a server or add a new one.");
        }
        let existing = self.config.bookmarks.iter().position(|b| b.address == addr);
        if editing.is_some() && existing.is_some() && editing != existing {
            return Err("Another saved server already uses this address.");
        }
        let idx = match editing.or(existing) {
            Some(i) => i,
            None => {
                self.config.bookmarks.push(ServerBookmark {
                    name: String::new(),
                    address: addr.clone(),
                    password: None,
                });
                self.config.bookmarks.len() - 1
            }
        };
        let bm = &mut self.config.bookmarks[idx];
        bm.address = addr.clone();
        bm.set_nickname(&self.server_nickname);
        bm.password = self.config.server_password.clone();
        self.server_nickname = bm.nickname().unwrap_or_default().to_string();
        self.config.server_address = addr.clone();
        self.editing_bookmark = Some(addr);
        Ok(())
    }

    /// Selecting a saved server views it (the connect screen shows the viewed server).
    fn select_saved_server(&mut self, bm: &ServerBookmark) {
        self.select_server(&crate::net::normalize_addr(bm.address.trim()));
        self.config.invite = None;
        self.config.server_address = bm.address.clone();
        self.config.server_password = bm.password.clone();
        self.server_nickname = bm.nickname().unwrap_or_default().to_string();
        self.editing_bookmark = None;
    }

    /// Error panel for a failed join: icon, title, explanation, actions, raw details.
    fn join_error_panel(&mut self, ui: &mut egui::Ui, width: f32) {
        let Some(err) = self.srv.join_error.clone() else { return };
        let color = match err.kind {
            super::join_error::Kind::UnknownServer => theme::accent(),
            _ if err.is_fatal() => theme::RED,
            _ => theme::YELLOW,
        };
        egui::Frame::default()
            .fill(color.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.7)))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(24))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.horizontal_top(|ui| {
                    ui.label(egui::RichText::new(err.icon()).size(26.0).color(color));
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(err.title()).strong().size(15.0).color(theme::HEADER));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if theme::ghost_button(ui, egui::RichText::new(icons::regular::X).color(theme::MUTED)).on_hover_text("Dismiss").clicked() {
                                    self.srv.join_error = None;
                                    self.srv.cert_check = None;
                                    self.set_status("");
                                }
                            });
                        });
                        ui.label(egui::RichText::new(err.explanation()).color(theme::TEXT));
                        ui.label(egui::RichText::new(err.suggestion()).size(12.5).color(theme::MUTED));
                        if matches!(err.kind, super::join_error::Kind::CertChanged | super::join_error::Kind::UnknownServer) {
                            self.cert_actions(ui);
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for &action in err.actions() {
                                if ui.button(action.label()).clicked() {
                                    self.run_join_action(ui.ctx(), action);
                                }
                            }
                            let caret = if err.details_open { icons::regular::CARET_DOWN } else { icons::regular::CARET_RIGHT };
                            if theme::ghost_button(ui, egui::RichText::new(format!("{caret}  Details")).color(theme::MUTED)).clicked() {
                                if let Some(e) = self.srv.join_error.as_mut() {
                                    e.details_open = !e.details_open;
                                }
                            }
                        });
                        if err.details_open {
                            ui.label(egui::RichText::new(&err.raw).monospace().size(11.5).color(theme::MUTED));
                        }
                    });
                });
            });
    }

    /// New or changed certificate: the only way on is an explicit trust decision.
    fn cert_actions(&mut self, ui: &mut egui::Ui) {
        let Some(check) = self.srv.cert_check.clone() else { return };
        ui.add_space(6.0);
        egui::Frame::default()
            .fill(theme::DARK)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(&check.addr).strong().color(theme::HEADER));
                theme::caption(ui, "SHA-256 fingerprint");
                ui.label(egui::RichText::new(super::join_error::group_fingerprint(&check.fingerprint)).monospace().size(12.0).color(theme::TEXT));
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let trust = if check.changed {
                theme::danger_button(ui, "Trust the new identity and connect")
            } else {
                theme::primary_button(ui, "Trust and connect")
            };
            if trust.clicked() && check.trust(&mut self.config) {
                self.config.save();
                self.srv.cert_check = None;
                self.start_connect();
            }
            if theme::ghost_button(ui, "Cancel").clicked() {
                self.srv.cert_check = None;
                self.srv.join_error = None;
                self.set_status("");
            }
        });
    }

    fn run_join_action(&mut self, ctx: &egui::Context, action: super::join_error::Action) {
        use super::join_error::Action;
        match action {
            Action::Retry => self.start_connect(),
            Action::EditPassword => {
                self.show_connect_form = true;
                self.srv.join_error = None;
                // A stale invite would be refused before the password is checked.
                self.config.invite = None;
                self.config.server_password = None;
                self.focus_password = true;
            }
            Action::ShowUpdate => {
                self.srv.join_error = None;
                self.check_for_updates();
                self.open_settings(SettingsPage::About);
            }
            Action::OpenReleases => {
                ctx.open_url(egui::OpenUrl::new_tab(format!("https://github.com/{}/releases/latest", crate::update::REPO)));
            }
        }
    }

    fn connect_form(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        if theme::ghost_button(ui, format!("{}  Hide details", icons::regular::CARET_LEFT)).clicked() {
            self.show_connect_form = false;
        }
        ui.add_space(4.0);
        theme::caption(ui, "Server nickname (optional)");
        ui.add_sized(
            [w, 32.0],
            egui::TextEdit::singleline(&mut self.server_nickname)
                .hint_text("e.g. Local Dev Server")
                .char_limit(crate::config::MAX_NICKNAME_CHARS),
        );
        ui.add_space(4.0);
        theme::caption(ui, "Server address");
        ui.add_sized([w, 32.0], egui::TextEdit::singleline(&mut self.config.server_address).hint_text("host, host:port or invite link"));
        if let Some((_, code)) = super::moderation_ui::parse_invite_link(&self.config.server_address) {
            let text = if code.is_some() { "Invite link - no password needed" } else { "RustTalk link" };
            ui.label(egui::RichText::new(format!("{}  {text}", icons::regular::LINK)).size(12.0).color(theme::GREEN));
        }
        ui.add_space(4.0);
        theme::caption(ui, "Username");
        ui.add_sized([w, 32.0], egui::TextEdit::singleline(&mut self.config.username));
        ui.add_space(4.0);
        theme::caption(ui, "Password (optional)");
        let mut pw = self.config.server_password.clone().unwrap_or_default();
        let pw_resp = ui.add_sized([w, 32.0], egui::TextEdit::singleline(&mut pw).password(true));
        if std::mem::take(&mut self.focus_password) {
            pw_resp.request_focus();
        }
        if pw_resp.changed() {
            self.config.server_password = if pw.is_empty() { None } else { Some(pw) };
        }
        ui.add_space(12.0);
        let connecting = self.srv.cmd_tx.is_some();
        let label = match (&self.srv.reconnect, connecting) {
            (Some(_), true) => "Reconnecting...",
            (Some(rc), false) => {
                let secs = rc.next_at.map(|t| t.saturating_duration_since(Instant::now()).as_secs() + 1).unwrap_or(0);
                ui.ctx().request_repaint_after(Duration::from_millis(250));
                if secs > 0 { "Reconnect now" } else { "Reconnecting..." }
            }
            (None, true) => "Connecting...",
            (None, false) => "Connect",
        };
        let connect = ui.add_enabled(
            !connecting,
            egui::Button::new(egui::RichText::new(format!("{}  {label}", icons::regular::SIGN_IN)).size(15.0).strong().color(theme::on_accent()))
                .fill(theme::accent())
                .min_size(egui::vec2(w, 38.0)),
        );
        if connect.clicked() || (ui.is_enabled() && !connecting && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            if let Some(rc) = self.srv.reconnect.as_mut() {
                rc.next_at = None; // manual attempt replaces the scheduled one
            }
            self.start_connect();
        }
        if let Some(rc) = &self.srv.reconnect {
            ui.add_space(4.0);
            let attempt = rc.attempt;
            let wait = rc.next_at.map(|t| t.saturating_duration_since(Instant::now()).as_secs() + 1);
            ui.horizontal(|ui| {
                let text = match wait {
                    Some(s) => format!("Connection lost - next attempt in {s} s ({attempt}/{MAX_RECONNECT_ATTEMPTS})"),
                    None => format!("Reconnecting (attempt {attempt}/{MAX_RECONNECT_ATTEMPTS})..."),
                };
                ui.label(egui::RichText::new(text).size(12.0).color(theme::YELLOW));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Cancel").clicked() {
                        self.cancel_reconnect();
                    }
                });
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::ghost_button(ui, format!("{}  Save server", icons::regular::BOOKMARK_SIMPLE)).clicked() {
                match self.update_saved_server() {
                    Ok(()) => {
                        self.config.save();
                        self.set_status("Server saved");
                        // Saved while connected: the user joined it, so it reconnects on start too.
                        let key = crate::net::normalize_addr(self.config.server_address.trim());
                        if self.srv.connected && self.srv.key == key {
                            let name = self.srv.server_settings.name.clone();
                            self.enroll(&key, &name);
                        }
                    }
                    Err(message) => self.set_error(message),
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost_button(ui, format!("{}  Settings", icons::regular::GEAR_SIX)).clicked() {
                    self.open_settings(SettingsPage::Voice);
                }
            });
        });
    }
}

/// One line of text, cut off at `max_w` so long names don't run over the row.
fn paint_clipped(ui: &egui::Ui, pos: egui::Pos2, text: &str, size: f32, color: egui::Color32, max_w: f32) {
    let clip = egui::Rect::from_min_size(pos, egui::vec2(max_w.max(0.0), size * 1.6));
    ui.painter().with_clip_rect(clip.intersect(ui.clip_rect())).text(pos, egui::Align2::LEFT_TOP, text, egui::FontId::proportional(size), color);
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
