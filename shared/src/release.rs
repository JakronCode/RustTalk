//! Shared bits of the client and server updaters: GitHub repository,
//! version comparison and the `.sha256` files next to release assets.

use sha2::{Digest, Sha256};

/// Repository whose GitHub releases carry the updates.
pub const REPO: &str = "JakronCode/RustTalk";

/// "v0.4.0" / "0.4.0" -> (0, 4, 0). Pre-release suffixes are ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

/// `remote` is a higher version than `current` (numeric, not text compare).
pub fn is_newer(remote: &str, current: &str) -> bool {
    matches!((parse_version(remote), parse_version(current)), (Some(r), Some(c)) if r > c)
}

/// SHA-256 file content ("<hex>  <name>") -> lowercase hex.
pub fn parse_sha_file(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?.to_lowercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
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
    fn checksums() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let line = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  rusttalk-windows.exe\n";
        assert_eq!(parse_sha_file(line).as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert_eq!(parse_sha_file("nothex  x"), None);
        assert_eq!(parse_sha_file(""), None);
    }
}
