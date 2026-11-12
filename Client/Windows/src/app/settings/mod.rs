//! App settings: separate window with a searchable sidebar of grouped pages.
//! Each page is one scrolling column of titled sections; the structure lives
//! in `table.rs`, the section contents in `pages.rs`.

mod pages;
mod table;

use super::theme::{self, dense};
use super::*;
pub(super) use table::SettingsPage;
use table::SectionId;

/// Widest the content column grows (Discord-like reading width).
const CONTENT_MAX_WIDTH: f32 = 700.0;
/// A section counts as "in view" once its heading is this close to the top.
const IN_VIEW_SLACK: f32 = 40.0;

/// Sidebar search and scroll state of the settings window.
#[derive(Default)]
pub(super) struct SettingsNav {
    search: String,
    /// Section highlighted in the sidebar (the one in view).
    active: Option<SectionId>,
    /// Smooth-scroll to this section on the next frame.
    scroll_to: Option<SectionId>,
    /// Jump to the top (page switch).
    reset_scroll: bool,
}

const SEARCH_ID: &str = "settings_search";

fn recorded_settings_binding(ctx: &egui::Context) -> Option<Option<String>> {
    if ctx.memory(|m| m.has_focus(egui::Id::new(SEARCH_ID))) {
        return Some(None);
    }
    ctx.input(|i| i.events.iter().find_map(hotkey::recorded_binding))
}

impl RustTalkApp {
    pub(super) fn open_settings(&mut self, page: SettingsPage) {
        self.input_devices = capture::input_device_names();
        self.output_devices = playback::output_device_names();
        self.cameras = None; // refresh lazily on the video page
        self.settings_page = page;
        self.settings_nav = SettingsNav { reset_scroll: true, ..Default::default() };
        self.show_settings = true;
    }

    /// Open `page`; with a section, smooth-scroll to it.
    fn goto_settings(&mut self, page: SettingsPage, section: Option<SectionId>) {
        if page != self.settings_page {
            self.settings_page = page;
            self.recording_key = None;
            self.settings_nav.reset_scroll = true;
        }
        self.settings_nav.scroll_to = section.or(table::page(page).sections.first().map(|s| s.id));
    }

