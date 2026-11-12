//! The server's own TLS certificate (self-signed, created on first start).
//!
//! Stored next to the database (`tls-cert.der`, `tls-key.der`) so the
//! fingerprint stays the same across restarts - clients pin it on first
//! connect and refuse a different one later.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

/// Loads or creates the certificate. Returns (cert DER, key DER).
pub fn load_or_create(dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let (cert_path, key_path) = paths(dir);
    if let (Some(cert), Some(key)) = (read_if_present(&cert_path)?, read_if_present(&key_path)?) {
        return Ok((cert, key));
    }
    let ck = rcgen::generate_simple_self_signed(vec![rusttalk_shared::secure::TLS_SERVER_NAME.to_string()])
        .context("Could not create certificate")?;
    let cert = ck.cert.der().to_vec();
    let key = ck.signing_key.serialize_der();
    std::fs::create_dir_all(dir).ok();
    write_private(&cert_path, &cert)?;
    write_private(&key_path, &key)?;
    info!("Created new TLS certificate: {}", cert_path.display());
    Ok((cert, key))
}

/// `None` only if the file does not exist; any other error (e.g. wrong owner) stops the
/// start instead of silently replacing the key that clients have pinned.
fn read_if_present(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Could not read {}", path.display())),
    }
}

fn paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("tls-cert.der"), dir.join("tls-key.der"))
}

/// Readable only by the server user (Unix: mode 0600 from creation on, never wider).
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    // Only a leftover half of a pair can be here; it is useless without the other half.
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .and_then(|mut f| f.write_all(data).and_then(|()| f.sync_all()))
        .with_context(|| format!("Could not write {}", path.display()))
}

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
        #[cfg(unix)]
        for name in ["tls-cert.der", "tls-key.der"] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
