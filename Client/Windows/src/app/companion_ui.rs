//! Companions in the UI: the one above the user panel (petting, boops,
//! snacks, happiness and bond, local time tracking), the adoption flow (name,
//! collar) and the settings page. Rules and saved data: [`crate::companion`].
//!
//! Controls: drag across it = pet, click = boop, bone button = snack.

use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Shape, Stroke, Vec2};

use super::{theme, RustTalkApp};
use crate::companion::{self, Companion, Emotion, Kind, Mood, Store};
use crate::config::CompanionStyle;

mod doodle;

/// The drawing style in use (set from the config every frame).
static DOODLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn doodle_style() -> bool {
    DOODLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// The companion's box above the user panel, in points.
const SIZE: Vec2 = vec2(104.0, 104.0);
/// The snack button left of it.
const SNACK: Vec2 = vec2(26.0, 26.0);
/// Save at the latest after this much new time together (seconds).
const SAVE_EVERY: f64 = 60.0;
const BLINK_EVERY: f64 = 4.2;
const BLINK_LEN: f64 = 0.13;
/// Adoption: bins in the alley, clicks on the right one until the companion
/// comes out, and how long after that the form shows.
const BINS: usize = 3;
const TRIES: u8 = 2;
const FORM_AT: f32 = 1.1;

/// A companion being adopted (the window is open while this is Some).
pub struct Adoption {
    kind: Kind,
    coat: u8,
    genes: u32,
    collar: [u8; 3],
    name: String,
    /// The bin it hides in.
    hide: usize,
    /// Clicks on that bin so far.
    tries: u8,
    /// Wrong bins that were looked into.
    opened: [bool; BINS],
    /// UI time of the last click on each bin (for the shaking).
    shake: [f64; BINS],
    /// UI time when it was found.
    found: Option<f64>,
}

impl Adoption {
    /// What you get is rolled from the user's seed: breed and colors are a
    /// surprise, and everyone gets their own. Name and collar are only
    /// suggestions.
    fn new(kind: Kind, seed: &str, slot: usize) -> Self {
        let g = companion::genes(seed, slot);
        Self {
            kind,
            coat: g[0] % kind.info().coats.len() as u8,
            genes: u32::from_le_bytes([g[1], g[2], g[3], g[4]]),
            collar: companion::COLLARS[g[5] as usize % companion::COLLARS.len()],
            name: companion::NAMES[g[6] as usize % companion::NAMES.len()].to_string(),
            hide: companion::random() as usize % BINS,
            tries: 0,
            opened: [false; BINS],
            shake: [f64::NEG_INFINITY; BINS],
            found: None,
        }
    }
}

/// Runtime state (the saved part is `store`).
pub struct State {
    pub store: Store,
    pub adoption: Option<Adoption>,
    /// Debug builds: show every coat and emotion.
    #[cfg(debug_assertions)]
    pub gallery: bool,
    mood: Option<Mood>,
    /// The companion the mood belongs to.
    mood_of: Option<u32>,
    last_tick: Option<Instant>,
    unsaved: f64,
    /// Normalized address of the server we're on (to notice arrivals).
    server: Option<String>,
    /// Arrived on a server: a happy hop once the UI knows the time.
    arrived: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            store: Store::load_from(&Store::path()),
            adoption: None,
            #[cfg(debug_assertions)]
            gallery: false,
            mood: None,
            mood_of: None,
            last_tick: None,
            unsaved: 0.0,
            server: None,
            arrived: false,
        }
    }
}

impl State {
    pub fn save(&mut self) {
        if self.store.companions.is_empty() {
            return; // never had one: no file
        }
        self.store.save_to(&Store::path());
        self.unsaved = 0.0;
    }

    /// Opens the adoption window (if there is room for a new companion).
    pub fn start_adoption(&mut self, kind: Kind) {
        if self.adoption.is_none() && self.store.can_adopt() {
            let seed = self.store.seed();
            self.adoption = Some(Adoption::new(kind, &seed, self.store.companions.len()));
        }
    }
}

fn collar_color(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl RustTalkApp {
    /// The companion that comes along (if companions are on and it still exists).
    fn active_companion(&self) -> Option<u32> {
        self.config.companion.filter(|_| self.config.companions).filter(|id| self.companion.store.get(*id).is_some())
    }

    /// Counts the time together while connected (call once per frame).
    pub(super) fn companion_tick(&mut self) {
        DOODLE.store(self.config.companion_style == CompanionStyle::Doodle, std::sync::atomic::Ordering::Relaxed);
        let id = match self.active_companion() {
            Some(id) if self.is_connected() => id,
            _ => {
                if self.companion.last_tick.take().is_some() {
                    self.companion.save();
                }
                self.companion.server = None;
                return;
            }
        };
        let now = Instant::now();
        // Capped: while shown, frames are never further apart than a blink.
        let dt = self.companion.last_tick.map_or(0.0, |t| now.duration_since(t).as_secs_f64().min(30.0));
        self.companion.last_tick = Some(now);
        let key = self.srv.key.clone();
        self.companion.store.spend(id, &key, &self.srv.server_settings.name, dt);
        if self.companion.server.as_deref() != Some(key.as_str()) {
            self.companion.server = Some(key);
            self.companion.arrived = true;
        }
        self.companion.unsaved += dt;
        if self.companion.unsaved >= SAVE_EVERY {
            self.companion.save();
        }
    }

    /// Draws the companion with its bottom right corner at `anchor`.
    pub(super) fn ui_companion(&mut self, ctx: &egui::Context, anchor: Pos2) {
        let Some(id) = self.active_companion() else { return };
        let total = vec2(SNACK.x + 4.0 + SIZE.x, SIZE.y);
        // Doodle dogs lean on the panel: their paws hang over its edge.
        let anchor = if doodle_style() { anchor + vec2(0.0, SIZE.y * (1.0 - doodle::LEDGE)) } else { anchor };
        let st = &mut self.companion;
        egui::Area::new(egui::Id::new("companion")).fixed_pos(anchor - total).order(egui::Order::Middle).show(ctx, |ui| {
            let now = ui.input(|i| i.time);
            let dt = ui.input(|i| i.stable_dt as f64);
            if st.mood_of != Some(id) {
                st.mood = None;
                st.mood_of = Some(id);
            }
            let mut mood = st.mood.take().unwrap_or_else(|| Mood::new(now));
            if std::mem::take(&mut st.arrived) {
                mood.cheer(now, 2.0);
            }
            let area = egui::Rect::from_min_size(ui.cursor().min, total);
            let hovered_area = ui.rect_contains_pointer(area);
            let me = st.store.get(id).expect("active").clone();

            let (snack_rect, rect, resp) = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let snack_rect = ui.allocate_exact_size(vec2(SNACK.x, SIZE.y), egui::Sense::hover()).0;
                    let (rect, resp) = ui.allocate_exact_size(SIZE, egui::Sense::click_and_drag());
                    (snack_rect, rect, resp)
                })
                .inner;

            // Petting = dragging across it; a click without moving = boop.
            if resp.dragged() && ui.input(|i| i.pointer.delta().length() > 0.3) {
                if mood.pet(now, dt) {
                    if let Some(c) = st.store.get_mut(id) {
                        c.petted();
                    }
                }
            } else if resp.clicked() {
                let before = mood.emotion(now, me.hunger);
                mood.boop(now, me.hunger);
                if mood.emotion(now, me.hunger) == Emotion::Angry && before != Emotion::Angry {
                    if let Some(c) = st.store.get_mut(id) {
                        c.annoyed();
                    }
                }
            }
            let cursor = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab };
            let emotion = mood.feel(now, &me);
            resp.on_hover_cursor(cursor).on_hover_text(format!("{} is {} - drag to pet, click to boop", me.name, emotion.label()));

            // Snack button: while hovered, or as a hint when it's hungry.
            if hovered_area || emotion == Emotion::Hungry {
                let btn = egui::Rect::from_min_size(pos2(snack_rect.left(), snack_rect.bottom() - SNACK.y - 4.0), SNACK);
                let r = ui.interact(btn, ui.id().with("snack"), egui::Sense::click());
                let fill = if r.hovered() { theme::SELECTED } else { theme::INPUT };
                ui.painter().circle_filled(btn.center(), SNACK.x / 2.0, fill);
                if emotion == Emotion::Hungry {
                    let pulse = ((now * 4.0).sin() * 0.5 + 0.5) as f32;
                    ui.painter().circle_stroke(btn.center(), SNACK.x / 2.0 + 1.0, Stroke::new(1.5_f32, theme::accent().gamma_multiply(0.4 + 0.6 * pulse)));
                }
                bone(ui.painter(), btn.center(), 7.0, -0.6);
                let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("Give {} a snack", me.name));
                if r.clicked() {
                    if mood.feed(now, me.hunger) {
                        if let Some(c) = st.store.get_mut(id) {
                            c.fed();
                        }
                    }
                }
            }

            let emotion = mood.feel(now, &me);
            let blink = matches!(emotion, Emotion::Content | Emotion::Hungry) && (now % BLINK_EVERY) < BLINK_LEN;
            let look = Look { emotion, blink, hovered: ui.rect_contains_pointer(rect), now: now as f32, eating: mood.eating_progress(now) };
            paint_companion_at(ui.painter(), rect, &me, &look, true);
            st.mood = Some(mood);

            // Smooth while something happens; otherwise only wake up to blink.
            let busy = look.hovered || !matches!(emotion, Emotion::Content | Emotion::Sad | Emotion::Sleepy | Emotion::Asleep);
            if busy {
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            } else if emotion == Emotion::Asleep {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            } else {
                let phase = now % BLINK_EVERY;
                let wait = if phase < BLINK_LEN { BLINK_LEN - phase } else { BLINK_EVERY - phase };
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.016)));
            }
        });
    }

    /// The adoption window: a poof with sparkles, then name + collar.
    pub(super) fn ui_adoption(&mut self, ctx: &egui::Context) {
        let Some(a) = self.companion.adoption.as_mut() else { return };
        let now = ctx.input(|i| i.time);
        // Seconds since the companion was found (negative = still hiding).
        let t = a.found.map_or(-1.0, |f| (now - f) as f32);
        let mut done: Option<bool> = None; // Some(true) = adopt, Some(false) = cancel

        egui::Window::new("adopt")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(380.0, 0.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let heading = if a.found.is_none() { "Who is hiding in the alley?" } else { "A new friend appeared!" };
                    ui.label(egui::RichText::new(heading).size(18.0).strong().color(theme::HEADER));
                });
                let (stage, _) = ui.allocate_exact_size(vec2(ui.available_width(), 200.0), egui::Sense::hover());
                // Look into the bins: the right one needs a few tries.
                if a.found.is_none() {
                    for (i, (rect, _)) in bin_spots(stage).into_iter().enumerate() {
                        let r = ui.interact(rect, ui.id().with(("bin", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clicked() {
                            a.shake[i] = now;
                            if i == a.hide {
                                a.tries += 1;
                                if a.tries >= TRIES {
                                    a.found = Some(now);
                                }
                            } else {
                                a.opened[i] = true;
                            }
                        }
                    }
                }
                let preview = Companion::new(a.kind, &a.name, a.coat, a.genes, a.collar);
                paint_alley(ui.painter(), stage, a, &preview, now);

                ui.add_space(4.0);
                let alpha = ((t - FORM_AT) / 0.35).clamp(0.0, 1.0);
                ui.multiply_opacity(alpha);
                ui.add_enabled_ui(alpha > 0.5, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(format!("A {} - what's their name?", look_name(&preview).to_lowercase())).color(theme::MUTED));
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Name").color(theme::HEADER));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(super::icons::regular::DICE_FIVE).on_hover_text("Another name").clicked() {
                                let current = a.name.clone();
                                while a.name == current {
                                    a.name = companion::pick(&companion::NAMES).to_string();
                                }
                            }
                            let edit = egui::TextEdit::singleline(&mut a.name).char_limit(companion::MAX_NAME).desired_width(ui.available_width());
                            let r = ui.add(edit);
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                done = Some(true);
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Collar").color(theme::HEADER));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(super::icons::regular::DICE_FIVE).on_hover_text("Surprise me").clicked() {
                                let current = a.collar;
                                while a.collar == current {
                                    a.collar = companion::pick(&companion::COLLARS);
                                }
                            }
                            ui.color_edit_button_srgb(&mut a.collar).on_hover_text("Any color");
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for c in companion::COLLARS.iter().rev() {
                                if swatch(ui, *c, a.collar == *c).clicked() {
                                    a.collar = *c;
                                }
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, &format!("{}  Welcome home!", super::icons::regular::HEART)).clicked() {
                            done = Some(true);
                        }
                        if ui.button("Later").clicked() {
                            done = Some(false);
                        }
                    });
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        match done {
            Some(true) => {
                let a = self.companion.adoption.take().expect("open");
                let id = self.companion.store.adopt(a.kind, &a.name, a.coat, a.genes, a.collar);
                self.companion.save();
                self.config.companion = Some(id);
                self.config.save();
                self.companion.arrived = true;
            }
            Some(false) => self.companion.adoption = None,
            None => {}
        }
    }
}

