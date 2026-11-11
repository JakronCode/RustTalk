//! App settings: separate window with a category sidebar
//! (account, appearance, voice, video, keybinds, notifications, advanced,
//! about). Placeholders are clearly marked with a WIP badge.

use super::theme::{self, card, caption, setting_row, toggle};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SettingsPage {
    Account,
    Appearance,
    Voice,
    Video,
    Keybinds,
    Notifications,
    Advanced,
    About,
}

impl SettingsPage {
    fn label(self) -> &'static str {
        match self {
            SettingsPage::Account => "My Account",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Voice => "Voice & Audio",
            SettingsPage::Video => "Video",
            SettingsPage::Keybinds => "Keybinds",
            SettingsPage::Notifications => "Notifications",
            SettingsPage::Advanced => "Advanced",
            SettingsPage::About => "About RustTalk",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            SettingsPage::Account => icons::regular::USER_CIRCLE,
            SettingsPage::Appearance => icons::regular::PAINT_BRUSH,
            SettingsPage::Voice => icons::regular::MICROPHONE,
            SettingsPage::Video => icons::regular::VIDEO_CAMERA,
            SettingsPage::Keybinds => icons::regular::KEYBOARD,
            SettingsPage::Notifications => icons::regular::BELL,
            SettingsPage::Advanced => icons::regular::SLIDERS_HORIZONTAL,
            SettingsPage::About => icons::regular::INFO,
        }
    }

    /// Navigation groups.
    const GROUPS: [(&'static str, &'static [SettingsPage]); 3] = [
        ("User Settings", &[SettingsPage::Account, SettingsPage::Appearance]),
        (
            "App Settings",
            &[
                SettingsPage::Voice,
                SettingsPage::Video,
                SettingsPage::Keybinds,
                SettingsPage::Notifications,
                SettingsPage::Advanced,
            ],
        ),
        ("Info", &[SettingsPage::About]),
    ];
}

impl RustTalkApp {
    pub(super) fn open_settings(&mut self, page: SettingsPage) {
        self.input_devices = capture::input_device_names();
        self.output_devices = playback::output_device_names();
        self.cameras = None; // refresh lazily on the video page
        self.settings_page = page;
        self.show_settings = true;
    }

