//! Volume mixer docked right of the voice chat: one row per stream you hear.
//! Volumes live only in the config (`user_volumes` / `share_volumes`, by
//! username), so the user menu, settings and this panel always agree.

use super::theme::{self, IconState};
use super::*;
use crate::audio::{StreamMeter, MAX_VOLUME};

/// A shared-audio row stays this long after its last packet (silence is not sent).
const SHARE_LINGER: Duration = Duration::from_secs(15);
/// Meter readings older than this show as silence.
const METER_FRESH: Duration = Duration::from_millis(300);
pub(super) const MIXER_ID: &str = "volume_mixer";

/// The one volume slider (0-200 %) used by the mixer and the user menu.
pub(super) fn volume_slider(pct: &mut f32) -> egui::Slider<'_> {
    egui::Slider::new(pct, 0.0..=MAX_VOLUME * 100.0)
        .step_by(5.0)
        .fixed_decimals(0)
        .suffix(" %")
}

/// Streams to list for `members` (everyone but us, by name): voice always,
/// shared computer audio while it plays or played a moment ago.
pub(super) fn mixer_rows(
    members: &[UserInfo],
    my_id: u64,
    meters: &HashMap<u128, StreamMeter>,
    now: Instant,
) -> Vec<(UserInfo, StreamKind)> {
    let mut others: Vec<&UserInfo> = members.iter().filter(|u| u.id != my_id).collect();
    others.sort_by_cached_key(|u| (u.name.to_lowercase(), u.id));
    let mut rows = Vec::new();
    for user in others {
        rows.push((user.clone(), StreamKind::Voice));
        let shared = meters
            .get(&StreamKind::Shared.key(user.id))
            .is_some_and(|m| now.saturating_duration_since(m.heard) < SHARE_LINGER);
        if shared {
            rows.push((user.clone(), StreamKind::Shared));
        }
    }
    rows
}

/// Level (linear) -> meter fill 0..1 over -60..0 dBFS.
pub(super) fn meter_fill(level: f32) -> f32 {
    if level <= 1e-6 {
        return 0.0;
    }
    ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn level_bar(p: &egui::Painter, rect: egui::Rect, level: f32, muted: bool) {
    p.rect_filled(rect, 2.0, theme::DARK());
    let w = rect.width() * meter_fill(level);
    if w >= 2.0 {
        let color = if muted {
            theme::DISABLED()
        } else if level >= 0.9 {
            theme::YELLOW
        } else {
            theme::GREEN
        };
        p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())), 2.0, color);
    }
}

fn kind_label(kind: StreamKind) -> (&'static str, &'static str) {
    match kind {
        StreamKind::Voice => (icons::regular::MICROPHONE, "Voice"),
        StreamKind::Shared | StreamKind::App(_) => (icons::regular::MONITOR_PLAY, "Shared audio"),
    }
}

/// What one mixer row shows: mute toggle, two-line label, slider value.
struct Row<'a> {
    title: &'a str,
    subtitle: String,
    muted: bool,
    mute_enabled: bool,
    mute_icon: (&'static str, &'static str),
    tip: &'a str,
}

/// Responses of one row: mute button, title label, slider.
struct RowResponse {
    mute: egui::Response,
    title: egui::Response,
    slider: egui::Response,
}

