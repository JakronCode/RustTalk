//! Roles & moderation in the client: permission helpers, the moderation part
//! of the user menu (move / kick / ban / role), the kick/ban dialog, the
//! admin tabs of Server Settings (roles, bans, invites, audit log) and
//! invite links (`rusttalk://host:port/CODE`).

use rusttalk_shared::protocol::{AuditEntry, BanInfo, ClientMessage, InviteInfo, Role, RoleEntry, DEFAULT_PORT};

use super::theme::{self, caption, card, setting_row};
use super::*;

/// Lists for the admin tabs (`AdminData`).
#[derive(Default)]
pub(super) struct AdminData {
    pub(super) roles: Vec<RoleEntry>,
    pub(super) bans: Vec<BanInfo>,
    pub(super) invites: Vec<InviteInfo>,
    pub(super) audit: Vec<AuditEntry>,
    pub(super) loaded: bool,
}

/// Kick / ban dialog.
pub(super) struct ModDialog {
    target: u64,
    name: String,
    ban: bool,
    reason: String,
    /// Ban duration (None = permanent).
    minutes: Option<u32>,
}

/// Inputs of the Invites tab.
pub(super) struct InviteForm {
    max_uses: Option<u32>,
    hours: Option<u32>,
}

impl Default for InviteForm {
    fn default() -> Self {
        Self { max_uses: None, hours: Some(24 * 7) }
    }
}

const BAN_DURATIONS: [(Option<u32>, &str); 5] =
    [(Some(60), "1 hour"), (Some(60 * 24), "1 day"), (Some(60 * 24 * 7), "7 days"), (Some(60 * 24 * 30), "30 days"), (None, "Permanent")];

