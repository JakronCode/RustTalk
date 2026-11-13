//! Contents of the settings sections, one function per [`SectionId`].

use super::table::{SectionId, SettingsPage};
use super::*;
use crate::audio::voice_pack::{self, State as PackState};
use theme::{ghost_button, primary_button, setting_row, toggle};

impl RustTalkApp {
    pub(super) fn settings_section(&mut self, ui: &mut egui::Ui, id: SectionId) {
        match id {
            SectionId::Profile => self.section_profile(ui),
            SectionId::SavedServers => self.section_saved_servers(ui),
            SectionId::Theme => section_theme(ui),
            SectionId::PrimaryColor => self.section_primary_color(ui),
            SectionId::Density => self.section_density(ui),
            SectionId::Scaling => self.section_scaling(ui),
            SectionId::Chat => self.section_chat(ui),
            SectionId::Devices => self.section_devices(ui),
            SectionId::InputMode => self.section_input_mode(ui),
            SectionId::MicTest => self.section_mic_test(ui),
            SectionId::VoiceProcessing => self.section_voice_processing(ui),
            SectionId::Codec => section_codec(ui),
            SectionId::Camera => self.section_camera(ui),
            SectionId::Streaming => self.section_streaming(ui),
            SectionId::VoiceKeys => self.section_voice_keys(ui),
            SectionId::CameraKeys => section_camera_keys(ui),
            SectionId::Desktop => self.section_desktop(ui),
            SectionId::Sounds => self.section_sounds(ui),
            SectionId::SpokenNames => self.section_spoken_names(ui),
            SectionId::Encryption => self.section_encryption(ui),
            SectionId::Data => section_data(ui),
            SectionId::Developer => section_developer(ui),
            SectionId::Version => section_version(ui),
            SectionId::Updates => self.updates_section(ui),
            SectionId::Feedback => section_feedback(ui),
        }
    }

