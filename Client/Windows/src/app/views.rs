//! Main content area: channel header + text chat, voice stage or lobby log.

use super::media_ui::kind_icon;
use super::sidebar::channel_icon;
use super::theme;
use super::*;

impl RustTalkApp {
    pub(super) fn ui_main(
        &mut self,
        ctx: &egui::Context,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                let Some(ch_id) = self.selected_channel.filter(|id| self.channels.contains_key(id)) else {
                    empty_state(ui, icons::regular::CHAT_CIRCLE_DOTS, "No channel selected", "Pick a channel on the left.");
                    return;
                };
                let kind = self.channel_kind(ch_id).unwrap_or(ChannelKind::Text);
                self.ui_channel_header(ui, ch_id, kind, cmd_tx);
                match kind {
                    ChannelKind::Text => self.ui_text_channel(ui, ch_id, cmd_tx),
                    ChannelKind::Voice => self.ui_voice_channel(ui, ch_id, cmd_tx),
                    ChannelKind::Spawn => self.ui_lobby(ui, ch_id),
                    ChannelKind::Category => {
                        empty_state(ui, icons::regular::FOLDER, "Category", "Categories only group channels.")
                    }
                }
            });
    }

    /// 48 px bar: icon, name, subtitle; right: context actions.
    fn ui_channel_header(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        kind: ChannelKind,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        egui::TopBottomPanel::top(format!("channel_header_{ch_id}"))
            .exact_height(48.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(16, 0))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new(channel_icon(kind)).size(22.0).color(theme::MUTED));
                    ui.label(egui::RichText::new(self.channel_name(ch_id)).size(16.0).strong().color(theme::HEADER));
                    let subtitle = match kind {
                        ChannelKind::Text => "Text channel".to_string(),
                        ChannelKind::Voice => {
                            let n = self.channels.get(&ch_id).map(|c| c.user_ids.len()).unwrap_or(0);
                            format!("Voice channel  -  {n} connected")
                        }
                        ChannelKind::Spawn => "Everyone lands here  -  talking disabled".to_string(),
                        ChannelKind::Category => String::new(),
                    };
                    ui.add(egui::Separator::default().vertical().spacing(12.0));
                    ui.label(egui::RichText::new(subtitle).color(theme::MUTED));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, icons::regular::GEAR_SIX, theme::IconState::Normal, "Channel settings")
                            .clicked()
                        {
                            self.open_channel_settings(ch_id);
                        }
                        let _ = theme::icon_button(
                            ui,
                            icons::regular::MAGNIFYING_GLASS,
                            theme::IconState::Disabled,
                            "Search (WIP)",
                        );
                        if kind == ChannelKind::Voice {
                            let here = self.my_channel() == Some(ch_id);
                            if !here && theme::primary_button(ui, &format!("{}  Join Voice", icons::regular::SIGN_IN)).clicked() {
                                let _ = cmd_tx.send(Command::JoinChannel(ch_id));
                            }
                        }
                    });
                });
            });
    }

    /// Text channel: message list + composer.
    fn ui_text_channel(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        // Composer FIRST as bottom panel (otherwise the scroll area pushes
        // it below the window edge).
        egui::TopBottomPanel::bottom(format!("composer_{ch_id}"))
            .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 16, right: 16, top: 4, bottom: 16 }))
            .show_inside(ui, |ui| self.composer(ui, ch_id, cmd_tx));
        let name = self.channel_name(ch_id);
        self.message_list(ui, ch_id, |ui| {
            ui.add_space(24.0);
            ui.label(egui::RichText::new(icons::regular::CHAT_CIRCLE_TEXT).size(44.0).color(theme::MUTED));
            ui.label(egui::RichText::new(format!("Welcome to {name}!")).size(22.0).strong().color(theme::HEADER));
            ui.label(egui::RichText::new("This is the start of the channel. Say hi!").color(theme::MUTED));
        });
    }

    /// Voice channel: stage (members + video tiles), then text feed.
    fn ui_voice_channel(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        let members: Vec<UserInfo> = self
            .channels
            .get(&ch_id)
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.users.get(&id).cloned())
            .collect();
        let talking: HashMap<u64, Instant> = self.talking.lock().unwrap().clone();
        let self_tx = self.control.transmitting.load(Ordering::Relaxed);

        // Stage (top, dark).
        egui::TopBottomPanel::top(format!("stage_{ch_id}"))
            .resizable(true)
            .default_height(260.0)
            .min_height(150.0)
            .frame(egui::Frame::default().fill(theme::DARK).inner_margin(egui::Margin::same(16)))
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if members.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(30.0);
                            ui.label(egui::RichText::new(icons::regular::SPEAKER_HIGH).size(40.0).color(theme::MUTED));
                            ui.label(egui::RichText::new("Nobody is here yet").size(18.0).color(theme::HEADER));
                            if theme::primary_button(ui, "Join Voice").clicked() {
                                let _ = cmd_tx.send(Command::JoinChannel(ch_id));
                            }
                        });
                        return;
                    }
                    let mut open_fs = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                        // Video tiles first (screen before camera).
                        for user in &members {
                            for kind in [VideoKind::Screen, VideoKind::Camera] {
                                if let Some(data) = self.video_frames.get(&(user.id, kind)).cloned() {
                                    if video_tile(ui, &mut self.textures, &data, user, kind).clicked() {
                                        open_fs = Some((user.id, kind));
                                    }
                                }
                            }
                        }
                        for user in &members {
                            let speaking = talking.get(&user.id).is_some_and(|t| t.elapsed() < Duration::from_millis(250))
                                || (user.id == self.my_id && self_tx);
                            member_card(ui, user, speaking, user.id == self.my_id);
                        }
                    });
                    if let Some(key) = open_fs {
                        self.enter_fullscreen(key);
                    }
                });
            });

        // Text feed below the stage.
        egui::TopBottomPanel::bottom(format!("voice_composer_{ch_id}"))
            .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 16, right: 16, top: 4, bottom: 16 }))
            .show_inside(ui, |ui| self.composer(ui, ch_id, cmd_tx));
        self.message_list(ui, ch_id, |ui| {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Voice chat").size(16.0).strong().color(theme::HEADER));
            ui.label(egui::RichText::new("Messages and images shared here are visible to everyone in this channel.").color(theme::MUTED));
        });
    }

    /// Lobby: hint banner + join log.
    fn ui_lobby(&mut self, ui: &mut egui::Ui, ch_id: u64) {
        egui::Frame::default().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icons::regular::HOUSE).size(28.0).color(theme::ACCENT));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Welcome to the lobby").size(17.0).strong().color(theme::HEADER));
                        ui.label(
                            egui::RichText::new(
                                "Everyone lands here when connecting. Microphone, camera and screen sharing are disabled - \
                                 click a voice channel on the left to start talking.",
                            )
                            .color(theme::MUTED),
                        );
                    });
                });
            });
            ui.add_space(12.0);
            theme::caption(ui, "Activity");
        });
        self.message_list(ui, ch_id, |ui| {
            ui.label(egui::RichText::new("No activity yet.").color(theme::MUTED));
        });
    }

    /// Scrollable message list with grouping of consecutive messages.
    fn message_list(&mut self, ui: &mut egui::Ui, ch_id: u64, empty: impl FnOnce(&mut egui::Ui)) {
        let show_ts = self.config.show_timestamps;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                egui::Frame::default().inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match self.chat.get(&ch_id) {
                        Some(lines) if !lines.is_empty() => {
                            let mut prev: Option<(&str, u64)> = None;
                            for line in lines {
                                let grouped = match (&line.kind, prev) {
                                    (ChatKind::Message { from, .. }, Some((pf, pt))) => {
                                        pf == from && line.timestamp.saturating_sub(pt) < 300
                                    }
                                    _ => false,
                                };
                                chat_line(ui, line, &mut self.textures, grouped, show_ts);
                                prev = match &line.kind {
                                    ChatKind::Message { from, .. } => Some((from.as_str(), line.timestamp)),
                                    ChatKind::System => None,
                                };
                            }
                        }
                        _ => empty(ui),
                    }
                });
            });
    }

    /// Message composer: attach button, input field, GIF (WIP), send.
    fn composer(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        if let Some((name, kb)) = self.pending_image.as_ref().map(|(n, b)| (n.clone(), b.len() / 1024)) {
            let mut clear = false;
            egui::Frame::default()
                .fill(theme::INPUT)
                .corner_radius(egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 })
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icons::regular::IMAGE).color(theme::ACCENT));
                        ui.label(egui::RichText::new(name).color(theme::HEADER));
                        ui.label(egui::RichText::new(format!("{kb} KB")).color(theme::MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(icons::regular::X).on_hover_text("Remove").clicked() {
                                clear = true;
                            }
                        });
                    });
                });
            if clear {
                self.pending_image = None;
            }
        }
        let name = self.channel_name(ch_id);
        egui::Frame::default()
            .fill(theme::INPUT)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new(icons::regular::PLUS_CIRCLE).size(20.0).color(theme::MUTED)).frame(false))
                        .on_hover_text("Attach image")
                        .clicked()
                    {
                        self.pick_image();
                    }
                    let right_w = 76.0;
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.chat_input)
                            .hint_text(format!("Message {name}"))
                            .desired_rows(1)
                            .frame(false)
                            .desired_width(ui.available_width() - right_w),
                    );
                    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    if enter {
                        // Strip the newline egui already inserted.
                        while self.chat_input.ends_with('\n') {
                            self.chat_input.pop();
                        }
                        resp.request_focus();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let send = ui
                            .add(egui::Button::new(egui::RichText::new(icons::regular::PAPER_PLANE_RIGHT).size(18.0).color(theme::ACCENT)).frame(false))
                            .on_hover_text("Send (Enter)  -  Shift+Enter for a new line")
                            .clicked();
                        let _ = ui
                            .add_enabled(false, egui::Button::new(egui::RichText::new(icons::regular::GIF).size(18.0)).frame(false))
                            .on_disabled_hover_text("GIF search (WIP)");
                        if send || enter {
                            self.send_chat(ch_id, cmd_tx);
                        }
                    });
                });
            });
    }

    /// Sends text + attached image.
    pub(super) fn send_chat(&mut self, ch_id: u64, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        let text = self.chat_input.trim().to_string();
        let image = self.pending_image.take().map(|(_, b)| b);
        if text.is_empty() && image.is_none() {
            return;
        }
        let _ = cmd_tx.send(Command::SendChat { channel_id: ch_id, text, image });
        self.chat_input.clear();
    }

    /// Pick and attach an image file.
    pub(super) fn pick_image(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
        {
            match std::fs::read(&path) {
                Ok(bytes) if bytes.len() <= MAX_MEDIA_BYTES => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "image".into());
                    self.pending_image = Some((name, bytes));
                }
                Ok(_) => self.set_error("Image too large (max. 4 MiB)"),
                Err(e) => self.set_error(format!("Could not read image: {e}")),
            }
        }
    }
}

