//! Explicit local capture selection; never persisted or sent over the network.
//! Display metadata (friendly name, icon) is separate from the exact capture identity.
use std::sync::atomic::AtomicBool;

#[cfg(target_os = "linux")]
pub use super::system_audio::ApplicationSource;
#[cfg(windows)]
pub use super::windows_audio::ApplicationSource;
#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationSource;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AudioShareSource {
    #[default]
    System,
    /// One app, or several mixed into one stream (at most `MAX_MIX`).
    Applications(Vec<ApplicationSource>),
    Separate(Vec<(ApplicationSource, rusttalk_shared::protocol::AudioStreamInfo)>),
}

/// Most applications that can be mixed into one shared stream.
pub const MAX_MIX: usize = 8;

/// Square RGBA picture, always `ICON_PX` wide.
#[derive(Clone, Debug)]
pub struct AppIcon {
    pub rgba: Vec<u8>,
}

pub const ICON_PX: usize = 64;
const MAX_APPLICATIONS: usize = 128;
const MAX_NAME_CHARS: usize = 64;

#[derive(Clone, Debug)]
pub struct AudioApplication {
    /// Friendly app name for display only.
    pub label: String,
    /// Shown only when it tells apart several streams of the same app.
    pub detail: Option<String>,
    pub icon: Option<AppIcon>,
    pub source: ApplicationSource,
}

/// Raw platform discovery result, before display cleanup.
pub struct RawApplication {
    pub app: String,
    pub stream: Option<String>,
    pub icon_hint: Option<String>,
    pub source: ApplicationSource,
}

pub fn applications(stop: &AtomicBool) -> anyhow::Result<Vec<AudioApplication>> {
    #[cfg(target_os = "linux")]
    return super::system_audio::applications(stop);
    #[cfg(windows)]
    return super::windows_audio::applications(stop);
    #[cfg(not(any(windows, target_os = "linux")))]
    anyhow::bail!("Application audio capture is unavailable on this platform")
}

/// "discord" -> "Discord", "Foo_App.exe" -> "Foo App", "PipeWire ALSA [rusttalk]" -> "Rusttalk".
pub fn friendly_name(raw: &str) -> String {
    let mut name = raw.trim();
    if let Some(inner) = name
        .strip_prefix("PipeWire ALSA [")
        .and_then(|n| n.strip_suffix(']'))
    {
        name = inner.trim();
    }
    if let Some(i) = name.find(" [") {
        name = name[..i].trim_end();
    }
    if name.len() > 4
        && name
            .get(name.len() - 4..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".exe"))
    {
        name = &name[..name.len() - 4];
    }
    let mut name: String = name
        .chars()
        .map(|c| if matches!(c, '_' | '-') { ' ' } else { c })
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    name = name.trim().to_string();
    if name.is_empty() {
        return "Application".into();
    }
    if name.chars().all(|c| !c.is_uppercase()) {
        let mut chars = name.chars();
        if let Some(first) = chars.next() {
            name = first.to_uppercase().chain(chars).collect();
        }
    }
    name
}

/// Stream names that repeat the app or carry no information are dropped.
fn stream_detail(label: &str, stream: Option<&str>) -> Option<String> {
    let detail: String = stream?
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    let detail = detail.trim();
    const GENERIC: [&str; 7] = [
        "playback",
        "audio",
        "audio stream",
        "playback stream",
        "alsa playback",
        "output",
        "stream",
    ];
    let lower = detail.to_lowercase();
    (!detail.is_empty() && lower != label.to_lowercase() && !GENERIC.contains(&lower.as_str()))
        .then(|| detail.to_string())
}

/// Same-app entries get unique details so every row stays distinguishable.
fn disambiguate(apps: &mut [AudioApplication]) {
    let word = if cfg!(windows) { "Instance" } else { "Stream" };
    let mut i = 0;
    while i < apps.len() {
        let end = apps[i..]
            .iter()
            .position(|a| a.label != apps[i].label)
            .map_or(apps.len(), |n| i + n);
        if end - i > 1 {
            let details: Vec<Option<String>> =
                apps[i..end].iter().map(|a| a.detail.clone()).collect();
            let mut reserved: std::collections::HashSet<String> =
                details.iter().flatten().cloned().collect();
            for (n, app) in apps[i..end].iter_mut().enumerate() {
                let unique = app.detail.is_some()
                    && details.iter().filter(|d| **d == app.detail).count() == 1;
                if !unique {
                    let mut number = n + 1;
                    let mut generated = format!("{word} {number}");
                    while reserved.contains(&generated) {
                        number += 1;
                        generated = format!("{word} {number}");
                    }
                    reserved.insert(generated.clone());
                    app.detail = Some(generated);
                }
            }
        }
        i = end;
    }
}