    /// "See also" link to a section on another page.
    fn related(&mut self, ui: &mut egui::Ui, text: &str, page: SettingsPage, section: SectionId) {
        ui.add_space(dense(4.0, 2.0));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icons::regular::ARROW_SQUARE_OUT).color(theme::accent()));
            if ui.link(text).clicked() {
                self.goto_settings(page, Some(section));
            }
        });
    }

    // ----- My Account ------------------------------------------------------

    fn section_profile(&mut self, ui: &mut egui::Ui) {
        egui::Frame::default()
            .fill(theme::SIDEBAR)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(dense(16.0, 12.0) as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::draw_avatar(ui, &self.config.username, dense(72.0, 56.0), false);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&self.config.username)
                                .size(20.0)
                                .strong()
                                .color(theme::HEADER),
                        );
                        ui.label(
                            egui::RichText::new("Avatar color is generated from your name")
                                .color(theme::MUTED),
                        );
                        ui.horizontal(|ui| {
                            ui.add_enabled(
                                false,
                                egui::Button::new(format!(
                                    "{}  Upload avatar",
                                    icons::regular::UPLOAD_SIMPLE
                                )),
                            );
                            theme::wip_badge(ui);
                        });
                    });
                });
            });
        ui.add_space(dense(6.0, 2.0));
        setting_row(
            ui,
            "Username",
            Some("Shown to others. Applies on the next connect."),
            false,
            |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.config.username).desired_width(250.0));
            },
        );
        setting_row(
            ui,
            "Status",
            Some("Online, away, do not disturb"),
            true,
            |ui| {
                egui::ComboBox::from_id_salt("status")
                    .selected_text("Online")
                    .show_ui(ui, |_| {});
            },
        );
        setting_row(
            ui,
            "About me",
            Some("Short bio on your profile card"),
            true,
            |ui| {
                let mut s = String::new();
                ui.add(egui::TextEdit::singleline(&mut s).desired_width(250.0));
            },
        );
    }

    fn section_saved_servers(&mut self, ui: &mut egui::Ui) {
        if self.config.bookmarks.is_empty() {
            hint(
                ui,
                "No saved servers yet. Use \"Save server\" on the connect screen.",
            );
        }
        let mut remove = None;
        for (i, bm) in self.config.bookmarks.iter().enumerate() {
            setting_row(ui, &bm.name, Some(&bm.address), false, |ui| {
                if theme::icon_button(
                    ui,
                    icons::regular::TRASH,
                    theme::IconState::Danger,
                    "Remove",
                )
                .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            // A removed server is no longer reconnected at startup.
            let removed = self.config.bookmarks.remove(i);
            self.config.auto_connect.remove(&crate::net::normalize_addr(removed.address.trim()));
            self.config.save();
        }
    }

    // ----- Appearance ------------------------------------------------------

    fn section_primary_color(&mut self, ui: &mut egui::Ui) {
        hint(ui, "Buttons, highlights, selections and links");
        ui.add_space(4.0);
        let before = self.config.accent_color;
        ui.horizontal_wrapped(|ui| {
            for (name, rgb) in theme::ACCENT_PRESETS {
                accent_swatch(ui, name, rgb, &mut self.config.accent_color);
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Custom").color(theme::MUTED));
            ui.color_edit_button_srgb(&mut self.config.accent_color)
                .on_hover_text("Pick any color");
            if self.config.accent_color != crate::config::DEFAULT_ACCENT
                && ghost_button(ui, "Reset").clicked()
            {
                self.config.accent_color = crate::config::DEFAULT_ACCENT;
            }
        });
        if before != self.config.accent_color {
            theme::set_appearance(ui.ctx(), self.config.accent_color, self.config.compact);
        }
    }

    fn section_density(&mut self, ui: &mut egui::Ui) {
        let mut compact = self.config.compact;
        ui.horizontal(|ui| {
            mode_card(
                ui,
                &mut compact,
                false,
                icons::regular::ROWS,
                "Cozy",
                "Roomy, avatars in chat",
            );
            mode_card(
                ui,
                &mut compact,
                true,
                icons::regular::LIST_DASHES,
                "Compact",
                "More fits on screen",
            );
        });
        if compact != self.config.compact {
            self.config.compact = compact;
            theme::set_appearance(ui.ctx(), self.config.accent_color, compact);
        }
    }

    fn section_scaling(&mut self, ui: &mut egui::Ui) {
        setting_row(
            ui,
            "UI scale",
            Some("Zoom the whole interface (also Ctrl + / Ctrl -)"),
            false,
            |ui| {
                let mut pct = (self.config.ui_scale * 100.0).round();
                if ui
                    .add(
                        egui::Slider::new(&mut pct, 75.0..=150.0)
                            .step_by(5.0)
                            .fixed_decimals(0)
                            .suffix(" %"),
                    )
                    .changed()
                {
                    self.config.ui_scale = pct / 100.0;
                    ui.ctx().set_zoom_factor(self.config.ui_scale);
                }
            },
        );
        setting_row(ui, "Chat font size", None, true, |ui| {
            let mut v = 14.0f32;
            ui.add(
                egui::Slider::new(&mut v, 12.0..=20.0)
                    .fixed_decimals(0)
                    .suffix(" px"),
            );
        });
    }

    fn section_chat(&mut self, ui: &mut egui::Ui) {
        setting_row(
            ui,
            "Show timestamps",
            Some("Time next to every message"),
            false,
            |ui| {
                toggle(ui, &mut self.config.show_timestamps);
            },
        );
        setting_row(
            ui,
            "Animated GIFs",
            Some("Play GIFs in chat (off = first frame only)"),
            false,
            |ui| {
                toggle(ui, &mut self.config.animate_gifs);
            },
        );
    }

    // ----- Voice & Audio ---------------------------------------------------

    /// A device was picked: use it right away (level meter and, when
    /// connected, the running audio). Falls back to `previous` if it fails.
    pub(in crate::app) fn apply_devices(&mut self, previous: (Option<String>, Option<String>)) {
        self.stop_shared_audio();
        self.mic_monitor = None; // restarts with the new input next frame
        let voice_channel = self.voice_channel();
        if let Some(engine) = self.audio.as_mut() {
            let (input, output) = (
                self.config.input_device.clone(),
                self.config.output_device.clone(),
            );
            if let Err(e) = engine.restart(input.as_deref(), output.as_deref(), voice_channel) {
                let failed = format!("{e:#}");
                (self.config.input_device, self.config.output_device) = previous;
                let back = engine.restart(
                    self.config.input_device.as_deref(),
                    self.config.output_device.as_deref(),
                    voice_channel,
                );
                if back.is_err() {
                    self.audio = None;
                    let _ = self.start_audio(None);
                }
                self.set_error(match back {
                    Ok(()) => {
                        format!("Could not use that device ({failed}) - kept the previous one")
                    }
                    Err(e) => format!("Voice audio stopped: {e:#} - rejoin voice to try again"),
                });
            }

        } else {
            self.sync_voice_audio();
        }
    }

    fn section_devices(&mut self, ui: &mut egui::Ui) {
        let before = (
            self.config.input_device.clone(),
            self.config.output_device.clone(),
        );
        setting_row(
            ui,
            "Input device",
            Some("Microphone or headset"),
            false,
            |ui| {
                device_combo(
                    ui,
                    "input_dev",
                    &mut self.config.input_device,
                    &self.input_devices,
                );
            },
        );
        setting_row(
            ui,
            "Output device",
            Some("Speakers or headphones"),
            false,
            |ui| {
                device_combo(
                    ui,
                    "output_dev",
                    &mut self.config.output_device,
                    &self.output_devices,
                );
            },
        );
        if before
            != (
                self.config.input_device.clone(),
                self.config.output_device.clone(),
            )
        {
            self.apply_devices(before);
            self.config.save();
        }
        setting_row(
            ui,
            "Output volume",
            Some("Volume of everyone you hear"),
            false,
            |ui| {
                let mut pct = (self.config.output_volume * 100.0).round();
                if ui
                    .add(
                        egui::Slider::new(&mut pct, 0.0..=200.0)
                            .step_by(5.0)
                            .fixed_decimals(0)
                            .suffix(" %"),
                    )
                    .changed()
                {
                    self.config.output_volume = pct / 100.0;
                    self.control.set_output_volume(self.config.output_volume);
                }
            },
        );
        setting_row(
            ui,
            "Input volume",
            Some("Manual microphone gain (before noise suppression and auto gain)"),
            false,
            |ui| {
                let mut pct = (self.config.input_volume * 100.0).round();
                if ui
                    .add(
                        egui::Slider::new(&mut pct, 0.0..=200.0)
                            .step_by(5.0)
                            .fixed_decimals(0)
                            .suffix(" %"),
                    )
                    .changed()
                {
                    self.config.input_volume = pct / 100.0;
                    self.control.set_input_volume(self.config.input_volume);
                }
            },
        );
    }

    fn section_input_mode(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut va = self.config.voice_activation;
            mode_card(
                ui,
                &mut va,
                false,
                icons::regular::HAND_TAP,
                "Push to Talk",
                "Hold a key to talk",
            );
            mode_card(
                ui,
                &mut va,
                true,
                icons::regular::WAVEFORM,
                "Voice Activity",
                "Talk when you speak",
            );
            if va != self.config.voice_activation {
                self.config.voice_activation = va;
                self.control.voice_activation.store(va, Ordering::Relaxed);
            }
        });
        ui.add_space(dense(4.0, 2.0));
        if self.config.voice_activation {
            setting_row(
                ui,
                "Sensitivity",
                Some("Below the white marker in the mic test nothing is sent"),
                false,
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut self.config.vad_threshold, 0.001..=0.2)
                                .logarithmic(true)
                                .show_value(false),
                        )
                        .changed()
                    {
                        self.control.set_vad_threshold(self.config.vad_threshold);
                    }
                },
            );
        } else {
            self.ptt_settings(ui);
        }
        self.related(
            ui,
            "Mute and deafen shortcuts are under Keybinds",
            SettingsPage::Keybinds,
            SectionId::VoiceKeys,
        );
    }

    fn section_mic_test(&mut self, ui: &mut egui::Ui) {
        let capture_running = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.voice_channel.is_some());
        let level = if capture_running {
            self.control.mic_level()
        } else {
            self.mic_monitor.as_ref().map(|m| m.level()).unwrap_or(0.0)
        };
        ui.add_space(4.0);
        theme::mic_meter(ui, level, self.config.vad_threshold);
        if !capture_running && self.mic_monitor.is_none() {
            ui.label(
                egui::RichText::new("No microphone access (monitor could not start).")
                    .color(theme::RED),
            );
        } else {
            hint(
                ui,
                "This page opens your microphone for a local level preview outside voice channels. Green = above the voice threshold.",
            );
        }
    }

    fn section_voice_processing(&mut self, ui: &mut egui::Ui) {
        setting_row(
            ui,
            "Noise suppression",
            Some("AI noise filter (RNNoise) removes keyboard, fans and hum"),
            false,
            |ui| {
                if toggle(ui, &mut self.config.noise_suppression).changed() {
                    self.control
                        .noise_suppression
                        .store(self.config.noise_suppression, Ordering::Relaxed);
                }
            },
        );
        setting_row(
            ui,
            "Automatic gain control",
            Some("Boosts quiet microphones and tames loud ones"),
            false,
            |ui| {
                if toggle(ui, &mut self.config.auto_gain).changed() {
                    self.control
                        .auto_gain
                        .store(self.config.auto_gain, Ordering::Relaxed);
                }
            },
        );
        setting_row(
            ui,
            "Echo cancellation",
            Some("Lets you use speakers instead of headphones (others don't hear themselves)"),
            false,
            |ui| {
                if toggle(ui, &mut self.config.echo_cancellation).changed() {
                    self.control
                        .echo_cancellation
                        .store(self.config.echo_cancellation, Ordering::Relaxed);
                }
            },
        );
    }

    // ----- Video -----------------------------------------------------------

    fn section_camera(&mut self, ui: &mut egui::Ui) {
        if self.cameras.is_none() {
            self.cameras = Some(video::list_cameras().unwrap_or_default());
        }
        let cameras = self.cameras.clone().unwrap_or_default();
        let desc = if cameras.is_empty() {
            "No camera found"
        } else {
            "Preselected when you turn on your camera"
        };
        setting_row(ui, "Default camera", Some(desc), false, |ui| {
            let current = self
                .config
                .camera
                .clone()
                .unwrap_or_else(|| "First available".into());
            egui::ComboBox::from_id_salt("default_cam")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.config.camera.is_none(), "First available")
                        .clicked()
                    {
                        self.config.camera = None;
                    }
                    for cam in &cameras {
                        if ui
                            .selectable_label(
                                self.config.camera.as_deref() == Some(&cam.name),
                                &cam.name,
                            )
                            .clicked()
                        {
                            self.config.camera = Some(cam.name.clone());
                        }
                    }
                });
            // Left of the combo, so the combo lines up with the others.
            if ghost_button(ui, icons::regular::ARROWS_CLOCKWISE)
                .on_hover_text("Refresh devices")
                .clicked()
            {
                self.cameras = None;
            }
        });
        setting_row(
            ui,
            "Mirror my camera",
            Some("Show your own preview mirrored"),
            true,
            |ui| {
                toggle(ui, &mut true.clone());
            },
        );
        setting_row(ui, "Background blur", None, true, |ui| {
            toggle(ui, &mut false.clone());
        });
    }

    fn section_streaming(&mut self, ui: &mut egui::Ui) {
        setting_row(ui, "Stream quality", Some("Camera and screen share. \"High\" for LAN, \"Best\" for big screens on fast connections, \"Balanced\" for Wi-Fi. Applies the next time you start your camera or screen share."), false, |ui| {
            egui::ComboBox::from_id_salt("video_quality")
                .selected_text(self.config.video_quality.label())
                .show_ui(ui, |ui| {
                    for q in [video::VideoQuality::Best, video::VideoQuality::High, video::VideoQuality::Balanced] {
                        ui.selectable_value(&mut self.config.video_quality, q, q.label());
                    }
                });
        });
        setting_row(
            ui,
            "Share audio with screen",
            Some("Send computer sound while sharing. Linux captures all sound on the default system output, including for a single shared window; requires libpulse and PulseAudio or pipewire-pulse."),
            !cfg!(any(windows, target_os = "linux")),
            |ui| {
                toggle(ui, &mut self.config.share_screen_audio);
            },
        );
        setting_row(
            ui,
            "Hardware encoding",
            Some("H.264 via GPU instead of MJPEG"),
            true,
            |ui| {
                toggle(ui, &mut false.clone());
            },
        );
    }

    // ----- Keybinds --------------------------------------------------------

    fn section_voice_keys(&mut self, ui: &mut egui::Ui) {
        self.ptt_settings(ui);
        for action in [hotkey::Action::Mute, hotkey::Action::Deafen] {
            setting_row(
                ui,
                action.label(),
                Some("Global shortcut — press once to toggle"),
                false,
                |ui| {
                    self.keybind_button(ui, action);
                    if self.binding_name(action).is_some() && ghost_button(ui, "Clear").clicked() {
                        match action {
                            hotkey::Action::Mute => self.config.mute_key = None,
                            hotkey::Action::Deafen => self.config.deafen_key = None,
                            _ => {}
                        }
                        self.recording_key = None;
                        self.config.save();
                        self.global_ptt.lock().unwrap().set([None; 3]);
                        self.control.ptt_pressed.store(false, Ordering::Relaxed);
                    }
                },
            );
        }
        #[cfg(target_os = "linux")]
        if !self.config.global_ptt
            && (self.config.mute_key.is_some() || self.config.deafen_key.is_some())
        {
            let status = if self.any_connected() {
                hotkey::status()
            } else {
                "Connect to a server to activate global shortcuts.".to_string()
            };
            if !status.is_empty() {
                hint(ui, &status);
            }
        }
        self.related(
            ui,
            "Input mode and microphone are under Voice & Audio",
            SettingsPage::Voice,
            SectionId::InputMode,
        );
    }

    // Keep both settings pages consistent: Wayland bindings belong to the desktop.
    fn ptt_settings(&mut self, ui: &mut egui::Ui) {
        setting_row(
            ui,
            "Push to talk in the background",
            Some("Hold to talk even while another app is focused"),
            false,
            |ui| {
                if toggle(ui, &mut self.config.global_ptt).changed() {
                    self.config.save();
                }
            },
        );
        let description = if self.config.global_ptt {
            "Hold to talk from any app"
        } else {
            "Only while the RustTalk window is focused"
        };
        setting_row(ui, "Push-to-talk key", Some(description), false, |ui| {
            self.keybind_button(ui, hotkey::Action::PushToTalk);
        });
        #[cfg(target_os = "linux")]
        if self.config.global_ptt {
            if self.any_connected() {
                let status = hotkey::status();
                if !status.is_empty() {
                    hint(ui, &status);
                }
            } else {
                hint(ui, "Connect to a server to activate this global shortcut.");
            }
        }
        if let Some(error) = &self.keybind_error {
            ui.colored_label(theme::RED, error);
        }
    }

    // ----- Notifications ---------------------------------------------------

    fn section_desktop(&mut self, ui: &mut egui::Ui) {
        if crate::notify::TASKBAR_ATTENTION {
            setting_row(
                ui,
                "Flash taskbar",
                Some("Highlight the taskbar icon on new messages while RustTalk is in the background"),
                false,
                |ui| {
                    toggle(ui, &mut self.config.flash_taskbar);
                },
            );
        }
        setting_row(ui, "Desktop notifications", Some("Desktop pop-up for new messages while RustTalk is in the background (click opens the channel)"), false, |ui| {
            toggle(ui, &mut self.config.desktop_notifications);
        });
        ui.add_enabled_ui(self.config.desktop_notifications, |ui| {
            setting_row(
                ui,
                "Show message text",
                Some("Off = the pop-up only says \"New message\""),
                false,
                |ui| {
                    toggle(ui, &mut self.config.notification_preview);
                },
            );
        });
    }

    fn section_sounds(&mut self, ui: &mut egui::Ui) {
        let rows: [(&str, Option<&str>, Sound, &mut bool); 4] = [
            ("User joins your channel", None, Sound::Join, &mut self.config.sound_join),
            ("User leaves your channel", None, Sound::Leave, &mut self.config.sound_leave),
            ("New message", Some("In another channel or while RustTalk is in the background. @mentions always ring, with their own sound."), Sound::Message, &mut self.config.sound_message),
            ("Mute / unmute", Some("Also for deafen"), Sound::Mute, &mut self.config.sound_mute),
        ];
        let mut preview = None;
        for (label, desc, sound, value) in rows {
            setting_row(ui, label, desc, false, |ui| {
                toggle(ui, value);
                if ghost_button(ui, icons::regular::PLAY)
                    .on_hover_text("Preview (while connected)")
                    .clicked()
                {
                    preview = Some(sound);
                }
            });
        }
        if let Some(sound) = preview {
            if self.audio.is_none() {
                self.set_status("Connect to a server to preview sounds");
            } else {
                crate::audio::sfx::play(&self.sfx, sound);
                if sound == Sound::Message {
                    // The @mention sound shares this switch: play it right after.
                    crate::audio::sfx::play(&self.sfx, Sound::Mention);
                }
            }
        }
    }

    fn section_spoken_names(&mut self, ui: &mut egui::Ui) {
        let mut preview_name = false;
        let mut voice_changed = false;
        setting_row(
            ui,
            "Say names",
            Some("Speak the user's name after the join / leave sound, e.g. \"Alex joined\""),
            false,
            |ui| {
                voice_changed |= toggle(ui, &mut self.config.announce_names).changed();
                if ghost_button(ui, icons::regular::PLAY)
                    .on_hover_text("Preview with your own name (while connected)")
                    .clicked()
                {
                    preview_name = true;
                }
            },
        );
        let voices = crate::audio::speech::voices();
        let pack = self.voice_pack.state();
        let pack_ready = pack == PackState::Installed;
        let natural = self
            .config
            .announce_voice
            .as_deref()
            .is_some_and(|v| voice_pack::neural_id(v).is_some())
            || (!cfg!(windows) && self.config.announce_voice.is_none());
        ui.add_enabled_ui(self.config.announce_names, |ui| {
            setting_row(ui, "Voice", Some("Natural voices work offline after a one-time download"), false, |ui| {
                let current = match &self.config.announce_voice {
                    Some(v) if natural && !pack_ready => format!("{} (not installed)", voice_pack::label(v)),
                    Some(v) => voice_pack::label(v),
                    None if !cfg!(windows) && !pack_ready => "Download natural voices below".to_string(),
                    None if !cfg!(windows) => voice_pack::label(voice_pack::DEFAULT_VOICE),
                    None if voices.is_empty() => "No Windows voice installed".to_string(),
                    None => "Windows default".to_string(),
                };
                let before = self.config.announce_voice.clone();
                egui::ComboBox::from_id_salt("announce_voice").selected_text(current).height(360.0).show_ui(ui, |ui| {
                    if pack_ready || !cfg!(windows) {
                        for (id, label) in voice_pack::VOICES {
                            ui.selectable_value(&mut self.config.announce_voice, Some(format!("{}{id}", voice_pack::PREFIX)), label);
                        }
                        ui.separator();
                    }
                    ui.selectable_value(&mut self.config.announce_voice, None,
                        if cfg!(windows) { "Windows default" } else { "Natural default" });
                    for v in voices {
                        ui.selectable_value(&mut self.config.announce_voice, Some(v.name.clone()), format!("{} ({})", v.name, v.language));
                    }
                });
                if self.config.announce_voice != before {
                    voice_changed = true;
                    preview_name = true;
                }
            });
            ui.add_enabled_ui(natural && pack_ready, |ui| {
                setting_row(ui, "Language", Some("What natural voices say: \"Alex joined\" or \"Alex ist beigetreten\". Windows voices use their own language."), false, |ui| {
                    let before = self.config.announce_language.clone();
                    let current = if before == "de" { "Deutsch" } else { "English" };
                    egui::ComboBox::from_id_salt("announce_language").selected_text(current).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.config.announce_language, "en".to_string(), "English");
                        ui.selectable_value(&mut self.config.announce_language, "de".to_string(), "Deutsch");
                    });
                    if self.config.announce_language != before {
                        voice_changed = true;
                        preview_name = true;
                    }
                });
            });
        });
        let size_mb = voice_pack::total_size() / 1_000_000;
        let desc = match &pack {
            PackState::Missing if self.config.auto_download_voices && crate::install::mode() == crate::install::Mode::Installed => {
                format!("10 voices that sound like a person, speak English and German, and work offline. Downloads on its own in the background ({size_mb} MB, once).")
            }
            PackState::Missing => format!("10 voices that sound like a person, speak English and German, and work offline. One-time download of {size_mb} MB."),
            PackState::Downloading { done, total } => format!("Downloading... {} of {} MB", done / 1_000_000, total / 1_000_000),
            PackState::Failed(e) => format!("Download failed: {e}"),
            PackState::Installed => if cfg!(windows) {
                format!("Installed ({size_mb} MB). Removing them switches back to the Windows voice.")
            } else {
                format!("Installed ({size_mb} MB). Removing them disables spoken names.")
            },
        };
        let mut download = false;
        let mut remove = false;
        setting_row(ui, "Natural voices", Some(&desc), false, |ui| match &pack {
            PackState::Missing => download = primary_button(ui, "Download").clicked(),
            PackState::Failed(_) => download = primary_button(ui, "Try again").clicked(),
            PackState::Downloading { done, total } => {
                ui.add(
                    egui::ProgressBar::new(*done as f32 / (*total).max(1) as f32)
                        .desired_width(140.0)
                        .show_percentage(),
                );
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
            PackState::Installed => remove = ui.button("Remove").clicked(),
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Supertonic 3 model by Supertone Inc., used under the")
                    .small()
                    .weak(),
            );
            ui.hyperlink_to(
                egui::RichText::new("OpenRAIL-M license").small(),
                voice_pack::LICENSE_URL,
            );
            ui.label(
                egui::RichText::new("- no impersonating people, no harassment, nothing unlawful.")
                    .small()
                    .weak(),
            );
        });
        #[cfg(windows)]
        setting_row(ui, "Download natural voices automatically", Some("After installing, RustTalk fetches them once in the background and switches to a natural voice"), false, |ui| {
            if toggle(ui, &mut self.config.auto_download_voices).changed() {
                self.config.save();
            }
        });
        if download {
            self.voice_pack.download();
        }
        if remove {
            self.voice_pack.remove();
            if natural {
                self.config.announce_voice = None;
                voice_changed = true;
            }
        }
        if voice_changed {
            self.config.save();
            self.prepare_all_names();
        }
        if preview_name {
            if !cfg!(windows) && !pack_ready {
                self.set_status("Download natural voices below to preview spoken names");
            } else if self.audio.is_none() {
                self.set_status("Connect to a server to preview sounds");
            } else {
                crate::audio::sfx::play(&self.sfx, Sound::Join);
                self.announcer.announce(
                    &self.config.username,
                    true,
                    self.config.announce_voice.as_deref(),
                    &self.config.announce_language,
                );
            }
        }
    }

    // ----- Advanced --------------------------------------------------------

    fn section_encryption(&mut self, ui: &mut egui::Ui) {
        if self.is_connected() && !self.srv.server_fingerprint.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "{}  Connected encrypted (TLS + encrypted voice)",
                    icons::regular::LOCK
                ))
                .color(theme::GREEN),
            );
            ui.label(
                egui::RichText::new(format!("Server fingerprint: {}", self.srv.server_fingerprint))
                    .monospace()
                    .size(11.0)
                    .color(theme::MUTED),
            );
            ui.add_space(dense(8.0, 4.0));
        }
        hint(
            ui,
            "Known servers (their certificate is checked on every connect):",
        );
        if self.config.known_servers.is_empty() {
            hint(ui, "None yet.");
        }
        let mut forget = None;
        for (addr, fp) in &self.config.known_servers {
            let short: String = fp.chars().take(23).collect();
            setting_row(ui, addr, Some(&format!("{short}...")), false, |ui| {
                if ghost_button(ui, "Forget")
                    .on_hover_text("Trust whatever certificate it has on the next connect")
                    .clicked()
                {
                    forget = Some(addr.clone());
                }
                ui.label(egui::RichText::new(icons::regular::FINGERPRINT).color(theme::MUTED))
                    .on_hover_text(fp);
            });
        }
        if let Some(a) = forget {
            self.config.known_servers.remove(&a);
            self.config.save();
        }
    }
}

