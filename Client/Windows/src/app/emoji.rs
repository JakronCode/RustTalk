//! Offline emoji: bundled monochrome font, picker catalog and `:shortcode:` aliases.
use super::{icons, theme, RustTalkApp};
use eframe::egui;
use emojis::{Emoji, Group, SkinTone, UnicodeVersion};
use std::sync::{Arc, OnceLock};

const FONT: &str = "NotoEmoji-v3";
const MAX_QUERY: usize = 32;
const MAX_SHORTCODE: usize = 64;

/// Discord names that differ from the GitHub (gemoji) shortcodes in `emojis`.
const DISCORD_ALIASES: &[(&str, &str)] = &[
    ("clown", "clown_face"),
    ("nerd", "nerd_face"),
    ("slight_smile", "slightly_smiling_face"),
    ("slight_frown", "slightly_frowning_face"),
    ("frowning2", "frowning_face"),
    ("upside_down", "upside_down_face"),
    ("hugging", "hugs"),
    ("rolling_eyes", "roll_eyes"),
    ("zipper_mouth", "zipper_mouth_face"),
    ("money_mouth", "money_mouth_face"),
    ("thermometer_face", "face_with_thermometer"),
    ("head_bandage", "face_with_head_bandage"),
    ("cowboy", "cowboy_hat_face"),
    ("face_palm", "facepalm"),
    ("skull_crossbones", "skull_and_crossbones"),
    (
        "smiling_face_with_3_hearts",
        "smiling_face_with_three_hearts",
    ),
    ("face_with_monocle", "monocle_face"),
    ("face_with_symbols_over_mouth", "cursing_face"),
    ("face_with_hand_over_mouth", "hand_over_mouth"),
    ("crazy_face", "zany_face"),
    ("face_vomiting", "vomiting_face"),
    ("fingers_crossed", "crossed_fingers"),
    ("hand_splayed", "raised_hand_with_fingers_splayed"),
    ("vulcan", "vulcan_salute"),
    ("thumbup", "+1"),
    ("thumbdown", "-1"),
    ("heart_exclamation", "heavy_heart_exclamation"),
    ("spy", "detective"),
];

/// Places the bundled Noto Emoji before egui's older emoji font.
pub(super) fn add_font(fonts: &mut egui::FontDefinitions) {
    let data = egui::FontData::from_static(include_bytes!("../../assets/NotoEmoji-wght.ttf"));
    fonts.font_data.insert(FONT.into(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        let at = list
            .iter()
            .position(|n| n == "NotoEmoji-Regular")
            .unwrap_or(list.len().min(1));
        list.insert(at, FONT.into());
    }
}

fn lookup(code: &str) -> Option<&'static Emoji> {
    let code = DISCORD_ALIASES
        .iter()
        .find(|(alias, _)| *alias == code)
        .map_or(code, |(_, target)| target);
    emojis::get_by_shortcode(code)
}