/// Round color button with a ring when selected.
fn swatch(ui: &mut egui::Ui, c: [u8; 3], selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
    let p = ui.painter();
    p.circle_filled(rect.center(), 7.0, collar_color(c));
    p.circle_stroke(rect.center(), 7.0, Stroke::new(1.0_f32, Color32::from_black_alpha(90)));
    if selected {
        p.circle_stroke(rect.center(), 9.0, Stroke::new(2.0_f32, theme::accent()));
    } else if resp.hovered() {
        p.circle_stroke(rect.center(), 9.0, Stroke::new(1.0_f32, theme::MUTED));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// What to draw.
struct Look {
    emotion: Emotion,
    blink: bool,
    hovered: bool,
    now: f32,
    eating: Option<f32>,
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Draws a companion in the current style. `on_panel`: it sits on the user
/// panel, whose top edge is the edge the doodle dogs lean on.
fn paint_companion_at(p: &egui::Painter, rect: egui::Rect, c: &Companion, look: &Look, on_panel: bool) {
    match c.kind {
        Kind::Dog if doodle_style() => doodle::paint(p, rect, c.coat, collar_color(c.collar), c.genes, look, !on_panel),
        Kind::Dog => paint_dog(p, rect, &DOG_COATS[c.coat as usize % DOG_COATS.len()], collar_color(c.collar), c.genes, look),
    }
}

fn paint_companion(p: &egui::Painter, rect: egui::Rect, c: &Companion, look: &Look) {
    paint_companion_at(p, rect, c, look, false);
}

/// What the companion is called in lists: its breed (doodle) or coat.
fn look_name(c: &Companion) -> String {
    if doodle_style() {
        doodle::breed_name(c.coat).to_string()
    } else {
        format!("{} {}", c.coat_name(), c.kind.info().species.to_lowercase())
    }
}

/// Colors of one dog coat (same order as `Kind::Dog`'s coat names).
struct Coat {
    fur: u32,
    /// Chest, muzzle, paws, underside of the tail.
    light: u32,
    /// Inside of the ears.
    ear: u32,
    /// Outline.
    line: u32,
    /// The spots above the eyes (same as `fur` = none).
    brow: u32,
}

const DOG_COATS: [Coat; 6] = [
    Coat { fur: 0xC8823F, light: 0xFFF6EA, ear: 0xE9B9A4, line: 0x2B1D14, brow: 0xC8823F },
    Coat { fur: 0xE3A25C, light: 0xFFF2DE, ear: 0xEDBBA2, line: 0x33200F, brow: 0xFFF2DE },
    Coat { fur: 0x302C32, light: 0xF4F1EC, ear: 0x8E7A7C, line: 0x121013, brow: 0xF4F1EC },
    Coat { fur: 0x3B3337, light: 0xE4AD76, ear: 0x8E7672, line: 0x151113, brow: 0xE4AD76 },
    Coat { fur: 0xF5F2ED, light: 0xFFFFFF, ear: 0xEBC7BE, line: 0x4A423F, brow: 0xF5F2ED },
    Coat { fur: 0x8D8990, light: 0xF3F0EB, ear: 0xC9AAA6, line: 0x1F1C21, brow: 0xF3F0EB },
];

/// Maps a design grid (units) into the box.
#[derive(Clone, Copy)]
struct Grid {
    o: Pos2,
    s: f32,
}

impl Grid {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.o + vec2(x, y) * self.s
    }
    fn v(&self, x: f32, y: f32) -> Vec2 {
        vec2(x, y) * self.s
    }
    fn r(&self, r: f32) -> f32 {
        r * self.s
    }
    /// A soft shape around circles given in grid units.
    fn soft(&self, circles: &[(f32, f32, f32)]) -> Vec<Pos2> {
        let c: Vec<(Pos2, f32)> = circles.iter().map(|&(x, y, r)| (self.p(x, y), self.r(r))).collect();
        soft(&c)
    }
}

/// A shape that can be drawn as a fill or, grown, as its outline.
enum Blob {
    Circle(Pos2, f32),
    Ellipse(Pos2, Vec2),
    Poly(Vec<Pos2>),
}

/// Draws `blobs` as one silhouette: first every outline (grown by `w`),
/// then every fill on top - so lines only show around the group's edge.
fn group(p: &egui::Painter, blobs: &[(Blob, Color32)], line: Color32, w: f32) {
    for (b, _) in blobs {
        match b {
            Blob::Circle(c, r) => {
                p.circle_filled(*c, r + w, line);
            }
            Blob::Ellipse(c, r) => {
                p.add(Shape::ellipse_filled(*c, *r + vec2(w, w), line));
            }
            Blob::Poly(pts) => {
                p.add(Shape::convex_polygon(pts.clone(), line, Stroke::new(w * 2.0, line)));
            }
        }
    }
    for (b, fill) in blobs {
        match b {
            Blob::Circle(c, r) => {
                p.circle_filled(*c, *r, *fill);
            }
            Blob::Ellipse(c, r) => {
                p.add(Shape::ellipse_filled(*c, *r, *fill));
            }
            Blob::Poly(pts) => {
                p.add(Shape::convex_polygon(pts.clone(), *fill, Stroke::NONE));
            }
        }
    }
}

/// A soft shape around a few circles (their convex hull): rounded
/// triangles for ears, legs, muzzles, ...
fn soft(circles: &[(Pos2, f32)]) -> Vec<Pos2> {
    let mut pts: Vec<Pos2> = Vec::with_capacity(circles.len() * 14);
    for &(c, r) in circles {
        for i in 0..14 {
            let a = i as f32 / 14.0 * std::f32::consts::TAU;
            pts.push(c + vec2(a.cos(), a.sin()) * r);
        }
    }
    // Andrew's monotone chain.
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut hull: Vec<Pos2> = Vec::with_capacity(pts.len());
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Pos2>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &q in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
                hull.pop();
            }
            hull.push(q);
        }
        hull.pop();
    }
    hull
}

