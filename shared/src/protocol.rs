//! TCP-Steuerprotokoll zwischen Client und Server.
//!
//! Serialisierung via `bincode`, Framing via [`crate::framing`].
//!
//! Channel-Typen (v2):
//! - **Spawn:** genau einer pro Server. Hier landen alle Clients beim Verbinden.
//!   Sprechen ist technisch blockiert. Enthaelt das serverseitige Join-Log.
//! - **Category:** reine Gruppierung. Kein Join, kein Chat, kein Audio.
//! - **Voice:** Sprechen + Media-Chat (Text/Bilder), wie Discords Sprach-Chat.
//! - **Text:** nur Text-/Bild-Chat (fuer alle Clients sichtbar, kein Join noetig).
//!
//! v4: Bilder im Chat, Video-Frames (JPEG), Sharing-Status.
//! v5: Video-Frames mit Art (Kamera/Bildschirm) -> beide gleichzeitig teilbar;
//!     `StopVideo`/`VideoStopped` beendet einen einzelnen Stream.
//! v6: Channel-Reihenfolge (`position`, `MoveChannel`/`ChannelsReordered`),
//!     Servername im `Welcome`.
//! v7: Servereinstellungen (`ServerSettings`: Name, Icon, Willkommenstext,
//!     AFK-Mover) statt nur Servername; `Notice` fuer Hinweise.
//! v8: Server messages in `ServerSettings`: join/leave messages (posted in an
//!     announcement text channel), rules (lobby), AFK notice template, MOTD.
//! v9: Chat message ids: replies (`reply_to`), `EditChat`/`DeleteChat` (own
//!     messages only, by name) -> `ChatEdited`/`ChatDeleted`.
//! v10: GIF search through the server (`GifSearch` -> `GifResults`); the
//!     GIPHY key is set per server (`SetGifKey`) and never sent to clients.
//! v11: Identities (secret per client, server keeps a hash), roles
//!     (Member/Moderator/Admin), kick/ban/move users, invites, audit log.
//!     Without any admin the server is "open" (everyone may do everything).
//! v12: Encryption: TCP over TLS (pinned self-signed certificate), audio
//!     packets ChaCha20-Poly1305 with a per-session key (`Welcome.udp_key`).
//!     Video is H.264 (`VideoFrame.data` = frame number + Annex-B); viewers
//!     ask the sender for a keyframe (`RequestKeyframe` -> `KeyframeRequest`).
//! v13: Poke: `Poke { target, message }` -> the target gets `Poked` (popup +
//!     wake-up sound, like TeamSpeak).

use serde::{Deserialize, Serialize};

/// Aktuelle Protokollversion. Client und Server müssen übereinstimmen.
pub const PROTOCOL_VERSION: u16 = 13;

/// Max. length of a poke message.
pub const MAX_POKE_LEN: usize = 100;

/// Art eines Video-Streams. Ein User kann beide gleichzeitig senden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VideoKind {
    Camera,
    Screen,
}

/// Maximale Groesse eines Bildes/Video-Frames in Bytes (4 MiB).
pub const MAX_MEDIA_BYTES: usize = 4 * 1024 * 1024;

/// Max. Groesse des Server-Icons (Client verkleinert vorher auf 128x128).
pub const MAX_ICON_BYTES: usize = 256 * 1024;
/// Max. Laenge des Willkommenstexts.
pub const MAX_WELCOME_LEN: usize = 1000;

/// Max. length of a short server message (join/leave/AFK notice/MOTD).
pub const MAX_SERVER_MESSAGE_LEN: usize = 300;

/// Default join message (`{user}` = username).
pub const DEFAULT_JOIN_MESSAGE: &str = "{user} joined the server";
/// Default leave message (`{user}` = username).
pub const DEFAULT_LEAVE_MESSAGE: &str = "{user} left the server";
/// Default AFK notice (`{channel}` = target channel, `{minutes}` = timeout).
pub const DEFAULT_AFK_NOTICE: &str = "You were moved to {channel} after {minutes} min muted (AFK)";

