//! Look & feel: colors, egui style and small shared widgets.
//!
//! Dark only (by design). Palette follows the familiar Discord-style
//! layering: darkest = server/user bars, mid = sidebar, lightest = content.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use eframe::egui;

use super::icons;

pub const BG: egui::Color32 = egui::Color32::from_rgb(0x31, 0x33, 0x38);
pub const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(0x2B, 0x2D, 0x31);
pub const DARK: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x1F, 0x22);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(0x23, 0x24, 0x28);
pub const INPUT: egui::Color32 = egui::Color32::from_rgb(0x38, 0x3A, 0x40);
pub const HOVER: egui::Color32 = egui::Color32::from_rgb(0x35, 0x37, 0x3C);
pub const SELECTED: egui::Color32 = egui::Color32::from_rgb(0x40, 0x42, 0x49);
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(0x1F, 0x20, 0x23);
/// Fixed blurple, for things that must not follow the user's primary color (role badges).
pub const BLURPLE: egui::Color32 = egui::Color32::from_rgb(0x58, 0x65, 0xF2);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(0x23, 0xA5, 0x5A);
pub const RED: egui::Color32 = egui::Color32::from_rgb(0xF2, 0x3F, 0x43);
pub const YELLOW: egui::Color32 = egui::Color32::from_rgb(0xF0, 0xB2, 0x32);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xDB, 0xDE, 0xE1);
pub const HEADER: egui::Color32 = egui::Color32::from_rgb(0xF2, 0xF3, 0xF5);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(0x94, 0x9B, 0xA4);
pub const DISABLED: egui::Color32 = egui::Color32::from_rgb(0x5C, 0x60, 0x68);

/// Avatar palette (name -> deterministic color).
pub const AVATAR_PALETTE: [egui::Color32; 8] = [
    egui::Color32::from_rgb(0x58, 0x65, 0xF2),
    egui::Color32::from_rgb(0x3B, 0xA5, 0x5D),
    egui::Color32::from_rgb(0xC2, 0x8F, 0x1F),
    egui::Color32::from_rgb(0xEB, 0x45, 0x95),
    egui::Color32::from_rgb(0xED, 0x42, 0x45),
    egui::Color32::from_rgb(0xE0, 0x6C, 0x18),
    egui::Color32::from_rgb(0x9B, 0x84, 0xEE),
    egui::Color32::from_rgb(0x1F, 0xA0, 0x92),
];

/// Preset primary colors for the appearance settings.
pub const ACCENT_PRESETS: [(&str, [u8; 3]); 8] = [
    ("Blurple", [0x58, 0x65, 0xF2]),
    ("Green", [0x23, 0xA5, 0x5A]),
    ("Teal", [0x1F, 0xA0, 0x92]),
    ("Sky", [0x1E, 0x90, 0xD8]),
    ("Purple", [0x9B, 0x59, 0xD9]),
    ("Pink", [0xEB, 0x45, 0x95]),
    ("Red", [0xE0, 0x3E, 0x3E]),
    ("Orange", [0xE0, 0x6C, 0x18]),
];

static ACCENT: AtomicU32 = AtomicU32::new(0x5865F2);
static COMPACT: AtomicBool = AtomicBool::new(false);

