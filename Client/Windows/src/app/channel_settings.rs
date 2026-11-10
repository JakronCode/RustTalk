//! Channel settings dialog (mostly visual for now, marked WIP) plus the
//! "create channel" and "confirm delete" dialogs.
//!
//! Functional today: name, category (move) and delete. Everything else is a
//! placeholder so the layout can be reviewed.

use super::sidebar::channel_icon;
use super::theme::{self, card, caption, setting_row, toggle};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ChannelTab {
    Overview,
    Permissions,
    Invites,
}

pub(super) struct ChannelSettings {
    pub id: u64,
    pub tab: ChannelTab,
    pub name: String,
    pub category: Option<u64>,
    pub topic: String,
}

impl RustTalkApp {
    pub(super) fn open_channel_settings(&mut self, id: u64) {
        if let Some(ch) = self.channels.get(&id) {
            self.channel_settings = Some(ChannelSettings {
                id,
                tab: ChannelTab::Overview,
                name: ch.name.clone(),
                category: ch.parent,
                topic: String::new(),
            });
        }
    }

    pub(super) fn open_new_channel(&mut self, parent: Option<u64>, kind: ChannelKind) {
        self.new_channel_parent = parent;
        self.new_channel_kind = kind;
        self.new_channel_name.clear();
        self.show_new_channel = true;
    }