/// One line like a system volume mixer: [mute] name/kind ...slider... 100 %
fn mixer_row(ui: &mut egui::Ui, row: &Row, pct: &mut f32, level: f32, dim: bool) -> RowResponse {
    ui.horizontal(|ui| {
        let state = match (row.mute_enabled, row.muted) {
            (false, _) => IconState::Disabled,
            (true, true) => IconState::Danger,
            (true, false) => IconState::Normal,
        };
        let glyph = if row.muted { row.mute_icon.1 } else { row.mute_icon.0 };
        let mute = theme::icon_button(ui, glyph, state, row.tip);
        let label_w = (ui.available_width() * 0.36).clamp(80.0, 170.0);
        let title = ui
            .allocate_ui_with_layout(egui::vec2(label_w, 30.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(label_w);
                ui.spacing_mut().item_spacing.y = 0.0;
                let color = if row.muted { theme::MUTED() } else { theme::HEADER() };
                let title = ui.add(
                    egui::Label::new(egui::RichText::new(row.title).strong().color(color))
                        .truncate()
                        .sense(egui::Sense::click()),
                );
                ui.add(egui::Label::new(egui::RichText::new(&row.subtitle).size(11.5).color(theme::MUTED())).truncate());
                title
            })
            .inner;
        let track = (ui.available_width() - 64.0).max(60.0);
        ui.spacing_mut().slider_width = track;
        let slider = ui.add(volume_slider(pct));
        // Level meter in the free space under the slider track.
        let bar = egui::Rect::from_min_size(egui::pos2(slider.rect.left(), slider.rect.bottom() + 2.0), egui::vec2(track, 3.0));
        level_bar(ui.painter(), bar, level, dim);
        RowResponse { mute, title, slider }
    })
    .inner
}

