//! Chat links: strict http(s) detection, a confirm-before-open dialog, exact-origin
//! trust and an optional local threat feed. Nothing here touches the network; only a
//! deliberate "Open in browser" hands the canonical URL to the system browser.

use super::{theme, RustTalkApp};
use eframe::egui;
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use url::{Host, Position, Url};

const MAX_URL_BYTES: usize = 2048;
pub(super) const MAX_TRUSTED: usize = 200;
pub(super) const TRUST_DAYS: u64 = 90;
const DAY: u64 = 86_400;
const FEED_MAX_BYTES: u64 = 32 * 1024 * 1024;
const FEED_MAX_ENTRIES: usize = 250_000;
pub(super) const FEED_STALE_DAYS: u64 = 7;

/// File types a browser would download and the user might run or open with macros.
const RISKY: &[&str] = &[
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "msi",
    "msix",
    "msixbundle",
    "msp",
    "mst",
    "appx",
    "appxbundle",
    "appinstaller",
    "application",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "psd1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "wsc",
    "hta",
    "lnk",
    "url",
    "scf",
    "reg",
    "inf",
    "dll",
    "sys",
    "ocx",
    "cab",
    "chm",
    "gadget",
    "jar",
    "jnlp",
    "xll",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "library-ms",
    "settingcontent-ms",
    "docm",
    "dotm",
    "xlsm",
    "xltm",
    "xlam",
    "pptm",
    "potm",
    "ppam",
    "py",
    "pyw",
    "dmg",
    "pkg",
    "mpkg",
    "app",
    "command",
    "sh",
    "run",
    "bin",
    "appimage",
    "deb",
    "rpm",
    "snap",
    "flatpakref",
    "desktop",
    "apk",
    "apks",
    "xapk",
    "aab",
    "zip",
    "rar",
    "7z",
    "tar",
    "gz",
    "tgz",
    "xz",
    "bz2",
];

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Invisible or direction-changing characters that make text read differently than it goes.
fn deceptive(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reject {
    TooLong,
    Deceptive,
    Credentials,
    Unparsable,
    NotWeb,
}

/// Validates link text; the returned URL is the canonical destination that gets opened.
pub(super) fn parse(raw: &str) -> Result<Url, Reject> {
    if raw.len() > MAX_URL_BYTES {
        return Err(Reject::TooLong);
    }
    // Browsers read `\` as `/` in web URLs, so `evil.com\@github.com` would mislead.
    if raw.chars().any(|c| deceptive(c) || c == '\\') {
        return Err(Reject::Deceptive);
    }
    let authority = raw.split_once("://").map_or("", |(_, rest)| {
        rest.split(['/', '?', '#']).next().unwrap_or("")
    });
    if authority.contains('@') {
        return Err(Reject::Credentials);
    }
    let url = Url::parse(raw).map_err(|_| Reject::Unparsable)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(Reject::NotWeb);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Reject::Credentials);
    }
    if url.as_str().len() > MAX_URL_BYTES {
        return Err(Reject::TooLong);
    }
    Ok(url)
}

/// A valid web link at the start of `rest` (byte length and target). `prev` is the
/// character before it; links only start at a word boundary.
pub(super) fn detect(rest: &str, prev: Option<char>) -> Option<(usize, Url)> {
    if prev.is_some_and(|c| c.is_alphanumeric() || "/@.-_+%:&=".contains(c)) {
        return None;
    }
    let bytes = rest.as_bytes();
    let scheme = if bytes.len() >= 8 && bytes[..8].eq_ignore_ascii_case(b"https://") {
        8
    } else if bytes.len() >= 7 && bytes[..7].eq_ignore_ascii_case(b"http://") {
        7
    } else {
        return None;
    };
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
        .unwrap_or(rest.len());
    if end > MAX_URL_BYTES {
        return None;
    }
    let mut candidate = &rest[..end];
    // Opens minus closes per bracket pair; trailing punctuation and unbalanced
    // closers belong to the sentence around the link.
    let mut depth = [0i32; 3];
    for c in candidate.chars() {
        match c {
            '(' => depth[0] += 1,
            ')' => depth[0] -= 1,
            '[' => depth[1] += 1,
            ']' => depth[1] -= 1,
            '{' => depth[2] += 1,
            '}' => depth[2] -= 1,
            _ => {}
        }
    }
    while let Some(last) = candidate.chars().next_back() {
        let pair = match last {
            ')' => Some(0),
            ']' => Some(1),
            '}' => Some(2),
            _ => None,
        };
        if !(".,:;!?'*_~".contains(last) || pair.is_some_and(|i| depth[i] < 0)) {
            break;
        }
        if let Some(i) = pair {
            depth[i] += 1;
        }
        candidate = &candidate[..candidate.len() - last.len_utf8()];
    }
    if candidate.len() <= scheme {
        return None;
    }
    parse(candidate).ok().map(|url| (candidate.len(), url))
}

