//! Laden der Server-Konfiguration (`server.toml`).
//!
//! Beispiel:
//! ```toml
//! bind_address = "0.0.0.0:7331"
//! server_name = "Mein Server"
//!
//! [[channels]]
//! name = "Spawn"                    # optional; sonst wird "Spawn" automatisch erstellt
//! kind = "spawn"
//!
//! [[channels]]
//! name = "Text"
//! kind = "category"
//!
//! [[channels]]
//! name = "allgemein"
//! kind = "text"
//! category = "Text"                 # Kategorie per Name referenzieren
//!
//! [[channels]]
//! name = "Funkturm 116"
//! kind = "voice"
//! category = "Voice-Area"
//! ```

use serde::Deserialize;
use rusttalk_shared::protocol::ChannelKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKindConfig {
    Spawn,
    Category,
    Text,
    Voice,
}

impl From<ChannelKindConfig> for ChannelKind {
    fn from(k: ChannelKindConfig) -> Self {
        match k {
            ChannelKindConfig::Spawn => ChannelKind::Spawn,
            ChannelKindConfig::Category => ChannelKind::Category,
            ChannelKindConfig::Text => ChannelKind::Text,
            ChannelKindConfig::Voice => ChannelKind::Voice,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConfig {
    pub name: String,
    /// Standard: Sprach-Channel.
    #[serde(default = "default_kind")]
    pub kind: ChannelKindConfig,
    /// Optionale Kategorie (per Name, muss vorher definiert sein).
    #[serde(default)]
    pub category: Option<String>,
}

const fn default_kind() -> ChannelKindConfig {
    ChannelKindConfig::Voice
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_server_name")]
    pub server_name: String,
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
    /// Optionales Server-Passwort (MVP: Klartext in der Config, LAN-Einsatz).
    #[serde(default)]
    pub password: Option<String>,
    /// SQLite-Persistenz fuer Channels, Chat-Verlauf und Spawn-Log.
    #[serde(default)]
    pub persist_state: bool,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    /// Start-Channels. Ohne spawn-Eintrag wird automatisch "Spawn" erstellt.
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
}

fn default_bind_address() -> String {
    format!("0.0.0.0:{}", rusttalk_shared::protocol::DEFAULT_PORT)
}
fn default_server_name() -> String {
    "RustTalk Server".to_string()
}
fn default_max_clients() -> usize {
    32
}
fn default_db_path() -> String {
    "rusttalk.db".to_string()
}

impl ServerConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: ServerConfig = toml::from_str(&text)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let toml_text = r#"
bind_address = "0.0.0.0:7331"
server_name = "Testserver"
max_clients = 16
password = "geheim"

[[channels]]
name = "Spawn"
kind = "spawn"

[[channels]]
name = "Text"
kind = "category"

[[channels]]
name = "info"
kind = "text"
category = "Text"

[[channels]]
name = "Funkturm 116"
category = "Voice-Area"
"#;
        let cfg: ServerConfig = toml::from_str(toml_text).expect("parse");
        assert_eq!(cfg.bind_address, "0.0.0.0:7331");
        assert_eq!(cfg.channels.len(), 4);
        assert_eq!(cfg.channels[0].kind, ChannelKindConfig::Spawn);
        assert_eq!(cfg.channels[1].kind, ChannelKindConfig::Category);
        assert_eq!(cfg.channels[2].kind, ChannelKindConfig::Text);
        assert_eq!(cfg.channels[2].category.as_deref(), Some("Text"));
        // Default-Kind = Voice:
        assert_eq!(cfg.channels[3].kind, ChannelKindConfig::Voice);
        assert_eq!(cfg.channels[3].category.as_deref(), Some("Voice-Area"));
    }

    #[test]
    fn defaults_are_applied() {
        let cfg: ServerConfig = toml::from_str("").expect("parse empty");
        assert!(cfg.bind_address.ends_with(":7331"));
        assert_eq!(cfg.max_clients, 32);
        assert!(cfg.password.is_none());
        assert!(cfg.channels.is_empty());
        assert!(!cfg.persist_state, "Persistenz default aus");
        assert_eq!(cfg.db_path, "rusttalk.db");
    }

    #[test]
    fn parses_persistence_fields() {
        let cfg: ServerConfig =
            toml::from_str("persist_state = true\ndb_path = \"/tmp/test.db\"").unwrap();
        assert!(cfg.persist_state);
        assert_eq!(cfg.db_path, "/tmp/test.db");
    }
}
