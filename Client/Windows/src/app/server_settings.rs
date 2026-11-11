//! Server settings dialog: overview (logo, name), AFK mover, messages
//! (welcome, join/leave, rules, AFK notice, MOTD) and WIP tabs (roles, bans, invites, audit
//! log). Also the server icon painter and the "Report a Bug" link.
//!
//! There is no role system yet: everyone connected may change these
//! settings (LAN use). The server validates and persists them.

use super::theme::{self, card, caption, setting_row, toggle};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerTab {
    Overview,
    Afk,
    Messages,
    Integrations,
    Roles,
    Bans,
    Invites,
    AuditLog,
}

impl ServerTab {
    const ALL: [(ServerTab, &'static str, &'static str, bool); 8] = [
        (ServerTab::Overview, icons::regular::HOUSE_LINE, "Overview", false),
        (ServerTab::Afk, icons::regular::MOON, "AFK", false),
        (ServerTab::Messages, icons::regular::CHAT_CENTERED_TEXT, "Messages", false),
        (ServerTab::Integrations, icons::regular::PLUGS_CONNECTED, "Integrations", false),
        (ServerTab::Roles, icons::regular::SHIELD_CHECK, "Roles", false),
        (ServerTab::Bans, icons::regular::PROHIBIT, "Bans", false),
        (ServerTab::Invites, icons::regular::LINK, "Invites", false),
        (ServerTab::AuditLog, icons::regular::LIST_MAGNIFYING_GLASS, "Audit Log", false),
    ];
}

/// Dialog state: tab + editable copy of the settings.
pub(super) struct ServerSettingsDialog {
    tab: ServerTab,
    draft: ServerSettings,
    error: Option<String>,
    /// Input for a new GIPHY key (sent on its own, not with "Save Changes").
    gif_key: String,
    /// Admin password input (Roles tab).
    admin_pw: String,
    invite_form: super::moderation_ui::InviteForm,
}

impl RustTalkApp {
    pub(super) fn open_server_settings(&mut self, tab: ServerTab) {
        self.server_settings_dialog =
            Some(ServerSettingsDialog {
                tab,
                draft: self.server_settings.clone(),
                error: None,
                gif_key: String::new(),
                admin_pw: String::new(),
                invite_form: Default::default(),
            });
        self.request_admin_data();
    }

