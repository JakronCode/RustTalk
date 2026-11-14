//! Server settings dialog: overview (logo, name), AFK mover, messages
//! (welcome, join/leave, rules, AFK notice, MOTD) and WIP tabs (roles, bans, invites, audit
//! log). Also the server icon painter and the "Report a Bug" link.
//!
//! There is no role system yet: everyone connected may change these
//! settings (LAN use). The server validates and persists them.

use super::settings::{close_panel, content_panel, nav_panel, nav_sidebar, page_body, s, Group, Nav, Page};
use super::theme::{self, card, setting_row, toggle};
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerSection {
    Icon,
    Name,
    Limits,
    AfkMover,
    AfkMore,
    Welcome,
    JoinLeave,
    Rules,
    Notices,
    Gifs,
    Roles,
    Bans,
    Invites,
    Audit,
}

use ServerSection as S;

/// Same table shape as the app settings: groups -> pages -> sections.
#[rustfmt::skip]
const GROUPS: &[Group<ServerTab, ServerSection>] = &[
    Group {
        title: "Server",
        pages: &[
            Page { page: ServerTab::Overview, title: "Overview", icon: icons::regular::HOUSE_LINE, sections: &[
                s(S::Icon, "Server icon", &["icon", "image", "logo", "picture", "upload", "avatar"]),
                s(S::Name, "Server name", &["name", "title"]),
                s(S::Limits, "Region & limits", &["region", "limits", "max users", "server password", "password"]),
            ] },
            Page { page: ServerTab::Afk, title: "AFK", icon: icons::regular::MOON, sections: &[
                s(S::AfkMover, "Auto AFK mover", &["afk", "move", "inactive", "timeout", "muted", "deafened", "channel"]),
                s(S::AfkMore, "More", &["idle", "silent", "exempt", "roles", "moderators"]),
            ] },
            Page { page: ServerTab::Messages, title: "Messages", icon: icons::regular::CHAT_CENTERED_TEXT, sections: &[
                s(S::Welcome, "Welcome message", &["welcome", "lobby", "greeting"]),
                s(S::JoinLeave, "Join & leave messages", &["join", "leave", "announcement", "channel", "connects", "disconnects"]),
                s(S::Rules, "Rules", &["rules", "lobby"]),
                s(S::Notices, "Notices", &["afk notice", "message of the day", "motd", "notice"]),
            ] },
            Page { page: ServerTab::Integrations, title: "Integrations", icon: icons::regular::PLUGS_CONNECTED, sections: &[
                s(S::Gifs, "GIFs (GIPHY)", &["gif", "giphy", "api key", "key", "search"]),
            ] },
        ],
    },
    Group {
        title: "Moderation",
        pages: &[
            Page { page: ServerTab::Roles, title: "Roles", icon: icons::regular::SHIELD_CHECK, sections: &[
                s(S::Roles, "Roles", &["roles", "admin", "moderator", "member", "permissions", "password"]),
            ] },
            Page { page: ServerTab::Bans, title: "Bans", icon: icons::regular::PROHIBIT, sections: &[
                s(S::Bans, "Bans", &["bans", "banned", "unban"]),
            ] },
            Page { page: ServerTab::Invites, title: "Invites", icon: icons::regular::LINK, sections: &[
                s(S::Invites, "Invites", &["invites", "invite link", "uses", "expire"]),
            ] },
            Page { page: ServerTab::AuditLog, title: "Audit Log", icon: icons::regular::LIST_MAGNIFYING_GLASS, sections: &[
                s(S::Audit, "Audit Log", &["audit", "log", "history", "actions"]),
            ] },
        ],
    },
];

fn page(tab: ServerTab) -> &'static Page<ServerTab, ServerSection> {
    GROUPS
        .iter()
        .flat_map(|g| g.pages)
        .find(|p| p.page == tab)
        .expect("every ServerTab is in GROUPS")
}

/// Page state: tab + editable copy of the settings, bound to the server
/// session it was opened for.
pub(super) struct ServerSettingsDialog {
    tab: ServerTab,
    session: SessionId,
    nav: Nav<ServerSection>,
    draft: ServerSettings,
    error: Option<String>,
    /// Input for a new GIPHY key (sent on its own, not with "Save Changes").
    gif_key: String,
    /// Admin password input (Roles tab).
    admin_pw: String,
    invite_form: super::moderation_ui::InviteForm,
}

impl ServerSettingsDialog {
    pub(super) fn is_for(&self, session: SessionId) -> bool {
        self.session == session
    }
}