/// Centered placeholder for empty views.
fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.label(egui::RichText::new(icon).size(48.0).color(theme::MUTED));
        ui.label(egui::RichText::new(title).size(20.0).strong().color(theme::HEADER));
        ui.label(egui::RichText::new(text).color(theme::MUTED));
    });
}

/// Member card on the voice stage.
fn member_card(ui: &mut egui::Ui, user: &UserInfo, speaking: bool, me: bool) {
    let size = egui::vec2(150.0, 112.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, theme::avatar_color(&user.name).linear_multiply(0.35));
    if speaking {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(2.0_f32, theme::GREEN), egui::StrokeKind::Inside);
    }
    let av = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, 12.0), egui::vec2(52.0, 52.0));
    theme::paint_avatar(p, av, &user.name, speaking);
    let label = if me { format!("{} (you)", user.name) } else { user.name.clone() };
    p.text(
        egui::pos2(rect.center().x, rect.bottom() - 18.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        theme::HEADER,
    );
    let mut x = rect.right() - 16.0;
    for (on, icon) in [(user.deafened, icons::regular::SPEAKER_SLASH), (user.muted, icons::regular::MICROPHONE_SLASH)] {
        if on {
            p.circle_filled(egui::pos2(x, rect.top() + 16.0), 11.0, theme::DARK);
            p.text(egui::pos2(x, rect.top() + 16.0), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(13.0), theme::RED);
            x -= 26.0;
        }
    }
}

