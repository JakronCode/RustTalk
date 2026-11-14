//! Shared bits of the client and server updaters: GitHub repository,
//! version comparison and the `.sha256` files next to release assets.

use sha2::{Digest, Sha256};

/// Repository whose GitHub releases carry the updates.
pub const REPO: &str = "rusttalkcrew/RustTalk";

/// "v0.4.0" / "0.4.0" -> (0, 4, 0). Pre-release suffixes are ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

/// Stable release channels. Metadata identifies a component, never version order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Client,
    Server,
    IndexServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseTag<'a> {
    pub core: (u64, u64, u64),
    pub component: Option<Component>,
    pub commit_prefix: Option<&'a str>,
}

/// Accept only canonical stable legacy or component release tags.
pub fn parse_release_tag(tag: &str) -> Option<ReleaseTag<'_>> {
    let version = tag.strip_prefix('v')?;
    let (core, metadata) = match version.split_once('+') {
        Some((core, metadata)) => (core, Some(metadata)),
        None => (version, None),
    };
    let mut numbers = core.split('.');
    let mut number = || {
        let text = numbers.next()?;
        if text.is_empty()
            || (text.len() > 1 && text.starts_with('0'))
            || !text.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        text.parse::<u64>().ok()
    };
    let core = (number()?, number()?, number()?);
    if numbers.next().is_some() {
        return None;
    }
    let (component, commit_prefix) = if let Some(metadata) = metadata {
        let (component, hash) = metadata.split_once('.')?;
        let component = match component {
            "client" => Component::Client,
            "server" => Component::Server,
            "index-server" => Component::IndexServer,
            _ => return None,
        };
        if !(7..=40).contains(&hash.len())
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        (Some(component), Some(hash))
    } else {
        (None, None)
    };
    Some(ReleaseTag {
        core,
        component,
        commit_prefix,
    })
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
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
        assert!(!is_newer("v0.4.0+client.abcdef1", "0.4.0"));
        assert!(is_newer("v0.5.0+server.abcdef1", "0.4.0"));
    }

    #[test]
    fn strict_release_tags() {
        for tag in [
            "v0.0.0",
            "v12.10.3+client.abcdef1",
            "v1.2.3+server.0123456789abcdef",
            "v1.2.3+index-server.abcdef1",
        ] {
            assert!(parse_release_tag(tag).is_some(), "{tag}");
        }
        for tag in [
            "1.2.3",
            "V1.2.3",
            " v1.2.3",
            "v1.2.3\n",
            "v01.2.3",
            "v1.02.3",
            "v1.2.03",
            "v1.2",
            "v1.2.3.4",
            "v1.2.3-rc.1",
            "v18446744073709551616.0.0",
            "v1.2.3+client.abcdef",
            "v1.2.3+client.ABCDEF1",
            "v1.2.3+unknown.abcdef1",
            "v1.2.3+client.abcdef1.junk",
            "v1.2.3+server.abcdef1+foo",
        ] {
            assert!(parse_release_tag(tag).is_none(), "{tag}");
        }
        assert!(parse_release_tag(&format!("v1.2.3+client.{}", "a".repeat(41))).is_none());
        for tag in ["v1.18446744073709551616.0", "v1.0.18446744073709551616"] {
            assert!(parse_release_tag(tag).is_none());
        }
        assert!(parse_release_tag(&format!(
            "v18446744073709551615.0.0+client.{}",
            "a".repeat(40)
        ))
        .is_some());
        assert_eq!(
            parse_release_tag("v1.2.3+client.abcdef1").unwrap().core,
            (1, 2, 3)
        );
    }

    #[test]
    fn checksums() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let line = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  rusttalk-windows.exe\n";
        assert_eq!(
            parse_sha_file(line).as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(parse_sha_file("nothex  x"), None);
        assert_eq!(parse_sha_file(""), None);
    }
}
