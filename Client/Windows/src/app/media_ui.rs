//! Video UI: fullscreen view, camera preview dialog, screen/window picker,
//! GIF search window.

use super::theme;
use super::*;

impl RustTalkApp {
    /// Stream im Vollbild zeigen (App-Fenster geht in den Vollbildmodus).
    pub(super) fn enter_fullscreen(&mut self, key: (u64, VideoKind)) {
        self.fullscreen = Some(key);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }

    pub(super) fn leave_fullscreen(&mut self) {
        if self.fullscreen.take().is_some() {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// Vollbild: Stream fuellt das Fenster, unten die Mitglieder des Channels
    /// (gruener Punkt = spricht). Esc / Doppelklick / Button beendet.
    pub(super) fn ui_fullscreen(&mut self, ctx: &egui::Context, key: (u64, VideoKind)) {
        let (uid, kind) = key;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.leave_fullscreen();
            return;
        }
        let streamer = self.srv.users.get(&uid).map(|u| u.name.clone()).unwrap_or_default();
        let members: Vec<UserInfo> = self
            .srv.users
            .get(&uid)
            .and_then(|u| self.srv.channels.get(&u.channel_id))
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.srv.users.get(&id).cloned())
            .collect();
        let talking: HashMap<u64, Instant> = self.talking.lock().unwrap().clone();
        let self_transmitting = self.control.transmitting.load(Ordering::Relaxed);
        let mut switch_to: Option<(u64, VideoKind)> = None;
        let mut leave = false;

        // --- Unten: User-Leiste ---
        egui::TopBottomPanel::bottom("fs_users")
            .frame(
                egui::Frame::default().fill(theme::DARK).inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                if self.screen_audio.is_some() {
                    ui.horizontal(|ui| {
                        ui.label("Sharing computer audio");
                        if ui.button("Stop sharing audio").clicked() {
                            self.stop_shared_audio();
                        }
                    });
                }
                // Centered member row (scrolls only when it does not fit).
                let row_w = members.len() as f32 * 84.0 + members.len().saturating_sub(1) as f32 * ui.spacing().item_spacing.x;
                let pad = ((ui.available_width() - row_w) / 2.0 - ui.spacing().item_spacing.x).max(0.0);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        for user in &members {
                            let talking_now = talking
                                .get(&user.id)
                                .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
                                || (user.id == self.srv.my_id && self_transmitting);
                            ui.allocate_ui(egui::vec2(84.0, 86.0), |ui| {
                                ui.vertical_centered(|ui| {
                                    let r = theme::draw_avatar(ui, &user.name, 40.0, talking_now);
                                    if talking_now {
                                        theme::draw_talk_dot(ui, r);
                                    }
                                    let mut name = egui::RichText::new(&user.name)
                                        .size(12.0)
                                        .color(theme::TEXT);
                                    if user.id == uid {
                                        name = name.strong();
                                    }
                                    ui.label(name);
                                    ui.horizontal(|ui| {
                                        if user.muted {
                                            ui.label(
                                                egui::RichText::new(icons::regular::MICROPHONE_SLASH)
                                                    .color(theme::RED),
                                            );
                                        }
                                        if user.deafened {
                                            ui.label(
                                                egui::RichText::new(icons::regular::SPEAKER_SLASH)
                                                    .color(theme::RED),
                                            );
                                        }
                                        if user.audio_sharing {
                                            let lit = self.shared_audio_lit(user);
                                            let response = ui.small_button(egui::RichText::new(icons::regular::MUSIC_NOTES)
                                                .color(if lit { theme::GREEN } else { theme::MUTED }));
                                            self.shared_audio_popup(ui, user, &response);
                                        }
                                        // Streams dieses Users: Klick wechselt die Ansicht.
                                        // Unwatched streams too: clicking one starts watching it.
                                        for k in [VideoKind::Screen, VideoKind::Camera] {
                                            if !self.streams_of(user).contains(&k) {
                                                continue;
                                            }
                                            let active = (user.id, k) == key;
                                            let btn = egui::Button::new(
                                                egui::RichText::new(kind_icon(k)).color(
                                                    if active { theme::on_accent() } else { theme::MUTED },
                                                ),
                                            )
                                            .small()
                                            .fill(if active { theme::accent() } else { theme::HOVER });
                                            if ui
                                                .add(btn)
                                                .on_hover_text(format!(
                                                    "Watch {}'s {}",
                                                    user.name,
                                                    kind_label(k)
                                                ))
                                                .clicked()
                                            {
                                                switch_to = Some((user.id, k));
                                            }
                                        }
                                    });
                                });
                            });
                        }
                    });
                });
            });

        // --- Mitte: Stream ---
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                if let Some(tex) = self.video_frames.get(&key).and_then(|v| v.tex.clone()) {
                    {
                        let fit = theme::fit_size(tex.size_vec2(), rect.size());
                        ui.painter().image(
                            tex.id(),
                            egui::Rect::from_center_size(rect.center(), fit),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
                let resp = ui.interact(rect, ui.id().with("fs_video"), egui::Sense::click());
                if resp.double_clicked() {
                    leave = true;
                }
                // Presenter layout maximized: the camera stays on top of the screen.
                if let Some(user) = self.presenter_of(key) {
                    let speaking = talking.get(&user.id).is_some_and(|t| t.elapsed() < Duration::from_millis(250))
                        || (user.id == self.srv.my_id && self_transmitting);
                    let mut open = None;
                    // Same corner as on the stage: bottom left of the 16:9 frame.
                    let frame = egui::Rect::from_center_size(rect.center(), theme::fit_size(egui::vec2(16.0, 9.0), rect.size()));
                    self.presenter_overlay(ui, frame, &user, speaking, &mut open);
                    switch_to = open.or(switch_to);
                }
            });

        // --- Overlays: Titel links oben, Beenden rechts oben ---
        let overlay = egui::Frame::default()
            .fill(egui::Color32::from_black_alpha(170))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6));
        egui::Area::new(egui::Id::new("fs_title"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  {} – {}",
                            kind_icon(kind),
                            streamer,
                            kind_label(kind)
                        ))
                        .color(egui::Color32::WHITE),
                    );
                });
            });
        egui::Area::new(egui::Id::new("fs_exit"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .show(ctx, |ui| {
                overlay.show(ui, |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{}  Exit fullscreen (Esc)",
                                    icons::regular::CORNERS_IN
                                ))
                                .color(egui::Color32::WHITE),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        leave = true;
                    }
                });
            });

        if leave {
            self.leave_fullscreen();
        } else if let Some(k) = switch_to {
            self.watch(k.0, k.1);
            self.fullscreen = Some(k);
        }
    }

    /// Kamera-Button: live -> ausschalten, sonst Vorschau-Dialog oeffnen.
    pub(super) fn toggle_cam(&mut self) {
        if self.cam.as_ref().is_some_and(|c| c.is_sending()) {
            self.cam = None;
            self.after_source_stopped(VideoKind::Camera);
            return;
        }
        if !self.in_voice() {
            return;
        }
        if self.cam_dialog.is_some() {
            return;
        }
        let (devices, error) = match video::list_cameras() {
            Ok(d) if d.is_empty() => (d, Some("No camera found.".to_string())),
            Ok(d) => (d, None),
            Err(e) => (Vec::new(), Some(format!("{e:#}"))),
        };
        let selected = devices
            .iter()
            .find(|d| Some(&d.name) == self.config.camera.as_ref())
            .or(devices.first())
            .map(|d| d.index);
        self.cam_dialog = Some(CamDialog { devices, selected, error });
        if let Some(index) = selected {
            self.start_cam_preview(index);
        }
    }

    /// Kamera im Vorschau-Modus starten (sendet noch nichts).
    pub(super) fn start_cam_preview(&mut self, index: u32) {
        // Invalidate callbacks from the previous device before replacing its preview.
        self.video_epoch[epoch_slot(VideoKind::Camera)].fetch_add(1, Ordering::Relaxed);
        self.cam = None; // alten Thread stoppen
        *self.local_preview.lock().unwrap() = None;
        if let Some(d) = &mut self.cam_dialog {
            d.error = None;
        }
        match video::start_camera(
            index,
            self.config.video_quality,
            self.make_frame_sink(VideoKind::Camera),
            self.make_cam_error_sink(),
        ) {
            Ok(cap) => self.cam = Some(cap),
            Err(e) => {
                if let Some(d) = &mut self.cam_dialog {
                    d.error = Some(format!("{e:#}"));
                }
            }
        }
    }

    /// Screen button changes settings; stopping is an explicit action.
    pub(super) fn toggle_screen(&mut self) {
        if !self.in_voice() { return; }
        if self.screen_picker.is_some() {
            self.screen_picker = None;
        } else {
            let at = self.ctx.pointer_hover_pos().unwrap_or(self.ctx.screen_rect().left_bottom() + egui::vec2(16.0, -16.0));
            self.screen_picker = Some(ScreenPicker::open(at, self.config.share_screen_audio));
        }
    }

    pub(super) fn stop_screen_share(&mut self) {
        self.screen_picker = None;
        if self.screen.take().is_some() {
            self.after_source_stopped(VideoKind::Screen);
        }
    }

    /// Ausgewaehlte Quelle teilen (eine laufende Kamera bleibt an).
    /// Shares on the voice server only (nothing without voice).
    pub(super) fn start_screen_share(&mut self, source: ShareSource) {
        let Some(cmd_tx) = self.voice_tx().filter(|_| self.in_voice()) else { return };
        // A previous capture may still deliver frames or its error: a new epoch drops them.
        self.video_epoch[epoch_slot(VideoKind::Screen)].fetch_add(1, Ordering::Relaxed);
        self.screen = None;
        self.screen_audio_failed = false; // explicit capture request permits a retry
        // Erst Sharing melden, dann senden (Server relayed nur Sharer).
        let _ = cmd_tx.send(Command::SetSharing(true));
        match video::start_screen(
            source,
            self.config.video_quality,
            self.make_frame_sink(VideoKind::Screen),
            self.make_screen_error_sink(),
        ) {
            Ok(cap) => self.screen = Some(cap),
            Err(e) => {
                self.set_error(format!("Screen share error: {e:#}"));
                let still = self.cam.as_ref().is_some_and(|c| c.is_sending());
                let _ = cmd_tx.send(Command::SetSharing(still));
            }
        }
    }

    /// Nach dem Stoppen einer Quelle: Zuschauern den einzelnen Stream
    /// abmelden, eigene Kachel weg, Sharing-Status aktualisieren.
    pub(super) fn after_source_stopped(&mut self, kind: VideoKind) {
        // Reihenfolge wichtig: StopVideo vor SetSharing(false), sonst hat der
        // Server keine Relay-Ziele mehr.
        // New generation: frames the capture/encoder threads still deliver
        // after the stop are dropped by the old sink (no frozen last frame).
        self.video_epoch[epoch_slot(kind)].fetch_add(1, Ordering::Relaxed);
        let still = self.screen.is_some() || self.cam.as_ref().is_some_and(|c| c.is_sending());
        if let Some(cmd_tx) = self.voice_tx() {
            let _ = cmd_tx.send(Command::StopVideo(kind));
            let _ = cmd_tx.send(Command::SetSharing(still));
        }
        self.clear_video_of(self.voice_my_id(), Some(kind));
        if kind == VideoKind::Camera {
            *self.local_preview.lock().unwrap() = None;
            self.local_preview_tex = None;
        }
    }

    /// Streams of a user that can be shown: our own (local preview) or what
    /// the server announced for others.
    /// Only on the voice server: elsewhere the ids would name other people.
    pub(super) fn streams_of(&self, user: &UserInfo) -> Vec<VideoKind> {
        if !self.viewing_voice() {
            Vec::new()
        } else if user.id == self.srv.my_id {
            [VideoKind::Screen, VideoKind::Camera].into_iter().filter(|k| self.video_frames.contains_key(&(user.id, *k))).collect()
        } else {
            let mut v = user.streams.clone();
            v.sort_by_key(|k| *k != VideoKind::Screen); // screen first
            v
        }
    }

    /// Start receiving a stream (the server sends it only to watchers).
    pub(super) fn watch(&mut self, from_id: u64, kind: VideoKind) {
        if !self.viewing_voice() || from_id == self.srv.my_id || !self.watching.insert((from_id, kind)) {
            return;
        }
        // A stream we stopped watching a moment ago: let its frames in again.
        self.video_stopped.remove(&(from_id, kind));
        if let Some(tx) = &self.voice_tx() {
            let _ = tx.send(Command::Raw(rusttalk_shared::protocol::ClientMessage::Watch { from_id, kind, watching: true }));
        }
    }

    /// Stop receiving a stream: no more bandwidth for it, the tile goes back
    /// to "Watch".
    pub(super) fn unwatch(&mut self, from_id: u64, kind: VideoKind) {
        if !self.watching.remove(&(from_id, kind)) {
            return;
        }
        if let Some(tx) = &self.voice_tx() {
            let _ = tx.send(Command::Raw(rusttalk_shared::protocol::ClientMessage::Watch { from_id, kind, watching: false }));
        }
        self.clear_video_of(from_id, Some(kind));
        if self.fullscreen == Some((from_id, kind)) {
            self.leave_fullscreen();
        }
    }

    /// Video-Kacheln eines Users entfernen (eine Art oder alle).
    pub(super) fn clear_video_of(&mut self, user_id: u64, kind: Option<VideoKind>) {
        for k in [VideoKind::Camera, VideoKind::Screen] {
            if kind.is_some_and(|want| want != k) {
                continue;
            }
            // Dropping the view ends its decoder thread.
            if self.video_frames.remove(&(user_id, k)).is_some() {
                self.video_stopped.insert((user_id, k), Instant::now());
            }
        }
    }

    /// Frame-Senke fuer Capture-Threads. Kamera-Frames landen zusaetzlich in
    /// der lokalen Vorschau (Kamera-Dialog); nur wenn `sending` -> an den
    /// Server + eigene Kachel. Stoesst sofort ein Neuzeichnen an.
    /// Bound to the voice server's connection at start: frames never go elsewhere.
    pub(super) fn make_frame_sink(&self, kind: VideoKind) -> impl FnMut(video::VideoOut) + Send + 'static {
        let voice = self.voice_owner.and_then(|id| self.session(id));
        let tx = voice.and_then(|s| s.tx());
        let events = voice.map(|s| s.event_tx.clone());
        let preview = self.local_preview.clone();
        let ctx = self.ctx.clone();
        let backlog = voice.map(|s| s.upload_backlog.clone()).unwrap_or_default();
        let my_id = voice.map_or(0, |s| s.my_id);
        let epochs = self.video_epoch.clone();
        let slot = epoch_slot(kind);
        let generation = epochs[slot].load(Ordering::Relaxed);
        move |out: video::VideoOut| {
            if epochs[slot].load(Ordering::Relaxed) != generation {
                return; // source was stopped: late frame from the capture/encoder threads
            }
            if let Some(img) = out.preview {
                *preview.lock().unwrap() = Some(img);
            }
            if let Some(jpeg) = out.frame {
                // Upload staut (langsames WLAN o. ae.) -> Frame nicht senden,
                // statt Latenz und Speicher endlos wachsen zu lassen.
                let len = jpeg.len();
                if let Some(tx) = tx.as_ref().filter(|_| backlog.load(Ordering::Relaxed) + len <= MAX_UPLOAD_BACKLOG) {
                    backlog.fetch_add(len, Ordering::Relaxed);
                    if tx.send(Command::VideoFrame(kind, jpeg.clone())).is_err() {
                        backlog.fetch_sub(len, Ordering::Relaxed);
                    }
                }
                if let Some(events) = &events {
                    // Own tile; a full queue drops it rather than block the capture thread.
                    let _ = events.try_send(NetEvent::Message(ServerMessage::VideoFrame { from_id: my_id, kind, data: jpeg }), len);
                }
            }
            ctx.request_repaint();
        }
    }

    /// Fehler des Bildschirm-Threads: Thread ist beendet -> in `update`
    /// wird das Teilen sauber abgemeldet. Bound to the share it was made for:
    /// an older capture that fails late never stops a newer one.
    pub(super) fn make_screen_error_sink(&self) -> impl FnMut(String) + Send + 'static {
        let events = self.voice_events();
        let failed = self.screen_failed.clone();
        let epochs = self.video_epoch.clone();
        let slot = epoch_slot(VideoKind::Screen);
        let generation = epochs[slot].load(Ordering::Relaxed);
        move |msg: String| {
            let mut pending = failed.lock().unwrap();
            if epochs[slot].load(Ordering::Relaxed) != generation {
                return;
            }
            *pending = Some(generation);
            if let Some(events) = &events {
                let _ = events.try_send(NetEvent::Status(msg), 0);
            }
        }
    }

    /// The current screen share failed (its thread ended with an error).
    pub(super) fn take_screen_failure(&mut self) -> bool {
        let current = self.video_epoch[epoch_slot(VideoKind::Screen)].load(Ordering::Relaxed);
        self.screen_failed.lock().unwrap().take() == Some(current)
    }

    /// Fehler des Kamera-Threads: Statuszeile + Anzeige im Kamera-Dialog.
    pub(super) fn make_cam_error_sink(&self) -> impl FnMut(String) + Send + 'static {
        let events = self.voice_events();
        let errors = self.cam_error.clone();
        let epochs = self.video_epoch.clone();
        let slot = epoch_slot(VideoKind::Camera);
        let generation = epochs[slot].load(Ordering::Relaxed);
        move |msg: String| {
            let mut pending = errors.lock().unwrap();
            if epochs[slot].load(Ordering::Relaxed) != generation {
                return;
            }
            *pending = Some((generation, msg.clone()));
            if let Some(events) = &events {
                let _ = events.try_send(NetEvent::Status(msg), 0);
            }
        }
    }

    /// Eigene Vorschau als Textur (eine Textur, bei jedem neuen Bild aktualisiert).
    pub(super) fn local_preview_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Some(img) = self.local_preview.lock().unwrap().take() {
            match &mut self.local_preview_tex {
                Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                None => self.local_preview_tex = Some(ctx.load_texture("local-preview", img, egui::TextureOptions::LINEAR)),
            }
        }
        self.local_preview_tex.clone()
    }

    /// An incoming H.264 frame: to the decoder thread of its stream (started on first use).
    pub(super) fn feed_video(&mut self, key: (u64, VideoKind), data: Vec<u8>) {
        // Just ended: late frames (still in flight) must not bring the tile
        // back with a frozen last picture. A real restart gets through after
        // the grace period (its decoder then asks for a keyframe).
        if !self.video_frames.contains_key(&key) {
            if let Some(t) = self.video_stopped.get(&key) {
                if t.elapsed() < STOP_GRACE {
                    return;
                }
                self.video_stopped.remove(&key);
            }
        }
        if !self.video_frames.contains_key(&key) {
            self.next_video_view += 1;
            let view = VideoView::spawn(key, self.next_video_view, self.video_tx.clone(), self.ctx.clone());
            self.video_frames.insert(key, view);
        }
        let Some(view) = self.video_frames.get(&key) else { return };
        if let Err(std::sync::mpsc::TrySendError::Full(_)) = view.tx.try_send(data) {
            // Decoder can't keep up: this frame is lost -> the next ones need a keyframe.
            let _ = self.video_tx.send(DecodeMsg::NeedKeyframe(key, view.id));
        }
    }

    /// Decoded pictures -> textures; keyframe needs -> request (max. 1/s per stream).
    pub(super) fn poll_video(&mut self, ctx: &egui::Context) {
        let camera_error = self.cam_error.lock().unwrap().take()
            .filter(|(generation, _)| *generation == self.video_epoch[epoch_slot(VideoKind::Camera)].load(Ordering::Relaxed));
        if let Some((_, error)) = camera_error {
            self.cam = None;
            self.after_source_stopped(VideoKind::Camera);
            if let Some(dialog) = &mut self.cam_dialog {
                dialog.error = Some(error.clone());
            }
            self.set_error(error);
        }
        while let Ok(msg) = self.video_rx.try_recv() {
            // Results of a tile that is gone (another voice server, a restarted
            // stream) name a tile id that no longer exists: dropped.
            match msg {
                DecodeMsg::Picture(key, view, img) => {
                    if let Some(v) = self.video_frames.get_mut(&key).filter(|v| v.id == view) {
                        match &mut v.tex {
                            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                            None => v.tex = Some(ctx.load_texture(format!("video-{}-{:?}", key.0, key.1), img, egui::TextureOptions::LINEAR)),
                        }
                    }
                }
                DecodeMsg::NeedKeyframe(key, view) => {
                    let Some(v) = self.video_frames.get_mut(&key).filter(|v| v.id == view) else { continue };
                    if v.last_request.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                        continue;
                    }
                    v.last_request = Some(Instant::now());
                    if key.0 == self.voice_my_id() {
                        // Own tile: ask our own encoder directly.
                        let src = match key.1 {
                            VideoKind::Camera => self.cam.as_ref(),
                            VideoKind::Screen => self.screen.as_ref(),
                        };
                        if let Some(c) = src {
                            c.request_keyframe();
                        }
                    } else if let Some(tx) = &self.voice_tx() {
                        let _ = tx.send(Command::Raw(rusttalk_shared::protocol::ClientMessage::RequestKeyframe { from_id: key.0, kind: key.1 }));
                    }
                }
            }
        }
    }

    /// Kamera-Dialog: Geraet waehlen, Live-Vorschau, dann "Kamera teilen".
    pub(super) fn ui_camera_dialog(&mut self, ctx: &egui::Context) {
        if self.cam_dialog.is_none() {
            return;
        }
        let Some(cmd_tx) = self.voice_tx().filter(|_| self.in_voice()) else {
            self.cam_dialog = None;
            return;
        };
        let preview = self.local_preview_texture(ctx);
        let mut open = true;
        let mut action: Option<bool> = None; // Some(true) = teilen, Some(false) = abbrechen
        let mut switch_to: Option<u32> = None;
        egui::Window::new(format!("{}  Camera", icons::regular::VIDEO_CAMERA))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([480.0, 360.0])
            .show(ctx, |ui| {
                let Some(dialog) = self.cam_dialog.as_mut() else { return };
                if !dialog.devices.is_empty() {
                    let current = dialog
                        .devices
                        .iter()
                        .find(|d| Some(d.index) == dialog.selected)
                        .map(|d| d.name.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_label("Camera")
                        .selected_text(current)
                        .width(320.0)
                        .show_ui(ui, |ui| {
                            for d in &dialog.devices {
                                if ui
                                    .selectable_label(dialog.selected == Some(d.index), &d.name)
                                    .clicked()
                                    && dialog.selected != Some(d.index)
                                {
                                    dialog.selected = Some(d.index);
                                    switch_to = Some(d.index);
                                }
                            }
                        });
                }
                ui.add_space(8.0);
                let size = egui::vec2(480.0, 270.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 8.0, theme::DARK);
                if let Some(tex) = &preview {
                    let fit = theme::fit_size(tex.size_vec2(), size);
                    let img_rect = egui::Rect::from_center_size(rect.center(), fit);
                    egui::Image::new(tex).corner_radius(8.0).paint_at(ui, img_rect);
                } else {
                    let text = match &dialog.error {
                        Some(e) => e.clone(),
                        None => "Starting camera...".to_string(),
                    };
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(14.0),
                        theme::MUTED,
                    );
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            preview.is_some(),
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{}  Turn On Camera",
                                    icons::regular::VIDEO_CAMERA
                                ))
                                .color(theme::on_accent()),
                            )
                            .fill(theme::accent()),
                        )
                        .clicked()
                    {
                        action = Some(true);
                    }
                    if theme::ghost_button(ui, "Cancel").clicked() {
                        action = Some(false);
                    }
                });
            });
        if let Some(index) = switch_to {
            self.start_cam_preview(index);
        }
        match action {
            Some(true) => {
                if let Some(d) = &self.cam_dialog {
                    self.config.camera = d
                        .devices
                        .iter()
                        .find(|x| Some(x.index) == d.selected)
                        .map(|x| x.name.clone());
                    self.config.save();
                }
                // Bildschirm-Teilen laeuft ggf. parallel weiter.
                if let Some(cam) = &self.cam {
                    // Erst Sharing melden, dann senden (Server relayed nur Sharer).
                    let _ = cmd_tx.send(Command::SetSharing(true));
                    cam.set_sending(true);
                }
                self.cam_dialog = None;
                return;
            }
            Some(false) => open = false,
            None => {}
        }
        if !open {
            // Abbrechen: Vorschau-Kamera aus (eine live laufende bleibt).
            self.cam_dialog = None;
            if !self.cam.as_ref().is_some_and(|c| c.is_sending()) {
                self.cam = None;
                *self.local_preview.lock().unwrap() = None;
                self.local_preview_tex = None;
            }
        }
    }

    /// Bildschirm/Fenster-Auswahl mit Reitern "Anwendungen" / "Gesamter Bildschirm".
    pub(super) fn ui_screen_picker(&mut self, ctx: &egui::Context) {
        if !self.in_voice() {
            self.screen_picker = None;
            return;
        }
        let Some(picker) = &mut self.screen_picker else { return };
        if let Ok(list) = picker.rx.try_recv() {
            picker.loading = false;
            picker.selected = None;
            picker.entries = list
                .into_iter()
                .enumerate()
                .map(|(i, e)| {
                    let tex = e.thumbnail.as_ref().map(|img| {
                        ctx.load_texture(
                            format!("share-thumb-{i}"),
                            egui::ColorImage::from_rgba_unmultiplied(
                                [img.width() as usize, img.height() as usize],
                                img,
                            ),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    (e, tex)
                })
                .collect();
            if !picker.entries.iter().any(|(e, _)| matches!(e.source, ShareSource::Window(_))) {
                picker.tab = ShareTab::Screens;
            }
        }

        let mut cancel = false;
        let mut stop = false;
        let active = self.screen.is_some();
        let mut share: Option<ShareSource> = None;
        let mut refresh = false;
        let dismiss = theme::share_popup(ctx, "screen_share_popup", picker.at, 460.0,
            &format!("{}  Share screen", icons::regular::SCREENCAST), |ui| {
                if cfg!(target_os = "linux") {
                    ui.label("Choose a window or screen in the system permission picker.");
                    if active { ui.weak("Choosing a new source restarts screen capture."); }
                    picker.selected = Some(ShareSource::Monitor(0));
                } else {
                // Reiter
                ui.horizontal(|ui| {
                    let tab_w = (ui.available_width() - 8.0) / 2.0;
                    for (tab, icon, label) in [
                        (ShareTab::Apps, icons::regular::APP_WINDOW, "Applications"),
                        (ShareTab::Screens, icons::regular::MONITOR, "Entire Screen"),
                    ] {
                        let active = picker.tab == tab;
                        let btn = egui::Button::new(
                            egui::RichText::new(format!("{icon}  {label}"))
                                .size(15.0)
                                .color(if active { egui::Color32::WHITE } else { theme::MUTED }),
                        )
                        .fill(if active { theme::HOVER } else { theme::DARK })
                        .min_size(egui::vec2(tab_w, 34.0));
                        if ui.add(btn).clicked() && !active {
                            picker.tab = tab;
                            picker.selected = None;
                        }
                    }
                });
                ui.add_space(8.0);

                let list_h = 250.0;
                if picker.loading {
                    ui.allocate_ui(egui::vec2(ui.available_width(), list_h), |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Looking for windows and screens...");
                            });
                        });
                    });
                } else {
                    let wanted_window = picker.tab == ShareTab::Apps;
                    egui::ScrollArea::vertical()
                        .max_height(list_h)
                        .min_scrolled_height(list_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let items: Vec<usize> = picker
                                .entries
                                .iter()
                                .enumerate()
                                .filter(|(_, (e, _))| {
                                    matches!(e.source, ShareSource::Window(_)) == wanted_window
                                })
                                .map(|(i, _)| i)
                                .collect();
                            if items.is_empty() {
                                ui.weak(if wanted_window {
                                    "No shareable windows found."
                                } else {
                                    "No screens found."
                                });
                            }
                            let card_w = (ui.available_width() - 12.0) / 2.0;
                            for row in items.chunks(2) {
                                ui.horizontal(|ui| {
                                    for &i in row {
                                        let (entry, tex) = &picker.entries[i];
                                        let selected = picker.selected == Some(entry.source);
                                        let resp =
                                            share_card(ui, entry, tex.as_ref(), selected, card_w);
                                        if resp.clicked() {
                                            picker.selected = Some(entry.source);
                                        }
                                        if resp.double_clicked() {
                                            share = Some(entry.source);
                                        }
                                    }
                                });
                                ui.add_space(10.0);
                            }
                        });
                }

                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if !cfg!(target_os = "linux") && ui
                        .add_enabled(
                            !picker.loading,
                            egui::Button::new(format!(
                                "{}  Refresh",
                                icons::regular::ARROWS_CLOCKWISE
                            )),
                        )
                        .clicked()
                    {
                        refresh = true;
                    }
                    ui.checkbox(&mut picker.include_audio, format!("{}  Share audio", icons::regular::MUSIC_NOTES))
                        .on_hover_text("Also send what your computer plays (RustTalk voices are filtered out)");
                });
                ui.horizontal(|ui| {
                    if active && theme::ghost_button(ui, egui::RichText::new("Stop sharing").color(theme::RED)).clicked() {
                        stop = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                picker.selected.is_some(),
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}  {}",
                                        icons::regular::BROADCAST,
                                        if cfg!(target_os = "linux") { "Choose source" } else if active { "Apply" } else { "Go Live" }
                                    ))
                                    .color(theme::on_accent()),
                                )
                                .fill(theme::accent()),
                            )
                            .clicked()
                        {
                            share = picker.selected;
                        }
                        if theme::ghost_button(ui, "Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        if refresh {
            let tab = picker.tab;
            let mut fresh = ScreenPicker::open(picker.at, picker.include_audio);
            fresh.tab = tab;
            *picker = fresh;
        }
        if let Some(source) = share {
            self.config.share_screen_audio = picker.include_audio;
            self.config.save();
            self.screen_picker = None;
            self.start_screen_share(source);
        } else if stop {
            self.stop_screen_share();
        } else if dismiss || cancel {
            self.screen_picker = None;
        }
    }

    pub(super) fn open_gif_picker(&mut self) {
        self.giphy.open = true;
        self.giphy.focus = true;
        if self.giphy.shown_query.is_none() {
            self.gif_request(String::new(), 0); // trending
        }
    }

    /// Closes the picker and frees the preview images. Previews still
    /// downloading belong to the old picker and are dropped when they arrive.
    pub(super) fn close_gif_picker(&mut self) {
        for bytes in self.giphy.previews.values() {
            self.textures.remove_bytes(bytes);
        }
        self.giphy = GifPicker { epoch: self.giphy.epoch + 1, ..Default::default() };
    }

    /// Asks the server for a result page (`offset` 0 = new search).
    fn gif_request(&mut self, query: String, offset: u32) {
        if !self.srv.gif_available {
            return;
        }
        let Some(tx) = self.srv.cmd_tx.as_ref() else { return };
        if offset == 0 {
            self.giphy.results.clear();
            self.giphy.end = false;
            self.giphy.error = None;
        }
        self.giphy.shown_query = Some(query.clone());
        let sent = tx.send(Command::GifSearch { query, offset }).is_ok();
        self.giphy.loading = sent;
        if !sent {
            self.giphy.error = Some("Not sent: the connection is busy. Try again in a moment.".into());
        }
    }

    /// A result page from the server (stale pages are ignored).
    pub(super) fn on_gif_results(&mut self, query: String, offset: u32, results: Vec<GifInfo>, error: Option<String>) {
        let p = &mut self.giphy;
        if p.shown_query.as_deref() != Some(query.as_str()) || offset as usize != p.results.len() {
            return;
        }
        p.loading = false;
        p.error = error;
        p.end = results.len() < GIF_PAGE as usize;
        p.results.extend(results);
    }

    /// GIF picker popup above the GIF button.
    pub(super) fn ui_gif_picker(&mut self, ctx: &egui::Context) {
        // Background downloads (also after closing: a chosen GIF may still arrive).
        while let Ok(msg) = self.giphy_rx.try_recv() {
            self.on_giphy_msg(msg);
        }
        if !self.giphy.open {
            return;
        }
        if !self.is_connected() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_gif_picker();
            return;
        }
        // Search while typing (debounced).
        let typed = self.giphy.query.trim().chars().take(rusttalk_shared::protocol::MAX_GIF_QUERY_LEN).collect::<String>();
        if let Some(at) = self.giphy.typed_at {
            if at.elapsed() >= Duration::from_millis(350) {
                self.giphy.typed_at = None;
                if self.giphy.shown_query.as_deref() != Some(typed.as_str()) {
                    self.gif_request(typed, 0);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        const W: f32 = 430.0;
        const H: f32 = 470.0;
        let screen = ctx.screen_rect();
        let anchor = self.gif_button_rect.unwrap_or_else(|| egui::Rect::from_min_size(screen.right_bottom() - egui::vec2(60.0, 70.0), egui::vec2(24.0, 24.0)));
        let pos = egui::pos2(
            (anchor.right() - W + 8.0).clamp(screen.left() + 8.0, (screen.right() - W - 8.0).max(screen.left() + 8.0)),
            (anchor.top() - H - 10.0).max(screen.top() + 8.0),
        );
        let animate = self.config.animate_gifs;
        let mut choose: Option<GifInfo> = None;
        let mut want_more = false;
        let mut open_server_settings = false;
        let area = egui::Area::new(egui::Id::new("gif_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::SIDEBAR)
                    .corner_radius(10.0)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(120) })
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(W - 20.0);
                        ui.set_height(H - 20.0);
                        if !self.srv.gif_available {
                            ui.vertical_centered(|ui| {
                                ui.add_space(110.0);
                                ui.label(egui::RichText::new(icons::regular::GIF).size(46.0).color(theme::MUTED));
                                ui.label(egui::RichText::new("GIFs are not set up on this server").size(16.0).strong().color(theme::HEADER));
                                ui.label(
                                    egui::RichText::new("Someone has to add a GIPHY API key once under\nServer Settings > Integrations - then everyone can use GIFs.")
                                        .color(theme::MUTED),
                                );
                                ui.add_space(8.0);
                                if theme::primary_button(ui, &format!("{}  Open Server Settings", icons::regular::GEAR_SIX)).clicked() {
                                    open_server_settings = true;
                                }
                            });
                            return;
                        }
                        // Search field.
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.giphy.query)
                                .hint_text(format!("{}  Search GIPHY", icons::regular::MAGNIFYING_GLASS))
                                .desired_width(f32::INFINITY),
                        );
                        if std::mem::take(&mut self.giphy.focus) {
                            resp.request_focus();
                        }
                        if resp.changed() {
                            self.giphy.typed_at = Some(Instant::now());
                        }
                        ui.horizontal(|ui| {
                            let title = match self.giphy.shown_query.as_deref() {
                                Some("") | None => "Trending".to_string(),
                                Some(q) => format!("Results for \"{q}\""),
                            };
                            ui.label(egui::RichText::new(title).strong().color(theme::HEADER));
                            if self.giphy.loading || self.giphy.sending {
                                ui.spinner();
                            }
                            if self.giphy.sending {
                                ui.label(egui::RichText::new("Sending...").color(theme::MUTED));
                            }
                        });
                        if let Some(e) = &self.giphy.error {
                            ui.label(egui::RichText::new(e).color(theme::RED));
                        }
                        let scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(H - 110.0).show(ui, |ui| {
                            let width = ui.available_width();
                            let sizes: Vec<(u32, u32)> = self.giphy.results.iter().map(|g| (g.preview_width, g.preview_height)).collect();
                            let (rects, total) = masonry(&sizes, width, 6.0);
                            let (outer, _) = ui.allocate_exact_size(egui::vec2(width, total.max(1.0)), egui::Sense::hover());
                            let results = self.giphy.results.clone();
                            for (i, (g, r)) in results.iter().zip(rects).enumerate() {
                                let rect = r.translate(outer.min.to_vec2());
                                if !ui.is_rect_visible(rect) {
                                    continue; // only visible previews are downloaded
                                }
                                let resp = ui.interact(rect, egui::Id::new(("gif", i, &g.id)), egui::Sense::click());
                                let hovered = resp.hovered();
                                let tex = self.giphy.previews.get(&g.preview_url).and_then(|bytes| {
                                    if hovered && animate {
                                        self.textures.image_for(ctx, bytes, &g.id, true)
                                    } else {
                                        self.textures.texture_for(ctx, bytes, &g.id)
                                    }
                                });
                                match tex {
                                    Some(tex) => {
                                        egui::Image::new(&tex).corner_radius(6.0).paint_at(ui, rect);
                                    }
                                    None => {
                                        ui.painter().rect_filled(rect, 6.0, theme::HOVER);
                                        if self.giphy.requested.insert(g.preview_url.clone()) {
                                            let tx = self.giphy_tx.clone();
                                            let url = g.preview_url.clone();
                                            let repaint = ctx.clone();
                                            let epoch = self.giphy.epoch;
                                            std::thread::spawn(move || {
                                                if let Ok(bytes) = media::download(&url) {
                                                    let _ = tx.send(GiphyMsg::Preview { epoch, url, bytes });
                                                    repaint.request_repaint();
                                                }
                                            });
                                        }
                                    }
                                }
                                if hovered {
                                    ui.painter().rect_stroke(rect, 6.0, egui::Stroke::new(2.0_f32, theme::accent()), egui::StrokeKind::Inside);
                                }
                                let resp = if g.title.is_empty() { resp } else { resp.on_hover_text(&g.title) };
                                if resp.clicked() && !self.giphy.sending {
                                    choose = Some(g.clone());
                                }
                            }
                            if self.giphy.results.is_empty() && !self.giphy.loading && self.giphy.error.is_none() {
                                ui.label(egui::RichText::new("No GIFs found.").color(theme::MUTED));
                            }
                        });
                        // Near the end: load the next page.
                        let s = &scroll;
                        if s.state.offset.y + s.inner_rect.height() >= s.content_size.y - 250.0 {
                            want_more = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("Powered by GIPHY").size(11.0).color(theme::DISABLED));
                        });
                    });
            });

        if open_server_settings {
            self.close_gif_picker();
            self.open_server_settings(super::server_settings::ServerTab::Integrations);
            return;
        }
        if let Some(g) = choose {
            let Some(target) = self.new_gif_target() else { return };
            self.gif_target = Some(target);
            self.giphy.sending = true;
            self.giphy.error = None;
            let tx = self.giphy_tx.clone();
            let repaint = ctx.clone();
            let request = target.request;
            std::thread::spawn(move || {
                let msg = match media::download(&g.url) {
                    Ok(bytes) => GiphyMsg::Full { request, bytes },
                    Err(e) => GiphyMsg::FullFailed { request, error: format!("{e:#}") },
                };
                let _ = tx.send(msg);
                repaint.request_repaint();
            });
        }
        let p = &self.giphy;
        if want_more && !p.loading && !p.end && p.error.is_none() && !p.results.is_empty() {
            let q = p.shown_query.clone().unwrap_or_default();
            let offset = p.results.len() as u32;
            self.gif_request(q, offset);
        }
        // Click outside the popup (and not on the GIF button) closes it.
        let popup = area.response.rect;
        let clicked_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| !popup.contains(p) && !self.gif_button_rect.is_some_and(|b| b.contains(p)))
        });
        if clicked_outside && !self.giphy.sending {
            self.close_gif_picker();
        }
    }

}