/// Replaces `{key}` placeholders in a server message template.
pub fn fill_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

/// Serverweite Einstellungen (im Client unter "Server Settings" editierbar).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    pub name: String,
    /// Server-Icon (PNG), None = aus dem Namen generiert.
    pub icon: Option<Vec<u8>>,
    /// Wird in der Lobby angezeigt. Leer = keiner.
    pub welcome_message: String,
    /// Auto-AFK: wer so lange stumm ist, wird verschoben.
    pub afk_enabled: bool,
    pub afk_timeout_mins: u32,
    /// Ziel-Channel (Voice oder Lobby); None = Lobby.
    pub afk_channel: Option<u64>,
    /// Posted in `announce_channel` when someone connects (empty = off).
    pub join_message: String,
    /// Posted in `announce_channel` when someone disconnects (empty = off).
    pub leave_message: String,
    /// Text channel for join/leave messages; None = no announcements.
    pub announce_channel: Option<u64>,
    /// Server rules, shown in the lobby (empty = none).
    pub rules: String,
    /// Sent to users moved by the AFK mover (empty = default text).
    pub afk_notice: String,
    /// Message of the day, shown to everyone on connect (empty = none).
    pub motd: String,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            name: "RustTalk Server".to_string(),
            icon: None,
            welcome_message: String::new(),
            afk_enabled: true,
            afk_timeout_mins: 10,
            afk_channel: None,
            join_message: DEFAULT_JOIN_MESSAGE.to_string(),
            leave_message: DEFAULT_LEAVE_MESSAGE.to_string(),
            announce_channel: None,
            rules: String::new(),
            afk_notice: DEFAULT_AFK_NOTICE.to_string(),
            motd: String::new(),
        }
    }
}

/// Standard-Port für TCP (Steuerung) und UDP (Audio).
pub const DEFAULT_PORT: u16 = 7331;

/// Maximale Länge einer Chat-Nachricht in Zeichen.
pub const MAX_CHAT_LEN: usize = 2000;

/// Maximale Länge eines Channel-/Usernamens in Zeichen.
pub const MAX_NAME_LEN: usize = 64;

/// Maximale Anzahl gebufferter Spawn-Log-Eintraege auf dem Server.
pub const SPAWN_LOG_CAP: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    Spawn,
    Category,
    Text,
    Voice,
}

impl ChannelKind {
    /// In diese Channel-Art darf man joinen (Voice-Teilnahme).
    pub fn can_join(&self) -> bool {
        matches!(self, ChannelKind::Spawn | ChannelKind::Voice)
    }

    /// In dieser Channel-Art gibt es Chat (Text: rein; Voice: Media-Feed).
    pub fn has_chat(&self) -> bool {
        matches!(self, ChannelKind::Text | ChannelKind::Voice)
    }

    /// In dieser Channel-Art wird Audio gerelayed.
    pub fn has_voice(&self) -> bool {
        matches!(self, ChannelKind::Voice)
    }

    /// Channel-Art steht im Sidebar-Baum als aufklappbare Gruppe.
    pub fn is_group(&self) -> bool {
        matches!(self, ChannelKind::Category)
    }
}

/// Eintrag im Join-Log des Spawn-Channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Unix-Timestamp (Sekunden).
    pub timestamp: u64,
    pub text: String,
}

/// Eine gespeicherte Chat-Nachricht (Persistenz/Verlauf).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRecord {
    /// Message id (unique per server).
    pub id: u64,
    pub channel_id: u64,
    pub from_name: String,
    pub text: String,
    /// Unix-Timestamp (Sekunden).
    pub timestamp: u64,
    /// Optionales Bild (PNG/JPEG/GIF-Bytes).
    pub image: Option<Vec<u8>>,
    /// Id of the message this one answers.
    pub reply_to: Option<u64>,
    /// Text was changed after sending.
    pub edited: bool,
}