/// Video tile (click = fullscreen).
fn video_tile(
    ui: &mut egui::Ui,
    textures: &mut TextureCache,
    data: &[u8],
    user: &UserInfo,
    kind: VideoKind,
) -> egui::Response {
    let size = egui::vec2(300.0, 169.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, egui::Color32::BLACK);
    if let Some(tex) = textures.texture_for(ui.ctx(), data, &format!("video-{}-{kind:?}", user.id)) {
        let fit = theme::fit_size(tex.size_vec2(), rect.size());
        egui::Image::new(&tex)
            .corner_radius(10.0)
            .paint_at(ui, egui::Rect::from_center_size(rect.center(), fit));
    }
    let label = format!("{}  {}", kind_icon(kind), user.name);
    let galley = p.layout_no_wrap(label, egui::FontId::proportional(12.5), egui::Color32::WHITE);
    let pill = egui::Rect::from_min_size(rect.left_bottom() + egui::vec2(8.0, -28.0), galley.size() + egui::vec2(14.0, 8.0));
    p.rect_filled(pill, 6.0, egui::Color32::from_black_alpha(170));
    p.galley(pill.min + egui::vec2(7.0, 4.0), galley, egui::Color32::WHITE);
    if resp.hovered() {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(2.0_f32, theme::ACCENT), egui::StrokeKind::Inside);
        p.text(rect.right_top() + egui::vec2(-14.0, 14.0), egui::Align2::CENTER_CENTER, icons::regular::CORNERS_OUT, egui::FontId::proportional(16.0), egui::Color32::WHITE);
    }
    resp.on_hover_text("Click for fullscreen").on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One chat line: message (avatar, name, time, text, image) or system note.
