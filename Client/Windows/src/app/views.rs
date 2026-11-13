//! Main content area: channel header + text chat, voice stage or lobby log.

use super::media_ui::kind_icon;
use super::sidebar::channel_icon;
use super::theme;
use super::*;

/// Debug builds only: user whose tile menu the `tile_menu:<name>` dev hook opens.
#[cfg(debug_assertions)]
pub(super) static DEV_TILE_MENU: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

impl RustTalkApp {
    /// "..." button in the bottom right corner of a stage tile (visible on
    /// hover or while its menu is open): opens the user menu.
    fn tile_menu_button(&mut self, ui: &mut egui::Ui, tile: egui::Rect, tile_hovered: bool, user: &UserInfo, kind: Option<VideoKind>) {
        let popup_id = ui.make_persistent_id(("tile_menu", user.id, kind));
        let open = ui.memory(|m| m.is_popup_open(popup_id));
        let rect = egui::Rect::from_center_size(tile.right_bottom() + egui::vec2(-22.0, -20.0), egui::vec2(30.0, 24.0));
        let resp = ui.interact(rect, popup_id.with("btn"), egui::Sense::click());
        if tile_hovered || resp.hovered() || open {
            let p = ui.painter();
            p.rect_filled(rect, 8.0, if resp.hovered() || open { theme::SELECTED } else { egui::Color32::from_black_alpha(160) });
            p.text(rect.center(), egui::Align2::CENTER_CENTER, icons::regular::DOTS_THREE, egui::FontId::proportional(18.0), egui::Color32::WHITE);
        }
        #[cfg(debug_assertions)]
        let dev_open = {
            let mut want = DEV_TILE_MENU.lock().unwrap();
            want.as_deref() == Some(user.name.as_str()) && want.take().is_some()
        };
        #[cfg(not(debug_assertions))]
        let dev_open = false;
        if resp.clicked() || dev_open {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        let resp = resp.on_hover_text("More options");
        egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            self.user_menu(ui, user);
        });
    }

    pub(super) fn ui_main(
        &mut self,
        ctx: &egui::Context,
        cmd_tx: &net::CommandSender,
    ) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                let Some(ch_id) = self.srv.selected_channel.filter(|id| self.srv.channels.contains_key(id)) else {
                    empty_state(ui, icons::regular::CHAT_CIRCLE_DOTS, "No channel selected", "Pick a channel on the left.");
                    self.update_banner_overlay(ctx);
                    return;
                };
                let kind = self.channel_kind(ch_id).unwrap_or(ChannelKind::Text);
                self.ui_channel_header(ui, ch_id, kind);
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
    fn ui_channel_header(&mut self, ui: &mut egui::Ui, ch_id: u64, kind: ChannelKind) {
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
                            let n = self.srv.channels.get(&ch_id).map(|c| c.user_ids.len()).unwrap_or(0);
                            format!("Voice channel  -  {n} connected")
                        }
                        ChannelKind::Spawn => "Everyone lands here  -  talking disabled".to_string(),
                        ChannelKind::Category => String::new(),
                    };
                    ui.add(egui::Separator::default().vertical().spacing(12.0));
                    ui.label(egui::RichText::new(subtitle).color(theme::MUTED));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.update_banner(ui);
                        if theme::icon_button(ui, icons::regular::GEAR_SIX, theme::IconState::Normal, "Channel settings")
                            .clicked()
                        {
                            self.open_channel_settings(ch_id);
                        }
                        let searching = self.chat_search.is_some();
                        let state = if searching { theme::IconState::Active } else { theme::IconState::Normal };
                        if theme::icon_button(ui, icons::regular::MAGNIFYING_GLASS, state, "Search this channel").clicked() {
                            self.chat_search = if searching { None } else { Some(String::new()) };
                        }
                        if let Some(q) = self.chat_search.as_mut() {
                            let resp = ui.add(egui::TextEdit::singleline(q).hint_text("Search messages").desired_width(180.0));
                            if !resp.has_focus() && q.is_empty() && !resp.lost_focus() {
                                resp.request_focus();
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                self.chat_search = None;
                            }
                        }
                        if kind == ChannelKind::Voice {
                            let here = self.my_channel() == Some(ch_id);
                            if !here && theme::primary_button(ui, &format!("{}  Join Voice", icons::regular::SIGN_IN)).clicked() {
                                self.join_channel(ch_id);
                            }
                        }
                        if kind == ChannelKind::Spawn && self.my_channel() != Some(ch_id) && ui.button(format!("{}  Back to Lobby", icons::regular::SIGN_IN)).clicked() {
                            self.join_channel(ch_id);
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
        cmd_tx: &net::CommandSender,
    ) {
        // Composer FIRST as bottom panel (otherwise the scroll area pushes
        // it below the window edge).
        egui::TopBottomPanel::bottom(format!("composer_{ch_id}"))
            .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 16, right: 16, top: 8, bottom: 8 }))
            .show_separator_line(false)
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
        cmd_tx: &net::CommandSender,
    ) {
        let members: Vec<UserInfo> = self
            .srv.channels
            .get(&ch_id)
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.srv.users.get(&id).cloned())
            .collect();
        let (talking, _) = self.voice_maps();
        let self_tx = self.viewing_voice() && self.control.transmitting.load(Ordering::Relaxed);

        // Stage (top, dark): all tiles in one centered grid that scales with
        // the available space.
        let has_video = members.iter().any(|u| !self.streams_of(u).is_empty());
        // With video the stage takes most of the height (big streams), the
        // chat below stays usable; without video it stays compact.
        let full = ui.available_height();
        let stage_h = if has_video { (full * 0.62).max(260.0) } else { 260.0f32.min(full * 0.5) };
        egui::TopBottomPanel::top(format!("stage_{ch_id}"))
            .exact_height(stage_h)
            .frame(egui::Frame::default().fill(theme::DARK).inner_margin(egui::Margin::same(16)))
            .show_inside(ui, |ui| {
                if members.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(30.0);
                        ui.label(egui::RichText::new(icons::regular::SPEAKER_HIGH).size(40.0).color(theme::MUTED));
                        ui.label(egui::RichText::new("Nobody is here yet").size(18.0).color(theme::HEADER));
                        if theme::primary_button(ui, "Join Voice").clicked() {
                            self.join_channel(ch_id);
                        }
                    });
                    return;
                }
                // Tiles: screen shares first, then everyone else - with the
                // camera instead of the avatar card when it is on.
                let my_id = self.srv.my_id;
                let speaking_of = |u: &UserInfo| {
                    talking.get(&u.id).is_some_and(|t| t.elapsed() < Duration::from_millis(250))
                        || (u.id == my_id && self_tx)
                };
                // Streams of others only arrive once watched (opt-in): until
                // then their tile is a "Watch" card.
                let mut tiles: Vec<(UserInfo, Option<VideoKind>)> = Vec::new();
                for user in &members {
                    if self.streams_of(user).contains(&VideoKind::Screen) {
                        tiles.push((user.clone(), Some(VideoKind::Screen)));
                    }
                }
                for user in &members {
                    let cam = self.streams_of(user).contains(&VideoKind::Camera);
                    tiles.push((user.clone(), cam.then_some(VideoKind::Camera)));
                }

                let area = ui.available_rect_before_wrap();
                ui.allocate_rect(area, egui::Sense::hover());
                let gap = 12.0;
                let (cols, tile) = stage_grid(tiles.len(), area.size(), gap);
                let mut open_fs = None;
                for (i, rect) in grid_rects(tiles.len(), cols, tile, gap, area).into_iter().enumerate() {
                    let (user, kind) = &tiles[i];
                    let me = user.id == self.srv.my_id;
                    let resp = match kind {
                        Some(kind) if !me && !self.watching.contains(&(user.id, *kind)) => {
                            let speaking = *kind == VideoKind::Camera && speaking_of(user);
                            let resp = watch_tile(ui, rect, user, *kind, speaking);
                            if resp.clicked() {
                                self.watch(user.id, *kind);
                            }
                            resp
                        }
                        Some(kind) => {
                            let tex = self.video_frames.get(&(user.id, *kind)).and_then(|v| v.tex.clone());
                            // Speaking ring only on the person's camera, not on screen shares.
                            let speaking = *kind == VideoKind::Camera && speaking_of(user);
                            let resp = video_tile(ui, rect, tex.as_ref(), user, *kind, speaking, me);
                            if resp.clicked() {
                                open_fs = Some((user.id, *kind));
                            }
                            // Watching someone else: "stop watching" in the top right corner.
                            if !me && (resp.hovered() || ui.rect_contains_pointer(rect)) {
                                let b = egui::Rect::from_center_size(rect.right_top() + egui::vec2(-22.0, 22.0), egui::vec2(30.0, 30.0));
                                let stop = ui
                                    .put(b, egui::Button::new(egui::RichText::new(icons::regular::EYE_SLASH).size(16.0).color(egui::Color32::WHITE)).fill(egui::Color32::from_black_alpha(160)).corner_radius(15.0))
                                    .on_hover_text("Stop watching");
                                if stop.clicked() {
                                    self.unwatch(user.id, *kind);
                                }
                            }
                            resp
                        }
                        None => member_card(ui, rect, user, speaking_of(user), me),
                    };
                    // Right click / "..." button: same menu as in the sidebar.
                    if !me {
                        resp.context_menu(|ui| self.user_menu(ui, user));
                        self.tile_menu_button(ui, rect, resp.hovered(), user, *kind);
                    }
                }
                if let Some(key) = open_fs {
                    self.enter_fullscreen(key);
                }
            });

        // Text feed below the stage.
        egui::TopBottomPanel::bottom(format!("voice_composer_{ch_id}"))
            .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 16, right: 16, top: 8, bottom: 8 }))
            .show_separator_line(false)
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
            let settings = self.srv.server_settings.clone();
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
                    super::server_settings::paint_server_icon(ui, r, &settings, &mut self.textures);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(format!("Welcome to {}", settings.name)).size(18.0).strong().color(theme::HEADER));
                        if !settings.welcome_message.is_empty() {
                            ui.add(egui::Label::new(egui::RichText::new(&settings.welcome_message).color(theme::TEXT)).wrap());
                            ui.add_space(2.0);
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  This is the lobby: microphone, camera and screen sharing are disabled here. \
                                 Click a voice channel on the left to start talking.",
                                icons::regular::INFO
                            ))
                            .size(12.5)
                            .color(theme::MUTED),
                        );
                    });
                });
            });
            if !settings.rules.is_empty() {
                ui.add_space(8.0);
                theme::card(ui, |ui| {
                    theme::caption(ui, &format!("{}  Server rules", icons::regular::SCROLL));
                    ui.add(egui::Label::new(egui::RichText::new(&settings.rules).color(theme::TEXT)).wrap());
                });
            }
            ui.add_space(12.0);
            theme::caption(ui, "Activity");
        });
        self.message_list(ui, ch_id, |ui| {
            ui.label(egui::RichText::new("No activity yet.").color(theme::MUTED));
        });
    }

    /// Scrollable message list with grouping of consecutive messages.
    /// With an open search only matching lines are shown.
    fn message_list(&mut self, ui: &mut egui::Ui, ch_id: u64, empty: impl FnOnce(&mut egui::Ui)) {
        let show_ts = self.config.show_timestamps;
        let animate = self.config.animate_gifs;
        let me = self.config.username.clone();
        let query = self.chat_search.as_ref().map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty());
        let replying = self.reply_to;
        let editing = self.editing;
        let mut action = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(query.is_none())
            .show(ui, |ui| {
                egui::Frame::default().inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let lines = self.srv.chat.get(&ch_id).map(Vec::as_slice).unwrap_or(&[]);
                    if lines.is_empty() {
                        empty(ui);
                        return;
                    }
                    let matches = |l: &ChatLine| match &query {
                        None => true,
                        Some(q) => {
                            l.text.to_lowercase().contains(q)
                                || matches!(&l.kind, ChatKind::Message { from, .. } if from.to_lowercase().contains(q))
                        }
                    };
                    if query.is_some() {
                        let n = lines.iter().filter(|l| matches(l)).count();
                        ui.label(egui::RichText::new(format!("{} {n} result(s)", icons::regular::MAGNIFYING_GLASS)).color(theme::MUTED));
                    }
                    let mut prev: Option<(&str, u64)> = None;
                    for line in lines.iter().filter(|l| matches(l)) {
                        let grouped = query.is_none()
                            && line.reply_to.is_none()
                            && match (&line.kind, prev) {
                                (ChatKind::Message { from, .. }, Some((pf, pt))) => pf == from && line.timestamp.saturating_sub(pt) < 300,
                                _ => false,
                            };
                        let is_edit = editing == Some(line.id) && line.id != 0;
                        let ctx = LineCtx {
                            lines,
                            me: &me,
                            grouped: grouped && !is_edit,
                            show_ts,
                            animate,
                            replying: replying == Some(line.id) && line.id != 0,
                        };
                        let edit = is_edit.then_some(EditBox { text: &mut self.edit_text, focus: &mut self.focus_edit });
                        if let Some(a) = chat_line(ui, line, &mut self.textures, &ctx, edit, &mut self.markdown) {
                            action = Some(a);
                        }
                        prev = match &line.kind {
                            ChatKind::Message { from, .. } => Some((from.as_str(), line.timestamp)),
                            ChatKind::System => None,
                        };
                    }
                });
            });
        if action.is_some() {
            self.chat_action = action;
        }
    }

    /// Message composer: attach button, input field, emoji and GIF pickers, send.
    fn composer(
        &mut self,
        ui: &mut egui::Ui,
        ch_id: u64,
        cmd_tx: &net::CommandSender,
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
                        ui.label(egui::RichText::new(icons::regular::IMAGE).color(theme::accent()));
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
        self.composer_context_bar(ui, ch_id);
        let name = self.channel_name(ch_id);
        let focused = ui.memory(|m| m.has_focus(composer_id(ch_id)));
        self.mention_keys(ui.ctx(), ch_id, focused);
        let frame = egui::Frame::default()
            .fill(theme::INPUT)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if theme::ghost_button(ui, egui::RichText::new(icons::regular::PLUS_CIRCLE).size(20.0).color(theme::MUTED))
                        .on_hover_text("Attach image")
                        .clicked()
                    {
                        self.pick_image();
                    }
                    let right_w = 110.0;
                    let mut layouter = super::markdown::composer;
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.chat_input)
                            .layouter(&mut layouter)
                            .char_limit(rusttalk_shared::protocol::MAX_CHAT_LEN)
                            .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))
                            .id(composer_id(ch_id))
                            .hint_text(format!("Message {name}"))
                            .desired_rows(1)
                            .frame(false)
                            .desired_width(ui.available_width() - right_w),
                    );
                    // After Reply / Edit: type right away, cursor at the end.
                    if std::mem::take(&mut self.focus_composer) {
                        resp.request_focus();
                        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                            let end = egui::text::CCursor::new(self.chat_input.chars().count());
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                            state.store(ui.ctx(), resp.id);
                        }
                    }
                    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    // Esc cancels a reply; Up in an empty input edits the last own message.
                    if (resp.has_focus() || resp.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.reply_to = None;
                    }
                    if resp.has_focus() && self.chat_input.is_empty() && self.editing.is_none() && ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        let last_own = self.srv.chat.get(&ch_id).and_then(|v| {
                            v.iter().rev().find(|l| l.id != 0 && matches!(l.kind, ChatKind::Message { own: true, .. })).map(|l| l.id)
                        });
                        if let Some(id) = last_own {
                            self.chat_action = Some(ChatAction::Edit(id));
                        }
                    }
                    if enter {
                        resp.request_focus();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let send = theme::ghost_button(ui, egui::RichText::new(icons::regular::PAPER_PLANE_RIGHT).size(18.0).color(theme::accent()))
                            .on_hover_text("Send (Enter)  -  Shift+Enter for a new line")
                            .clicked();
                        let gif_color = if self.giphy.open { theme::accent() } else { theme::MUTED };
                        let gif = theme::ghost_button(ui, egui::RichText::new(icons::regular::GIF).size(18.0).color(gif_color))
                            .on_hover_text("GIFs");
                        self.gif_button_rect = Some(gif.rect);
                        if gif.clicked() {
                            if self.giphy.open {
                                self.close_gif_picker();
                            } else {
                                self.emoji = Default::default();
                                self.open_gif_picker();
                            }
                        }
                        let emoji_color = if self.emoji.open { theme::accent() } else { theme::MUTED };
                        let emoji = theme::ghost_button(ui, egui::RichText::new(icons::regular::SMILEY).size(18.0).color(emoji_color))
                            .on_hover_text("Emoji");
                        self.emoji_button_rect = Some(emoji.rect);
                        if emoji.clicked() {
                            self.toggle_emoji_picker(ch_id);
                        }
                        if send || enter {
                            self.send_chat(ch_id, cmd_tx);
                        }
                    });
                });
            });
        self.mention_popup(ui.ctx(), ch_id, focused, frame.response.rect);
    }

    /// Sends text + attached image (as a reply if one is selected).
    pub(super) fn send_chat(&mut self, ch_id: u64, cmd_tx: &net::CommandSender) {
        let text = super::emoji::replace_shortcodes(self.chat_input.trim());
        if text.is_empty() && self.pending_image.is_none() {
            return;
        }
        // Kept until the command is queued: a busy connection loses nothing.
        let image = self.pending_image.as_ref().map(|(_, b)| b.clone());
        let reply_to = self.reply_to;
        if cmd_tx.send(Command::SendChat { channel_id: ch_id, text, image, reply_to }).is_err() {
            self.set_error("Not sent: the connection is busy. Try again in a moment.");
            return;
        }
        self.pending_image = None;
        self.reply_to = None;
        self.chat_input.clear();
    }

    /// "Replying to ..." bar above the input.
    fn composer_context_bar(&mut self, ui: &mut egui::Ui, ch_id: u64) {
        let (icon, title, target) = match self.reply_to {
            Some(id) => {
                let who = self
                    .srv.chat
                    .get(&ch_id)
                    .and_then(|v| v.iter().find(|l| l.id == id))
                    .map(|l| match &l.kind {
                        ChatKind::Message { from, .. } => from.clone(),
                        ChatKind::System => String::new(),
                    })
                    .unwrap_or_default();
                (icons::regular::ARROW_BEND_UP_LEFT, format!("Replying to {who}"), id)
            }
            None => return,
        };
        let snippet = self
            .srv.chat
            .get(&ch_id)
            .and_then(|v| v.iter().find(|l| l.id == target))
            .map(|l| snippet(&l.text, l.image.is_some()))
            .unwrap_or_default();
        let mut cancel = false;
        egui::Frame::default()
            .fill(theme::INPUT)
            .corner_radius(egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 })
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icon).color(theme::accent()));
                    ui.label(egui::RichText::new(title).strong().color(theme::HEADER));
                    ui.label(egui::RichText::new(snippet).color(theme::MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(icons::regular::X).on_hover_text("Cancel (Esc)").clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        if cancel {
            self.reply_to = None;
        }
    }

    /// "Delete message?" dialog with a preview of the message.
    pub(super) fn ui_confirm_delete_message(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete_msg else { return };
        let Some(line) = self.srv.chat.values().flatten().find(|l| l.id == id) else {
            self.confirm_delete_msg = None;
            return;
        };
        let from = match &line.kind {
            ChatKind::Message { from, .. } => from.clone(),
            ChatKind::System => String::new(),
        };
        let preview = snippet(&line.text, line.image.is_some());
        let time = format_time(line.timestamp);
        let mut open = true;
        let mut confirm = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new("Delete Message")
            .id(egui::Id::new("confirm_delete_message"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([420.0, 0.0])
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Are you sure you want to delete this message? This cannot be undone.").color(theme::TEXT));
                ui.add_space(6.0);
                egui::Frame::default()
                    .fill(theme::DARK)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            theme::draw_avatar(ui, &from, 28.0, false);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&from).strong().color(theme::HEADER));
                                    ui.label(egui::RichText::new(&time).size(11.0).color(theme::MUTED));
                                });
                                ui.add(egui::Label::new(egui::RichText::new(&preview).color(theme::TEXT)).wrap());
                            });
                        });
                    });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{}  Tip: hold Shift while clicking the trash icon to delete without asking.", icons::regular::INFO))
                        .size(11.5)
                        .color(theme::MUTED),
                );
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::danger_button(ui, &format!("{}  Delete", icons::regular::TRASH)).clicked() {
                        confirm = true;
                    }
                    if theme::ghost_button(ui, "Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm && !cancel {
            if let Some(tx) = &self.srv.cmd_tx {
                let _ = tx.send(Command::DeleteChat(id));
            }
        }
        if confirm || cancel || !open {
            self.confirm_delete_msg = None;
        }
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