    pub(super) fn ui_server_settings(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.server_settings_dialog.take() else { return };
        let mut open = true;
        let mut close = false;
        egui::Window::new(format!("{}  Server Settings", icons::regular::SHIELD_CHECK))
            .id(egui::Id::new("server_settings_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([760.0, 540.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // Left: server + tabs.
                    ui.vertical(|ui| {
                        ui.set_width(180.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                            paint_server_icon(ui, r, &dlg.draft, &mut self.textures);
                            ui.label(egui::RichText::new(super::sidebar::elide(ui, &dlg.draft.name, &egui::FontId::proportional(14.0), 140.0)).strong().color(theme::HEADER));
                        });
                        ui.add_space(8.0);
                        for (tab, icon, label, wip) in ServerTab::ALL {
                            let sel = dlg.tab == tab;
                            let resp = ui.add(
                                egui::Button::new(egui::RichText::new(format!("{icon}  {label}")).color(if sel { theme::HEADER } else { theme::MUTED }))
                                    .fill(if sel { theme::SELECTED } else { egui::Color32::TRANSPARENT })
                                    .min_size(egui::vec2(180.0, 32.0)),
                            );
                            if wip {
                                ui.painter().circle_filled(resp.rect.right_center() - egui::vec2(12.0, 0.0), 4.0, theme::YELLOW);
                            }
                            if resp.clicked() {
                                dlg.tab = tab;
                                self.request_admin_data();
                            }
                        }
                    });
                    ui.separator();

                    // Right: content + save bar.
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            match dlg.tab {
                                ServerTab::Overview => self.server_overview(ui, &mut dlg),
                                ServerTab::Afk => self.server_afk(ui, &mut dlg),
                                ServerTab::Messages => self.server_messages(ui, &mut dlg),
                                ServerTab::Integrations => self.server_integrations(ui, &mut dlg),
                                ServerTab::Roles => self.server_roles(ui, &mut dlg.admin_pw),
                                ServerTab::Bans => self.server_bans(ui),
                                ServerTab::Invites => self.server_invites(ui, &mut dlg.invite_form),
                                ServerTab::AuditLog => self.server_audit(ui),
                            }
                        });
                        if let Some(e) = &dlg.error {
                            ui.label(egui::RichText::new(e).color(theme::RED));
                        }
                        let admin = self.can(rusttalk_shared::protocol::Role::Admin);
                        let changed = admin && dlg.draft != self.server_settings;
                        if !admin {
                            ui.label(egui::RichText::new(format!("{}  Only admins can change server settings.", icons::regular::LOCK)).color(theme::MUTED));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let save = ui.add_enabled(
                                changed,
                                egui::Button::new(egui::RichText::new("Save Changes").strong().color(egui::Color32::WHITE))
                                    .fill(theme::ACCENT)
                                    .min_size(egui::vec2(110.0, 32.0)),
                            );
                            if save.clicked() {
                                if let Some(tx) = &self.cmd_tx {
                                    let _ = tx.send(Command::UpdateServerSettings(dlg.draft.clone()));
                                }
                                close = true;
                            }
                            if ui.add(egui::Button::new(if changed { "Discard" } else { "Close" }).frame(false)).clicked() {
                                close = true;
                            }
                            if changed {
                                ui.label(egui::RichText::new("Unsaved changes").color(theme::YELLOW));
                            }
                        });
                    });
                });
            });
        if open && !close {
            self.server_settings_dialog = Some(dlg);
        }
    }

    fn server_overview(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        card(ui, |ui| {
            caption(ui, "Server icon");
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(88.0, 88.0), egui::Sense::hover());
                paint_server_icon(ui, r, &dlg.draft, &mut self.textures);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Shown in the sidebar for everyone. PNG or JPEG, it is cropped to a square and scaled to 128 px.").color(theme::MUTED));
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  Upload Image", icons::regular::UPLOAD_SIMPLE)).clicked() {
                            match pick_icon() {
                                Ok(Some(png)) => {
                                    dlg.draft.icon = Some(png);
                                    dlg.error = None;
                                }
                                Ok(None) => {}
                                Err(e) => dlg.error = Some(e),
                            }
                        }
                        if dlg.draft.icon.is_some() && ui.add(egui::Button::new(egui::RichText::new("Remove").color(theme::RED)).frame(false)).clicked() {
                            dlg.draft.icon = None;
                        }
                    });
                });
            });
        });
        card(ui, |ui| {
            caption(ui, "Server name");
            ui.add(egui::TextEdit::singleline(&mut dlg.draft.name).char_limit(rusttalk_shared::protocol::MAX_NAME_LEN).desired_width(f32::INFINITY));
            ui.label(egui::RichText::new("Shown at the top of the sidebar and when people connect.").size(12.0).color(theme::MUTED));
        });
        card(ui, |ui| {
            caption(ui, "Region & limits");
            setting_row(ui, "Max. users", Some("Set in server.toml for now"), true, |ui| {
                let mut v = 32.0f32;
                ui.add(egui::Slider::new(&mut v, 2.0..=128.0).fixed_decimals(0));
            });
            ui.separator();
            setting_row(ui, "Server password", Some("Set in server.toml for now"), true, |ui| {
                let mut s = String::new();
                ui.add(egui::TextEdit::singleline(&mut s).password(true).desired_width(180.0));
            });
        });
    }

    fn server_afk(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        card(ui, |ui| {
            caption(ui, "Auto AFK mover");
            setting_row(ui, "Move inactive users", Some("Users who stay muted or deafened in a voice channel are moved automatically"), false, |ui| {
                toggle(ui, &mut dlg.draft.afk_enabled);
            });
            ui.add_enabled_ui(dlg.draft.afk_enabled, |ui| {
                ui.separator();
                setting_row(ui, "Timeout", Some("How long someone has to be muted"), false, |ui| {
                    ui.add(
                        egui::Slider::new(&mut dlg.draft.afk_timeout_mins, 1..=120)
                            .logarithmic(true)
                            .suffix(" min"),
                    );
                });
                ui.separator();
                setting_row(ui, "AFK channel", Some("Where inactive users are moved to"), false, |ui| {
                    let mut targets: Vec<(Option<u64>, String)> = vec![(None, self.spawn_channel_id().map(|id| self.channel_name(id)).unwrap_or_else(|| "Lobby".into()))];
                    for parent in std::iter::once(None).chain(self.sorted_children(None).into_iter().filter(|c| c.kind == ChannelKind::Category).map(|c| Some(c.id))) {
                        for ch in self.sorted_children(parent).into_iter().filter(|c| c.kind == ChannelKind::Voice) {
                            targets.push((Some(ch.id), ch.name));
                        }
                    }
                    let current = targets.iter().find(|(id, _)| *id == dlg.draft.afk_channel).map(|(_, n)| n.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("afk_channel").width(200.0).selected_text(current).show_ui(ui, |ui| {
                        for (id, name) in targets {
                            let icon = if id.is_none() { icons::regular::HOUSE } else { icons::regular::SPEAKER_HIGH };
                            ui.selectable_value(&mut dlg.draft.afk_channel, id, format!("{icon}  {name}"));
                        }
                    });
                });
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "{}  The server checks every 15 seconds. Moved users get a notice and stay muted.",
                    icons::regular::INFO
                ))
                .size(12.0)
                .color(theme::MUTED),
            );
        });
        card(ui, |ui| {
            caption(ui, "More");
            setting_row(ui, "Also move idle users who don't talk", Some("Not muted, but silent for the timeout"), true, |ui| {
                toggle(ui, &mut false.clone());
            });
            ui.separator();
            setting_row(ui, "Exempt roles", Some("e.g. moderators are never moved"), true, |ui| {
                egui::ComboBox::from_id_salt("afk_exempt").selected_text("None").show_ui(ui, |_| {});
            });
        });
    }
}