/// Turns raw discovery results into bounded, sorted display entries.
pub fn finish(raw: Vec<RawApplication>) -> Vec<AudioApplication> {
    let mut icons: std::collections::HashMap<String, Option<AppIcon>> = Default::default();
    let mut apps: Vec<AudioApplication> = raw
        .into_iter()
        .take(MAX_APPLICATIONS)
        .map(|r| {
            let label = friendly_name(&r.app);
            let icon = r
                .icon_hint
                .filter(|hint| hint.len() <= 64)
                .and_then(|hint| {
                    icons
                        .entry(hint.clone())
                        .or_insert_with(|| load_icon(&hint))
                        .clone()
                });
            AudioApplication {
                detail: stream_detail(&label, r.stream.as_deref()),
                label,
                icon,
                source: r.source,
            }
        })
        .collect();
    apps.sort_by_cached_key(|a| (a.label.to_lowercase(), a.label.clone(), a.detail.clone()));
    disambiguate(&mut apps);
    apps
}

#[cfg(not(target_os = "linux"))]
fn load_icon(_hint: &str) -> Option<AppIcon> {
    None // shortcut: Windows shows a letter tile; exe icon extraction needs shell APIs, add when wanted
}

#[cfg(target_os = "linux")]
fn load_icon(hint: &str) -> Option<AppIcon> {
    let dirs = icon_dirs(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("XDG_DATA_DIRS"),
    );
    icon_candidates(hint, &dirs)
        .into_iter()
        .find_map(|path| read_icon(&path))
}

#[cfg(target_os = "linux")]
fn icon_dirs(
    data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    data_dirs: Option<std::ffi::OsString>,
) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs = Vec::new();
    match (data_home.filter(|v| !v.is_empty()), home) {
        (Some(d), _) => dirs.push(PathBuf::from(d)),
        (None, Some(h)) => dirs.push(PathBuf::from(h).join(".local/share")),
        _ => {}
    }
    let data_dirs = data_dirs
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(std::env::split_paths(&data_dirs));
    dirs.retain(|d| d.is_absolute());
    dirs.truncate(8);
    dirs
}

/// Icon names are plain file stems; anything path-like is refused.
#[cfg(target_os = "linux")]
fn safe_icon_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

/// Only the hicolor fallback theme and pixmaps are searched, PNG only.
#[cfg(target_os = "linux")]
fn icon_candidates(name: &str, dirs: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    const SIZES: [&str; 6] = ["64x64", "48x48", "96x96", "128x128", "256x256", "512x512"];
    if !safe_icon_name(name) {
        return vec![];
    }
    let file = format!("{name}.png");
    let mut paths = Vec::new();
    for dir in dirs {
        for size in SIZES {
            paths.push(
                dir.join("icons/hicolor")
                    .join(size)
                    .join("apps")
                    .join(&file),
            );
        }
        paths.push(dir.join("pixmaps").join(&file));
    }
    paths
}

#[cfg(target_os = "linux")]
fn read_icon(path: &std::path::Path) -> Option<AppIcon> {
    use std::io::Read;
    const MAX_BYTES: u64 = 1 << 20;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
    decode_icon(&bytes)
}

