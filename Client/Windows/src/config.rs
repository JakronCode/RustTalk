//! Lokale Client-Konfiguration (`client_config.toml` im Arbeitsverzeichnis).
//!
//! Bewusst lokal-first: keine Online-Accounts, nur diese Datei.

use serde::{Deserialize, Serialize};

/// Ein gespeicherter Server im Lesezeichen-Bereich des Startfensters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerBookmark {
    /// Nickname; empty (or the address, as older versions saved) means none.
    #[serde(default)]
    pub name: String,
    pub address: String,
    /// Achtung: Klartext in der lokalen Config-Datei (bewusst lokal-first).
    #[serde(default)]
    pub password: Option<String>,
}

/// Longest nickname for a saved server.
pub const MAX_NICKNAME_CHARS: usize = 48;

impl ServerBookmark {
    pub fn nickname(&self) -> Option<&str> {
        let name = self.name.trim();
        (!name.is_empty() && name != self.address.trim()).then_some(name)
    }

    /// Stores a trimmed, length-bounded nickname (empty clears it).
    pub fn set_nickname(&mut self, name: &str) {
        self.name = name.trim().chars().take(MAX_NICKNAME_CHARS).collect();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default = "default_server_address")]
    pub server_address: String,
    #[serde(default)]
    pub server_password: Option<String>,
    #[serde(default = "default_ptt_key")]
    pub ptt_key: String,
    #[serde(default)]
    pub mute_key: Option<String>,
    #[serde(default)]
    pub deafen_key: Option<String>,
    #[serde(default)]
    pub voice_activation: bool,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,
    /// KI-Rauschunterdrueckung (RNNoise) vor dem Opus-Encoder.
    #[serde(default = "default_true")]
    pub noise_suppression: bool,
    /// Send the computer audio while sharing the screen.
    #[serde(default = "default_true")]
    pub share_screen_audio: bool,
    /// Echo cancellation: lets you use speakers instead of headphones.
    #[serde(default = "default_true")]
    pub echo_cancellation: bool,
    /// Automatische Lautstaerke fuer das eigene Mikro (AGC + Limiter).
    #[serde(default = "default_true")]
    pub auto_gain: bool,
    #[serde(default = "default_bookmarks")]
    pub bookmarks: Vec<ServerBookmark>,
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
    /// Zuletzt geteilte Kamera (Name), Vorauswahl im Kamera-Dialog.
    #[serde(default)]
    pub camera: Option<String>,
    /// Qualitaet gesendeter Video-Streams (Kamera + Bildschirm).
    #[serde(default)]
    pub video_quality: crate::video::VideoQuality,
    /// UI zoom (1.0 = 100 %).
    #[serde(default = "default_one")]
    pub ui_scale: f32,
    /// Show the time next to chat messages.
    #[serde(default = "default_true")]
    pub show_timestamps: bool,
    /// Playback volume for everyone (1.0 = 100 %, max 2.0).
    #[serde(default = "default_one")]
    pub output_volume: f32,
    /// Secret identity (64 hex chars, created on first start). Roles and bans
    /// on servers hang on it - keep it private, don't share this file.
    #[serde(default)]
    pub identity: String,
    /// TLS certificate fingerprints of servers we connected to ("host:port" -> SHA-256).
    #[serde(default)]
    pub known_servers: std::collections::BTreeMap<String, String>,
    /// Saved servers ("host:port") reconnected when RustTalk starts: the user
    /// connected to them and did not disconnect. A saved address alone is not
    /// consent, so older configs start with none.
    #[serde(default)]
    pub auto_connect: std::collections::BTreeSet<String>,
    /// Invite code for the next connect (from an invite link; not saved).
    #[serde(skip)]
    pub invite: Option<String>,
    /// Windows pop-up for new messages while RustTalk is in the background.
    #[serde(default = "default_true")]
    pub desktop_notifications: bool,
    /// Show the message text in the pop-up (off = only "New message").
    #[serde(default = "default_true")]
    pub notification_preview: bool,
    /// Ask GitHub for a newer release on start.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// Flash the taskbar icon on new messages while RustTalk is in the background.
    #[serde(default = "default_true")]
    pub flash_taskbar: bool,
    /// Notification sounds.
    #[serde(default = "default_true")]
    pub sound_join: bool,
    #[serde(default = "default_true")]
    pub sound_leave: bool,
    #[serde(default = "default_true")]
    pub sound_message: bool,
    #[serde(default = "default_true")]
    pub sound_mute: bool,
    /// Say the user's name after the join / leave sound.
    #[serde(default = "default_true")]
    pub announce_names: bool,
    /// Voice for the names: a Windows voice name, "neural:<preset>" for a
    /// natural voice, or None for the Windows default voice.
    #[serde(default)]
    pub announce_voice: Option<String>,
    /// Language of the sentence with natural voices ("en" or "de").
    #[serde(default = "default_announce_language")]
    pub announce_language: String,
    /// Download the natural voices on their own after installing (once, in the background).
    #[serde(default = "default_true")]
    pub auto_download_voices: bool,
    /// The one automatic switch to a natural voice has happened (never again after that).
    #[serde(default)]
    pub voices_switched: bool,
    /// Play animated GIFs in chat (off = first frame only).
    #[serde(default = "default_true")]
    pub animate_gifs: bool,
    /// Push-to-talk also works while RustTalk is in the background.
    #[serde(default)]
    pub global_ptt: bool,
    /// Manual microphone gain before the processing chain (1.0 = 100 %, max 2.0).
    #[serde(default = "default_one")]
    pub input_volume: f32,
    /// Per-user playback volume by username (local only; missing = 100 %).
    #[serde(default)]
    pub user_volumes: std::collections::BTreeMap<String, f32>,
    /// Compact layout: tighter spacing, chat without avatars.
    #[serde(default)]
    pub compact: bool,
    /// Primary (accent) color as RGB.
    #[serde(default = "default_accent")]
    pub accent_color: [u8; 3],
    /// Width of the channel sidebar in points (drag its right edge).
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    /// Link origins ("https://host[:port]") opened without asking -> unix time trusted.
    /// Bounded and expiring; see `app::links`.
    #[serde(default)]
    pub trusted_links: std::collections::BTreeMap<String, u64>,
    /// Local threat feed file the user imported (read only, never downloaded by RustTalk).
    #[serde(default)]
    pub threat_feed: Option<String>,
}

