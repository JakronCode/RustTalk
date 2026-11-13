//! What one server may make the client hold. State outside these bounds is
//! a protocol violation: only that server is disconnected (fail closed).
//! Chat history is not a violation; it is trimmed, oldest first.
//!
//! Accounted payload per server (`MAX_RETAINED_BYTES`): users and channels by count
//! and name length, chat text and images by their budgets, admin lists by
//! bytes, settings by protocol limits, plus encoded bytes in the network queue.
//! Allocator capacity, decoded structures, textures and media workers are separate.

use rusttalk_shared::protocol::{
    AuditEntry, BanInfo, ChannelInfo, ChatRecord, InviteInfo, LogEntry, RoleEntry, ServerMessage,
    ServerSettings, UserInfo, MAX_CHAT_LEN, MAX_ICON_BYTES, MAX_MEDIA_BYTES, MAX_NAME_LEN,
    MAX_POKE_LEN, MAX_SERVER_MESSAGE_LEN, MAX_WELCOME_LEN, SPAWN_LOG_CAP,
};

use super::session::ServerSession;
#[cfg(test)]
use super::session::{MAX_IMAGE_BYTES, MAX_TEXT_BYTES};

pub(super) const MAX_USERS: usize = 4096;
pub(super) const MAX_CHANNELS: usize = 1024;
/// User, channel and server names in characters. The server enforces
/// `MAX_NAME_LEN` for typed names; names from its config get slack.
pub(super) const MAX_LABEL_CHARS: usize = 4 * MAX_NAME_LEN;
/// Other server-made text (activity lines, notices, errors, GIF fields) in bytes.
pub(super) const MAX_TEXT_LEN: usize = 16 * 1024;
/// Records in the Welcome chat history (the server sends 100 per channel).
pub(super) const MAX_HISTORY: usize = 20_000;
/// Roles, bans, invites and audit log together, in bytes.
pub(super) const MAX_ADMIN_BYTES: usize = 4 * 1024 * 1024;
/// GIF results in one page (the server sends 24).
pub(super) const MAX_GIF_RESULTS: usize = 64;
/// Counted per kept item on top of its strings.
pub(super) const ITEM_OVERHEAD: usize = 128;
/// Bound on accounted retained payload, including encoded queue payload.
#[cfg(test)]
pub(super) const MAX_RETAINED_BYTES: usize = 80 * 1024 * 1024;

/// The sum the caps above allow (checked against `MAX_RETAINED_BYTES` in tests).
#[cfg(test)]
pub(super) const fn worst_case_bytes() -> usize {
    let label = MAX_LABEL_CHARS * 4 + ITEM_OVERHEAD;
    let users = MAX_USERS * (label + 2);
    let channels = MAX_CHANNELS * label + MAX_USERS * 8;
    let settings =
        MAX_ICON_BYTES + 4 * (MAX_LABEL_CHARS + 2 * MAX_WELCOME_LEN + 4 * MAX_SERVER_MESSAGE_LEN);
    users
        + channels
        + MAX_TEXT_BYTES
        + MAX_IMAGE_BYTES
        + MAX_ADMIN_BYTES
        + settings
        + crate::net::EVENT_QUEUE_BYTES
}

type Check = Result<(), String>;

fn fail(what: &str) -> Check {
    Err(format!("the server sent invalid data ({what})"))
}

fn label(name: &str, what: &str) -> Check {
    if name.len() > MAX_LABEL_CHARS * 4 || name.chars().count() > MAX_LABEL_CHARS {
        return fail(what);
    }
    Ok(())
}

fn text(s: &str, what: &str) -> Check {
    if s.len() > MAX_TEXT_LEN {
        return fail(what);
    }
    Ok(())
}

fn chars(s: &str, max: usize, what: &str) -> Check {
    if s.len() > max * 4 || s.chars().count() > max {
        return fail(what);
    }
    Ok(())
}

fn image(img: &Option<Vec<u8>>, max: usize) -> Check {
    if img.as_ref().is_some_and(|i| i.len() > max) {
        return fail("image too large");
    }
    Ok(())
}

pub(super) fn settings(s: &ServerSettings) -> Check {
    label(&s.name, "server name")?;
    image(&s.icon, MAX_ICON_BYTES)?;
    chars(&s.welcome_message, MAX_WELCOME_LEN, "welcome message")?;
    chars(&s.rules, MAX_WELCOME_LEN, "rules")?;
    for t in [&s.join_message, &s.leave_message, &s.afk_notice, &s.motd] {
        chars(t, MAX_SERVER_MESSAGE_LEN, "server message")?;
    }
    Ok(())
}

fn user(u: &UserInfo) -> Check {
    label(&u.name, "user name")?;
    if u.streams.len() > 2 {
        return fail("streams");
    }
    Ok(())
}

fn channel(c: &ChannelInfo) -> Check {
    label(&c.name, "channel name")?;
    if c.user_ids.len() > MAX_USERS {
        return fail("channel members");
    }
    Ok(())
}