impl RustTalkApp {
    /// The GIF about to download goes to the viewed server's open channel,
    /// on this connection only.
    pub(super) fn new_gif_target(&mut self) -> Option<GifTarget> {
        let channel = self.srv.selected_channel?;
        self.next_gif_request += 1;
        Some(GifTarget {
            request: self.next_gif_request,
            session: self.srv.id,
            generation: self.srv.generation,
            channel,
            reply_to: self.reply_to.take(),
        })
    }

    /// A finished download. Only the request still pending may change the
    /// picker or send: a late one (other server, closed picker) is dropped.
    pub(super) fn on_giphy_msg(&mut self, msg: GiphyMsg) {
        match msg {
            GiphyMsg::Preview { epoch, url, bytes } => {
                if self.giphy.open && epoch == self.giphy.epoch {
                    self.giphy.previews.insert(url, bytes);
                }
            }
            GiphyMsg::Full { request, bytes } => {
                let Some(t) = self.gif_target.filter(|t| t.request == request) else { return };
                self.gif_target = None;
                self.giphy.sending = false;
                let tx = self
                    .session(t.session)
                    .filter(|s| s.connected && s.generation == t.generation && s.channels.contains_key(&t.channel))
                    .and_then(|s| s.tx());
                let cmd = Command::SendChat { channel_id: t.channel, text: String::new(), image: Some(bytes), reply_to: t.reply_to };
                match tx.map(|tx| tx.send(cmd)) {
                    Some(Ok(())) => self.close_gif_picker(),
                    Some(Err(_)) => self.giphy.error = Some("Not sent: the connection is busy. Try again in a moment.".into()),
                    None => {
                        self.close_gif_picker();
                        self.set_error("GIF not sent: that server or channel is gone");
                    }
                }
            }
            GiphyMsg::FullFailed { request, error } => {
                if self.gif_target.is_some_and(|t| t.request == request) {
                    self.gif_target = None;
                    self.giphy.sending = false;
                    self.giphy.error = Some(error);
                }
            }
        }
    }
}

