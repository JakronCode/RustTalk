//! Architecture-specific client updates from GitHub releases.
//!
//! Every download requires a SHA-256 checksum. Windows uses the installed
//! executable swap; Linux atomically replaces the running AppImage file.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};

pub use rusttalk_shared::release::{is_newer, parse_sha_file, sha256_hex, REPO};
#[path = "../../../shared/update_discovery.rs"]
mod discovery;

pub const ASSET: &str = if cfg!(target_os = "linux") {
    if cfg!(target_arch = "aarch64") {
        "rusttalk-linux-aarch64.AppImage"
    } else {
        "rusttalk-linux-x86_64.AppImage"
    }
} else if cfg!(target_arch = "aarch64") {
    "rusttalk-windows-arm64.exe"
} else {
    "rusttalk-windows.exe"
};
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

/// A newer release that can be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// e.g. "0.4.0"
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub exe_url: String,
    pub sha_url: Option<String>,
}

/// Reads one stable release with the exact platform asset.
#[cfg(test)]
pub fn parse_release(json: &serde_json::Value) -> Option<Release> {
    discovery::parse_release(json, rusttalk_shared::release::Component::Client, ASSET)
        .ok()
        .flatten()
        .map(from_discovery)
}

fn from_discovery(release: discovery::Release) -> Release {
    Release {
        version: release.version,
        notes: release.notes,
        page_url: release.page_url,
        exe_url: release.asset_url,
        sha_url: release.sha_url,
    }
}

/// API base (tests can point it at a local fake).
fn api_base() -> String {
    std::env::var("RUSTTALK_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// Checks the complete bounded release list. Errors never mean up to date.
pub fn check() -> Result<Option<Release>> {
    Ok(discovery::discover(
        &agent(),
        &api_base(),
        "RustTalk-Updater",
        rusttalk_shared::release::Component::Client,
        ASSET,
    )?
    .map(from_discovery)
    .filter(|r| is_newer(&r.version, env!("CARGO_PKG_VERSION"))))
}

fn download(url: &str, progress: Option<&AtomicU32>) -> Result<Vec<u8>> {
    let resp = agent()
        .get(url)
        .set("User-Agent", "RustTalk-Updater")
        .call()
        .map_err(|e| anyhow::anyhow!("Download failed: {e}"))?;
    let total: Option<u64> = resp.header("Content-Length").and_then(|v| v.parse().ok());
    let mut reader = resp.into_reader().take(MAX_DOWNLOAD + 1);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let (Some(p), Some(t)) = (progress, total) {
            p.store(
                ((buf.len() as u64 * 100) / t.max(1)).min(100) as u32,
                Ordering::Relaxed,
            );
        }
    }
    if buf.len() as u64 > MAX_DOWNLOAD {
        bail!("Download too large");
    }
    Ok(buf)
}

/// Downloads and installs `release` into the install folder. Returns the new
/// exe (start it, then quit). Only for the installed app.
pub fn download_and_install(release: &Release, progress: &AtomicU32) -> Result<PathBuf> {
    let exe = download(&release.exe_url, Some(progress))?;
    validate_program(&exe)?;
    match &release.sha_url {
        Some(url) => {
            let text = String::from_utf8(download(url, None)?).unwrap_or_default();
            let want = parse_sha_file(&text).context("Invalid checksum file in the release")?;
            if sha256_hex(&exe) != want {
                bail!("Checksum mismatch - update not installed");
            }
        }
        None => bail!("The release has no checksum file - update not installed"),
    }
    #[cfg(target_os = "linux")]
    let target = {
        let target =
            crate::install::appimage_path().context("Run the AppImage to install updates")?;
        replace_appimage(&target, &exe)?;
        target
    };
    #[cfg(windows)]
    let target = {
        let dir = crate::install::install_dir().context("Install folder unknown")?;
        let target = dir.join("rusttalk.exe");
        let new = dir.join("rusttalk.new.exe");
        let old = dir.join("rusttalk.old.exe");
        std::fs::write(&new, &exe).context("Could not save the update")?;
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&target, &old).context("Could not replace the running version")?;
        if let Err(e) = std::fs::rename(&new, &target) {
            let _ = std::fs::rename(&old, &target); // roll back
            bail!("Could not install the update: {e}");
        }
        target
    };
    progress.store(100, Ordering::Relaxed);
    Ok(target)
}

