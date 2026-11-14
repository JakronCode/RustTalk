//! Deliberately small, bounded chat dialect. Only bare http(s) links are live (see
//! `links`); markdown links, images and HTML stay inert text.
use super::theme;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat};
use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    ops::Range,
    sync::mpsc,
    time::{Duration, Instant},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color, StyleModifier, Theme, ThemeItem},
    parsing::SyntaxSet,
};

const MAX_BYTES: usize = 16 * 1024;
const MAX_BLOCKS: usize = 256;
const CACHE_SIZE: usize = 64;
const CODE_BYTES: usize = 4096;
const MEASUREMENT_LIMIT: usize = 10_000;
const MAX_LINKS: usize = 64;
const LINK: Color32 = Color32::from_rgb(0x00, 0xA8, 0xFC);

fn bounded(s: &str) -> &str {
    let mut end = s.len().min(MAX_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[derive(Clone, Debug)]
struct Block {
    job: LayoutJob,
    language: Option<String>,
    /// Byte ranges in `job.text` and their validated targets.
    links: Vec<(Range<usize>, url::Url)>,
}

fn format(size: f32, mono: bool) -> TextFormat {
    TextFormat {
        font_id: if mono {
            FontId::monospace(size)
        } else {
            FontId::proportional(size)
        },
        color: theme::TEXT(),
        ..Default::default()
    }
}

type Links = Vec<(Range<usize>, url::Url)>;

fn inline(
    text: &str,
    style: TextFormat,
    depth: usize,
    budget: &mut usize,
    job: &mut LayoutJob,
    links: &mut Links,
) {
    if depth == 8 || *budget < text.len() {
        job.append(text, 0.0, style);
        return;
    }
    *budget -= text.len();
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        if let Some(escaped) = rest.strip_prefix('\\').and_then(|s| s.chars().next()) {
            if "*_~`\\".contains(escaped) {
                job.append(&escaped.to_string(), 0.0, style.clone());
                pos += 1 + escaped.len_utf8();
                continue;
            }
        }
        // Links win over emphasis markers so `_` or `*` inside a URL stay part of it.
        if links.len() < MAX_LINKS {
            if let Some((len, url)) = super::links::detect(rest, text[..pos].chars().next_back()) {
                let mut styled = style.clone();
                styled.color = LINK;
                styled.underline = egui::Stroke::new(1.0_f32, LINK);
                let start = job.text.len();
                job.append(&rest[..len], 0.0, styled);
                links.push((start..job.text.len(), url));
                pos += len;
                continue;
            }
        }
        let marker = ["`", "**", "__", "~~", "*", "_"]
            .into_iter()
            .find(|m| rest.starts_with(m));
        if let Some(m) = marker {
            let tail = &rest[m.len()..];
            if *budget >= tail.len() {
                *budget -= tail.len();
                if let Some(end) = tail.match_indices(m).find_map(|(end, _)| {
                    let slashes = tail[..end]
                        .bytes()
                        .rev()
                        .take_while(|&b| b == b'\\')
                        .count();
                    (end > 0 && (m == "`" || slashes % 2 == 0)).then_some(end)
                }) {
                    let mut styled = style.clone();
                    match m {
                        "`" => {
                            styled.font_id = FontId::monospace(style.font_id.size);
                            styled.background = theme::DARK();
                        }
                        "**" | "__" => {
                            styled.color = theme::HEADER();
                            styled.extra_letter_spacing = 0.5;
                        }
                        "~~" => styled.strikethrough = egui::Stroke::new(1.0_f32, style.color),
                        _ => styled.italics = true,
                    }
                    if m == "`" {
                        job.append(&tail[..end], 0.0, styled);
                    } else {
                        inline(&tail[..end], styled, depth + 1, budget, job, links);
                    }
                    pos += m.len() * 2 + end;
                    continue;
                }
            }
        }
        let next = rest.chars().next().map_or(1, char::len_utf8);
        job.append(&rest[..next], 0.0, style.clone());
        pos += next;
    }
}

fn parse(text: &str) -> Vec<Block> {
    let text = bounded(text);
    let mut blocks = Vec::new();
    let mut lines = text.split_inclusive('\n').peekable();
    let mut budget = MAX_BYTES * 8;
    while let Some(line) = lines.next() {
        if blocks.len() == MAX_BLOCKS - 1 {
            let mut job = LayoutJob::default();
            job.append(line, 0.0, format(14.0, false));
            for remaining in lines {
                job.append(remaining, 0.0, format(14.0, false));
            }
            blocks.push(Block {
                job,
                language: None,
                links: Vec::new(),
            });
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(tag) = trimmed.strip_prefix("```") {
            let mut code = String::new();
            for line in lines.by_ref() {
                if line.trim_end() == "```" {
                    break;
                }
                code.push_str(line);
            }
            let mut job = LayoutJob::default();
            job.append(&code, 0.0, format(13.0, true));
            blocks.push(Block {
                job,
                language: Some(tag.trim().to_ascii_lowercase()),
                links: Vec::new(),
            });
            continue;
        }
        let mut body = trimmed;
        let mut size = 14.0;
        let mut prefix = "";
        for (mark, heading_size) in [("### ", 17.0), ("## ", 20.0), ("# ", 24.0)] {
            if let Some(t) = body.strip_prefix(mark) {
                body = t;
                size = heading_size;
                break;
            }
        }
        if let Some(t) = body.strip_prefix("> ") {
            body = t;
            prefix = "│ ";
        } else if let Some(t) = body
            .strip_prefix("- ")
            .or_else(|| body.strip_prefix("* "))
            .or_else(|| body.strip_prefix("+ "))
        {
            body = t;
            prefix = "• ";
        }
        // Ordered list numbers are already the right visible marker.
        let mut job = LayoutJob::default();
        job.append(prefix, 0.0, format(size, false));
        let mut links = Vec::new();
        inline(
            body,
            format(size, false),
            0,
            &mut budget,
            &mut job,
            &mut links,
        );
        blocks.push(Block {
            job,
            language: None,
            links,
        });
    }
    for block in &mut blocks {
        let sections = std::mem::take(&mut block.job.sections);
        for section in sections {
            if let Some(last) = block
                .job
                .sections
                .last_mut()
                .filter(|last| last.format == section.format)
            {
                last.byte_range.end = section.byte_range.end;
            } else {
                block.job.sections.push(section);
            }
        }
    }
    if blocks.iter().map(|b| b.job.sections.len()).sum::<usize>() > 1024 {
        let mut job = LayoutJob::default();
        job.append(text, 0.0, format(14.0, false));
        return vec![Block {
            job,
            language: None,
            links: Vec::new(),
        }];
    }
    blocks
}

pub(super) fn plain(text: &str) -> String {
    parse(text)
        .into_iter()
        .map(|b| {
            if b.language.is_some() {
                return b.job.text;
            }
            let text = b.job.text;
            let text = text
                .strip_prefix("│ ")
                .or_else(|| text.strip_prefix("• "))
                .unwrap_or(&text);
            let text = text
                .split_once(". ")
                .filter(|(n, _)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
                .map_or(text, |(_, t)| t);
            text.to_owned()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn language(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "rust" | "rs" => "Rust",
        "c" => "C",
        "cpp" | "c++" | "cc" | "cxx" => "C++",
        "csharp" | "cs" | "c#" => "C#",
        "java" => "Java",
        "javascript" | "js" | "jsx" => "JavaScript",
        "typescript" | "ts" | "tsx" => "TypeScript",
        "python" | "py" => "Python",
        "go" | "golang" => "Go",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "bash" | "sh" | "shell" => "Bourne Again Shell (bash)",
        "sql" => "SQL",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        _ => return None,
    })
}

fn highlight_theme() -> Theme {
    let mut theme = Theme::default();
    theme.settings.foreground = Some(Color {
        r: 220,
        g: 220,
        b: 220,
        a: 255,
    });
    for (selector, (r, g, b)) in [
        ("comment", (130, 145, 130)),
        ("string", (180, 210, 150)),
        ("keyword, storage", (195, 150, 230)),
        ("constant", (230, 180, 120)),
        ("entity.name", (130, 195, 235)),
    ] {
        if let Ok(scope) = selector.parse() {
            theme.scopes.push(ThemeItem {
                scope,
                style: StyleModifier {
                    foreground: Some(Color { r, g, b, a: 255 }),
                    ..Default::default()
                },
            });
        }
    }
    theme
}

fn highlight(blocks: &mut [Block], syntaxes: &SyntaxSet, theme: &Theme) -> bool {
    let start = Instant::now();
    for block in blocks {
        let Some(syntax) = block
            .language
            .as_deref()
            .and_then(language)
            .and_then(|name| syntaxes.find_syntax_by_name(name))
        else {
            continue;
        };
        let code = &block.job.text;
        if code.len() > CODE_BYTES
            || code.lines().count() > 128
            || code.lines().any(|l| l.len() > 256)
        {
            continue;
        }
        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut job = LayoutJob::default();
        for line in code.split_inclusive('\n') {
            if start.elapsed() > Duration::from_millis(30) {
                return false;
            }
            let Ok(spans) = highlighter.highlight_line(line, syntaxes) else {
                return true;
            };
            for (style, text) in spans {
                let mut f = format(13.0, true);
                f.color =
                    Color32::from_rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                job.append(text, 0.0, f);
            }
        }
        block.job = job;
    }
    true
}

// A timeout can include lazy regex compilation; retries stay on the single worker.
fn highlight_with_retry(blocks: &mut [Block], syntaxes: &SyntaxSet, theme: &Theme) {
    for _ in 0..3 {
        if highlight(blocks, syntaxes, theme) {
            break;
        }
    }
}

struct Entry {
    id: u64,
    source: String,
    blocks: Vec<Block>,
    serial: u64,
    submitted: bool,
}
type Work = (u64, Vec<Block>, egui::Context);

pub(super) struct Cache {
    entries: VecDeque<Entry>,
    serial: u64,
    measurements: HashMap<egui::Id, (u64, egui::Vec2)>,
    pending: bool,
    tx: mpsc::SyncSender<Work>,
    rx: mpsc::Receiver<(u64, Vec<Block>)>,
    /// Link clicked this frame, for the confirmation dialog.
    clicked: Option<url::Url>,
}

impl Default for Cache {
    fn default() -> Self {
        let (tx, work) = mpsc::sync_channel::<Work>(1);
        let (result, rx) = mpsc::sync_channel(1);
        // One worker, never one thread per message. A slow regex cannot block the UI
        // or accumulate jobs; elapsed budget is checked between short lines.
        std::thread::spawn(move || {
            let Ok(syntaxes) = syntect::dumps::from_uncompressed_data::<SyntaxSet>(include_bytes!(
                "../../assets/syntaxes.packdump"
            )) else {
                return;
            };
            let theme = highlight_theme();
            while let Ok((serial, mut blocks, ctx)) = work.recv() {
                highlight_with_retry(&mut blocks, &syntaxes, &theme);
                if result.send((serial, blocks)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            entries: VecDeque::new(),
            serial: 0,
            measurements: HashMap::new(),
            pending: false,
            tx,
            rx,
            clicked: None,
        }
    }
}

impl Cache {
    pub(super) fn take_clicked(&mut self) -> Option<url::Url> {
        self.clicked.take()
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, id: u64, text: &str) {
        if let Ok((serial, blocks)) = self.rx.try_recv() {
            self.pending = false;
            if let Some(entry) = self.entries.iter_mut().find(|e| e.serial == serial) {
                entry.blocks = blocks;
            }
        }
        let source = bounded(text);
        let measurement_id = ui.id().with(id);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        text.len().hash(&mut hasher);
        ui.available_size_before_wrap()
            .x
            .to_bits()
            .hash(&mut hasher);
        ui.ctx().pixels_per_point().to_bits().hash(&mut hasher);
        theme::compact().hash(&mut hasher);
        ui.spacing().item_spacing.x.to_bits().hash(&mut hasher);
        ui.spacing().item_spacing.y.to_bits().hash(&mut hasher);
        let fingerprint = hasher.finish();
        if let Some(&(old, size)) = self.measurements.get(&measurement_id) {
            let rect = egui::Rect::from_min_size(ui.next_widget_position(), size);
            if old == fingerprint && !ui.is_rect_visible(rect) {
                ui.vertical(|ui| ui.set_min_size(size));
                return;
            }
        }
        let index = self
            .entries
            .iter()
            .position(|e| e.id == id && e.source == source);
        let mut entry = if let Some(index) = index {
            self.entries
                .remove(index)
                .expect("index found in unchanged local cache")
        } else {
            self.serial = self.serial.wrapping_add(1);
            Entry {
                id,
                source: source.to_owned(),
                blocks: parse(source),
                serial: self.serial,
                submitted: false,
            }
        };
        let mut clicked = None;
        let response = ui.vertical(|ui| {
            for (i, block) in entry.blocks.iter().enumerate() {
                if let Some(tag) = &block.language {
                    ui.push_id((id, i), |ui| {
                        egui::Frame::new()
                            .fill(theme::DARK())
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(tag).small().color(theme::MUTED()));
                                    if ui.small_button("Copy").clicked() {
                                        ui.ctx().copy_text(block.job.text.clone());
                                    }
                                });
                                ui.add(egui::Label::new(block.job.clone()).wrap());
                            });
                    });
                } else if block.links.is_empty() {
                    ui.add(egui::Label::new(block.job.clone()).wrap());
                } else if let Some(url) = link_label(ui, block) {
                    clicked = Some(url);
                }
            }
            if text.len() > MAX_BYTES {
                ui.label("[message display truncated]");
            }
        });
        if clicked.is_some() {
            self.clicked = clicked;
        }
        if ui.is_rect_visible(response.response.rect)
            && !entry.submitted
            && !self.pending
            && entry
                .blocks
                .iter()
                .any(|b| b.language.as_deref().and_then(language).is_some())
        {
            if self
                .tx
                .try_send((entry.serial, entry.blocks.clone(), ui.ctx().clone()))
                .is_ok()
            {
                self.pending = true;
            }
            entry.submitted = true;
        }
        if self.measurements.len() >= MEASUREMENT_LIMIT
            && !self.measurements.contains_key(&measurement_id)
        {
            self.measurements.clear();
        }
        self.measurements
            .insert(measurement_id, (fingerprint, response.response.rect.size()));
        if ui.is_rect_visible(response.response.rect) {
            self.entries.push_back(entry);
            if self.entries.len() > CACHE_SIZE {
                self.entries.pop_front();
            }
        }
    }
}

/// Paints a block like `egui::Label` (selection included) and returns the link clicked.
fn link_label(ui: &mut egui::Ui, block: &Block) -> Option<url::Url> {
    let selectable = ui.style().interaction.selectable_labels;
    let (pos, galley, response) = egui::Label::new(block.job.clone())
        .wrap()
        .sense(egui::Sense::click())
        .layout_in_ui(ui);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), galley.text())
    });
    let hit = response
        .hover_pos()
        .and_then(|p| link_at(&galley, p - pos, &block.links));
    if ui.is_rect_visible(response.rect) {
        let color = ui.visuals().text_color();
        if selectable {
            egui::text_selection::LabelSelectionState::label_text_selection(
                ui,
                &response,
                pos,
                galley,
                color,
                egui::Stroke::NONE,
            );
        } else {
            ui.painter().galley(pos, galley, color);
        }
    }
    let url = &block.links[hit?].1;
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    response.clicked().then(|| url.clone())
}

