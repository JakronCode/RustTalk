//! Connect screen (not connected): logo, saved servers, connect form.

use super::theme;
use super::*;

impl RustTalkApp {
    pub(super) fn ui_connect(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::DARK))
            .show(ctx, |ui| {
                if self.join_error.is_some() {
                    ui.disable();
                }
                let avail = ui.available_size();
                let card_w = 760.0f32.min(avail.x - 32.0);
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
                                    egui::vec2(240.0, 360.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| self.saved_servers(ui),
                                );
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);
                                // --- Right: connect form ---
                                ui.allocate_ui_with_layout(
                                    egui::vec2(ui.available_width(), 360.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| self.connect_form(ui),
                                );
                            });
                        });
                });

                if self.cert_check.as_ref().is_some_and(|check| !check.matches_address(&self.config.server_address)) {
                    self.cert_check = None;
                    self.join_error = None;
                    self.set_status("Server address changed. Connect again to verify its identity.");
                }
                if let Some(r) = self.update.available.clone() {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(((avail.x - 420.0) / 2.0).max(0.0));
                            ui.label(egui::RichText::new(format!("{}  RustTalk {} is available", icons::regular::ARROW_CIRCLE_UP, r.version)).color(theme::GREEN));
                            if ui.button("Show update").clicked() {
                                self.open_settings(SettingsPage::About);
                            }
                        });
                    });
                }
                if !self.status.is_empty() && self.join_error.is_none() {
                    ui.add_space(14.0);
                    ui.vertical_centered(|ui| {
                        let color = if self.status_is_error { theme::RED } else { theme::MUTED };
                        ui.label(egui::RichText::new(&self.status).color(color));
                    });
                }
            });
        if self.join_error.is_some() {
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
                self.join_error = None;
                self.cert_check = None;
                self.set_status("");
            }
        }
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
                if self.renaming_bookmark.as_ref().is_some_and(|(r, _)| *r == i) {
                    self.rename_row(ui, i);
                    continue;
                }
                let selected = self.config.server_address == bm.address;
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
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
                if resp.clicked() {
                    self.config.server_address = bm.address.clone();
                    self.config.server_password = bm.password.clone();
                    self.server_nickname = bm.nickname().unwrap_or_default().to_string();
                }
                if resp.double_clicked() {
                    self.config.server_address = bm.address.clone();
                    self.config.server_password = bm.password.clone();
                    self.server_nickname = bm.nickname().unwrap_or_default().to_string();
                    self.start_connect();
                }
                resp.on_hover_text(format!("{}\nClick = fill in  -  double click = connect  -  right click = rename or remove", bm.address))
                    .context_menu(|ui| {
                        ui.set_width(160.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if theme::menu_item(ui, icons::regular::PENCIL_SIMPLE, "Rename", false).clicked() {
                            self.renaming_bookmark = Some((i, bm.nickname().unwrap_or_default().to_string()));
                            ui.close_menu();
                        }
                        theme::menu_separator(ui);
                        if theme::menu_item(ui, icons::regular::TRASH, "Remove", true).clicked() {
                            remove_idx = Some(i);
                            ui.close_menu();
                        }
                    });
            }
        });
        if let Some(i) = remove_idx {
            self.config.bookmarks.remove(i);
            self.renaming_bookmark = None;
            self.config.save();
        }
    }

    /// Inline nickname editor: Enter or clicking away saves, Escape cancels.
    fn rename_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some((_, text)) = self.renaming_bookmark.as_mut() else { return };
        let address = self.config.bookmarks.get(i).map(|b| b.address.clone()).unwrap_or_default();
        let resp = ui.add_sized(
            [ui.available_width(), 30.0],
            egui::TextEdit::singleline(text)
                .hint_text(format!("Nickname for {address}"))
                .char_limit(crate::config::MAX_NICKNAME_CHARS),
        );
        ui.label(egui::RichText::new("Enter = save  -  Esc = cancel  -  empty = show address").size(11.0).color(theme::MUTED));
        ui.add_space(4.0);
        if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
            self.renaming_bookmark = None;
        } else if resp.lost_focus() {
            // The connect form must not also act on this Enter.
            ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            if let Some((_, text)) = self.renaming_bookmark.take() {
                if let Some(bm) = self.config.bookmarks.get_mut(i) {
                    bm.set_nickname(&text);
                    if bm.address == self.config.server_address {
                        self.server_nickname = bm.nickname().unwrap_or_default().to_string();
                    }
                    self.config.save();
                }
            }
        } else if !resp.has_focus() {
            resp.request_focus();
        }
    }

    /// Error panel for a failed join: icon, title, explanation, actions, raw details.
    fn join_error_panel(&mut self, ui: &mut egui::Ui, width: f32) {
        let Some(err) = self.join_error.clone() else { return };
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
                                    self.join_error = None;
                                    self.cert_check = None;
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
                                if let Some(e) = self.join_error.as_mut() {
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
        let Some(check) = self.cert_check.clone() else { return };
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
                self.cert_check = None;
                self.start_connect();
            }
            if theme::ghost_button(ui, "Cancel").clicked() {
                self.cert_check = None;
                self.join_error = None;
                self.set_status("");
            }
        });
    }

    fn run_join_action(&mut self, ctx: &egui::Context, action: super::join_error::Action) {
        use super::join_error::Action;
        match action {
            Action::Retry => self.start_connect(),
            Action::EditPassword => {
                self.join_error = None;
                // A stale invite would be refused before the password is checked.
                self.config.invite = None;
                self.config.server_password = None;
                self.focus_password = true;
            }
            Action::ShowUpdate => {
                self.join_error = None;
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
            egui::Button::new(egui::RichText::new(format!("{}  {label}", icons::regular::SIGN_IN)).size(15.0).strong().color(theme::on_accent()))
                .fill(theme::accent())
                .min_size(egui::vec2(w, 38.0)),
        );
        if connect.clicked() || (ui.is_enabled() && !connecting && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
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
            if theme::ghost_button(ui, format!("{}  Save server", icons::regular::BOOKMARK_SIMPLE)).clicked() {
                let addr = self.config.server_address.trim().to_string();
                if !addr.is_empty() {
                    let idx = match self.config.bookmarks.iter().position(|b| b.address == addr) {
                        Some(i) => i,
                        None => {
                            self.config.bookmarks.push(ServerBookmark {
                                name: String::new(),
                                address: addr.clone(),
                                password: self.config.server_password.clone(),
                            });
                            self.config.bookmarks.len() - 1
                        }
                    };
                    self.config.bookmarks[idx].set_nickname(&self.server_nickname);
                    self.config.bookmarks[idx].password = self.config.server_password.clone();
                    self.server_nickname = self.config.bookmarks[idx].nickname().unwrap_or_default().to_string();
                    self.config.save();
                    self.set_status("Server saved");
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