impl RustTalkApp {
    pub(super) fn open_server_settings(&mut self, tab: ServerTab) {
        self.show_settings = false; // one settings page at a time
        self.recording_key = None;
        self.server_settings_dialog =
            Some(ServerSettingsDialog {
                tab,
                session: self.srv.id,
                nav: Nav::fresh(),
                draft: self.srv.server_settings.clone(),
                error: None,
                gif_key: String::new(),
                admin_pw: String::new(),
                invite_form: Default::default(),
            });
        self.request_admin_data();
    }

    /// Server settings as the page of the main window. Draws the window's
    /// `CentralPanel`. Leaving it (X, Esc, Discard) drops the draft.
    pub(super) fn ui_server_settings(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.server_settings_dialog.take() else { return };
        let mut nav = std::mem::take(&mut dlg.nav);
        let mut close = nav.escape(ctx, self.settings_escape_blocked(ctx));
        nav_panel(ctx, "server_settings_nav", |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                paint_server_icon(ui, r, &dlg.draft, &mut self.textures);
                ui.label(egui::RichText::new(super::sidebar::elide(ui, &dlg.draft.name, &egui::FontId::proportional(14.0), 150.0)).strong().color(theme::HEADER));
            });
            ui.add_space(10.0);
            if let Some((tab, section)) = nav_sidebar(ui, GROUPS, &mut nav, dlg.tab, "Server settings") {
                let changed = tab != dlg.tab;
                if changed {
                    dlg.tab = tab;
                    self.request_admin_data();
                }
                nav.goto(changed, page(tab).sections.first().map(|s| s.id), section);
            }
        });
        close |= close_panel(ctx, "server_settings_close");
        egui::TopBottomPanel::bottom("server_settings_bar")
            .frame(egui::Frame::default().fill(theme::BG).stroke(egui::Stroke::new(1.0_f32, theme::BORDER)).inner_margin(egui::Margin::symmetric(40, 8)))
            .show(ctx, |ui| close |= self.server_settings_bar(ui, &mut dlg));
        content_panel(ctx, "server_settings_page", &mut nav, |ui, nav| {
            page_body(ui, page(dlg.tab), nav, |ui, id| self.server_section(ui, &mut dlg, id));
        });
        dlg.nav = nav;
        if !close {
            self.server_settings_dialog = Some(dlg);
        }
    }

    /// Error, hint, "Save Changes" and "Discard"/"Close". True to leave the page.
    fn server_settings_bar(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) -> bool {
        let mut close = false;
        if let Some(e) = &dlg.error {
            ui.label(egui::RichText::new(e).color(theme::RED));
        }
        let admin = self.can(rusttalk_shared::protocol::Role::Admin);
        let changed = admin && dlg.draft != self.srv.server_settings;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save = ui.add_enabled(
                changed,
                egui::Button::new(egui::RichText::new("Save Changes").strong().color(theme::on_accent()))
                    .fill(theme::accent())
                    .min_size(egui::vec2(110.0, 32.0)),
            );
            if save.clicked() {
                if let Some(tx) = &self.srv.cmd_tx {
                    let _ = tx.send(Command::UpdateServerSettings(dlg.draft.clone()));
                }
                close = true;
            }
            if theme::ghost_button(ui, if changed { "Discard" } else { "Close" }).clicked() {
                close = true;
            }
            if changed {
                ui.label(egui::RichText::new("Unsaved changes").color(theme::YELLOW));
            } else if !admin {
                ui.label(egui::RichText::new(format!("{}  Only admins can change server settings.", icons::regular::LOCK)).color(theme::MUTED));
            }
        });
        close
    }

    fn server_section(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog, id: ServerSection) {
        ui.spacing_mut().item_spacing.y = 10.0;
        match id {
            S::Icon => self.server_icon(ui, dlg),
            S::Name => server_name(ui, dlg),
            S::Limits => server_limits(ui),
            S::AfkMover => self.server_afk(ui, dlg),
            S::AfkMore => server_afk_more(ui),
            S::Welcome => server_welcome(ui, dlg),
            S::JoinLeave => self.server_join_leave(ui, dlg),
            S::Rules => server_rules(ui, dlg),
            S::Notices => server_notices(ui, dlg),
            S::Gifs => self.server_integrations(ui, dlg),
            S::Roles => self.server_roles(ui, &mut dlg.admin_pw),
            S::Bans => self.server_bans(ui),
            S::Invites => self.server_invites(ui, &mut dlg.invite_form),
            S::Audit => self.server_audit(ui),
        }
    }

    fn server_icon(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        card(ui, |ui| {
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
                        if dlg.draft.icon.is_some() && theme::ghost_button(ui, egui::RichText::new("Remove").color(theme::RED)).clicked() {
                            dlg.draft.icon = None;
                        }
                    });
                });
            });
        });
    }

    fn server_afk(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        card(ui, |ui| {
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
    }
}

fn server_name(ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
    card(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut dlg.draft.name).char_limit(rusttalk_shared::protocol::MAX_NAME_LEN).desired_width(f32::INFINITY));
        ui.label(egui::RichText::new("Shown at the top of the sidebar and when people connect.").size(12.0).color(theme::MUTED));
    });
}