/// Lighter (f > 1) or darker (f < 1).
fn tint(c: Color32, f: f32) -> Color32 {
    let m = |v: u8| ((v as f32) * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

/// A sitting dog seen from the front, drawn like in a (fairly realistic)
/// anime: real proportions, a proper muzzle and long front legs. Only the
/// silhouette has a line (in a darker fur tone); inside, forms come from a
/// shadow tone, a highlight tone and a few fur ticks.
/// `genes` gives each one traits of its own (ears, markings, socks, eyes).
fn paint_dog(p: &egui::Painter, rect: egui::Rect, coat: &Coat, collar: Color32, genes: u32, l: &Look) {
    // Individual traits.
    let shade = 0.95 + (genes & 0xFF) as f32 / 255.0 * 0.1;
    let floppy = (genes >> 8) & 3 == 0; // one in four has floppy ears
    let marking = (genes >> 10) % 3; // 0 = muzzle, 1 = muzzle + cheeks, 2 = with a blaze
    let socks = (genes >> 12) & 1 == 0;
    let ear_size = 0.9 + ((genes >> 14) & 0xFF) as f32 / 255.0 * 0.25;
    let iris = match (genes >> 22) % 8 {
        0 => rgb(0x6FA0CF), // the rare blue-eyed one
        1..=3 => rgb(0xB36A24),
        _ => rgb(0x7A4A22),
    };

    let fur = tint(rgb(coat.fur), shade);
    let fur_shadow = tint(fur, 0.8);
    let fur_deep = tint(fur, 0.66);
    let fur_hi = tint(fur, 1.09);
    let light = rgb(coat.light);
    let light_shadow = tint(light, 0.87);
    // Silhouette line: a dark tone of the coat, not black.
    let line = tint(fur, 0.48);
    let ink = rgb(coat.line);
    let e = l.emotion;
    let t = l.now;

    let hop = match e {
        Emotion::Happy => (t * 9.0).sin().abs() * 1.5,
        _ => 0.0,
    };
    let droop = match e {
        Emotion::Asleep => 5.0,
        Emotion::Sleepy | Emotion::Sad => 2.0,
        _ => 0.0,
    };
    let tilt = match e {
        Emotion::Full => -2.5,
        Emotion::Hungry => 1.5 + (t * 1.5).sin() * 0.6,
        _ => 0.0,
    };
    let wag = match e {
        Emotion::Happy | Emotion::Petted | Emotion::Eating => (t * 15.0).sin() * 0.3,
        Emotion::Content | Emotion::Hungry if l.hovered => (t * 6.0).sin() * 0.15,
        _ => 0.0,
    };
    let ears_up = match e {
        Emotion::Angry | Emotion::Petted => 0.25,
        Emotion::Asleep | Emotion::Sleepy | Emotion::Sad => 0.6,
        _ => 1.0,
    };
    let breath = (t * 1.8).sin() * 0.35;

    let s = rect.width() / 100.0;
    let w = (1.0 * s).max(0.85); // silhouette line width
    let hair = (0.8 * s).max(0.7); // fine inner strokes
    let g = Grid { o: rect.center() + vec2(0.0, -hop * s), s };
    // Head grid: origin between the eyes.
    let h = Grid { o: g.p(tilt, -27.0 + droop), s: s * 1.12 };
    let fill = |pts: Vec<Pos2>, c: Color32| {
        p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
    };

    // Shadow on the ground.
    p.add(Shape::ellipse_filled(rect.center() + vec2(2.0, 45.0) * s, vec2(35.0, 3.4) * s, Color32::from_black_alpha(60)));

    // --- Tail: curls up beside the body; feathered, light underneath ---
    let (sn, cs) = wag.sin_cos();
    let rot = |x: f32, y: f32| g.p(23.0, 36.0) + g.v(x * cs - y * sn, x * sn + y * cs);
    let tail_pts = [(2.0, -2.0, 5.5), (9.0, -7.0, 5.4), (14.0, -14.0, 5.0), (15.0, -22.0, 4.3), (11.0, -28.0, 3.2)];
    let chain = |shrink: f32, dx: f32, from: usize| -> Vec<Vec<Pos2>> {
        tail_pts[from..].windows(2).map(|pair| soft(&[(rot(pair[0].0 + dx, pair[0].1), g.r(pair[0].2 * shrink)), (rot(pair[1].0 + dx, pair[1].1), g.r(pair[1].2 * shrink))])).collect()
    };
    let mut tail: Vec<(Blob, Color32)> = chain(1.0, 0.0, 0).into_iter().map(|poly| (Blob::Poly(poly), fur)).collect();
    // Feathering: tufts sticking out along the outer edge and at the tip.
    for (x, y, dx, dy) in [(13.0f32, -5.0f32, 5.0f32, 1.5f32), (18.5, -13.0, 5.0, 0.0), (19.0, -21.0, 4.0, -2.5), (11.0, -30.0, -1.5, -4.5)] {
        tail.push((Blob::Poly(soft(&[(rot(x - 1.5, y - 2.5), g.r(1.4)), (rot(x - 1.5, y + 2.5), g.r(1.4)), (rot(x + dx, y + dy), g.r(0.8))])), fur));
    }
    group(p, &tail, line, w);
    for poly in chain(0.52, -1.5, 1) {
        fill(poly, light);
    }
    fill(soft(&[(rot(3.0, -1.0), g.r(3.4)), (rot(9.0, -5.0), g.r(3.0))]), fur_shadow);

    // --- Body: one silhouette (hind legs, torso, shoulders, front legs) ---
    let mut body: Vec<(Blob, Color32)> = Vec::new();
    for side in [-1.0f32, 1.0] {
        body.push((Blob::Ellipse(g.p(side * 18.0, 27.0), g.v(11.0, 14.0)), fur));
        body.push((Blob::Ellipse(g.p(side * 23.5, 41.0), g.v(8.5, 3.8)), light));
        body.push((Blob::Circle(g.p(side * 12.0, -4.0), g.r(8.5)), fur));
        body.push((Blob::Ellipse(g.p(side * 8.5, 41.0), g.v(6.0, 3.8)), light));
        // Fur tufts breaking the outline at the shoulder and the thigh.
        body.push((Blob::Poly(g.soft(&[(side * 19.0, -1.0, 1.6), (side * 23.5, 3.5, 0.9), (side * 18.5, 5.5, 1.6)])), fur));
        body.push((Blob::Poly(g.soft(&[(side * 27.0, 21.0, 1.6), (side * 32.0, 26.5, 0.9), (side * 27.5, 29.5, 1.6)])), fur));
        body.push((Blob::Poly(g.soft(&[(side * 28.0, 28.0, 1.5), (side * 31.5, 33.5, 0.9), (side * 27.0, 35.0, 1.5)])), fur));
    }
    body.push((Blob::Ellipse(g.p(0.0, 10.0), g.v(17.5, 24.0 + breath)), fur));
    group(p, &body, line, w);

    // Inside the body: shadow and light instead of lines.
    for side in [-1.0f32, 1.0] {
        // Crease between thigh and belly, a highlight on the thigh.
        p.add(Shape::ellipse_filled(g.p(side * 13.2, 30.0), g.v(4.2, 10.5), fur_shadow));
        p.add(Shape::ellipse_filled(g.p(side * 21.5, 21.0), g.v(4.6, 6.0), fur_hi));
        p.add(Shape::ellipse_filled(g.p(side * 13.5, -5.5), g.v(4.2, 3.4), fur_hi));
        // Fur ticks on the thigh.
        for (x, y) in [(22.0f32, 30.0f32), (25.0, 25.0)] {
            p.add(Shape::line(vec![g.p(side * (x - 1.5), y - 1.6), g.p(side * x, y), g.p(side * (x + 1.6), y - 1.4)], Stroke::new(hair, fur_shadow)));
        }
        // Hind paw: shaded top, toes.
        p.add(Shape::ellipse_filled(g.p(side * 22.0, 39.2), g.v(6.0, 1.6), light_shadow));
        for dx in [-3.0f32, 0.2, 3.4] {
            p.line_segment([g.p(side * (24.0 + dx), 40.6), g.p(side * (24.4 + dx), 43.2)], Stroke::new(hair, line));
        }
    }
    // The neck in the shade of the head.
    p.add(Shape::ellipse_filled(g.p(0.0, -9.0), g.v(15.5, 4.6), fur_shadow));
    // Belly in the shade between the front legs.
    fill(g.soft(&[(0.0, 26.0, 4.5), (0.0, 38.0, 6.0)]), fur_deep);

    // Chest: a long light bib with tufts along its edge, shaded under the chin.
    fill(g.soft(&[(0.0, -6.0, 11.0), (0.0, 10.0, 8.5), (0.0, 21.0, 5.0)]), light);
    for side in [-1.0f32, 1.0] {
        fill(g.soft(&[(side * 8.5, 0.0, 1.8), (side * 14.0, 4.0, 0.9), (side * 8.0, 6.0, 1.8)]), light);
        fill(g.soft(&[(side * 7.0, 9.0, 1.8), (side * 12.0, 14.0, 0.9), (side * 6.0, 15.0, 1.8)]), light);
        fill(g.soft(&[(side * 4.0, 18.0, 1.6), (side * 7.5, 23.5, 0.8), (side * 2.5, 23.0, 1.6)]), light);
    }
    fill(g.soft(&[(-2.0, 23.0, 1.6), (2.0, 23.0, 1.6), (0.0, 29.0, 0.9)]), light);
    p.add(Shape::ellipse_filled(g.p(0.0, -7.5), g.v(11.0, 4.6), light_shadow));
    for (x, y) in [(-5.0f32, 5.0f32), (4.5, 7.5), (-1.0, 12.5), (3.0, 17.0), (-4.0, 18.0)] {
        p.add(Shape::line(vec![g.p(x - 1.7, y - 1.9), g.p(x, y), g.p(x + 1.7, y - 1.9)], Stroke::new(hair, light_shadow)));
    }

    // Front legs in front of the chest: no outline, a shaded inner edge.
    for side in [-1.0f32, 1.0] {
        let x = side * 8.0;
        fill(g.soft(&[(x, 12.0, 4.6), (x, 36.0, 3.9)]), fur);
        fill(g.soft(&[(x - side * 2.6, 15.0, 1.5), (x - side * 2.3, 36.0, 1.4)]), fur_shadow);
        fill(g.soft(&[(x + side * 1.6, 15.0, 1.1), (x + side * 1.4, 30.0, 1.0)]), fur_hi);
        if socks {
            fill(g.soft(&[(x, 29.0, 4.1), (x, 36.5, 3.9)]), light);
            fill(g.soft(&[(x - side * 2.5, 30.0, 1.4), (x - side * 2.3, 36.5, 1.4)]), light_shadow);
            // A jagged top edge to the sock.
            fill(g.soft(&[(x - 2.0, 28.5, 1.3), (x - 0.5, 25.0, 0.7), (x + 1.0, 28.5, 1.3)]), light);
        }
        // Paw with toes.
        p.add(Shape::ellipse_filled(g.p(x + side * 0.5, 41.0), g.v(6.0, 3.8), light));
        p.add(Shape::ellipse_filled(g.p(x + side * 0.5, 39.0), g.v(4.6, 1.5), light_shadow));
        for dx in [-2.2f32, 0.0, 2.2] {
            p.line_segment([g.p(x + side * 0.5 + dx, 40.4), g.p(x + side * 0.5 + dx * 1.1, 43.6)], Stroke::new(hair, line));
        }
        // Elbow tick.
        p.add(Shape::line(vec![g.p(x + side * 4.6, 13.0), g.p(x + side * 5.4, 18.0), g.p(x + side * 4.8, 22.0)], Stroke::new(hair, fur_shadow)));
    }

    // --- Collar: band with a highlight, a ring and a tag ---
    let band = vec![g.p(-13.0, -7.0), g.p(-6.5, -3.4), g.p(0.0, -2.2), g.p(6.5, -3.4), g.p(13.0, -7.0)];
    p.add(Shape::line(band.clone(), Stroke::new(g.r(3.2) + w * 1.4, tint(collar, 0.5))));
    p.add(Shape::line(band.clone(), Stroke::new(g.r(3.2), collar)));
    p.add(Shape::line(band.iter().map(|q| *q + g.v(0.0, -0.8)).collect(), Stroke::new(g.r(0.8), tint(collar, 1.3))));
    let tag = g.p(0.0, 2.6);
    p.circle_stroke(g.p(0.0, -0.4), g.r(1.3), Stroke::new(hair, rgb(0x9A7B2E)));
    p.circle_filled(tag, g.r(3.0) + w * 0.7, rgb(0x8A6A1E));
    p.circle_filled(tag, g.r(3.0), rgb(0xE9C46A));
    p.circle_filled(tag + g.v(0.4, 0.5), g.r(1.8), rgb(0xD9AE4A));
    p.circle_filled(tag + g.v(-0.9, -1.0), g.r(0.9), Color32::from_white_alpha(210));

    // --- Head: skull, jaw, cheek tufts, ears - one silhouette ---
    let upright = |scale: f32, inset: f32| -> [Vec<Pos2>; 2] {
        [-1.0f32, 1.0].map(|side| {
            let tip = h.p(side * 14.0, -22.0 * scale).lerp(h.p(side * 25.0, -8.0), 1.0 - ears_up);
            let tip = tip + (h.p(side * 11.0, -6.0) - tip) * inset;
            soft(&[
                (h.p(side * (5.5 + inset * 6.0), -9.0 + inset * 2.0), h.r(2.4 * (1.0 - inset))),
                (h.p(side * (16.5 - inset * 4.0), -3.5), h.r(2.4 * (1.0 - inset))),
                (tip, h.r(1.7 * (1.0 - inset * 0.5))),
            ])
        })
    };
    let mut head: Vec<(Blob, Color32)> = Vec::new();
    if !floppy {
        for ear in upright(ear_size, 0.0) {
            head.push((Blob::Poly(ear), fur));
        }
    }
    head.push((Blob::Ellipse(h.p(0.0, 2.0), h.v(17.0, 15.0)), fur));
    head.push((Blob::Poly(h.soft(&[(-11.5, 8.0, 7.0), (11.5, 8.0, 7.0), (0.0, 13.5, 8.0)])), fur));
    for side in [-1.0f32, 1.0] {
        head.push((Blob::Poly(h.soft(&[(side * 15.5, 5.0, 1.7), (side * 22.0, 9.0, 0.9), (side * 16.0, 11.0, 1.7)])), fur));
        head.push((Blob::Poly(h.soft(&[(side * 13.5, 11.5, 1.7), (side * 18.5, 16.5, 0.9), (side * 11.0, 16.5, 1.7)])), fur));
    }
    // A little tuft on top.
    head.push((Blob::Poly(h.soft(&[(-2.6, -12.0, 1.3), (-0.5, -16.2, 0.8), (2.2, -12.2, 1.3)])), fur));
    group(p, &head, line, w);
    // Light on the forehead, shade under the jaw.
    p.add(Shape::ellipse_filled(h.p(0.0, -7.0), h.v(8.5, 3.8), fur_hi));
    p.add(Shape::ellipse_filled(h.p(0.0, 18.0), h.v(9.0, 2.8), fur_shadow));
    if floppy {
        // Folded ears hanging beside the head, a shade darker, with a fold.
        for side in [-1.0f32, 1.0] {
            let drop = 9.0 * ear_size + (1.0 - ears_up) * 3.0;
            group(p, &[(Blob::Poly(h.soft(&[(side * 10.0, -10.0, 3.6), (side * 18.5, -5.0, 4.2), (side * 20.5, drop, 3.8)])), fur_shadow)], line, w);
            fill(h.soft(&[(side * 12.0, -9.5, 1.6), (side * 18.0, -6.0, 1.8)]), fur);
            p.add(Shape::line(vec![h.p(side * 19.0, 0.0), h.p(side * 20.0, drop - 2.0)], Stroke::new(hair, fur_deep)));
        }
    } else {
        for (i, ear) in upright(ear_size, 0.38).into_iter().enumerate() {
            let side = if i == 0 { -1.0 } else { 1.0 };
            fill(ear, rgb(coat.ear));
            // Fur inside the ear.
            for k in [0.0f32, 1.0] {
                let from = h.p(side * (9.5 + k * 2.2), -7.5);
                p.line_segment([from, from + h.v(side * 1.0, -4.0 - k)], Stroke::new(hair, light));
            }
        }
        // The ears sit in a bit of shade where they meet the head.
        for side in [-1.0f32, 1.0] {
            p.add(Shape::ellipse_filled(h.p(side * 11.0, -8.6), h.v(5.0, 1.3), fur_shadow));
        }
    }
    // Markings: light muzzle, maybe light cheeks or a blaze up the forehead.
    if marking == 1 {
        fill(h.soft(&[(-11.0, 9.0, 6.0), (11.0, 9.0, 6.0), (0.0, 14.0, 7.0)]), light);
    }
    if marking == 2 {
        fill(h.soft(&[(0.0, -10.0, 2.0), (0.0, 2.0, 4.0)]), light);
    }
    fill(h.soft(&[(0.0, 1.5, 4.6), (-5.8, 11.0, 6.6), (5.8, 11.0, 6.6)]), light);
    p.add(Shape::ellipse_filled(h.p(0.0, 16.6), h.v(6.2, 1.8), light_shadow));
    if coat.brow != coat.fur {
        for side in [-1.0f32, 1.0] {
            fill(h.soft(&[(side * 6.8, -6.0, 1.5), (side * 9.6, -6.6, 1.3)]), rgb(coat.brow));
        }
    }
    // Whisker dots and cheek fur ticks.
    for side in [-1.0f32, 1.0] {
        for (x, y) in [(4.3f32, 12.3f32), (6.4, 11.4), (6.0, 13.6)] {
            p.circle_filled(h.p(side * x, y), h.r(0.42), tint(light, 0.6));
        }
        p.add(Shape::line(vec![h.p(side * 14.0, 4.0), h.p(side * 15.6, 6.4), h.p(side * 13.8, 7.0)], Stroke::new(hair, fur_shadow)));
    }

    // Eyes: iris, pupil, a shine and a lid line.
    let lid = Stroke::new((1.3 * s).max(1.0), ink);
    let arc = |c: Pos2, wdt: f32, hgt: f32, up: bool| -> Vec<Pos2> {
        (0..=8)
            .map(|i| {
                let a = std::f32::consts::PI * i as f32 / 8.0;
                c + h.v(-wdt * a.cos(), if up { -hgt * a.sin() } else { hgt * a.sin() })
            })
            .collect()
    };
    let eyeball = |c: Pos2, size: f32, squint: f32| {
        p.add(Shape::ellipse_filled(c, h.v(2.7 * size, 3.1 * size * squint), ink));
        p.add(Shape::ellipse_filled(c + h.v(0.0, 0.2), h.v(2.1 * size, 2.5 * size * squint), iris));
        p.add(Shape::ellipse_filled(c + h.v(0.0, 0.3), h.v(1.15 * size, 1.5 * size * squint), rgb(0x150E0B)));
        p.circle_filled(c + h.v(-0.9 * size, -1.1 * size), h.r(0.95 * size), Color32::WHITE);
        p.circle_filled(c + h.v(1.0 * size, 1.2 * size), h.r(0.45 * size), Color32::from_white_alpha(200));
        p.add(Shape::line(arc(c + h.v(0.0, -1.0 * squint), 3.2 * size, 2.4 * size * squint, true), lid));
    };
    for side in [-1.0f32, 1.0] {
        let c = h.p(side * 8.0, 0.0);
        match e {
            Emotion::Petted | Emotion::Asleep => {
                p.add(Shape::line(arc(c + h.v(0.0, -0.4), 3.0, 1.5, false), lid));
            }
            Emotion::Sleepy => {
                fill(arc(c + h.v(0.0, -0.2), 2.5, 2.3, false), ink);
                fill(arc(c + h.v(0.0, -0.2), 1.9, 1.7, false), iris);
                p.line_segment([c + h.v(-3.3, -0.2), c + h.v(3.3, -0.2)], lid);
            }
            _ if l.blink => {
                p.add(Shape::line(arc(c, 2.8, 1.0, false), lid));
            }
            Emotion::Hungry => eyeball(c, 1.25, 1.0),
            Emotion::Angry => {
                eyeball(c + h.v(0.0, 0.8), 0.85, 0.75);
                p.line_segment([c + h.v(-side * 4.4, -4.4), c + h.v(side * 3.4, -1.6)], Stroke::new(h.r(1.7), ink));
            }
            Emotion::Full => eyeball(c + h.v(-1.2, 0.0), 0.95, 0.9),
            Emotion::Happy | Emotion::Eating => eyeball(c, 1.0, 0.85),
            Emotion::Sad => {
                eyeball(c + h.v(0.0, 0.6), 1.15, 0.9);
                p.line_segment([c + h.v(-side * 3.0, -5.6), c + h.v(side * 3.6, -3.8)], Stroke::new(h.r(1.2), ink));
            }
            Emotion::Content => eyeball(c, 1.0, 1.0),
        }
    }

    // Nose: big and dark, with nostrils and a shine.
    fill(h.soft(&[(-2.9, 6.6, 2.3), (2.9, 6.6, 2.3), (0.0, 9.3, 2.0)]), ink);
    p.add(Shape::ellipse_filled(h.p(-1.0, 5.7), h.v(1.7, 0.8), Color32::from_white_alpha(130)));
    for side in [-1.0f32, 1.0] {
        p.circle_filled(h.p(side * 1.7, 8.2), h.r(0.55), Color32::from_black_alpha(140));
    }

    // Mouth.
    let thin = Stroke::new((0.9 * s).max(0.8), ink);
    let lips = |p: &egui::Painter| {
        p.line_segment([h.p(0.0, 11.2), h.p(0.0, 13.4)], thin);
        p.add(Shape::line(vec![h.p(-6.6, 13.6), h.p(-4.8, 15.0), h.p(-2.4, 15.2), h.p(0.0, 13.4), h.p(2.4, 15.2), h.p(4.8, 15.0), h.p(6.6, 13.6)], thin));
    };
    let open = |p: &egui::Painter, depth: f32, tongue: bool| {
        p.add(Shape::convex_polygon(h.soft(&[(-3.8, 15.0, 2.2), (3.8, 15.0, 2.2), (0.0, 15.0 + depth, 3.4)]), light, Stroke::new(w * 2.0, line)));
        fill(h.soft(&[(-3.2, 15.0, 1.6), (3.2, 15.0, 1.6), (0.0, 14.2 + depth, 2.4)]), rgb(0x6E2430));
        if tongue {
            let tng = h.soft(&[(-1.6, 16.0, 2.0), (1.6, 16.0, 2.0), (0.0, 17.5 + depth, 2.6)]);
            p.add(Shape::convex_polygon(tng, rgb(0xF08FA0), Stroke::new(hair, rgb(0xB8566A))));
            p.line_segment([h.p(0.0, 16.5), h.p(0.0, 17.0 + depth)], Stroke::new(hair, rgb(0xB8566A)));
            p.add(Shape::ellipse_filled(h.p(-0.9, 17.0 + depth * 0.5), h.v(0.6, 1.4), Color32::from_white_alpha(90)));
        }
        lips(p);
    };
    match e {
        Emotion::Happy => open(p, 5.0, true),
        Emotion::Petted => open(p, 3.0, true),
        Emotion::Eating => {
            if (t * 13.0).sin() > 0.0 {
                open(p, 3.5, false);
            } else {
                lips(p);
            }
        }
        Emotion::Angry => {
            // A snarl: lifted lips and teeth.
            p.add(Shape::convex_polygon(vec![h.p(-6.0, 13.6), h.p(6.0, 13.6), h.p(5.0, 16.4), h.p(-5.0, 16.4)], Color32::WHITE, Stroke::new(thin.width, ink)));
            for x in [-3.0f32, 0.0, 3.0] {
                p.line_segment([h.p(x, 13.8), h.p(x, 16.2)], Stroke::new(thin.width * 0.8, ink));
            }
            p.line_segment([h.p(0.0, 11.2), h.p(0.0, 13.4)], thin);
        }
        Emotion::Sleepy if (t % 9.0) < 1.5 => open(p, 6.0, true), // a yawn
        Emotion::Full => {
            p.line_segment([h.p(0.0, 11.2), h.p(0.0, 13.4)], thin);
            p.add(Shape::line(vec![h.p(-6.0, 14.6), h.p(0.0, 13.4), h.p(6.0, 13.8)], thin));
        }
        _ => lips(p),
    }

    // Extras around it.
    match e {
        Emotion::Petted => {
            for (i, (x, delay)) in [(-28.0f32, 0.0f32), (27.0, 0.45), (-2.0, 0.9)].into_iter().enumerate() {
                let phase = ((t + delay) % 1.4) / 1.4;
                let c = h.p(x + (t * 3.0 + i as f32).sin() * 2.0, -22.0 - phase * 12.0);
                heart(p, c, g.r(3.6 + i as f32 * 0.6), rgb(0xFF6FAE).gamma_multiply(1.0 - phase));
            }
        }
        Emotion::Asleep => {
            for i in 0..3 {
                let phase = ((t * 0.5 + i as f32 / 3.0) % 1.0).clamp(0.0, 1.0);
                let pos = h.p(20.0 + phase * 9.0, -14.0 - phase * 14.0);
                let alpha = (1.0 - phase) * (phase * 4.0).min(1.0);
                p.text(pos, egui::Align2::CENTER_CENTER, "z", egui::FontId::proportional(h.r(8.0 + phase * 6.0)), theme::MUTED.gamma_multiply(alpha));
            }
        }
        Emotion::Angry => {
            let c = h.p(21.0, -13.0);
            let st = Stroke::new(h.r(2.0), rgb(0xFF4D6D));
            for (dx, dy) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                let corner = c + h.v(dx * 1.8, dy * 1.8);
                p.add(Shape::line(vec![corner + h.v(dx * 3.6, 0.0), corner, corner + h.v(0.0, dy * 3.6)], st));
            }
        }
        Emotion::Hungry => {
            let c = h.p(-30.0, -14.0);
            let bubble = theme::HEADER.gamma_multiply(0.94);
            p.circle_filled(h.p(-19.0, -6.0), h.r(1.6), bubble);
            p.circle_filled(h.p(-22.5, -9.5), h.r(2.3), bubble);
            p.add(Shape::ellipse_filled(c, h.v(11.0, 7.5), bubble));
            bone(p, c, h.r(6.5), 0.0);
        }
        _ => {}
    }

    if let Some(progress) = l.eating {
        let left = (1.0 - progress).max(0.0);
        if left > 0.05 {
            bone(p, h.p(7.0, 19.0), h.r(7.0) * left, 0.3);
        }
        for i in 0..3 {
            let k = ((t * 3.0 + i as f32 * 0.37) % 1.0).clamp(0.0, 1.0);
            p.circle_filled(h.p(-4.0 + i as f32 * 4.0, 20.0 + k * 12.0), h.r(0.9), rgb(0xE8D3AE).gamma_multiply(1.0 - k));
        }
    }
}

/// Where the bins of the adoption scene stand: (rect to click, ground point).
fn bin_spots(stage: egui::Rect) -> [(egui::Rect, Pos2); BINS] {
    let ground = stage.bottom() - 24.0;
    [-1.0f32, 0.0, 1.0].map(|k| {
        let base = pos2(stage.center().x + k * 104.0, ground);
        (egui::Rect::from_min_max(base + vec2(-34.0, -100.0), base + vec2(34.0, 6.0)), base)
    })
}

/// The adoption scene: a back alley at night with three bins. One of them
/// rustles; when the companion is found it jumps out in a poof of sparks.
fn paint_alley(p: &egui::Painter, stage: egui::Rect, a: &Adoption, c: &Companion, now: f64) {
    let accent = theme::accent();
    let gold = rgb(0xFFD36E);
    let found = a.found.map(|f| (now - f) as f32);
    let t = now as f32;
    let p = &p.with_clip_rect(stage.expand(2.0));

    // Wall, a lamp's light cone, the ground.
    p.rect_filled(stage, 10.0, rgb(0x191525));
    for row in 0..6 {
        for col in 0..9 {
            let x = stage.left() + col as f32 * 44.0 - if row % 2 == 0 { 0.0 } else { 22.0 };
            let y = stage.top() + 8.0 + row as f32 * 26.0;
            if (row * 7 + col * 13) % 5 < 2 {
                p.rect_filled(egui::Rect::from_min_size(pos2(x, y), vec2(38.0, 20.0)), 2.0, rgb(0x201B2F));
            }
        }
    }
    let lamp = pos2(stage.right() - 46.0, stage.top() + 16.0);
    p.add(Shape::convex_polygon(vec![lamp, lamp + vec2(-120.0, 170.0), lamp + vec2(70.0, 170.0)], gold.gamma_multiply(0.05), Stroke::NONE));
    p.add(Shape::convex_polygon(vec![lamp, lamp + vec2(-70.0, 170.0), lamp + vec2(40.0, 170.0)], gold.gamma_multiply(0.05), Stroke::NONE));
    p.rect_filled(egui::Rect::from_center_size(lamp + vec2(0.0, -3.0), vec2(26.0, 7.0)), 3.0, rgb(0x2E2840));
    p.circle_filled(lamp + vec2(0.0, 2.0), 4.0, gold);
    let ground = stage.bottom() - 24.0;
    p.rect_filled(egui::Rect::from_min_max(pos2(stage.left(), ground - 6.0), stage.max), egui::CornerRadius { nw: 0, ne: 0, sw: 10, se: 10 }, rgb(0x120F1B));
    p.line_segment([pos2(stage.left() + 10.0, ground - 6.0), pos2(stage.right() - 10.0, ground - 6.0)], Stroke::new(1.0_f32, rgb(0x2A2440)));

    let spots = bin_spots(stage);
    for (i, (_, base)) in spots.iter().enumerate() {
        let hiding = i == a.hide;
        let since = (now - a.shake[i]) as f32;
        // A click shakes the bin hard; the right one also rustles by itself.
        let mut dx = if since < 0.5 { (since * 42.0).sin() * 5.0 * (1.0 - since / 0.5) } else { 0.0 };
        let rustling = hiding && found.is_none() && ((t * 0.55) % 1.0) < 0.3;
        if rustling {
            dx += (t * 46.0).sin() * 1.8;
        }
        let lid = if hiding && found.is_some() {
            Lid::Flying(found.unwrap_or(0.0))
        } else if a.opened[i] {
            Lid::Off
        } else if hiding && a.tries > 0 {
            Lid::Lifted(7.0 + (t * 5.0).sin() * 1.5)
        } else {
            Lid::Closed(if since < 0.5 || rustling { (t * 38.0).sin() * 0.12 } else { 0.0 })
        };
        let origin = *base + vec2(dx, 0.0);
        paint_bin(p, origin, &lid);
        // Peeking out after the first try: ears and two shining eyes.
        if hiding && a.tries > 0 && found.is_none() {
            let fur = rgb(DOG_COATS[c.coat as usize % DOG_COATS.len()].fur);
            let rim = origin + vec2(0.0, -78.0);
            for side in [-1.0f32, 1.0] {
                let ear = soft(&[(rim + vec2(side * 6.0, 1.0), 2.0), (rim + vec2(side * 16.0, 1.0), 2.0), (rim + vec2(side * 12.0, -9.0 - (t * 5.0).sin() * 1.5), 1.6)]);
                p.add(Shape::convex_polygon(ear, fur, Stroke::new(1.0_f32, tint(fur, 0.5))));
                p.circle_filled(rim + vec2(side * 7.0, 6.0), 1.8, Color32::WHITE);
            }
        }
        // Motion lines while it rustles.
        if rustling || since < 0.5 {
            for side in [-1.0f32, 1.0] {
                for (k, y) in [-58.0f32, -40.0, -22.0].into_iter().enumerate() {
                    let wob = (t * 30.0 + k as f32).sin() * 2.0;
                    let from = origin + vec2(side * (34.0 + wob), y);
                    p.line_segment([from, from + vec2(side * 7.0, -2.0 + k as f32 * 2.0)], Stroke::new(1.5_f32, theme::MUTED.gamma_multiply(0.8)));
                }
            }
        }
        // An opened wrong bin: just rubbish.
        if a.opened[i] && !hiding {
            let rim = origin + vec2(0.0, -76.0);
            let st = Stroke::new(1.6_f32, rgb(0xD8D2C4));
            // A fish bone sticking out.
            p.line_segment([rim + vec2(-8.0, -2.0), rim + vec2(9.0, -16.0)], st);
            for k in 0..3 {
                let m = rim + vec2(-4.0 + k as f32 * 4.5, -5.3 - k as f32 * 3.7);
                p.line_segment([m + vec2(-3.0, -3.5), m + vec2(3.0, 3.5)], st);
            }
            p.circle_stroke(rim + vec2(11.5, -18.0), 3.2, st);
            // A lazy fly.
            let fly = rim + vec2((t * 2.3 + i as f32).sin() * 14.0, -26.0 + (t * 3.1).cos() * 6.0);
            p.circle_filled(fly, 1.3, rgb(0x0C0A12));
        }
    }

    // Hint under the bins.
    if found.is_none() {
        let hint = match (a.tries, a.opened.iter().filter(|o| **o).count()) {
            (0, 0) => "Something is rustling... click a bin to look inside",
            (0, _) => "Only rubbish in there. Listen... where does it rustle?",
            _ => "Something moved! Look again...",
        };
        p.text(pos2(stage.center().x, stage.bottom() - 11.0), egui::Align2::CENTER_CENTER, hint, egui::FontId::proportional(12.0), theme::MUTED);
        return;
    }

    // Found: the companion jumps out of its bin and lands in the middle.
    let tf = found.unwrap_or(0.0);
    let from = spots[a.hide].1 + vec2(0.0, -80.0);
    let k = ((tf - 0.12) / 0.6).clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - k).powi(3);
    let size = 118.0 * (0.35 + 0.65 * ease);
    let land = pos2(stage.center().x, ground + 2.0 - size / 2.0);
    let pos = from.lerp(land, ease) + vec2(0.0, -(k * std::f32::consts::PI).sin() * 46.0);
    if tf > 0.12 {
        let emotion = if tf < 4.0 { Emotion::Happy } else { Emotion::Content };
        let blink = emotion == Emotion::Content && (now % BLINK_EVERY) < BLINK_LEN;
        paint_companion(p, egui::Rect::from_center_size(pos, Vec2::splat(size)), c, &Look { emotion, blink, hovered: false, now: t, eating: None });
    }
    // Poof: puffs pushing out of the bin and fading.
    let q = (tf / 0.9).clamp(0.0, 1.0);
    if q < 1.0 {
        for i in 0..10 {
            let ang = i as f32 / 10.0 * std::f32::consts::TAU + 0.3;
            let d = 14.0 + q * 56.0;
            p.circle_filled(from + vec2(ang.cos() * d, ang.sin() * d * 0.7), 22.0 * (1.0 - q) + 4.0, Color32::WHITE.gamma_multiply(0.7 * (1.0 - q)));
        }
    }
    // Sparks flying out.
    let f = (tf / 1.3).clamp(0.0, 1.0);
    if f < 1.0 {
        for i in 0..14 {
            let ang = i as f32 / 14.0 * std::f32::consts::TAU;
            let speed = 70.0 + (i % 3) as f32 * 25.0;
            let at = from + vec2(ang.cos(), ang.sin()) * speed * f + vec2(0.0, 30.0 * f * f);
            let col = match i % 3 {
                0 => accent,
                1 => gold,
                _ => Color32::WHITE,
            };
            sparkle(p, at, 7.0 * (1.0 - f * 0.6), ang + f * 6.0, col.gamma_multiply(1.0 - f));
        }
    }
    // Gentle twinkles afterwards.
    if tf > 1.0 {
        for (i, (x, y)) in [(-150.0f32, -60.0f32), (140.0, -80.0), (-60.0, -95.0), (75.0, -30.0), (0.0, -120.0)].into_iter().enumerate() {
            let tw = ((t * 2.2 + i as f32 * 1.3).sin() * 0.5 + 0.5).powi(2);
            let col = if i % 2 == 0 { gold } else { accent };
            sparkle(p, pos2(stage.center().x + x, ground + y), 3.0 + 5.0 * tw, 0.0, col.gamma_multiply(tw * ((tf - 1.0) / 0.5).min(1.0)));
        }
    }
}