/// Host as people read it; international names are decoded from punycode.
pub(super) fn display_host(url: &Url) -> String {
    match url.host() {
        Some(Host::Domain(d)) => url::quirks::domain_to_unicode(d),
        Some(host) => host.to_string(),
        None => String::new(),
    }
}

pub(super) fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Warning {
    Listed(String),
    Download(String),
    Insecure,
    IpAddress,
    LocalNetwork,
    International(String),
}

impl Warning {
    fn text(&self) -> String {
        match self {
            Self::Listed(source) => format!("Reported as malicious in your threat feed ({source}). Do not open it unless you are sure the report is wrong."),
            Self::Download(ext) => format!("Looks like a download of a .{ext} file (program, script, installer, disk image or archive). Files from links can take over your computer; only run them if you trust the sender and the site."),
            Self::Insecure => "Not encrypted (http). Anyone on the network can read or change this page.".into(),
            Self::IpAddress => "Uses a bare IP address instead of a site name.".into(),
            Self::LocalNetwork => "Points to this computer or your local network (router, printer, internal service).".into(),
            Self::International(ascii) => format!("International site name. The real address is {ascii}; letters from other alphabets can imitate familiar sites."),
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).copied().and_then(hex),
            b.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn risky_ext(name: &str) -> Option<String> {
    // Twice, so `%252e` hides nothing either.
    let name = percent_decode(&percent_decode(name));
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    let name = name.split(';').next().unwrap_or("");
    // Windows drops trailing dots and spaces from file names.
    let name = name.trim_end_matches(|c: char| c == '.' || c.is_whitespace() || c.is_control());
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    RISKY.contains(&ext.as_str()).then_some(ext)
}

/// File extension of a likely risky download: last path segment or a query value.
pub(super) fn download_ext(url: &Url) -> Option<String> {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()));
    last.and_then(risky_ext).or_else(|| {
        url.query()?
            .split('&')
            .take(32)
            .flat_map(|pair| pair.split('='))
            .find_map(|part| risky_ext(&part.replace('+', " ")))
    })
}

fn local_name(domain: &str) -> bool {
    let d = domain.trim_end_matches('.');
    !d.contains('.')
        || [
            ".localhost",
            ".local",
            ".internal",
            ".lan",
            ".home.arpa",
            ".intranet",
            ".corp",
        ]
        .iter()
        .any(|suffix| d.ends_with(suffix))
}

fn local_v4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
}

fn local_v6(ip: std::net::Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || ip.to_ipv4_mapped().is_some_and(local_v4)
}

/// Everything worth a warning, most serious first. Trust never removes these.
pub(super) fn assess(url: &Url, feed: &FeedState) -> Vec<Warning> {
    let mut warnings = Vec::new();
    if let FeedState::Ready(feed) = feed {
        if feed.listed(url) {
            warnings.push(Warning::Listed(feed.name.clone()));
        }
    }
    if let Some(ext) = download_ext(url) {
        warnings.push(Warning::Download(ext));
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => {
            warnings.push(Warning::IpAddress);
            if local_v4(ip) {
                warnings.push(Warning::LocalNetwork);
            }
        }
        Some(Host::Ipv6(ip)) => {
            warnings.push(Warning::IpAddress);
            if local_v6(ip) {
                warnings.push(Warning::LocalNetwork);
            }
        }
        Some(Host::Domain(d)) => {
            if local_name(d) {
                warnings.push(Warning::LocalNetwork);
            }
            if d.split('.').any(|label| label.starts_with("xn--")) {
                warnings.push(Warning::International(d.to_owned()));
            }
        }
        None => {}
    }
    if url.scheme() == "http" {
        warnings.push(Warning::Insecure);
    }
    warnings
}

// ----- trust ---------------------------------------------------------------

fn trust_valid(at: u64, now: u64) -> bool {
    at <= now + DAY && now.saturating_sub(at) < TRUST_DAYS * DAY
}

pub(super) fn is_trusted(trusted: &BTreeMap<String, u64>, url: &Url, now: u64) -> bool {
    trusted
        .get(&origin(url))
        .is_some_and(|&at| trust_valid(at, now))
}

/// Drops expired or future-dated entries and keeps the newest `MAX_TRUSTED`.
pub(super) fn prune(trusted: &mut BTreeMap<String, u64>, now: u64) {
    trusted.retain(|_, at| trust_valid(*at, now));
    while trusted.len() > MAX_TRUSTED {
        let Some(oldest) = trusted
            .iter()
            .min_by_key(|(_, at)| **at)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        trusted.remove(&oldest);
    }
}