// ----- sections without app state ------------------------------------------

fn section_theme(ui: &mut egui::Ui) {
    hint(ui, "Only the dark theme is available for now.");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        theme_swatch(ui, "Dark", theme::BG, true, true);
        theme_swatch(
            ui,
            "Light",
            egui::Color32::from_rgb(0xF2, 0xF3, 0xF5),
            false,
            false,
        );
        theme_swatch(
            ui,
            "Midnight",
            egui::Color32::from_rgb(0x0B, 0x0C, 0x0E),
            false,
            false,
        );
        theme::wip_badge(ui);
    });
}

fn section_codec(ui: &mut egui::Ui) {
    hint(ui, "Opus 64 kbit/s, 48 kHz mono, 20 ms frames, in-band FEC");
    setting_row(
        ui,
        "Audio quality",
        Some("Bitrate per voice channel"),
        true,
        |ui| {
            egui::ComboBox::from_id_salt("bitrate")
                .selected_text("64 kbit/s")
                .show_ui(ui, |_| {});
        },
    );
}

fn section_camera_keys(ui: &mut egui::Ui) {
    setting_row(
        ui,
        "Toggle camera",
        Some("Turn your camera on or off"),
        true,
        |ui| {
            ui.add(egui::Button::new("Not set").min_size(egui::vec2(140.0, 28.0)));
        },
    );
}