/// What a bin's lid is doing.
enum Lid {
    /// On the bin, tilted by this angle (rattling).
    Closed(f32),
    /// Held up by whoever is inside.
    Lifted(f32),
    /// Leaning against the bin.
    Off,
    /// Blown off, this many seconds ago.
    Flying(f32),
}

/// A dented metal bin standing on `base` (middle of its bottom edge).
fn paint_bin(p: &egui::Painter, base: Pos2, lid: &Lid) {
    let metal = rgb(0x73808F);
    let dark = rgb(0x4C5664);
    let lite = rgb(0x9AA7B5);
    let edge = rgb(0x232A34);
    let at = |x: f32, y: f32| base + vec2(x, y);

    p.add(Shape::ellipse_filled(at(0.0, 1.0), vec2(34.0, 4.0), Color32::from_black_alpha(70)));
    // Body: slightly wider at the top, with a shaded side and ribs.
    p.add(Shape::convex_polygon(vec![at(-24.0, 0.0), at(24.0, 0.0), at(28.0, -72.0), at(-28.0, -72.0)], metal, Stroke::new(1.4_f32, edge)));
    p.add(Shape::convex_polygon(vec![at(8.0, -1.0), at(23.2, -1.0), at(27.0, -71.0), at(10.0, -71.0)], dark, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![at(-21.0, -1.0), at(-16.0, -1.0), at(-19.0, -71.0), at(-25.0, -71.0)], lite, Stroke::NONE));
    for x in [-13.0f32, -4.5, 4.0, 12.5, 20.0] {
        p.line_segment([at(x, -3.0), at(x * 1.14, -69.0)], Stroke::new(1.2_f32, edge.gamma_multiply(0.55)));
    }
    for y in [-18.0f32, -54.0] {
        let half = 24.0 + (-y / 72.0) * 4.0;
        p.line_segment([at(-half, y), at(half, y)], Stroke::new(2.4_f32, dark));
        p.line_segment([at(-half, y - 1.6), at(half, y - 1.6)], Stroke::new(1.0_f32, lite.gamma_multiply(0.7)));
    }
    // Handles and a dent.
    for side in [-1.0f32, 1.0] {
        p.add(Shape::line(vec![at(side * 27.5, -46.0), at(side * 32.5, -43.0), at(side * 32.0, -35.0), at(side * 26.5, -33.0)], Stroke::new(2.0_f32, edge)));
    }
    p.add(Shape::line(vec![at(-8.0, -30.0), at(-3.0, -26.0), at(-9.0, -22.0)], Stroke::new(1.2_f32, dark)));
    // The opening.
    p.add(Shape::ellipse_filled(at(0.0, -72.0), vec2(28.0, 5.5), edge));
    p.add(Shape::ellipse_filled(at(0.0, -72.0), vec2(25.0, 4.0), rgb(0x0E0C14)));

    // Lid: a low dome with a rim and a handle.
    let draw_lid = |center: Pos2, angle: f32, alpha: f32| {
        let (s, c) = angle.sin_cos();
        let m = |x: f32, y: f32| center + vec2(x * c - y * s, x * s + y * c);
        let dome: Vec<Pos2> = (0..=12)
            .map(|i| {
                let a = std::f32::consts::PI * i as f32 / 12.0;
                m(-29.0 * a.cos(), -9.0 * a.sin())
            })
            .collect();
        p.add(Shape::convex_polygon(dome, metal.gamma_multiply(alpha), Stroke::new(1.4_f32, edge.gamma_multiply(alpha))));
        p.add(Shape::convex_polygon(vec![m(-31.0, 0.0), m(31.0, 0.0), m(31.0, 4.0), m(-31.0, 4.0)], dark.gamma_multiply(alpha), Stroke::new(1.2_f32, edge.gamma_multiply(alpha))));
        p.add(Shape::line(vec![m(-6.0, -8.5), m(-5.0, -13.0), m(5.0, -13.0), m(6.0, -8.5)], Stroke::new(2.0_f32, edge.gamma_multiply(alpha))));
        p.line_segment([m(-20.0, -4.0), m(-9.0, -7.5)], Stroke::new(1.4_f32, lite.gamma_multiply(alpha)));
    };
    match lid {
        Lid::Closed(angle) => draw_lid(at(0.0, -75.0), *angle, 1.0),
        Lid::Lifted(up) => draw_lid(at(0.0, -75.0 - up), -0.12, 1.0),
        Lid::Off => draw_lid(at(-33.0, -14.0), -1.15, 1.0),
        Lid::Flying(t) => {
            let alpha = (1.0 - (t - 0.6) / 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                draw_lid(at(60.0 * t, -75.0 - 190.0 * t + 260.0 * t * t), t * 9.0, alpha);
            }
        }
    }
}