    pub(super) fn ui_channel_settings(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.channel_settings.take() else { return };
        let Some(ch) = self.channels.get(&st.id).cloned() else { return };
        let mut open = true;
        let mut close = false;
        let is_category = ch.kind == ChannelKind::Category;
        let title = if is_category { "Category Settings" } else { "Channel Settings" };

        egui::Window::new(format!("{}  {}", icons::regular::GEAR_SIX, title))
            .id(egui::Id::new("channel_settings_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([720.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // Left: channel + tabs.
                    ui.vertical(|ui| {
                        ui.set_width(170.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(channel_icon(ch.kind)).size(16.0).color(theme::MUTED));
                            ui.label(egui::RichText::new(&ch.name).strong().color(theme::HEADER));
                        });
                        ui.add_space(8.0);
                        for (tab, icon, label, wip) in [
                            (ChannelTab::Overview, icons::regular::NOTE_PENCIL, "Overview", false),
                            (ChannelTab::Permissions, icons::regular::LOCK_KEY, "Permissions", true),
                            (ChannelTab::Invites, icons::regular::LINK, "Invites", true),
                        ] {
                            let sel = st.tab == tab;
                            let resp = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(format!("{icon}  {label}"))
                                        .color(if sel { theme::HEADER } else { theme::MUTED }),
                                )
                                .fill(if sel { theme::SELECTED } else { egui::Color32::TRANSPARENT })
                                .min_size(egui::vec2(170.0, 32.0)),
                            );
                            if wip {
                                let r = resp.rect;
                                ui.painter().circle_filled(r.right_center() - egui::vec2(12.0, 0.0), 4.0, theme::YELLOW);
                            }
                            if resp.clicked() {
                                st.tab = tab;
                            }
                        }
                        if ch.kind != ChannelKind::Spawn {
                            ui.add_space(12.0);
                            ui.separator();
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(format!("{}  Delete {}", icons::regular::TRASH, if is_category { "Category" } else { "Channel" }))
                                            .color(theme::RED),
                                    )
                                    .frame(false)
                                    .min_size(egui::vec2(170.0, 30.0)),
                                )
                                .clicked()
                            {
                                self.confirm_delete = Some(ch.id);
                                close = true;
                            }
                        }
                    });
                    ui.separator();

                    // Right: content.
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            theme::wip_banner(
                                ui,
                                "Work in progress: only name, category and delete are saved for now. The other options are placeholders.",
                            );
                            match st.tab {
                                ChannelTab::Overview => self.channel_overview(ui, &ch, &mut st),
                                ChannelTab::Permissions => channel_permissions(ui, ch.kind),
                                ChannelTab::Invites => channel_invites(ui),
                            }
                        });
                        ui.add_space(6.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::primary_button(ui, "Save Changes").clicked() {
                                self.save_channel_settings(&ch, &st);
                                close = true;
                            }
                            if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                                close = true;
                            }
                        });
                    });
                });
            });
        if open && !close {
            self.channel_settings = Some(st);
        }
    }

    fn channel_overview(&mut self, ui: &mut egui::Ui, ch: &ChannelInfo, st: &mut ChannelSettings) {
        card(ui, |ui| {
            caption(ui, if ch.kind == ChannelKind::Category { "Category name" } else { "Channel name" });
            ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(f32::INFINITY));
            if matches!(ch.kind, ChannelKind::Text | ChannelKind::Voice) {
                ui.add_space(4.0);
                caption(ui, "Category");
                let mut cats: Vec<(u64, String)> = self
                    .sorted_children(None)
                    .into_iter()
                    .filter(|c| c.kind == ChannelKind::Category)
                    .map(|c| (c.id, c.name))
                    .collect();
                cats.insert(0, (0, "No category".into()));
                let current = cats
                    .iter()
                    .find(|(id, _)| Some(*id) == st.category || (*id == 0 && st.category.is_none()))
                    .map(|(_, n)| n.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("ch_category")
                    .width(260.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (id, name) in cats {
                            let value = if id == 0 { None } else { Some(id) };
                            ui.selectable_value(&mut st.category, value, name);
                        }
                    });
            }
        });
        match ch.kind {
            ChannelKind::Text | ChannelKind::Spawn => {
                card(ui, |ui| {
                    caption(ui, "Channel topic");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::multiline(&mut st.topic)
                            .hint_text("Let everyone know how to use this channel")
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.separator();
                    setting_row(ui, "Slowmode", Some("Limit how often each member can post"), true, |ui| {
                        egui::ComboBox::from_id_salt("slowmode").selected_text("Off").show_ui(ui, |_| {});
                    });
                    ui.separator();
                    setting_row(ui, "Age-restricted", Some("Members must confirm before viewing"), true, |ui| {
                        toggle(ui, &mut false.clone());
                    });
                    ui.separator();
                    setting_row(ui, "Message history", Some("Messages kept on the server"), true, |ui| {
                        egui::ComboBox::from_id_salt("history").selected_text("10,000").show_ui(ui, |_| {});
                    });
                });
            }
            ChannelKind::Voice => {
                card(ui, |ui| {
                    setting_row(ui, "Bitrate", Some("Audio quality for everyone in this channel"), true, |ui| {
                        let mut v = 64.0f32;
                        ui.add(egui::Slider::new(&mut v, 8.0..=128.0).fixed_decimals(0).suffix(" kbps"));
                    });
                    ui.separator();
                    setting_row(ui, "User limit", Some("Max. members in this channel"), true, |ui| {
                        let mut v = 0.0f32;
                        ui.add(egui::Slider::new(&mut v, 0.0..=99.0).custom_formatter(|v, _| if v == 0.0 { "No limit".into() } else { format!("{v:.0}") }));
                    });
                    ui.separator();
                    setting_row(ui, "Video quality", Some("Max. resolution for camera and screen share"), true, |ui| {
                        egui::ComboBox::from_id_salt("ch_video").selected_text("Auto").show_ui(ui, |_| {});
                    });
                    ui.separator();
                    setting_row(ui, "Push-to-talk required", Some("Disable voice activity in this channel"), true, |ui| {
                        toggle(ui, &mut false.clone());
                    });
                });
            }
            ChannelKind::Category => {
                card(ui, |ui| {
                    setting_row(ui, "Sync permissions", Some("Channels inherit this category's permissions"), true, |ui| {
                        toggle(ui, &mut true.clone());
                    });
                });
            }
        }
    }

    fn save_channel_settings(&mut self, ch: &ChannelInfo, st: &ChannelSettings) {
        let Some(tx) = self.cmd_tx.clone() else { return };
        let name = st.name.trim();
        if !name.is_empty() && name != ch.name {
            let _ = tx.send(Command::RenameChannel(ch.id, name.to_string()));
        }
        if matches!(ch.kind, ChannelKind::Text | ChannelKind::Voice) && st.category != ch.parent {
            let _ = tx.send(Command::MoveChannel { channel_id: ch.id, parent: st.category, index: u32::MAX });
        }
    }

    /// "Create channel" dialog.
    pub(super) fn ui_new_channel(&mut self, ctx: &egui::Context) {
        if !self.show_new_channel {
            return;
        }
        let mut open = true;
        let is_cat = self.new_channel_kind == ChannelKind::Category;
        egui::Window::new(if is_cat { "Create Category" } else { "Create Channel" })
            .id(egui::Id::new("new_channel_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([420.0, 0.0])
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                if !is_cat {
                    if let Some(p) = self.new_channel_parent {
                        ui.label(egui::RichText::new(format!("in {}", self.channel_name(p))).color(theme::MUTED));
                    }
                    caption(ui, "Channel type");
                    for (kind, icon, title, desc) in [
                        (ChannelKind::Text, icons::regular::CHAT_CIRCLE_TEXT, "Text", "Send messages, images and GIFs"),
                        (ChannelKind::Voice, icons::regular::SPEAKER_HIGH, "Voice", "Hang out with voice, video and screen share"),
                    ] {
                        let sel = self.new_channel_kind == kind;
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::click());
                        let p = ui.painter();
                        p.rect_filled(rect, 8.0, if sel { theme::SELECTED } else if resp.hovered() { theme::HOVER } else { theme::INPUT });
                        p.text(rect.left_center() + egui::vec2(20.0, 0.0), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(20.0), theme::MUTED);
                        p.text(rect.left_top() + egui::vec2(44.0, 9.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(14.5), theme::HEADER);
                        p.text(rect.left_top() + egui::vec2(44.0, 29.0), egui::Align2::LEFT_TOP, desc, egui::FontId::proportional(12.0), theme::MUTED);
                        let dot = rect.right_center() - egui::vec2(20.0, 0.0);
                        p.circle_stroke(dot, 8.0, egui::Stroke::new(2.0_f32, if sel { theme::HEADER } else { theme::MUTED }));
                        if sel {
                            p.circle_filled(dot, 4.5, theme::HEADER);
                        }
                        if resp.clicked() {
                            self.new_channel_kind = kind;
                        }
                    }
                }
                caption(ui, if is_cat { "Category name" } else { "Channel name" });
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.new_channel_name)
                        .hint_text(if is_cat { "New Category" } else { "new-channel" })
                        .desired_width(f32::INFINITY),
                );
                if !resp.has_focus() && self.new_channel_name.is_empty() {
                    resp.request_focus();
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let create = theme::primary_button(ui, if is_cat { "Create Category" } else { "Create Channel" }).clicked()
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if create && !self.new_channel_name.trim().is_empty() {
                            if let Some(tx) = &self.cmd_tx {
                                let parent = if is_cat { None } else { self.new_channel_parent };
                                let _ = tx.send(Command::CreateChannel {
                                    name: self.new_channel_name.trim().to_string(),
                                    kind: self.new_channel_kind,
                                    parent,
                                });
                            }
                            self.new_channel_name.clear();
                            self.show_new_channel = false;
                        }
                        if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                            self.show_new_channel = false;
                        }
                    });
                });
            });
        self.show_new_channel = open && self.show_new_channel;
    }

    /// "Really delete?" confirmation.
    pub(super) fn ui_confirm_delete(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else { return };
        let Some(ch) = self.channels.get(&id).cloned() else {
            self.confirm_delete = None;
            return;
        };
        let what = if ch.kind == ChannelKind::Category { "Category" } else { "Channel" };
        let mut open = true;
        let mut done = false;
        egui::Window::new(format!("Delete {what}"))
            .id(egui::Id::new("confirm_delete_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([400.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("Are you sure you want to delete \"{}\"? This cannot be undone.", ch.name))
                        .color(theme::TEXT),
                );
                match ch.kind {
                    ChannelKind::Text => {
                        ui.label(egui::RichText::new("All messages in this channel will be deleted.").color(theme::MUTED));
                    }
                    ChannelKind::Voice => {
                        ui.label(egui::RichText::new("Everyone in it is moved to the lobby.").color(theme::MUTED));
                    }
                    ChannelKind::Category => {
                        ui.label(egui::RichText::new("Its channels are kept and moved to the top level.").color(theme::MUTED));
                    }
                    ChannelKind::Spawn => {}
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::danger_button(ui, &format!("Delete {what}")).clicked() {
                        if let Some(tx) = &self.cmd_tx {
                            let _ = tx.send(Command::DeleteChannel(id));
                        }
                        done = true;
                    }
                    if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                        done = true;
                    }
                });
            });
        if !open || done {
            self.confirm_delete = None;
        }
    }
}

