//! Identities, roles, bans, invites and the audit log (pure logic; the
//! `State` persists changes to the DB).
//!
//! Identity: every client sends a secret (`Hello.identity`); the server
//! only keeps its hash ("uid"). Roles and bans hang on the uid, bans also
//! on the IP. As long as nobody is admin, the server is "open": everyone may
//! do everything (like before roles existed).

use std::collections::{HashMap, VecDeque};

use rusttalk_shared::protocol::{AuditEntry, BanInfo, InviteInfo, Role, RoleEntry, IDENTITY_HEX_LEN};
use sha2::{Digest, Sha256};

/// Kept audit entries.
pub const AUDIT_CAP: usize = 500;
/// Characters of invite codes (no 0/O/1/I).
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;

/// Public id of an identity secret (hex, 32 chars). `None` = invalid secret.
pub fn uid_from_identity(identity: &str) -> Option<String> {
    let id = identity.trim();
    if id.len() != IDENTITY_HEX_LEN || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hash = Sha256::digest(id.to_ascii_lowercase().as_bytes());
    Some(hash.iter().take(16).map(|b| format!("{b:02x}")).collect())
}

#[derive(Default)]
pub struct Moderation {
    roles: HashMap<String, RoleEntry>,
    bans: Vec<BanInfo>,
    next_ban_id: u64,
    invites: Vec<InviteInfo>,
    audit: VecDeque<AuditEntry>,
    /// Never open, even before the first admin (admin password required).
    closed: bool,
}

impl Moderation {
    pub fn load(roles: Vec<RoleEntry>, bans: Vec<BanInfo>, invites: Vec<InviteInfo>, audit: Vec<AuditEntry>) -> Self {
        let next_ban_id = bans.iter().map(|b| b.id).max().unwrap_or(0) + 1;
        Self {
            roles: roles.into_iter().map(|r| (r.uid.clone(), r)).collect(),
            bans,
            next_ban_id,
            invites,
            audit: audit.into_iter().collect(),
            closed: false,
        }
    }

    pub fn set_closed(&mut self, closed: bool) {
        self.closed = closed;
    }

    // --- roles ---

    pub fn role_of(&self, uid: &str) -> Role {
        self.roles.get(uid).map(|r| r.role).unwrap_or_default()
    }

    pub fn has_admin(&self) -> bool {
        self.roles.values().any(|r| r.role == Role::Admin)
    }

    /// No admin yet (and not closed): everyone may do everything.
    pub fn is_open(&self) -> bool {
        !self.closed && !self.has_admin()
    }

    /// May someone with `role` do something that needs `need`?
    pub fn allowed(&self, role: Role, need: Role) -> bool {
        self.is_open() || role >= need
    }

    /// Sets a role; refuses to remove the last admin. Returns the stored entry
    /// (`role == Member` removes the entry).
    pub fn set_role(&mut self, uid: &str, name: &str, role: Role) -> Result<RoleEntry, String> {
        let current = self.role_of(uid);
        if current == Role::Admin && role != Role::Admin && self.roles.values().filter(|r| r.role == Role::Admin).count() == 1 {
            return Err("There must be at least one admin".to_string());
        }
        let entry = RoleEntry { uid: uid.to_string(), name: name.to_string(), role };
        if role == Role::Member {
            self.roles.remove(uid);
        } else {
            self.roles.insert(uid.to_string(), entry.clone());
        }
        Ok(entry)
    }

    /// Keeps the stored name of a role holder current.
    pub fn touch_name(&mut self, uid: &str, name: &str) -> Option<RoleEntry> {
        let entry = self.roles.get_mut(uid)?;
        if entry.name == name {
            return None;
        }
        entry.name = name.to_string();
        Some(entry.clone())
    }