    /// Settings as a separate OS window (own viewport).
    pub(super) fn ui_settings(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("settings_viewport"),
            egui::ViewportBuilder::default()
                .with_title("RustTalk - Settings")
                .with_inner_size([1040.0, 720.0])
                .with_min_inner_size([800.0, 540.0]),
            |ctx, _class| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    self.show_settings = false;
                    self.recording_key = None;
                    self.config.save();
                }
                egui::SidePanel::left("settings_nav")
                    .exact_width(240.0)
                    .resizable(false)
                    .frame(egui::Frame::default().fill(theme::SIDEBAR).inner_margin(egui::Margin { left: 12, right: 10, top: 16, bottom: 10 }))
                    .show(ctx, |ui| self.settings_nav(ui));
                self.capture_keybind(ctx);
                egui::SidePanel::right("settings_close")
                    .exact_width(72.0)
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 0, right: 16, top: 24, bottom: 0 }))
                    .show(ctx, |ui| self.settings_close_button(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(theme::BG).inner_margin(egui::Margin { left: 40, right: 8, top: 0, bottom: 0 }))
                    .show(ctx, |ui| {
                        let mut area = egui::ScrollArea::vertical().id_salt("settings_page").auto_shrink([false, false]);
                        if std::mem::take(&mut self.settings_nav.reset_scroll) {
                            area = area.vertical_scroll_offset(0.0);
                        }
                        area.show(ui, |ui| {
                            ui.set_max_width((ui.available_width() - 16.0).clamp(0.0, CONTENT_MAX_WIDTH));
                            self.settings_page_body(ui);
                        });
                    });
            },
        );
    }

    fn settings_close_button(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            let close = ui
                .add(
                    egui::Button::new(egui::RichText::new(icons::regular::X).size(18.0).color(theme::MUTED))
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::new(1.5_f32, theme::MUTED))
                        .corner_radius(18.0)
                        .min_size(egui::vec2(36.0, 36.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Close (Esc)");
            ui.label(egui::RichText::new("ESC").size(11.5).strong().color(theme::MUTED));
            if close.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape) && self.recording_key.is_none()) {
                self.show_settings = false;
                self.config.save();
            }
        });
    }

    // ----- sidebar ---------------------------------------------------------

    fn settings_nav(&mut self, ui: &mut egui::Ui) {
        let search = self.settings_search_field(ui);
        ui.add_space(dense(10.0, 6.0));
        let results = table::search(&self.settings_nav.search);
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(hit) = results.first().and_then(|g| g.pages.first()) {
                self.goto_settings(hit.page.page, hit.sections.first().map(|s| s.id));
            }
        }
        let searching = !self.settings_nav.search.trim().is_empty();
        let footer = 22.0;
        egui::ScrollArea::vertical()
            .id_salt("settings_nav_scroll")
            .auto_shrink([false, true])
            .max_height((ui.available_height() - footer).max(0.0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if results.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(format!("No settings match \"{}\"", self.settings_nav.search.trim())).color(theme::MUTED));
                }
                let mut clicked = None;
                for (gi, hit) in results.iter().enumerate() {
                    if gi > 0 {
                        ui.add_space(dense(10.0, 6.0));
                    }
                    ui.add_space(4.0);
                    nav_caption(ui, hit.group.title);
                    ui.add_space(2.0);
                    for ph in &hit.pages {
                        let current = self.settings_page == ph.page.page;
                        if nav_page_row(ui, ph.page.icon, ph.page.title, current).clicked() {
                            clicked = Some((ph.page.page, None));
                        }
                        if current || searching {
                            let active = if current { self.settings_nav.active } else { None };
                            if let Some(id) = nav_section_list(ui, &ph.sections, active) {
                                clicked = Some((ph.page.page, Some(id)));
                            }
                        }
                    }
                }
                if let Some((page, section)) = clicked {
                    self.goto_settings(page, section);
                }
            });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.label(
                egui::RichText::new(format!("RustTalk v{}  -  protocol v{}", env!("CARGO_PKG_VERSION"), rusttalk_shared::protocol::PROTOCOL_VERSION))
                    .size(11.0)
                    .color(theme::DISABLED),
            );
        });
    }

    /// Search box with magnifier and clear button; Escape clears it.
    /// Its id is [`SEARCH_ID`]; focusing it cancels shortcut recording.
    fn settings_search_field(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let nav = &mut self.settings_nav;
        if !nav.search.is_empty() && self.recording_key.is_none() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            nav.search.clear();
        }
        egui::Frame::default()
            .fill(theme::DARK)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(8, dense(5.0, 3.0) as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(egui::RichText::new(icons::regular::MAGNIFYING_GLASS).color(theme::MUTED));
                    ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);
                    let clear_w = if nav.search.is_empty() { 0.0 } else { 26.0 };
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut nav.search)
                            .id(egui::Id::new(SEARCH_ID))
                            .hint_text("Search")
                            .frame(false)
                            .margin(egui::Margin::symmetric(0, 2))
                            .desired_width((ui.available_width() - clear_w).max(0.0)),
                    );
                    if !nav.search.is_empty() {
                        let clear = ui
                            .add(egui::Button::new(egui::RichText::new(icons::regular::X).color(theme::MUTED)).frame(false))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Clear (Esc)");
                        if clear.clicked() {
                            nav.search.clear();
                            edit.request_focus();
                        }
                    }
                    edit
                })
                .inner
            })
            .inner
    }

    // ----- page ------------------------------------------------------------

    fn settings_page_body(&mut self, ui: &mut egui::Ui) {
        let page = table::page(self.settings_page);
        let view = ui.clip_rect();
        let left = ui.max_rect().left();
        let page_top = ui.cursor().top();
        ui.spacing_mut().item_spacing.y = dense(8.0, 4.0);
        ui.spacing_mut().interact_size.x = 64.0; // equal width for slider values
        ui.spacing_mut().combo_width = 250.0;
        ui.spacing_mut().slider_width = 180.0;
        ui.visuals_mut().extreme_bg_color = theme::DARK; // text fields stand out on BG
        ui.add_space(dense(28.0, 20.0));
        ui.label(egui::RichText::new(page.title).size(26.0).strong().color(theme::HEADER));

        let anim = egui::style::ScrollAnimation::new(2400.0, egui::Rangef::new(0.2, 0.45));
        let mut active = page.sections.first().map(|s| s.id);
        let mut last_anchor = page_top;
        for (i, section) in page.sections.iter().enumerate() {
            if i > 0 {
                ui.add_space(dense(24.0, 16.0));
                divider(ui);
            }
            ui.add_space(dense(20.0, 12.0));
            // Scroll anchor sits a little above the heading; the first section scrolls to the page top.
            let anchor = if i == 0 { page_top } else { ui.cursor().top() - 16.0 };
            if self.settings_nav.scroll_to == Some(section.id) {
                self.settings_nav.scroll_to = None;
                ui.scroll_to_rect_animation(egui::Rect::from_min_size(egui::pos2(left, anchor), egui::vec2(1.0, 1.0)), Some(egui::Align::TOP), anim);
            }
            if anchor <= view.top() + IN_VIEW_SLACK {
                active = Some(section.id);
            }
            last_anchor = anchor;
            ui.label(egui::RichText::new(section.title).size(20.0).strong().color(theme::HEADER));
            ui.add_space(dense(6.0, 2.0));
            self.settings_section(ui, section.id);
        }
        // Room below the last section so every section can scroll up to the top.
        let used = ui.cursor().top() - last_anchor;
        ui.add_space((view.height() - used).max(dense(40.0, 28.0)));
        self.settings_nav.active = active;
    }

    // ----- keybind capture -------------------------------------------------

    fn binding_name(&self, action: hotkey::Action) -> Option<&str> {
        match action {
            hotkey::Action::PushToTalk => Some(&self.config.ptt_key),
            hotkey::Action::Mute => self.config.mute_key.as_deref(),
            hotkey::Action::Deafen => self.config.deafen_key.as_deref(),
        }
    }

    fn keybind_button(&mut self, ui: &mut egui::Ui, action: hotkey::Action) {
        let label = if self.recording_key == Some(action) {
            egui::RichText::new("Press a key...").color(theme::YELLOW)
        } else {
            egui::RichText::new(format!("{}  {}", icons::regular::KEYBOARD, self.binding_name(action).unwrap_or("Not set"))).color(theme::HEADER)
        };
        let resp = ui.add(egui::Button::new(label).min_size(egui::vec2(140.0, 28.0)))
            .on_hover_text("Click, then press a key or side mouse button with optional modifiers (Esc cancels)");
        if resp.clicked() {
            self.recording_key = if self.recording_key == Some(action) { None } else { Some(action) };
            self.keybind_error = None;
            self.global_ptt.lock().unwrap().set([None; 3]);
            self.control.ptt_pressed.store(false, Ordering::Relaxed);
        }
    }

    fn capture_keybind(&mut self, ctx: &egui::Context) {
        let Some(action) = self.recording_key else { return; };
        let pressed = recorded_settings_binding(ctx);
        if let Some(choice) = pressed {
            self.recording_key = None;
            if let Some(name) = choice {
                let binding = hotkey::parse_binding(&name);
                #[cfg(windows)]
                if matches!(binding.map(|b| b.button), Some(hotkey::Button::Key(key)) if hotkey::keycode_for(key).is_none()) {
                    self.keybind_error = Some(format!("{name} is not supported as a Windows global shortcut"));
                    return;
                }
                if let Some(other) = hotkey::Action::ALL.into_iter().find(|other| *other != action
                    && self.binding_name(*other).and_then(hotkey::parse_binding).is_some_and(|key| Some(key) == binding)) {
                    self.keybind_error = Some(format!("{name} is already assigned to {}", other.label()));
                    return;
                }
                match action {
                    hotkey::Action::PushToTalk => {
                        self.config.ptt_key = name;
                        #[cfg(target_os = "linux")]
                        { self.config.global_ptt = true; }
                    }
                    hotkey::Action::Mute => self.config.mute_key = Some(name),
                    hotkey::Action::Deafen => self.config.deafen_key = Some(name),
                }
                self.config.save();
            }
        }
    }

}