/// Index of the link whose glyph is at `p` (galley coordinates).
fn link_at(
    galley: &egui::Galley,
    p: egui::Vec2,
    links: &[(Range<usize>, url::Url)],
) -> Option<usize> {
    let mut chars = 0;
    for row in &galley.rows {
        if row.rect.min.y <= p.y && p.y < row.rect.max.y {
            let i = row
                .glyphs
                .iter()
                .position(|g| g.pos.x <= p.x && p.x < g.max_x())?;
            let byte = galley.job.text.char_indices().nth(chars + i)?.0;
            return links.iter().position(|(r, _)| r.contains(&byte));
        }
        chars += row.char_count_including_newline();
    }
    None
}

/// Keep the original text and cursor offsets; only fenced lines change font.
pub(super) fn composer(ui: &egui::Ui, text: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let fence = line.starts_with("```");
        job.append(line, 0.0, format(14.0, fenced || fence));
        if fence {
            fenced = !fenced;
        }
    }
    job.wrap.max_width = width;
    ui.fonts(|f| f.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_rows_keep_geometry_after_scroll_and_timestamp_changes() {
        let (tx, _work) = mpsc::sync_channel(1);
        let (_result, rx) = mpsc::sync_channel(1);
        let mut cache = Cache {
            entries: VecDeque::new(),
            serial: 0,
            measurements: HashMap::new(),
            pending: true,
            tx,
            rx,
            clicked: None,
        };
        let ctx = egui::Context::default();
        let draw = |cache: &mut Cache, offset, timestamps, text: &str| {
            let mut height = 0.0;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(360.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        height = egui::ScrollArea::vertical()
                            .vertical_scroll_offset(offset)
                            .show(ui, |ui| {
                                for id in 0..100 {
                                    ui.horizontal_wrapped(|ui| {
                                        if timestamps {
                                            ui.label("12:34");
                                        }
                                        ui.label("Alice");
                                        cache.show(ui, id, text);
                                    });
                                }
                            })
                            .content_size
                            .y;
                    });
                },
            );
            height
        };
        let first = draw(&mut cache, 0.0, true, "short");
        for offset in [0.0, 500.0, 1000.0, 0.0] {
            assert!((draw(&mut cache, offset, true, "short") - first).abs() < 0.1);
        }
        let text = "some words wrapping across several lines ".repeat(5);
        draw(&mut cache, 0.0, true, &text);
        draw(&mut cache, 0.0, true, &text);
        let serial = cache.serial;
        let cached = draw(&mut cache, 0.0, false, &text);
        assert!(
            cache.serial > serial,
            "remaining width change must invalidate offscreen rows"
        );
        cache.measurements.clear();
        let fresh = draw(&mut cache, 0.0, false, &text);
        assert!(
            (cached - fresh).abs() < 0.1,
            "timestamp change kept stale wrapping"
        );
    }

    #[test]
    fn escaped_closing_markers_preserve_text() {
        assert_eq!(plain(r"*a\*b*"), "a*b");
        assert_eq!(plain(r"**a\**b**"), "a**b");
        assert_eq!(plain(r"~~a\~~b~~"), "a~~b");
        assert_eq!(plain(r"*a\\*"), r"a\");
        assert_eq!(plain(r"`a\*b`"), r"a\*b");
    }

    #[test]
    fn cold_languages_highlight_multiple_lines() {
        for (tag, code) in [
            ("rust", "let n = 42;\nlet m = 7;\n"),
            ("csharp", "var n = 42;\nvar m = 7;\n"),
            ("python", "n = 42\nm = 7\n"),
        ] {
            let syntaxes: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
                "../../assets/syntaxes.packdump"
            ))
            .unwrap();
            let mut blocks = parse(&format!("```{tag}\n{code}```"));
            highlight_with_retry(&mut blocks, &syntaxes, &highlight_theme());
            assert_eq!(blocks[0].job.text, code);
            assert!(blocks[0].job.sections.len() > 1, "{tag} stayed plain");
        }
    }

    #[test]
    fn offscreen_history_does_not_thrash_visible_cache() {
        let (tx, _work) = mpsc::sync_channel(1);
        let (_result, rx) = mpsc::sync_channel(1);
        let mut cache = Cache {
            entries: VecDeque::new(),
            serial: 0,
            measurements: HashMap::new(),
            pending: true,
            tx,
            rx,
            clicked: None,
        };
        let ctx = egui::Context::default();
        let text = "ordinary words ".repeat(130);
        let render = |cache: &mut Cache, width: f32, edited: bool| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 480.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for id in 0..100 {
                                cache.show(
                                    ui,
                                    id,
                                    if edited && id == 99 { "edited" } else { &text },
                                );
                            }
                        });
                    });
                },
            );
        };
        render(&mut cache, 640.0, false);
        render(&mut cache, 640.0, false);
        let serial = cache.serial;
        render(&mut cache, 640.0, false);
        assert_eq!(cache.serial, serial, "unchanged history was reparsed");
        render(&mut cache, 640.0, true);
        assert!(
            cache.serial > serial,
            "offscreen edits must invalidate height"
        );
        let serial = cache.serial;
        render(&mut cache, 400.0, true);
        assert!(cache.serial > serial, "resize must invalidate height");
        assert!(cache.entries.len() <= CACHE_SIZE);
    }
    #[test]
    fn dialect_and_snippets() {
        assert_eq!(plain("# Heading\n**bold** *italic* ~~gone~~ `x*y`\n> quote\n- one\n1. two\n```rust\nfn main() {}\n```"), "Heading bold italic gone x*y quote one two fn main() {}\n");
        let blocks = parse("## heading\n```unknown\n<evil>\n```");
        assert_eq!(blocks[0].job.sections[0].format.font_id.size, 20.0);
        assert_eq!(blocks[1].job.text, "<evil>\n");
        assert!(language("unknown").is_none());
    }
    #[test]
    fn hostile_and_malformed() {
        for text in [
            "*".repeat(100_000),
            "🦀".repeat(100_000),
            "x\n".repeat(100_000),
            "```rust\n".to_owned(),
            "[x](https://invalid) ![image](https://invalid) <script>hi</script>".to_owned(),
        ] {
            let blocks = parse(&text);
            assert!(blocks.len() <= MAX_BLOCKS);
            assert!(blocks.iter().map(|b| b.job.text.len()).sum::<usize>() <= MAX_BYTES);
        }
        assert_eq!(
            plain("<script>x</script> ![x](https://invalid)"),
            "<script>x</script> ![x](https://invalid)"
        );
        assert_eq!(plain("unclosed ** bold"), "unclosed ** bold");
    }
    #[test]
    fn cache_is_bounded_and_edits_replace_rendered_content() {
        let (tx, _work) = mpsc::sync_channel(1);
        let (_result, rx) = mpsc::sync_channel(1);
        let mut cache = Cache {
            entries: VecDeque::new(),
            serial: 0,
            measurements: HashMap::new(),
            pending: true,
            tx,
            rx,
            clicked: None,
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for id in 0..100 {
                    cache.show(ui, id, "```rust\nlet x = 1;\n```");
                }
                cache.show(ui, 99, "**edited**");
            });
        });
        assert!(cache.entries.len() <= CACHE_SIZE);
        assert!(cache.measurements.len() <= MEASUREMENT_LIMIT);
        assert!(cache.pending, "UI rendered with a stalled worker");
    }

    #[test]
    fn composer_keeps_text_and_shift_enter_newline() {
        let ctx = egui::Context::default();
        let mut text = "```csharp".to_owned();
        let mut frame = |events: Vec<egui::Event>| {
            let mut send = false;
            let modifiers = events
                .iter()
                .find_map(|e| match e {
                    egui::Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default();
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    modifiers,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut layouter = composer;
                        let response = ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .layouter(&mut layouter)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    egui::Key::Enter,
                                )),
                        );
                        response.request_focus();
                        send = response.has_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    });
                },
            );
            send
        };
        let key = |shift| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        };
        frame(vec![]);
        frame(vec![]);
        assert!(!frame(vec![key(true)]));
        frame(vec![egui::Event::Text("var n = 1;".into())]);
        assert!(frame(vec![key(false)]));
        assert_eq!(text, "```csharp\nvar n = 1;");
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let galley = composer(ui, "```\ncode\n```\nnormal", 400.0);
                assert_eq!(galley.job.text, "```\ncode\n```\nnormal");
                assert_eq!(
                    galley.job.sections[1].format.font_id.family,
                    egui::FontFamily::Monospace
                );
                assert_eq!(
                    galley.job.sections[3].format.font_id.family,
                    egui::FontFamily::Proportional
                );
            });
        });
    }

    #[test]
    fn bundled_languages_and_caps() {
        let syntaxes: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
            "../../assets/syntaxes.packdump"
        ))
        .unwrap();
        for tag in [
            "rs", "c", "cpp", "cs", "java", "js", "ts", "py", "go", "json", "toml", "yml", "sh",
            "sql", "html", "css",
        ] {
            assert!(
                syntaxes
                    .find_syntax_by_name(language(tag).unwrap())
                    .is_some(),
                "{tag}"
            );
        }
        let mut blocks = parse("```rust\nfn main() { let n = 42; }\n```");
        highlight(&mut blocks, &syntaxes, &highlight_theme());
        assert!(blocks[0].job.sections.len() > 1);
        let mut large = parse(&format!("```rust\n{}\n```", "a".repeat(5000)));
        highlight(&mut large, &syntaxes, &highlight_theme());
        assert_eq!(large[0].job.sections.len(), 1);
    }

    #[test]
    fn links_outside_code_keep_text_and_are_bounded() {
        let blocks = parse("see https://example.com/a_b_c, `https://code.example` **https://bold.example**\n```\nhttps://fenced.example\n```\n> https://quote.example/x\nhttps://user@evil.example javascript:alert(1) https://evil\u{202E}.example");
        let linked: Vec<&str> = blocks
            .iter()
            .flat_map(|b| b.links.iter().map(move |(r, _)| &b.job.text[r.clone()]))
            .collect();
        assert_eq!(
            linked,
            [
                "https://example.com/a_b_c",
                "https://bold.example",
                "https://quote.example/x"
            ]
        );
        assert_eq!(
            plain("see https://example.com/a_b_c, done"),
            "see https://example.com/a_b_c, done"
        );
        assert_eq!(
            parse(&"https://a.example ".repeat(200))[0].links.len(),
            MAX_LINKS
        );
        let capped = parse(&format!(
            "{}https://a.example",
            "x\n".repeat(MAX_BLOCKS + 10)
        ));
        assert!(
            capped.iter().all(|b| b.links.is_empty()),
            "lines past the block cap stay plain"
        );
    }

    #[test]
    fn clicking_a_link_reports_only_its_target() {
        let (tx, _work) = mpsc::sync_channel(1);
        let (_result, rx) = mpsc::sync_channel(1);
        let mut cache = Cache {
            entries: VecDeque::new(),
            serial: 0,
            measurements: HashMap::new(),
            pending: true,
            tx,
            rx,
            clicked: None,
        };
        let ctx = egui::Context::default();
        let click = |cache: &mut Cache, text: &str, at: egui::Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            for events in [
                vec![],
                vec![egui::Event::PointerMoved(at)],
                vec![button(true)],
                vec![button(false)],
            ] {
                let _ = ctx.run(
                    egui::RawInput {
                        events,
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(400.0, 200.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| cache.show(ui, 1, text));
                    },
                );
            }
            cache.take_clicked().map(|u| u.to_string())
        };
        // Panel content starts at (8, 8).
        assert_eq!(
            click(&mut cache, "https://example.com/x", egui::pos2(20.0, 16.0)).as_deref(),
            Some("https://example.com/x")
        );
        assert_eq!(
            click(
                &mut cache,
                "plain words https://example.com/x",
                egui::pos2(12.0, 16.0)
            ),
            None
        );
        assert_eq!(
            click(&mut cache, "https://example.com/x", egui::pos2(380.0, 16.0)),
            None,
            "beyond the text"
        );
    }
}