impl RustTalkApp {
    fn server_integrations(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        card(ui, |ui| {
            caption(ui, "GIFs (GIPHY)");
            ui.horizontal(|ui| {
                let (icon, text, color) = if self.gif_available {
                    (icons::regular::CHECK_CIRCLE, "GIF search is active for everyone on this server", theme::GREEN)
                } else {
                    (icons::regular::X_CIRCLE, "GIF search is off - no API key set", theme::MUTED)
                };
                ui.label(egui::RichText::new(icon).size(18.0).color(color));
                ui.label(egui::RichText::new(text).color(theme::HEADER));
            });
            ui.label(
                egui::RichText::new(
                    "The key is stored on the server only - members never see it and don't have to enter anything. \
                     The server searches GIPHY for everyone and caches results.",
                )
                .size(12.0)
                .color(theme::MUTED),
            );
            ui.separator();
            let label = if self.gif_available { "Replace API key" } else { "GIPHY API key" };
            setting_row(ui, label, Some("Free at developers.giphy.com (create an app, type \"API\")"), false, |ui| {
                ui.add(egui::TextEdit::singleline(&mut dlg.gif_key).password(true).hint_text("paste key").desired_width(200.0));
            });
            ui.horizontal(|ui| {
                let key = dlg.gif_key.trim().to_string();
                if ui.add_enabled(!key.is_empty(), egui::Button::new(format!("{}  Check & save", icons::regular::FLOPPY_DISK))).clicked() {
                    if let Some(tx) = &self.cmd_tx {
                        let _ = tx.send(Command::SetGifKey(Some(key)));
                    }
                    dlg.gif_key.clear();
                    self.set_status("Checking the GIPHY key...");
                }
                if self.gif_available
                    && ui.add(egui::Button::new(egui::RichText::new(format!("{}  Remove key", icons::regular::TRASH)).color(theme::RED))).clicked()
                {
                    if let Some(tx) = &self.cmd_tx {
                        let _ = tx.send(Command::SetGifKey(None));
                    }
                }
                if ui.link("Get a free key").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab("https://developers.giphy.com/dashboard/"));
                }
            });
        });
    }

    fn server_messages(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        use rusttalk_shared::protocol::{MAX_SERVER_MESSAGE_LEN, MAX_WELCOME_LEN};
        card(ui, |ui| {
            caption(ui, "Welcome message");
            ui.add(
                egui::TextEdit::multiline(&mut dlg.draft.welcome_message)
                    .hint_text("Shown to everyone in the lobby, e.g. \"Welcome! Please read #announcements first.\"")
                    .char_limit(MAX_WELCOME_LEN)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            char_count(ui, &dlg.draft.welcome_message, MAX_WELCOME_LEN);
        });
        card(ui, |ui| {
            caption(ui, "Join & leave messages");
            setting_row(ui, "Announcement channel", Some("Text channel where join and leave messages are posted"), false, |ui| {
                let mut targets: Vec<(Option<u64>, String)> = vec![(None, "Off".to_string())];
                for parent in std::iter::once(None).chain(self.sorted_children(None).into_iter().filter(|c| c.kind == ChannelKind::Category).map(|c| Some(c.id))) {
                    for ch in self.sorted_children(parent).into_iter().filter(|c| c.kind == ChannelKind::Text) {
                        targets.push((Some(ch.id), ch.name));
                    }
                }
                let current = targets.iter().find(|(id, _)| *id == dlg.draft.announce_channel).map(|(_, n)| n.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("announce_channel").width(200.0).selected_text(current).show_ui(ui, |ui| {
                    for (id, name) in targets {
                        let icon = if id.is_none() { icons::regular::PROHIBIT } else { icons::regular::CHAT_CIRCLE };
                        ui.selectable_value(&mut dlg.draft.announce_channel, id, format!("{icon}  {name}"));
                    }
                });
            });
            ui.add_enabled_ui(dlg.draft.announce_channel.is_some(), |ui| {
                ui.separator();
                message_field(ui, "Join message", "Posted when someone connects (empty = off).  {user} = name", "{user} joined the server - say hi!", &mut dlg.draft.join_message, MAX_SERVER_MESSAGE_LEN);
                ui.separator();
                message_field(ui, "Leave message", "Posted when someone disconnects (empty = off).  {user} = name", "{user} left the server", &mut dlg.draft.leave_message, MAX_SERVER_MESSAGE_LEN);
            });
        });
        card(ui, |ui| {
            caption(ui, "Rules");
            ui.add(
                egui::TextEdit::multiline(&mut dlg.draft.rules)
                    .hint_text("Shown in the lobby below the welcome message, e.g. \"1. Be nice  2. No spam\"")
                    .char_limit(MAX_WELCOME_LEN)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            char_count(ui, &dlg.draft.rules, MAX_WELCOME_LEN);
        });
        card(ui, |ui| {
            caption(ui, "Notices");
            message_field(ui, "AFK notice", "Sent to users moved by the AFK mover (empty = default).  {channel}, {minutes}", rusttalk_shared::protocol::DEFAULT_AFK_NOTICE, &mut dlg.draft.afk_notice, MAX_SERVER_MESSAGE_LEN);
            ui.separator();
            message_field(ui, "Message of the day", "Shown to everyone when they connect (empty = none)", "Game night on Friday at 8 pm!", &mut dlg.draft.motd, MAX_SERVER_MESSAGE_LEN);
        });
    }
}

/// "12 / 300" counter below a text field.
fn char_count(ui: &mut egui::Ui, text: &str, max: usize) {
    ui.label(egui::RichText::new(format!("{} / {max}", text.chars().count())).size(11.5).color(theme::MUTED));
}

/// Label, description and single-line input for a short server message.
fn message_field(ui: &mut egui::Ui, label: &str, desc: &str, hint: &str, value: &mut String, max: usize) {
    ui.label(egui::RichText::new(label).color(theme::HEADER));
    ui.label(egui::RichText::new(desc).size(12.0).color(theme::MUTED));
    ui.add(egui::TextEdit::singleline(value).hint_text(hint).char_limit(max).desired_width(f32::INFINITY));
}


/// Choose an image, crop to a square, scale to 128x128, encode as PNG.
fn pick_icon() -> Result<Option<Vec<u8>>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("Could not read image: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("Not a supported image: {e}"))?;
    let png = icon_png(img)?;
    if png.len() > rusttalk_shared::protocol::MAX_ICON_BYTES {
        return Err("Image is too complex, please pick a simpler one".into());
    }
    Ok(Some(png))
}

/// Square crop + 128 px PNG (also used by tests).
fn icon_png(img: image::DynamicImage) -> Result<Vec<u8>, String> {
    let img = img.resize_to_fill(128, 128, image::imageops::FilterType::Lanczos3);
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| format!("PNG encode: {e}"))?;
    Ok(out.into_inner())
}

/// Server icon: uploaded image, otherwise colored square with initials.
pub(super) fn paint_server_icon(ui: &mut egui::Ui, rect: egui::Rect, s: &ServerSettings, textures: &mut TextureCache) {
    let radius = (rect.width() * 0.28) as u8;
    if let Some(tex) = s.icon.as_ref().and_then(|b| textures.texture_for(ui.ctx(), b, "server-icon")) {
        egui::Image::new(&tex).corner_radius(radius).paint_at(ui, rect);
        return;
    }
    let p = ui.painter();
    p.rect_filled(rect, radius, theme::avatar_color(&s.name));
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        server_initials(&s.name),
        egui::FontId::proportional(rect.width() * 0.38),
        egui::Color32::WHITE,
    );
}