/// Columns and tile size for `n` equal 16:9 tiles in `avail`: tries every
/// column count and keeps the one with the biggest tiles.
pub(super) fn stage_grid(n: usize, avail: egui::Vec2, gap: f32) -> (usize, egui::Vec2) {
    const ASPECT: f32 = 16.0 / 9.0;
    if n == 0 || avail.x <= 0.0 || avail.y <= 0.0 {
        return (1, egui::Vec2::ZERO);
    }
    let mut best = (1, egui::Vec2::ZERO);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let w_by_width = (avail.x - gap * (cols as f32 - 1.0)) / cols as f32;
        let h_by_height = (avail.y - gap * (rows as f32 - 1.0)) / rows as f32;
        let w = w_by_width.min(h_by_height * ASPECT).max(0.0);
        if w > best.1.x {
            best = (cols, egui::vec2(w, w / ASPECT));
        }
    }
    best
}

/// Positions of the tiles: rows centered horizontally (also the last,
/// shorter row), the whole block centered vertically.
pub(super) fn grid_rects(n: usize, cols: usize, tile: egui::Vec2, gap: f32, area: egui::Rect) -> Vec<egui::Rect> {
    if n == 0 || cols == 0 {
        return Vec::new();
    }
    let rows = n.div_ceil(cols);
    let block_h = rows as f32 * tile.y + (rows as f32 - 1.0) * gap;
    let top = area.top() + ((area.height() - block_h) / 2.0).max(0.0);
    let mut out = Vec::with_capacity(n);
    for row in 0..rows {
        let in_row = (n - row * cols).min(cols);
        let row_w = in_row as f32 * tile.x + (in_row as f32 - 1.0) * gap;
        let left = area.left() + ((area.width() - row_w) / 2.0).max(0.0);
        for c in 0..in_row {
            let min = egui::pos2(left + c as f32 * (tile.x + gap), top + row as f32 * (tile.y + gap));
            out.push(egui::Rect::from_min_size(min, tile));
        }
    }
    out
}