pub(super) fn trust(trusted: &mut BTreeMap<String, u64>, url: &Url, now: u64) {
    trusted.insert(origin(url), now);
    prune(trusted, now);
}

// ----- threat feed ---------------------------------------------------------

pub(super) struct Feed {
    urls: HashSet<String>,
    hosts: HashSet<String>,
    pub name: String,
    pub entries: usize,
    /// File modification time (unix seconds): the age of the data.
    pub modified: Option<u64>,
}

pub(super) enum FeedState {
    Off,
    Failed(String),
    Ready(Feed),
}

/// One key for http and https and for trivially re-encoded forms: no scheme, fragment
/// or trailing host dot, unreserved escapes decoded, other escapes in upper case.
fn feed_key(url: &Url) -> String {
    let mut key = host_key(url);
    let rest = url[Position::BeforePort..Position::AfterQuery].as_bytes();
    let hex = |c: Option<&u8>| c.and_then(|&c| (c as char).to_digit(16));
    let mut i = 0;
    while i < rest.len() {
        if let (b'%', Some(h), Some(l)) = (rest[i], hex(rest.get(i + 1)), hex(rest.get(i + 2))) {
            let c = (h * 16 + l) as u8 as char;
            if c.is_ascii_alphanumeric() || "-._~".contains(c) {
                key.push(c);
            } else {
                key.push_str(&format!("%{:02X}", h * 16 + l));
            }
            i += 3;
        } else {
            key.push(rest[i] as char);
            i += 1;
        }
    }
    key
}

fn host_key(url: &Url) -> String {
    url.host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_owned()
}

impl Feed {
    fn listed(&self, url: &Url) -> bool {
        self.urls.contains(&feed_key(url)) || self.hosts.contains(&host_key(url))
    }

    pub(super) fn age_days(&self, now: u64) -> Option<u64> {
        self.modified.map(|m| now.saturating_sub(m) / DAY)
    }
}

/// Parses plain URL lists, hosts files (`0.0.0.0 host`), bare host lists and
/// URLhaus-style CSV rows. Lines that are none of these are skipped.
pub(super) fn parse_feed(text: &str) -> Result<(HashSet<String>, HashSet<String>), String> {
    let (mut urls, mut hosts) = (HashSet::new(), HashSet::new());
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with('!')
            || line.len() > MAX_URL_BYTES
        {
            continue;
        }
        let token = if line.starts_with('"') {
            line.split("\",\"")
                .map(|field| field.trim_matches('"'))
                .find(|field| field.starts_with("http://") || field.starts_with("https://"))
                .unwrap_or("")
        } else {
            let mut parts = line.split_whitespace();
            match parts.next().unwrap_or("") {
                "0.0.0.0" | "127.0.0.1" | "::" | "::1" => parts.next().unwrap_or(""),
                first => first,
            }
        };
        if token.contains("://") {
            if let Ok(url) = Url::parse(token) {
                if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
                    urls.insert(feed_key(&url));
                }
            }
        } else if let Ok(url) = Url::parse(&format!("http://{token}/")) {
            let host = host_key(&url);
            if url.path() == "/"
                && url.query().is_none()
                && url.username().is_empty()
                && host.contains('.')
                && !matches!(host.as_str(), "0.0.0.0" | "127.0.0.1")
            {
                hosts.insert(host);
            }
        }
        if urls.len() + hosts.len() > FEED_MAX_ENTRIES {
            return Err(format!("more than {FEED_MAX_ENTRIES} entries; use a smaller list such as the currently online URLs"));
        }
    }
    if urls.is_empty() && hosts.is_empty() {
        return Err("no web addresses or host names found".into());
    }
    Ok((urls, hosts))
}

