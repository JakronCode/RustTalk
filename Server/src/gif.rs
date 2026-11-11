//! GIF search via GIPHY, done by the server: the API key is stored once per
//! server (Server Settings > Integrations) and never sent to clients.
//!
//! Results are cached for a while – the free GIPHY key allows only ~100
//! requests per hour, and the picker opens with the same "trending" page.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rusttalk_shared::protocol::GifInfo;

/// GIFs per page (picker loads more while scrolling).
pub const PAGE_SIZE: u32 = 24;
/// Highest offset we ask for (GIPHY allows up to 4999 for search).
pub const MAX_OFFSET: u32 = 480;
/// How long a result page is reused.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_MAX: usize = 200;
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Cache of result pages: (lowercase query, offset) -> (when, results).
#[derive(Default)]
pub struct GifCache {
    pages: HashMap<(String, u32), (Instant, Vec<GifInfo>)>,
}

impl GifCache {
    pub fn get(&self, query: &str, offset: u32, now: Instant) -> Option<Vec<GifInfo>> {
        self.pages
            .get(&(normalize(query), offset))
            .filter(|(at, _)| now.saturating_duration_since(*at) < CACHE_TTL)
            .map(|(_, r)| r.clone())
    }

    pub fn put(&mut self, query: &str, offset: u32, results: Vec<GifInfo>, now: Instant) {
        self.pages.retain(|_, (at, _)| now.saturating_duration_since(*at) < CACHE_TTL);
        if self.pages.len() >= CACHE_MAX {
            // Drop the oldest page.
            if let Some(oldest) = self.pages.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone()) {
                self.pages.remove(&oldest);
            }
        }
        self.pages.insert((normalize(query), offset), (now, results));
    }

    pub fn clear(&mut self) {
        self.pages.clear();
    }
}

fn normalize(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// One page of trending (empty query) or search results. Blocking HTTP –
/// call it from `spawn_blocking`. Errors never contain the key.
pub fn fetch(key: &str, query: &str, offset: u32) -> Result<Vec<GifInfo>, String> {
    let query = query.trim();
    // Tests can point this at a local fake GIPHY.
    let base = std::env::var("RUSTTALK_GIPHY_API").unwrap_or_else(|_| "https://api.giphy.com/v1/gifs".to_string());
    let url = format!("{}/{}", base.trim_end_matches('/'), if query.is_empty() { "trending" } else { "search" });
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let mut req = agent
        .get(&url)
        .query("api_key", key)
        .query("limit", &PAGE_SIZE.to_string())
        .query("offset", &offset.min(MAX_OFFSET).to_string())
        .query("rating", "pg-13")
        .query("bundle", "messaging_non_clips");
    if !query.is_empty() {
        req = req.query("q", query);
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(401 | 403, _)) => return Err("GIPHY rejected the API key".to_string()),
        Err(ureq::Error::Status(429, _)) => return Err("GIPHY rate limit reached - try again later".to_string()),
        Err(ureq::Error::Status(code, _)) => return Err(format!("GIPHY returned HTTP {code}")),
        // Transport errors print the URL - which contains the key.
        Err(e) => return Err(format!("GIPHY not reachable: {}", e.to_string().replace(key, "***"))),
    };
    let json: serde_json::Value = resp.into_json().map_err(|e| format!("Invalid GIPHY response: {e}"))?;
    Ok(parse(&json))
}

/// Extracts the GIFs from a GIPHY response. Preview = small, low frame rate
/// rendition for the picker; `url` = `downsized` (GIPHY keeps it < ~2 MB).
pub fn parse(json: &serde_json::Value) -> Vec<GifInfo> {
    let Some(arr) = json.get("data").and_then(|d| d.as_array()) else { return Vec::new() };
    let rendition = |g: &serde_json::Value, name: &str| -> Option<(String, u32, u32)> {
        let r = &g["images"][name];
        let url = r["url"].as_str().filter(|u| !u.is_empty())?.to_string();
        // GIPHY sends sizes as strings ("200").
        let num = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64().map(|n| n as u32));
        Some((url, num(&r["width"]).unwrap_or(200), num(&r["height"]).unwrap_or(150)))
    };
    arr.iter()
        .filter_map(|g| {
            let (url, w, h) = rendition(g, "downsized").or_else(|| rendition(g, "original"))?;
            let (preview_url, pw, ph) = rendition(g, "fixed_width_downsampled")
                .or_else(|| rendition(g, "fixed_width"))
                .unwrap_or((url.clone(), w, h));
            Some(GifInfo {
                id: g["id"].as_str().unwrap_or_default().to_string(),
                title: g["title"].as_str().unwrap_or_default().to_string(),
                preview_url,
                preview_width: pw.max(1),
                preview_height: ph.max(1),
                url,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "data": [
                { "id": "a", "title": "Cat GIF", "images": {
                    "fixed_width_downsampled": { "url": "https://x/a-prev.gif", "width": "200", "height": "113" },
                    "downsized": { "url": "https://x/a.gif", "width": "480", "height": "270" },
                    "original": { "url": "https://x/a-orig.gif" } } },
                { "id": "b", "images": { "original": { "url": "https://x/b.gif", "width": 300, "height": 300 } } },
                { "id": "c", "images": {} }
            ]
        })
    }

    #[test]
    fn parse_picks_renditions_and_sizes() {
        let r = parse(&sample());
        assert_eq!(r.len(), 2, "entries without a URL are skipped");
        assert_eq!(r[0].url, "https://x/a.gif");
        assert_eq!(r[0].preview_url, "https://x/a-prev.gif");
        assert_eq!((r[0].preview_width, r[0].preview_height), (200, 113));
        assert_eq!(r[0].title, "Cat GIF");
        assert_eq!(r[1].url, "https://x/b.gif", "fallback to original");
        assert_eq!(r[1].preview_url, r[1].url, "no preview -> full GIF");
        assert_eq!((r[1].preview_width, r[1].preview_height), (300, 300), "numeric sizes too");
        assert!(parse(&serde_json::json!({ "meta": {} })).is_empty());
    }

    #[test]
    fn cache_reuses_pages_until_they_expire() {
        let mut c = GifCache::default();
        let t0 = Instant::now();
        let page = parse(&sample());
        c.put("Funny  Cat", 0, page.clone(), t0);
        assert_eq!(c.get("funny cat", 0, t0), Some(page), "query normalized");
        assert!(c.get("funny cat", 24, t0).is_none(), "other page");
        assert!(c.get("funny cat", 0, t0 + CACHE_TTL + Duration::from_secs(1)).is_none(), "expired");
        for i in 0..(CACHE_MAX as u32 + 10) {
            c.put("q", i, Vec::new(), t0);
        }
        assert!(c.pages.len() <= CACHE_MAX);
    }
}