/// One GIF from the server's GIF search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifInfo {
    pub id: String,
    pub title: String,
    /// Small animated preview for the picker grid.
    pub preview_url: String,
    pub preview_width: u32,
    pub preview_height: u32,
    /// The GIF that is sent (below ~2 MB).
    pub url: String,
}

/// Max. length of a GIF search query.
pub const MAX_GIF_QUERY_LEN: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: u64,
    pub name: String,
    pub kind: ChannelKind,
    /// Zugehoerige Kategorie (None = Top-Level).
    pub parent: Option<u64>,
    /// Sortierposition unter Geschwistern (gleicher `parent`), aufsteigend.
    pub position: u32,
    /// Belegte User (nur bei Voice/Spawn sinnvoll befuellt).
    pub user_ids: Vec<u64>,
}

/// Ort eines Channels im Baum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPlacement {
    pub id: u64,
    pub parent: Option<u64>,
    pub position: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: u64,
    pub name: String,
    pub channel_id: u64,
    pub muted: bool,
    pub deafened: bool,
    /// Teilt gerade Kamera/Bildschirm.
    pub sharing: bool,
    pub role: Role,
}

/// Role of an identity on a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub enum Role {
    /// Chat, talk, join channels.
    #[default]
    Member,
    /// + move and kick users, delete any message.
    Moderator,
    /// + channels, server settings, roles, bans, invites, audit log.
    Admin,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Member => "Member",
            Role::Moderator => "Moderator",
            Role::Admin => "Admin",
        }
    }
}

/// Length of the client identity secret (hex characters, 256 bit).
pub const IDENTITY_HEX_LEN: usize = 64;