/// GIFs per page (must match the server's page size).
const GIF_PAGE: u32 = 24;

/// Two-column masonry layout: rects (relative to the grid origin) for items
/// of the given sizes, each placed into the shorter column, plus total height.
pub(super) fn masonry(sizes: &[(u32, u32)], width: f32, gap: f32) -> (Vec<egui::Rect>, f32) {
    let col_w = ((width - gap) / 2.0).max(1.0);
    let mut heights = [0.0f32; 2];
    let mut rects = Vec::with_capacity(sizes.len());
    for &(w, h) in sizes {
        let col = if heights[0] <= heights[1] { 0 } else { 1 };
        let item_h = (col_w * h.max(1) as f32 / w.max(1) as f32).clamp(40.0, 360.0);
        let min = egui::pos2(col as f32 * (col_w + gap), heights[col]);
        rects.push(egui::Rect::from_min_size(min, egui::vec2(col_w, item_h)));
        heights[col] += item_h + gap;
    }
    (rects, (heights[0].max(heights[1]) - gap).max(0.0))
}

#[cfg(test)]
mod gif_tests {
    use super::*;

    #[test]
    fn masonry_fills_the_shorter_column() {
        // Tall, wide, wide: the two wide ones go to the right column.
        let (r, total) = masonry(&[(100, 200), (200, 100), (200, 100)], 206.0, 6.0);
        assert_eq!(r[0].left(), 0.0);
        assert_eq!(r[1].left(), 106.0, "second column");
        assert_eq!(r[2].left(), 106.0, "right column is still shorter");
        assert_eq!(r[2].top(), r[1].bottom() + 6.0);
        assert!((r[0].height() - 200.0).abs() < 0.01, "aspect ratio kept");
        assert!((total - 200.0).abs() < 0.01);
        assert!(r.iter().all(|x| x.width() == 100.0));
        let (none, h) = masonry(&[], 300.0, 6.0);
        assert!(none.is_empty() && h == 0.0);
    }
}