/// Name shown on hover: the Discord alias if there is one.
fn shortcode(e: &'static Emoji) -> Option<&'static str> {
    let code = e.shortcode()?;
    Some(
        DISCORD_ALIASES
            .iter()
            .find(|(_, target)| *target == code)
            .map_or(code, |(alias, _)| alias),
    )
}

fn shortcode_len(rest: &str) -> Option<usize> {
    let end = rest
        .bytes()
        .take(MAX_SHORTCODE + 1)
        .position(|b| b == b':')?;
    (end > 0
        && rest.as_bytes()[..end]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_+-".contains(b)))
    .then_some(end)
}

/// Replaces known `:shortcodes:` outside code; unknown ones stay as typed.
/// Mirrors the chat dialect: fences at column zero, paired backticks, `\` escapes.
pub(super) fn replace_shortcodes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    let mut budget = 16 * 1024 * 8;
    for line in text.split_inclusive('\n') {
        if fenced {
            fenced = line.trim_end() != "```";
            out.push_str(line);
            continue;
        }
        if line.starts_with("```") {
            fenced = true;
            out.push_str(line);
            continue;
        }
        let body = line.trim_end_matches(['\r', '\n']);
        let body = ["### ", "## ", "# "]
            .iter()
            .find_map(|m| body.strip_prefix(m))
            .unwrap_or(body);
        let body = ["> ", "- ", "* ", "+ "]
            .iter()
            .find_map(|m| body.strip_prefix(m))
            .unwrap_or(body);
        let start = body.as_ptr() as usize - line.as_ptr() as usize;
        out.push_str(&line[..start]);
        let mut code = vec![false; body.len()];
        mark_inline_code(body, 0, &mut budget, &mut code);
        let mut pos = 0;
        while pos < body.len() {
            let rest = &body[pos..];
            if let Some(candidate) = rest.strip_prefix(':') {
                if let Some((len, e)) =
                    shortcode_len(candidate).and_then(|len| Some((len, lookup(&candidate[..len])?)))
                {
                    if !code[pos..pos + len + 2].iter().any(|&protected| protected) {
                        out.push_str(e.as_str());
                        pos += len + 2;
                        continue;
                    }
                }
            }
            let take = rest.chars().next().map_or(1, char::len_utf8);
            out.push_str(&rest[..take]);
            pos += take;
        }
        out.push_str(&line[start + body.len()..]);
    }
    out
}

// Match markdown::inline's delimiter precedence, recursion and work budget.
fn mark_inline_code(text: &str, depth: usize, budget: &mut usize, code: &mut [bool]) {
    if depth == 8 || *budget < text.len() {
        code.fill(true);
        return;
    }
    *budget -= text.len();
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        if let Some(escaped) = rest.strip_prefix('\\').and_then(|s| s.chars().next()) {
            if "*_~`\\".contains(escaped) {
                let take = 1 + escaped.len_utf8();
                pos += take;
                continue;
            }
        }
        if let Some(m) = ["`", "**", "__", "~~", "*", "_"]
            .into_iter()
            .find(|m| rest.starts_with(m))
        {
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
                    let start = pos + m.len();
                    if m == "`" {
                        code[pos..start + end + m.len()].fill(true);
                    } else {
                        mark_inline_code(
                            &tail[..end],
                            depth + 1,
                            budget,
                            &mut code[start..start + end],
                        );
                    }
                    pos += m.len() * 2 + end;
                    continue;
                }
            }
        }
        pos += rest.chars().next().map_or(1, char::len_utf8);
    }
}

/// Inserts `emoji` over the selection (or at the end); returns the new cursor
/// as a char index, or `None` if the text would exceed `max_chars`.
pub(super) fn insert(
    text: &mut String,
    range: Option<egui::text::CCursorRange>,
    emoji: &str,
    max_chars: usize,
) -> Option<usize> {
    let total = text.chars().count();
    let (a, b) = range.map_or((total, total), |r| {
        let (x, y) = (r.primary.index.min(total), r.secondary.index.min(total));
        (x.min(y), x.max(y))
    });
    let added = emoji.chars().count();
    if total - (b - a) + added > max_chars {
        return None;
    }
    let byte = |i: usize| text.char_indices().nth(i).map_or(text.len(), |(at, _)| at);
    let (start, end) = (byte(a), byte(b));
    text.replace_range(start..end, emoji);
    Some(a + added)
}

/// Picker entries: one code point (plus optional VS16) that the bundled font
/// draws without shaping, no skin-tone variants.
fn catalog() -> &'static [(Group, Vec<&'static Emoji>)] {
    static CATALOG: OnceLock<Vec<(Group, Vec<&'static Emoji>)>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        Group::iter()
            .map(|g| {
                let list = g
                    .emojis()
                    .filter(|e| e.skin_tone().is_none_or(|t| t == SkinTone::Default))
                    .filter(|e| e.unicode_version() <= UnicodeVersion::new(15, 1))
                    .filter(|e| e.as_str().chars().filter(|&c| c != '\u{fe0f}').count() == 1)
                    .collect();
                (g, list)
            })
            .collect()
    })
}

fn group_label(g: Group) -> (&'static str, &'static str) {
    match g {
        Group::SmileysAndEmotion => ("😀", "Smileys & Emotion"),
        Group::PeopleAndBody => ("👋", "People & Body"),
        Group::AnimalsAndNature => ("🐶", "Animals & Nature"),
        Group::FoodAndDrink => ("🍔", "Food & Drink"),
        Group::TravelAndPlaces => ("🚗", "Travel & Places"),
        Group::Activities => ("⚽", "Activities"),
        Group::Objects => ("💡", "Objects"),
        Group::Symbols => ("🔣", "Symbols"),
        Group::Flags => ("🏁", "Flags"),
    }
}

