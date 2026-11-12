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

use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context};
use rusttalk_shared::protocol::ChannelKind;
use serde::Deserialize;

/// Names a file holding the admin password (Docker/systemd secret); overrides `admin_password`.
pub const ADMIN_PASSWORD_FILE_ENV: &str = "RUSTTALK_ADMIN_PASSWORD_FILE";
const SECRET_MIN_CHARS: usize = 16;
const SECRET_MAX_BYTES: u64 = 1024;

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
    /// Makes whoever enters it (Server Settings > Roles) an admin. Optional;
    /// without any admin the server is open (everyone may do everything).
    #[serde(default)]
    pub admin_password: Option<String>,
    /// SQLite-Persistenz fuer Channels, Chat-Verlauf und Spawn-Log.
    #[serde(default)]
    pub persist_state: bool,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    /// Look for a newer release on GitHub (on start, then daily) and tell
    /// the admins. Never installs anything by itself.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// Start-Channels. Ohne spawn-Eintrag wird automatisch "Spawn" erstellt.
    #[serde(default)]
    pub channels: Vec<ChannelConfig>,
    /// Set by the container image, not by server.toml: no "open" mode, becoming
    /// admin always needs the admin password.
    #[serde(skip)]
    pub admin_password_required: bool,
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
fn default_true() -> bool {
    true
}
fn default_db_path() -> String {
    "rusttalk.db".to_string()
}

impl ServerConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: ServerConfig = toml::from_str(&text).map_err(|e: toml::de::Error| {
            let offset = e.span().map(|s| s.start).unwrap_or(0).min(text.len());
            let line = text.as_bytes()[..offset]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
                + 1;
            anyhow::anyhow!("Invalid server configuration near line {line}")
        })?;
        Ok(cfg)
    }
}

/// Reads a one-line secret from a file. Fails closed; errors never contain the content.
pub fn read_secret_file(path: &Path) -> anyhow::Result<String> {
    let name = path.display();
    let meta =
        std::fs::metadata(path).with_context(|| format!("Cannot read secret file {name}"))?;
    if !meta.is_file() {
        bail!("Secret file {name} is not a regular file");
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(SECRET_MAX_BYTES + 1).read_to_end(&mut buf))
        .with_context(|| format!("Cannot read secret file {name}"))?;
    if buf.len() as u64 > SECRET_MAX_BYTES {
        bail!("Secret file {name} is larger than {SECRET_MAX_BYTES} bytes");
    }
    let Ok(text) = String::from_utf8(buf) else {
        bail!("Secret file {name} is not UTF-8 text")
    };
    let secret = text.trim_end_matches(['\r', '\n']);
    if secret.chars().any(char::is_control) {
        bail!("Secret file {name} must hold a single line without control characters");
    }
    if secret.chars().count() < SECRET_MIN_CHARS {
        bail!("Secret in {name} is shorter than {SECRET_MIN_CHARS} characters");
    }
    Ok(secret.to_string())
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

    #[test]
    fn secret_file_is_bounded_and_fails_closed() {
        let dir = std::env::temp_dir().join(format!("rusttalk-secret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, data: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, data).unwrap();
            p
        };
        let marker = "TOPSECRET-MARKER";

        // One line, the line ending is not part of the secret (echo / Windows editors).
        assert_eq!(
            read_secret_file(&write("ok", b"correct horse battery\n")).unwrap(),
            "correct horse battery"
        );
        assert_eq!(
            read_secret_file(&write("crlf", b" spaces stay inside \r\n")).unwrap(),
            " spaces stay inside "
        );
        let exact = "x".repeat(SECRET_MAX_BYTES as usize);
        assert_eq!(
            read_secret_file(&write("max", exact.as_bytes())).unwrap(),
            exact
        );

        let bad: Vec<(std::path::PathBuf, &str)> = vec![
            (dir.join("missing"), "Cannot read"),
            (dir.clone(), "not a regular file"),
            (write("empty", b""), "shorter than"),
            (write("newline", b"\n"), "shorter than"),
            (
                write("short", format!("{}\n", &marker[..9]).as_bytes()),
                "shorter than",
            ),
            (
                write(
                    "big",
                    format!("{marker}{}", "y".repeat(SECRET_MAX_BYTES as usize)).as_bytes(),
                ),
                "larger than",
            ),
            (
                write("twolines", format!("{marker}\nsecond line").as_bytes()),
                "single line",
            ),
            (
                write("tab", format!("{marker}\tmore").as_bytes()),
                "single line",
            ),
            (
                write(
                    "binary",
                    &[
                        0xff, 0xfe, b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h', b'i', b'j',
                        b'k', b'l', b'm', b'n',
                    ],
                ),
                "not UTF-8",
            ),
        ];
        for (path, want) in &bad {
            let err = format!("{:#}", read_secret_file(path).unwrap_err());
            assert!(err.contains(want), "{}: {err}", path.display());
            assert!(!err.contains(&marker[..9]), "secret leaked into: {err}");
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = write("locked", format!("{marker}-long-enough").as_bytes());
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            // root ignores file modes; the case only means something for a normal user.
            if std::fs::File::open(&locked).is_err() {
                let err = format!("{:#}", read_secret_file(&locked).unwrap_err());
                assert!(
                    err.contains("Cannot read") && !err.contains(marker),
                    "{err}"
                );
            } else {
                eprintln!("note: running as root, unreadable-file case not exercised");
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
