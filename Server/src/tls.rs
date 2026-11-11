//! The server's own TLS certificate (self-signed, created on first start).
//!
//! Stored next to the database (`tls-cert.der`, `tls-key.der`) so the
//! fingerprint stays the same across restarts - clients pin it on first
//! connect and refuse a different one later.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

/// Loads or creates the certificate. Returns (cert DER, key DER).
pub fn load_or_create(dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let (cert_path, key_path) = paths(dir);
    if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        return Ok((cert, key));
    }
    let ck = rcgen::generate_simple_self_signed(vec![rusttalk_shared::secure::TLS_SERVER_NAME.to_string()])
        .context("Zertifikat erzeugen")?;
    let cert = ck.cert.der().to_vec();
    let key = ck.signing_key.serialize_der();
    std::fs::create_dir_all(dir).ok();
    std::fs::write(&cert_path, &cert).with_context(|| format!("{} schreiben", cert_path.display()))?;
    std::fs::write(&key_path, &key).with_context(|| format!("{} schreiben", key_path.display()))?;
    restrict(&key_path);
    info!("Neues TLS-Zertifikat erzeugt: {}", cert_path.display());
    Ok((cert, key))
}

fn paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("tls-cert.der"), dir.join("tls-key.der"))
}

/// Private key readable only by the server user (Unix).
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("rusttalk-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (c1, k1) = load_or_create(&dir).unwrap();
        let (c2, k2) = load_or_create(&dir).unwrap();
        assert_eq!(c1, c2, "same certificate after restart");
        assert_eq!(k1, k2);
        assert!(rusttalk_shared::secure::server_config(c1, k1).is_ok(), "usable for TLS");
        std::fs::remove_dir_all(&dir).ok();
    }
}