/// Mute/deafen badges in the top right corner of a tile.
fn status_badges(p: &egui::Painter, rect: egui::Rect, user: &UserInfo) {
    let r = (rect.height() * 0.09).clamp(10.0, 15.0);
    let mut x = rect.right() - r - 6.0;
    for (on, icon) in [(user.deafened, icons::regular::SPEAKER_SLASH), (user.muted, icons::regular::MICROPHONE_SLASH)] {
        if on {
            let c = egui::pos2(x, rect.top() + r + 6.0);
            p.circle_filled(c, r, theme::DARK);
            p.text(c, egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(r * 1.15), theme::RED);
            x -= 2.0 * r + 6.0;
        }
    }
}

/// Name pill in the bottom left corner of a tile.
fn name_pill(p: &egui::Painter, rect: egui::Rect, text: String) {
    let size = (rect.height() * 0.075).clamp(12.0, 15.0);
    let galley = p.layout_no_wrap(text, egui::FontId::proportional(size), egui::Color32::WHITE);
    let pill = egui::Rect::from_min_size(
        rect.left_bottom() + egui::vec2(8.0, -(galley.size().y + 16.0)),
        galley.size() + egui::vec2(14.0, 8.0),
    );
    p.rect_filled(pill, 6.0, egui::Color32::from_black_alpha(170));
    p.galley(pill.min + egui::vec2(7.0, 4.0), galley, egui::Color32::WHITE);
}