fn channel_permissions(ui: &mut egui::Ui, kind: ChannelKind) {
    card(ui, |ui| {
        caption(ui, "Roles");
        ui.horizontal(|ui| {
            for (role, color) in [("@everyone", theme::MUTED), ("Admin", theme::RED), ("Moderator", theme::GREEN)] {
                ui.add_enabled(false, egui::Button::new(egui::RichText::new(role).color(color)));
            }
        });
    });
    card(ui, |ui| {
        caption(ui, "Permissions for @everyone");
        let perms: &[(&str, &str)] = match kind {
            ChannelKind::Voice => &[
                ("View channel", "See this channel in the list"),
                ("Connect", "Join the voice channel"),
                ("Speak", "Talk in the channel"),
                ("Video", "Turn on the camera"),
                ("Screen share", "Share a screen or window"),
                ("Send messages", "Write in the voice chat"),
            ],
            _ => &[
                ("View channel", "See this channel in the list"),
                ("Send messages", "Write messages"),
                ("Attach files", "Upload images"),
                ("Manage messages", "Delete messages of others"),
            ],
        };
        for (i, (label, desc)) in perms.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            setting_row(ui, label, Some(desc), true, |ui| {
                toggle(ui, &mut true.clone());
            });
        }
    });
}

fn channel_invites(ui: &mut egui::Ui) {
    card(ui, |ui| {
        caption(ui, "Invite links");
        ui.label(egui::RichText::new("Create links that drop people directly into this channel.").color(theme::MUTED));
        ui.horizontal(|ui| {
            let mut link = "rusttalk://host:7331/channel".to_string();
            ui.add_enabled(false, egui::TextEdit::singleline(&mut link).desired_width(300.0));
            ui.add_enabled(false, egui::Button::new(format!("{}  Copy", icons::regular::COPY)));
        });
    });
}