/// Thin full-width line between sections.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(0.0), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::SELECTED);
}

fn nav_caption(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        theme::caption(ui, text);
    });
}

fn nav_page_row(ui: &mut egui::Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(0.0), dense(32.0, 28.0)), egui::Sense::click());
    let hovered = resp.hovered();
    let bg = if selected {
        theme::SELECTED
    } else if hovered {
        theme::HOVER
    } else {
        egui::Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, 6.0, bg);
    let fg = if selected || hovered { theme::HEADER } else { theme::MUTED };
    p.text(rect.left_center() + egui::vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, icon, egui::FontId::proportional(16.0), fg);
    p.text(rect.left_center() + egui::vec2(34.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(dense(14.5, 14.0)), fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Indented section list under a page, with a thin guide line and an accent
/// marker on `active`. Returns the clicked section.
fn nav_section_list(ui: &mut egui::Ui, sections: &[&table::Section], active: Option<SectionId>) -> Option<SectionId> {
    let row_h = dense(26.0, 22.0);
    let line_x = ui.cursor().left() + 18.0;
    let top = ui.cursor().top();
    let mut clicked = None;
    let mut rows = Vec::with_capacity(sections.len());
    for sec in sections {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(0.0), row_h), egui::Sense::click());
        if resp.clicked() {
            clicked = Some(sec.id);
        }
        rows.push((sec, rect, resp.hovered()));
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let &(_, last, _) = rows.last()?;
    let p = ui.painter();
    p.line_segment([egui::pos2(line_x, top + 2.0), egui::pos2(line_x, last.bottom() - 2.0)], egui::Stroke::new(1.5_f32, theme::SELECTED));
    for (sec, rect, hovered) in rows {
        let is_active = active == Some(sec.id);
        if hovered {
            let bg = egui::Rect::from_min_max(egui::pos2(line_x + 8.0, rect.top()), rect.max);
            p.rect_filled(bg, 4.0, theme::HOVER);
        }
        if is_active {
            let bar = egui::Rect::from_center_size(egui::pos2(line_x, rect.center().y), egui::vec2(3.0, row_h - 8.0));
            p.rect_filled(bar, 1.5, theme::accent());
        }
        let fg = if is_active || hovered { theme::HEADER } else { theme::MUTED };
        p.text(rect.left_center() + egui::vec2(34.0, 0.0), egui::Align2::LEFT_CENTER, sec.title, egui::FontId::proportional(dense(13.5, 13.0)), fg);
    }
    clicked
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn focused_search_cancels_shortcut_capture_before_typing() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |_| {});
        assert_eq!(recorded_settings_binding(&ctx), None);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
        assert_eq!(recorded_settings_binding(&ctx), Some(None));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            assert_eq!(recorded_settings_binding(ctx), Some(None));
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(SEARCH_ID)));
            assert_eq!(recorded_settings_binding(ctx), Some(Some("A".to_owned())));
        });
    }
}
