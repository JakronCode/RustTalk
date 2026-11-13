//! One connection to one server. Every joined community has its own
//! `ServerSession`: transport, event queue, mirrored server state, unread
//! markers, errors and retry state. The app keeps the viewed one in
//! `RustTalkApp::srv` and the rest in `RustTalkApp::others`; a session is
//! identified by its `SessionId`, never by where it is kept, so switching the
//! view moves no voice or media ownership.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusttalk_shared::protocol::{ChannelInfo, ChannelKind, Role, ServerSettings, UserInfo};
use rusttalk_shared::secure::ReplayWindow;

use super::{join_error, moderation_ui, ChatLine};
use crate::audio::AudioSession;
use crate::net::{Command, NetEvent};

pub(super) type SessionId = u64;

/// Servers connected, connecting or waiting to retry at the same time.
pub(super) const MAX_ACTIVE_SESSIONS: usize = 8;
/// Automatic connection attempts (startup, retries) running at the same time.
pub(super) const MAX_AUTO_CONNECTING: usize = 2;
/// Network events waiting for the UI per session. Further video frames are
/// dropped; other messages wait (TCP backpressure) instead of piling up.
pub(super) const EVENT_QUEUE: usize = 256;
/// Events handled per session and frame, so one busy server cannot starve the others.
pub(super) const EVENTS_PER_FRAME: usize = 512;
/// Chat lines kept per channel (the oldest are dropped).
pub(super) const MAX_LINES_PER_CHANNEL: usize = 2_000;
/// Image bytes kept in one session's chat; the oldest images are dropped first.
pub(super) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Give up after this many failed attempts (~10 minutes with backoff).
pub(super) const MAX_RECONNECT_ATTEMPTS: u32 = 25;

/// Wait time before reconnect attempt `attempt` (1-based): 1, 2, 4, 8, 15, 30, 30, ... s.
pub(super) fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 15,
        _ => 30,
    })
}

/// Spreads retries of several servers that failed at the same moment.
pub(super) fn retry_stagger(id: SessionId) -> Duration {
    Duration::from_millis((id % 5) * 300)
}

/// Automatic (re)connect: after an unexpected loss, or for a server enrolled at startup.
pub(super) struct Reconnect {
    /// Number of the next attempt (1-based).
    pub attempt: u32,
    /// When the next attempt may start (`None` = attempt is running).
    pub next_at: Option<Instant>,
    /// Voice channel we were in (id, name as fallback), rejoined afterwards.
    pub channel: Option<(u64, String)>,
}

/// Connection state of one server, as the server switcher shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerStatus {
    Disconnected,
    /// An automatic attempt is due but waits for a free connection slot.
    Waiting,
    Connecting,
    Connected,
    /// Connection lost or not reachable yet; the next attempt is scheduled.
    Retrying,
    /// Not connected: the limit of servers open at once is reached.
    LimitReached,
    /// Stopped with an error that needs the user (password, ban, certificate, ...).
    Failed,
}

/// One row of the server switcher (saved servers first, then unsaved live ones).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ServerRow {
    /// Normalized address: pass it to `select_server`, `connect_server`, `disconnect_server`.
    pub key: String,
    /// Server name while connected, else the nickname or address.
    pub name: String,
    pub address: String,
    pub saved: bool,
    pub status: ServerStatus,
    /// Some channel has unread messages.
    pub unread: bool,
    /// We are in a voice channel on this server (only one server at a time).
    pub voice: bool,
    pub viewed: bool,
    /// Reconnected when RustTalk starts (the user connected and did not disconnect).
    pub auto_connect: bool,
    /// Short reason for `Retrying`, `LimitReached` and `Failed`.
    pub error: Option<String>,
}