    /// Settings as a separate OS window (own viewport).
    pub(super) fn ui_settings(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("settings_viewport"),
            egui::ViewportBuilder::default()
                .with_title("RustTalk - Settings")
                .with_inner_size([900.0, 640.0])
                .with_min_inner_size([760.0, 520.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    self.show_settings = false;
                    self.recording_key = false;
                    self.config.save();
                }
                self.capture_keybind(ctx);
                egui::SidePanel::left("settings_nav")
                    .exact_width(220.0)
                    .resizable(false)
                    .frame(egui::Frame::default().fill(theme::SIDEBAR).inner_margin(egui::Margin { left: 12, right: 12, top: 20, bottom: 12 }))
                    .show(ctx, |ui| self.settings_nav(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 28, right: 28, top: 20, bottom: 12 }))
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(self.settings_page.label()).size(22.0).strong().color(theme::HEADER));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let close = ui
                                    .add(egui::Button::new(egui::RichText::new(icons::regular::X).size(18.0)).corner_radius(16.0).min_size(egui::vec2(32.0, 32.0)))
                                    .on_hover_text("Close (Esc)");
                                if close.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape) && !self.recording_key) {
                                    self.show_settings = false;
                                    self.config.save();
                                }
                            });
                        });
                        ui.add_space(12.0);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            match self.settings_page {
                                SettingsPage::Account => self.page_account(ui),
                                SettingsPage::Appearance => self.page_appearance(ui),
                                SettingsPage::Voice => self.page_voice(ui),
                                SettingsPage::Video => self.page_video(ui),
                                SettingsPage::Keybinds => self.page_keybinds(ui),
                                SettingsPage::Notifications => self.page_notifications(ui),
                                SettingsPage::Advanced => self.page_advanced(ui),
                                SettingsPage::About => self.page_about(ui),
                            }
                            ui.add_space(20.0);
                        });
                    });
            },
        );
    }

    fn settings_nav(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (group, pages) in SettingsPage::GROUPS {
            ui.add_space(6.0);
            caption(ui, group);
            ui.add_space(2.0);
            for &page in pages {
                let selected = self.settings_page == page;
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
                let bg = if selected {
                    theme::SELECTED
                } else if resp.hovered() {
                    theme::HOVER
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 6.0, bg);
                let fg = if selected || resp.hovered() { theme::HEADER } else { theme::MUTED };
                ui.painter().text(rect.left_center() + egui::vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, page.icon(), egui::FontId::proportional(16.0), fg);
                ui.painter().text(rect.left_center() + egui::vec2(34.0, 0.0), egui::Align2::LEFT_CENTER, page.label(), egui::FontId::proportional(14.0), fg);
                if resp.clicked() {
                    self.settings_page = page;
                    self.recording_key = false;
                }
            }
            ui.add_space(8.0);
            ui.separator();
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.label(egui::RichText::new(format!("RustTalk v{}  -  protocol v{}", env!("CARGO_PKG_VERSION"), rusttalk_shared::protocol::PROTOCOL_VERSION)).size(11.0).color(theme::DISABLED));
        });
    }

    // ----- pages -----------------------------------------------------------

    fn page_account(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let r = theme::draw_avatar(ui, &self.config.username, 72.0, false);
                let _ = r;
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&self.config.username).size(20.0).strong().color(theme::HEADER));
                    ui.label(egui::RichText::new("Avatar color is generated from your name").color(theme::MUTED));
                    ui.horizontal(|ui| {
                        ui.add_enabled(false, egui::Button::new(format!("{}  Upload avatar", icons::regular::UPLOAD_SIMPLE)));
                        theme::wip_badge(ui);
                    });
                });
            });
        });
        card(ui, |ui| {
            caption(ui, "Profile");
            setting_row(ui, "Username", Some("Shown to others. Applies on the next connect."), false, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.config.username).desired_width(220.0));
            });
            ui.separator();
            setting_row(ui, "Status", Some("Online, away, do not disturb"), true, |ui| {
                egui::ComboBox::from_id_salt("status").selected_text("Online").show_ui(ui, |_| {});
            });
            ui.separator();
            setting_row(ui, "About me", Some("Short bio on your profile card"), true, |ui| {
                let mut s = String::new();
                ui.add(egui::TextEdit::singleline(&mut s).desired_width(220.0));
            });
        });
        card(ui, |ui| {
            caption(ui, "Saved servers");
            if self.config.bookmarks.is_empty() {
                ui.label(egui::RichText::new("No saved servers yet. Use \"Save server\" on the connect screen.").color(theme::MUTED));
            }
            let mut remove = None;
            for (i, bm) in self.config.bookmarks.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icons::regular::HARD_DRIVES).color(theme::MUTED));
                    ui.label(egui::RichText::new(&bm.name).color(theme::HEADER));
                    ui.label(egui::RichText::new(&bm.address).color(theme::MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new(icons::regular::TRASH).color(theme::RED)).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove {
                self.config.bookmarks.remove(i);
                self.config.save();
            }
        });
    }

    fn page_appearance(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            caption(ui, "Theme");
            ui.horizontal(|ui| {
                theme_swatch(ui, "Dark", theme::BG, true, true);
                theme_swatch(ui, "Light", egui::Color32::from_rgb(0xF2, 0xF3, 0xF5), false, false);
                theme_swatch(ui, "Midnight", egui::Color32::from_rgb(0x0B, 0x0C, 0x0E), false, false);
                theme::wip_badge(ui);
            });
        });
        card(ui, |ui| {
            caption(ui, "Scaling");
            setting_row(ui, "UI scale", Some("Zoom the whole interface (also Ctrl + / Ctrl -)"), false, |ui| {
                let mut pct = (self.config.ui_scale * 100.0).round();
                if ui.add(egui::Slider::new(&mut pct, 75.0..=150.0).step_by(5.0).fixed_decimals(0).suffix(" %")).changed() {
                    self.config.ui_scale = pct / 100.0;
                    ui.ctx().set_zoom_factor(self.config.ui_scale);
                }
            });
            ui.separator();
            setting_row(ui, "Chat font size", None, true, |ui| {
                let mut v = 14.0f32;
                ui.add(egui::Slider::new(&mut v, 12.0..=20.0).fixed_decimals(0).suffix(" px"));
            });
        });
        card(ui, |ui| {
            caption(ui, "Chat");
            setting_row(ui, "Show timestamps", Some("Time next to every message"), false, |ui| {
                toggle(ui, &mut self.config.show_timestamps);
            });
            ui.separator();
            setting_row(ui, "Compact mode", Some("Denser message list"), true, |ui| {
                toggle(ui, &mut false.clone());
            });
            ui.separator();
            setting_row(ui, "Animated GIFs", Some("Play GIFs in chat (off = first frame only)"), false, |ui| {
                toggle(ui, &mut self.config.animate_gifs);
            });
        });
    }

    fn page_voice(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            caption(ui, "Devices");
            ui.columns(2, |cols| {
                cols[0].label(egui::RichText::new("Input device").color(theme::HEADER));
                device_combo(&mut cols[0], "input_dev", &mut self.config.input_device, &self.input_devices);
                cols[1].label(egui::RichText::new("Output device").color(theme::HEADER));
                device_combo(&mut cols[1], "output_dev", &mut self.config.output_device, &self.output_devices);
            });
            ui.label(egui::RichText::new(format!("{}  Device changes apply on the next connect.", icons::regular::INFO)).size(12.0).color(theme::MUTED));
            ui.separator();
            setting_row(ui, "Output volume", Some("Volume of everyone you hear"), false, |ui| {
                let mut pct = (self.config.output_volume * 100.0).round();
                if ui.add(egui::Slider::new(&mut pct, 0.0..=200.0).step_by(5.0).fixed_decimals(0).suffix(" %")).changed() {
                    self.config.output_volume = pct / 100.0;
                    self.control.set_output_volume(self.config.output_volume);
                }
            });
            ui.separator();
            setting_row(ui, "Input volume", Some("Manual microphone gain (before noise suppression and auto gain)"), false, |ui| {
                let mut pct = (self.config.input_volume * 100.0).round();
                if ui.add(egui::Slider::new(&mut pct, 0.0..=200.0).step_by(5.0).fixed_decimals(0).suffix(" %")).changed() {
                    self.config.input_volume = pct / 100.0;
                    self.control.set_input_volume(self.config.input_volume);
                }
            });
        });

        card(ui, |ui| {
            caption(ui, "Input mode");
            ui.horizontal(|ui| {
                let mut va = self.config.voice_activation;
                mode_card(ui, &mut va, false, icons::regular::HAND_TAP, "Push to Talk", "Hold a key to talk");
                mode_card(ui, &mut va, true, icons::regular::WAVEFORM, "Voice Activity", "Talk when you speak");
                if va != self.config.voice_activation {
                    self.config.voice_activation = va;
                    self.control.voice_activation.store(va, Ordering::Relaxed);
                }
            });
            ui.add_space(4.0);
            if self.config.voice_activation {
                setting_row(ui, "Sensitivity", Some("Below the white marker nothing is sent"), false, |ui| {
                    if ui
                        .add(egui::Slider::new(&mut self.config.vad_threshold, 0.001..=0.2).logarithmic(true).show_value(false))
                        .changed()
                    {
                        self.control.set_vad_threshold(self.config.vad_threshold);
                    }
                });
            } else {
                setting_row(ui, "Push-to-talk key", Some("Only while the RustTalk window is focused"), false, |ui| {
                    self.keybind_button(ui);
                });
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Mic test").color(theme::HEADER));
            let level = if self.audio.is_some() {
                self.control.mic_level()
            } else {
                self.mic_monitor.as_ref().map(|m| m.level()).unwrap_or(0.0)
            };
            theme::mic_meter(ui, level, self.config.vad_threshold);
            if self.audio.is_none() && self.mic_monitor.is_none() {
                ui.label(egui::RichText::new("No microphone access (monitor could not start).").color(theme::RED));
            } else {
                ui.label(egui::RichText::new("Speak to see your level. Green = above the threshold (sent).").size(12.0).color(theme::MUTED));
            }
        });

        card(ui, |ui| {
            caption(ui, "Voice processing");
            setting_row(ui, "Noise suppression", Some("AI noise filter (RNNoise) removes keyboard, fans and hum"), false, |ui| {
                if toggle(ui, &mut self.config.noise_suppression).changed() {
                    self.control.noise_suppression.store(self.config.noise_suppression, Ordering::Relaxed);
                }
            });
            ui.separator();
            setting_row(ui, "Automatic gain control", Some("Boosts quiet microphones and tames loud ones"), false, |ui| {
                if toggle(ui, &mut self.config.auto_gain).changed() {
                    self.control.auto_gain.store(self.config.auto_gain, Ordering::Relaxed);
                }
            });
            ui.separator();
            setting_row(ui, "Echo cancellation", Some("Lets you use speakers instead of headphones (others don't hear themselves)"), false, |ui| {
                if toggle(ui, &mut self.config.echo_cancellation).changed() {
                    self.control.echo_cancellation.store(self.config.echo_cancellation, Ordering::Relaxed);
                }
            });
        });

        card(ui, |ui| {
            caption(ui, "Codec");
            ui.label(egui::RichText::new("Opus 64 kbit/s, 48 kHz mono, 20 ms frames, in-band FEC").color(theme::MUTED));
            setting_row(ui, "Audio quality", Some("Bitrate per voice channel"), true, |ui| {
                egui::ComboBox::from_id_salt("bitrate").selected_text("64 kbit/s").show_ui(ui, |_| {});
            });
        });
    }

    fn page_video(&mut self, ui: &mut egui::Ui) {
        if self.cameras.is_none() {
            self.cameras = Some(video::list_cameras().unwrap_or_default());
        }
        let cameras = self.cameras.clone().unwrap_or_default();
        card(ui, |ui| {
            caption(ui, "Camera");
            setting_row(ui, "Default camera", Some("Preselected when you turn on your camera"), false, |ui| {
                let current = self.config.camera.clone().unwrap_or_else(|| "First available".into());
                egui::ComboBox::from_id_salt("default_cam").selected_text(current).show_ui(ui, |ui| {
                    if ui.selectable_label(self.config.camera.is_none(), "First available").clicked() {
                        self.config.camera = None;
                    }
                    for cam in &cameras {
                        if ui.selectable_label(self.config.camera.as_deref() == Some(&cam.name), &cam.name).clicked() {
                            self.config.camera = Some(cam.name.clone());
                        }
                    }
                });
            });
            if cameras.is_empty() {
                ui.label(egui::RichText::new("No camera found.").color(theme::MUTED));
            }
            if ui.button(format!("{}  Refresh devices", icons::regular::ARROWS_CLOCKWISE)).clicked() {
                self.cameras = None;
            }
            ui.separator();
            setting_row(ui, "Mirror my camera", Some("Show your own preview mirrored"), true, |ui| {
                toggle(ui, &mut true.clone());
            });
            ui.separator();
            setting_row(ui, "Background blur", None, true, |ui| {
                toggle(ui, &mut false.clone());
            });
        });
        card(ui, |ui| {
            caption(ui, "Streaming");
            setting_row(ui, "Stream quality", Some("Camera and screen share. \"High\" for LAN, \"Balanced\" for Wi-Fi."), false, |ui| {
                egui::ComboBox::from_id_salt("video_quality")
                    .selected_text(self.config.video_quality.label())
                    .show_ui(ui, |ui| {
                        for q in [video::VideoQuality::High, video::VideoQuality::Balanced] {
                            ui.selectable_value(&mut self.config.video_quality, q, q.label());
                        }
                    });
            });
            ui.label(egui::RichText::new("Applies the next time you start your camera or screen share.").size(12.0).color(theme::MUTED));
            ui.separator();
            setting_row(ui, "Share audio with screen", Some("Send the computer sound while sharing (your RustTalk voices are filtered out)"), false, |ui| {
                toggle(ui, &mut self.config.share_screen_audio);
            });
            ui.separator();
            setting_row(ui, "Hardware encoding", Some("H.264 via GPU instead of MJPEG"), true, |ui| {
                toggle(ui, &mut false.clone());
            });
        });
    }

    fn page_keybinds(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            caption(ui, "Voice");
            let ptt_desc = if self.config.global_ptt { "Hold to transmit (works in the background)" } else { "Hold to transmit (window must be focused)" };
            setting_row(ui, "Push to talk", Some(ptt_desc), false, |ui| {
                self.keybind_button(ui);
            });
            for (label, desc) in [
                ("Toggle mute", "Mute / unmute your microphone"),
                ("Toggle deafen", "Mute everything you hear"),
                ("Toggle camera", "Turn your camera on or off"),
            ] {
                ui.separator();
                setting_row(ui, label, Some(desc), true, |ui| {
                    ui.add(egui::Button::new("Not set").min_size(egui::vec2(120.0, 28.0)));
                });
            }
        });
        card(ui, |ui| {
            caption(ui, "Global hotkeys");
            setting_row(ui, "Push to talk in the background", Some("Also works while another app is focused (e.g. in a game). The key still reaches that app."), false, |ui| {
                if toggle(ui, &mut self.config.global_ptt).changed() {
                    self.config.save();
                }
            });
            ui.label(
                egui::RichText::new(format!(
                    "{}  Tip: side mouse buttons (Mouse4 / Mouse5) make good push-to-talk keys - click the key button and press one.",
                    icons::regular::INFO
                ))
                .size(12.0)
                .color(theme::MUTED),
            );
        });
    }

    fn page_notifications(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            caption(ui, "Desktop");
            setting_row(ui, "Flash taskbar", Some("Highlight the taskbar icon on new messages while RustTalk is in the background"), false, |ui| {
                toggle(ui, &mut self.config.flash_taskbar);
            });
            ui.separator();
            setting_row(ui, "Desktop notifications", Some("Windows pop-up for new messages while RustTalk is in the background (click opens the channel)"), false, |ui| {
                toggle(ui, &mut self.config.desktop_notifications);
            });
            ui.separator();
            ui.add_enabled_ui(self.config.desktop_notifications, |ui| {
                setting_row(ui, "Show message text", Some("Off = the pop-up only says \"New message\""), false, |ui| {
                    toggle(ui, &mut self.config.notification_preview);
                });
            });
        });
        card(ui, |ui| {
            caption(ui, "Sounds");
            let rows: [(&str, Option<&str>, Sound, &mut bool); 4] = [
                ("User joins your channel", None, Sound::Join, &mut self.config.sound_join),
                ("User leaves your channel", None, Sound::Leave, &mut self.config.sound_leave),
                ("New message", Some("In another channel, while RustTalk is in the background, or when you are @mentioned"), Sound::Message, &mut self.config.sound_message),
                ("Mute / unmute", Some("Also for deafen"), Sound::Mute, &mut self.config.sound_mute),
            ];
            let mut preview = None;
            for (i, (label, desc, sound, value)) in rows.into_iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                setting_row(ui, label, desc, false, |ui| {
                    toggle(ui, value);
                    if ui.small_button(icons::regular::PLAY).on_hover_text("Preview (while connected)").clicked() {
                        preview = Some(sound);
                    }
                });
            }
            if let Some(sound) = preview {
                if self.audio.is_some() {
                    crate::audio::sfx::play(&self.sfx, sound);
                } else {
                    self.set_status("Connect to a server to preview sounds");
                }
            }
        });
    }

    fn page_advanced(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            caption(ui, "Data");
            let path = std::path::absolute(ClientConfig::path()).unwrap_or_else(|_| ClientConfig::path());
            setting_row(ui, "Settings file", Some(&path.display().to_string()), false, |ui| {
                if ui.button(format!("{}  Show in folder", icons::regular::FOLDER_OPEN)).clicked() {
                    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                }
            });
            ui.separator();
            setting_row(ui, "Reset all settings", Some("Back to defaults (keeps saved servers)"), true, |ui| {
                ui.add(egui::Button::new(egui::RichText::new("Reset").color(theme::RED)));
            });
        });
        card(ui, |ui| {
            caption(ui, "Developer");
            setting_row(ui, "Debug overlay", Some("Network, jitter buffer and codec statistics"), true, |ui| {
                toggle(ui, &mut false.clone());
            });
            ui.separator();
            setting_row(ui, "Log level", None, true, |ui| {
                egui::ComboBox::from_id_salt("log_level").selected_text("warn").show_ui(ui, |_| {});
            });
        });
    }

    fn page_about(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                logo::paint_logo(ui, 72.0);
                ui.label(egui::RichText::new("RustTalk").size(26.0).strong().color(theme::HEADER));
                ui.label(egui::RichText::new("Lightweight voice chat. Local first. No accounts.").color(theme::MUTED));
                ui.add_space(8.0);
            });
        });
        card(ui, |ui| {
            for (k, v) in [
                ("Version", env!("CARGO_PKG_VERSION").to_string()),
                ("Protocol", format!("v{}", rusttalk_shared::protocol::PROTOCOL_VERSION)),
                ("Voice", "Opus 64 kbit/s, RNNoise, FEC".to_string()),
                ("Video", "MJPEG up to 1080p / 25 fps".to_string()),
                ("UI", "egui + Phosphor icons".to_string()),
            ] {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(k).color(theme::MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(v).color(theme::HEADER));
                    });
                });
            }
        });
        self.updates_card(ui);
        card(ui, |ui| {
            caption(ui, "Feedback");
            setting_row(ui, "Found a bug?", Some("Opens a new GitHub issue with your version pre-filled"), false, |ui| {
                if ui.button(format!("{}  Report a Bug", icons::regular::BUG)).clicked() {
                    super::server_settings::open_bug_report(ui.ctx());
                }
            });
        });
    }

    // ----- keybind capture -------------------------------------------------

    fn keybind_button(&mut self, ui: &mut egui::Ui) {
        let label = if self.recording_key {
            egui::RichText::new("Press a key...").color(theme::YELLOW)
        } else {
            egui::RichText::new(format!("{}  {}", icons::regular::KEYBOARD, self.config.ptt_key)).color(theme::HEADER)
        };
        let resp = ui
            .add(egui::Button::new(label).min_size(egui::vec2(140.0, 28.0)))
            .on_hover_text("Click, then press the key or side mouse button you want to use (Esc cancels)");
        if resp.clicked() {
            self.recording_key = !self.recording_key;
        }
    }

    /// While recording: take the next key press as push-to-talk key.
    fn capture_keybind(&mut self, ctx: &egui::Context) {
        if !self.recording_key {
            return;
        }
        // Next key press or side mouse button (Esc cancels).
        let pressed: Option<Option<String>> = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => Some(None),
                egui::Event::Key { key, pressed: true, .. } => Some(Some(key.name().to_string())),
                egui::Event::PointerButton { button, pressed: true, .. } => {
                    crate::hotkey::mouse_name(*button).map(|n| Some(n.to_string()))
                }
                _ => None,
            })
        });
        if let Some(choice) = pressed {
            if let Some(name) = choice {
                self.config.ptt_key = name;
                self.config.save();
            }
            self.recording_key = false;
        }
    }
}