fn record(r: &ChatRecord) -> Check {
    label(&r.from_name, "sender name")?;
    chars(&r.text, MAX_CHAT_LEN, "chat message")?;
    image(&r.image, MAX_MEDIA_BYTES)
}

fn log(e: &LogEntry) -> Check {
    text(&e.text, "activity line")
}

/// Bytes the admin lists take while kept.
pub(super) fn admin_bytes(
    roles: &[RoleEntry],
    bans: &[BanInfo],
    invites: &[InviteInfo],
    audit: &[AuditEntry],
) -> usize {
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    roles
        .iter()
        .map(|r| r.uid.len() + r.name.len())
        .sum::<usize>()
        + bans
            .iter()
            .map(|b| b.name.len() + opt(&b.uid) + opt(&b.ip) + b.reason.len() + b.by.len())
            .sum::<usize>()
        + invites
            .iter()
            .map(|i| i.code.len() + i.by.len())
            .sum::<usize>()
        + audit
            .iter()
            .map(|a| a.actor.len() + a.action.len())
            .sum::<usize>()
        + (roles.len() + bans.len() + invites.len() + audit.len()) * ITEM_OVERHEAD
}

/// The initial state of a login.
pub(super) fn welcome(
    s: &ServerSettings,
    users: &[UserInfo],
    channels: &[ChannelInfo],
    spawn_log: &[LogEntry],
    history: &[ChatRecord],
) -> Check {
    settings(s)?;
    if users.len() > MAX_USERS {
        return fail("too many users");
    }
    if channels.len() > MAX_CHANNELS {
        return fail("too many channels");
    }
    if channels.iter().map(|c| c.user_ids.len()).sum::<usize>() > MAX_USERS {
        return fail("channel members");
    }
    if spawn_log.len() > SPAWN_LOG_CAP || history.len() > MAX_HISTORY {
        return fail("history too long");
    }
    users.iter().try_for_each(user)?;
    channels.iter().try_for_each(channel)?;
    spawn_log.iter().try_for_each(log)?;
    history.iter().try_for_each(record)
}

/// One message against the state it would change.
pub(super) fn message(s: &ServerSession, msg: &ServerMessage) -> Check {
    match msg {
        ServerMessage::UserJoined { user: u } => {
            user(u)?;
            if !s.users.contains_key(&u.id) && s.users.len() >= MAX_USERS {
                return fail("too many users");
            }
            Ok(())
        }
        ServerMessage::ChannelCreated { channel: c } => {
            channel(c)?;
            let replaced = s.channels.get(&c.id);
            if replaced.is_none() && s.channels.len() >= MAX_CHANNELS {
                return fail("too many channels");
            }
            let members = s.member_count() - replaced.map_or(0, |r| r.user_ids.len());
            if members + c.user_ids.len() > MAX_USERS {
                return fail("channel members");
            }
            Ok(())
        }
        ServerMessage::ChannelRenamed { name, .. } => label(name, "channel name"),
        ServerMessage::SpawnLogEntry { entry } => log(entry),
        ServerMessage::Chat {
            from_name,
            text: t,
            image: img,
            ..
        } => {
            label(from_name, "sender name")?;
            chars(t, MAX_CHAT_LEN, "chat message")?;
            image(img, MAX_MEDIA_BYTES)
        }
        ServerMessage::ChatEdited { text: t, .. } => chars(t, MAX_CHAT_LEN, "chat message"),
        ServerMessage::GifResults {
            query,
            results,
            error,
            ..
        } => {
            text(query, "GIF query")?;
            text(error.as_deref().unwrap_or_default(), "GIF error")?;
            if results.len() > MAX_GIF_RESULTS {
                return fail("too many GIFs");
            }
            for g in results {
                for f in [&g.id, &g.title, &g.preview_url, &g.url] {
                    text(f, "GIF")?;
                }
            }
            Ok(())
        }
        ServerMessage::Removed { reason, .. } => text(reason, "reason"),
        ServerMessage::AdminData {
            roles,
            bans,
            invites,
            audit,
        } => {
            if admin_bytes(roles, bans, invites, audit) > MAX_ADMIN_BYTES {
                return fail("admin lists too large");
            }
            Ok(())
        }
        ServerMessage::InviteCreated { invite } => {
            text(&invite.code, "invite")?;
            text(&invite.by, "invite")
        }
        ServerMessage::StreamsChanged { streams, .. } if streams.len() > 2 => fail("streams"),
        ServerMessage::VideoFrame { data, .. } if data.len() > MAX_MEDIA_BYTES => {
            fail("video frame too large")
        }
        ServerMessage::ServerSettingsChanged { settings: new } => settings(new),
        ServerMessage::Poked {
            from_name,
            message: m,
            ..
        } => {
            label(from_name, "poke")?;
            chars(m, MAX_POKE_LEN, "poke")
        }
        ServerMessage::Notice { message: m } | ServerMessage::Error { message: m } => {
            text(m, "notice")
        }
        _ => Ok(()),
    }
}
