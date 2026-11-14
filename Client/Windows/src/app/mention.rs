//! @mention popup above the composer: members matching the "@query" at the
//! cursor, arrow keys to pick, Tab/Enter or click to complete, Esc to close.

use super::theme;
use super::views::composer_id;
use super::RustTalkApp;
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use rusttalk_shared::protocol::{MAX_CHAT_LEN, MAX_NAME_LEN};

/// Rows kept in the popup (the list scrolls).
const MAX_ROWS: usize = 50;
const ROW_H: f32 = 40.0;
const VISIBLE_ROWS: f32 = 6.0;

#[derive(Default)]
pub(super) struct MentionPopup {
    /// Highlighted row.
    selected: usize,
    /// Query the selection belongs to; a new query starts at the top.
    query: String,
    /// Composer text and '@' position that was completed or dismissed (Esc):
    /// the popup stays closed until either changes.
    dismissed: Option<(String, usize)>,
    /// The keyboard moved the selection: scroll it into view.
    scroll: bool,
    deferred: Vec<egui::Event>,
}

fn popup_key(event: &egui::Event) -> Option<egui::Key> {
    if let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = event
    {
        if modifiers.is_none()
            && matches!(
                key,
                egui::Key::ArrowDown
                    | egui::Key::ArrowUp
                    | egui::Key::Escape
                    | egui::Key::Tab
                    | egui::Key::Enter
            )
        {
            return Some(*key);
        }
    }
    None
}

/// The open popup for this frame.
struct Active {
    /// Char index of the '@'.
    at: usize,
    /// Char index of the cursor (end of the query).
    cursor: usize,
    names: Vec<String>,
}

/// The "@query" that ends at char index `cursor`: char index of the '@' and
/// the query. The '@' must start a word; the query may hold spaces (names
/// with spaces) but no line break.
fn mention_at(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let end = input
        .char_indices()
        .nth(cursor)
        .map_or(input.len(), |(i, _)| i);
    let before = &input[..end];
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    let starts_word = before[..at]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (starts_word && !query.contains(['\n', '\r']) && query.chars().count() <= MAX_NAME_LEN)
        .then(|| (before[..at].chars().count(), query))
}

/// Names starting with `query` (case-insensitive), sorted, at most `MAX_ROWS`.
fn candidates<'a>(names: impl Iterator<Item = &'a str>, query: &str) -> Vec<String> {
    let query = query.to_lowercase();
    let mut out: Vec<String> = names
        .filter(|n| n.to_lowercase().starts_with(&query))
        .map(str::to_owned)
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out.dedup();
    out.truncate(MAX_ROWS);
    out
}

/// Replaces chars `at..cursor` ("@query") with "@name " and returns the new
/// cursor (after the space), or `None` past `max_chars`. Text after the
/// cursor stays; an existing space there is reused.
fn complete(
    text: &mut String,
    at: usize,
    cursor: usize,
    name: &str,
    max_chars: usize,
) -> Option<usize> {
    let spaced = text.chars().nth(cursor).is_some_and(char::is_whitespace);
    let mention = if spaced {
        format!("@{name}")
    } else {
        format!("@{name} ")
    };
    let range = CCursorRange::two(CCursor::new(at), CCursor::new(cursor));
    super::emoji::insert(text, Some(range), &mention, max_chars)
        .map(|end| end + usize::from(spaced))
}

impl RustTalkApp {
    /// The popup, if the composer of `ch_id` was focused at the start of the
    /// frame and a member matches the "@query" at its cursor.
    fn active_mention(&mut self, ctx: &egui::Context, ch_id: u64, focused: bool) -> Option<Active> {
        if !focused || self.emoji.open || self.giphy.open {
            return None;
        }
        let range = egui::TextEdit::load_state(ctx, composer_id(ch_id))?
            .cursor
            .char_range()?;
        if range.primary.index != range.secondary.index {
            return None;
        }
        let cursor = range.primary.index;
        let (at, query) = mention_at(&self.chat_input, cursor)?;
        if self
            .mention
            .dismissed
            .as_ref()
            .is_some_and(|(t, a)| *a == at && *t == self.chat_input)
        {
            return None;
        }
        let me = self.srv.my_id;
        let names = candidates(
            self.srv
                .users
                .values()
                .filter(|u| u.id != me)
                .map(|u| u.name.as_str()),
            query,
        );
        if names.is_empty() {
            return None;
        }
        if self.mention.query != query {
            self.mention.query = query.to_owned();
            self.mention.selected = 0;
        }
        self.mention.selected = self.mention.selected.min(names.len() - 1);
        Some(Active { at, cursor, names })
    }