/// Member without camera: avatar + name, scales with the tile.
fn member_card(ui: &mut egui::Ui, rect: egui::Rect, user: &UserInfo, speaking: bool, me: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("member_tile", user.id)), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, theme::avatar_color(&user.name).linear_multiply(0.35));
    if speaking {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(3.0_f32, theme::GREEN), egui::StrokeKind::Inside);
    }
    let d = (rect.height() * 0.42).clamp(40.0, 128.0);
    let av = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, rect.height() * 0.08), egui::vec2(d, d));
    theme::paint_avatar(p, av, &user.name, speaking);
    let label = if me { format!("{} (you)", user.name) } else { user.name.clone() };
    p.text(
        egui::pos2(rect.center().x, av.bottom() + (rect.bottom() - av.bottom()) / 2.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional((rect.height() * 0.08).clamp(13.0, 17.0)),
        theme::HEADER,
    );
    status_badges(p, rect, user);
    resp
}

/// Camera or screen tile (click = fullscreen). The camera replaces the
/// avatar card: green frame while the person speaks, no avatar.
/// A stream we don't receive yet: avatar, what is shared and a "Watch" button
/// (the whole tile is the button).
fn watch_tile(ui: &mut egui::Ui, rect: egui::Rect, user: &UserInfo, kind: VideoKind, speaking: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("watch_tile", user.id, kind)), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, egui::Color32::from_rgb(0x11, 0x12, 0x14));
    p.rect_filled(rect, 10.0, theme::avatar_color(&user.name).linear_multiply(0.18));
    if speaking {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(3.0_f32, theme::GREEN), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(2.0_f32, theme::accent()), egui::StrokeKind::Inside);
    }
    let d = (rect.height() * 0.30).clamp(32.0, 96.0);
    let av = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, rect.height() * 0.16), egui::vec2(d, d));
    theme::paint_avatar(p, av, &user.name, speaking);
    // LIVE badge top left.
    let live = p.layout_no_wrap("LIVE".into(), egui::FontId::proportional(11.0), egui::Color32::WHITE);
    let badge = egui::Rect::from_min_size(rect.left_top() + egui::vec2(10.0, 10.0), live.size() + egui::vec2(12.0, 6.0));
    p.rect_filled(badge, 4.0, theme::RED);
    p.galley(badge.min + egui::vec2(6.0, 3.0), live, egui::Color32::WHITE);
    let what = match kind {
        VideoKind::Screen => format!("{} is sharing their screen", user.name),
        VideoKind::Camera => format!("{} turned on their camera", user.name),
    };
    let text_y = av.bottom() + 16.0;
    p.text(egui::pos2(rect.center().x, text_y), egui::Align2::CENTER_TOP, what, egui::FontId::proportional((rect.height() * 0.065).clamp(12.0, 15.0)), theme::HEADER);
    // Button look (the click is the whole tile).
    let label = p.layout_no_wrap(format!("{}  Watch {}", icons::regular::EYE, if kind == VideoKind::Screen { "Stream" } else { "Camera" }), egui::FontId::proportional(14.0), theme::on_accent());
    let btn = egui::Rect::from_center_size(egui::pos2(rect.center().x, text_y + 44.0), label.size() + egui::vec2(28.0, 14.0));
    p.rect_filled(btn, 8.0, if resp.hovered() { theme::accent().gamma_multiply(1.15) } else { theme::accent() });
    p.galley(btn.center() - label.size() / 2.0, label, theme::on_accent());
    status_badges(p, rect, user);
    resp.on_hover_text("Click to watch - nothing is downloaded until you do").on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn video_tile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tex: Option<&egui::TextureHandle>,
    user: &UserInfo,
    kind: VideoKind,
    speaking: bool,
    me: bool,
) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("stage_tile", user.id, kind)), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, egui::Color32::BLACK);
    match tex {
        Some(tex) => {
            let fit = theme::fit_size(tex.size_vec2(), rect.size());
            egui::Image::new(tex)
                .corner_radius(10.0)
                .paint_at(ui, egui::Rect::from_center_size(rect.center(), fit));
        }
        None => {
            // First keyframe not there yet.
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "Connecting...", egui::FontId::proportional(13.0), theme::MUTED);
        }
    }
    let name = if me { format!("{} (you)", user.name) } else { user.name.clone() };
    let label = match kind {
        VideoKind::Screen => format!("{}  {name}", kind_icon(kind)),
        VideoKind::Camera => name,
    };
    name_pill(p, rect, label);
    if kind == VideoKind::Camera {
        status_badges(p, rect, user);
    }
    if speaking {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(3.0_f32, theme::GREEN), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_stroke(rect, 10.0, egui::Stroke::new(2.0_f32, theme::accent()), egui::StrokeKind::Inside);
    }
    if resp.hovered() {
        // Bottom right belongs to the "..." menu button; fullscreen hint next to it.
        let c = rect.right_bottom() + egui::vec2(if me { -20.0 } else { -56.0 }, -20.0);
        p.circle_filled(c, 14.0, egui::Color32::from_black_alpha(150));
        p.text(c, egui::Align2::CENTER_CENTER, icons::regular::CORNERS_OUT, egui::FontId::proportional(16.0), egui::Color32::WHITE);
    }
    resp.on_hover_text("Click for fullscreen").on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Stable id of the message input of a channel (for focusing it).