/// The user's primary color.
pub fn accent() -> egui::Color32 {
    let v = ACCENT.load(Ordering::Relaxed);
    egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Readable text color on an `accent()` fill (dark text on light accents).
pub fn on_accent() -> egui::Color32 {
    on_color(accent())
}

fn on_color(c: egui::Color32) -> egui::Color32 {
    let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if lum > 165.0 {
        DARK
    } else {
        egui::Color32::WHITE
    }
}

/// Compact density (tighter spacing, chat without avatars).
pub fn compact() -> bool {
    COMPACT.load(Ordering::Relaxed)
}

/// Pick `cozy` or `compact` by the current density.
pub fn dense(cozy: f32, compact: f32) -> f32 {
    if self::compact() {
        compact
    } else {
        cozy
    }
}

/// Set primary color and density, then restyle egui.
pub fn set_appearance(ctx: &egui::Context, accent: [u8; 3], compact: bool) {
    let [r, g, b] = accent;
    ACCENT.store(u32::from_be_bytes([0, r, g, b]), Ordering::Relaxed);
    COMPACT.store(compact, Ordering::Relaxed);
    apply(ctx);
}

/// Global egui style from the current appearance.
pub fn apply(ctx: &egui::Context) {
    let accent = accent();
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = INPUT;
    v.faint_bg_color = SIDEBAR;
    v.code_bg_color = DARK;
    v.hyperlink_color = accent;
    v.selection.bg_fill = accent.linear_multiply(0.6);
    v.selection.stroke = egui::Stroke::new(1.0_f32, HEADER);
    v.window_corner_radius = egui::CornerRadius::same(10);
    v.menu_corner_radius = egui::CornerRadius::same(8);
    v.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(140),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: egui::Color32::from_black_alpha(120),
    };

    let w = &mut v.widgets;
    let r = egui::CornerRadius::same(6);
    w.noninteractive.bg_fill = SIDEBAR;
    w.noninteractive.weak_bg_fill = SIDEBAR;
    w.noninteractive.fg_stroke.color = TEXT;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = INPUT;
    w.inactive.weak_bg_fill = INPUT;
    w.inactive.fg_stroke.color = TEXT;
    w.inactive.bg_stroke = egui::Stroke::NONE;
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = SELECTED;
    w.hovered.weak_bg_fill = SELECTED;
    w.hovered.fg_stroke.color = HEADER;
    w.hovered.bg_stroke = egui::Stroke::NONE;
    w.hovered.corner_radius = r;
    w.active.bg_fill = accent;
    w.active.weak_bg_fill = accent;
    w.active.fg_stroke.color = on_color(accent);
    w.active.corner_radius = r;
    w.open.bg_fill = SELECTED;
    w.open.weak_bg_fill = SELECTED;
    w.open.corner_radius = r;
    ctx.set_visuals(v);

    let compact = compact();
    ctx.style_mut(|s| {
        if compact {
            s.spacing.item_spacing = egui::vec2(6.0, 4.0);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
            s.spacing.interact_size.y = 22.0;
            s.spacing.window_margin = egui::Margin::same(10);
        } else {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 5.0);
            s.spacing.interact_size.y = 26.0;
            s.spacing.window_margin = egui::Margin::same(14);
        }
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.combo_width = 220.0;
        s.spacing.slider_width = 220.0;
        s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
        s.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    });
}

/// Deterministic avatar color from the name.
pub fn avatar_color(name: &str) -> egui::Color32 {
    let hash = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    AVATAR_PALETTE[(hash as usize) % AVATAR_PALETTE.len()]
}

/// Round avatar with initial; green ring while `talking`.
pub fn draw_avatar(ui: &mut egui::Ui, name: &str, size: f32, talking: bool) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_avatar(ui.painter(), rect, name, talking);
    rect
}

pub fn paint_avatar(painter: &egui::Painter, rect: egui::Rect, name: &str, talking: bool) {
    let center = rect.center();
    let radius = rect.width() / 2.0;
    painter.circle_filled(center, radius, avatar_color(name));
    if talking {
        painter.circle_stroke(center, radius + 2.0, egui::Stroke::new(2.0_f32, GREEN));
    }
    let initial: String = name.chars().next().unwrap_or('?').to_uppercase().collect();
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(rect.width() * 0.48),
        egui::Color32::WHITE,
    );
}

/// Green dot at the bottom right of an avatar (currently speaking).
pub fn draw_talk_dot(ui: &egui::Ui, avatar: egui::Rect) {
    let r = (avatar.width() * 0.16).max(4.0);
    let c = avatar.right_bottom() - egui::vec2(r * 0.6, r * 0.6);
    ui.painter().circle_filled(c, r + 2.0, DARK);
    ui.painter().circle_filled(c, r, GREEN);
}

/// Square icon button for toolbars. `state`: Normal, Active (green, e.g.
/// live), Danger (red, e.g. muted), Disabled (greyed, not clickable).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    Normal,
    Active,
    Danger,
    Disabled,
}

pub fn icon_button(ui: &mut egui::Ui, icon: &str, state: IconState, tooltip: &str) -> egui::Response {
    let (fg, bg) = match state {
        IconState::Normal => (TEXT, egui::Color32::TRANSPARENT),
        IconState::Active => (egui::Color32::WHITE, GREEN),
        IconState::Danger => (RED, egui::Color32::TRANSPARENT),
        IconState::Disabled => (DISABLED, egui::Color32::TRANSPARENT),
    };
    let hover_bg = ui.painter().add(egui::Shape::Noop);
    let btn = egui::Button::new(egui::RichText::new(icon).size(18.0).color(fg))
        .fill(bg)
        .min_size(egui::vec2(32.0, 32.0));
    let resp = ui.add_enabled(state != IconState::Disabled, btn);
    // Transparent buttons get no hover fill from egui; paint one (red tint for Danger).
    if resp.hovered() && matches!(state, IconState::Normal | IconState::Danger) {
        let fill = if state == IconState::Danger { RED.gamma_multiply(0.18) } else { egui::Color32::from_white_alpha(14) };
        ui.painter().set(hover_bg, egui::Shape::rect_filled(resp.rect, 6.0, fill));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip).on_disabled_hover_text(tooltip)
}