    pub fn roles(&self) -> Vec<RoleEntry> {
        let mut v: Vec<RoleEntry> = self.roles.values().cloned().collect();
        v.sort_by(|a, b| b.role.cmp(&a.role).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }

    // --- bans ---

    /// Active ban for this uid or IP (expired bans are dropped; returned in `expired`).
    pub fn find_ban(&mut self, uid: &str, ip: Option<&str>, now: u64) -> (Option<BanInfo>, Vec<u64>) {
        let expired: Vec<u64> = self.bans.iter().filter(|b| b.until.is_some_and(|u| u <= now)).map(|b| b.id).collect();
        self.bans.retain(|b| !expired.contains(&b.id));
        let hit = self
            .bans
            .iter()
            .find(|b| b.uid.as_deref() == Some(uid) || (ip.is_some() && b.ip.as_deref() == ip))
            .cloned();
        (hit, expired)
    }

    pub fn add_ban(&mut self, name: &str, uid: Option<String>, ip: Option<String>, reason: &str, minutes: Option<u32>, by: &str, now: u64) -> BanInfo {
        let ban = BanInfo {
            id: self.next_ban_id,
            name: name.to_string(),
            uid,
            ip,
            reason: reason.to_string(),
            created: now,
            until: minutes.map(|m| now + u64::from(m) * 60),
            by: by.to_string(),
        };
        self.next_ban_id += 1;
        self.bans.push(ban.clone());
        ban
    }

    pub fn unban(&mut self, id: u64) -> Option<BanInfo> {
        let pos = self.bans.iter().position(|b| b.id == id)?;
        Some(self.bans.remove(pos))
    }

    pub fn bans(&self) -> Vec<BanInfo> {
        self.bans.clone()
    }

    // --- invites ---

    pub fn create_invite(&mut self, max_uses: Option<u32>, hours: Option<u32>, by: &str, now: u64) -> InviteInfo {
        let code = loop {
            let c: String = (0..CODE_LEN).map(|_| CODE_CHARS[rand::random_range(0..CODE_CHARS.len())] as char).collect();
            if !self.invites.iter().any(|i| i.code == c) {
                break c;
            }
        };
        let invite = InviteInfo {
            code,
            created: now,
            expires: hours.map(|h| now + u64::from(h) * 3600),
            max_uses: max_uses.filter(|m| *m > 0),
            uses: 0,
            by: by.to_string(),
        };
        self.invites.push(invite.clone());
        invite
    }

    /// Uses a valid invite once. Returns the updated invite.
    pub fn use_invite(&mut self, code: &str, now: u64) -> Option<InviteInfo> {
        let code = code.trim().to_ascii_uppercase();
        let inv = self.invites.iter_mut().find(|i| i.code == code)?;
        if inv.expires.is_some_and(|e| e <= now) || inv.max_uses.is_some_and(|m| inv.uses >= m) {
            return None;
        }
        inv.uses += 1;
        Some(inv.clone())
    }

    pub fn delete_invite(&mut self, code: &str) -> bool {
        let before = self.invites.len();
        self.invites.retain(|i| i.code != code);
        self.invites.len() != before
    }

    /// Valid (not expired, not used up) invites.
    pub fn invites(&self, now: u64) -> Vec<InviteInfo> {
        self.invites
            .iter()
            .filter(|i| !i.expires.is_some_and(|e| e <= now) && !i.max_uses.is_some_and(|m| i.uses >= m))
            .cloned()
            .collect()
    }

    // --- audit ---

    pub fn log(&mut self, actor: &str, action: String, now: u64) -> AuditEntry {
        let entry = AuditEntry { timestamp: now, actor: actor.to_string(), action };
        if self.audit.len() >= AUDIT_CAP {
            self.audit.pop_front();
        }
        self.audit.push_back(entry.clone());
        entry
    }

    /// Newest first.
    pub fn audit(&self) -> Vec<AuditEntry> {
        self.audit.iter().rev().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_hashing() {
        let secret = "ab".repeat(32);
        let uid = uid_from_identity(&secret).unwrap();
        assert_eq!(uid.len(), 32);
        assert_eq!(uid_from_identity(&secret.to_uppercase()), Some(uid.clone()), "case-insensitive");
        assert_ne!(uid_from_identity(&"cd".repeat(32)), Some(uid));
        assert!(!secret.contains(&uid_from_identity(&secret).unwrap()), "uid is not the secret");
        assert_eq!(uid_from_identity("short"), None);
        assert_eq!(uid_from_identity(&"zz".repeat(32)), None, "not hex");
    }

    #[test]
    fn open_until_first_admin_and_last_admin_protected() {
        let mut m = Moderation::default();
        assert!(m.is_open());
        assert!(m.allowed(Role::Member, Role::Admin), "open: everyone may");
        m.set_role("u1", "anna", Role::Admin).unwrap();
        assert!(!m.is_open());
        assert!(!m.allowed(Role::Member, Role::Moderator));
        assert!(m.allowed(Role::Moderator, Role::Moderator));
        assert!(!m.allowed(Role::Moderator, Role::Admin));
        assert!(m.set_role("u1", "anna", Role::Member).is_err(), "last admin stays");
        m.set_role("u2", "ben", Role::Admin).unwrap();
        m.set_role("u1", "anna", Role::Member).unwrap();
        assert_eq!(m.role_of("u1"), Role::Member);
        assert_eq!(m.roles().len(), 1, "members are not stored");
        assert_eq!(m.touch_name("u2", "Ben").map(|e| e.name), Some("Ben".into()));
        assert_eq!(m.touch_name("u2", "Ben"), None, "unchanged");
    }

    #[test]
    fn closed_is_never_open() {
        let mut m = Moderation::default();
        m.set_closed(true);
        assert!(!m.is_open(), "no admin yet, still closed");
        assert!(!m.allowed(Role::Member, Role::Moderator));
        m.set_role("u1", "anna", Role::Admin).unwrap();
        assert!(!m.is_open());
        assert!(m.allowed(Role::Admin, Role::Admin));
    }

    #[test]
    fn bans_by_uid_or_ip_and_expiry() {
        let mut m = Moderation::default();
        let b = m.add_ban("eve", Some("u9".into()), Some("10.0.0.9".into()), "spam", Some(10), "anna", 1000);
        assert_eq!(b.until, Some(1600));
        assert!(m.find_ban("u9", None, 1100).0.is_some(), "by uid");
        assert!(m.find_ban("other", Some("10.0.0.9"), 1100).0.is_some(), "by ip");
        assert!(m.find_ban("other", Some("10.0.0.8"), 1100).0.is_none());
        let (hit, expired) = m.find_ban("u9", None, 1600);
        assert!(hit.is_none() && expired == vec![b.id], "expired");
        let p = m.add_ban("mal", Some("u8".into()), None, "x", None, "anna", 0);
        assert!(m.find_ban("u8", None, u64::MAX / 2).0.is_some(), "permanent");
        assert!(m.unban(p.id).is_some());
        assert!(m.find_ban("u8", None, 0).0.is_none());
    }

    #[test]
    fn invites_limit_uses_and_time() {
        let mut m = Moderation::default();
        let i = m.create_invite(Some(2), Some(1), "anna", 0);
        assert_eq!(i.code.len(), CODE_LEN);
        assert!(m.use_invite(&i.code.to_lowercase(), 10).is_some(), "case-insensitive");
        assert!(m.use_invite(&i.code, 10).is_some());
        assert!(m.use_invite(&i.code, 10).is_none(), "used up");
        let t = m.create_invite(None, Some(1), "anna", 0);
        assert!(m.use_invite(&t.code, 3599).is_some());
        assert!(m.use_invite(&t.code, 3600).is_none(), "expired");
        assert!(m.invites(10).iter().all(|x| x.code != i.code), "used-up invites are hidden");
        assert!(m.use_invite("NOPE", 0).is_none());
        let u = m.create_invite(None, None, "anna", 0);
        assert!(m.delete_invite(&u.code));
        assert!(m.use_invite(&u.code, 0).is_none());
    }

    #[test]
    fn audit_is_capped_newest_first() {
        let mut m = Moderation::default();
        for i in 0..(AUDIT_CAP + 5) {
            m.log("anna", format!("action {i}"), i as u64);
        }
        let a = m.audit();
        assert_eq!(a.len(), AUDIT_CAP);
        assert_eq!(a[0].action, format!("action {}", AUDIT_CAP + 4));
    }
}