pub(super) struct ServerSession {
    pub id: SessionId,
    /// `net::normalize_addr` of the address; empty for the placeholder shown before any server.
    pub key: String,
    /// Address as the user saved or typed it.
    pub address: String,
    pub password: Option<String>,
    /// Invite code for the next login (used once).
    pub invite: Option<String>,
    /// Bumped for every connection attempt; binds async results to one attempt.
    pub generation: u64,
    pub cmd_tx: Option<tokio::sync::mpsc::UnboundedSender<Command>>,
    /// Replaced on every attempt and disconnect: late events of an old
    /// attempt have no receiver any more.
    pub event_tx: SyncSender<NetEvent>,
    pub event_rx: Receiver<NetEvent>,
    /// Video bytes in this connection's send queue (see `net::connect`).
    pub upload_backlog: Arc<AtomicUsize>,
    /// Login finished (Welcome received). `cmd_tx` alone = connecting.
    pub connected: bool,
    /// The user chose "Connect and remember": a successful login enrolls the
    /// server for startup. Only that choice sets it; plain connects never do.
    pub remember: bool,
    pub reconnect: Option<Reconnect>,
    /// Not started: the active session limit was reached.
    pub overflow: bool,
    /// Short reason for the last failure (server switcher).
    pub last_error: Option<String>,
    /// We were kicked/banned: no automatic reconnect, show this reason.
    pub removed: Option<String>,
    /// TLS fingerprint of the connected server.
    pub server_fingerprint: String,
    /// Certificate waiting for the user's trust decision (new or changed).
    pub cert_check: Option<join_error::CertCheck>,
    /// Last failed join, shown as a panel on the connect screen.
    pub join_error: Option<join_error::JoinError>,
    // Mirrored server state.
    pub my_id: u64,
    pub users: HashMap<u64, UserInfo>,
    pub channels: HashMap<u64, ChannelInfo>,
    pub chat: HashMap<u64, Vec<ChatLine>>,
    /// Image bytes held in `chat` (bounded by `MAX_IMAGE_BYTES`).
    pub image_bytes: usize,
    pub unread: HashSet<u64>,
    pub selected_channel: Option<u64>,
    pub collapsed: HashMap<u64, bool>,
    pub server_settings: ServerSettings,
    /// No admin on the server yet: everyone may do everything.
    pub open_permissions: bool,
    /// Roles, bans, invites, audit log (Server Settings, admins).
    pub admin: moderation_ui::AdminData,
    /// The server offers GIF search (it has a GIPHY key).
    pub gif_available: bool,
    /// UDP socket, token and key of this connection (voice runs on one session only).
    pub audio_session: Option<AudioSession>,
    /// Replay window of this connection while another one owns the audio link.
    pub replay: Option<ReplayWindow>,
    /// Unsent composer text while another server is viewed.
    pub draft: String,
}

fn event_channel() -> (SyncSender<NetEvent>, Receiver<NetEvent>) {
    std::sync::mpsc::sync_channel(EVENT_QUEUE)
}

impl ServerSession {
    pub fn new(id: SessionId, address: &str) -> Self {
        let (event_tx, event_rx) = event_channel();
        let address = address.trim().to_string();
        Self {
            id,
            key: if address.is_empty() {
                String::new()
            } else {
                crate::net::normalize_addr(&address)
            },
            address,
            password: None,
            invite: None,
            generation: 0,
            cmd_tx: None,
            event_tx,
            event_rx,
            upload_backlog: Arc::new(AtomicUsize::new(0)),
            connected: false,
            remember: false,
            reconnect: None,
            overflow: false,
            last_error: None,
            removed: None,
            server_fingerprint: String::new(),
            cert_check: None,
            join_error: None,
            my_id: 0,
            users: HashMap::new(),
            channels: HashMap::new(),
            chat: HashMap::new(),
            image_bytes: 0,
            unread: HashSet::new(),
            selected_channel: None,
            collapsed: HashMap::new(),
            server_settings: ServerSettings::default(),
            open_permissions: true,
            admin: Default::default(),
            gif_available: false,
            audio_session: None,
            replay: None,
            draft: String::new(),
        }
    }

    /// Shown before any server was picked.
    pub fn is_placeholder(&self) -> bool {
        self.key.is_empty()
    }

    /// Uses a slot of `MAX_ACTIVE_SESSIONS`.
    pub fn is_active(&self) -> bool {
        self.cmd_tx.is_some() || self.reconnect.is_some()
    }

    pub fn is_connecting(&self) -> bool {
        self.cmd_tx.is_some() && !self.connected
    }

    /// Holds nothing worth keeping once it is not viewed.
    pub fn is_disposable(&self) -> bool {
        self.is_placeholder()
            || (!self.is_active()
                && !self.overflow
                && self.join_error.is_none()
                && self.cert_check.is_none()
                && self.draft.is_empty())
    }

    pub fn status(&self) -> ServerStatus {
        if self.connected {
            ServerStatus::Connected
        } else if self.cmd_tx.is_some() {
            ServerStatus::Connecting
        } else if let Some(rc) = &self.reconnect {
            match rc.next_at {
                Some(t) if t > Instant::now() => ServerStatus::Retrying,
                _ => ServerStatus::Waiting,
            }
        } else if self.overflow {
            ServerStatus::LimitReached
        } else if self.join_error.is_some() || self.cert_check.is_some() {
            ServerStatus::Failed
        } else {
            ServerStatus::Disconnected
        }
    }

