//! Medien-Helfer: Bild-Decoding zu egui-Textures (inkl. animierter GIFs)
//! + GIPHY-Suche/Download.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use rusttalk_shared::protocol::MAX_MEDIA_BYTES;

/// Texture-Cache fuer Chat-Bilder, Video-Frames und GIF-Vorschauen.
/// Schluessel: FNV-1a-Hash der Bytes.
#[derive(Default)]
pub struct TextureCache {
    map: HashMap<u64, egui::TextureHandle>,
    /// Decoded GIF animations (`None` = not animated, use the still image).
    anims: HashMap<u64, Option<Arc<Animation>>>,
}

/// Decoded animation: one texture per frame + when each frame ends (s).
pub struct Animation {
    frames: Vec<(egui::TextureHandle, f32)>,
    total: f32,
}

impl Animation {
    /// Frame at time `t` (seconds, looping) and how long it stays visible.
    fn frame_at(&self, t: f64) -> (&egui::TextureHandle, f32) {
        let pos = (t % self.total as f64) as f32;
        let idx = self.frames.iter().position(|(_, end)| pos < *end).unwrap_or(self.frames.len() - 1);
        let (tex, end) = &self.frames[idx];
        (tex, (end - pos).max(0.01))
    }
}

/// Limits for decoding GIF animations (memory: frames * w * h * 4 bytes).
const MAX_ANIM_FRAMES: usize = 200;
const MAX_ANIM_PIXELS: usize = 60_000_000;

/// Frame delay like browsers: <= 10 ms counts as 100 ms.
fn frame_delay_secs(numer_ms: u32, denom: u32) -> f32 {
    let ms = numer_ms as f32 / denom.max(1) as f32;
    if ms <= 10.0 {
        0.1
    } else {
        ms / 1000.0
    }
}

/// GIF magic bytes.
pub fn is_gif(data: &[u8]) -> bool {
    data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")
}

/// FNV-1a (kein extra Crate noetig).
pub fn hash_bytes(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl TextureCache {
    /// Dekodiert die Bytes (PNG/JPEG/GIF-Standbild) und cached die Textur.
    pub fn texture_for(
        &mut self,
        ctx: &egui::Context,
        data: &[u8],
        name: &str,
    ) -> Option<egui::TextureHandle> {
        let key = hash_bytes(data);
        if let Some(t) = self.map.get(&key) {
            return Some(t.clone());
        }
        let img = image::load_from_memory(data).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let tex = ctx.load_texture(
            format!("{name}-{key:016x}"),
            egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba),
            egui::TextureOptions::LINEAR,
        );
        self.map.insert(key, tex.clone());
        Some(tex)
    }

    /// Entfernt eine Textur (z. B. wenn ein User aufhoert zu teilen).
    pub fn remove_bytes(&mut self, data: &[u8]) {
        self.map.remove(&hash_bytes(data));
    }

    /// Like [`texture_for`](Self::texture_for), but plays animated GIFs when
    /// `animate` is set (schedules the repaint for the next frame).
    pub fn image_for(
        &mut self,
        ctx: &egui::Context,
        data: &[u8],
        name: &str,
        animate: bool,
    ) -> Option<egui::TextureHandle> {
        if animate && is_gif(data) {
            let key = hash_bytes(data);
            let anim = self
                .anims
                .entry(key)
                .or_insert_with(|| decode_animation(ctx, data, name).map(Arc::new))
                .clone();
            if let Some(anim) = anim {
                let (tex, remaining) = anim.frame_at(ctx.input(|i| i.time));
                ctx.request_repaint_after(std::time::Duration::from_secs_f32(remaining));
                return Some(tex.clone());
            }
        }
        self.texture_for(ctx, data, name)
    }
}

/// Decodes all frames of a GIF. `None` for stills, broken or oversized files.
fn decode_animation(ctx: &egui::Context, data: &[u8], name: &str) -> Option<Animation> {
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(data)).ok()?;
    let mut frames = Vec::new();
    let mut end = 0.0f32;
    let mut pixels = 0usize;
    for (i, frame) in decoder.into_frames().enumerate() {
        let frame = frame.ok()?;
        if i >= MAX_ANIM_FRAMES {
            break;
        }
        let (numer, denom) = frame.delay().numer_denom_ms();
        let buf = frame.into_buffer();
        let (w, h) = (buf.width() as usize, buf.height() as usize);
        pixels += w * h;
        if pixels > MAX_ANIM_PIXELS {
            break;
        }
        end += frame_delay_secs(numer, denom);
        let tex = ctx.load_texture(
            format!("{name}-anim-{:016x}-{i}", hash_bytes(data)),
            egui::ColorImage::from_rgba_unmultiplied([w, h], &buf),
            egui::TextureOptions::LINEAR,
        );
        frames.push((tex, end));
    }
    (frames.len() > 1).then_some(Animation { frames, total: end })
}

/// Ein GIPHY-Suchergebnis.
#[derive(Debug, Clone, PartialEq)]
pub struct GifResult {
    pub id: String,
    /// Small animated preview for the picker grid.
    pub preview_url: String,
    /// What gets attached: `downsized` (GIPHY keeps it below ~2 MB), else original.
    pub full_url: String,
}