/// "RustTalk Server" -> "RS".
pub(super) fn server_initials(name: &str) -> String {
    let s: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(|c| c.to_uppercase())
        .collect();
    if s.is_empty() { "?".into() } else { s }
}

/// Opens a new GitHub issue with version info pre-filled.
pub(super) fn open_bug_report(ctx: &egui::Context) {
    let body = format!(
        "**What happened?**\n\n\n**What did you expect?**\n\n\n**Steps to reproduce**\n1. \n\n---\nRustTalk v{} (protocol v{}), Windows",
        env!("CARGO_PKG_VERSION"),
        rusttalk_shared::protocol::PROTOCOL_VERSION
    );
    let url = format!(
        "https://github.com/JakronCode/RustTalk/issues/new?labels=bug&title={}&body={}",
        url_encode("Bug: "),
        url_encode(&body)
    );
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// Percent-encoding for URL query values.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials() {
        assert_eq!(server_initials("RustTalk Server"), "RS");
        assert_eq!(server_initials("gaming"), "G");
        assert_eq!(server_initials("  "), "?");
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("Bug: a b"), "Bug%3A%20a%20b");
        assert_eq!(url_encode("ä\n"), "%C3%A4%0A");
    }

    #[test]
    fn icon_is_square_png_and_small() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(640, 360, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        }));
        let png = icon_png(img).unwrap();
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (128, 128));
        assert!(png.len() <= rusttalk_shared::protocol::MAX_ICON_BYTES);
    }
}