/// Primary (accent) button.
pub fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(on_accent()).strong())
            .fill(accent())
            .min_size(egui::vec2(96.0, 32.0)),
    )
}

/// Menu row: label left, icon right, accent fill on hover (red for `danger`).
/// Call `ui.set_width(..)` on the menu first; the row takes the full width.
pub fn menu_item(ui: &mut egui::Ui, icon: &str, label: &str, danger: bool) -> egui::Response {
    let h = dense(32.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let enabled = ui.is_enabled();
    let hovered = enabled && resp.hovered();
    let (fill, hover_fg) = if danger { (RED, egui::Color32::WHITE) } else { (accent(), on_accent()) };
    let fg = if !enabled {
        DISABLED
    } else if hovered {
        hover_fg
    } else if danger {
        RED
    } else {
        TEXT
    };
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, 4.0, fill);
    }
    p.text(rect.left_center() + egui::vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(14.0), fg);
    p.text(rect.right_center() - egui::vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, icon, egui::FontId::proportional(16.0), fg);
    resp
}

/// Thin divider between menu groups.
pub fn menu_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter().hline(
        (rect.left() + 6.0)..=(rect.right() - 6.0),
        rect.center().y,
        egui::Stroke::new(1.0_f32, SELECTED),
    );
}

/// Text-only button (Cancel, Close, ...) with a subtle background on hover.
pub fn ghost_button(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ghost_button_sized(ui, text, egui::Vec2::ZERO)
}

/// [`ghost_button`] with a minimum size (fixed-layout toolbars).
pub fn ghost_button_sized(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>, min: egui::Vec2) -> egui::Response {
    let bg = ui.painter().add(egui::Shape::Noop);
    let resp = ui.add(egui::Button::new(text).frame(false).min_size(min));
    if resp.hovered() && ui.is_enabled() {
        // Light overlay: visible on every panel shade (a fixed HOVER vanishes on INPUT).
        ui.painter().set(bg, egui::Shape::rect_filled(resp.rect.expand(2.0), 6.0, egui::Color32::from_white_alpha(14)));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Red button for destructive actions.
pub fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE).strong())
            .fill(RED)
            .min_size(egui::vec2(96.0, 32.0)),
    )
}

/// Small yellow "WIP" pill next to features that are placeholders.
pub fn wip_badge(ui: &mut egui::Ui) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        "WIP".to_string(),
        egui::FontId::proportional(10.5),
        egui::Color32::BLACK,
    );
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 8.0, YELLOW);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, egui::Color32::BLACK);
    resp.on_hover_text("Work in progress - not functional yet")
}

/// Full-width yellow notice ("this page is a work in progress").
pub fn wip_banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::default()
        .fill(YELLOW.linear_multiply(0.08))
        .stroke(egui::Stroke::new(1.0_f32, YELLOW.linear_multiply(0.45)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(icons::regular::WRENCH).color(YELLOW));
                ui.label(egui::RichText::new(text).color(HEADER));
            });
        });
}

/// Uppercase small caption (section label).
pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).size(11.5).strong().color(MUTED));
}

/// Card container for settings sections.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::default()
        .fill(SIDEBAR)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Setting row: label (+ optional description, wrapping) on the left, control
/// right-aligned on the label's line. `wip` rows are greyed out with a badge.
pub fn setting_row(
    ui: &mut egui::Ui,
    label: &str,
    desc: Option<&str>,
    wip: bool,
    control: impl FnOnce(&mut egui::Ui),
) {
    let enabled = ui.is_enabled() && !wip;
    let pad = dense(6.0, 3.0);
    ui.add_space(pad);
    let top = ui.cursor().min;
    // Can be negative for a frame while a window opens; egui asserts on negative child sizes.
    let width = ui.available_width().max(0.0);
    let row_h = ui.spacing().interact_size.y;

    // Control first (right to left), so the text knows how much room is left.
    let mut ctrl = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(top, egui::vec2(width, row_h)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    if wip {
        ctrl.disable();
    }
    control(&mut ctrl);
    let ctrl_rect = ctrl.min_rect();

    let text_w = (ctrl_rect.left() - top.x - 16.0).clamp(0.0, width);
    // Stack below the control when a narrow window leaves too little label space.
    let (text_top, text_w) = if text_w < 120.0 {
        (egui::pos2(top.x, ctrl_rect.bottom() + pad), width)
    } else {
        (top, text_w)
    };
    let mut text = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(text_top, egui::vec2(text_w, row_h)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    text.spacing_mut().item_spacing.y = 2.0;
    let line_h = text.text_style_height(&egui::TextStyle::Body);
    text.add_space(((row_h - line_h) / 2.0).max(0.0));
    let title = egui::RichText::new(label).size(14.5).color(if enabled { HEADER } else { MUTED });
    if wip {
        text.horizontal(|ui| {
            ui.label(title);
            wip_badge(ui);
        });
    } else {
        text.label(title);
    }
    if let Some(d) = desc {
        text.label(egui::RichText::new(d).size(12.5).color(if enabled { MUTED } else { DISABLED }));
    }

    let bottom = text.min_rect().bottom().max(ctrl_rect.bottom()).max(top.y + row_h);
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(top, egui::pos2(top.x + width, bottom)));
    ui.add_space(pad);
}

/// Pill-style toggle switch.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let enabled = ui.is_enabled();
    let bg = if !enabled {
        DISABLED
    } else if *on {
        accent()
    } else {
        egui::Color32::from_rgb(0x80, 0x84, 0x8E)
    };
    let p = ui.painter();
    p.rect_filled(rect, 11.0, bg);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    p.circle_filled(egui::pos2(x, rect.center().y), 8.0, egui::Color32::WHITE);
    resp
}