/// Stored role of an identity (also for people who are offline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleEntry {
    /// Public id = hash of the identity secret.
    pub uid: String,
    /// Last known name.
    pub name: String,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanInfo {
    pub id: u64,
    pub name: String,
    pub uid: Option<String>,
    pub ip: Option<String>,
    pub reason: String,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds; None = permanent.
    pub until: Option<u64>,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteInfo {
    pub code: String,
    pub created: u64,
    /// Unix seconds; None = never expires.
    pub expires: Option<u64>,
    /// None = unlimited.
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub actor: String,
    pub action: String,
}

/// Max. length of a kick/ban reason.
pub const MAX_REASON_LEN: usize = 200;

/// Client -> Server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Erste Nachricht nach dem TCP-Verbindungsaufbau.
    Hello {
        username: String,
        protocol_version: u16,
        password: Option<String>,
        /// Secret identity (hex, `IDENTITY_HEX_LEN`). The server stores only a hash.
        identity: String,
        /// Invite code (replaces the server password).
        invite: Option<String>,
    },
    JoinChannel {
        channel_id: u64,
    },
    CreateChannel {
        name: String,
        kind: ChannelKind,
        /// Gewuenschte Kategorie (None = Top-Level).
        parent: Option<u64>,
    },
    DeleteChannel {
        channel_id: u64,
    },
    /// Channel verschieben (Drag & Drop): neue Kategorie (None = oberste
    /// Ebene) und Index unter den neuen Geschwistern.
    MoveChannel {
        channel_id: u64,
        parent: Option<u64>,
        index: u32,
    },
    /// Servereinstellungen aendern (kein Rollen-System: jeder darf, LAN).
    UpdateServerSettings {
        settings: ServerSettings,
    },
    RenameChannel {
        channel_id: u64,
        name: String,
    },
    /// Chat (Text- und Voice-Channels); optionales Bild. Wird an ALLE Clients broadcastet.
    Chat {
        channel_id: u64,
        text: String,
        image: Option<Vec<u8>>,
        /// Answer to this message (same channel).
        reply_to: Option<u64>,
    },
    /// Change the text of an own message.
    EditChat {
        message_id: u64,
        text: String,
    },
    /// Delete an own message.
    DeleteChat {
        message_id: u64,
    },
    /// GIF search; empty query = trending. `offset` for the next page.
    GifSearch {
        query: String,
        offset: u32,
    },
    /// Set (or remove) the server's GIPHY API key. The server checks it first.
    SetGifKey {
        key: Option<String>,
    },
    /// Become admin: works without password while the server has no admin,
    /// otherwise with `admin_password` from server.toml.
    ClaimAdmin {
        password: Option<String>,
    },
    /// Admin: set the role of a connected user.
    SetRole {
        client_id: u64,
        role: Role,
    },
    /// Admin: set the role of a stored identity (also offline).
    SetRoleByUid {
        uid: String,
        role: Role,
    },
    /// Moderator: move a user into a channel.
    MoveUser {
        client_id: u64,
        channel_id: u64,
    },
    /// Moderator: disconnect a user.
    Kick {
        client_id: u64,
        reason: String,
    },
    /// Admin: ban identity + IP of a user (minutes: None = permanent).
    Ban {
        client_id: u64,
        reason: String,
        minutes: Option<u32>,
    },
    Unban {
        ban_id: u64,
    },
    /// Admin: new invite (hours: None = no expiry).
    CreateInvite {
        max_uses: Option<u32>,
        hours: Option<u32>,
    },
    DeleteInvite {
        code: String,
    },
    /// Admin: roles, bans, invites and audit log (-> `AdminData`).
    RequestAdminData,
    SetMuteState {
        muted: bool,
        deafened: bool,
    },
    /// Kamera/Screen-Teilen an-/ausschalten.
    SetSharing {
        sharing: bool,
    },
    /// Ein Video-Frame (JPEG) fuer die Mitglieder des eigenen Voice-Channels.
    VideoFrame {
        kind: VideoKind,
        data: Vec<u8>,
    },
    /// Einen einzelnen Stream beenden (der andere darf weiterlaufen).
    StopVideo {
        kind: VideoKind,
    },
    /// Poke another user: they get a popup with `message` (may be empty) and
    /// a wake-up sound.
    Poke {
        target: u64,
        message: String,
    },
    /// Viewer: please send a full picture of this stream (joined late / frames lost).
    RequestKeyframe {
        from_id: u64,
        kind: VideoKind,
    },
    /// Sauberes Trennen (Server antwortet nicht, schließt nur).
    Disconnect,
}