/// A four-pointed star.
fn sparkle(p: &egui::Painter, c: Pos2, r: f32, angle: f32, color: Color32) {
    if r <= 0.5 || color.a() == 0 {
        return;
    }
    for k in [0.0f32, std::f32::consts::FRAC_PI_2] {
        let (s, co) = (angle + k).sin_cos();
        let d = vec2(co, s);
        let n = vec2(-s, co);
        p.add(Shape::convex_polygon(vec![c + d * r, c + n * r * 0.22, c - d * r, c - n * r * 0.22], color, Stroke::NONE));
    }
}

/// A cartoon bone centered at `c`, `half` = half its length, turned by `angle`.
fn bone(p: &egui::Painter, c: Pos2, half: f32, angle: f32) {
    let fill = rgb(0xF6E7CC);
    let edge = rgb(0xC9A97A);
    let d = vec2(angle.cos(), angle.sin());
    let n = vec2(-d.y, d.x);
    let k = half * 0.42;
    for end in [-1.0f32, 1.0] {
        for side in [-1.0f32, 1.0] {
            p.circle(c + d * half * end + n * k * 0.75 * side, k * 0.62, fill, Stroke::new(1.0_f32, edge));
        }
    }
    let w = k * 0.55;
    p.add(Shape::convex_polygon(vec![c - d * half + n * w, c + d * half + n * w, c + d * half - n * w, c - d * half - n * w], fill, Stroke::NONE));
}