/// Square icon button that darkens on hover (green while the mixer is open).
fn mixer_icon_button(ui: &mut egui::Ui, rect: egui::Rect, open: bool) -> egui::Response {
    let resp = ui
        .interact(rect, ui.id().with("mixer_toggle"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = match (open, resp.hovered()) {
        (true, false) => theme::GREEN,
        (true, true) => theme::GREEN.gamma_multiply(0.7),
        (false, false) => theme::INPUT(),
        (false, true) => theme::DARK(),
    };
    let fg = if open { egui::Color32::WHITE } else if resp.hovered() { theme::HEADER() } else { theme::TEXT() };
    let p = ui.painter();
    p.rect_filled(rect, 6.0, fill);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icons::regular::SLIDERS_HORIZONTAL,
        egui::FontId::proportional(18.0),
        fg,
    );
    resp
}

const SPEAKER: (&str, &str) =(icons::regular::SPEAKER_HIGH, icons::regular::SPEAKER_SLASH);

impl RustTalkApp {
    pub(super) fn toggle_mixer(&mut self) {
        self.config.mixer_open = !self.config.mixer_open;
        self.config.save();
    }

    /// The mixer belongs to the chat of our own voice channel only.
    pub(super) fn mixer_available(&self, ch_id: u64) -> bool {
        self.viewing_voice() && self.voice_channel() == Some(ch_id)
    }

    /// Mixer icon in the top right corner of the voice chat; the panel slides
    /// in from there, and the same spot closes it again.
    pub(super) fn mixer_toggle(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let open = self.config.mixer_open;
        let tip = if open { "Close the volume mixer" } else { "Volume mixer: every stream you hear" };
        if mixer_icon_button(ui, rect, open).on_hover_text(tip).clicked() {
            self.toggle_mixer();
        }
    }

    pub(super) fn ui_mixer(&mut self, ui: &mut egui::Ui, members: &[UserInfo]) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Volume Mixer")
                    .size(15.0)
                    .strong()
                    .color(theme::HEADER()),
            );
        });
        ui.add_space(2.0);
        let now = Instant::now();
        let meters = self.control.stream_meters.lock().unwrap().clone();
        let mut rows = mixer_rows(members, self.srv.my_id, &meters, now);
        let streams = self.control.shared_streams.lock().unwrap().clone();
        for user in members.iter().filter(|u| u.id != self.srv.my_id) {
            if let Some(apps) = streams.get(&user.id) {
                rows.extend(apps.iter().map(|a| (user.clone(), StreamKind::App(a.id))));
            }
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            self.mixer_own_rows(ui);
            ui.add_space(4.0);
            theme::menu_separator(ui);
            theme::caption(ui, "In this channel");
            if rows.is_empty() {
                ui.label(egui::RichText::new("Nobody else is here yet.").color(theme::MUTED()));
            }
            self.mixer_stream_rows(ui, rows);
        });
        // Live meters.
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }

    pub(super) fn shared_audio_popup(&mut self, ui: &mut egui::Ui, user: &UserInfo, response: &egui::Response) {
        let me = user.id == self.srv.my_id;
        let available = !me && self.viewing_voice() && self.voice_channel() == Some(user.channel_id);
        response.clone().on_hover_text(if me {
            "You are sharing audio. Use Audio to change what you share."
        } else if !available {
            "Sharing audio — join their voice channel to adjust volume"
        } else {
            "Sharing audio — click for volume and mute (only for you)"
        }).on_hover_cursor(if available { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Default });
        if !available {
            return;
        }
        let popup_id = response.id.with(("shared_audio_mixer", self.srv.id, self.srv.generation, user.id));
        if response.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        egui::popup_below_widget(ui, popup_id, response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_width(340.0);
            ui.label(egui::RichText::new(format!("{}'s shared audio", user.name)).strong());
            ui.label(egui::RichText::new("Only changes what you hear.").small().color(theme::MUTED()));
            theme::menu_separator(ui);
            let streams = self.control.shared_streams.lock().unwrap().get(&user.id).cloned().unwrap_or_default();
            let kinds = if streams.is_empty() {
                vec![StreamKind::Shared]
            } else {
                streams.iter().map(|s| StreamKind::App(s.id)).collect()
            };
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                self.mixer_stream_rows(ui, kinds.into_iter().map(|k| (user.clone(), k)).collect());
            });
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        });
    }

    fn mixer_stream_rows(&mut self, ui: &mut egui::Ui, rows: Vec<(UserInfo, StreamKind)>) {
        let now = Instant::now();
        let meters = self.control.stream_meters.lock().unwrap().clone();
        let streams = self.control.shared_streams.lock().unwrap().clone();
        let user_muted = self.local_muted.lock().unwrap().clone();
        let stream_muted = self.control.muted_streams.lock().unwrap().clone();
        let deafened = self.control.self_deafened.load(Ordering::Relaxed);
        for (user, kind) in rows {
            let key = kind.key(user.id);
            let whole_user = user_muted.contains(&user.id);
            let muted = whole_user || stream_muted.contains(&key);
            let (icon, default_label) = kind_label(kind);
            let label = if let StreamKind::App(id) = kind {
                streams.get(&user.id).and_then(|v| v.iter().find(|s| s.id == id)).map(|s| s.label.as_str()).unwrap_or("Application")
            } else { default_label };
            let (mute_enabled, tip) = match kind {
                StreamKind::Voice if whole_user => (true, "Unmute for me"),
                StreamKind::Voice => (true, "Mute for me (voice and shared audio)"),
                StreamKind::Shared | StreamKind::App(_) if whole_user => (false, "Everything from this user is muted"),
                StreamKind::Shared | StreamKind::App(_) if muted => (true, "Unmute their shared audio"),
                StreamKind::Shared | StreamKind::App(_) => (true, "Mute only their shared audio"),
            };
            let row = Row {
                title: &user.name,
                subtitle: format!("{icon} {label}"),
                muted,
                mute_enabled,
                mute_icon: SPEAKER,
                tip,
            };
            ui.push_id(key, |ui| {
                let level = meters
                    .get(&key)
                    .filter(|m| now.saturating_duration_since(m.heard) < METER_FRESH)
                    .map_or(0.0, |m| m.level);
                let gain = if matches!(kind, StreamKind::App(_)) { self.user_volumes.lock().unwrap().get(&key).copied().unwrap_or(1.0) } else { self.user_volume(&user.name, kind) };
                let mut pct = (gain * 100.0).round();
                let r = mixer_row(ui, &row, &mut pct, level, muted || deafened);
                r.title.on_hover_text("Right-click for more").context_menu(|ui| self.user_menu(ui, &user));
                if r.mute.clicked() {
                    match kind {
                        StreamKind::Voice => toggle(&mut self.local_muted.lock().unwrap(), user.id),
                        StreamKind::Shared | StreamKind::App(_) => toggle(&mut self.control.muted_streams.lock().unwrap(), key),
                    }
                }
                if r.slider.changed() {
                    if matches!(kind, StreamKind::App(_)) { self.user_volumes.lock().unwrap().insert(key, pct / 100.0); } else { self.set_user_volume(user.id, &user.name, kind, pct / 100.0); }
                }
                if r.slider.drag_stopped() || (r.slider.changed() && !r.slider.dragged()) {
                    self.config.save();
                }
            });
        }
    }

    /// Our own controls (same values as the user panel and Settings): what we
    /// hear and our microphone; their speaker buttons deafen / mute.
    fn mixer_own_rows(&mut self, ui: &mut egui::Ui) {
        theme::caption(ui, "You");
        let deafened = self.control.self_deafened.load(Ordering::Relaxed);
        let self_muted = self.control.self_muted.load(Ordering::Relaxed);
        let mut save = false;

        let mut pct = (self.config.output_volume * 100.0).round();
        let row = Row {
            title: "Output",
            subtitle: format!("{} Everyone you hear", icons::regular::HEADPHONES),
            muted: deafened,
            mute_enabled: true,
            mute_icon: (icons::regular::HEADPHONES, icons::regular::SPEAKER_SLASH),
            tip: if deafened { "Undeafen" } else { "Deafen" },
        };
        let r = ui.push_id("mixer_output", |ui| mixer_row(ui, &row, &mut pct, 0.0, deafened)).inner;
        if r.slider.changed() {
            self.config.output_volume = pct / 100.0;
            self.control.set_output_volume(self.config.output_volume);
        }
        save |= r.slider.drag_stopped() || (r.slider.changed() && !r.slider.dragged());
        if r.mute.clicked() {
            self.toggle_deafen();
        }

        let mut pct = (self.config.input_volume * 100.0).round();
        let row = Row {
            title: "Microphone",
            subtitle: format!("{} What others hear", icons::regular::MICROPHONE),
            muted: self_muted || deafened,
            mute_enabled: true,
            mute_icon: (icons::regular::MICROPHONE, icons::regular::MICROPHONE_SLASH),
            tip: if self_muted { "Unmute" } else { "Mute" },
        };
        let r = ui.push_id("mixer_mic", |ui| mixer_row(ui, &row, &mut pct, self.control.mic_level(), self_muted || deafened)).inner;
        if r.slider.changed() {
            self.config.input_volume = pct / 100.0;
            self.control.set_input_volume(self.config.input_volume);
        }
        save |= r.slider.drag_stopped() || (r.slider.changed() && !r.slider.dragged());
        if r.mute.clicked() {
            self.toggle_mute();
        }
        if save {
            self.config.save();
        }
    }
}