fn default_sidebar_width() -> f32 {
    260.0
}

pub const DEFAULT_ACCENT: [u8; 3] = [0x58, 0x65, 0xF2];

fn default_accent() -> [u8; 3] {
    DEFAULT_ACCENT
}

fn default_username() -> String {
    "User".to_string()
}
fn default_server_address() -> String {
    format!("127.0.0.1:{}", rusttalk_shared::protocol::DEFAULT_PORT)
}
fn default_ptt_key() -> String {
    "F8".to_string()
}
fn default_vad_threshold() -> f32 {
    0.02
}
fn default_announce_language() -> String {
    "en".to_string()
}
fn default_one() -> f32 {
    1.0
}
fn default_true() -> bool {
    true
}

fn default_bookmarks() -> Vec<ServerBookmark> {
    vec![ServerBookmark {
        name: "rusttalk official server".to_owned(),
        address: "public.rusttalk.com:7331".to_owned(),
        password: None,
    }]
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            username: default_username(),
            server_address: default_server_address(),
            server_password: None,
            ptt_key: default_ptt_key(),
            mute_key: None,
            deafen_key: None,
            voice_activation: false,
            vad_threshold: default_vad_threshold(),
            noise_suppression: true,
            auto_gain: true,
            echo_cancellation: true,
            share_screen_audio: true,
            bookmarks: default_bookmarks(),
            input_device: None,
            output_device: None,
            camera: None,
            video_quality: crate::video::VideoQuality::default(),
            ui_scale: 1.0,
            show_timestamps: true,
            output_volume: 1.0,
            identity: String::new(),
            known_servers: Default::default(),
            auto_connect: Default::default(),
            invite: None,
            desktop_notifications: true,
            notification_preview: true,
            check_updates: true,
            flash_taskbar: true,
            sound_join: true,
            sound_leave: true,
            sound_message: true,
            sound_mute: true,
            announce_names: true,
            announce_voice: None,
            announce_language: default_announce_language(),
            auto_download_voices: true,
            voices_switched: false,
            animate_gifs: true,
            global_ptt: false,
            input_volume: 1.0,
            user_volumes: Default::default(),
            compact: false,
            accent_color: DEFAULT_ACCENT,
            sidebar_width: default_sidebar_width(),
            trusted_links: Default::default(),
            threat_feed: None,
        }
    }
}

impl ClientConfig {
    /// Settings file: `%APPDATA%\RustTalk` when installed, else the working dir.
    pub fn path() -> std::path::PathBuf {
        crate::install::config_path()
    }

    /// Laedt die Config; bei fehlender/kaputter Datei: Defaults.
    pub fn load() -> Self {
        let mut cfg: Self = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        if cfg.ensure_identity() {
            cfg.save();
        }
        cfg
    }

