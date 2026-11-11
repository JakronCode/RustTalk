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
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
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
                                        for k in [VideoKind::Screen, VideoKind::Camera] {
                                            if !self.video_frames.contains_key(&(user.id, k)) {
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
                                                    kind_label(k),
                                                    user.name
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
                if let Some(data) = self.video_frames.get(&key).cloned() {
                    if let Some(tex) =
                        self.textures.texture_for(ctx, &data, &format!("video-{uid}-{kind:?}"))
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
        let _ = cmd_tx.send(Command::StopVideo(kind));
        let still = self.screen.is_some() || self.cam.as_ref().is_some_and(|c| c.is_sending());
        let _ = cmd_tx.send(Command::SetSharing(still));
        self.clear_video_of(self.my_id, Some(kind));
        if kind == VideoKind::Camera {
            *self.local_preview.lock().unwrap() = None;
            self.local_preview_tex = None;
        }
    }

    /// Video-Kacheln eines Users entfernen (eine Art oder alle).
    pub(super) fn clear_video_of(&mut self, user_id: u64, kind: Option<VideoKind>) {
        for k in [VideoKind::Camera, VideoKind::Screen] {
            if kind.is_some_and(|want| want != k) {
                continue;
            }
            if let Some(old) = self.video_frames.remove(&(user_id, k)) {
                self.textures.remove_bytes(&old);
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
    ) -> impl FnMut(Vec<u8>, bool) + Send + 'static {
        let tx = cmd_tx.clone();
        let events = self.event_tx.clone();
        let preview = self.local_preview.clone();
        let ctx = self.ctx.clone();
        let backlog = self.upload_backlog.clone();
        let my_id = self.my_id;
        move |jpeg: Vec<u8>, sending: bool| {
            if kind == VideoKind::Camera {
                *preview.lock().unwrap() = Some(jpeg.clone());
            }
            if sending {
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
        move |msg: String| {
            *errors.lock().unwrap() = Some(msg.clone());
            let _ = events.send(NetEvent::Status(msg));
        }
    }

    /// Eigene Vorschau als Textur (eine Textur, wird pro Frame aktualisiert).
    pub(super) fn local_preview_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let data = self.local_preview.lock().unwrap().clone()?;
        let key = media::hash_bytes(&data);
        if let Some((k, tex)) = &self.local_preview_tex {
            if *k == key {
                return Some(tex.clone());
            }
        }
        let img = image::load_from_memory(&data).ok()?.to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            &img,
        );
        match &mut self.local_preview_tex {
            Some((k, tex)) => {
                tex.set(color, egui::TextureOptions::LINEAR);
                *k = key;
                Some(tex.clone())
            }
            None => {
                let tex = ctx.load_texture("local-preview", color, egui::TextureOptions::LINEAR);
                self.local_preview_tex = Some((key, tex.clone()));
                Some(tex)
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
        // Fehler aus dem Kamera-Thread in den Dialog uebernehmen.
        if let Some(err) = self.cam_error.lock().unwrap().take() {
            if let Some(d) = &mut self.cam_dialog {
                d.error = Some(err);
            }
        }
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

    /// GIPHY window: search, animated preview grid, click = attach.
    pub(super) fn ui_giphy(&mut self, ctx: &egui::Context) {
        // Collect background results also while the window is closed
        // (a chosen GIF may finish downloading after closing it).
        while let Ok(msg) = self.giphy_rx.try_recv() {
            match msg {
                GiphyMsg::Results(Ok(results)) => {
                    self.giphy.loading = false;
                    self.giphy.error = if results.is_empty() { Some("No GIFs found.".into()) } else { None };
                    self.giphy.results = results;
                }
                GiphyMsg::Results(Err(e)) => {
                    self.giphy.loading = false;
                    self.giphy.error = Some(e);
                }
                GiphyMsg::Preview(url, bytes) => {
                    self.giphy.previews.insert(url, bytes);
                }
                GiphyMsg::Full(bytes) => {
                    self.giphy.attaching = false;
                    self.pending_image = Some(("giphy.gif".to_string(), bytes));
                    self.giphy.open = false;
                }
                GiphyMsg::FullFailed(e) => {
                    self.giphy.attaching = false;
                    self.giphy.error = Some(e);
                }
            }
        }
        if !self.giphy.open {
            return;
        }
        let mut open = self.giphy.open;
        let animate = self.config.animate_gifs;
        let mut open_settings = false;
        egui::Window::new(format!("{}  Search GIFs (GIPHY)", icons::regular::GIF))
            .open(&mut open)
            .collapsible(false)
            .default_size([460.0, 420.0])
            .show(ctx, |ui| {
                let Some(key) = self.config.giphy_api_key.clone() else {
                    ui.label(
                        egui::RichText::new(
                            "No GIPHY API key set.\nCreate one for free at developers.giphy.com and enter it under Settings > Advanced.",
                        )
                        .color(theme::MUTED),
                    );
                    if ui.button(format!("{}  Open settings", icons::regular::GEAR_SIX)).clicked() {
                        open_settings = true;
                    }
                    return;
                };
                let search = ui
                    .horizontal(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.giphy.query)
                                .hint_text("Search GIPHY...")
                                .desired_width(ui.available_width() - 90.0),
                        );
                        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter {
                            resp.request_focus();
                        }
                        enter || ui.button(format!("{}  Search", icons::regular::MAGNIFYING_GLASS)).clicked()
                    })
                    .inner;
                if search && !self.giphy.query.trim().is_empty() && !self.giphy.loading {
                    self.giphy.loading = true;
                    self.giphy.error = None;
                    self.giphy.results.clear();
                    self.giphy.requested.clear();
                    self.giphy.previews.clear();
                    let q = self.giphy.query.trim().to_string();
                    let tx = self.giphy_tx.clone();
                    let repaint = ctx.clone();
                    std::thread::spawn(move || {
                        let res = media::giphy_search(&key, &q).map_err(|e| format!("{e:#}"));
                        let _ = tx.send(GiphyMsg::Results(res));
                        repaint.request_repaint();
                    });
                }
                if self.giphy.loading {
                    ui.label(egui::RichText::new("Searching...").color(theme::MUTED));
                }
                if self.giphy.attaching {
                    ui.label(egui::RichText::new("Downloading GIF...").color(theme::MUTED));
                }
                if let Some(e) = &self.giphy.error {
                    ui.label(egui::RichText::new(e).color(theme::RED));
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let results = self.giphy.results.clone();
                        for gif in results {
                            let tex = self
                                .giphy
                                .previews
                                .get(&gif.preview_url)
                                .and_then(|bytes| self.textures.image_for(ctx, bytes, &gif.id, animate));
                            match tex {
                                Some(tex) => {
                                    let clicked = ui
                                        .add_enabled(
                                            !self.giphy.attaching,
                                            egui::ImageButton::new(egui::Image::new(&tex).max_width(130.0).max_height(100.0)).frame(true),
                                        )
                                        .on_hover_text("Attach")
                                        .clicked();
                                    if clicked {
                                        self.giphy.attaching = true;
                                        self.giphy.error = None;
                                        let tx = self.giphy_tx.clone();
                                        let url = gif.full_url.clone();
                                        let repaint = ctx.clone();
                                        std::thread::spawn(move || {
                                            let msg = match media::download(&url) {
                                                Ok(bytes) => GiphyMsg::Full(bytes),
                                                Err(e) => GiphyMsg::FullFailed(format!("{e:#}")),
                                            };
                                            let _ = tx.send(msg);
                                            repaint.request_repaint();
                                        });
                                    }
                                }
                                None => {
                                    // Load the preview once (by URL).
                                    if self.giphy.requested.insert(gif.preview_url.clone()) {
                                        let tx = self.giphy_tx.clone();
                                        let url = gif.preview_url.clone();
                                        let repaint = ctx.clone();
                                        std::thread::spawn(move || {
                                            if let Ok(bytes) = media::download(&url) {
                                                let _ = tx.send(GiphyMsg::Preview(url, bytes));
                                                repaint.request_repaint();
                                            }
                                        });
                                    }
                                    ui.add_enabled(false, egui::Button::new("...").min_size(egui::vec2(100.0, 75.0)));
                                }
                            }
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Powered by GIPHY").size(11.0).color(theme::DISABLED));
            });
        self.giphy.open = open && self.giphy.open;
        if open_settings {
            self.giphy.open = false;
            self.open_settings(SettingsPage::Advanced);
        }
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