pub(super) fn composer_id(ch_id: u64) -> egui::Id {
    egui::Id::new(("composer", ch_id))
}

/// Context for drawing one chat line.
pub(super) struct LineCtx<'a> {
    /// All lines of the channel (to show what a reply refers to).
    lines: &'a [ChatLine],
    /// Own username (mention highlight).
    me: &'a str,
    grouped: bool,
    show_ts: bool,
    animate: bool,
    /// This message is being answered (composer "Replying to").
    replying: bool,
}

/// Inline editor of the message being edited.
pub(super) struct EditBox<'a> {
    text: &'a mut String,
    /// Focus the field (once, after "Edit").
    focus: &'a mut bool,
}

/// Short one-line preview of a message.
pub(super) fn snippet(text: &str, has_image: bool) -> String {
    let plain = super::markdown::plain(text);
    let flat = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s: String = flat.chars().take(70).collect();
    if flat.chars().count() > 70 {
        s.push_str("...");
    }
    if s.is_empty() && has_image {
        s = "[image]".to_string();
    }
    s
}

/// One chat line: message (avatar, name, time, reply, text, image) or system
/// note. Returns the action picked via hover buttons or right click.
pub(super) fn chat_line(
    ui: &mut egui::Ui,
    line: &ChatLine,
    textures: &mut TextureCache,
    c: &LineCtx,
    edit: Option<EditBox>,
    markdown: &mut super::markdown::Cache,
) -> Option<ChatAction> {
    let (from, own) = match &line.kind {
        ChatKind::System => {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(icons::regular::ARROW_RIGHT).color(theme::GREEN));
                ui.label(egui::RichText::new(&line.text).color(theme::MUTED));
                if c.show_ts {
                    ui.label(egui::RichText::new(format_time(line.timestamp)).size(11.0).color(theme::DISABLED));
                }
            });
            ui.add_space(2.0);
            return None;
        }
        ChatKind::Message { from, own } => (from, *own),
    };
    let compact = theme::compact();
    if !c.grouped {
        ui.add_space(theme::dense(10.0, 4.0));
    }
    let name_color = if own { theme::accent().gamma_multiply(1.35) } else { theme::avatar_color(from).gamma_multiply(1.4) };
    let mentioned = !own && mentions(&line.text, c.me);
    let editing = edit.is_some();
    let mut action = None;
    // Background (mention / reply target) is painted below the content afterwards.
    let bg = ui.painter().add(egui::Shape::Noop);
    // Reserve a stable interaction before children; updating its rect keeps their click priority.
    let row_id = ui.id().with(("chat-line", line.id));
    ui.interact(egui::Rect::from_min_size(ui.next_widget_position(), egui::Vec2::ZERO), row_id, egui::Sense::click());
    let row = ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            if compact {
            } else if c.grouped {
                ui.add_space(44.0);
            } else {
                ui.vertical(|ui| {
                    theme::draw_avatar(ui, from, 36.0, false);
                });
                ui.add_space(4.0);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if let Some(target) = line.reply_to {
                    let quoted = c.lines.iter().find(|l| l.id == target);
                    let text = match quoted {
                        Some(q) => {
                            let who = match &q.kind {
                                ChatKind::Message { from, .. } => from.as_str(),
                                ChatKind::System => "",
                            };
                            format!("{}  {who}: {}", icons::regular::ARROW_BEND_UP_RIGHT, snippet(&q.text, q.image.is_some()))
                        }
                        None => format!("{}  Original message was deleted", icons::regular::ARROW_BEND_UP_RIGHT),
                    };
                    ui.label(egui::RichText::new(text).size(12.0).color(theme::MUTED));
                }
                if !c.grouped && !compact {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(from).strong().color(name_color));
                        if c.show_ts {
                            ui.label(egui::RichText::new(format_time(line.timestamp)).size(11.0).color(theme::MUTED));
                        }
                    });
                }
                if let Some(edit) = edit {
                    action = inline_editor(ui, line.id, edit);
                } else if compact {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if c.show_ts {
                            ui.label(egui::RichText::new(format_time(line.timestamp)).size(11.0).color(theme::MUTED));
                        }
                        ui.label(egui::RichText::new(from).strong().color(name_color));
                        if !line.text.is_empty() {
                            markdown.show(ui, line.id, &line.text);
                        }
                        if line.edited {
                            ui.label(egui::RichText::new("(edited)").size(11.0).color(theme::DISABLED));
                        }
                    });
                } else if !line.text.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        markdown.show(ui, line.id, &line.text);
                        if line.edited {
                            ui.label(egui::RichText::new("(edited)").size(11.0).color(theme::DISABLED));
                        }
                    });
                }
                if let Some(img) = &line.image {
                    match textures.image_for(ui.ctx(), img, "chat-img", c.animate) {
                        Some(tex) => {
                            ui.add(egui::Image::new(&tex).max_width(360.0).max_height(260.0).corner_radius(8.0));
                        }
                        None => {
                            ui.label(egui::RichText::new("[image could not be decoded]").color(theme::MUTED));
                        }
                    }
                }
            });
        }).response.rect;

    let tint = if c.replying || editing {
        Some((theme::accent().gamma_multiply(0.22), theme::accent()))
    } else if mentioned {
        Some((theme::YELLOW.gamma_multiply(0.12), theme::YELLOW))
    } else {
        None
    };
    if let Some((fill, bar)) = tint {
        let r = row.expand2(egui::vec2(8.0, 2.0));
        ui.painter().set(bg, egui::Shape::rect_filled(r, 4.0, fill));
        ui.painter().rect_filled(egui::Rect::from_min_max(r.left_top(), egui::pos2(r.left() + 3.0, r.bottom())), 0.0, bar);
        if c.replying {
            // Small badge top right: this is the message you are answering.
            ui.painter().text(
                egui::pos2(r.right() - 8.0, r.top() + 4.0),
                egui::Align2::RIGHT_TOP,
                format!("{}  Replying", icons::regular::ARROW_BEND_UP_LEFT),
                egui::FontId::proportional(11.5),
                theme::accent(),
            );
        }
    }

    if line.id == 0 || editing {
        return action; // no id (old server / local line) or inline editor open: no other actions
    }
    let shift = ui.input(|i| i.modifiers.shift);
    let delete = |id| if shift { ChatAction::DeleteNow(id) } else { ChatAction::Delete(id) };
    // Right click: same actions as the hover buttons.
    let mut resp = ui.interact(row, row_id, egui::Sense::click());
    // Selectable labels own their clicks; forward only secondary clicks to the row menu.
    if ui.rect_contains_pointer(row)
        && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
    {
        resp.flags.insert(egui::response::Flags::HOVERED | egui::response::Flags::CLICKED);
    }
    resp.context_menu(|ui| {
        ui.set_width(180.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        if theme::menu_item(ui, icons::regular::ARROW_BEND_UP_LEFT, "Reply", false).clicked() {
            action = Some(ChatAction::Reply(line.id));
            ui.close_menu();
        }
        if own {
            if theme::menu_item(ui, icons::regular::PENCIL_SIMPLE, "Edit", false).clicked() {
                action = Some(ChatAction::Edit(line.id));
                ui.close_menu();
            }
            if theme::menu_item(ui, icons::regular::TRASH, "Delete", true).clicked() {
                action = Some(delete(line.id));
                ui.close_menu();
            }
        }
    });
    // Hover toolbar (top right of the message, only while fully visible).
    // It lives in its own foreground layer on top of the message. "Hovered"
    // therefore means: pointer over the message OR the toolbar, and the top
    // layer there is the chat or the toolbar (not a menu/window above).
    // (`rect_contains_pointer` alone turned false as soon as the pointer
    // reached the toolbar -> it vanished and flickered, clicks were lost.)
    let mut buttons: Vec<(&str, &str, ChatAction)> = vec![(icons::regular::ARROW_BEND_UP_LEFT, "Reply", ChatAction::Reply(line.id))];
    if own {
        buttons.push((icons::regular::PENCIL_SIMPLE, "Edit", ChatAction::Edit(line.id)));
        buttons.push((icons::regular::TRASH, "Delete  (Shift: without asking)", ChatAction::Delete(line.id)));
    }
    let width = buttons.len() as f32 * 28.0 + 8.0;
    let tools_id = egui::Id::new(("chat-tools", line.id));
    let tools_layer = egui::LayerId::new(egui::Order::Foreground, tools_id);
    let tools_rect = egui::Rect::from_min_size(egui::pos2(row.right() - width, row.top() - 6.0), egui::vec2(width, 30.0));
    let visible_row = ui.clip_rect().intersect(row);
    let hovered = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
        (visible_row.contains(p) || tools_rect.contains(p))
            && ui.ctx().layer_id_at(p).is_some_and(|l| l == ui.layer_id() || l == tools_layer)
    });
    if hovered && ui.clip_rect().contains(row.right_top()) {
        egui::Area::new(tools_id)
            .order(egui::Order::Foreground)
            .fixed_pos(tools_rect.min)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(theme::DARK)
                    .corner_radius(6.0)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::same(2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (icon, tip, a) in buttons {
                                let color = if matches!(a, ChatAction::Delete(_)) { theme::RED } else { theme::MUTED };
                                if theme::ghost_button_sized(ui, egui::RichText::new(icon).size(15.0).color(color), egui::vec2(24.0, 22.0))
                                    .on_hover_text(tip)
                                    .clicked()
                                {
                                    action = Some(match a {
                                        ChatAction::Delete(id) => delete(id),
                                        other => other,
                                    });
                                }
                            }
                        });
                    });
            });
    }
    action
}