impl FeedState {
    /// Reads the user's feed file, bounded. Never downloads anything.
    pub(super) fn load(path: Option<&str>) -> Self {
        let Some(path) = path else { return Self::Off };
        let read = || -> Result<Feed, String> {
            // Checked before open: opening a FIFO would block the UI.
            if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
                return Err("file not found or not a regular file".into());
            }
            let file = std::fs::File::open(path)
                .map_err(|_| "file not found or not readable".to_owned())?;
            let meta = file
                .metadata()
                .map_err(|_| "file not readable".to_owned())?;
            if !meta.is_file() || meta.len() > FEED_MAX_BYTES {
                return Err(format!(
                    "not a file or larger than {} MiB",
                    FEED_MAX_BYTES >> 20
                ));
            }
            let mut bytes = Vec::new();
            file.take(FEED_MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "file not readable".to_owned())?;
            if bytes.len() as u64 > FEED_MAX_BYTES {
                return Err(format!("larger than {} MiB", FEED_MAX_BYTES >> 20));
            }
            let text = String::from_utf8(bytes).map_err(|_| "not a UTF-8 text file".to_owned())?;
            let (urls, hosts) = parse_feed(&text)?;
            let name: String = std::path::Path::new(path)
                .file_name()
                .map(|n| {
                    n.to_string_lossy()
                        .chars()
                        .filter(|&c| !deceptive(c))
                        .take(64)
                        .collect()
                })
                .unwrap_or_default();
            let modified = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            Ok(Feed {
                entries: urls.len() + hosts.len(),
                urls,
                hosts,
                name,
                modified,
            })
        };
        read().map_or_else(Self::Failed, Self::Ready)
    }

    /// One line for the dialog and settings; never claims a link is safe.
    pub(super) fn status(&self, now: u64) -> String {
        match self {
            Self::Off => "Known-malicious check is off: no threat feed imported (Settings > Advanced > Links).".into(),
            Self::Failed(e) => format!("Threat feed could not be loaded ({e}); known-malicious check is off."),
            Self::Ready(feed) => {
                let age = match feed.age_days(now) {
                    Some(0) => "updated today".to_owned(),
                    Some(1) => "updated 1 day ago".to_owned(),
                    Some(d) => format!("updated {d} days ago"),
                    None => "update time unknown".to_owned(),
                };
                let stale = if feed.age_days(now).is_none_or(|d| d >= FEED_STALE_DAYS) {
                    " It is out of date: import a fresh copy."
                } else {
                    ""
                };
                format!("Threat feed {} ({} entries, {age}).{stale}", feed.name, feed.entries)
            }
        }
    }
}

// ----- confirmation --------------------------------------------------------

pub(super) struct Prompt {
    pub url: Url,
    pub warnings: Vec<Warning>,
    remember: bool,
    acknowledged: bool,
    focus: bool,
}

impl Prompt {
    fn new(url: Url, warnings: Vec<Warning>) -> Self {
        Self {
            url,
            warnings,
            remember: false,
            acknowledged: false,
            focus: true,
        }
    }

    /// Only clean https links can be remembered; anything with a warning always asks.
    pub(super) fn can_remember(&self) -> bool {
        self.warnings.is_empty() && self.url.scheme() == "https"
    }

    fn needs_ack(&self) -> bool {
        self.warnings
            .iter()
            .any(|w| matches!(w, Warning::Listed(_)))
    }
}

pub(super) enum Next {
    Open(Url),
    Ask(Prompt),
}

pub(super) fn decide(
    url: Url,
    trusted: &BTreeMap<String, u64>,
    feed: &FeedState,
    now: u64,
) -> Next {
    let warnings = assess(&url, feed);
    if warnings.is_empty() && url.scheme() == "https" && is_trusted(trusted, &url, now) {
        Next::Open(url)
    } else {
        Next::Ask(Prompt::new(url, warnings))
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Choice {
    Cancel,
    Open { remember: bool },
}

pub(super) fn prompt_ui(
    ctx: &egui::Context,
    p: &mut Prompt,
    feed: &FeedState,
    now: u64,
) -> Option<Choice> {
    let mut choice = None;
    let width = (ctx.screen_rect().width() - 48.0).clamp(200.0, 520.0);
    let modal = egui::Modal::new(egui::Id::new("link_prompt"))
        .frame(egui::Frame::default().fill(theme::BG).corner_radius(10.0).inner_margin(egui::Margin::same(20)))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(egui::RichText::new("Open external link?").strong().size(16.0).color(theme::HEADER));
            ui.add(egui::Label::new("This leaves RustTalk and opens the address below in your web browser.").wrap());
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Site").color(theme::MUTED));
                ui.label(egui::RichText::new(display_host(&p.url)).strong().size(15.0).color(theme::HEADER));
            });
            ui.label(egui::RichText::new("Full address").color(theme::MUTED));
            egui::Frame::new().fill(theme::DARK).corner_radius(4.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(p.url.as_str()).monospace().size(12.5)).wrap());
                });
            });
            for w in &p.warnings {
                let (icon, color) = match w {
                    Warning::Listed(_) | Warning::Download(_) => (super::icons::regular::WARNING_OCTAGON, theme::RED),
                    _ => (super::icons::regular::WARNING, theme::YELLOW),
                };
                ui.horizontal_top(|ui| {
                    ui.label(egui::RichText::new(icon).color(color));
                    ui.add(egui::Label::new(egui::RichText::new(w.text()).color(color)).wrap());
                });
            }
            if !p.needs_ack() {
                let status = feed.status(now);
                let note = if matches!(feed, FeedState::Ready(_)) {
                    format!("{status} This address is not on it, which does not mean it is safe.")
                } else {
                    status
                };
                ui.add(egui::Label::new(egui::RichText::new(note).size(12.0).color(theme::MUTED)).wrap());
            }
            if p.needs_ack() {
                ui.checkbox(&mut p.acknowledged, "I understand this address was reported as malicious");
            }
            if p.can_remember() {
                ui.checkbox(&mut p.remember, format!("Always open links to {} without asking", origin(&p.url)))
                    .on_hover_text(format!("Only this exact site and protocol. Warnings still show. Expires after {TRUST_DAYS} days; remove it any time in Settings > Advanced > Links."));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                ui.spacing_mut().button_padding = egui::vec2(12.0, 10.0);
                let cancel = ui.button("Cancel");
                if std::mem::take(&mut p.focus) {
                    cancel.request_focus();
                }
                if cancel.clicked() {
                    choice = Some(Choice::Cancel);
                }
                let allowed = !p.needs_ack() || p.acknowledged;
                let open = egui::Button::new(egui::RichText::new("Open in browser").strong().color(theme::on_accent())).fill(theme::accent());
                if ui.add_enabled(allowed, open).clicked() {
                    choice = Some(Choice::Open { remember: p.remember && p.can_remember() });
                }
            });
        });
    if choice.is_none() && modal.should_close() {
        choice = Some(Choice::Cancel);
    }
    choice
}

