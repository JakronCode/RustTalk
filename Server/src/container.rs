//! Running in the official container image (`RUSTTALK_CONTAINER=1`, see `Server/Dockerfile`):
//! default config on first start, and the `--healthcheck` probe.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::ServerConfig;

pub const CONTAINER_ENV: &str = "RUSTTALK_CONTAINER";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Set by the image; changes first-run, admin and update behaviour.
pub fn enabled() -> bool {
    std::env::var_os(CONTAINER_ENV).is_some_and(|v| v == "1")
}

/// Writes a default config to `path` if there is none yet (mode 0600). Returns true if it wrote one.
pub fn write_default_config(path: &Path) -> Result<bool> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let db_path = dir.join("rusttalk.db");
    // TOML-quoted, so an odd path cannot inject config.
    let db_path = toml::Value::String(db_path.to_string_lossy().into_owned()).to_string();
    let text = DEFAULT_CONFIG.replace("{db_path}", &db_path);

    crate::private_file::create(path, text.as_bytes())
        .with_context(|| format!("Could not create {}", path.display()))
}

const DEFAULT_CONFIG: &str = r#"# RustTalk server configuration, created on the first start of the container.
# Edit it while the server is stopped. The admin password is not stored here:
# it is read from the file named by RUSTTALK_ADMIN_PASSWORD_FILE (a mounted secret).

bind_address = "0.0.0.0:7331"   # TCP and UDP; publish both ports
server_name = "RustTalk Server"
max_clients = 32
# password = "secret"           # optional server password (plain text)
check_updates = true            # tell admins about new releases; update by pulling a new image

persist_state = true
db_path = {db_path}

[[channels]]
name = "Lobby"
kind = "spawn"

[[channels]]
name = "Text Channels"
kind = "category"

[[channels]]
name = "general"
kind = "text"
category = "Text Channels"

[[channels]]
name = "Voice Channels"
kind = "category"

[[channels]]
name = "Lounge"
kind = "voice"
category = "Voice Channels"
"#;

/// `--healthcheck [config]`: exit code 0 if the TCP listener accepts a connection.
/// Only connects and closes again: no TLS, no login, nothing that needs a secret.
pub async fn healthcheck(config_path: &str) -> i32 {
    let probe = async {
        let cfg = ServerConfig::load(config_path)
            .with_context(|| format!("Could not load config '{config_path}'"))?;
        let addr = probe_addr(&cfg.bind_address).await?;
        tokio::net::TcpStream::connect(addr)
            .await
            .with_context(|| format!("{addr} does not accept connections"))?;
        Ok::<(), anyhow::Error>(())
    };
    match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            eprintln!("unhealthy: {e:#}");
            1
        }
        Err(_) => {
            eprintln!("unhealthy: probe timed out");
            1
        }
    }
}

/// Where to reach a listener bound to `bind` from the same host (wildcard -> loopback).
async fn probe_addr(bind: &str) -> Result<SocketAddr> {
    let mut addr = tokio::net::lookup_host(bind)
        .await
        .with_context(|| format!("Invalid bind_address '{bind}'"))?
        .next()
        .with_context(|| format!("bind_address '{bind}' has no address"))?;
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
        });
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_written_once() {
        let dir = std::env::temp_dir().join(format!("rusttalk-container-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("server.toml");

        assert!(write_default_config(&path).unwrap());
        let cfg = ServerConfig::load(path.to_str().unwrap()).unwrap();
        assert_eq!(cfg.bind_address, "0.0.0.0:7331");
        assert!(cfg.persist_state);
        assert!(cfg.check_updates);
        assert_eq!(Path::new(&cfg.db_path), dir.join("rusttalk.db"));
        assert!(
            cfg.admin_password.is_none() && cfg.password.is_none(),
            "no secrets in the file"
        );
        assert_eq!(cfg.channels.len(), 5);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        // An existing config (edited by the owner) is never touched.
        std::fs::write(&path, "server_name = \"Mine\"\n").unwrap();
        assert!(!write_default_config(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "server_name = \"Mine\"\n"
        );

        // A path that would break out of the TOML string stays a plain string (not a valid Windows name).
        #[cfg(unix)]
        {
            let odd = dir.join("we\"ird\ndir");
            std::fs::create_dir_all(&odd).unwrap();
            assert!(write_default_config(&odd.join("server.toml")).unwrap());
            let cfg = ServerConfig::load(odd.join("server.toml").to_str().unwrap()).unwrap();
            assert_eq!(Path::new(&cfg.db_path), odd.join("rusttalk.db"));
        }

        assert!(write_default_config(&dir.join("missing/server.toml")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn probe_maps_wildcards_to_loopback() {
        assert_eq!(
            probe_addr("0.0.0.0:7331").await.unwrap(),
            "127.0.0.1:7331".parse().unwrap()
        );
        assert_eq!(
            probe_addr("[::]:7331").await.unwrap(),
            "[::1]:7331".parse().unwrap()
        );
        assert_eq!(
            probe_addr("192.0.2.5:9000").await.unwrap(),
            "192.0.2.5:9000".parse().unwrap()
        );
        assert!(probe_addr("no-port").await.is_err());
    }
}
