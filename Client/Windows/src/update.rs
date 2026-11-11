//! Updates from the GitHub releases of the repository.
//!
//! A release carries `rusttalk-windows.exe` (+ `rusttalk-windows.exe.sha256`).
//! The installed app checks the latest release on start; installing an
//! update downloads the exe, checks the SHA-256, swaps the files (a running
//! exe can be renamed, not overwritten) and restarts.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub const REPO: &str = "JakronCode/RustTalk";
pub const ASSET: &str = "rusttalk-windows.exe";
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

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

/// "v0.4.0" / "0.4.0" -> (0, 4, 0). Pre-release suffixes are ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    matches!((parse_version(remote), parse_version(current)), (Some(r), Some(c)) if r > c)
}

/// Reads a `releases/latest` response. `None` if there is no exe asset.
pub fn parse_release(json: &serde_json::Value) -> Option<Release> {
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
        notes: json["body"].as_str().unwrap_or_default().to_string(),
        page_url: json["html_url"].as_str().unwrap_or_default().to_string(),
        exe_url: url_of(ASSET)?,
        sha_url: url_of(&format!("{ASSET}.sha256")),
    })
}

/// API base (tests can point it at a local fake).
fn api_base() -> String {
    std::env::var("RUSTTALK_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(60)).build()
}

/// Asks GitHub for the latest release. `Ok(None)` = up to date.
pub fn check() -> Result<Option<Release>> {
    let url = format!("{}/repos/{REPO}/releases/latest", api_base());
    let resp = agent()
        .get(&url)
        // GitHub requires a User-Agent; a generic one, nothing about the user.
        .set("User-Agent", "RustTalk-Updater")
        .set("Accept", "application/vnd.github+json")
        .call();
    let json: serde_json::Value = match resp {
        Ok(r) => r.into_json().context("Invalid answer from GitHub")?,
        Err(ureq::Error::Status(404, _)) => return Ok(None), // no release yet
        Err(ureq::Error::Status(code, _)) => bail!("GitHub returned HTTP {code}"),
        Err(e) => bail!("GitHub not reachable: {e}"),
    };
    Ok(parse_release(&json).filter(|r| is_newer(&r.version, env!("CARGO_PKG_VERSION"))))
}

/// SHA-256 file content ("<hex>  <name>") -> lowercase hex.
pub fn parse_sha_file(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?.to_lowercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn download(url: &str, progress: Option<&AtomicU32>) -> Result<Vec<u8>> {
    let resp = agent().get(url).set("User-Agent", "RustTalk-Updater").call().map_err(|e| anyhow::anyhow!("Download failed: {e}"))?;
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
            p.store(((buf.len() as u64 * 100) / t.max(1)).min(100) as u32, Ordering::Relaxed);
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
    let dir = crate::install::install_dir().context("Install folder unknown")?;
    let exe = download(&release.exe_url, Some(progress))?;
    if !exe.starts_with(b"MZ") {
        bail!("The download is not a Windows program");
    }
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
    progress.store(100, Ordering::Relaxed);
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("v2.0.1-beta"), Some((2, 0, 1)));
        assert_eq!(parse_version("abc"), None);
        assert!(is_newer("v0.4.0", "0.3.0"));
        assert!(is_newer("0.10.0", "0.9.9"), "numeric, not text compare");
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.9", "0.3.0"));
        assert!(!is_newer("garbage", "0.3.0"));
    }

    #[test]
    fn release_parsing_needs_the_exe() {
        let json = serde_json::json!({
            "tag_name": "v0.4.0",
            "html_url": "https://github.com/x/releases/v0.4.0",
            "body": "Changes",
            "assets": [
                { "name": "rusttalk-v0.4.0.zip", "browser_download_url": "https://dl/zip" },
                { "name": "rusttalk-windows.exe", "browser_download_url": "https://dl/exe" },
                { "name": "rusttalk-windows.exe.sha256", "browser_download_url": "https://dl/sha" }
            ]
        });
        let r = parse_release(&json).unwrap();
        assert_eq!(r.version, "0.4.0");
        assert_eq!(r.exe_url, "https://dl/exe");
        assert_eq!(r.sha_url.as_deref(), Some("https://dl/sha"));
        // Old releases (zip only) cannot be installed automatically.
        let old = serde_json::json!({ "tag_name": "v0.3.0", "assets": [{ "name": "rusttalk-v0.3.0.zip", "browser_download_url": "x" }] });
        assert!(parse_release(&old).is_none());
    }

    #[test]
    fn checksums() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let line = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  rusttalk-windows.exe\n";
        assert_eq!(parse_sha_file(line).as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert_eq!(parse_sha_file("nothex  x"), None);
        assert_eq!(parse_sha_file(""), None);
    }
}
