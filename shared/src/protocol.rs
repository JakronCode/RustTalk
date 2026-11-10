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

use serde::{Deserialize, Serialize};

/// Aktuelle Protokollversion. Client und Server müssen übereinstimmen.
pub const PROTOCOL_VERSION: u16 = 4;

/// Maximale Groesse eines Bildes/Video-Frames in Bytes (4 MiB).
pub const MAX_MEDIA_BYTES: usize = 4 * 1024 * 1024;

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
    pub channel_id: u64,
    pub from_name: String,
    pub text: String,
    /// Unix-Timestamp (Sekunden).
    pub timestamp: u64,
    /// Optionales Bild (PNG/JPEG/GIF-Bytes).
    pub image: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: u64,
    pub name: String,
    pub kind: ChannelKind,
    /// Zugehoerige Kategorie (None = Top-Level).
    pub parent: Option<u64>,
    /// Belegte User (nur bei Voice/Spawn sinnvoll befuellt).
    pub user_ids: Vec<u64>,
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
}

/// Client -> Server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Erste Nachricht nach dem TCP-Verbindungsaufbau.
    Hello {
        username: String,
        protocol_version: u16,
        password: Option<String>,
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
    RenameChannel {
        channel_id: u64,
        name: String,
    },
    /// Chat (Text- und Voice-Channels); optionales Bild. Wird an ALLE Clients broadcastet.
    Chat {
        channel_id: u64,
        text: String,
        image: Option<Vec<u8>>,
    },
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
        data: Vec<u8>,
    },
    /// Sauberes Trennen (Server antwortet nicht, schließt nur).
    Disconnect,
}

/// Server -> Client
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Antwort auf `Hello`. Enthält den kompletten Anfangszustand.
    Welcome {
        client_id: u64,
        /// Token, mit dem sich der Client per UDP beim Server registriert.
        udp_token: u64,
        channels: Vec<ChannelInfo>,
        users: Vec<UserInfo>,
        /// Bisheriges Join-Log des Spawn-Channels (aelteste zuerst).
        spawn_log: Vec<LogEntry>,
        /// Letzte Nachrichten der Text-Channels (Persistenz, aelteste zuerst).
        chat_history: Vec<ChatRecord>,
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
    ChannelRenamed {
        channel_id: u64,
        name: String,
    },
    /// Neuer Eintrag im Spawn-Log (bei jedem Join).
    SpawnLogEntry {
        entry: LogEntry,
    },
    Chat {
        channel_id: u64,
        from_id: u64,
        from_name: String,
        text: String,
        /// Unix-Timestamp (Sekunden).
        timestamp: u64,
        image: Option<Vec<u8>>,
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
        data: Vec<u8>,
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
            },
            ClientMessage::Hello {
                username: "ohne passwort".into(),
                protocol_version: PROTOCOL_VERSION,
                password: None,
            },
            ClientMessage::JoinChannel { channel_id: 42 },
            ClientMessage::CreateChannel {
                name: "Gaming".into(),
                kind: ChannelKind::Voice,
                parent: Some(3),
            },
            ClientMessage::DeleteChannel { channel_id: 7 },
            ClientMessage::RenameChannel { channel_id: 7, name: "Neu".into() },
            ClientMessage::Chat { channel_id: 1, text: "Hallo Welt".into(), image: None },
            ClientMessage::Chat { channel_id: 1, text: String::new(), image: Some(vec![0xFF, 0xD8]) },
            ClientMessage::SetMuteState { muted: true, deafened: false },
            ClientMessage::SetSharing { sharing: true },
            ClientMessage::VideoFrame { data: vec![1, 2, 3] },
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
            user_ids: vec![1],
        };
        let entry = LogEntry { timestamp: 1_763_000_000, text: "alice ist erschienen".into() };
        let record = ChatRecord {
            channel_id: 3,
            from_name: "alice".into(),
            text: "moin".into(),
            timestamp: 1_763_000_001,
            image: Some(vec![0x89, 0x50]),
        };
        let msgs = vec![
            ServerMessage::Welcome {
                client_id: 1,
                udp_token: u64::MAX,
                channels: vec![channel.clone()],
                users: vec![user.clone()],
                spawn_log: vec![entry.clone()],
                chat_history: vec![record],
            },
            ServerMessage::UserJoined { user: user.clone() },
            ServerMessage::UserLeft { client_id: 1 },
            ServerMessage::UserMoved { client_id: 1, channel_id: 2 },
            ServerMessage::ChannelCreated { channel },
            ServerMessage::ChannelDeleted { channel_id: 2 },
            ServerMessage::ChannelRenamed { channel_id: 1, name: "Lobby".into() },
            ServerMessage::SpawnLogEntry { entry },
            ServerMessage::Chat {
                channel_id: 1,
                from_id: 1,
                from_name: "alice".into(),
                text: "moin".into(),
                timestamp: 1_763_000_000,
                image: None,
            },
            ServerMessage::MuteStateChanged { client_id: 1, muted: true, deafened: true },
            ServerMessage::SharingChanged { client_id: 1, sharing: true },
            ServerMessage::VideoFrame { from_id: 1, data: vec![9, 8, 7] },
            ServerMessage::Error { message: "Fehler".into() },
        ];
        for msg in msgs {
            let bytes = bincode::serialize(&msg).expect("serialize");
            let back: ServerMessage = bincode::deserialize(&bytes).expect("deserialize");
            assert_eq!(msg, back);
        }
    }
}
