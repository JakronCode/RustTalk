//! Lokale Client-Konfiguration (`client_config.toml` im Arbeitsverzeichnis).
//!
//! Bewusst lokal-first: keine Online-Accounts, nur diese Datei.

use serde::{Deserialize, Serialize};

/// Ein gespeicherter Server im Lesezeichen-Bereich des Startfensters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerBookmark {
    pub name: String,
    pub address: String,
    /// Achtung: Klartext in der lokalen Config-Datei (bewusst lokal-first).
    #[serde(default)]
    pub password: Option<String>,
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
    pub voice_activation: bool,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,
    /// KI-Rauschunterdrueckung (RNNoise) vor dem Opus-Encoder.
    #[serde(default = "default_true")]
    pub noise_suppression: bool,
    /// Automatische Lautstaerke fuer das eigene Mikro (AGC + Limiter).
    #[serde(default = "default_true")]
    pub auto_gain: bool,
    #[serde(default)]
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
}

fn default_username() -> String {
    "User".to_string()
}
fn default_server_address() -> String {
    format!("127.0.0.1:{}", rusttalk_shared::protocol::DEFAULT_PORT)
}
fn default_ptt_key() -> String {
    "V".to_string()
}
fn default_vad_threshold() -> f32 {
    0.02
}
fn default_one() -> f32 {
    1.0
}
fn default_true() -> bool {
    true
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            username: default_username(),
            server_address: default_server_address(),
            server_password: None,
            ptt_key: default_ptt_key(),
            voice_activation: false,
            vad_threshold: default_vad_threshold(),
            noise_suppression: true,
            auto_gain: true,
            bookmarks: Vec::new(),
            input_device: None,
            output_device: None,
            camera: None,
            video_quality: crate::video::VideoQuality::default(),
            ui_scale: 1.0,
            show_timestamps: true,
            output_volume: 1.0,
            flash_taskbar: true,
            sound_join: true,
            sound_leave: true,
            sound_message: true,
            sound_mute: true,
            animate_gifs: true,
            global_ptt: false,
            input_volume: 1.0,
            user_volumes: Default::default(),
        }
    }
}

impl ClientConfig {
    pub const PATH: &'static str = "client_config.toml";

    /// Laedt die Config; bei fehlender/kaputter Datei: Defaults.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::PATH)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Speichert die Config (Fehler werden nur geloggt).
    pub fn save(&self) {
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(Self::PATH, text) {
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
    fn roundtrip() {
        let cfg = ClientConfig {
            username: "jb".into(),
            server_address: "10.69.40.101:7331".into(),
            server_password: Some("x".into()),
            ptt_key: "SPACE".into(),
            voice_activation: true,
            vad_threshold: 0.05,
            noise_suppression: false,
            auto_gain: false,
            bookmarks: vec![
                ServerBookmark { name: "Home".into(), address: "10.69.40.101:7331".into(), password: None },
                ServerBookmark { name: "LAN".into(), address: "192.168.1.5".into(), password: Some("pw".into()) },
            ],
            input_device: Some("Mikro".into()),
            output_device: None,
            camera: None,
            video_quality: crate::video::VideoQuality::default(),
            ui_scale: 1.0,
            show_timestamps: true,
            output_volume: 1.0,
            flash_taskbar: false,
            sound_join: true,
            sound_leave: false,
            sound_message: true,
            sound_mute: false,
            animate_gifs: false,
            global_ptt: true,
            input_volume: 0.5,
            user_volumes: [("anna".to_string(), 1.5)].into_iter().collect(),
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: ClientConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.username, "jb");
        assert_eq!(back.bookmarks.len(), 2);
        assert_eq!(back.bookmarks[0].name, "Home");
        assert_eq!(back.bookmarks[1].password.as_deref(), Some("pw"));
        assert!(!back.noise_suppression);
        assert!(!back.auto_gain);
        assert_eq!(back.input_volume, 0.5);
        assert!(back.global_ptt);
        assert!(!back.animate_gifs);
        assert!(!back.flash_taskbar && !back.sound_leave && back.sound_join);
        assert_eq!(back.user_volumes.get("anna"), Some(&1.5));
    }

    #[test]
    fn empty_file_yields_defaults() {
        let cfg: ClientConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.username, "User");
        assert_eq!(cfg.ptt_key, "V");
        assert!(!cfg.voice_activation);
        assert!(cfg.noise_suppression, "Rauschunterdrueckung default an");
        assert!(cfg.auto_gain, "Auto-Lautstaerke default an (alte Configs ohne Feld)");
        assert!(cfg.bookmarks.is_empty());
        assert_eq!(cfg.input_volume, 1.0);
        assert!(!cfg.global_ptt, "global PTT is opt-in");
        assert!(cfg.animate_gifs);
        assert!(cfg.flash_taskbar && cfg.sound_join && cfg.sound_leave && cfg.sound_message && cfg.sound_mute);
        assert!(cfg.user_volumes.is_empty());
        assert!(cfg.server_address.ends_with(":7331"));
    }
}