fn section_data(ui: &mut egui::Ui) {
    let path = std::path::absolute(ClientConfig::path()).unwrap_or_else(|_| ClientConfig::path());
    setting_row(
        ui,
        "Settings file",
        Some(&path.display().to_string()),
        false,
        |ui| {
            if ui
                .button(format!("{}  Show in folder", icons::regular::FOLDER_OPEN))
                .clicked()
            {
                let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        },
    );
    setting_row(
        ui,
        "Reset all settings",
        Some("Back to defaults (keeps saved servers)"),
        true,
        |ui| {
            theme::danger_button(ui, "Reset");
        },
    );
}

fn section_developer(ui: &mut egui::Ui) {
    setting_row(
        ui,
        "Debug overlay",
        Some("Network, jitter buffer and codec statistics"),
        true,
        |ui| {
            toggle(ui, &mut false.clone());
        },
    );
    setting_row(ui, "Log level", None, true, |ui| {
        egui::ComboBox::from_id_salt("log_level")
            .selected_text("warn")
            .show_ui(ui, |_| {});
    });
}

fn section_version(ui: &mut egui::Ui) {
    egui::Frame::default()
        .fill(theme::SIDEBAR)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(dense(16.0, 10.0) as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                logo::paint_logo(ui, dense(72.0, 56.0));
                ui.label(
                    egui::RichText::new("RustTalk")
                        .size(26.0)
                        .strong()
                        .color(theme::HEADER),
                );
                ui.label(
                    egui::RichText::new("Lightweight voice chat. Local first. No accounts.")
                        .color(theme::MUTED),
                );
            });
        });
    ui.add_space(dense(6.0, 2.0));
    for (k, v) in [
        ("Version", env!("CARGO_PKG_VERSION").to_string()),
        (
            "Protocol",
            format!("v{}", rusttalk_shared::protocol::PROTOCOL_VERSION),
        ),
        ("Voice", "Opus 64 kbit/s, RNNoise, FEC".to_string()),
        ("Video", "MJPEG up to 1080p / 25 fps".to_string()),
        ("UI", "egui + Phosphor icons".to_string()),
    ] {
        setting_row(ui, k, None, false, |ui| {
            ui.label(egui::RichText::new(v).color(theme::TEXT));
        });
    }
}

