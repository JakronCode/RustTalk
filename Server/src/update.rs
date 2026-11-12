//! Server updates from the GitHub releases of the repository.
//!
//! The server only *checks* (on start and once a day, `check_updates` in
//! server.toml) and tells the admins in the client. Installing is a command
//! run by whoever administers the machine - the service user cannot (and
//! should not) overwrite its own program:
//!
//! ```text
//! sudo /opt/rusttalk/server/bin/rusttalk-server update          # install + restart the service
//! rusttalk-server update --check                                 # only look
//! ```
//!
//! A release carries the bare binary per platform plus `<name>.sha256`; an
//! update without a matching checksum is refused. The new binary has to run
//! (`--version`) before it replaces the old one, which stays as `.old`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusttalk_shared::release::{is_newer, parse_sha_file, sha256_hex, REPO};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;
/// systemd unit from `deploy/` (restarted after an update when it is running).
const SERVICE: &str = "rusttalk-server";

/// Release asset with the server binary for this platform.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("rusttalk-server-linux-x86_64")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("rusttalk-server-windows.exe")
    } else {
        None
    }
}

/// A newer release with a server binary for this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub page_url: String,
    pub bin_url: String,
    pub sha_url: Option<String>,
}

/// Reads a `releases/latest` response. `None` if it has no binary named `asset`.
pub fn parse_release(json: &serde_json::Value, asset: &str) -> Option<Release> {
    let tag = json["tag_name"].as_str()?;
    let assets = json["assets"].as_array()?;
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_string)
    };
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page_url: json["html_url"].as_str().unwrap_or_default().to_string(),
        bin_url: url_of(asset)?,
        sha_url: url_of(&format!("{asset}.sha256")),
    })
}

/// API base (tests point it at a local fake).
fn api_base() -> String {
    std::env::var("RUSTTALK_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(60)).build()
}

/// Asks GitHub for the latest release. `Ok(None)` = up to date (or no binary for this platform).
pub fn check() -> Result<Option<Release>> {
    let Some(asset) = asset_name() else { return Ok(None) };
    let url = format!("{}/repos/{REPO}/releases/latest", api_base());
    // GitHub requires a User-Agent; a generic one, nothing about the server.
    let resp = agent().get(&url).set("User-Agent", "RustTalk-Server-Updater").set("Accept", "application/vnd.github+json").call();
    let json: serde_json::Value = match resp {
        Ok(r) => r.into_json().context("Invalid answer from GitHub")?,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(code, _)) => bail!("GitHub returned HTTP {code}"),
        Err(e) => bail!("GitHub not reachable: {e}"),
    };
    Ok(parse_release(&json, asset).filter(|r| is_newer(&r.version, VERSION)))
}

