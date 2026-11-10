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
    /// GIPHY-API-Key (kostenlos unter developers.giphy.com).
    #[serde(default)]
    pub giphy_api_key: Option<String>,
    #[serde(default)]
    pub bookmarks: Vec<ServerBookmark>,
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
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
            giphy_api_key: None,
            bookmarks: Vec::new(),
            input_device: None,
            output_device: None,
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
            giphy_api_key: Some("test-key".into()),
            bookmarks: vec![
                ServerBookmark { name: "Home".into(), address: "10.69.40.101:7331".into(), password: None },
                ServerBookmark { name: "LAN".into(), address: "192.168.1.5".into(), password: Some("pw".into()) },
            ],
            input_device: Some("Mikro".into()),
            output_device: None,
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: ClientConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.username, "jb");
        assert_eq!(back.bookmarks.len(), 2);
        assert_eq!(back.bookmarks[0].name, "Home");
        assert_eq!(back.bookmarks[1].password.as_deref(), Some("pw"));
        assert!(!back.noise_suppression);
    }

    #[test]
    fn empty_file_yields_defaults() {
        let cfg: ClientConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.username, "User");
        assert_eq!(cfg.ptt_key, "V");
        assert!(!cfg.voice_activation);
        assert!(cfg.noise_suppression, "Rauschunterdrueckung default an");
        assert!(cfg.bookmarks.is_empty());
        assert!(cfg.server_address.ends_with(":7331"));
    }
}