/// Bounded PNG decode: dimension and allocation limits stop decompression bombs.
#[cfg(target_os = "linux")]
fn decode_icon(bytes: &[u8]) -> Option<AppIcon> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(512);
    limits.max_alloc = Some(4 << 20);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    if img.width() != img.height() || img.width() < 16 {
        return None;
    }
    let px = ICON_PX as u32;
    let rgba = image::imageops::resize(
        &img.to_rgba8(),
        px,
        px,
        image::imageops::FilterType::Triangle,
    );
    Some(AppIcon {
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(app: &str, stream: Option<&str>, pid: u32) -> RawApplication {
        RawApplication {
            app: app.into(),
            stream: stream.map(Into::into),
            icon_hint: None,
            source: test_source(pid),
        }
    }

    #[cfg(target_os = "linux")]
    fn test_source(pid: u32) -> ApplicationSource {
        ApplicationSource::for_test(pid)
    }
    #[cfg(not(target_os = "linux"))]
    fn test_source(_: u32) -> ApplicationSource {
        unreachable!("display tests run on Linux only")
    }

    #[test]
    fn names_are_simplified() {
        assert_eq!(friendly_name("discord"), "Discord");
        assert_eq!(friendly_name("éabc"), "Éabc");
        assert_eq!(friendly_name("调音台"), "调音台");
        assert_eq!(friendly_name("Limusic"), "Limusic");
        assert_eq!(friendly_name("chrome.EXE"), "Chrome");
        assert_eq!(friendly_name("PipeWire ALSA [rusttalk]"), "Rusttalk");
        assert_eq!(friendly_name("  \u{7}  "), "Application");
        assert_eq!(
            friendly_name(&"x".repeat(500)).chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn redundant_stream_text_is_dropped_and_duplicates_are_distinguished() {
        assert_eq!(stream_detail("Limusic", Some("Limusic")), None);
        assert_eq!(stream_detail("Discord", Some("Playback")), None);
        assert_eq!(
            stream_detail("Firefox", Some("Video - Cats")).as_deref(),
            Some("Video - Cats")
        );
        #[cfg(target_os = "linux")]
        {
            let apps = finish(vec![
                raw("firefox", Some("Playback"), 1),
                raw("firefox", Some("Playback"), 2),
                raw("firefox", Some("Stream 1"), 3),
                raw("scrcpy", Some("scrcpy"), 4),
            ]);
            let labels: Vec<_> = apps
                .iter()
                .map(|a| (a.label.as_str(), a.detail.as_deref()))
                .collect();
            assert_eq!(labels.len(), 4);
            assert!(labels.contains(&("Scrcpy", None)));
            let firefox: std::collections::HashSet<_> = labels
                .iter()
                .filter(|l| l.0 == "Firefox")
                .map(|l| l.1)
                .collect();
            assert_eq!(firefox.len(), 3, "same-app rows must stay distinguishable");
            assert!(firefox.iter().all(Option::is_some));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mixed_case_app_names_keep_duplicate_rows_distinct() {
        let apps = finish(vec![
            raw("Firefox", Some("Playback"), 1),
            raw("FIREFOX", Some("Playback"), 2),
            raw("Firefox", Some("Playback"), 3),
        ]);
        let displayed: std::collections::HashSet<_> =
            apps.iter().map(|app| (&app.label, &app.detail)).collect();
        assert_eq!(displayed.len(), 3);
        for id in 1..=3 {
            assert_eq!(
                apps.iter()
                    .filter(|app| app.source == test_source(id))
                    .count(),
                1
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn icon_lookup_is_bounded_and_path_safe() {
        let dirs: Vec<std::path::PathBuf> = (0..20).map(|i| format!("/d{i}").into()).collect();
        let kept = icon_dirs(
            None,
            Some("/home/x".into()),
            Some(std::env::join_paths(&dirs).unwrap()),
        );
        assert!(kept.len() <= 8);
        for bad in ["", "../etc/passwd", "a/b", ".hidden", &"a".repeat(65)] {
            assert!(icon_candidates(bad, &kept).is_empty(), "{bad}");
        }
        assert_eq!(icon_candidates("discord", &kept).len(), kept.len() * 7);
        assert!(decode_icon(b"not a png").is_none());
        // 513x513 exceeds the limit
        let mut big = Vec::new();
        image::DynamicImage::new_rgba8(513, 513)
            .write_to(&mut std::io::Cursor::new(&mut big), image::ImageFormat::Png)
            .unwrap();
        assert!(decode_icon(&big).is_none());
        let mut ok = Vec::new();
        image::DynamicImage::new_rgba8(48, 48)
            .write_to(&mut std::io::Cursor::new(&mut ok), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_icon(&ok).unwrap().rgba.len(), ICON_PX * ICON_PX * 4);
    }
}