/// Parses `rusttalk://host[:port][/CODE]` -> (address, invite code).
pub(super) fn parse_invite_link(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.trim().strip_prefix("rusttalk://").or_else(|| text.trim().strip_prefix("RUSTTALK://"))?;
    let rest = rest.trim_end_matches('/');
    let (host, code) = match rest.split_once('/') {
        Some((h, c)) => (h, Some(c.trim_matches('/').to_ascii_uppercase()).filter(|c| !c.is_empty())),
        None => (rest, None),
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:{DEFAULT_PORT}") };
    Some((addr, code))
}

/// Invite link for a server address (as typed by the admin) and a code.
pub(super) fn invite_link(server_address: &str, code: &str) -> String {
    let addr = server_address.trim().trim_start_matches("rusttalk://");
    let host = addr.split('/').next().unwrap_or(addr);
    format!("rusttalk://{host}/{code}")
}

/// "12.11. 14:05" (UTC like the chat).
fn short_date(ts: u64) -> String {
    let days = ts / 86_400;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{d:02}.{m:02}. {}", format_time(ts))
}

pub(super) fn role_color(role: Role) -> egui::Color32 {
    match role {
        Role::Admin => theme::RED,
        Role::Moderator => theme::BLURPLE,
        Role::Member => theme::MUTED,
    }
}

impl RustTalkApp {
    pub(super) fn my_role(&self) -> Role {
        self.srv.users.get(&self.srv.my_id).map(|u| u.role).unwrap_or_default()
    }

    /// May I do something that needs `need`? (Open server: yes.)
    pub(super) fn can(&self, need: Role) -> bool {
        self.srv.open_permissions || self.my_role() >= need
    }

    fn send_raw(&self, msg: ClientMessage) {
        if let Some(tx) = &self.srv.cmd_tx {
            let _ = tx.send(Command::Raw(msg));
        }
    }

    /// Admin tabs need fresh lists.
    pub(super) fn request_admin_data(&mut self) {
        if self.can(Role::Admin) {
            self.send_raw(ClientMessage::RequestAdminData);
        }
    }

    /// Moderation entries of the right-click menu of another user.
    pub(super) fn moderation_menu(&mut self, ui: &mut egui::Ui, user: &UserInfo) {
        let mine = self.my_role();
        // Same or higher role is off limits (unless the server is open).
        let above = self.srv.open_permissions || mine > user.role;
        if !self.can(Role::Moderator) || !above {
            return;
        }
        theme::menu_separator(ui);
        ui.menu_button(format!("{}  Move to", icons::regular::ARROWS_LEFT_RIGHT), |ui| {
            let mut targets: Vec<(u64, String)> = Vec::new();
            if let Some(spawn) = self.spawn_channel_id() {
                targets.push((spawn, self.channel_name(spawn)));
            }
            for parent in std::iter::once(None).chain(self.sorted_children(None).into_iter().filter(|c| c.kind == ChannelKind::Category).map(|c| Some(c.id))) {
                for ch in self.sorted_children(parent).into_iter().filter(|c| c.kind == ChannelKind::Voice) {
                    targets.push((ch.id, ch.name));
                }
            }
            for (id, name) in targets {
                let here = user.channel_id == id;
                if ui.add_enabled(!here, egui::Button::new(name)).clicked() {
                    self.send_raw(ClientMessage::MoveUser { client_id: user.id, channel_id: id });
                    ui.close_menu();
                }
            }
        });
        if theme::menu_item(ui, icons::regular::SIGN_OUT, "Kick...", false).clicked() {
            self.mod_dialog = Some(ModDialog { target: user.id, name: user.name.clone(), ban: false, reason: String::new(), minutes: None });
            ui.close_menu();
        }
        if self.can(Role::Admin) {
            if theme::menu_item(ui, icons::regular::PROHIBIT, "Ban...", true).clicked() {
                self.mod_dialog = Some(ModDialog { target: user.id, name: user.name.clone(), ban: true, reason: String::new(), minutes: Some(60 * 24) });
                ui.close_menu();
            }
            ui.menu_button(format!("{}  Role", icons::regular::SHIELD_CHECK), |ui| {
                for role in [Role::Member, Role::Moderator, Role::Admin] {
                    if ui.radio(user.role == role, role.label()).clicked() && user.role != role {
                        self.send_raw(ClientMessage::SetRole { client_id: user.id, role });
                        ui.close_menu();
                    }
                }
            });
        }
    }

    /// Kick / ban confirmation with reason (and duration).
    pub(super) fn ui_mod_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.mod_dialog.take() else { return };
        let mut open = true;
        let mut done = false;
        let title = if d.ban { format!("Ban {}", d.name) } else { format!("Kick {}", d.name) };
        theme::dialog_window("mod_dialog")
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size([400.0, 0.0])
            .show(ctx, |ui| {
                theme::dialog_header(ui, &title, &mut open);
                let text = if d.ban {
                    format!("{} is disconnected and cannot come back (same RustTalk or same IP) until the ban ends.", d.name)
                } else {
                    format!("{} is disconnected. They can reconnect right away.", d.name)
                };
                ui.label(egui::RichText::new(text).color(theme::TEXT));
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut d.reason)
                        .hint_text("Reason (optional, shown to them)")
                        .char_limit(rusttalk_shared::protocol::MAX_REASON_LEN)
                        .desired_width(f32::INFINITY),
                );
                if d.ban {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Duration");
                        for (m, label) in BAN_DURATIONS {
                            ui.selectable_value(&mut d.minutes, m, label);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if d.ban { format!("{}  Ban", icons::regular::PROHIBIT) } else { format!("{}  Kick", icons::regular::SIGN_OUT) };
                    if theme::danger_button(ui, &label).clicked() {
                        let reason = d.reason.trim().to_string();
                        let msg = if d.ban {
                            ClientMessage::Ban { client_id: d.target, reason, minutes: d.minutes }
                        } else {
                            ClientMessage::Kick { client_id: d.target, reason }
                        };
                        self.send_raw(msg);
                        done = true;
                    }
                    if theme::ghost_button(ui, "Cancel").clicked() {
                        done = true;
                    }
                });
            });
        if open && !done && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.mod_dialog = Some(d);
        }
    }

    // ----- Server Settings tabs ------------------------------------------

    pub(super) fn server_roles(&mut self, ui: &mut egui::Ui, admin_pw: &mut String) {
        if self.srv.open_permissions {
            card(ui, |ui| {
                caption(ui, "Open server");
                ui.label(
                    egui::RichText::new(
                        "Nobody is admin yet, so everyone can change channels and settings, kick and ban. \
                         Become the first admin to protect the server - after that only admins (and moderators) can.",
                    )
                    .color(theme::TEXT),
                );
                ui.add_space(6.0);
                if theme::primary_button(ui, &format!("{}  Become admin", icons::regular::CROWN)).clicked() {
                    self.send_raw(ClientMessage::ClaimAdmin { password: None });
                }
            });
        }
        let mine = self.my_role();
        card(ui, |ui| {
            caption(ui, "Your role");
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icons::regular::SHIELD_CHECK).size(18.0).color(role_color(mine)));
                ui.label(egui::RichText::new(mine.label()).strong().color(theme::HEADER));
            });
            if !self.srv.open_permissions && mine != Role::Admin {
                ui.separator();
                setting_row(ui, "Admin password", Some("Set by the server owner in server.toml (admin_password)"), false, |ui| {
                    ui.add(egui::TextEdit::singleline(admin_pw).password(true).desired_width(160.0));
                    if ui.add_enabled(!admin_pw.is_empty(), egui::Button::new("Log in")).clicked() {
                        self.send_raw(ClientMessage::ClaimAdmin { password: Some(std::mem::take(admin_pw)) });
                    }
                });
            }
        });
        if !self.can(Role::Admin) || self.srv.open_permissions {
            return;
        }
        card(ui, |ui| {
            caption(ui, "Roles");
            ui.label(
                egui::RichText::new("Roles belong to a person's RustTalk (not their name). Give roles to online people via right-click in the channel list.")
                    .size(12.0)
                    .color(theme::MUTED),
            );
            ui.separator();
            if self.srv.admin.roles.is_empty() {
                ui.label(egui::RichText::new("No moderators or admins yet.").color(theme::MUTED));
            }
            let roles = self.srv.admin.roles.clone();
            for r in roles {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icons::regular::SHIELD_CHECK).color(role_color(r.role)));
                    ui.label(egui::RichText::new(&r.name).color(theme::HEADER));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut role = r.role;
                        egui::ComboBox::from_id_salt(("role", &r.uid)).width(120.0).selected_text(role.label()).show_ui(ui, |ui| {
                            for x in [Role::Member, Role::Moderator, Role::Admin] {
                                ui.selectable_value(&mut role, x, x.label());
                            }
                        });
                        if role != r.role {
                            self.send_raw(ClientMessage::SetRoleByUid { uid: r.uid.clone(), role });
                        }
                    });
                });
            }
        });
        role_legend(ui);
    }

    pub(super) fn server_bans(&mut self, ui: &mut egui::Ui) {
        if !self.admin_tab_allowed(ui) {
            return;
        }
        card(ui, |ui| {
            caption(ui, "Banned");
            if self.srv.admin.bans.is_empty() {
                ui.label(egui::RichText::new("Nobody is banned. Ban someone via right-click on their name.").color(theme::MUTED));
            }
            let now = Self::now();
            let bans = self.srv.admin.bans.clone();
            for (i, b) in bans.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&b.name).strong().color(theme::HEADER));
                        let until = match b.until {
                            Some(t) if t > now => format!("{} min left", (t - now + 59) / 60),
                            Some(_) => "ending".to_string(),
                            None => "permanent".to_string(),
                        };
                        let reason = if b.reason.is_empty() { "no reason".to_string() } else { b.reason.clone() };
                        ui.label(egui::RichText::new(format!("{reason}  -  {until}  -  by {} on {}", b.by, short_date(b.created))).size(12.0).color(theme::MUTED));
                        if let Some(ip) = &b.ip {
                            ui.label(egui::RichText::new(format!("IP {ip}")).size(11.0).color(theme::DISABLED));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Unban").clicked() {
                            self.send_raw(ClientMessage::Unban { ban_id: b.id });
                        }
                    });
                });
            }
        });
    }

    pub(super) fn server_invites(&mut self, ui: &mut egui::Ui, form: &mut InviteForm) {
        if !self.admin_tab_allowed(ui) {
            return;
        }
        card(ui, |ui| {
            caption(ui, "New invite");
            ui.label(
                egui::RichText::new("Invite links open RustTalk and connect directly - no server password needed.")
                    .size(12.0)
                    .color(theme::MUTED),
            );
            ui.horizontal(|ui| {
                ui.label("Uses");
                egui::ComboBox::from_id_salt("invite_uses")
                    .width(90.0)
                    .selected_text(form.max_uses.map(|m| m.to_string()).unwrap_or_else(|| "No limit".into()))
                    .show_ui(ui, |ui| {
                        for m in [Some(1), Some(5), Some(10), Some(25), None] {
                            ui.selectable_value(&mut form.max_uses, m, m.map(|m| m.to_string()).unwrap_or_else(|| "No limit".into()));
                        }
                    });
                ui.label("Expires after");
                egui::ComboBox::from_id_salt("invite_hours")
                    .width(90.0)
                    .selected_text(hours_label(form.hours))
                    .show_ui(ui, |ui| {
                        for h in [Some(1), Some(24), Some(24 * 7), Some(24 * 30), None] {
                            ui.selectable_value(&mut form.hours, h, hours_label(h));
                        }
                    });
                if theme::primary_button(ui, &format!("{}  Create", icons::regular::LINK)).clicked() {
                    self.send_raw(ClientMessage::CreateInvite { max_uses: form.max_uses, hours: form.hours });
                }
            });
        });
        let address = self.config.server_address.clone();
        card(ui, |ui| {
            caption(ui, "Active invites");
            if self.srv.admin.invites.is_empty() {
                ui.label(egui::RichText::new("No active invites.").color(theme::MUTED));
            }
            let now = Self::now();
            let invites = self.srv.admin.invites.clone();
            for (i, inv) in invites.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let link = invite_link(&address, &inv.code);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&link).monospace().color(theme::HEADER));
                        let uses = match inv.max_uses {
                            Some(m) => format!("{}/{m} used", inv.uses),
                            None => format!("{} used", inv.uses),
                        };
                        let exp = match inv.expires {
                            Some(t) if t > now => format!("expires in {}", duration_label(t - now)),
                            Some(_) => "expired".into(),
                            None => "never expires".into(),
                        };
                        ui.label(egui::RichText::new(format!("{uses}  -  {exp}  -  by {}", inv.by)).size(12.0).color(theme::MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new(icons::regular::TRASH).color(theme::RED)).on_hover_text("Delete").clicked() {
                            self.send_raw(ClientMessage::DeleteInvite { code: inv.code.clone() });
                        }
                        if ui.button(format!("{}  Copy", icons::regular::COPY)).clicked() {
                            ui.ctx().copy_text(link.clone());
                            self.set_status("Invite link copied");
                        }
                    });
                });
            }
        });
    }

    pub(super) fn server_audit(&mut self, ui: &mut egui::Ui) {
        if !self.admin_tab_allowed(ui) {
            return;
        }
        card(ui, |ui| {
            caption(ui, "Audit log (newest first)");
            if self.srv.admin.audit.is_empty() {
                ui.label(egui::RichText::new("Nothing logged yet.").color(theme::MUTED));
            }
            for e in &self.srv.admin.audit {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(short_date(e.timestamp)).monospace().size(11.5).color(theme::MUTED));
                    ui.label(egui::RichText::new(&e.actor).strong().color(theme::HEADER));
                    ui.label(egui::RichText::new(&e.action).color(theme::TEXT));
                });
            }
        });
    }

    /// Admin tabs: explain instead of showing nothing.
    fn admin_tab_allowed(&mut self, ui: &mut egui::Ui) -> bool {
        if self.srv.open_permissions {
            theme::wip_banner(ui, "Open server: become admin first (tab \"Roles\").");
            return false;
        }
        if !self.can(Role::Admin) {
            theme::wip_banner(ui, "Only admins can see this.");
            return false;
        }
        if !self.srv.admin.loaded {
            ui.spinner();
        }
        true
    }
}