    /// Before the composer's TextEdit: arrows, Tab/Enter and Esc go to the
    /// popup instead of the text (and Enter does not send).
    pub(super) fn mention_keys(&mut self, ctx: &egui::Context, ch_id: u64, focused: bool) {
        if !focused {
            self.mention.deferred.clear();
            return;
        }
        let deferred = std::mem::take(&mut self.mention.deferred);
        ctx.input_mut(|i| {
            if !deferred.is_empty() {
                let fresh = std::mem::replace(&mut i.events, deferred);
                i.events.extend(fresh);
            }
            let mut edited = false;
            let split = i.events.iter().position(|event| {
                if popup_key(event).is_some() {
                    edited
                } else {
                    edited |= matches!(
                        event,
                        egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::Ime(_)
                            | egui::Event::PointerButton { .. }
                            | egui::Event::Key { pressed: true, .. }
                    );
                    false
                }
            });
            if let Some(at) = split {
                // Let TextEdit apply earlier edits before evaluating the next popup action.
                self.mention.deferred = i.events.split_off(at);
            }
        });
        if !self.mention.deferred.is_empty() {
            ctx.request_repaint();
        }
        let Some(m) = self.active_mention(ctx, ch_id, focused) else {
            return;
        };
        let keys = ctx.input_mut(|i| {
            let mut keys = Vec::new();
            i.events.retain(|event| {
                if let Some(key) = popup_key(event) {
                    keys.push(key);
                    return false;
                }
                true
            });
            keys
        });
        let n = m.names.len();
        // Consume the whole popup-key batch so a second Enter cannot send this frame.
        for key in keys {
            match key {
                egui::Key::ArrowDown => {
                    self.mention.selected = (self.mention.selected + 1) % n;
                    self.mention.scroll = true;
                }
                egui::Key::ArrowUp => {
                    self.mention.selected = (self.mention.selected + n - 1) % n;
                    self.mention.scroll = true;
                }
                egui::Key::Escape => {
                    self.mention.dismissed = Some((self.chat_input.clone(), m.at));
                    self.mention
                        .deferred
                        .retain(|event| popup_key(event).is_none());
                    break;
                }
                egui::Key::Tab | egui::Key::Enter => {
                    let name = m.names[self.mention.selected].clone();
                    self.accept_mention(ctx, ch_id, &m, &name);
                    break;
                }
                _ => {}
            }
        }
    }