impl RustTalkApp {
    /// Handles a link clicked in chat this frame and shows the pending confirmation.
    pub(super) fn ui_links(&mut self, ctx: &egui::Context) {
        let now = now();
        if let Some(url) = self.markdown.take_clicked() {
            if self.link_prompt.is_none() {
                match decide(url, &self.config.trusted_links, &self.threat_feed, now) {
                    Next::Open(url) => open(ctx, &url),
                    Next::Ask(prompt) => self.link_prompt = Some(prompt),
                }
            }
        }
        let Some(prompt) = self.link_prompt.as_mut() else {
            return;
        };
        match prompt_ui(ctx, prompt, &self.threat_feed, now) {
            None => {}
            Some(Choice::Cancel) => self.link_prompt = None,
            Some(Choice::Open { remember }) => {
                let Some(prompt) = self.link_prompt.take() else {
                    return;
                };
                if remember && prompt.can_remember() {
                    trust(&mut self.config.trusted_links, &prompt.url, now);
                    self.config.save();
                }
                open(ctx, &prompt.url);
            }
        }
    }
}

/// eframe passes this to the `webbrowser` crate: no shell, http(s) only after our checks.
fn open(ctx: &egui::Context, url: &Url) {
    ctx.open_url(egui::OpenUrl::new_tab(url.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"))
    }

    fn linked(text: &str) -> Option<(String, String)> {
        detect(text, None).map(|(len, u)| (text[..len].to_owned(), u.to_string()))
    }

    #[test]
    fn detection_boundaries() {
        assert_eq!(
            linked("https://github.com/rusttalkcrew/RustTalk/actions/runs/43452482855 next"),
            Some((
                "https://github.com/rusttalkcrew/RustTalk/actions/runs/43452482855".into(),
                "https://github.com/rusttalkcrew/RustTalk/actions/runs/43452482855".into()
            ))
        );
        assert_eq!(
            linked("https://example.com/a."),
            Some((
                "https://example.com/a".into(),
                "https://example.com/a".into()
            ))
        );
        assert_eq!(
            linked("https://en.wikipedia.org/wiki/Rust_(language))."),
            Some((
                "https://en.wikipedia.org/wiki/Rust_(language)".into(),
                "https://en.wikipedia.org/wiki/Rust_(language)".into()
            ))
        );
        assert_eq!(
            linked("HTTPS://Example.COM/X"),
            Some((
                "HTTPS://Example.COM/X".into(),
                "https://example.com/X".into()
            ))
        );
        assert_eq!(
            linked("https://example.com/\"onclick"),
            Some(("https://example.com/".into(), "https://example.com/".into()))
        );
        assert_eq!(
            linked("https://example.com/`code`").map(|l| l.0),
            Some("https://example.com/".into())
        );
        assert!(linked("https://").is_none());
        assert!(linked("https://.,").is_none());
        assert!(linked("ftp://example.com").is_none());
        assert!(linked("javascript:alert(1)").is_none());
        assert!(linked("file:///etc/passwd").is_none());
        assert!(linked("data:text/html,hi").is_none());
        assert!(
            detect("https://example.com", Some('x')).is_none(),
            "xhttps:// is not a link"
        );
        assert!(detect("https://example.com", Some('(')).is_some());
        assert!(linked(&format!("https://example.com/{}", "a".repeat(5000))).is_none());
        // Truncated and odd byte boundaries must not panic.
        for text in ["h", "http", "https:/", "https:/\u{1F980}", "\u{1F980}"] {
            assert!(linked(text).is_none());
        }
        assert!(
            linked(&format!("https://a{}", ")".repeat(2000))).is_some(),
            "trim stays linear"
        );
    }

    #[test]
    fn rejects_credentials_and_deception() {
        assert_eq!(
            parse("https://user:pw@example.com/"),
            Err(Reject::Credentials)
        );
        assert_eq!(
            parse("https://github.com@evil.example/"),
            Err(Reject::Credentials)
        );
        assert_eq!(parse("https://@evil.example/"), Err(Reject::Credentials));
        assert_eq!(
            parse("https://evil.example\\@github.com"),
            Err(Reject::Deceptive)
        );
        for c in [
            '\u{202E}',
            '\u{200B}',
            '\u{2066}',
            '\u{FEFF}',
            '\u{00AD}',
            '\u{7}',
            '\u{E0041}',
        ] {
            assert_eq!(
                parse(&format!("https://exa{c}mple.com/")),
                Err(Reject::Deceptive),
                "{:?}",
                c
            );
            assert!(
                linked(&format!("https://example.com/a{c}b")).is_none(),
                "{:?}",
                c
            );
        }
        assert_eq!(parse("https://exa mple.com/"), Err(Reject::Unparsable));
        assert_eq!(
            parse("https://github.com\u{FF0F}evil.example/"),
            Err(Reject::Unparsable)
        );
        assert_eq!(parse("http://[::1"), Err(Reject::Unparsable));
        assert!(
            parse(&format!("https://e.com/{}", "\u{00FC}".repeat(400))).is_err(),
            "canonical form is bounded too"
        );
    }

    #[test]
    fn canonical_form_shows_real_destination() {
        assert_eq!(url("http://0x7f.1/").as_str(), "http://127.0.0.1/");
        assert_eq!(url("https://%67ithub.com/").host_str(), Some("github.com"));
        let idn = url("https://\u{0430}pple.com/");
        assert_eq!(idn.host_str(), Some("xn--pple-43d.com"));
        assert_eq!(display_host(&idn), "\u{0430}pple.com");
        assert!(assess(&idn, &FeedState::Off)
            .contains(&Warning::International("xn--pple-43d.com".into())));
        // Reparsing what we hand to the browser gives the same address.
        for s in [
            "https://example.com/a b?c=\u{00FC}#\"x\"",
            "http://[::ffff:10.0.0.1]:8080/",
            "https://example.com:443/x",
        ] {
            let u = Url::parse(s).unwrap();
            assert_eq!(Url::parse(u.as_str()).unwrap().as_str(), u.as_str());
            assert!(!u.as_str().contains([' ', '"']));
        }
    }

    #[test]
    fn download_warnings() {
        for s in [
            "https://example.com/setup.exe",
            "https://example.com/SETUP.EXE",
            "https://example.com/setup%2Eexe",
            "https://example.com/setup%252Eexe",
            "https://example.com/invoice.pdf.exe",
            "https://example.com/setup.exe.",
            "https://example.com/setup.exe%20",
            "https://example.com/setup.msi;jsessionid=1",
            "https://example.com/files/run.ps1/",
            "https://example.com/get?file=Setup.MSI",
            "https://example.com/get?setup.exe",
            "https://example.com/dl?name=a%2Fb%2Fevil.bat&x=1",
            "https://example.com/a%E2%80%AEgpj.exe",
            "https://github.com/o/r/releases/download/v1/app.AppImage",
        ] {
            assert!(download_ext(&url(s)).is_some(), "{s}");
        }
        for s in [
            "https://example.com/",
            "https://example.com/readme.md",
            "https://example.com/exe",
            "https://example.com/page?q=exe",
            "https://example.com/x#setup.exe",
        ] {
            assert!(download_ext(&url(s)).is_none(), "{s}");
        }
    }

    #[test]
    fn suspicious_destinations() {
        let w = |s: &str| assess(&url(s), &FeedState::Off);
        assert!(w("https://github.com/rusttalkcrew/RustTalk").is_empty());
        assert_eq!(w("http://example.com/"), vec![Warning::Insecure]);
        assert_eq!(w("https://8.8.8.8/"), vec![Warning::IpAddress]);
        for s in [
            "https://192.168.1.1/",
            "https://10.0.0.1/",
            "https://127.0.0.1:8080/",
            "https://169.254.1.1/",
            "https://100.64.0.1/",
            "https://0.0.0.0/",
            "https://[::1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "https://[::ffff:192.168.0.1]/",
            "https://localhost/",
            "https://router/",
            "https://nas.local/",
            "https://app.localhost./",
            "https://printer.home.arpa/",
        ] {
            assert!(w(s).contains(&Warning::LocalNetwork), "{s}");
        }
    }

    #[test]
    fn trust_is_exact_origin_bounded_and_expires() {
        let now = 1_700_000_000;
        let mut trusted = BTreeMap::new();
        trust(&mut trusted, &url("https://github.com/a"), now);
        assert!(is_trusted(
            &trusted,
            &url("https://github.com/other?x"),
            now
        ));
        for s in [
            "http://github.com/",
            "https://github.com:8443/",
            "https://gist.github.com/",
            "https://github.com.evil.example/",
            "https://evilgithub.com/",
        ] {
            assert!(!is_trusted(&trusted, &url(s), now), "{s}");
        }
        assert!(!is_trusted(
            &trusted,
            &url("https://github.com/"),
            now + TRUST_DAYS * DAY
        ));
        trusted.insert("https://future.example".into(), now + 30 * DAY);
        assert!(
            !is_trusted(&trusted, &url("https://future.example/"), now),
            "future dates are not trusted"
        );
        for i in 0..(MAX_TRUSTED as u64 + 50) {
            trusted.insert(format!("https://site{i}.example"), now - 1000 + i);
        }
        prune(&mut trusted, now);
        assert_eq!(trusted.len(), MAX_TRUSTED);
        assert!(!trusted.contains_key("https://future.example"));
        assert!(
            trusted.contains_key(&format!("https://site{}.example", MAX_TRUSTED + 49)),
            "newest kept"
        );
    }

    fn feed(text: &str, age_days: u64, now: u64) -> FeedState {
        let (urls, hosts) = parse_feed(text).unwrap();
        FeedState::Ready(Feed {
            entries: urls.len() + hosts.len(),
            urls,
            hosts,
            name: "test.txt".into(),
            modified: Some(now - age_days * DAY),
        })
    }

    #[test]
    fn decide_trust_never_hides_warnings() {
        let now = 1_700_000_000;
        let mut trusted = BTreeMap::new();
        for s in [
            "https://github.com/",
            "http://plain.example/",
            "https://8.8.8.8/",
        ] {
            trusted.insert(origin(&url(s)), now);
        }
        assert!(matches!(
            decide(url("https://github.com/x"), &trusted, &FeedState::Off, now),
            Next::Open(_)
        ));
        assert!(
            matches!(decide(url("https://example.org/"), &trusted, &FeedState::Off, now), Next::Ask(p) if p.can_remember())
        );
        for s in [
            "https://github.com/x/setup.exe",
            "http://plain.example/",
            "https://8.8.8.8/",
            "http://github.com/x",
        ] {
            let Next::Ask(p) = decide(url(s), &trusted, &FeedState::Off, now) else {
                panic!("{s} opened without asking")
            };
            assert!(!p.can_remember(), "{s}");
        }
        let listed = feed("https://github.com/bad/payload\n", 0, now);
        let Next::Ask(p) = decide(
            url("https://github.com/bad/payload#x"),
            &trusted,
            &listed,
            now,
        ) else {
            panic!("listed URL opened")
        };
        assert!(p.needs_ack() && !p.can_remember());
    }

    #[test]
    fn feed_formats_bounds_and_matching() {
        let now = 1_700_000_000;
        let text = "# URLhaus\n\
            http://evil.example/a.bin\n\
            \"1\",\"2024-01-01\",\"http://csv.example/x?y=1\",\"online\"\n\
            0.0.0.0 hosts.example\n\
            127.0.0.1\tlocalhost\n\
            bare.example\n\
            ! comment\n\
            garbage line with words\n\
            ftp://nope.example/\n\
            https://enc.example/a%2fb\n";
        let state = feed(text, 0, now);
        let FeedState::Ready(f) = &state else {
            unreachable!()
        };
        assert_eq!(f.entries, 5, "{:?} {:?}", f.urls, f.hosts);
        for s in [
            "https://evil.example/a.bin",
            "https://EVIL.example./%61.bin#frag",
            "http://csv.example/x?y=1",
            "https://hosts.example/anything",
            "http://bare.example/",
            "http://enc.example/a%2Fb",
        ] {
            assert!(f.listed(&url(s)), "{s}");
        }
        for s in [
            "https://evil.example/",
            "https://sub.hosts.example/",
            "https://csv.example/x",
            "http://localhost/",
        ] {
            assert!(!f.listed(&url(s)), "{s}");
        }
        assert!(parse_feed("# only comments\n\n").is_err());
        assert!(parse_feed(&"x.example\n".repeat(3)).is_ok());
        let big: String = (0..FEED_MAX_ENTRIES + 1)
            .map(|i| format!("h{i}.example\n"))
            .collect();
        assert!(parse_feed(&big).is_err(), "too many entries fails closed");
        assert!(
            parse_feed(&format!("https://e.com/{}\n", "a".repeat(5000))).is_err(),
            "overlong lines skipped"
        );
    }

    #[test]
    fn feed_states_never_claim_safety() {
        let now = 1_700_000_000;
        assert!(FeedState::Off.status(now).contains("off"));
        assert!(FeedState::Failed("x".into()).status(now).contains("off"));
        assert!(!feed("a.example\n", 0, now)
            .status(now)
            .contains("out of date"));
        assert!(feed("a.example\n", FEED_STALE_DAYS, now)
            .status(now)
            .contains("out of date"));
        for s in [
            FeedState::Off.status(now),
            feed("a.example\n", 0, now).status(now),
        ] {
            assert!(!s.to_lowercase().contains(" safe"), "{s}");
        }
        assert!(matches!(FeedState::load(None), FeedState::Off));
        assert!(matches!(
            FeedState::load(Some("/nonexistent/rusttalk-feed.txt")),
            FeedState::Failed(_)
        ));
    }

    #[test]
    fn feed_file_load_is_bounded() {
        let dir = std::env::temp_dir().join(format!("rusttalk-feed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("feed.txt");
        std::fs::write(&good, "https://evil.example/x\n").unwrap();
        let FeedState::Ready(f) = FeedState::load(good.to_str()) else {
            panic!("feed not loaded")
        };
        assert_eq!((f.entries, f.name.as_str()), (1, "feed.txt"));
        let binary = dir.join("feed.bin");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert!(
            matches!(FeedState::load(binary.to_str()), FeedState::Failed(e) if e.contains("UTF-8"))
        );
        let huge = dir.join("huge.txt");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(FEED_MAX_BYTES + 1)
            .unwrap();
        assert!(
            matches!(FeedState::load(huge.to_str()), FeedState::Failed(e) if e.contains("MiB"))
        );
        assert!(
            matches!(FeedState::load(dir.to_str()), FeedState::Failed(_)),
            "directories are refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// Runs the dialog with `keys` pressed one per frame after it settles.
    fn drive(p: &mut Prompt, keys: &[egui::Key]) -> (Option<Choice>, bool) {
        let ctx = egui::Context::default();
        let mut choice = None;
        let mut opened = false;
        let mut frame = |events: Vec<egui::Event>| {
            let out = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    if choice.is_none() {
                        choice = prompt_ui(ctx, p, &FeedState::Off, 0);
                    }
                },
            );
            opened |= out
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::OpenUrl(_)));
        };
        frame(vec![]);
        frame(vec![]);
        for &k in keys {
            frame(vec![key(k)]);
        }
        (choice, opened)
    }

    #[test]
    fn dialog_defaults_to_cancel_and_never_opens_by_itself() {
        let clean = || Prompt::new(url("https://example.com/"), vec![]);
        let (choice, opened) = drive(&mut clean(), &[egui::Key::Enter]);
        assert_eq!(
            choice,
            Some(Choice::Cancel),
            "Enter activates the focused Cancel"
        );
        assert!(!opened, "the dialog itself never opens a browser");
        assert_eq!(
            drive(&mut clean(), &[egui::Key::Escape]).0,
            Some(Choice::Cancel)
        );
        assert_eq!(drive(&mut clean(), &[]).0, None);
        assert_eq!(
            drive(&mut clean(), &[egui::Key::Tab, egui::Key::Enter]).0,
            Some(Choice::Open { remember: false })
        );
    }

    #[test]
    fn listed_link_needs_acknowledgement() {
        let mut p = Prompt::new(
            url("https://evil.example/"),
            vec![Warning::Listed("feed".into())],
        );
        p.remember = true;
        let (choice, _) = drive(&mut p, &[egui::Key::Tab, egui::Key::Enter]);
        assert_ne!(
            choice.as_ref().map(|c| matches!(c, Choice::Open { .. })),
            Some(true),
            "{choice:?}"
        );
        p.acknowledged = true;
        p.focus = true;
        let (choice, _) = drive(&mut p, &[egui::Key::Tab, egui::Key::Enter]);
        assert_eq!(
            choice,
            Some(Choice::Open { remember: false }),
            "listed origins are never remembered"
        );
    }
}
