//! Medien-Helfer: Bild-Decoding zu egui-Textures + GIPHY-Suche/Download.

use std::collections::HashMap;

use eframe::egui;

/// Texture-Cache fuer Chat-Bilder, Video-Frames und GIF-Vorschauen.
/// Schluessel: FNV-1a-Hash der Bytes (bzw. der URL).
#[derive(Default)]
pub struct TextureCache {
    map: HashMap<u64, egui::TextureHandle>,
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
}

/// Ein GIPHY-Suchergebnis.
#[derive(Debug, Clone)]
pub struct GifResult {
    pub id: String,
    pub preview_url: String,
    pub full_url: String,
}

/// GIPHY-Suche (Free-API; Key von developers.giphy.com).
pub fn giphy_search(api_key: &str, query: &str) -> anyhow::Result<Vec<GifResult>> {
    let resp: serde_json::Value = ureq::get("https://api.giphy.com/v1/gifs/search")
        .query("api_key", api_key)
        .query("q", query)
        .query("limit", "12")
        .query("rating", "pg-13")
        .call()?
        .into_json()?;
    let mut out = Vec::new();
    if let Some(arr) = resp.get("data").and_then(|d| d.as_array()) {
        for g in arr {
            let id = g["id"].as_str().unwrap_or_default().to_string();
            let preview_url =
                g["images"]["fixed_height_small"]["url"].as_str().unwrap_or_default().to_string();
            let full_url = g["images"]["original"]["url"].as_str().unwrap_or_default().to_string();
            if !full_url.is_empty() {
                out.push(GifResult { id, preview_url, full_url });
            }
        }
    }
    Ok(out)
}

/// Laedt Bytes von einer URL (GIF-Original, Vorschau).
pub fn download(url: &str) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let resp = ureq::get(url).call()?;
    let mut buf = Vec::new();
    resp.into_reader().take(6 * 1024 * 1024).read_to_end(&mut buf)?;
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
}
