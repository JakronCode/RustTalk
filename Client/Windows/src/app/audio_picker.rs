//! Audio source chooser. Discovery never opens a recording stream.
use super::theme::IconState;
use super::*;
use crate::audio::share_source::{self, AudioApplication, AudioShareSource, MAX_MIX};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub(super) struct AudioPicker {
    selected: AudioShareSource,
    /// A selected app stopped playing and left the selection.
    dropped: bool,
    applications: Vec<AudioApplication>,
    textures: Vec<Option<egui::TextureHandle>>,
    pending: Option<Receiver<anyhow::Result<Vec<AudioApplication>>>>,
    stop: Arc<AtomicBool>,
    error: Option<String>,
}

impl Drop for AudioPicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl AudioPicker {
    fn new(selected: AudioShareSource) -> Self {
        let mut picker = Self {
            selected,
            dropped: false,
            applications: vec![],
            textures: vec![],
            pending: None,
            stop: Arc::new(AtomicBool::new(false)),
            error: None,
        };
        picker.refresh();
        picker
    }

    fn refresh(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.stop = Arc::new(AtomicBool::new(false));
        let stop = self.stop.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("audio-sources".into())
            .spawn(move || {
                let _ = tx.send(share_source::applications(&stop));
            }) {
            Ok(_) => {
                self.pending = Some(rx);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Could not discover applications: {e}")),
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.pending = None;
                match result {
                    Ok(apps) => {
                        self.applications = apps;
                        self.textures.clear();
                        self.error = None;
                        if let AudioShareSource::Applications(list) = &mut self.selected {
                            let before = list.len();
                            list.retain(|s| self.applications.iter().any(|app| &app.source == s));
                            self.dropped = list.len() < before;
                        }
                    }
                    Err(e) => {
                        self.applications.clear();
                        self.textures.clear();
                        self.error = Some(format!("Application audio unavailable: {e:#}"));
                    }
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.error = Some("Application discovery stopped; refresh to retry".into());
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Uploads app icons once per discovery result.
    fn sync_icons(&mut self, ctx: &egui::Context) {
        if self.textures.len() == self.applications.len() {
            return;
        }
        self.textures = self
            .applications
            .iter()
            .enumerate()
            .map(|(i, app)| {
                app.icon.as_ref().map(|icon| {
                    ctx.load_texture(
                        format!("audio-app-icon-{i}"),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [share_source::ICON_PX; 2],
                            &icon.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                })
            })
            .collect();
    }

    fn selected_apps(&self) -> &[share_source::ApplicationSource] {
        match &self.selected {
            AudioShareSource::Applications(list) => list,
            AudioShareSource::System => &[],
        }
    }

    /// Adds or removes one app from the mix; picking an app leaves "System audio".
    fn toggle_app(&mut self, source: share_source::ApplicationSource) {
        if let AudioShareSource::Applications(list) = &mut self.selected {
            if let Some(i) = list.iter().position(|s| *s == source) {
                list.remove(i);
            } else if list.len() < MAX_MIX {
                list.push(source);
            }
        } else {
            self.selected = AudioShareSource::Applications(vec![source]);
        }
    }

    fn selection_available(&self) -> bool {
        match &self.selected {
            AudioShareSource::System => true,
            AudioShareSource::Applications(list) => self.pending.is_none() && !list.is_empty(),
        }
    }
}

impl RustTalkApp {
    pub(super) fn open_audio_picker(&mut self) {
        if self.in_voice() {
            self.audio_picker = Some(AudioPicker::new(self.shared_audio_source.clone()));
        }
    }

    pub(super) fn audio_sharing_active(&self) -> bool {
        self.screen_audio.is_some() || self.audio_only
    }

    /// Audio button: always opens the dialog; it holds "Stop sharing" and the mix while sharing.
    pub(super) fn toggle_audio_sharing(&mut self) {
        if self.audio_picker.is_some() {
            self.audio_picker = None;
        } else {
            self.open_audio_picker();
        }
    }

    pub(super) fn start_audio_selection(&mut self, source: AudioShareSource) {
        if !self.in_voice() {
            return;
        }
        self.stop_shared_audio();
        self.shared_audio_source = source;
        self.audio_only = true;
        self.screen_audio_failed = false;
        self.sync_shared_audio();
    }

    pub(super) fn ui_audio_picker(&mut self, ctx: &egui::Context) {
        let active = self.audio_sharing_active();
        let Some(picker) = self.audio_picker.as_mut() else {
            return;
        };
        picker.poll();
        picker.sync_icons(ctx);
        let mut open = true;
        let mut close = false;
        let mut start = None;
        let mut stop = false;
        egui::Window::new(format!("{}  Share audio", icons::regular::MUSIC_NOTES))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([400.0, 520.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Your microphone stays separate.").color(theme::MUTED),
                );
                ui.add_space(10.0);
                let system = AudioShareSource::System;
                if source_row(
                    ui,
                    RowIcon::System,
                    "System audio",
                    Some(if cfg!(windows) {
                        "Everything on the selected output device"
                    } else {
                        "Everything on the default output"
                    }),
                    picker.selected == system,
                    false,
                )
                .clicked()
                {
                    picker.selected = system;
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    theme::caption(ui, "Applications");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let idle = picker.pending.is_none();
                        let state = if idle { IconState::Normal } else { IconState::Disabled };
                        if theme::icon_button(ui, icons::regular::ARROWS_CLOCKWISE, state, "Refresh list").clicked() {
                            picker.refresh();
                        }
                        if !idle {
                            ui.spinner();
                        }
                    });
                });
                ui.add_space(4.0);
                let mut toggled = None;
                let listed = picker.pending.is_none() && !picker.applications.is_empty();
                egui::ScrollArea::vertical()
                    .max_height(264.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        for (i, app) in picker.applications.iter().enumerate() {
                            let icon = match picker.textures.get(i) {
                                Some(Some(tex)) => RowIcon::Image(tex),
                                _ => RowIcon::Letter(&app.label),
                            };
                            let on = picker.selected_apps().contains(&app.source);
                            if source_row(ui, icon, &app.label, app.detail.as_deref(), on, true).clicked() {
                                toggled = Some(app.source.clone());
                            }
                        }
                        if picker.pending.is_some() && picker.applications.is_empty() {
                            hint(ui, "Looking for apps\u{2026}");
                        } else if !listed && picker.error.is_none() {
                            hint(ui, "Nothing is playing audio right now. Start playback in an app, then refresh.");
                        }
                    });
                if let Some(source) = toggled {
                    picker.toggle_app(source);
                }
                if let Some(error) = &picker.error {
                    ui.colored_label(theme::RED, error);
                }
                let count = picker.selected_apps().len();
                if picker.dropped {
                    ui.colored_label(theme::YELLOW, "An app stopped playing audio and left the selection.");
                } else if count >= MAX_MIX {
                    hint(ui, &format!("Up to {MAX_MIX} apps can be mixed."));
                } else if count > 1 {
                    hint(ui, &format!("Mixing {count} apps into one stream."));
                } else if count == 1 {
                    hint(ui, if cfg!(windows) {
                        "Pick more apps to mix them. Includes helper processes."
                    } else {
                        "Pick more apps to mix them. An app that closes leaves the mix."
                    });
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if active
                        && theme::ghost_button(ui, egui::RichText::new("Stop sharing").color(theme::RED)).clicked()
                    {
                        stop = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = picker.selection_available();
                        let go = ui
                            .add_enabled_ui(ready, |ui| {
                                theme::primary_button(ui, &format!("{}  {}", icons::regular::MUSIC_NOTES, if active { "Apply" } else { "Share audio" }))
                            })
                            .inner;
                        if go.clicked() {
                            start = Some(picker.selected.clone());
                        }
                        if theme::ghost_button(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if let Some(source) = start {
            self.start_audio_selection(source);
            self.audio_picker = None;
        } else if stop {
            self.stop_shared_audio();
        } else if !open || close {
            self.audio_picker = None;
        }
    }
}

enum RowIcon<'a> {
    System,
    Image(&'a egui::TextureHandle),
    /// Deterministic fallback: first letter on a name-colored tile.
    Letter(&'a str),
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.5).color(theme::MUTED));
}

fn source_row(
    ui: &mut egui::Ui,
    icon: RowIcon,
    title: &str,
    subtitle: Option<&str>,
    selected: bool,
    multi: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, title)
    });
    let painter = ui.painter();
    let accent = theme::accent();
    painter.rect_filled(
        rect,
        8.0,
        if selected {
            accent.gamma_multiply(0.16)
        } else if resp.hovered() {
            theme::HOVER
        } else {
            theme::DARK
        },
    );
    if selected {
        painter.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.5_f32, accent),
            egui::StrokeKind::Inside,
        );
    }
    let tile = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(10.0, 10.0),
        egui::vec2(32.0, 32.0),
    );
    match icon {
        RowIcon::System => {
            painter.rect_filled(tile, 8.0, accent);
            painter.text(
                tile.center(),
                egui::Align2::CENTER_CENTER,
                icons::regular::SPEAKER_HIGH,
                egui::FontId::proportional(18.0),
                theme::on_accent(),
            );
        }
        RowIcon::Image(tex) => {
            painter.image(
                tex.id(),
                tile,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        RowIcon::Letter(name) => {
            painter.rect_filled(tile, 8.0, theme::avatar_color(name));
            painter.text(
                tile.center(),
                egui::Align2::CENTER_CENTER,
                name.chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .collect::<String>(),
                egui::FontId::proportional(17.0),
                egui::Color32::WHITE,
            );
        }
    }
    let text_x = tile.right() + 12.0;
    let max_w = rect.right() - 40.0 - text_x;
    let one_line = |text: &str, size: f32, color: egui::Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(
            text.to_string(),
            egui::FontId::proportional(size),
            color,
        );
        job.wrap = egui::text::TextWrapping {
            max_width: max_w,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        };
        painter.layout_job(job)
    };
    let title_galley = one_line(title, 14.5, theme::HEADER);
    let sub_galley = subtitle.map(|t| one_line(t, 12.0, theme::MUTED));
    let total = title_galley.size().y + sub_galley.as_ref().map_or(0.0, |g| g.size().y + 1.0);
    let mut y = rect.center().y - total / 2.0;
    let h = title_galley.size().y;
    painter.galley(egui::pos2(text_x, y), title_galley, theme::HEADER);
    y += h + 1.0;
    if let Some(g) = sub_galley {
        painter.galley(egui::pos2(text_x, y), g, theme::MUTED);
    }
    if selected {
        painter.text(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            icons::regular::CHECK_CIRCLE,
            egui::FontId::proportional(22.0),
            accent,
        );
    } else if multi {
        painter.text(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            icons::regular::CIRCLE,
            egui::FontId::proportional(22.0),
            theme::MUTED.gamma_multiply(0.5),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn picker_keeps_application_cards_visible() {
        let ctx = egui::Context::default();
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        app.audio_picker = Some(AudioPicker {
            selected: AudioShareSource::System,
            dropped: false,
            applications: ["Firefox", "Discord", "Limusic", "Scrcpy"]
                .into_iter()
                .enumerate()
                .map(|(i, name)| AudioApplication {
                    label: name.into(),
                    detail: None,
                    icon: None,
                    source: share_source::ApplicationSource::for_test(i as u32 + 1),
                })
                .collect(),
            textures: vec![],
            pending: None,
            stop: Arc::new(AtomicBool::new(false)),
            error: None,
        });
        let mut output = None;
        for frame in 0..3 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    time: Some(frame as f64),
                    ..Default::default()
                },
                |ctx| app.ui_audio_picker(ctx),
            ));
        }
        let output = output.unwrap();
        for label in ["Firefox", "Discord", "Limusic", "Scrcpy"] {
            assert!(
                output.shapes.iter().any(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        text.galley.job.text == label
                            && shape.clip_rect.contains_rect(egui::Rect::from_min_size(
                                text.pos,
                                text.galley.size(),
                            ))
                    } else {
                        false
                    }
                }),
                "{label} must be fully visible"
            );
        }
        for shape in &output.shapes {
            if let egui::epaint::Shape::Rect(card) = &shape.shape {
                if (card.rect.height() - 52.0).abs() < 0.1 {
                    assert!(
                        shape.clip_rect.contains_rect(card.rect.shrink(0.5)),
                        "source cards must not be cropped by the list viewport"
                    );
                }
            }
        }
    }

    #[test]
    fn system_selection_needs_no_discovery_or_capture() {
        let picker = AudioPicker {
            selected: AudioShareSource::System,
            dropped: false,
            applications: vec![],
            textures: vec![],
            pending: None,
            stop: Arc::new(AtomicBool::new(false)),
            error: Some("unavailable".into()),
        };
        assert!(picker.selection_available());
        let stop = picker.stop.clone();
        drop(picker);
        assert!(stop.load(Ordering::Relaxed));
    }
}
