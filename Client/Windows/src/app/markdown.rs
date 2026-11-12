//! Deliberately small, bounded chat dialect; links, images and HTML stay inert text.
use super::theme;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat};
use std::{
    collections::VecDeque,
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
}

fn format(size: f32, mono: bool) -> TextFormat {
    TextFormat {
        font_id: if mono {
            FontId::monospace(size)
        } else {
            FontId::proportional(size)
        },
        color: theme::TEXT,
        ..Default::default()
    }
}

fn inline(text: &str, style: TextFormat, depth: usize, budget: &mut usize, job: &mut LayoutJob) {
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
        let marker = ["`", "**", "__", "~~", "*", "_"]
            .into_iter()
            .find(|m| rest.starts_with(m));
        if let Some(m) = marker {
            let tail = &rest[m.len()..];
            if *budget >= tail.len() {
                *budget -= tail.len();
                if let Some(end) = tail.find(m).filter(|&end| end > 0) {
                    let mut styled = style.clone();
                    match m {
                        "`" => {
                            styled.font_id = FontId::monospace(style.font_id.size);
                            styled.background = theme::DARK;
                        }
                        "**" | "__" => {
                            styled.color = theme::HEADER;
                            styled.extra_letter_spacing = 0.5;
                        }
                        "~~" => styled.strikethrough = egui::Stroke::new(1.0_f32, style.color),
                        _ => styled.italics = true,
                    }
                    if m == "`" {
                        job.append(&tail[..end], 0.0, styled);
                    } else {
                        inline(&tail[..end], styled, depth + 1, budget, job);
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
        inline(body, format(size, false), 0, &mut budget, &mut job);
        blocks.push(Block {
            job,
            language: None,
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

fn highlight(blocks: &mut [Block], syntaxes: &SyntaxSet, theme: &Theme) {
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
                return;
            }
            let Ok(spans) = highlighter.highlight_line(line, syntaxes) else {
                return;
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
    pending: bool,
    tx: mpsc::SyncSender<Work>,
    rx: mpsc::Receiver<(u64, Vec<Block>)>,
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
                highlight(&mut blocks, &syntaxes, &theme);
                if result.send((serial, blocks)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            entries: VecDeque::new(),
            serial: 0,
            pending: false,
            tx,
            rx,
        }
    }
}

impl Cache {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, id: u64, text: &str) {
        if let Ok((serial, blocks)) = self.rx.try_recv() {
            self.pending = false;
            if let Some(entry) = self.entries.iter_mut().find(|e| e.serial == serial) {
                entry.blocks = blocks;
            }
        }
        let source = bounded(text);
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
        if !entry.submitted
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
        ui.vertical(|ui| {
            for (i, block) in entry.blocks.iter().enumerate() {
                if let Some(tag) = &block.language {
                    ui.push_id((id, i), |ui| {
                        egui::Frame::new()
                            .fill(theme::DARK)
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(tag).small().color(theme::MUTED));
                                    if ui.small_button("Copy").clicked() {
                                        ui.ctx().copy_text(block.job.text.clone());
                                    }
                                });
                                ui.add(egui::Label::new(block.job.clone()).wrap());
                            });
                    });
                } else {
                    ui.add(egui::Label::new(block.job.clone()).wrap());
                }
            }
            if text.len() > MAX_BYTES {
                ui.label("[message display truncated]");
            }
        });
        self.entries.push_back(entry);
        if self.entries.len() > CACHE_SIZE {
            self.entries.pop_front();
        }
    }
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
            pending: true,
            tx,
            rx,
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
        assert_eq!(cache.entries.len(), CACHE_SIZE);
        assert_eq!(cache.entries.back().unwrap().blocks[0].job.text, "edited");
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
}