fn section_feedback(ui: &mut egui::Ui) {
    setting_row(
        ui,
        "Found a bug?",
        Some("Opens a new GitHub issue with your version pre-filled"),
        false,
        |ui| {
            if ui
                .button(format!("{}  Report a Bug", icons::regular::BUG))
                .clicked()
            {
                super::super::server_settings::open_bug_report(ui.ctx());
            }
        },
    );
}

/// Muted explanatory text under a section heading.
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(13.0).color(theme::MUTED));
}

fn device_combo(ui: &mut egui::Ui, id: &str, selected: &mut Option<String>, devices: &[String]) {
    let text = selected.clone().unwrap_or_else(|| "System default".into());
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(selected.is_none(), "System default")
                .clicked()
            {
                *selected = None;
            }
            for name in devices {
                if ui
                    .selectable_label(selected.as_deref() == Some(name), name)
                    .clicked()
                {
                    *selected = Some(name.clone());
                }
            }
        });
}

/// Round color swatch; click selects `rgb`.
fn accent_swatch(ui: &mut egui::Ui, name: &str, rgb: [u8; 3], current: &mut [u8; 3]) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let p = ui.painter();
    p.circle_filled(rect.center(), 12.0, color);
    if *current == rgb {
        p.circle_stroke(
            rect.center(),
            13.5,
            egui::Stroke::new(2.0_f32, theme::HEADER),
        );
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icons::regular::CHECK,
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
    } else if resp.hovered() {
        p.circle_stroke(
            rect.center(),
            13.5,
            egui::Stroke::new(1.5_f32, theme::MUTED),
        );
    }
    if resp
        .on_hover_text(name)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *current = rgb;
    }
}