    /// Creates the identity secret if missing. Returns true if it was created.
    pub fn ensure_identity(&mut self) -> bool {
        let valid = self.identity.len() == rusttalk_shared::protocol::IDENTITY_HEX_LEN && self.identity.chars().all(|c| c.is_ascii_hexdigit());
        if valid {
            return false;
        }
        let mut bytes = [0u8; 32];
        if getrandom::fill(&mut bytes).is_err() {
            // Practically impossible; fall back to time + address entropy.
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = ((seed >> ((i % 16) * 8)) as u8) ^ (i as u8).wrapping_mul(97);
            }
        }
        self.identity = bytes.iter().map(|b| format!("{b:02x}")).collect();
        true
    }

    /// Speichert die Config (Fehler werden nur geloggt).
    pub fn save(&self) {
        if cfg!(test) {
            return; // tests never touch the real settings file
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                let path = Self::path();
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::warn!("Config konnte nicht gespeichert werden: {e}");
                }
            }
            Err(e) => tracing::warn!("Config konnte nicht serialisiert werden: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_bookmark_is_default_but_removal_is_preserved() {
        let fresh = ClientConfig::default();
        assert_eq!(fresh.bookmarks, default_bookmarks());
        assert_eq!(fresh.bookmarks[0].nickname(), Some("rusttalk official server"));
        assert!(fresh.bookmarks[0].password.is_none());
        let empty: ClientConfig = toml::from_str("bookmarks = []").unwrap();
        assert!(empty.bookmarks.is_empty());
        let back: ClientConfig = toml::from_str(&toml::to_string(&empty).unwrap()).unwrap();
        assert!(back.bookmarks.is_empty());
        let custom: ClientConfig = toml::from_str(
            "[[bookmarks]]\nname = 'Mine'\naddress = 'localhost:7331'",
        ).unwrap();
        assert_eq!(custom.bookmarks.len(), 1);
        assert_eq!(custom.bookmarks[0].name, "Mine");
    }

    #[test]
    fn roundtrip() {
        let cfg = ClientConfig {
            username: "jb".into(),
            server_address: "10.69.40.101:7331".into(),
            server_password: Some("x".into()),
            ptt_key: "SPACE".into(),
            mute_key: Some("F10".into()),
            deafen_key: Some("Mouse5".into()),
            voice_activation: true,
            vad_threshold: 0.05,
            noise_suppression: false,
            auto_gain: false,
            echo_cancellation: false,
            share_screen_audio: false,
            bookmarks: vec![
                ServerBookmark {
                    name: "Home".into(),
                    address: "10.69.40.101:7331".into(),
                    password: None,
                },
                ServerBookmark {
                    name: "LAN".into(),
                    address: "192.168.1.5".into(),
                    password: Some("pw".into()),
                },
            ],
            input_device: Some("Mikro".into()),
            output_device: None,
            camera: None,
            video_quality: crate::video::VideoQuality::default(),
            ui_scale: 1.0,
            show_timestamps: true,
            output_volume: 1.0,
            identity: "ab".repeat(32),
            known_servers: [("10.0.0.5:7331".to_string(), "AB:CD".to_string())].into_iter().collect(),
            auto_connect: ["10.69.40.101:7331".to_string()].into_iter().collect(),
            invite: Some("NOT-SAVED".into()),
            desktop_notifications: false,
            notification_preview: false,
            check_updates: false,
            flash_taskbar: false,
            sound_join: true,
            sound_leave: false,
            sound_message: true,
            sound_mute: false,
            announce_names: false,
            announce_voice: Some("neural:F1".into()),
            announce_language: "de".into(),
            auto_download_voices: false,
            voices_switched: true,
            animate_gifs: false,
            global_ptt: true,
            input_volume: 0.5,
            user_volumes: [("anna".to_string(), 1.5)].into_iter().collect(),
            compact: true,
            accent_color: [0xEB, 0x45, 0x95],
            sidebar_width: 320.0,
            trusted_links: [("https://github.com".to_string(), 1_700_000_000)].into_iter().collect(),
            threat_feed: Some("feeds/urlhaus.txt".into()),
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: ClientConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.username, "jb");
        assert_eq!(back.mute_key.as_deref(), Some("F10"));
        assert_eq!(back.deafen_key.as_deref(), Some("Mouse5"));
        assert_eq!(back.bookmarks.len(), 2);
        assert_eq!(back.bookmarks[0].name, "Home");
        assert_eq!(back.bookmarks[1].password.as_deref(), Some("pw"));
        assert!(!back.noise_suppression);
        assert!(!back.auto_gain);
        assert!(!back.echo_cancellation);
        assert_eq!(back.input_volume, 0.5);
        assert!(back.global_ptt);
        assert!(back.compact);
        assert_eq!(back.accent_color, [0xEB, 0x45, 0x95]);
        assert_eq!(back.sidebar_width, 320.0);
        assert!(!back.animate_gifs);
        assert!(!back.flash_taskbar && !back.sound_leave && back.sound_join);
        assert!(!back.announce_names);
        assert_eq!(back.announce_voice.as_deref(), Some("neural:F1"));
        assert_eq!(back.announce_language, "de");
        assert!(!back.desktop_notifications && !back.notification_preview && !back.check_updates);
        assert_eq!(back.identity, "ab".repeat(32));
        assert_eq!(back.invite, None, "invite codes are not saved");
        assert_eq!(back.known_servers.get("10.0.0.5:7331").map(String::as_str), Some("AB:CD"));
        assert!(back.auto_connect.contains("10.69.40.101:7331"));
        assert_eq!(back.user_volumes.get("anna"), Some(&1.5));
        assert_eq!(back.trusted_links.get("https://github.com"), Some(&1_700_000_000));
        assert_eq!(back.threat_feed.as_deref(), Some("feeds/urlhaus.txt"));
    }

    #[test]
    fn bookmark_nicknames() {
        let text = r#"
            [[bookmarks]]
            name = "gw01.el.k0mplex.de"
            address = "gw01.el.k0mplex.de"

            [[bookmarks]]
            address = "10.0.0.5:7331"

            [[bookmarks]]
            name = "Jakob's Devbude"
            address = "gw02.example.org"
            password = "pw"
        "#;
        let mut cfg: ClientConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.bookmarks.len(), 3);
        assert_eq!(cfg.bookmarks[0].nickname(), None, "old bookmarks saved the address as name");
        assert_eq!(cfg.bookmarks[1].nickname(), None, "name is optional");
        assert_eq!(cfg.bookmarks[2].nickname(), Some("Jakob's Devbude"));

        cfg.bookmarks[1].set_nickname("  Home  ");
        cfg.bookmarks[2].set_nickname("");
        cfg.bookmarks[0].set_nickname(&"x".repeat(200));
        let back: ClientConfig = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.bookmarks[1].nickname(), Some("Home"));
        assert_eq!(back.bookmarks[2].nickname(), None);
        assert_eq!(back.bookmarks[2].password.as_deref(), Some("pw"));
        assert_eq!(back.bookmarks[0].name.chars().count(), MAX_NICKNAME_CHARS);
    }

    #[test]
    fn identity_is_created_once() {
        let mut cfg = ClientConfig::default();
        assert!(cfg.ensure_identity());
        assert_eq!(cfg.identity.len(), 64);
        let first = cfg.identity.clone();
        assert!(!cfg.ensure_identity(), "kept");
        assert_eq!(cfg.identity, first);
        let mut other = ClientConfig::default();
        other.ensure_identity();
        assert_ne!(other.identity, first, "random");
    }

    #[test]
    fn empty_file_yields_defaults() {
        let cfg: ClientConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.username, "User");
        assert_eq!(cfg.ptt_key, "F8");
        assert!(cfg.mute_key.is_none() && cfg.deafen_key.is_none());
        assert!(!cfg.voice_activation);
        assert!(cfg.noise_suppression, "Rauschunterdrueckung default an");
        assert!(cfg.auto_gain, "Auto-Lautstaerke default an (alte Configs ohne Feld)");
        assert_eq!(cfg.bookmarks, default_bookmarks());
        assert_eq!(cfg.input_volume, 1.0);
        assert!(!cfg.global_ptt, "global PTT is opt-in");
        assert!(cfg.animate_gifs);
        assert!(cfg.desktop_notifications && cfg.notification_preview && cfg.check_updates);
        assert!(cfg.flash_taskbar && cfg.sound_join && cfg.sound_leave && cfg.sound_message && cfg.sound_mute);
        assert!(cfg.announce_names && cfg.announce_voice.is_none());
        assert_eq!(cfg.announce_language, "en");
        assert!(cfg.user_volumes.is_empty());
        assert!(!cfg.compact);
        assert_eq!(cfg.accent_color, DEFAULT_ACCENT);
        assert_eq!(cfg.sidebar_width, 260.0);
        assert!(cfg.server_address.ends_with(":7331"));
        assert!(cfg.auto_connect.is_empty(), "existing installs are not enrolled");
        assert!(cfg.trusted_links.is_empty() && cfg.threat_feed.is_none(), "links ask and no feed by default");
    }
}