/// Inline editor: text field + check (save) / x (cancel). Enter saves,
/// Shift+Enter = new line, Esc cancels.
fn inline_editor(ui: &mut egui::Ui, id: u64, edit: EditBox) -> Option<ChatAction> {
    let field_id = egui::Id::new(("edit-message", id));
    let resp = egui::Frame::default()
        .fill(theme::INPUT)
        .corner_radius(6.0)
        .stroke(egui::Stroke::new(1.0_f32, theme::accent()))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(edit.text)
                    .id(field_id)
                    .frame(false)
                    .desired_rows(1)
                    .desired_width(ui.available_width().min(640.0)),
            )
        })
        .inner;
    if std::mem::take(edit.focus) {
        resp.request_focus();
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field_id) {
            let end = egui::text::CCursor::new(edit.text.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), field_id);
        }
    }
    let mut action = None;
    // egui drops the focus on Esc before we look -> also accept "just lost it".
    if (resp.has_focus() || resp.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(ChatAction::CancelEdit);
    }
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
        while edit.text.ends_with('\n') {
            edit.text.pop(); // egui already inserted the newline
        }
        action = Some(ChatAction::SaveEdit);
    }
    ui.horizontal(|ui| {
        let save = ui
            .add(egui::Button::new(egui::RichText::new(format!("{}  Save", icons::regular::CHECK)).color(egui::Color32::WHITE)).fill(theme::GREEN))
            .on_hover_text("Save (Enter)");
        if save.clicked() {
            action = Some(ChatAction::SaveEdit);
        }
        let cancel = ui
            .add(egui::Button::new(egui::RichText::new(format!("{}  Cancel", icons::regular::X)).color(theme::HEADER)))
            .on_hover_text("Cancel (Esc)");
        if cancel.clicked() {
            action = Some(ChatAction::CancelEdit);
        }
        ui.label(egui::RichText::new("Enter = save  -  Esc = cancel  -  Shift+Enter = new line").size(11.0).color(theme::MUTED));
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;



    #[test]
    fn batched_mention_keys_do_not_send_or_cancel_reply() {
        use super::super::servers::tests::{app, login};
        use egui::text::{CCursor, CCursorRange};
        let key = |key| egui::Event::Key {
            key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default(),
        };
        for (events, expected) in [
            (vec![key(egui::Key::ArrowDown), key(egui::Key::Enter)], "@Anton "),
            (vec![key(egui::Key::ArrowDown), key(egui::Key::Escape)], "@an"),
            (vec![key(egui::Key::ArrowDown), key(egui::Key::ArrowUp), key(egui::Key::Tab)], "@Anna "),
            (vec![key(egui::Key::Enter), key(egui::Key::Enter)], "@Anna "),
            (vec![egui::Event::Text("t".into()), key(egui::Key::Enter)], "@Anton "),
            (vec![egui::Event::Paste("t".into()), key(egui::Key::Tab)], "@Anton "),
            (vec![key(egui::Key::Enter), egui::Event::Text("t".into())], "@Anna t"),
            (vec![egui::Event::Text("t".into()), key(egui::Key::ArrowDown), key(egui::Key::Enter)], "@Anton "),
        ] {
            let mut app = app();
            let (session, mut commands) = login(&mut app, "test.invalid:7331", 1);
            app.view_session(session);
            for (id, name) in [(2, "Anna"), (3, "Anton")] {
                app.srv.users.insert(id, UserInfo {
                    id, name: name.into(), channel_id: 1, muted: false,
                    deafened: false, sharing: false, role: Default::default(), streams: vec![],
                });
            }
            app.chat_input = "@an".into();
            app.reply_to = Some(42);
            let sender = app.srv.cmd_tx.as_ref().unwrap().sender();
            let ctx = egui::Context::default();
            let id = composer_id(3);
            let draw = |app: &mut RustTalkApp, events| {
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    events, focused: true, ..Default::default()
                };
                input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(true);
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.composer(ui, 3, &sender));
                });
            };
            draw(&mut app, vec![]);
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(3))));
            state.store(&ctx, id);
            ctx.memory_mut(|m| m.request_focus(id));
            draw(&mut app, vec![]);
            while commands.try_recv().is_some() {}
            draw(&mut app, events);
            draw(&mut app, vec![]); // deferred popup actions use the updated text/cursor
            assert_eq!(app.chat_input, expected);
            assert_eq!(app.reply_to, Some(42));
            assert!(commands.try_recv().is_none(), "completion must not send a chat command");
            assert!(ctx.memory(|m| m.has_focus(id)));
        }
    }

    #[test]
    fn context_menu_stays_with_message_after_preceding_deletion() {
        let ctx = egui::Context::default();
        let mut textures = TextureCache::default();
        let mut markdown = super::super::markdown::Cache::default();
        let mut lines: Vec<_> = (1..=3).map(|id| ChatLine {
            id, kind: ChatKind::Message { from: "me".into(), own: true },
            text: "hello".into(), timestamp: 0, image: None, reply_to: None, edited: false,
        }).collect();
        let mut draw = |lines: &[ChatLine], events| {
            let mut responses = Vec::new();
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))),
                events, ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let c = LineCtx { lines, me: "me", grouped: false, show_ts: true, animate: false, replying: false };
                    for line in lines {
                        let id = ui.id().with(("chat-line", line.id));
                        chat_line(ui, line, &mut textures, &c, None, &mut markdown);
                        responses.push(ctx.read_response(id).unwrap());
                    }
                });
            });
            let text_positions: Vec<_> = output.shapes.iter().filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    (text.galley.job.text == "hello").then(|| text.pos + text.galley.size() / 2.0)
                } else { None }
            }).collect();
            (responses, text_positions)
        };
        draw(&lines, vec![]);
        let (_, positions) = draw(&lines, vec![]);
        let pos = positions[1];
        draw(&lines, vec![egui::Event::PointerMoved(pos)]);
        let button = |pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default(),
        };
        draw(&lines, vec![button(true)]);
        let (responses, _) = draw(&lines, vec![button(false)]);
        assert!(responses[1].context_menu_opened());
        lines.remove(0);
        let (responses, _) = draw(&lines, vec![]);
        assert!(responses[0].context_menu_opened(), "message 2 lost its menu");
        assert!(!responses[1].context_menu_opened(), "menu moved to message 3");
    }

    #[test]
    fn code_copy_receives_click_inside_message_row() {
        let ctx = egui::Context::default();
        let mut textures = TextureCache::default();
        let mut markdown = super::super::markdown::Cache::default();
        let lines = vec![ChatLine {
            id: 7,
            kind: ChatKind::Message { from: "alice".into(), own: false },
            text: "```unknown\nlet n = 42;\n```".into(),
            timestamp: 0,
            image: None,
            reply_to: None,
            edited: false,
        }];
        let mut frame = |events| {
            ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))),
                events,
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let c = LineCtx { lines: &lines, me: "me", grouped: false, show_ts: true, animate: false, replying: false };
                    chat_line(ui, &lines[0], &mut textures, &c, None, &mut markdown);
                });
            })
        };
        frame(vec![]);
        let output = frame(vec![]);
        let pos = output.shapes.iter().find_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                (text.galley.job.text == "Copy").then(|| text.pos + text.galley.size() / 2.0)
            } else {
                None
            }
        }).expect("copy button is visible");
        frame(vec![egui::Event::PointerMoved(pos)]);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![button(true)]);
        let output = frame(vec![button(false)]);
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == "let n = 42;\n")
        }), "row swallowed the copy click");
    }

    #[test]
    fn stage_grid_uses_space_and_keeps_16_9() {
        // One tile in a wide stage: limited by the height, full 16:9.
        let (c, t) = stage_grid(1, egui::vec2(1200.0, 400.0), 12.0);
        assert_eq!(c, 1);
        assert!((t.y - 400.0).abs() < 0.01 && (t.x / t.y - 16.0 / 9.0).abs() < 0.01);
        // Two tiles side by side, not stacked (bigger).
        let (c, t) = stage_grid(2, egui::vec2(1200.0, 400.0), 12.0);
        assert_eq!(c, 2);
        assert!(t.x > 500.0);
        // Four tiles in a square area: 2x2.
        assert_eq!(stage_grid(4, egui::vec2(800.0, 450.0), 12.0).0, 2);
        // Tiles always fit.
        for n in 1..=9 {
            let area = egui::vec2(900.0, 380.0);
            let (c, t) = stage_grid(n, area, 12.0);
            let rows = n.div_ceil(c) as f32;
            assert!(c as f32 * t.x + (c as f32 - 1.0) * 12.0 <= area.x + 0.01, "n={n}");
            assert!(rows * t.y + (rows - 1.0) * 12.0 <= area.y + 0.01, "n={n}");
        }
    }

    #[test]
    fn grid_rects_are_centered() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 500.0));
        let tile = egui::vec2(160.0, 90.0);
        // Two tiles: centered horizontally and vertically.
        let r = grid_rects(2, 2, tile, 10.0, area);
        assert!((r[0].left() - (1000.0 - 330.0) / 2.0).abs() < 0.01);
        assert!((r[1].right() - (1000.0 + 330.0) / 2.0).abs() < 0.01);
        assert!((r[0].center().y - 250.0).abs() < 0.01);
        // Three tiles in two columns: the single tile of the last row is centered.
        let r = grid_rects(3, 2, tile, 10.0, area);
        assert!((r[2].center().x - 500.0).abs() < 0.01);
    }

    /// Runs one egui frame with `events` and draws a single own chat line.
    fn frame(ctx: &egui::Context, lines: &[ChatLine], textures: &mut TextureCache, events: Vec<egui::Event>) -> Option<ChatAction> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0))),
            events,
            ..Default::default()
        };
        let mut action = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let c = LineCtx { lines, me: "me", grouped: false, show_ts: true, animate: false, replying: false };
                action = chat_line(ui, &lines[0], textures, &c, None, &mut super::super::markdown::Cache::default());
            });
        });
        action
    }

    /// Regression: the hover toolbar vanished as soon as the pointer reached
    /// it (its own layer covered the message), so its buttons never got a click.
    #[test]
    fn hover_toolbar_stays_visible_and_buttons_click() {
        let ctx = egui::Context::default();
        let mut textures = TextureCache::default();
        let lines = vec![ChatLine {
            id: 7,
            kind: ChatKind::Message { from: "me".into(), own: true },
            text: "hello".into(),
            timestamp: 0,
            image: None,
            reply_to: None,
            edited: false,
        }];
        let tools = egui::Id::new(("chat-tools", 7u64));
        let moved = |p: egui::Pos2| vec![egui::Event::PointerMoved(p)];

        // Pointer on the message -> toolbar appears.
        for _ in 0..3 {
            frame(&ctx, &lines, &mut textures, moved(egui::pos2(300.0, 40.0)));
        }
        let rect = ctx.memory(|m| m.area_rect(tools)).expect("toolbar shown on hover");

        // Move onto the second button (Edit) and stay there: toolbar must remain.
        let edit = rect.min + egui::vec2(2.0 + 24.0 + ctx.style().spacing.item_spacing.x + 12.0, rect.height() / 2.0);
        for _ in 0..3 {
            frame(&ctx, &lines, &mut textures, moved(edit));
        }
        assert!(ctx.layer_id_at(edit).is_some_and(|l| l.id == tools), "toolbar still under the pointer");

        // Click it.
        let press = |pressed| egui::Event::PointerButton { pos: edit, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut got = frame(&ctx, &lines, &mut textures, vec![press(true)]);
        got = got.or(frame(&ctx, &lines, &mut textures, vec![press(false)]));
        assert_eq!(got, Some(ChatAction::Edit(7)));
    }

    /// Inline editor: Enter saves, Esc cancels (field gets the focus itself).
    #[test]
    fn inline_editor_keys() {
        let ctx = egui::Context::default();
        let run = |text: &mut String, focus: &mut bool, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0))),
                events,
                ..Default::default()
            };
            let mut action = None;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = inline_editor(ui, 1, EditBox { text, focus });
                });
            });
            action
        };
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let (mut text, mut focus) = ("hello".to_string(), true);
        assert_eq!(run(&mut text, &mut focus, vec![]), None);
        assert!(!focus, "focus request consumed");
        run(&mut text, &mut focus, vec![]); // focus is active from now on
        assert_eq!(run(&mut text, &mut focus, vec![key(egui::Key::Enter)]), Some(ChatAction::SaveEdit));
        assert_eq!(text.trim_end(), "hello", "no newline left behind");
        assert_eq!(run(&mut text, &mut focus, vec![key(egui::Key::Escape)]), Some(ChatAction::CancelEdit));
    }

    #[test]
    fn snippets_are_short_and_flat() {
        assert_eq!(snippet("a\n  b", false), "a b");
        assert_eq!(snippet("", true), "[image]");
        assert!(snippet(&"x".repeat(200), false).ends_with("..."));
    }
}