fn device_combo(ui: &mut egui::Ui, id: &str, selected: &mut Option<String>, devices: &[String]) {
    let text = selected.clone().unwrap_or_else(|| "System default".into());
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width() - 8.0)
        .selected_text(text)
        .show_ui(ui, |ui| {
            if ui.selectable_label(selected.is_none(), "System default").clicked() {
                *selected = None;
            }
            for name in devices {
                if ui.selectable_label(selected.as_deref() == Some(name), name).clicked() {
                    *selected = Some(name.clone());
                }
            }
        });
}

/// Large selectable card for the input mode.
fn mode_card(ui: &mut egui::Ui, value: &mut bool, this: bool, icon: &str, title: &str, desc: &str) {
    let selected = *value == this;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(220.0, 64.0), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, if selected { theme::SELECTED } else if resp.hovered() { theme::HOVER } else { theme::INPUT });
    if selected {
        p.rect_stroke(rect, 8.0, egui::Stroke::new(2.0_f32, theme::ACCENT), egui::StrokeKind::Inside);
    }
    p.text(rect.left_center() + egui::vec2(18.0, 0.0), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(22.0), if selected { theme::ACCENT } else { theme::MUTED });
    p.text(rect.left_top() + egui::vec2(38.0, 14.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(14.5), theme::HEADER);
    p.text(rect.left_top() + egui::vec2(38.0, 34.0), egui::Align2::LEFT_TOP, desc, egui::FontId::proportional(12.0), theme::MUTED);
    if resp.clicked() {
        *value = this;
    }
}

fn theme_swatch(ui: &mut egui::Ui, name: &str, color: egui::Color32, selected: bool, enabled: bool) {
    ui.vertical(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 44.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 8.0, color);
        let stroke = if selected { egui::Stroke::new(2.0_f32, theme::ACCENT) } else { egui::Stroke::new(1.0_f32, theme::BORDER) };
        ui.painter().rect_stroke(rect, 8.0, stroke, egui::StrokeKind::Inside);
        if selected {
            ui.painter().text(rect.right_top() + egui::vec2(-10.0, 10.0), egui::Align2::CENTER_CENTER, icons::regular::CHECK_CIRCLE, egui::FontId::proportional(14.0), theme::ACCENT);
        }
        ui.label(egui::RichText::new(name).size(12.0).color(if enabled { theme::HEADER } else { theme::DISABLED }));
    });
}