pub(super) fn chat_line(
    ui: &mut egui::Ui,
    line: &ChatLine,
    textures: &mut TextureCache,
    grouped: bool,
    show_ts: bool,
) {
    match &line.kind {
        ChatKind::System => {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(icons::regular::ARROW_RIGHT).color(theme::GREEN));
                ui.label(egui::RichText::new(&line.text).color(theme::MUTED));
                if show_ts {
                    ui.label(egui::RichText::new(format_time(line.timestamp)).size(11.0).color(theme::DISABLED));
                }
            });
            ui.add_space(2.0);
        }
        ChatKind::Message { from, own } => {
            if !grouped {
                ui.add_space(10.0);
            }
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                if grouped {
                    ui.add_space(44.0);
                } else {
                    ui.vertical(|ui| {
                        theme::draw_avatar(ui, from, 36.0, false);
                    });
                    ui.add_space(4.0);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if !grouped {
                        ui.horizontal(|ui| {
                            let color = if *own { theme::ACCENT.gamma_multiply(1.35) } else { theme::avatar_color(from).gamma_multiply(1.4) };
                            ui.label(egui::RichText::new(from).strong().color(color));
                            if show_ts {
                                ui.label(egui::RichText::new(format_time(line.timestamp)).size(11.0).color(theme::MUTED));
                            }
                        });
                    }
                    if !line.text.is_empty() {
                        ui.add(egui::Label::new(egui::RichText::new(&line.text).color(theme::TEXT)).wrap());
                    }
                    if let Some(img) = &line.image {
                        match textures.texture_for(ui.ctx(), img, "chat-img") {
                            Some(tex) => {
                                ui.add(egui::Image::new(&tex).max_width(360.0).max_height(260.0).corner_radius(8.0));
                            }
                            None => {
                                ui.label(egui::RichText::new("[image could not be decoded]").color(theme::MUTED));
                            }
                        }
                    }
                });
            });
        }
    }
}