/// Live mic level bar with threshold marker (like a mic test).
/// Perceptual (sqrt) scale so quiet signals are visible.
pub fn mic_meter(ui: &mut egui::Ui, level: f32, threshold: f32) {
    let scale = |v: f32| (v.sqrt() * 1.6).clamp(0.0, 1.0);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 12.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, DARK);
    let lw = rect.width() * scale(level);
    if lw >= 6.0 {
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(lw, rect.height()));
        let color = if level >= threshold { GREEN } else { YELLOW };
        p.rect_filled(fill, 6.0, color);
    }
    let tx = rect.left() + rect.width() * scale(threshold);
    p.line_segment(
        [egui::pos2(tx, rect.top() - 2.0), egui::pos2(tx, rect.bottom() + 2.0)],
        egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
    );
}

/// Aspect-preserving size of `img` within `bounds`.
pub fn fit_size(img: egui::Vec2, bounds: egui::Vec2) -> egui::Vec2 {
    if img.x <= 0.0 || img.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.x / img.x).min(bounds.y / img.y);
    img * scale
}

/// Toast in the bottom right of the main area (status/errors), fades out.
pub fn toast(ctx: &egui::Context, text: &str, age_secs: f32, is_error: bool) {
    const SHOW: f32 = 6.0;
    if text.is_empty() || age_secs > SHOW {
        return;
    }
    let alpha = ((SHOW - age_secs) / 0.6).clamp(0.0, 1.0);
    egui::Area::new(egui::Id::new("status_toast"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-18.0, -86.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(DARK.linear_multiply(alpha))
                .stroke(egui::Stroke::new(1.0_f32, if is_error { RED } else { BORDER }.linear_multiply(alpha)))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(380.0);
                    let icon = if is_error { icons::regular::WARNING_CIRCLE } else { icons::regular::INFO };
                    let color = if is_error { RED } else { accent() };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icon).color(color.linear_multiply(alpha)));
                        ui.label(egui::RichText::new(text).color(HEADER.linear_multiply(alpha)));
                    });
                });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_rows_handle_narrow_and_opening_frames() {
        for width in [0.0, 1.0, 100.0, 400.0] {
            let ctx = egui::Context::default();
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_max_width(width);
                    setting_row(ui, "Input device", Some("Device used for voice capture"), false, |ui| {
                        ui.add_sized([250.0, 24.0], egui::Button::new("A long device name"));
                    });
                    setting_row(ui, "Unavailable setting", None, true, |ui| {
                        toggle(ui, &mut false);
                    });
                    assert!(ui.min_rect().is_finite());
                });
            });
        }
    }

    #[test]
    fn avatar_color_is_deterministic() {
        assert_eq!(avatar_color("alice"), avatar_color("alice"));
        assert!(AVATAR_PALETTE.contains(&avatar_color("alice")));
        assert!(AVATAR_PALETTE.contains(&avatar_color("bob")));
    }

    #[test]
    fn text_on_accent_is_readable() {
        assert_eq!(on_color(BLURPLE), egui::Color32::WHITE);
        assert_eq!(on_color(egui::Color32::from_rgb(0xF0, 0xE0, 0x40)), DARK);
    }

    #[test]
    fn fit_keeps_aspect() {
        let s = fit_size(egui::vec2(1920.0, 1080.0), egui::vec2(480.0, 480.0));
        assert!((s.x - 480.0).abs() < 0.01 && (s.y - 270.0).abs() < 0.01);
    }
}
