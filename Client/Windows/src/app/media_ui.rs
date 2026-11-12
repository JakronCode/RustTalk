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
        let streamer = self.users.get(&uid).map(|u| u.name.clone()).unwrap_or_default();
        let members: Vec<UserInfo> = self
            .users
            .get(&uid)
            .and_then(|u| self.channels.get(&u.channel_id))
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.users.get(&id).cloned())
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
                                || (user.id == self.my_id && self_transmitting);
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
                                        // Streams dieses Users: Klick wechselt die Ansicht.
                                        // Unwatched streams too: clicking one starts watching it.
                                        for k in [VideoKind::Screen, VideoKind::Camera] {
                                            if !self.streams_of(user).contains(&k) {
                                                continue;
                                            }
                                            let active = (user.id, k) == key;
                                            let btn = egui::Button::new(
                                                egui::RichText::new(kind_icon(k)).color(
                                                    if active { egui::Color32::WHITE } else { theme::MUTED },
                                                ),
                                            )
                                            .small()
                                            .fill(if active { theme::ACCENT } else { theme::HOVER });
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
    pub(super) fn toggle_cam(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        if self.cam.as_ref().is_some_and(|c| c.is_sending()) {
            self.cam = None;
            self.after_source_stopped(VideoKind::Camera, cmd_tx);
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
            self.start_cam_preview(index, cmd_tx);
        }
    }

    /// Kamera im Vorschau-Modus starten (sendet noch nichts).
    pub(super) fn start_cam_preview(
        &mut self,
        index: u32,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
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
            self.make_frame_sink(VideoKind::Camera, cmd_tx),
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

    /// Bildschirm-Button: teilt -> beenden, sonst Auswahl-Dialog oeffnen.
    pub(super) fn toggle_screen(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        if self.screen.is_some() {
            self.screen = None;
            self.after_source_stopped(VideoKind::Screen, cmd_tx);
            return;
        }
        #[cfg(target_os = "linux")]
        {
            self.start_screen_share(ShareSource::Monitor(0), cmd_tx);
        }
        #[cfg(not(target_os = "linux"))]
        if self.screen_picker.is_none() {
            self.screen_picker = Some(ScreenPicker::open());
        }
    }

    /// Ausgewaehlte Quelle teilen (eine laufende Kamera bleibt an).
    pub(super) fn start_screen_share(
        &mut self,
        source: ShareSource,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        self.screen = None;
        // Erst Sharing melden, dann senden (Server relayed nur Sharer).
        let _ = cmd_tx.send(Command::SetSharing(true));
        match video::start_screen(
            source,
            self.config.video_quality,
            self.make_frame_sink(VideoKind::Screen, cmd_tx),
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
    pub(super) fn after_source_stopped(
        &mut self,
        kind: VideoKind,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        // Reihenfolge wichtig: StopVideo vor SetSharing(false), sonst hat der
        // Server keine Relay-Ziele mehr.
        // New generation: frames the capture/encoder threads still deliver
        // after the stop are dropped by the old sink (no frozen last frame).
        self.video_epoch[epoch_slot(kind)].fetch_add(1, Ordering::Relaxed);
        let _ = cmd_tx.send(Command::StopVideo(kind));
        let still = self.screen.is_some() || self.cam.as_ref().is_some_and(|c| c.is_sending());
        let _ = cmd_tx.send(Command::SetSharing(still));
        self.clear_video_of(self.my_id, Some(kind));
        if kind == VideoKind::Camera {
            *self.local_preview.lock().unwrap() = None;
            self.local_preview_tex = None;
        }
    }

    /// Streams of a user that can be shown: our own (local preview) or what
    /// the server announced for others.
    pub(super) fn streams_of(&self, user: &UserInfo) -> Vec<VideoKind> {
        if user.id == self.my_id {
            [VideoKind::Screen, VideoKind::Camera].into_iter().filter(|k| self.video_frames.contains_key(&(user.id, *k))).collect()
        } else {
            let mut v = user.streams.clone();
            v.sort_by_key(|k| *k != VideoKind::Screen); // screen first
            v
        }
    }

    /// Start receiving a stream (the server sends it only to watchers).
    pub(super) fn watch(&mut self, from_id: u64, kind: VideoKind) {
        if from_id == self.my_id || !self.watching.insert((from_id, kind)) {
            return;
        }
        // A stream we stopped watching a moment ago: let its frames in again.
        self.video_stopped.remove(&(from_id, kind));
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Raw(rusttalk_shared::protocol::ClientMessage::Watch { from_id, kind, watching: true }));
        }
    }

    /// Stop receiving a stream: no more bandwidth for it, the tile goes back
    /// to "Watch".
    pub(super) fn unwatch(&mut self, from_id: u64, kind: VideoKind) {
        if !self.watching.remove(&(from_id, kind)) {
            return;
        }
        if let Some(tx) = &self.cmd_tx {
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
    pub(super) fn make_frame_sink(
        &self,
        kind: VideoKind,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) -> impl FnMut(video::VideoOut) + Send + 'static {
        let tx = cmd_tx.clone();
        let events = self.event_tx.clone();
        let preview = self.local_preview.clone();
        let ctx = self.ctx.clone();
        let backlog = self.upload_backlog.clone();
        let my_id = self.my_id;
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
                if backlog.load(Ordering::Relaxed) + len <= MAX_UPLOAD_BACKLOG {
                    backlog.fetch_add(len, Ordering::Relaxed);
                    if tx.send(Command::VideoFrame(kind, jpeg.clone())).is_err() {
                        backlog.fetch_sub(len, Ordering::Relaxed);
                    }
                }
                let _ = events.send(NetEvent::Message(ServerMessage::VideoFrame {
                    from_id: my_id,
                    kind,
                    data: jpeg,
                }));
            }
            ctx.request_repaint();
        }
    }

    /// Fehler des Bildschirm-Threads: Thread ist beendet -> in `update`
    /// wird das Teilen sauber abgemeldet.
    pub(super) fn make_screen_error_sink(&self) -> impl FnMut(String) + Send + 'static {
        let events = self.event_tx.clone();
        let failed = self.screen_failed.clone();
        move |msg: String| {
            failed.store(true, Ordering::Relaxed);
            let _ = events.send(NetEvent::Status(msg));
        }
    }

    /// Fehler des Kamera-Threads: Statuszeile + Anzeige im Kamera-Dialog.
    pub(super) fn make_cam_error_sink(&self) -> impl FnMut(String) + Send + 'static {
        let events = self.event_tx.clone();
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
            let _ = events.send(NetEvent::Status(msg));
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
        let view = self.video_frames.entry(key).or_insert_with(|| VideoView::spawn(key, self.video_tx.clone(), self.ctx.clone()));
        if let Err(std::sync::mpsc::TrySendError::Full(_)) = view.tx.try_send(data) {
            // Decoder can't keep up: this frame is lost -> the next ones need a keyframe.
            let _ = self.video_tx.send(DecodeMsg::NeedKeyframe(key));
        }
    }

    /// Decoded pictures -> textures; keyframe needs -> request (max. 1/s per stream).
    pub(super) fn poll_video(&mut self, ctx: &egui::Context) {
        let camera_error = self.cam_error.lock().unwrap().take()
            .filter(|(generation, _)| *generation == self.video_epoch[epoch_slot(VideoKind::Camera)].load(Ordering::Relaxed));
        if let Some((_, error)) = camera_error {
            self.cam = None;
            if let Some(tx) = self.cmd_tx.clone() {
                self.after_source_stopped(VideoKind::Camera, &tx);
            }
            if let Some(dialog) = &mut self.cam_dialog {
                dialog.error = Some(error.clone());
            }
            self.set_error(error);
        }
        while let Ok(msg) = self.video_rx.try_recv() {
            match msg {
                DecodeMsg::Picture(key, img) => {
                    if let Some(v) = self.video_frames.get_mut(&key) {
                        match &mut v.tex {
                            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                            None => v.tex = Some(ctx.load_texture(format!("video-{}-{:?}", key.0, key.1), img, egui::TextureOptions::LINEAR)),
                        }
                    }
                }
                DecodeMsg::NeedKeyframe(key) => {
                    let Some(v) = self.video_frames.get_mut(&key) else { continue };
                    if v.last_request.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                        continue;
                    }
                    v.last_request = Some(Instant::now());
                    if key.0 == self.my_id {
                        // Own tile: ask our own encoder directly.
                        let src = match key.1 {
                            VideoKind::Camera => self.cam.as_ref(),
                            VideoKind::Screen => self.screen.as_ref(),
                        };
                        if let Some(c) = src {
                            c.request_keyframe();
                        }
                    } else if let Some(tx) = &self.cmd_tx {
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
        let Some(cmd_tx) = self.cmd_tx.clone() else {
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
                                .color(egui::Color32::WHITE),
                            )
                            .fill(theme::ACCENT),
                        )
                        .clicked()
                    {
                        action = Some(true);
                    }
                    if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                        action = Some(false);
                    }
                });
            });
        if let Some(index) = switch_to {
            self.start_cam_preview(index, &cmd_tx);
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
        let Some(cmd_tx) = self.cmd_tx.clone() else {
            self.screen_picker = None;
            return;
        };
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

        let mut open = true;
        let mut cancel = false;
        let mut share: Option<ShareSource> = None;
        let mut refresh = false;
        egui::Window::new(format!("{}  Share Your Screen", icons::regular::SCREENCAST))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([700.0, 520.0])
            .show(ctx, |ui| {
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

                let list_h = 420.0;
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

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
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
                    ui.checkbox(&mut self.config.share_screen_audio, format!("{}  Share audio", icons::regular::SPEAKER_HIGH))
                        .on_hover_text("Also send what your computer plays (RustTalk voices are filtered out)");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                picker.selected.is_some(),
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}  Go Live",
                                        icons::regular::BROADCAST
                                    ))
                                    .color(egui::Color32::WHITE),
                                )
                                .fill(theme::ACCENT),
                            )
                            .clicked()
                        {
                            share = picker.selected;
                        }
                        if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        if refresh {
            let tab = picker.tab;
            let mut fresh = ScreenPicker::open();
            fresh.tab = tab;
            *picker = fresh;
        }
        if let Some(source) = share {
            self.screen_picker = None;
            self.start_screen_share(source, &cmd_tx);
        } else if !open || cancel {
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

    /// Closes the picker and frees the preview images.
    pub(super) fn close_gif_picker(&mut self) {
        for bytes in self.giphy.previews.values() {
            self.textures.remove_bytes(bytes);
        }
        self.giphy = GifPicker::default();
    }

    /// Asks the server for a result page (`offset` 0 = new search).
    fn gif_request(&mut self, query: String, offset: u32) {
        if !self.gif_available {
            return;
        }
        let Some(tx) = self.cmd_tx.clone() else { return };
        if offset == 0 {
            self.giphy.results.clear();
            self.giphy.end = false;
            self.giphy.error = None;
        }
        self.giphy.shown_query = Some(query.clone());
        self.giphy.loading = true;
        let _ = tx.send(Command::GifSearch { query, offset });
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
            match msg {
                GiphyMsg::Preview(url, bytes) => {
                    if self.giphy.open {
                        self.giphy.previews.insert(url, bytes);
                    }
                }
                GiphyMsg::Full(bytes) => {
                    self.giphy.sending = false;
                    if let (Some((channel_id, reply_to)), Some(tx)) = (self.gif_target.take(), self.cmd_tx.clone()) {
                        let _ = tx.send(Command::SendChat { channel_id, text: String::new(), image: Some(bytes), reply_to });
                    }
                    self.close_gif_picker();
                }
                GiphyMsg::FullFailed(e) => {
                    self.giphy.sending = false;
                    self.gif_target = None;
                    self.giphy.error = Some(e);
                }
            }
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
                        if !self.gif_available {
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
                                            std::thread::spawn(move || {
                                                if let Ok(bytes) = media::download(&url) {
                                                    let _ = tx.send(GiphyMsg::Preview(url, bytes));
                                                    repaint.request_repaint();
                                                }
                                            });
                                        }
                                    }
                                }
                                if hovered {
                                    ui.painter().rect_stroke(rect, 6.0, egui::Stroke::new(2.0_f32, theme::ACCENT), egui::StrokeKind::Inside);
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
            let Some(ch) = self.selected_channel else { return };
            self.gif_target = Some((ch, self.reply_to.take()));
            self.giphy.sending = true;
            self.giphy.error = None;
            let tx = self.giphy_tx.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let msg = match media::download(&g.url) {
                    Ok(bytes) => GiphyMsg::Full(bytes),
                    Err(e) => GiphyMsg::FullFailed(format!("{e:#}")),
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
            egui::Stroke::new(2.0_f32, theme::ACCENT),
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
    pub(super) tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    pub(super) tex: Option<egui::TextureHandle>,
    pub(super) last_request: Option<Instant>,
}

/// From decoder threads to the UI.
pub(super) enum DecodeMsg {
    Picture((u64, VideoKind), egui::ColorImage),
    NeedKeyframe((u64, VideoKind)),
}

impl VideoView {
    /// Starts the decoder thread (ends when the view and its sender are dropped).
    fn spawn(key: (u64, VideoKind), out: std::sync::mpsc::Sender<DecodeMsg>, ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        let _ = std::thread::Builder::new().name(format!("video-dec-{}", key.0)).spawn(move || {
            let mut dec = match crate::video_codec::H264Decoder::new() {
                Ok(d) => d,
                Err(e) => return tracing::warn!("{e:#}"),
            };
            for frame in rx {
                match dec.decode(&frame) {
                    Ok(crate::video_codec::Decoded::Picture(img)) => {
                        dec.got_picture();
                        if out.send(DecodeMsg::Picture(key, img)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    Ok(crate::video_codec::Decoded::Nothing) => {}
                    Err(e) => tracing::debug!("{e:#}"),
                }
                if dec.need_keyframe && out.send(DecodeMsg::NeedKeyframe(key)).is_err() {
                    break;
                }
            }
        });
        Self { tx, tex: None, last_request: None }
    }
}