fn server_limits(ui: &mut egui::Ui) {
    card(ui, |ui| {
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

fn server_afk_more(ui: &mut egui::Ui) {
    card(ui, |ui| {
        setting_row(ui, "Also move idle users who don't talk", Some("Not muted, but silent for the timeout"), true, |ui| {
            toggle(ui, &mut false.clone());
        });
        ui.separator();
        setting_row(ui, "Exempt roles", Some("e.g. moderators are never moved"), true, |ui| {
            egui::ComboBox::from_id_salt("afk_exempt").selected_text("None").show_ui(ui, |_| {});
        });
    });
}

impl RustTalkApp {
    fn server_integrations(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let (icon, text, color) = if self.srv.gif_available {
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
            let label = if self.srv.gif_available { "Replace API key" } else { "GIPHY API key" };
            setting_row(ui, label, Some("Free at developers.giphy.com (create an app, type \"API\")"), false, |ui| {
                ui.add(egui::TextEdit::singleline(&mut dlg.gif_key).password(true).hint_text("paste key").desired_width(200.0));
            });
            ui.horizontal(|ui| {
                let key = dlg.gif_key.trim().to_string();
                if ui.add_enabled(!key.is_empty(), egui::Button::new(format!("{}  Check & save", icons::regular::FLOPPY_DISK))).clicked() {
                    if let Some(tx) = &self.srv.cmd_tx {
                        let _ = tx.send(Command::SetGifKey(Some(key)));
                    }
                    dlg.gif_key.clear();
                    self.set_status("Checking the GIPHY key...");
                }
                if self.srv.gif_available
                    && ui.add(egui::Button::new(egui::RichText::new(format!("{}  Remove key", icons::regular::TRASH)).color(theme::RED))).clicked()
                {
                    if let Some(tx) = &self.srv.cmd_tx {
                        let _ = tx.send(Command::SetGifKey(None));
                    }
                }
                if ui.link("Get a free key").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab("https://developers.giphy.com/dashboard/"));
                }
            });
        });
    }

    fn server_join_leave(&mut self, ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
        use rusttalk_shared::protocol::MAX_SERVER_MESSAGE_LEN;
        card(ui, |ui| {
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
    }
}

fn server_welcome(ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
    use rusttalk_shared::protocol::MAX_WELCOME_LEN;
    card(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut dlg.draft.welcome_message)
                .hint_text("Shown to everyone in the lobby, e.g. \"Welcome! Please read #announcements first.\"")
                .char_limit(MAX_WELCOME_LEN)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        char_count(ui, &dlg.draft.welcome_message, MAX_WELCOME_LEN);
    });
}

fn server_rules(ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
    use rusttalk_shared::protocol::MAX_WELCOME_LEN;
    card(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut dlg.draft.rules)
                .hint_text("Shown in the lobby below the welcome message, e.g. \"1. Be nice  2. No spam\"")
                .char_limit(MAX_WELCOME_LEN)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        char_count(ui, &dlg.draft.rules, MAX_WELCOME_LEN);
    });
}

