//! Bounded GitHub release-list discovery, included by both updater crates.
use std::collections::BTreeMap;
use std::io::Read;

use anyhow::{bail, Context, Result};
use rusttalk_shared::release::{parse_release_tag, Component, REPO};
use serde_json::Value;

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub asset_url: String,
    pub sha_url: Option<String>,
}

#[derive(Default)]
struct Selection {
    tags: BTreeMap<String, Value>,
    components: BTreeMap<(u64, u64, u64), String>,
    best: Option<((u64, u64, u64), bool, Release)>,
}

impl Selection {
    fn add(&mut self, json: &Value, component: Component, asset: &str) -> Result<()> {
        let tag = json["tag_name"]
            .as_str()
            .context("Release tag is not a string")?;
        let draft = json["draft"]
            .as_bool()
            .context("Release draft is not a boolean")?;
        let prerelease = json["prerelease"]
            .as_bool()
            .context("Release prerelease is not a boolean")?;
        let page_url = json["html_url"]
            .as_str()
            .context("Release page URL is not a string")?;
        let notes = match json.get("body") {
            Some(Value::String(text)) => text.as_str(),
            Some(Value::Null) => "",
            _ => bail!("Release body is not a string or null"),
        };
        let assets = json["assets"]
            .as_array()
            .context("Release assets are not an array")?;
        let mut urls = BTreeMap::new();
        for entry in assets {
            let name = entry["name"]
                .as_str()
                .context("Release asset name is not a string")?;
            let url = entry["browser_download_url"]
                .as_str()
                .context("Release asset URL is not a string")?;
            if urls.insert(name, url).is_some() {
                bail!("Duplicate release asset name");
            }
        }
        if draft || prerelease {
            return Ok(());
        }
        let Some(parsed) = parse_release_tag(tag) else {
            return Ok(());
        };
        if parsed.component.is_some_and(|c| c != component) {
            return Ok(());
        }
        // Compare duplicates before asset filtering: missing assets cannot hide ambiguity.
        if let Some(previous) = self.tags.get(tag) {
            if previous != json {
                bail!("Inconsistent duplicate release record");
            }
            return Ok(());
        }
        self.tags.insert(tag.to_owned(), json.clone());
        if parsed.component.is_some() {
            if let Some(previous) = self.components.insert(parsed.core, tag.to_owned()) {
                if previous != tag {
                    bail!("Ambiguous component release version");
                }
            }
        }
        let Some(url) = urls.get(asset) else {
            return Ok(());
        };
        let rank = (parsed.core, parsed.component.is_some());
        if self
            .best
            .as_ref()
            .is_none_or(|(core, component, _)| rank > (*core, *component))
        {
            self.best = Some((
                rank.0,
                rank.1,
                Release {
                    version: tag
                        .strip_prefix('v')
                        .context("Invalid release tag")?
                        .to_owned(),
                    notes: notes.to_owned(),
                    page_url: page_url.to_owned(),
                    asset_url: (*url).to_owned(),
                    sha_url: urls
                        .get(format!("{asset}.sha256").as_str())
                        .map(|s| (*s).to_owned()),
                },
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
pub fn parse_release(json: &Value, component: Component, asset: &str) -> Result<Option<Release>> {
    let mut selection = Selection::default();
    selection.add(json, component, asset)?;
    Ok(selection.best.map(|(_, _, release)| release))
}

fn read_page(reader: impl Read) -> Result<Vec<Value>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_PAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Could not read release list")?;
    if bytes.len() as u64 > MAX_PAGE_BYTES {
        bail!("Release list page exceeds 2 MiB");
    }
    let records: Vec<Value> =
        serde_json::from_slice(&bytes).context("Invalid release list from GitHub")?;
    if records.len() > PAGE_SIZE {
        bail!("Release list page exceeds 100 records");
    }
    Ok(records)
}

/// Only a short page proves completion; never trust order or remote pagination URLs.
pub fn discover(
    agent: &ureq::Agent,
    base: &str,
    user_agent: &str,
    component: Component,
    asset: &str,
) -> Result<Option<Release>> {
    let mut selection = Selection::default();
    for page in 1..=MAX_PAGES {
        let url = format!(
            "{}/repos/{REPO}/releases?per_page={PAGE_SIZE}&page={page}",
            base.trim_end_matches('/')
        );
        let response = agent
            .get(&url)
            .set("User-Agent", user_agent)
            .set("Accept", "application/vnd.github+json")
            .call();
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, _)) => bail!("GitHub returned HTTP {code}"),
            Err(error) => bail!("GitHub not reachable: {error}"),
        };
        let records = read_page(response.into_reader())?;
        for record in &records {
            selection.add(record, component, asset)?;
        }
        if records.len() < PAGE_SIZE {
            return Ok(selection.best.map(|(_, _, release)| release));
        }
    }
    bail!("Release list incomplete after 10 pages")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    fn release(tag: &str, asset: &str) -> Value {
        json!({"tag_name": tag, "draft": false, "prerelease": false,
            "html_url": "https://example.invalid/release", "body": null,
            "assets": [{"name": asset, "browser_download_url": format!("https://example.invalid/{tag}")},
                {"name": format!("{asset}.sha256"), "browser_download_url": "https://example.invalid/sha"}]})
    }

    fn select(records: &[Value]) -> Result<Option<Release>> {
        let mut selection = Selection::default();
        for record in records {
            selection.add(record, Component::Client, "client")?;
        }
        Ok(selection.best.map(|(_, _, release)| release))
    }

    #[test]
    fn components_numeric_order_legacy_and_exact_assets() {
        let mut records = vec![
            release("v1.9.9+client.abcdef1", "client"),
            release("v99.0.0+server.abcdef1", "client"),
            release("v99.0.0+index-server.abcdef1", "client"),
            release("v1.10.0", "client"),
            release("v2.0.0+client.abcdef1", "CLIENT"),
            release("v1.10.0+client.abcdef1", "client"),
        ];
        assert_eq!(
            select(&records).unwrap().unwrap().version,
            "1.10.0+client.abcdef1"
        );
        records.reverse();
        assert_eq!(
            select(&records).unwrap().unwrap().version,
            "1.10.0+client.abcdef1"
        );
        assert_eq!(
            select(&[release("v2.0.0", "client")])
                .unwrap()
                .unwrap()
                .version,
            "2.0.0"
        );
        let mut draft = release("v99.0.0+client.abcdef1", "client");
        draft["draft"] = json!(true);
        let mut prerelease = release("v99.0.0+client.abcdef2", "client");
        prerelease["prerelease"] = json!(true);
        assert!(select(&[
            draft,
            prerelease,
            release("v3.0.0-rc.1", "client"),
            release("v03.0.0", "client"),
            release("v3.0.0+client.ABCDEF1", "client")
        ])
        .unwrap()
        .is_none());
    }

    #[test]
    fn server_selection_rejects_forged_client_and_index_assets() {
        let mut selection = Selection::default();
        for record in [
            release("v10.0.0+client.abcdef1", "server"),
            release("v10.0.0+index-server.abcdef1", "server"),
            release("v1.9.0", "server"),
            release("v1.10.0+server.abcdef1", "server"),
        ] {
            selection.add(&record, Component::Server, "server").unwrap();
        }
        assert_eq!(selection.best.unwrap().2.version, "1.10.0+server.abcdef1");
    }

    #[test]
    fn ambiguity_and_duplicate_policy_precede_asset_filtering() {
        let one = release("v2.0.0+client.abcdef1", "client");
        assert!(select(&[one.clone(), one.clone()]).unwrap().is_some());
        let missing = release("v2.0.0+client.abcdef2", "other");
        for records in [
            vec![one.clone(), missing.clone()],
            vec![missing, one.clone()],
        ] {
            assert!(select(&records)
                .unwrap_err()
                .to_string()
                .contains("Ambiguous"));
        }
        assert!(select(&[one.clone(), release("v2.0.0+client.abcdef10", "other")]).is_err());
        let mut inconsistent = one.clone();
        inconsistent["assets"] = json!([]);
        assert!(select(&[one, inconsistent])
            .unwrap_err()
            .to_string()
            .contains("Inconsistent"));
    }

    #[test]
    fn malformed_records_fail_closed_and_checksum_stays_optional_until_install() {
        let valid = release("v1.0.0", "client");
        for (field, value) in [
            ("tag_name", json!(3)),
            ("draft", Value::Null),
            ("prerelease", json!("false")),
            ("html_url", json!(true)),
            ("body", json!([])),
            ("assets", json!({})),
        ] {
            let mut malformed = valid.clone();
            malformed[field] = value;
            assert!(select(&[malformed]).is_err(), "{field}");
        }
        let mut malformed = valid.clone();
        malformed["assets"][0]["browser_download_url"] = Value::Null;
        assert!(select(&[malformed]).is_err());
        let mut duplicate_asset = valid.clone();
        duplicate_asset["assets"]
            .as_array_mut()
            .unwrap()
            .push(valid["assets"][0].clone());
        assert!(select(&[duplicate_asset]).is_err());
        let mut no_checksum = valid;
        no_checksum["assets"].as_array_mut().unwrap().pop();
        assert!(select(&[no_checksum]).unwrap().unwrap().sha_url.is_none());
    }

    #[test]
    fn page_shape_record_count_and_streamed_byte_limit() {
        for bytes in [b"null".as_slice(), b"{}", b"[", b"[null]"] {
            if let Ok(records) = read_page(bytes) {
                assert!(select(&records).is_err());
            }
        }
        assert!(read_page(
            serde_json::to_vec(&vec![Value::Null; 101])
                .unwrap()
                .as_slice()
        )
        .is_err());
        struct Endless(usize);
        impl Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                buffer.fill(b' ');
                self.0 += buffer.len();
                Ok(buffer.len())
            }
        }
        let mut stream = Endless(0);
        assert!(read_page(&mut stream)
            .unwrap_err()
            .to_string()
            .contains("2 MiB"));
        assert_eq!(stream.0, MAX_PAGE_BYTES as usize + 1);
    }

    fn http(pages: Vec<(u16, Vec<u8>)>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            for (index, (status, body)) in pages.into_iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(
                    line,
                    format!(
                        "GET /repos/{REPO}/releases?per_page=100&page={} HTTP/1.1\r\n",
                        index + 1
                    )
                );
                // No Content-Length: the cap must apply to the actual stream.
                write!(stream, "HTTP/1.1 {status} Test\r\nConnection: close\r\nLink: <http://untrusted.invalid/>; rel=\"next\"\r\n\r\n").unwrap();
                let _ = stream.write_all(&body);
            }
        });
        (base, handle)
    }

    fn discover_pages(pages: Vec<(u16, Vec<u8>)>) -> Result<Option<Release>> {
        let (base, handle) = http(pages);
        let result = discover(
            &ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(2))
                .build(),
            &base,
            "test",
            Component::Client,
            "client",
        );
        handle.join().unwrap();
        result
    }

    fn page(records: Vec<Value>) -> (u16, Vec<u8>) {
        (200, serde_json::to_vec(&records).unwrap())
    }

    #[test]
    fn pagination_continues_after_candidate_and_detects_late_ambiguity() {
        assert!(discover_pages(vec![page(vec![])]).unwrap().is_none());
        let first = vec![release("v1.0.0+client.abcdef1", "client"); 100];
        let highest = release("v10.0.0+client.abcdef1", "client");
        assert_eq!(
            discover_pages(vec![page(first.clone()), page(vec![highest])])
                .unwrap()
                .unwrap()
                .version,
            "10.0.0+client.abcdef1"
        );
        assert!(discover_pages(vec![
            page(first.clone()),
            page(vec![release("v1.0.0+client.abcdef2", "other")])
        ])
        .is_err());
        assert!(discover_pages(vec![page(first.clone()), (500, b"failure".to_vec())]).is_err());
        assert!(discover_pages(vec![page(first.clone()), (404, b"missing".to_vec())]).is_err());
        assert!(discover_pages(vec![page(first.clone()), page(vec![])])
            .unwrap()
            .is_some());
        assert!(discover_pages(vec![page(first.clone()); 10])
            .unwrap_err()
            .to_string()
            .contains("incomplete"));
        assert!(discover_pages(vec![(200, vec![b' '; MAX_PAGE_BYTES as usize + 100])]).is_err());
        assert!(discover_pages(vec![(200, b"{}".to_vec())]).is_err());
        assert!(discover_pages(vec![page(vec![Value::Null; 101])]).is_err());
    }

    #[test]
    fn read_timeout_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(150));
        });
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_millis(30))
            .build();
        assert!(discover(&agent, &base, "test", Component::Server, "server").is_err());
        handle.join().unwrap();
    }
}
