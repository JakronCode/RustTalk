//! Audio source chooser. Discovery never opens a recording stream.
use super::*;
use crate::audio::share_source::{self, AudioApplication, AudioShareSource};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub(super) struct AudioPicker {
    selected: AudioShareSource,
    applications: Vec<AudioApplication>,
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
            applications: vec![],
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
                        self.error = None;
                    }
                    Err(e) => {
                        self.applications.clear();
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

    fn selection_available(&self) -> bool {
        self.selected == AudioShareSource::System
            || (self.pending.is_none()
                && self
                    .applications
                    .iter()
                    .any(|app| self.selected == AudioShareSource::Application(app.source.clone())))
    }
}

impl RustTalkApp {
    pub(super) fn open_audio_picker(&mut self) {
        if self.connected && self.my_voice_channel().is_some() {
            self.audio_picker = Some(AudioPicker::new(self.shared_audio_source.clone()));
        }
    }

    pub(super) fn start_audio_selection(&mut self, source: AudioShareSource) {
        if !self.connected || self.my_voice_channel().is_none() {
            return;
        }
        self.stop_shared_audio();
        self.shared_audio_source = source;
        self.audio_only = true;
        self.screen_audio_failed = false;
        self.sync_shared_audio();
    }

    pub(super) fn ui_audio_picker(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.audio_picker.as_mut() else {
            return;
        };
        picker.poll();
        let mut open = true;
        let mut close = false;
        let mut start = None;
        let mut stop = false;
        let active = self.screen_audio.is_some();
        egui::Window::new(format!("{}  Share audio", icons::regular::SPEAKER_HIGH))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("Choose what this voice channel hears. Microphone stays independent.");
                ui.add_space(8.0);
                ui.selectable_value(
                    &mut picker.selected,
                    AudioShareSource::System,
                    "System-wide audio",
                );
                ui.label(
                    egui::RichText::new(if cfg!(windows) {
                        "All sound on the selected output device."
                    } else {
                        "All sound on the default system output."
                    })
                    .small()
                    .color(theme::MUTED),
                );
                ui.separator();
                ui.label(egui::RichText::new(if cfg!(windows) {
                    "Per-app audio includes the selected process and its children. Requires Windows 10 build 20348 or newer."
                } else { "Each app playback stream is listed separately. Closed streams are not replaced automatically." }).small().color(theme::MUTED));
                ui.horizontal(|ui| {
                    ui.label("Applications playing audio");
                    if ui
                        .add_enabled(picker.pending.is_none(), egui::Button::new("Refresh"))
                        .clicked()
                    {
                        picker.refresh();
                    }
                });
                if picker.pending.is_some() {
                    ui.spinner();
                }
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for app in &picker.applications {
                            ui.selectable_value(
                                &mut picker.selected,
                                AudioShareSource::Application(app.source.clone()),
                                &app.name,
                            );
                        }
                    });
                if picker.pending.is_none()
                    && picker.applications.is_empty()
                    && picker.error.is_none()
                {
                    ui.label("No apps are playing audio. Start playback, then Refresh.");
                }
                if let Some(error) = &picker.error {
                    ui.colored_label(theme::RED, error);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            picker.selection_available(),
                            egui::Button::new("Share audio"),
                        )
                        .clicked()
                    {
                        start = Some(picker.selected.clone());
                    }
                    if active && ui.button("Stop sharing audio").clicked() {
                        stop = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_selection_needs_no_discovery_or_capture() {
        let picker = AudioPicker {
            selected: AudioShareSource::System,
            applications: vec![],
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