/// GIPHY-Suche (Free-API; Key von developers.giphy.com).
pub fn giphy_search(api_key: &str, query: &str) -> anyhow::Result<Vec<GifResult>> {
    let result = ureq::get("https://api.giphy.com/v1/gifs/search")
        .query("api_key", api_key)
        .query("q", query)
        .query("limit", "18")
        .query("rating", "pg-13")
        .call();
    let resp = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(401 | 403, _)) => anyhow::bail!("GIPHY rejected the API key (check Settings > Advanced)"),
        Err(ureq::Error::Status(429, _)) => anyhow::bail!("GIPHY rate limit reached - try again later"),
        Err(ureq::Error::Status(code, _)) => anyhow::bail!("GIPHY returned HTTP {code}"),
        // Transport errors print the URL - which contains the API key.
        Err(e) => anyhow::bail!("{}", e.to_string().replace(api_key, "***")),
    };
    let json: serde_json::Value = resp.into_json()?;
    Ok(parse_search(&json))
}

/// Extracts the results from a GIPHY search response.
pub fn parse_search(json: &serde_json::Value) -> Vec<GifResult> {
    let url = |g: &serde_json::Value, rendition: &str| {
        g["images"][rendition]["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string)
    };
    let Some(arr) = json.get("data").and_then(|d| d.as_array()) else { return Vec::new() };
    arr.iter()
        .filter_map(|g| {
            let full_url = url(g, "downsized").or_else(|| url(g, "original"))?;
            let preview_url = url(g, "fixed_height_small").unwrap_or_else(|| full_url.clone());
            Some(GifResult { id: g["id"].as_str().unwrap_or_default().to_string(), preview_url, full_url })
        })
        .collect()
}

/// Laedt Bytes von einer URL (GIF, Vorschau). Fails above `MAX_MEDIA_BYTES`,
/// the most the server accepts for a chat image.
pub fn download(url: &str) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let resp = ureq::get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => anyhow::anyhow!("Download failed (HTTP {code})"),
        other => anyhow::anyhow!("Download failed: {other}"),
    })?;
    let mut buf = Vec::new();
    resp.into_reader().take(MAX_MEDIA_BYTES as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > MAX_MEDIA_BYTES {
        anyhow::bail!("GIF too large (max. {} MiB)", MAX_MEDIA_BYTES / 1024 / 1024);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_bytes_is_deterministic() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
    }

    #[test]
    fn texture_cache_decodes_and_caches() {
        // Minimales gueltiges 1x1-PNG erzeugen und dekodieren.
        let mut png = Vec::new();
        let img = image::ImageBuffer::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let ctx = egui::Context::default();
        let mut cache = TextureCache::default();
        let t1 = cache.texture_for(&ctx, &png, "test");
        assert!(t1.is_some(), "PNG dekodiert");
        let t2 = cache.texture_for(&ctx, &png, "test");
        assert!(t2.is_some());
        assert_eq!(t1.unwrap().id(), t2.unwrap().id(), "Cache-Treffer");
        // Muell gibt None statt Panic:
        assert!(cache.texture_for(&ctx, b"kein bild", "test").is_none());
    }

    #[test]
    fn giphy_parse_prefers_downsized() {
        let json = serde_json::json!({
            "data": [
                { "id": "a", "images": {
                    "fixed_height_small": { "url": "https://x/a-small.gif" },
                    "downsized": { "url": "https://x/a-down.gif" },
                    "original": { "url": "https://x/a-orig.gif" } } },
                { "id": "b", "images": { "original": { "url": "https://x/b-orig.gif" } } },
                { "id": "c", "images": {} }
            ]
        });
        let r = parse_search(&json);
        assert_eq!(r.len(), 2, "entries without any URL are skipped");
        assert_eq!(r[0].full_url, "https://x/a-down.gif");
        assert_eq!(r[0].preview_url, "https://x/a-small.gif");
        assert_eq!(r[1].full_url, "https://x/b-orig.gif", "fallback to original");
        assert_eq!(r[1].preview_url, r[1].full_url, "no preview -> full image");
        assert!(parse_search(&serde_json::json!({ "meta": {} })).is_empty());
    }

    /// Animated 2x2 GIF with `frames` frames.
    fn test_gif(frames: usize, delay_ms: u32) -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..frames {
                let px = image::Rgba([(i * 80) as u8, 0, 0, 255]);
                let buf = image::ImageBuffer::from_pixel(2, 2, px);
                let delay = image::Delay::from_numer_denom_ms(delay_ms, 1);
                enc.encode_frame(image::Frame::from_parts(buf, 0, 0, delay)).unwrap();
            }
        }
        out
    }

    #[test]
    fn gif_animation_decodes_and_loops() {
        let ctx = egui::Context::default();
        let gif = test_gif(3, 100);
        assert!(is_gif(&gif));
        let anim = decode_animation(&ctx, &gif, "t").expect("animated");
        assert_eq!(anim.frames.len(), 3);
        assert!((anim.total - 0.3).abs() < 1e-3);
        let (f0, _) = anim.frame_at(0.05);
        let (f1, rest) = anim.frame_at(0.15);
        let (f0_again, _) = anim.frame_at(0.35);
        assert_ne!(f0.id(), f1.id());
        assert_eq!(f0.id(), f0_again.id(), "loops");
        assert!((rest - 0.05).abs() < 1e-3, "time until the next frame");
        // Single-frame GIF = still image.
        assert!(decode_animation(&ctx, &test_gif(1, 100), "t").is_none());
        // Cache: animated and still both give a texture.
        let mut cache = TextureCache::default();
        assert!(cache.image_for(&ctx, &gif, "t", true).is_some());
        assert!(cache.image_for(&ctx, &gif, "t", false).is_some());
    }

    #[test]
    fn tiny_gif_delays_like_browsers() {
        assert_eq!(frame_delay_secs(0, 1), 0.1);
        assert_eq!(frame_delay_secs(10, 1), 0.1);
        assert!((frame_delay_secs(40, 1) - 0.04).abs() < 1e-6);
    }
}