    /// New event channel: queued and late events of the old attempt are dropped.
    pub fn reset_events(&mut self) {
        (self.event_tx, self.event_rx) = event_channel();
    }

    /// Forgets everything the server told us (after the connection ended).
    pub fn clear_server_state(&mut self) {
        self.connected = false;
        self.users.clear();
        self.channels.clear();
        self.chat.clear();
        self.image_bytes = 0;
        self.unread.clear();
        self.selected_channel = None;
        self.admin = Default::default();
        self.open_permissions = true;
        self.gif_available = false;
        self.audio_session = None;
        self.replay = None;
    }

    pub fn my_channel(&self) -> Option<u64> {
        self.users.get(&self.my_id).map(|u| u.channel_id)
    }

    /// Our voice channel (None in the lobby or when not in a voice channel).
    pub fn my_voice_channel(&self) -> Option<u64> {
        let ch = self.my_channel()?;
        (self.channel_kind(ch) == Some(ChannelKind::Voice)).then_some(ch)
    }

    pub fn spawn_channel_id(&self) -> Option<u64> {
        self.channels
            .values()
            .find(|c| c.kind == ChannelKind::Spawn)
            .map(|c| c.id)
    }

    pub fn channel_name(&self, id: u64) -> String {
        self.channels
            .get(&id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "?".into())
    }

    pub fn channel_kind(&self, id: u64) -> Option<ChannelKind> {
        self.channels.get(&id).map(|c| c.kind)
    }

    pub fn my_role(&self) -> Role {
        self.users
            .get(&self.my_id)
            .map(|u| u.role)
            .unwrap_or_default()
    }

    /// May I do something that needs `need`? (Open server: yes.)
    pub fn can(&self, need: Role) -> bool {
        self.open_permissions || self.my_role() >= need
    }

    /// Server name while known, else the saved nickname or the address.
    pub fn display_name(&self, nickname: Option<&str>) -> String {
        let name = self.server_settings.name.trim();
        if self.connected && !name.is_empty() {
            name.to_string()
        } else {
            nickname.unwrap_or(&self.address).to_string()
        }
    }

    /// Adds a chat line within the retention limits. Lines for unknown
    /// channels are dropped: a hostile server cannot open unbounded buckets.
    pub fn push_line(&mut self, channel_id: u64, line: ChatLine) -> bool {
        if !self.channels.contains_key(&channel_id) {
            return false;
        }
        let lines = self.chat.entry(channel_id).or_default();
        self.image_bytes += line.image.as_ref().map_or(0, Vec::len);
        lines.push(line);
        if lines.len() > MAX_LINES_PER_CHANNEL {
            let old = lines.remove(0);
            self.image_bytes -= old.image.as_ref().map_or(0, Vec::len);
        }
        while self.image_bytes > MAX_IMAGE_BYTES && self.drop_oldest_image() {}
        true
    }

    /// System line in a channel's activity list; marks it unread unless open.
    pub fn system_line(&mut self, channel_id: u64, text: String, timestamp: u64, open: bool) {
        if self.push_line(channel_id, ChatLine::system(text, timestamp)) && !open {
            self.unread.insert(channel_id);
        }
    }

    pub fn remove_line(&mut self, channel_id: u64, message_id: u64) {
        if let Some(lines) = self.chat.get_mut(&channel_id) {
            for l in lines.iter().filter(|l| l.id == message_id) {
                self.image_bytes -= l.image.as_ref().map_or(0, Vec::len);
            }
            lines.retain(|l| l.id != message_id);
        }
    }

    pub fn remove_channel_chat(&mut self, channel_id: u64) {
        if let Some(lines) = self.chat.remove(&channel_id) {
            self.image_bytes -= lines
                .iter()
                .map(|l| l.image.as_ref().map_or(0, Vec::len))
                .sum::<usize>();
        }
    }

    fn drop_oldest_image(&mut self) -> bool {
        let oldest = self
            .chat
            .values_mut()
            .flat_map(|v| v.iter_mut())
            .filter(|l| l.image.is_some())
            .min_by_key(|l| (l.timestamp, l.id));
        match oldest.and_then(|l| l.image.take()) {
            Some(img) => {
                self.image_bytes -= img.len();
                true
            }
            None => false,
        }
    }
}