fn hours_label(h: Option<u32>) -> String {
    match h {
        None => "Never".into(),
        Some(h) => duration_label(u64::from(h) * 3600),
    }
}

fn duration_label(secs: u64) -> String {
    let h = secs / 3600;
    if h >= 48 {
        format!("{} days", h / 24)
    } else if h >= 1 {
        format!("{h} h")
    } else {
        format!("{} min", (secs / 60).max(1))
    }
}

fn role_legend(ui: &mut egui::Ui) {
    card(ui, |ui| {
        caption(ui, "What roles may do");
        for (role, text) in [
            (Role::Member, "Chat, talk, join channels"),
            (Role::Moderator, "+ move and kick people, delete any message"),
            (Role::Admin, "+ channels, server settings, roles, bans, invites, audit log"),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{}  {}", icons::regular::SHIELD_CHECK, role.label())).strong().color(role_color(role)));
                ui.label(egui::RichText::new(text).color(theme::TEXT));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_links() {
        assert_eq!(parse_invite_link("rusttalk://10.0.0.5:7331/abcd2345"), Some(("10.0.0.5:7331".into(), Some("ABCD2345".into()))));
        assert_eq!(parse_invite_link("rusttalk://lan.box/XYZ/"), Some(("lan.box:7331".into(), Some("XYZ".into()))), "default port");
        assert_eq!(parse_invite_link("rusttalk://10.0.0.5"), Some(("10.0.0.5:7331".into(), None)));
        assert_eq!(parse_invite_link("10.0.0.5"), None, "plain address");
        assert_eq!(parse_invite_link("rusttalk://"), None);
        let link = invite_link("10.0.0.5:7331", "ABCD2345");
        assert_eq!(link, "rusttalk://10.0.0.5:7331/ABCD2345");
        assert_eq!(parse_invite_link(&link).unwrap().1.as_deref(), Some("ABCD2345"), "roundtrip");
    }

    #[test]
    fn dates_and_durations() {
        assert_eq!(short_date(0), "01.01. 00:00");
        assert_eq!(short_date(1_794_423_183), "11.11. 18:53");
        assert_eq!(duration_label(30), "1 min");
        assert_eq!(duration_label(7200), "2 h");
        assert_eq!(duration_label(7 * 86_400), "7 days");
    }
}