/// Server -> Client
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Antwort auf `Hello`. Enthält den kompletten Anfangszustand.
    Welcome {
        /// Servereinstellungen (Name, Icon, Willkommenstext, AFK).
        settings: ServerSettings,
        client_id: u64,
        /// Token, mit dem sich der Client per UDP beim Server registriert.
        udp_token: u64,
        channels: Vec<ChannelInfo>,
        users: Vec<UserInfo>,
        /// Bisheriges Join-Log des Spawn-Channels (aelteste zuerst).
        spawn_log: Vec<LogEntry>,
        /// Letzte Nachrichten der Text-Channels (Persistenz, aelteste zuerst).
        chat_history: Vec<ChatRecord>,
        /// The server has a GIPHY key (GIF button works).
        gif_search: bool,
        /// No admin yet: everyone may do everything.
        open_permissions: bool,
        /// Key for the audio packets of this session (`secure::UDP_KEY_LEN`).
        udp_key: Vec<u8>,
    },
    UserJoined {
        user: UserInfo,
    },
    UserLeft {
        client_id: u64,
    },
    UserMoved {
        client_id: u64,
        channel_id: u64,
    },
    ChannelCreated {
        channel: ChannelInfo,
    },
    ChannelDeleted {
        channel_id: u64,
    },
    /// Neue Anordnung aller Channels nach einem `MoveChannel`.
    ChannelsReordered {
        placements: Vec<ChannelPlacement>,
    },
    ChannelRenamed {
        channel_id: u64,
        name: String,
    },
    /// Neuer Eintrag im Spawn-Log (bei jedem Join).
    SpawnLogEntry {
        entry: LogEntry,
    },
    Chat {
        /// Message id (for replies, edits, deletes).
        id: u64,
        channel_id: u64,
        from_id: u64,
        from_name: String,
        text: String,
        /// Unix-Timestamp (Sekunden).
        timestamp: u64,
        image: Option<Vec<u8>>,
        reply_to: Option<u64>,
    },
    ChatEdited {
        channel_id: u64,
        message_id: u64,
        text: String,
    },
    ChatDeleted {
        channel_id: u64,
        message_id: u64,
    },
    /// Answer to `GifSearch` (only to the asking client).
    GifResults {
        query: String,
        offset: u32,
        results: Vec<GifInfo>,
        error: Option<String>,
    },
    /// GIF search was switched on/off (key set or removed).
    GifSearchChanged {
        available: bool,
    },
    UserRoleChanged {
        client_id: u64,
        role: Role,
    },
    /// First admin exists (false) / last admin gone (true).
    PermissionsChanged {
        open: bool,
    },
    /// You were kicked or banned; the connection closes right after.
    Removed {
        reason: String,
        banned: bool,
    },
    AdminData {
        roles: Vec<RoleEntry>,
        bans: Vec<BanInfo>,
        invites: Vec<InviteInfo>,
        audit: Vec<AuditEntry>,
    },
    /// Answer to `CreateInvite` (only to the creator).
    InviteCreated {
        invite: InviteInfo,
    },
    MuteStateChanged {
        client_id: u64,
        muted: bool,
        deafened: bool,
    },
    SharingChanged {
        client_id: u64,
        sharing: bool,
    },
    /// Video-Frame (JPEG) eines Mitglieds des eigenen Voice-Channels.
    VideoFrame {
        from_id: u64,
        kind: VideoKind,
        data: Vec<u8>,
    },
    /// Ein Mitglied hat einen einzelnen Stream beendet.
    VideoStopped {
        from_id: u64,
        kind: VideoKind,
    },
    /// To the sender: a viewer needs a keyframe of this stream.
    KeyframeRequest {
        kind: VideoKind,
    },
    /// Neue Servereinstellungen (an alle).
    ServerSettingsChanged {
        settings: ServerSettings,
    },
    /// You were poked (only to the target).
    Poked {
        from_id: u64,
        from_name: String,
        message: String,
    },
    /// Hinweis an einen Client (z. B. "wegen Inaktivitaet verschoben").
    Notice {
        message: String,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_kind_capabilities() {
        assert!(ChannelKind::Spawn.can_join());
        assert!(!ChannelKind::Spawn.has_chat());
        assert!(!ChannelKind::Spawn.has_voice());

        assert!(!ChannelKind::Category.can_join());
        assert!(!ChannelKind::Category.has_chat());
        assert!(!ChannelKind::Category.has_voice());
        assert!(ChannelKind::Category.is_group());

        assert!(!ChannelKind::Text.can_join());
        assert!(ChannelKind::Text.has_chat());
        assert!(!ChannelKind::Text.has_voice());

        assert!(ChannelKind::Voice.can_join());
        assert!(ChannelKind::Voice.has_chat(), "Voice hat Media-Chat (v4)");
        assert!(ChannelKind::Voice.has_voice());
    }

    #[test]
    fn client_message_roundtrip() {
        let msgs = vec![
            ClientMessage::Hello {
                username: "jb".into(),
                protocol_version: PROTOCOL_VERSION,
                password: Some("geheim".into()),
                identity: "a".repeat(IDENTITY_HEX_LEN),
                invite: None,
            },
            ClientMessage::Hello {
                username: "ohne passwort".into(),
                protocol_version: PROTOCOL_VERSION,
                password: None,
                identity: String::new(),
                invite: Some("CODE".into()),
            },
            ClientMessage::JoinChannel { channel_id: 42 },
            ClientMessage::CreateChannel {
                name: "Gaming".into(),
                kind: ChannelKind::Voice,
                parent: Some(3),
            },
            ClientMessage::DeleteChannel { channel_id: 7 },
            ClientMessage::RenameChannel { channel_id: 7, name: "Neu".into() },
            ClientMessage::MoveChannel { channel_id: 7, parent: Some(2), index: 1 },
            ClientMessage::Chat { channel_id: 1, text: "Hallo Welt".into(), image: None, reply_to: None },
            ClientMessage::Chat { channel_id: 1, text: String::new(), image: Some(vec![0xFF, 0xD8]), reply_to: Some(4) },
            ClientMessage::EditChat { message_id: 4, text: "neu".into() },
            ClientMessage::DeleteChat { message_id: 4 },
            ClientMessage::GifSearch { query: "cat".into(), offset: 24 },
            ClientMessage::SetGifKey { key: Some("abc".into()) },
            ClientMessage::SetGifKey { key: None },
            ClientMessage::ClaimAdmin { password: Some("pw".into()) },
            ClientMessage::SetRole { client_id: 2, role: Role::Moderator },
            ClientMessage::SetRoleByUid { uid: "ab".into(), role: Role::Member },
            ClientMessage::MoveUser { client_id: 2, channel_id: 3 },
            ClientMessage::Kick { client_id: 2, reason: "afk".into() },
            ClientMessage::Ban { client_id: 2, reason: "spam".into(), minutes: Some(60) },
            ClientMessage::Unban { ban_id: 1 },
            ClientMessage::CreateInvite { max_uses: Some(5), hours: None },
            ClientMessage::DeleteInvite { code: "XYZ".into() },
            ClientMessage::RequestAdminData,
            ClientMessage::RequestKeyframe { from_id: 3, kind: VideoKind::Screen },
            ClientMessage::SetMuteState { muted: true, deafened: false },
            ClientMessage::SetSharing { sharing: true },
            ClientMessage::VideoFrame { kind: VideoKind::Camera, data: vec![1, 2, 3] },
            ClientMessage::StopVideo { kind: VideoKind::Screen },
            ClientMessage::UpdateServerSettings { settings: ServerSettings { icon: Some(vec![1, 2]), ..Default::default() } },
            ClientMessage::Poke { target: 3, message: "Hey".into() },
            ClientMessage::Disconnect,
        ];
        for msg in msgs {
            let bytes = bincode::serialize(&msg).expect("serialize");
            let back: ClientMessage = bincode::deserialize(&bytes).expect("deserialize");
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn server_message_roundtrip() {
        let user = UserInfo {
            role: Role::Moderator,
            id: 1,
            name: "alice".into(),
            channel_id: 1,
            muted: false,
            deafened: false,
            sharing: true,
        };
        let channel = ChannelInfo {
            id: 1,
            name: "Spawn".into(),
            kind: ChannelKind::Spawn,
            parent: None,
            position: 0,
            user_ids: vec![1],
        };
        let entry = LogEntry { timestamp: 1_763_000_000, text: "alice ist erschienen".into() };
        let record = ChatRecord {
            id: 12,
            channel_id: 3,
            from_name: "alice".into(),
            text: "moin".into(),
            timestamp: 1_763_000_001,
            image: Some(vec![0x89, 0x50]),
            reply_to: Some(11),
            edited: true,
        };
        let msgs = vec![
            ServerMessage::Welcome {
                settings: ServerSettings::default(),
                client_id: 1,
                udp_token: u64::MAX,
                channels: vec![channel.clone()],
                users: vec![user.clone()],
                spawn_log: vec![entry.clone()],
                chat_history: vec![record],
                gif_search: true,
                open_permissions: false,
                udp_key: vec![7; 32],
            },
            ServerMessage::UserJoined { user: user.clone() },
            ServerMessage::UserLeft { client_id: 1 },
            ServerMessage::UserMoved { client_id: 1, channel_id: 2 },
            ServerMessage::ChannelCreated { channel },
            ServerMessage::ChannelDeleted { channel_id: 2 },
            ServerMessage::ChannelRenamed { channel_id: 1, name: "Lobby".into() },
            ServerMessage::ChannelsReordered {
                placements: vec![ChannelPlacement { id: 3, parent: None, position: 2 }],
            },
            ServerMessage::SpawnLogEntry { entry },
            ServerMessage::Chat {
                id: 5,
                channel_id: 1,
                from_id: 1,
                from_name: "alice".into(),
                text: "moin".into(),
                timestamp: 1_763_000_000,
                image: None,
                reply_to: Some(4),
            },
            ServerMessage::ChatEdited { channel_id: 1, message_id: 5, text: "moin!".into() },
            ServerMessage::ChatDeleted { channel_id: 1, message_id: 5 },
            ServerMessage::GifResults {
                query: "cat".into(),
                offset: 0,
                results: vec![GifInfo {
                    id: "x".into(),
                    title: "Cat".into(),
                    preview_url: "https://p".into(),
                    preview_width: 200,
                    preview_height: 100,
                    url: "https://g".into(),
                }],
                error: None,
            },
            ServerMessage::GifSearchChanged { available: true },
            ServerMessage::KeyframeRequest { kind: VideoKind::Camera },
            ServerMessage::UserRoleChanged { client_id: 1, role: Role::Admin },
            ServerMessage::PermissionsChanged { open: false },
            ServerMessage::Removed { reason: "spam".into(), banned: true },
            ServerMessage::AdminData {
                roles: vec![RoleEntry { uid: "ab".into(), name: "anna".into(), role: Role::Admin }],
                bans: vec![BanInfo {
                    id: 1,
                    name: "eve".into(),
                    uid: Some("cd".into()),
                    ip: Some("10.0.0.9".into()),
                    reason: "spam".into(),
                    created: 5,
                    until: None,
                    by: "anna".into(),
                }],
                invites: vec![InviteInfo { code: "XYZ".into(), created: 1, expires: Some(9), max_uses: Some(3), uses: 1, by: "anna".into() }],
                audit: vec![AuditEntry { timestamp: 1, actor: "anna".into(), action: "kicked eve".into() }],
            },
            ServerMessage::InviteCreated {
                invite: InviteInfo { code: "XYZ".into(), created: 1, expires: None, max_uses: None, uses: 0, by: "anna".into() },
            },
            ServerMessage::MuteStateChanged { client_id: 1, muted: true, deafened: true },
            ServerMessage::SharingChanged { client_id: 1, sharing: true },
            ServerMessage::VideoFrame { from_id: 1, kind: VideoKind::Screen, data: vec![9, 8, 7] },
            ServerMessage::VideoStopped { from_id: 1, kind: VideoKind::Camera },
            ServerMessage::ServerSettingsChanged { settings: ServerSettings::default() },
            ServerMessage::Poked { from_id: 3, from_name: "Phil".into(), message: "Aufwachen!".into() },
            ServerMessage::Notice { message: "AFK".into() },
            ServerMessage::Error { message: "Fehler".into() },
        ];
        for msg in msgs {
            let bytes = bincode::serialize(&msg).expect("serialize");
            let back: ServerMessage = bincode::deserialize(&bytes).expect("deserialize");
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn template_placeholders() {
        assert_eq!(fill_template(DEFAULT_JOIN_MESSAGE, &[("user", "anna")]), "anna joined the server");
        assert_eq!(
            fill_template(DEFAULT_AFK_NOTICE, &[("channel", "Lobby"), ("minutes", "10")]),
            "You were moved to Lobby after 10 min muted (AFK)"
        );
        assert_eq!(fill_template("no vars {x}", &[]), "no vars {x}", "unknown placeholders stay");
        assert_eq!(fill_template("{user} & {user}", &[("user", "b")]), "b & b");
    }
}