/// A filled heart centered at `c` with half-width `r`.
fn heart(p: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    p.circle_filled(c + vec2(-r * 0.5, -r * 0.2), r * 0.55, color);
    p.circle_filled(c + vec2(r * 0.5, -r * 0.2), r * 0.55, color);
    p.add(Shape::convex_polygon(vec![c + vec2(-r * 1.02, -r * 0.05), c + vec2(r * 1.02, -r * 0.05), c + vec2(0.0, r)], color, Stroke::NONE));
}

/// Settings page "Companions".
impl RustTalkApp {
    pub(super) fn page_companions(&mut self, ui: &mut egui::Ui) {
        use super::theme::{caption, card};
        let now = ui.input(|i| i.time) as f32;
        let meet_label = format!("{}  Meet a new companion", super::icons::regular::SPARKLE);

        card(ui, |ui| {
            super::theme::setting_row(
                ui,
                "Enable companions",
                Some("A little pet that sits above your user panel while you're connected. It stays on this computer - nobody else sees it."),
                false,
                |ui| {
                    super::theme::toggle(ui, &mut self.config.companions);
                },
            );
        });
        if !self.config.companions {
            return;
        }

        card(ui, |ui| {
            caption(ui, "Art style");
            ui.horizontal(|ui| {
                for (style, name, hint) in [
                    (CompanionStyle::Doodle, "Doodle", "Flat colors and a simple face - peeks over the edge of your user panel"),
                    (CompanionStyle::Anime, "Anime", "A sitting dog with shading and more detail"),
                ] {
                    if ui.selectable_label(self.config.companion_style == style, name).on_hover_text(hint).clicked() {
                        self.config.companion_style = style;
                    }
                }
            });
        });

        if self.companion.store.companions.is_empty() {
            card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("No companion yet").size(18.0).strong().color(theme::HEADER));
                    ui.label(
                        egui::RichText::new("A little friend who sits above your user panel while you're connected and comes along to every server you visit.")
                            .color(theme::MUTED),
                    );
                    ui.add_space(10.0);
                    if theme::primary_button(ui, &meet_label).clicked() {
                        self.companion.start_adoption(Kind::Dog);
                    }
                    ui.add_space(8.0);
                });
            });
            return;
        }

        card(ui, |ui| {
            caption(ui, "How it works");
            ui.label(
                egui::RichText::new("Drag across your companion to pet it, click to boop it, and use the bone button for snacks. It gets hungry over time and sleepy when ignored. Attention keeps it happy, and a happy companion bonds with you faster. All of that only counts while you're connected.")
                    .size(12.5)
                    .color(theme::MUTED),
            );
        });

        let ids: Vec<u32> = self.companion.store.companions.iter().map(|c| c.id).collect();
        for id in ids {
            let active = self.config.companion == Some(id);
            let mut name_done = false;
            let mut bring: Option<bool> = None;
            let mut recolor: Option<[u8; 3]> = None;
            let c = self.companion.store.get(id).expect("listed").clone();
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(96.0, 96.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 10.0, theme::DARK);
                    let look = Look { emotion: if active { Emotion::Happy } else { Emotion::Content }, blink: false, hovered: false, now, eating: None };
                    paint_companion(ui.painter(), rect.shrink(4.0), &c, &look);
                    ui.vertical(|ui| {
                        let entry = self.companion.store.get_mut(id).expect("listed");
                        let r = ui.add(egui::TextEdit::singleline(&mut entry.name).char_limit(companion::MAX_NAME).desired_width(180.0).font(egui::TextStyle::Heading));
                        name_done = r.lost_focus();
                        ui.label(egui::RichText::new(look_name(&c)).color(theme::MUTED));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(egui::RichText::new("Collar").size(12.0).color(theme::MUTED));
                            for col in companion::COLLARS {
                                if swatch(ui, col, c.collar == col).clicked() {
                                    recolor = Some(col);
                                }
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if active {
                                ui.label(egui::RichText::new(format!("{}  Coming along", super::icons::regular::PAW_PRINT)).color(theme::GREEN));
                                if ui.button("Leave at home").clicked() {
                                    bring = Some(false);
                                }
                            } else if theme::primary_button(ui, "Bring along").clicked() {
                                bring = Some(true);
                            }
                        });
                    });
                });
                ui.separator();
                let (level, level_name, progress) = c.bond_level();
                let hearts = super::icons::regular::HEART.repeat(level + 1);
                meter(ui, "Happiness", None, c.happiness, if c.happiness < companion::SAD_BELOW { theme::MUTED } else { theme::GREEN })
                    .on_hover_text("Petting and snacks make it happy. It sinks slowly when your companion gets no attention, faster when it's hungry.");
                meter(ui, "Bond", Some(&format!("{hearts}  {level_name}")), progress, theme::accent())
                    .on_hover_text("Grows with the time you spend together - faster when your companion is happy - and with snacks. It never shrinks.");
                stat(ui, "Time together", &companion::format_duration(c.seconds_together));
                stat(ui, "Pets", &c.pets.to_string());
                stat(ui, "Snacks", &c.snacks.to_string());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Hunger").color(theme::HEADER));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let fill = if c.hunger >= companion::HUNGRY_AT { theme::accent() } else { theme::GREEN };
                        ui.add(egui::ProgressBar::new(c.hunger).desired_width(160.0).fill(fill));
                    });
                });
                if !c.visits.is_empty() {
                    ui.separator();
                    caption(ui, &format!("Servers visited ({})", c.visits.len()));
                    let mut visits: Vec<_> = c.visits.values().collect();
                    visits.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
                    for v in visits {
                        stat(ui, if v.name.is_empty() { "?" } else { &v.name }, &companion::format_duration(v.seconds));
                    }
                }
            });
            let mut changed = recolor.is_some();
            if let Some(col) = recolor {
                if let Some(e) = self.companion.store.get_mut(id) {
                    e.collar = col;
                }
            }
            if name_done {
                if let Some(e) = self.companion.store.get_mut(id) {
                    e.name = companion::clean_name(&e.name).unwrap_or_else(|| c.name.clone());
                }
                changed = true;
            }
            match bring {
                Some(true) => self.config.companion = Some(id),
                Some(false) => self.config.companion = None,
                None => {}
            }
            if changed {
                self.companion.save();
            }
        }

        card(ui, |ui| {
            if self.companion.store.can_adopt() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("There's room for another friend!").color(theme::HEADER));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::primary_button(ui, &meet_label).clicked() {
                            self.companion.start_adoption(Kind::Dog);
                        }
                    });
                });
            } else {
                let left = companion::format_duration(self.companion.store.hours_to_next_slot() * 3600.0);
                ui.label(egui::RichText::new(format!("{}  A new companion can join after {left} more together.", super::icons::regular::LOCK_SIMPLE)).color(theme::MUTED));
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// A labeled bar (0..1), optionally with a note left of it.
fn meter(ui: &mut egui::Ui, label: &str, note: Option<&str>, value: f32, fill: Color32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(theme::HEADER));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::ProgressBar::new(value).desired_width(160.0).fill(fill));
            if let Some(note) = note {
                ui.label(egui::RichText::new(note).color(theme::MUTED));
            }
        });
    })
    .response
}