fn toggle<T: Eq + std::hash::Hash + Copy>(set: &mut HashSet<T>, id: T) {
    if !set.remove(&id) {
        set.insert(id);
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn user(id: u64, name: &str) -> UserInfo {
        UserInfo {
            id,
            name: name.into(),
            channel_id: 5,
            muted: false,
            deafened: false,
            sharing: false,
            audio_sharing: false,
            role: Default::default(),
            streams: Vec::new(),
        }
    }

    pub(in crate::app) fn voice_app(ctx: &egui::Context) -> RustTalkApp {
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        let id = app.ensure_session("voice.invalid:7331").unwrap();
        app.view_session(id);
        app.srv.connected = true;
        app.srv.my_id = 1;
        app.srv.users.insert(1, user(1, "me"));
        app.srv.channels.insert(5, ChannelInfo {
            id: 5, name: "Voice".into(), kind: ChannelKind::Voice,
            parent: None, position: 0, user_ids: vec![1],
        });
        app.voice_owner = Some(app.srv.id);
        app.voice_generation = app.srv.generation;
        app.voice_confirmed = true;
        app
    }

    pub(in crate::app) fn text_pos(out: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        out.shapes.iter().find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == text => Some(t.pos + t.galley.size() / 2.0),
            _ => None,
        })
    }

    fn popup_frame(ctx: &egui::Context, app: &mut RustTalkApp, user: &UserInfo, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events, ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.button("Audio badge");
                ui.memory_mut(|m| m.open_popup(response.id.with(("shared_audio_mixer", app.srv.id, app.srv.generation, user.id))));
                app.shared_audio_popup(ui, user, &response);
            });
        })
    }

    #[test]
    fn audio_badge_popup_controls_only_that_persons_shared_streams() {
        use rusttalk_shared::protocol::AudioStreamInfo;
        let ctx = egui::Context::default();
        let mut app = voice_app(&ctx);
        let mut target = user(2, "Friend");
        target.audio_sharing = true;
        app.control.shared_streams.lock().unwrap().insert(2, vec![
            AudioStreamInfo { id: 10, label: "Music".into() },
            AudioStreamInfo { id: 11, label: "Game".into() },
        ]);
        for _ in 0..2 { popup_frame(&ctx, &mut app, &target, vec![]); }
        let out = popup_frame(&ctx, &mut app, &target, vec![]);
        assert!(text_pos(&out, "Friend's shared audio").is_some());
        assert!(text_pos(&out, &format!("{} Music", icons::regular::MONITOR_PLAY)).is_some());
        assert!(text_pos(&out, &format!("{} Game", icons::regular::MONITOR_PLAY)).is_some());
        assert!(text_pos(&out, "Voice").is_none());
        let mute = text_pos(&out, icons::regular::SPEAKER_HIGH).expect("app mute button");
        for pressed in [true, false] {
            popup_frame(&ctx, &mut app, &target, vec![egui::Event::PointerMoved(mute), egui::Event::PointerButton {
                pos: mute, button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
            }]);
        }
        assert_eq!(*app.control.muted_streams.lock().unwrap(), HashSet::from([StreamKind::App(10).key(2)]));
        assert!(app.local_muted.lock().unwrap().is_empty(), "voice and other people remain audible");
        assert!(app.audio_picker.is_none(), "receiving mixer never opens source settings");
        let other_listener = voice_app(&egui::Context::default());
        assert!(other_listener.control.muted_streams.lock().unwrap().is_empty(), "listener settings remain independent");
        app.control.shared_streams.lock().unwrap().clear();
        let out = popup_frame(&ctx, &mut app, &target, vec![]);
        assert!(text_pos(&out, &format!("{} Shared audio", icons::regular::MONITOR_PLAY)).is_some(), "silent mixed share still adjustable");
        target.channel_id = 99;
        let out = popup_frame(&ctx, &mut app, &target, vec![]);
        assert!(text_pos(&out, "Friend's shared audio").is_none(), "other channel has no playback controls");
        target.channel_id = 5;
        app.voice_owner = None;
        let out = popup_frame(&ctx, &mut app, &target, vec![]);
        assert!(text_pos(&out, "Friend's shared audio").is_none(), "other server has no playback controls");
    }

    #[test]
    fn rows_list_voice_always_and_shared_audio_while_it_plays() {
        let now = Instant::now();
        let members = [user(1, "me"), user(3, "bianka"), user(2, "Anna")];
        let mut meters = HashMap::new();
        meters.insert(StreamKind::Shared.key(3), StreamMeter { level: 0.5, heard: now });
        let rows: Vec<_> = mixer_rows(&members, 1, &meters, now).into_iter().map(|(u, k)| (u.id, k)).collect();
        assert_eq!(rows, [(2, StreamKind::Voice), (3, StreamKind::Voice), (3, StreamKind::Shared)]);

        let later = now + SHARE_LINGER + Duration::from_secs(1);
        let rows = mixer_rows(&members, 1, &meters, later);
        assert!(rows.iter().all(|(_, k)| *k == StreamKind::Voice), "stopped share disappears");
    }

    #[test]
    fn stream_keys_are_distinct_per_user_and_kind() {
        let keys: HashSet<u128> = [1u64, 2]
            .iter()
            .flat_map(|&id| StreamKind::ALL.map(|k| k.key(id)))
            .collect();
        assert_eq!(keys.len(), 4);
        assert!(StreamKind::ALL.iter().all(|k| crate::audio::mixer_speaker(k.key(7)) == 7));
    }

    #[test]
    fn meter_scale() {
        assert_eq!(meter_fill(0.0), 0.0);
        assert_eq!(meter_fill(1.0), 1.0);
        assert!((meter_fill(0.001) - 0.0).abs() < 1e-4, "-60 dB is empty");
        assert!(meter_fill(0.1) > 0.6 && meter_fill(0.1) < 0.7);
    }
}