/// What admins see in the client when an update is waiting.
pub fn notice(release: &Release) -> String {
    format!("Server update available: {VERSION} -> {}. Install it on the server with: sudo rusttalk-server update", release.version)
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = agent().get(url).set("User-Agent", "RustTalk-Server-Updater").call().map_err(|e| anyhow::anyhow!("Download failed: {e}"))?;
    let mut buf = Vec::new();
    resp.into_reader().take(MAX_DOWNLOAD + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_DOWNLOAD {
        bail!("Download too large");
    }
    Ok(buf)
}

/// The download is a program for this platform (not an HTML error page etc.).
fn looks_like_program(data: &[u8]) -> bool {
    if cfg!(windows) {
        data.starts_with(b"MZ")
    } else {
        data.starts_with(b"\x7fELF")
    }
}

/// Checks the bytes and puts them in place of `target`: written next to it,
/// test-started with `--version`, then swapped (the old one stays as `.old`).
pub fn install_binary(data: &[u8], want_sha: &str, target: &Path) -> Result<PathBuf> {
    if sha256_hex(data) != want_sha {
        bail!("Checksum mismatch - update not installed");
    }
    if !looks_like_program(data) {
        bail!("The download is not a server program for this system");
    }
    let file_name = target.file_name().and_then(|n| n.to_str()).context("Unknown program path")?;
    let new = target.with_file_name(format!("{file_name}.new"));
    let old = target.with_file_name(format!("{file_name}.old"));
    std::fs::write(&new, data).with_context(|| format!("Could not write {} (run the update with sudo / as administrator)", new.display()))?;
    make_executable(&new)?;
    let ran = std::process::Command::new(&new).arg("--version").output();
    match ran {
        Ok(out) if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("rusttalk-server") => {}
        _ => {
            let _ = std::fs::remove_file(&new);
            bail!("The new version does not start on this system - update not installed");
        }
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(target, &old).context("Could not move the current version aside")?;
    if let Err(e) = std::fs::rename(&new, target) {
        let _ = std::fs::rename(&old, target); // roll back
        bail!("Could not install the update: {e}");
    }
    Ok(old)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).context("chmod")
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Restarts the systemd service if it is running (Linux). Returns what happened.
fn restart_service() -> String {
    if !cfg!(target_os = "linux") {
        return "Restart the server to use the new version.".to_string();
    }
    let active = std::process::Command::new("systemctl").args(["is-active", "--quiet", SERVICE]).status().is_ok_and(|s| s.success());
    if !active {
        return "Restart the server to use the new version.".to_string();
    }
    match std::process::Command::new("systemctl").args(["restart", SERVICE]).status() {
        Ok(s) if s.success() => format!("Service {SERVICE} restarted."),
        _ => format!("Could not restart {SERVICE} - run: sudo systemctl restart {SERVICE}"),
    }
}

/// `rusttalk-server update [--check] [--no-restart]`. Returns the exit code.
pub fn run_cli(args: &[String]) -> i32 {
    let only_check = args.iter().any(|a| a == "--check");
    let no_restart = args.iter().any(|a| a == "--no-restart");
    if asset_name().is_none() {
        eprintln!("No server builds for this system in the releases - update by building from source.");
        return 1;
    }
    let release = match check() {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("rusttalk-server {VERSION} is up to date.");
            return 0;
        }
        Err(e) => {
            eprintln!("Update check failed: {e:#}");
            return 1;
        }
    };
    println!("New version available: {VERSION} -> {} ({})", release.version, release.page_url);
    if only_check {
        return 0;
    }
    let result = (|| -> Result<PathBuf> {
        let sha_url = release.sha_url.as_deref().context("The release has no checksum file - update not installed")?;
        let want = parse_sha_file(&String::from_utf8_lossy(&download(sha_url)?)).context("Invalid checksum file in the release")?;
        println!("Downloading {} ...", release.bin_url);
        let data = download(&release.bin_url)?;
        let target = std::env::current_exe().context("Own program path unknown")?;
        install_binary(&data, &want, &target)
    })();
    match result {
        Ok(old) => {
            println!("Installed {} (previous version kept as {}).", release.version, old.display());
            if no_restart {
                println!("Restart the server to use the new version.");
            } else {
                println!("{}", restart_service());
            }
            0
        }
        Err(e) => {
            eprintln!("Update failed: {e:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_needs_the_platform_binary() {
        let json = serde_json::json!({
            "tag_name": "v0.7.0",
            "html_url": "https://github.com/x/releases/v0.7.0",
            "assets": [
                { "name": "rusttalk-windows.exe", "browser_download_url": "https://dl/client" },
                { "name": "rusttalk-server-linux-x86_64", "browser_download_url": "https://dl/linux" },
                { "name": "rusttalk-server-linux-x86_64.sha256", "browser_download_url": "https://dl/linux.sha" },
                { "name": "rusttalk-server-windows.exe", "browser_download_url": "https://dl/win" }
            ]
        });
        let r = parse_release(&json, "rusttalk-server-linux-x86_64").unwrap();
        assert_eq!(r.version, "0.7.0");
        assert_eq!(r.bin_url, "https://dl/linux");
        assert_eq!(r.sha_url.as_deref(), Some("https://dl/linux.sha"));
        let w = parse_release(&json, "rusttalk-server-windows.exe").unwrap();
        assert_eq!(w.sha_url, None, "no checksum -> the CLI refuses it");
        // Releases before the server updater only had the tarball.
        let old = serde_json::json!({ "tag_name": "v0.6.1", "assets": [{ "name": "rusttalk-server-linux-x86_64.tar.gz", "browser_download_url": "x" }] });
        assert!(parse_release(&old, "rusttalk-server-linux-x86_64").is_none());
        assert!(notice(&r).contains("0.7.0") && notice(&r).contains("rusttalk-server update"));
    }

    #[test]
    fn install_checks_before_it_swaps() {
        let dir = std::env::temp_dir().join(format!("rusttalk-srv-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join(if cfg!(windows) { "rusttalk-server.exe" } else { "rusttalk-server" });
        std::fs::write(&target, b"current").unwrap();

        // Wrong checksum, not a program, a program that does not run: the current file stays.
        let junk = b"<html>rate limited</html>".to_vec();
        assert!(install_binary(&junk, &"0".repeat(64), &target).unwrap_err().to_string().contains("Checksum"));
        assert!(install_binary(&junk, &sha256_hex(&junk), &target).unwrap_err().to_string().contains("not a server program"));
        let fake = [if cfg!(windows) { &b"MZ"[..] } else { &b"\x7fELF"[..] }, &b"broken"[..]].concat();
        assert!(install_binary(&fake, &sha256_hex(&fake), &target).unwrap_err().to_string().contains("does not start"));
        assert_eq!(std::fs::read(&target).unwrap(), b"current");
        assert!(!target.with_file_name(format!("{}.new", target.file_name().unwrap().to_str().unwrap())).exists(), "no leftover");

        // A real server binary (this test's own build of it, if there is one) is swapped in.
        let built = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(target.file_name().unwrap());
        if let Ok(real) = std::fs::read(&built) {
            let old = install_binary(&real, &sha256_hex(&real), &target).unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), real);
            assert_eq!(std::fs::read(&old).unwrap(), b"current", "previous version kept");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