/// Large selectable card (input mode, density).
fn mode_card(ui: &mut egui::Ui, value: &mut bool, this: bool, icon: &str, title: &str, desc: &str) {
    let selected = *value == this;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(220.0, dense(64.0, 54.0)), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        8.0,
        if selected {
            theme::SELECTED
        } else if resp.hovered() {
            theme::HOVER
        } else {
            theme::SIDEBAR
        },
    );
    if selected {
        p.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(2.0_f32, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    let mid = rect.center().y;
    p.text(
        rect.left_center() + egui::vec2(20.0, 0.0),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(22.0),
        if selected {
            theme::accent()
        } else {
            theme::MUTED
        },
    );
    p.text(
        egui::pos2(rect.left() + 40.0, mid - 1.0),
        egui::Align2::LEFT_BOTTOM,
        title,
        egui::FontId::proportional(14.5),
        theme::HEADER,
    );
    p.text(
        egui::pos2(rect.left() + 40.0, mid + 2.0),
        egui::Align2::LEFT_TOP,
        desc,
        egui::FontId::proportional(12.0),
        theme::MUTED,
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *value = this;
    }
}

fn theme_swatch(
    ui: &mut egui::Ui,
    name: &str,
    color: egui::Color32,
    selected: bool,
    enabled: bool,
) {
    ui.vertical(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 44.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 8.0, color);
        let stroke = if selected {
            egui::Stroke::new(2.0_f32, theme::accent())
        } else {
            egui::Stroke::new(1.0_f32, theme::BORDER)
        };
        ui.painter()
            .rect_stroke(rect, 8.0, stroke, egui::StrokeKind::Inside);
        if selected {
            ui.painter().text(
                rect.right_top() + egui::vec2(-10.0, 10.0),
                egui::Align2::CENTER_CENTER,
                icons::regular::CHECK_CIRCLE,
                egui::FontId::proportional(14.0),
                theme::accent(),
            );
        }
        ui.label(egui::RichText::new(name).size(12.0).color(if enabled {
            theme::HEADER
        } else {
            theme::DISABLED
        }));
    });
}