/// Karte im Bildschirm-Auswahldialog: Vorschaubild + Titel (+ App-Name).
fn share_card(
    ui: &mut egui::Ui,
    entry: &ShareEntry,
    tex: Option<&egui::TextureHandle>,
    selected: bool,
    width: f32,
) -> egui::Response {
    let thumb_h = width * 9.0 / 16.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, thumb_h + 44.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let bg = if selected || resp.hovered() { theme::HOVER } else { theme::DARK };
    painter.rect_filled(rect, 8.0, bg);
    let thumb_rect = egui::Rect::from_min_size(rect.min, egui::vec2(width, thumb_h)).shrink(6.0);
    painter.rect_filled(thumb_rect, 6.0, egui::Color32::BLACK);
    match tex {
        Some(tex) => {
            let fit = theme::fit_size(tex.size_vec2(), thumb_rect.size());
            let r = egui::Rect::from_center_size(thumb_rect.center(), fit);
            painter.image(
                tex.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                "No preview",
                egui::FontId::proportional(13.0),
                theme::MUTED,
            );
        }
    }
    let text_y = thumb_rect.bottom() + 8.0;
    let max_chars = ((width - 16.0) / 7.5) as usize;
    let shorten = |t: &str| -> String {
        if t.chars().count() > max_chars {
            let mut s: String = t.chars().take(max_chars.saturating_sub(1)).collect();
            s.push('…');
            s
        } else {
            t.to_string()
        }
    };
    painter.text(
        egui::pos2(rect.left() + 8.0, text_y),
        egui::Align2::LEFT_TOP,
        shorten(&entry.title),
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    if !entry.subtitle.is_empty() {
        painter.text(
            egui::pos2(rect.left() + 8.0, text_y + 17.0),
            egui::Align2::LEFT_TOP,
            shorten(&entry.subtitle),
            egui::FontId::proportional(12.0),
            theme::MUTED,
        );
    }
    if selected {
        painter.rect_stroke(
            rect.shrink(1.0),
            8.0,
            egui::Stroke::new(2.0_f32, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    resp.on_hover_text(&entry.title)
}

/// How long a just-ended stream ignores late frames.
const STOP_GRACE: Duration = Duration::from_millis(2000);

/// Index of a source in `video_epoch`.
fn epoch_slot(kind: VideoKind) -> usize {
    match kind {
        VideoKind::Camera => 0,
        VideoKind::Screen => 1,
    }
}

pub(super) fn kind_icon(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => icons::regular::VIDEO_CAMERA,
        VideoKind::Screen => icons::regular::MONITOR,
    }
}

pub(super) fn kind_label(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => "Camera",
        VideoKind::Screen => "Screen",
    }
}

/// One incoming video stream: its decoder thread and the latest picture.
pub(super) struct VideoView {
    /// Unique per tile, also across voice servers and restarted streams.
    pub(super) id: u64,
    pub(super) tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    pub(super) tex: Option<egui::TextureHandle>,
    pub(super) last_request: Option<Instant>,
    /// Cut the person out of this camera's pictures (Presenter, "Remove background").
    pub(super) cutout: Arc<AtomicBool>,
}

/// From decoder threads to the UI, naming the tile (`VideoView::id`) they decode for.
pub(super) enum DecodeMsg {
    Picture((u64, VideoKind), u64, egui::ColorImage),
    NeedKeyframe((u64, VideoKind), u64),
}

impl VideoView {
    /// Starts the decoder thread (ends when the view and its sender are dropped).
    pub(super) fn spawn(key: (u64, VideoKind), id: u64, out: std::sync::mpsc::Sender<DecodeMsg>, ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        let cutout = Arc::new(AtomicBool::new(false));
        let want_cutout = cutout.clone();
        let _ = std::thread::Builder::new().name(format!("video-dec-{}", key.0)).spawn(move || {
            let mut dec = match crate::video_codec::H264Decoder::new() {
                Ok(d) => d,
                Err(e) => return tracing::warn!("{e:#}"),
            };
            // Model thread while background removal is wanted, ended when not.
            let mut cutout: Option<crate::segment::Cutout> = None;
            for frame in rx {
                match dec.decode(&frame) {
                    Ok(crate::video_codec::Decoded::Picture(mut img)) => {
                        dec.got_picture();
                        if want_cutout.load(Ordering::Relaxed) {
                            cutout.get_or_insert_with(crate::segment::Cutout::start).apply(&mut img);
                        } else {
                            cutout = None;
                        }
                        if out.send(DecodeMsg::Picture(key, id, img)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    Ok(crate::video_codec::Decoded::Nothing) => {}
                    Err(e) => tracing::debug!("{e:#}"),
                }
                if dec.need_keyframe && out.send(DecodeMsg::NeedKeyframe(key, id)).is_err() {
                    break;
                }
            }
        });
        Self { id, tx, tex: None, last_request: None, cutout }
    }
}

#[cfg(test)]
mod fullscreen_audio_tests {
    use super::*;
    use super::super::mixer::tests::{text_pos, voice_app};

    fn frame(ctx: &egui::Context, app: &mut RustTalkApp, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
            events, ..Default::default()
        }, |ctx| app.ui_fullscreen(ctx, (1, VideoKind::Screen)))
    }

    #[test]
    fn fullscreen_audio_popup_does_not_follow_a_replaced_member() {
        let ctx = egui::Context::default();
        let mut app = voice_app(&ctx);
        for (id, name) in [(2, "Alice"), (3, "Bob")] {
            let mut user = app.srv.users[&1].clone();
            user.id = id;
            user.name = name.into();
            user.audio_sharing = true;
            app.srv.users.insert(id, user);
        }
        app.srv.channels.get_mut(&5).unwrap().user_ids = vec![1, 2, 3];
        frame(&ctx, &mut app, vec![]);
        let out = frame(&ctx, &mut app, vec![]);
        let note = text_pos(&out, icons::regular::MUSIC_NOTES).expect("fullscreen audio badge");
        for pressed in [true, false] {
            frame(&ctx, &mut app, vec![egui::Event::PointerMoved(note), egui::Event::PointerButton {
                pos: note, button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
            }]);
        }
        let out = frame(&ctx, &mut app, vec![]);
        assert!(text_pos(&out, "Alice's shared audio").is_some());
        app.srv.users.remove(&2);
        app.srv.channels.get_mut(&5).unwrap().user_ids = vec![1, 3];
        for _ in 0..2 {
            let out = frame(&ctx, &mut app, vec![]);
            assert!(text_pos(&out, "Bob's shared audio").is_none(), "popup must not silently change person");
        }
    }
}

#[cfg(test)]
mod share_popup_tests {
    use super::*;
    use super::super::mixer::tests::voice_app;

    #[test]
    fn screen_settings_and_dismissal_keep_capture_and_audio_choice_unchanged() {
        let ctx = egui::Context::default();
        let mut app = voice_app(&ctx);
        let stopped = Arc::new(AtomicBool::new(false));
        app.screen = Some(VideoCapture {
            stop: stopped.clone(),
            sending: Arc::new(AtomicBool::new(true)),
            keyframe: Arc::new(AtomicBool::new(false)),
        });
        let original_audio = app.config.share_screen_audio;
        app.toggle_screen();
        assert!(app.screen_picker.is_some() && app.screen.is_some());
        assert!(!stopped.load(Ordering::Relaxed));
        app.screen_picker.as_mut().unwrap().include_audio = !original_audio;
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))),
            events: vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }],
            ..Default::default()
        }, |ctx| app.ui_screen_picker(ctx));
        assert!(app.screen_picker.is_none() && app.screen.is_some());
        assert_eq!(app.config.share_screen_audio, original_audio);
        assert!(!stopped.load(Ordering::Relaxed));
        app.toggle_screen();
        app.stop_screen_share();
        assert!(app.screen_picker.is_none() && app.screen.is_none());
        assert!(stopped.load(Ordering::Relaxed), "quick stop still stops capture");
    }
}