    fn accept_mention(&mut self, ctx: &egui::Context, ch_id: u64, m: &Active, name: &str) {
        // A mouse selection may happen while completion keys await the next frame.
        self.mention
            .deferred
            .retain(|event| popup_key(event).is_none());
        let id = composer_id(ch_id);
        match complete(&mut self.chat_input, m.at, m.cursor, name, MAX_CHAT_LEN) {
            Some(end) => {
                let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(end))));
                state.store(ctx, id);
                self.mention.dismissed = Some((self.chat_input.clone(), m.at));
            }
            None => self.set_error("Message is at the length limit"),
        }
        ctx.memory_mut(|mem| mem.request_focus(id));
    }

    /// After the composer: the member list right above it.
    pub(super) fn mention_popup(
        &mut self,
        ctx: &egui::Context,
        ch_id: u64,
        focused: bool,
        composer: egui::Rect,
    ) {
        let Some(m) = self.active_mention(ctx, ch_id, focused) else {
            return;
        };
        let id = composer_id(ch_id);
        // Read at the start of the next frame: Tab and Esc must not take focus away.
        ctx.memory_mut(|mem| {
            mem.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        let scroll = std::mem::take(&mut self.mention.scroll);
        // Small windows: keep the header and a row on screen.
        let list_h = (ROW_H * VISIBLE_ROWS)
            .min(composer.top() - ctx.screen_rect().top() - 60.0)
            .max(ROW_H);
        let mut pick = None;
        egui::Area::new(egui::Id::new("mention_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(composer.left_top() - egui::vec2(0.0, 6.0))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::SIDEBAR())
                    .corner_radius(8.0)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER()))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(100),
                    })
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(composer.width() - 16.0);
                        ui.label(
                            egui::RichText::new("Members")
                                .size(12.0)
                                .color(theme::MUTED()),
                        );
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(list_h)
                            .show(ui, |ui| {
                                for (i, name) in m.names.iter().enumerate() {
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), ROW_H),
                                        egui::Sense::click(),
                                    );
                                    if resp.hovered() && moved {
                                        self.mention.selected = i;
                                    }
                                    // On press: a release would come after the composer lost focus.
                                    if resp.contains_pointer() && pressed {
                                        pick = Some(name.clone());
                                    }
                                    if i == self.mention.selected {
                                        ui.painter().rect_filled(rect, 4.0, theme::SELECTED());
                                        if scroll {
                                            ui.scroll_to_rect(rect, None);
                                        }
                                    }
                                    let avatar = egui::Rect::from_center_size(
                                        rect.left_center() + egui::vec2(22.0, 0.0),
                                        egui::vec2(28.0, 28.0),
                                    );
                                    theme::paint_avatar(ui.painter(), avatar, name, false);
                                    ui.painter()
                                        .with_clip_rect(rect.intersect(ui.clip_rect()))
                                        .text(
                                            avatar.right_center() + egui::vec2(10.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            name,
                                            egui::FontId::proportional(15.0),
                                            theme::HEADER(),
                                        );
                                }
                            });
                    });
            });
        if let Some(name) = pick {
            self.accept_mention(ctx, ch_id, &m, &name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::servers::tests::{app, login};
    use super::*;
    use rusttalk_shared::protocol::UserInfo;

    const CH: u64 = 3;

    fn add_user(app: &mut RustTalkApp, id: u64, name: &str) {
        let user = UserInfo {
            id,
            name: name.into(),
            channel_id: 1,
            muted: false,
            deafened: false,
            sharing: false,
            audio_sharing: false,
            role: Default::default(),
            streams: vec![],
        };
        app.srv.users.insert(id, user);
    }

    /// App with the composer of `CH` holding `text`, cursor at char `cursor`.
    fn typed(text: &str, cursor: usize) -> (RustTalkApp, egui::Context) {
        let mut app = app();
        let _ = login(&mut app, "a.invalid:7331", 1);
        add_user(&mut app, 2, "Anna");
        add_user(&mut app, 3, "Anton");
        add_user(&mut app, 4, "Max Power");
        app.srv.selected_channel = Some(CH);
        app.chat_input = text.into();
        let ctx = egui::Context::default();
        select(&ctx, cursor, cursor);
        (app, ctx)
    }

    fn select(ctx: &egui::Context, from: usize, to: usize) {
        let mut state = egui::TextEdit::load_state(ctx, composer_id(CH)).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(from),
            CCursor::new(to),
        )));
        state.store(ctx, composer_id(CH));
    }

    /// One frame with `keys` pressed (no modifiers) through `mention_keys`;
    /// returns the keys left over for the composer.
    fn press(app: &mut RustTalkApp, ctx: &egui::Context, keys: &[egui::Key]) -> Vec<egui::Key> {
        let events = keys
            .iter()
            .map(|&key| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            })
            .collect();
        ctx.begin_pass(egui::RawInput {
            events,
            ..Default::default()
        });
        app.mention_keys(ctx, CH, true);
        let left = ctx.input(|i| keys.iter().copied().filter(|&k| i.key_pressed(k)).collect());
        let _ = ctx.end_pass();
        left
    }

    fn cursor(ctx: &egui::Context) -> usize {
        egui::TextEdit::load_state(ctx, composer_id(CH))
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .primary
            .index
    }

    #[test]
    fn keyboard_picks_and_completes_without_sending() {
        let (mut app, ctx) = typed("hi @an see you", 6);
        assert!(
            press(&mut app, &ctx, &[egui::Key::ArrowDown]).is_empty(),
            "arrows go to the popup"
        );
        assert_eq!(app.mention.selected, 1);
        assert!(
            press(&mut app, &ctx, &[egui::Key::Enter]).is_empty(),
            "Enter completes, it does not send"
        );
        assert_eq!(app.chat_input, "hi @Anton see you", "suffix kept");
        assert_eq!(
            cursor(&ctx),
            10,
            "cursor after the name and the reused space"
        );
        assert_eq!(
            press(&mut app, &ctx, &[egui::Key::Enter]),
            [egui::Key::Enter],
            "closed after completing: Enter sends again"
        );

        let (mut app, ctx) = typed("@max p", 6);
        press(&mut app, &ctx, &[egui::Key::ArrowUp]);
        assert_eq!(app.mention.selected, 0, "wraps around in a one-row list");
        assert!(press(&mut app, &ctx, &[egui::Key::Tab]).is_empty());
        assert_eq!(app.chat_input, "@Max Power ");
        assert_eq!(cursor(&ctx), 11);
    }

    #[test]
    fn escape_dismisses_until_the_text_changes() {
        let (mut app, ctx) = typed("@a", 2);
        assert!(
            press(&mut app, &ctx, &[egui::Key::Escape]).is_empty(),
            "Esc closes the popup, not the reply"
        );
        assert_eq!(
            press(&mut app, &ctx, &[egui::Key::Tab]),
            [egui::Key::Tab],
            "stays closed"
        );
        assert_eq!(app.chat_input, "@a");
        app.chat_input = "@an".into();
        select(&ctx, 3, 3);
        assert!(
            press(&mut app, &ctx, &[egui::Key::Tab]).is_empty(),
            "typing reopens it"
        );
        assert_eq!(app.chat_input, "@Anna ");
    }

    #[test]
    fn closed_while_unfocused_selecting_or_with_a_picker() {
        let (mut app, ctx) = typed("@a", 2);
        ctx.begin_pass(Default::default());
        assert!(
            app.active_mention(&ctx, CH, false).is_none(),
            "composer not focused"
        );
        app.emoji.open = true;
        assert!(
            app.active_mention(&ctx, CH, true).is_none(),
            "emoji picker open"
        );
        app.emoji.open = false;
        app.giphy.open = true;
        assert!(
            app.active_mention(&ctx, CH, true).is_none(),
            "GIF picker open"
        );
        app.giphy.open = false;
        select(&ctx, 0, 2);
        assert!(
            app.active_mention(&ctx, CH, true).is_none(),
            "text selected"
        );
        let _ = ctx.end_pass();
    }

    #[test]
    fn channel_and_server_switch_reset_the_popup() {
        let (mut app, ctx) = typed("@a", 2);
        press(&mut app, &ctx, &[egui::Key::ArrowDown, egui::Key::Escape]);
        app.select_channel(2);
        assert!(app.mention.dismissed.is_none() && app.mention.selected == 0);
        press(&mut app, &ctx, &[egui::Key::Escape]);
        app.reset_view_ui();
        assert!(app.mention.dismissed.is_none());
    }

    #[test]
    fn own_name_is_not_suggested() {
        let (mut app, ctx) = typed("@m", 2);
        ctx.begin_pass(Default::default());
        assert_eq!(
            app.active_mention(&ctx, CH, true).unwrap().names,
            ["Max Power"],
            "'me' is left out"
        );
        let _ = ctx.end_pass();
    }

    #[test]
    fn query_at_cursor() {
        assert_eq!(mention_at("hi @an", 6), Some((3, "an")));
        assert_eq!(mention_at("@", 1), Some((0, "")));
        assert_eq!(
            mention_at("@an and more", 3),
            Some((0, "an")),
            "text after the cursor is ignored"
        );
        assert_eq!(
            mention_at("@Max Po", 7),
            Some((0, "Max Po")),
            "names with spaces"
        );
        assert_eq!(mention_at("mail@x", 6), None, "not at a word start");
        assert_eq!(mention_at("@a\nb", 4), None, "no line breaks");
        assert_eq!(mention_at("no at", 5), None);
        assert_eq!(
            mention_at("ÄÖ @ü", 5),
            Some((3, "ü")),
            "char indices, not bytes"
        );
        assert_eq!(mention_at("@x", 99), Some((0, "x")), "cursor past the end");
        let long = format!("@{}", "a".repeat(MAX_NAME_LEN + 1));
        assert_eq!(
            mention_at(&long, long.chars().count()),
            None,
            "longer than any name"
        );
    }

    #[test]
    fn candidates_filter_sort_and_bound() {
        let users = ["bob", "Anna", "anton", "Bea", "Anna"];
        assert_eq!(candidates(users.iter().copied(), "AN"), ["Anna", "anton"]);
        assert_eq!(
            candidates(users.iter().copied(), ""),
            ["Anna", "anton", "Bea", "bob"]
        );
        assert!(candidates(users.iter().copied(), "z").is_empty());
        let many: Vec<String> = (0..200).map(|i| format!("user{i}")).collect();
        assert_eq!(
            candidates(many.iter().map(String::as_str), "u").len(),
            MAX_ROWS
        );
    }

    #[test]
    fn completion_keeps_surrounding_text() {
        let mut t = String::from("hey @an, look");
        assert_eq!(complete(&mut t, 4, 7, "Anna", 100), Some(10));
        assert_eq!(t, "hey @Anna , look");

        let mut t = String::from("ä @mä x");
        assert_eq!(
            complete(&mut t, 2, 5, "Mäx Pöwer", 100),
            Some(13),
            "reuses the space after the cursor"
        );
        assert_eq!(t, "ä @Mäx Pöwer x");

        let mut t = String::from("@");
        assert_eq!(complete(&mut t, 0, 1, "ネオ", 100), Some(4));
        assert_eq!(t, "@ネオ ");

        let mut t = String::from("1234 @a");
        assert_eq!(
            complete(&mut t, 5, 7, "abcdef", 10),
            None,
            "over the length limit"
        );
        assert_eq!(t, "1234 @a", "unchanged when refused");
    }
}