fn stat(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(theme::HEADER));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).color(theme::MUTED));
        });
    });
}

/// Debug builds: every coat in every emotion, to check the drawings
/// (`RUSTTALK_DEV_OPEN=companion_gallery`).
#[cfg(debug_assertions)]
impl RustTalkApp {
    pub(super) fn ui_companion_gallery(&mut self, ctx: &egui::Context) {
        if !self.companion.gallery {
            return;
        }
        let now = ctx.input(|i| i.time) as f32;
        let emotions = [
            Emotion::Content,
            Emotion::Happy,
            Emotion::Petted,
            Emotion::Eating,
            Emotion::Hungry,
            Emotion::Sleepy,
            Emotion::Asleep,
            Emotion::Angry,
            Emotion::Full,
            Emotion::Sad,
        ];
        egui::Window::new("Companion gallery").default_pos(pos2(272.0, 36.0)).show(ctx, |ui| {
            for coat in 0..DOG_COATS.len() {
                ui.horizontal(|ui| {
                    for (i, e) in emotions.into_iter().enumerate() {
                        let size = if i == 0 { 116.0 } else { 74.0 };
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
                        let genes = (coat as u32).wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503);
                        let c = Companion::new(Kind::Dog, "", coat as u8, genes, companion::COLLARS[(coat * 3 + i) % companion::COLLARS.len()]);
                        let look = Look { emotion: e, blink: false, hovered: false, now, eating: (e == Emotion::Eating).then_some(0.3) };
                        paint_companion(ui.painter(), rect.shrink(6.0), &c, &look);
                    }
                });
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}
