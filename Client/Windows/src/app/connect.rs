//! Connect screen (not connected): logo, saved servers, connect form.

use super::theme;
use super::*;

impl RustTalkApp {
    pub(super) fn ui_connect(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::DARK))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                let card_w = 760.0f32.min(avail.x - 32.0);
                ui.add_space(((avail.y - 470.0) / 2.0).max(16.0));
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
                                    egui::vec2(240.0, 300.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| self.saved_servers(ui),
                                );
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);
                                // --- Right: connect form ---
                                ui.allocate_ui_with_layout(
                                    egui::vec2(ui.available_width(), 300.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| self.connect_form(ui),
                                );
                            });
                        });
                });

                if !self.status.is_empty() {
                    ui.add_space(14.0);
                    ui.vertical_centered(|ui| {
                        let color = if self.status_is_error { theme::RED } else { theme::MUTED };
                        ui.label(egui::RichText::new(&self.status).color(color));
                    });
                }
            });
    }

    fn saved_servers(&mut self, ui: &mut egui::Ui) {
        theme::caption(ui, "Saved servers");
        ui.add_space(4.0);
        let bookmarks = self.config.bookmarks.clone();
        let mut remove_idx: Option<usize> = None;
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, false]).show(ui, |ui| {
            if bookmarks.is_empty() {
                ui.label(
                    egui::RichText::new("No saved servers yet.\nClick = fill in  -  double click = connect")
                        .size(12.5)
                        .color(theme::MUTED),
                );
            }
            for (i, bm) in bookmarks.iter().enumerate() {
                let selected = self.config.server_address == bm.address;
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
                let p = ui.painter();
                p.rect_filled(rect, 8.0, if selected { theme::SELECTED } else if resp.hovered() { theme::HOVER } else { egui::Color32::TRANSPARENT });
                let icon_r = egui::Rect::from_center_size(rect.left_center() + egui::vec2(22.0, 0.0), egui::vec2(30.0, 30.0));
                p.rect_filled(icon_r, 15.0, theme::avatar_color(&bm.name));
                p.text(icon_r.center(), egui::Align2::CENTER_CENTER, icons::regular::HARD_DRIVES, egui::FontId::proportional(15.0), egui::Color32::WHITE);
                p.text(rect.left_top() + egui::vec2(46.0, 7.0), egui::Align2::LEFT_TOP, &bm.name, egui::FontId::proportional(14.0), theme::HEADER);
                p.text(rect.left_top() + egui::vec2(46.0, 25.0), egui::Align2::LEFT_TOP, &bm.address, egui::FontId::proportional(11.5), theme::MUTED);
                if resp.clicked() {
                    self.config.server_address = bm.address.clone();
                    self.config.server_password = bm.password.clone();
                }
                if resp.double_clicked() {
                    self.config.server_address = bm.address.clone();
                    self.config.server_password = bm.password.clone();
                    self.start_connect();
                }
                resp.on_hover_text("Click = fill in  -  double click = connect  -  right click = remove")
                    .context_menu(|ui| {
                        if ui.button(egui::RichText::new(format!("{}  Remove", icons::regular::TRASH)).color(theme::RED)).clicked() {
                            remove_idx = Some(i);
                            ui.close_menu();
                        }
                    });
            }
        });
        if let Some(i) = remove_idx {
            self.config.bookmarks.remove(i);
            self.config.save();
        }
    }

    fn connect_form(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        theme::caption(ui, "Server address");
        ui.add_sized([w, 32.0], egui::TextEdit::singleline(&mut self.config.server_address).hint_text("host or host:port"));
        ui.add_space(4.0);
        theme::caption(ui, "Username");
        ui.add_sized([w, 32.0], egui::TextEdit::singleline(&mut self.config.username));
        ui.add_space(4.0);
        theme::caption(ui, "Password (optional)");
        let mut pw = self.config.server_password.clone().unwrap_or_default();
        if ui.add_sized([w, 32.0], egui::TextEdit::singleline(&mut pw).password(true)).changed() {
            self.config.server_password = if pw.is_empty() { None } else { Some(pw) };
        }
        ui.add_space(12.0);
        let connecting = self.cmd_tx.is_some();
        let label = match (&self.reconnect, connecting) {
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
            egui::Button::new(egui::RichText::new(format!("{}  {label}", icons::regular::SIGN_IN)).size(15.0).strong().color(egui::Color32::WHITE))
                .fill(theme::ACCENT)
                .min_size(egui::vec2(w, 38.0)),
        );
        if connect.clicked() || (!connecting && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            if let Some(rc) = self.reconnect.as_mut() {
                rc.next_at = None; // manual attempt replaces the scheduled one
            }
            self.start_connect();
        }
        if let Some(rc) = &self.reconnect {
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
            if ui.add(egui::Button::new(format!("{}  Save server", icons::regular::BOOKMARK_SIMPLE)).frame(false)).clicked() {
                let addr = self.config.server_address.trim().to_string();
                if !addr.is_empty() && !self.config.bookmarks.iter().any(|b| b.address == addr) {
                    self.config.bookmarks.push(ServerBookmark {
                        name: addr.clone(),
                        address: addr,
                        password: self.config.server_password.clone(),
                    });
                    self.config.save();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(format!("{}  Settings", icons::regular::GEAR_SIX)).frame(false)).clicked() {
                    self.open_settings(SettingsPage::Voice);
                }
            });
        });
    }
}