fn server_notices(ui: &mut egui::Ui, dlg: &mut ServerSettingsDialog) {
    use rusttalk_shared::protocol::MAX_SERVER_MESSAGE_LEN;
    card(ui, |ui| {
        message_field(ui, "AFK notice", "Sent to users moved by the AFK mover (empty = default).  {channel}, {minutes}", rusttalk_shared::protocol::DEFAULT_AFK_NOTICE, &mut dlg.draft.afk_notice, MAX_SERVER_MESSAGE_LEN);
        ui.separator();
        message_field(ui, "Message of the day", "Shown to everyone when they connect (empty = none)", "Game night on Friday at 8 pm!", &mut dlg.draft.motd, MAX_SERVER_MESSAGE_LEN);
    });
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

#[cfg(test)]
mod page_tests {
    use super::super::layout_tests::{click, connect, frame, has_text, key, settle, text_rect, type_text, SIZES};
    use super::super::servers::tests::{app, drain, login};
    use super::*;
    use rusttalk_shared::protocol::Role;

    fn nav_labels() -> [&'static str; 8] {
        ["Overview", "AFK", "Messages", "Integrations", "Roles", "Bans", "Invites", "Audit Log"]
    }

    fn as_admin(app: &mut RustTalkApp, admin: bool) {
        let me = app.srv.my_id;
        app.srv.users.get_mut(&me).unwrap().role = if admin { Role::Admin } else { Role::Member };
    }

    fn draft(app: &mut RustTalkApp) -> &mut ServerSettings {
        &mut app.server_settings_dialog.as_mut().unwrap().draft
    }

    #[test]
    fn escape_dismisses_poke_without_discarding_server_draft() {
        for size in SIZES {
            let _devices = crate::audio::test_devices::Devices::new();
            let mut app = app();
            let mut commands = connect(&mut app);
            as_admin(&mut app, true);
            let original = app.srv.server_settings.name.clone();
            let mut time = 1.0;
            app.open_server_settings(ServerTab::Overview);
            draft(&mut app).name = "Unsaved draft".into();
            settle(&mut app, size, &mut time);
            app.receive_poke(app.srv.id, 2, "Friend".into(), "Hello".into());
            settle(&mut app, size, &mut time);
            key(&mut app, size, &mut time, egui::Key::Escape);
            assert!(app.poke_inbox.is_empty());
            assert!(app.server_settings_dialog.is_some(), "foreground poke must not discard draft");
            assert_eq!(draft(&mut app).name, "Unsaved draft");
            assert_eq!(app.srv.server_settings.name, original);
            assert!(!drain(&mut commands).iter().any(|c| c.contains("UpdateServerSettings")));
            key(&mut app, size, &mut time, egui::Key::Escape);
            assert!(app.server_settings_dialog.is_none());
        }
    }

    #[test]
    fn server_settings_is_a_page_with_the_client_settings_structure() {
        for size in SIZES {
            let mut app = app();
            let _rx = connect(&mut app);
            as_admin(&mut app, true);
            let mut time = 1.0;
            app.open_server_settings(ServerTab::Overview);
            let out = settle(&mut app, size, &mut time);

            assert_eq!(out.viewport_output.len(), 1);
            assert!(app.ctx.memory(|m| m.area_rect(egui::Id::new("server_settings_window"))).is_none(), "no floating window");
            assert!(!has_text(&out, "channel 2"), "replaces the main content");
            for label in nav_labels() {
                assert!(has_text(&out, label), "nav row {label}");
            }
            for heading in ["Server icon", "Server name", "Region & limits"] {
                assert!(has_text(&out, heading), "section {heading}");
            }
            assert!(has_text(&out, "Search") || text_rect(&out, icons::regular::MAGNIFYING_GLASS).is_some(), "search field");

            // Every tab opens and shows its own sections.
            for (label, expected) in [
                ("AFK", "Auto AFK mover"),
                ("Messages", "Welcome message"),
                ("Integrations", "GIF search is off - no API key set"),
                ("Roles", "Roles"),
                ("Bans", "Bans"),
                ("Invites", "Invites"),
                ("Audit Log", "Audit Log"),
            ] {
                let row = text_rect(&settle(&mut app, size, &mut time), label).expect("nav row");
                click(&mut app, size, &mut time, row.center());
                let out = settle(&mut app, size, &mut time);
                assert!(has_text(&out, expected), "{label}: {expected}");
            }
            // The last tab clicked is the current page.
            assert!(app.server_settings_dialog.as_ref().is_some_and(|d| d.tab == ServerTab::AuditLog));

            // Search narrows the sidebar; Escape clears it, a second one closes the page.
            let out = settle(&mut app, size, &mut time);
            let search = text_rect(&out, icons::regular::MAGNIFYING_GLASS).expect("search");
            click(&mut app, size, &mut time, search.center() + egui::vec2(60.0, 0.0));
            type_text(&mut app, size, &mut time, "giphy");
            let out = settle(&mut app, size, &mut time);
            assert!(has_text(&out, "Integrations") && !has_text(&out, "Bans"), "only matching pages remain");
            key(&mut app, size, &mut time, egui::Key::Escape);
            assert!(app.server_settings_dialog.is_some(), "first Escape clears the search");
            assert!(has_text(&settle(&mut app, size, &mut time), "Bans"));
            key(&mut app, size, &mut time, egui::Key::Escape);
            assert!(app.server_settings_dialog.is_none(), "second Escape closes");
            assert!(has_text(&settle(&mut app, size, &mut time), "channel 2"), "main page is back");
        }
    }

    #[test]
    fn discard_sends_nothing_and_save_sends_the_draft_once() {
        let size = SIZES[0];
        let mut app = app();
        let mut rx = connect(&mut app);
        as_admin(&mut app, true);
        let original = app.srv.server_settings.name.clone();
        let mut time = 1.0;

        // Cancel via the X button: draft is dropped, server state untouched.
        app.open_server_settings(ServerTab::Overview);
        draft(&mut app).name = "Renamed".into();
        let out = settle(&mut app, size, &mut time);
        assert!(has_text(&out, "Unsaved changes"));
        let x = text_rect(&out, icons::regular::X).expect("close button");
        click(&mut app, size, &mut time, x.center());
        assert!(app.server_settings_dialog.is_none());
        assert_eq!(app.srv.server_settings.name, original);
        assert!(!drain(&mut rx).iter().any(|c| c.contains("UpdateServerSettings")));

        // Cancel via Discard.
        app.open_server_settings(ServerTab::Overview);
        draft(&mut app).name = "Renamed".into();
        let out = settle(&mut app, size, &mut time);
        click(&mut app, size, &mut time, text_rect(&out, "Discard").unwrap().center());
        assert!(app.server_settings_dialog.is_none());
        assert!(!drain(&mut rx).iter().any(|c| c.contains("UpdateServerSettings")));

        // Save.
        app.open_server_settings(ServerTab::Overview);
        draft(&mut app).name = "Renamed".into();
        let out = settle(&mut app, size, &mut time);
        click(&mut app, size, &mut time, text_rect(&out, "Save Changes").unwrap().center());
        assert!(app.server_settings_dialog.is_none());
        let sent: Vec<_> = drain(&mut rx).into_iter().filter(|c| c.contains("UpdateServerSettings")).collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].contains("Renamed"));
        assert_eq!(app.srv.server_settings.name, original, "server state only changes when the server confirms");
    }

    #[test]
    fn members_cannot_save_and_the_page_says_why() {
        let size = SIZES[1];
        let mut app = app();
        let mut rx = connect(&mut app);
        as_admin(&mut app, false);
        let mut time = 1.0;
        app.open_server_settings(ServerTab::Overview);
        draft(&mut app).name = "Renamed".into();
        let out = settle(&mut app, size, &mut time);
        assert!(has_text(&out, &format!("{}  Only admins can change server settings.", icons::regular::LOCK)));
        assert!(!has_text(&out, "Unsaved changes"));
        click(&mut app, size, &mut time, text_rect(&out, "Save Changes").unwrap().center());
        assert!(app.server_settings_dialog.is_some(), "disabled save does nothing");
        assert!(!drain(&mut rx).iter().any(|c| c.contains("UpdateServerSettings")));
    }

    #[test]
    fn page_is_bound_to_its_server_session() {
        let size = SIZES[0];
        // Switching the viewed server drops the draft.
        let mut app = app();
        let _rx = connect(&mut app);
        let (other, _other_rx) = login(&mut app, "other.invalid:7331", 1);
        let mut time = 1.0;
        app.open_server_settings(ServerTab::Overview);
        settle(&mut app, size, &mut time);
        app.view_session(other);
        settle(&mut app, size, &mut time);
        assert!(app.server_settings_dialog.is_none());

        // Losing the connection drops it too, even if nothing cleaned up before.
        let mut app = super::super::servers::tests::app();
        let _rx = connect(&mut app);
        app.open_server_settings(ServerTab::Overview);
        app.srv.cmd_tx = None;
        let out = settle(&mut app, size, &mut time);
        assert!(app.server_settings_dialog.is_none());
        assert!(!has_text(&out, "Server icon"));

        // Opening client settings replaces the server page and vice versa.
        let mut app = super::super::servers::tests::app();
        let _rx = connect(&mut app);
        app.open_server_settings(ServerTab::Overview);
        app.open_settings(SettingsPage::Voice);
        assert!(app.server_settings_dialog.is_none() && app.show_settings);
        app.open_server_settings(ServerTab::Overview);
        assert!(!app.show_settings);
        let _ = frame(&mut app, size, 99.0, vec![]);
    }
}