fn validate_program(bytes: &[u8]) -> Result<()> {
    #[cfg(windows)]
    if !bytes.starts_with(b"MZ") {
        bail!("The download is not a Windows program");
    }
    #[cfg(target_os = "linux")]
    {
        let machine = if cfg!(target_arch = "aarch64") {
            183u16
        } else {
            62u16
        };
        if bytes.len() < 64
            || &bytes[..4] != b"\x7fELF"
            || bytes[4] != 2
            || bytes[5] != 1
            || &bytes[8..11] != b"AI\x02"
            || u16::from_le_bytes([bytes[18], bytes[19]]) != machine
        {
            bail!("The download is not an AppImage for this architecture");
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn replace_appimage(target: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(target)?;
    if !metadata.is_file() {
        bail!("AppImage must be a regular file");
    }
    let parent = target.parent().context("AppImage folder unknown")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(
            metadata.permissions().mode() & 0o777 | 0o100,
        ))?;
    staged.as_file().sync_all()?;
    // POSIX rename atomically replaces the file; the running mount keeps its old inode.
    staged
        .persist(target)
        .context("Could not replace AppImage; the original is unchanged")?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn restart(exe: &std::path::Path) -> std::io::Result<std::process::Child> {
    let mut command = std::process::Command::new(exe);
    #[cfg(target_os = "linux")]
    {
        for key in [
            "LD_LIBRARY_PATH",
            "GST_PLUGIN_PATH",
            "GST_PLUGIN_SYSTEM_PATH",
            "GST_PLUGIN_SCANNER",
            "XDG_DATA_DIRS",
            "SPA_PLUGIN_DIR",
            "ALSA_PLUGIN_DIR",
        ] {
            match std::env::var_os(format!("RUSTTALK_ORIGINAL_{key}")) {
                Some(value) if !value.is_empty() => {
                    command.env(key, value);
                }
                _ => {
                    command.env_remove(key);
                }
            }
        }
        for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
            command.env_remove(key);
        }
    }
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn appimage_update_validation_and_atomic_replacement() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[8..11].copy_from_slice(b"AI\x02");
        let machine = if cfg!(target_arch = "aarch64") {
            183u16
        } else {
            62u16
        };
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        assert!(validate_program(&bytes).is_ok());
        bytes[18] ^= 1;
        assert!(validate_program(&bytes).is_err());
        bytes[18] ^= 1;
        assert!(validate_program(&bytes[..10]).is_err());
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("RustTalk.AppImage");
        std::fs::write(&target, b"old version").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        let link = dir.path().join("linked.AppImage");
        symlink(&target, &link).unwrap();
        assert!(replace_appimage(&link, &bytes).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old version");
        replace_appimage(&target, &bytes).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    #[test]
    fn release_parsing_needs_the_exe() {
        let json = serde_json::json!({
            "tag_name": "v0.4.0", "draft": false, "prerelease": false,
            "html_url": "https://github.com/x/releases/v0.4.0",
            "body": "Changes",
            "assets": [
                { "name": "rusttalk-v0.4.0.zip", "browser_download_url": "https://dl/zip" },
                { "name": ASSET, "browser_download_url": "https://dl/exe" },
                { "name": format!("{ASSET}.sha256"), "browser_download_url": "https://dl/sha" }
            ]
        });
        let r = parse_release(&json).unwrap();
        assert_eq!(r.version, "0.4.0");
        assert_eq!(r.exe_url, "https://dl/exe");
        assert_eq!(r.sha_url.as_deref(), Some("https://dl/sha"));
        // Old releases (zip only) cannot be installed automatically.
        let old = serde_json::json!({ "tag_name": "v0.3.0", "draft": false, "prerelease": false, "body": null, "html_url": "https://example.invalid", "assets": [{ "name": "rusttalk-v0.3.0.zip", "browser_download_url": "x" }] });
        assert!(parse_release(&old).is_none());
    }
}