fn matches(e: &'static Emoji, query: &str) -> bool {
    e.name().contains(query)
        || e.shortcodes().any(|s| s.contains(query))
        || shortcode(e).is_some_and(|s| s.contains(query))
}

#[derive(Default)]
pub(super) struct EmojiPicker {
    pub open: bool,
    query: String,
    focus: bool,
    jump: Option<Group>,
    channel: u64,
}

impl RustTalkApp {
    pub(super) fn toggle_emoji_picker(&mut self, ch_id: u64) {
        if self.emoji.open {
            self.emoji = EmojiPicker::default();
        } else {
            self.close_gif_picker();
            self.emoji = EmojiPicker {
                open: true,
                focus: true,
                channel: ch_id,
                ..Default::default()
            };
        }
    }

    /// Emoji popup above the emoji button. Click inserts at the composer
    /// cursor; Shift+click keeps the picker open.
    pub(super) fn ui_emoji_picker(&mut self, ctx: &egui::Context) {
        if !self.emoji.open {
            return;
        }
        let ch_id = self.emoji.channel;
        if self.srv.selected_channel != Some(ch_id)
            || self.giphy.open
            || ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.emoji = EmojiPicker::default();
            return;
        }
        const W: f32 = 380.0;
        const H: f32 = 430.0;
        const CELL: f32 = 38.0;
        let screen = ctx.screen_rect();
        let anchor = self.emoji_button_rect.unwrap_or_else(|| {
            egui::Rect::from_min_size(
                screen.right_bottom() - egui::vec2(90.0, 70.0),
                egui::vec2(24.0, 24.0),
            )
        });
        let pos = egui::pos2(
            (anchor.right() - W + 8.0).clamp(
                screen.left() + 8.0,
                (screen.right() - W - 8.0).max(screen.left() + 8.0),
            ),
            (anchor.top() - H - 10.0).max(screen.top() + 8.0),
        );
        let mut chosen: Option<&'static str> = None;
        let mut hovered: Option<&'static Emoji> = None;
        let area = egui::Area::new(egui::Id::new("emoji_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::SIDEBAR)
                    .corner_radius(10.0)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(120),
                    })
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(W - 20.0);
                        ui.set_height(H - 20.0);
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.emoji.query)
                                .char_limit(MAX_QUERY)
                                .hint_text(format!(
                                    "{}  Search emoji",
                                    icons::regular::MAGNIFYING_GLASS
                                ))
                                .desired_width(f32::INFINITY),
                        );
                        if std::mem::take(&mut self.emoji.focus) {
                            resp.request_focus();
                        }
                        ui.horizontal(|ui| {
                            for (g, _) in catalog() {
                                let (icon, label) = group_label(*g);
                                if theme::ghost_button(
                                    ui,
                                    egui::RichText::new(icon).size(18.0).color(theme::MUTED),
                                )
                                .on_hover_text(label)
                                .clicked()
                                {
                                    self.emoji.query.clear();
                                    self.emoji.jump = Some(*g);
                                }
                            }
                        });
                        let query = self.emoji.query.trim().trim_matches(':').to_lowercase();
                        let jump = self.emoji.jump.take();
                        let results: Vec<(&str, Vec<&'static Emoji>)> = if query.is_empty() {
                            catalog()
                                .iter()
                                .map(|(g, list)| (group_label(*g).1, list.clone()))
                                .collect()
                        } else {
                            let found = catalog()
                                .iter()
                                .flat_map(|(_, list)| list.iter().copied())
                                .filter(|e| matches(e, &query))
                                .collect();
                            vec![("Search results", found)]
                        };
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(H - 130.0)
                            .show(ui, |ui| {
                                let width = ui.available_width();
                                let cols = ((width / CELL) as usize).max(1);
                                for (label, list) in &results {
                                    let (header, _) = ui.allocate_exact_size(
                                        egui::vec2(width, 22.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().text(
                                        header.left_center(),
                                        egui::Align2::LEFT_CENTER,
                                        *label,
                                        egui::FontId::proportional(12.0),
                                        theme::MUTED,
                                    );
                                    if jump.is_some_and(|g| group_label(g).1 == *label) {
                                        ui.scroll_to_rect(header, Some(egui::Align::TOP));
                                    }
                                    let rows = list.len().div_ceil(cols);
                                    let (outer, _) = ui.allocate_exact_size(
                                        egui::vec2(width, rows as f32 * CELL),
                                        egui::Sense::hover(),
                                    );
                                    for (i, &e) in list.iter().enumerate() {
                                        let min = outer.min
                                            + egui::vec2(
                                                (i % cols) as f32 * CELL,
                                                (i / cols) as f32 * CELL,
                                            );
                                        let rect =
                                            egui::Rect::from_min_size(min, egui::vec2(CELL, CELL));
                                        if !ui.is_rect_visible(rect) {
                                            continue;
                                        }
                                        let resp = ui.interact(
                                            rect,
                                            egui::Id::new(("emoji", e.as_str())),
                                            egui::Sense::click(),
                                        );
                                        if resp.hovered() {
                                            ui.painter().rect_filled(
                                                rect.shrink(2.0),
                                                6.0,
                                                theme::HOVER,
                                            );
                                            hovered = Some(e);
                                        }
                                        ui.painter().text(
                                            rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            e.as_str(),
                                            egui::FontId::proportional(24.0),
                                            theme::TEXT,
                                        );
                                        if resp.clicked() {
                                            chosen = Some(e.as_str());
                                        }
                                    }
                                    if list.is_empty() {
                                        ui.label(
                                            egui::RichText::new("No emoji found.")
                                                .color(theme::MUTED),
                                        );
                                    }
                                }
                            });
                        ui.separator();
                        ui.horizontal(|ui| match hovered {
                            Some(e) => {
                                ui.label(
                                    egui::RichText::new(e.as_str())
                                        .size(22.0)
                                        .color(theme::TEXT),
                                );
                                let name = shortcode(e)
                                    .map_or_else(|| e.name().to_string(), |s| format!(":{s}:"));
                                ui.label(egui::RichText::new(name).strong().color(theme::HEADER));
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("Shift+click to pick several")
                                        .size(12.0)
                                        .color(theme::DISABLED),
                                );
                            }
                        });
                    });
            });
        if let Some(emoji) = chosen {
            let id = super::views::composer_id(ch_id);
            let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
            match insert(
                &mut self.chat_input,
                state.cursor.char_range(),
                emoji,
                rusttalk_shared::protocol::MAX_CHAT_LEN,
            ) {
                Some(at) => {
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(at),
                        )));
                    state.store(ctx, id);
                }
                None => self.set_error("Message is at the length limit"),
            }
            ctx.memory_mut(|m| m.request_focus(id));
            if !ctx.input(|i| i.modifiers.shift) {
                self.emoji = EmojiPicker::default();
                return;
            }
        }
        let popup = area.response.rect;
        let clicked_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| {
                    !popup.contains(p) && !self.emoji_button_rect.is_some_and(|b| b.contains(p))
                })
        });
        if clicked_outside {
            self.emoji = EmojiPicker::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::text::{CCursor, CCursorRange};

    #[test]
    fn shortcodes_convert_outside_code() {
        assert_eq!(replace_shortcodes(":rofl: and :clown:"), "🤣 and 🤡");
        assert_eq!(replace_shortcodes(":thumbsup::+1::-1:"), "👍👍👎");
        assert_eq!(replace_shortcodes("x:heart:y"), "x❤️y");
        assert_eq!(replace_shortcodes("`:rofl:` :rofl:"), "`:rofl:` 🤣");
        assert_eq!(replace_shortcodes("\\`:rofl:`"), "\\`🤣`");
        assert_eq!(replace_shortcodes("``:rofl:`"), "``:rofl:`");
        assert_eq!(
            replace_shortcodes("```\n:rofl:\n```\n:rofl:"),
            "```\n:rofl:\n```\n🤣"
        );
        assert_eq!(
            replace_shortcodes("```rust\n:rofl:"),
            "```rust\n:rofl:",
            "unclosed fence stays code"
        );
        assert_eq!(
            replace_shortcodes(" ```\n:rofl:"),
            " ```\n🤣",
            "fences start at column zero"
        );
    }

    #[test]
    fn emphasis_boundaries_preserve_rendered_code() {
        for input in [
            "** `x ** `:rofl:`",
            "text * `x * `:rofl:`",
            "* ** `x ** `:rofl:`",
        ] {
            assert_eq!(replace_shortcodes(input), input);
        }
        assert_eq!(
            replace_shortcodes("**:rofl:** `:clown:`"),
            "**🤣** `:clown:`"
        );
    }

    #[test]
    fn alias_delimiters_do_not_change_original_code_boundaries() {
        assert_eq!(
            replace_shortcodes(":clown_face: `x _ `:rofl:`"),
            "🤡 `x _ `:rofl:`"
        );
    }

    #[test]
    fn unknown_and_malformed_stay_verbatim() {
        for s in [
            ":notanemoji:",
            ":ROFL:",
            "10:30:00",
            ":rofl",
            "rofl:",
            "::",
            ":",
            ":ro fl:",
            "a:b",
            ":\u{1F923}:",
            "é:🙂:ü",
        ] {
            assert_eq!(replace_shortcodes(s), s);
        }
        assert_eq!(replace_shortcodes("x:qq:rofl:"), "x:qq🤣");
        assert_eq!(replace_shortcodes(":unknown:rofl:"), ":unknown🤣");
        let long = format!(":{}:", "a".repeat(MAX_SHORTCODE + 1));
        assert_eq!(replace_shortcodes(&long), long);
        assert_eq!(replace_shortcodes("🙂 :joy:\r\n"), "🙂 😂\r\n");
    }

    #[test]
    fn conversion_never_grows_past_the_chat_limit() {
        let codes = emojis::iter()
            .flat_map(|e| e.shortcodes())
            .chain(DISCORD_ALIASES.iter().map(|(a, _)| *a));
        for code in codes {
            let src = format!(":{code}:");
            let out = replace_shortcodes(&src);
            assert_ne!(out, src, "{code} resolves");
            assert!(out.chars().count() <= src.chars().count(), "{code}");
        }
        let max = ":joy:".repeat(rusttalk_shared::protocol::MAX_CHAT_LEN / 5);
        assert!(
            replace_shortcodes(&max).chars().count() <= rusttalk_shared::protocol::MAX_CHAT_LEN
        );
    }

    #[test]
    fn discord_aliases_are_new_names_for_existing_emoji() {
        for (alias, target) in DISCORD_ALIASES {
            assert!(
                emojis::get_by_shortcode(alias).is_none(),
                "{alias} already exists"
            );
            assert!(
                emojis::get_by_shortcode(target).is_some(),
                "{target} missing"
            );
        }
        assert_eq!(shortcode(emojis::get("🤡").unwrap()), Some("clown"));
    }

    #[test]
    fn insert_replaces_selection_and_respects_limit() {
        let mut s = String::from("aé🙂b");
        assert_eq!(
            insert(&mut s, Some(CCursorRange::one(CCursor::new(2))), "🤣", 10),
            Some(3)
        );
        assert_eq!(s, "aé🤣🙂b");
        let sel = CCursorRange::two(CCursor::new(4), CCursor::new(1));
        assert_eq!(insert(&mut s, Some(sel), "❤️", 10), Some(3));
        assert_eq!(s, "a❤️b");
        assert_eq!(insert(&mut s, None, "🤡", 10), Some(5));
        assert_eq!(s, "a❤️b🤡");
        assert_eq!(
            insert(&mut s, Some(CCursorRange::one(CCursor::new(99))), "x", 6),
            Some(6)
        );
        assert_eq!(insert(&mut s, None, "🤡", 6), None);
        assert_eq!(s, "a❤️b🤡x");
    }

    #[test]
    fn picker_emoji_render_with_bundled_font() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        add_font(&mut fonts);
        ctx.set_fonts(fonts);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let total: usize = catalog().iter().map(|(_, l)| l.len()).sum();
            assert!(total > 1300, "catalog has {total}");
            for (g, list) in catalog() {
                // egui reports its own replacement glyph '◻' as missing, yet draws it.
                let missing: Vec<_> = list
                    .iter()
                    .filter(|e| {
                        !e.as_str().starts_with('◻')
                            && !ctx.fonts(|f| {
                                f.has_glyphs(&egui::FontId::proportional(14.0), e.as_str())
                            })
                    })
                    .map(|e| e.as_str())
                    .collect();
                assert!(missing.is_empty(), "{g:?}: {missing:?}");
                assert!(ctx
                    .fonts(|f| f.has_glyphs(&egui::FontId::proportional(14.0), group_label(*g).0)));
            }
            for e in ["🤣", "🤡", "🫡", "🫨"] {
                assert!(
                    ctx.fonts(|f| f.has_glyphs(&egui::FontId::monospace(14.0), e)),
                    "{e}"
                );
            }
        });
    }
}
