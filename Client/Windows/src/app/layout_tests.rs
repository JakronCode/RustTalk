//! Headless layout checks: real egui frames at common window sizes, with
//! pointer and keyboard input, asserting on what was painted and where.

use super::servers::tests::{app, login, moved};
use super::*;
use crate::audio::test_devices::Devices;
use egui::epaint::Shape;

pub(in crate::app) const SIZES: [(f32, f32); 2] = [(900.0, 560.0), (1100.0, 700.0)];

/// One full `ui_frame` at `size`; `time` must grow between calls.
pub(in crate::app) fn frame(
    app: &mut RustTalkApp,
    size: (f32, f32),
    time: f64,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let ctx = app.ctx.clone();
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size.0, size.1),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ctx| app.ui_frame(ctx),
    )
}

/// Frames until egui's sizing passes settle.
pub(in crate::app) fn settle(
    app: &mut RustTalkApp,
    size: (f32, f32),
    time: &mut f64,
) -> egui::FullOutput {
    let mut out = frame(app, size, *time, vec![]);
    for _ in 0..3 {
        *time += 0.05;
        out = frame(app, size, *time, vec![]);
    }
    out
}

fn walk<'a>(shape: &'a Shape, f: &mut impl FnMut(&'a Shape)) {
    f(shape);
    if let Shape::Vec(v) = shape {
        v.iter().for_each(|s| walk(s, f));
    }
}

pub(in crate::app) fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    let mut found = vec![];
    for clipped in &out.shapes {
        walk(&clipped.shape, &mut |s| {
            if let Shape::Text(t) = s {
                found.push((
                    t.galley.text().to_string(),
                    egui::Rect::from_min_size(t.pos, t.galley.size()),
                ));
            }
        });
    }
    found
}

pub(in crate::app) fn text_rect(out: &egui::FullOutput, text: &str) -> Option<egui::Rect> {
    texts(out)
        .into_iter()
        .find(|(t, _)| t == text)
        .map(|(_, r)| r)
}

pub(in crate::app) fn has_text(out: &egui::FullOutput, text: &str) -> bool {
    text_rect(out, text).is_some()
}

pub(in crate::app) fn click(
    app: &mut RustTalkApp,
    size: (f32, f32),
    time: &mut f64,
    pos: egui::Pos2,
) {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    *time += 0.05;
    frame(
        app,
        size,
        *time,
        vec![egui::Event::PointerMoved(pos), button(true)],
    );
    *time += 0.05;
    frame(app, size, *time, vec![button(false)]);
    *time += 0.05;
    frame(app, size, *time, vec![]);
}

pub(in crate::app) fn key(app: &mut RustTalkApp, size: (f32, f32), time: &mut f64, key: egui::Key) {
    let ev = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    *time += 0.05;
    frame(app, size, *time, vec![ev(true), ev(false)]);
    *time += 0.05;
    frame(app, size, *time, vec![]);
}

pub(in crate::app) fn type_text(
    app: &mut RustTalkApp,
    size: (f32, f32),
    time: &mut f64,
    text: &str,
) {
    *time += 0.05;
    frame(app, size, *time, vec![egui::Event::Text(text.into())]);
    *time += 0.05;
    frame(app, size, *time, vec![]);
}

/// A viewed, connected server (we are user 1 in the lobby).
pub(in crate::app) fn connect(app: &mut RustTalkApp) -> crate::net::TestCommands {
    let (id, rx) = login(app, "layout.invalid:7331", 1);
    app.view_session(id);
    rx
}

fn assert_inside(inner: egui::Rect, outer: egui::Rect, what: &str) {
    assert!(
        outer.expand(1.0).contains_rect(inner),
        "{what}: {inner:?} not inside {outer:?}"
    );
}

#[test]
fn connection_panel_is_readable_and_scrolls_inside_the_window() {
    for size in SIZES {
        let devices = Devices::new();
        let mut app = app();
        let _rx = connect(&mut app);
        let id = app.srv.id;
        app.join_channel(2);
        moved(&mut app, id, 1, 2);
        assert!(
            app.audio.is_some() && app.bandwidth.is_some(),
            "voice call needs audio and a bandwidth counter"
        );
        app.connection_panel = Some(egui::pos2(12.0, size.1 - 60.0));
        let mut time = 1.0;
        let out = settle(&mut app, size, &mut time);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1));
        let window = app
            .ctx
            .memory(|m| m.area_rect(egui::Id::new("connection_panel")))
            .expect("window shown");

        assert_inside(window, screen, "connection window");
        assert!(
            window.width() >= 380.0,
            "{size:?}: window too narrow: {}",
            window.width()
        );
        assert!(
            window.height() >= 300.0,
            "{size:?}: window too short: {}",
            window.height()
        );
        // Compact header: title sits in the first ~40 px, not in a 54 px bar.
        let title = text_rect(
            &out,
            &format!("{}  Voice Connection", icons::regular::CELL_SIGNAL_FULL),
        )
        .expect("title");
        assert!(
            title.bottom() - window.top() < 40.0,
            "{size:?}: header too thick"
        );
        // Ping graph and stats are on screen at full width.
        let graph = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Rect(r) if r.fill == theme::DARK() && r.rect.height() >= 100.0 => Some(r.rect),
            _ => None,
        });
        let graph = graph.expect("ping graph painted");
        assert!(
            graph.width() >= 340.0,
            "{size:?}: graph too narrow: {}",
            graph.width()
        );
        assert_inside(graph, window, "ping graph");
        let avg = text_rect(&out, "Average ping").expect("stats");
        assert_inside(avg, window, "stats");
        // The first bandwidth graph exists and is reachable by scrolling.
        assert!(has_text(&out, "Application payload bandwidth"));
        assert!(has_text(&out, "KiB/s"));

        // The X closes it.
        let close = text_rect(&out, icons::regular::X).expect("close button");
        click(&mut app, size, &mut time, close.center());
        assert!(app.connection_panel.is_none());
        drop(app);
        drop(devices);
    }
}

#[test]
fn settings_is_the_main_page_connected_and_disconnected() {
    for connected in [false, true] {
        for size in SIZES {
            let _devices = Devices::new();
            let mut app = app();
            let _rx = connected.then(|| connect(&mut app));
            let mut time = 1.0;
            let before = settle(&mut app, size, &mut time);
            if connected {
                assert!(has_text(&before, "channel 2"), "main view shows channels");
            }
            app.open_settings(SettingsPage::Voice);
            let out = settle(&mut app, size, &mut time);

            // One root viewport, no window area for settings, main content replaced.
            assert_eq!(
                out.viewport_output.len(),
                1,
                "settings must not open a native window"
            );
            assert!(has_text(&out, "Voice & Audio"), "page title / nav row");
            assert!(has_text(&out, "USER SETTINGS"), "grouped sidebar");
            assert!(
                !has_text(&out, "channel 2"),
                "channel list must not draw next to settings"
            );
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1));
            for (text, rect) in texts(&out) {
                assert!(rect.is_finite(), "{text}");
                assert!(
                    rect.left() >= -1.0 && rect.top() >= -1.0 && rect.left() <= screen.right(),
                    "{text} at {rect:?}"
                );
            }

            // Navigation through the sidebar.
            let nav = text_rect(&out, "Appearance").expect("nav row");
            click(&mut app, size, &mut time, nav.center());
            assert_eq!(app.settings_page, SettingsPage::Appearance);
            let out = settle(&mut app, size, &mut time);
            assert!(has_text(&out, "Theme"), "Appearance sections");

            // Search: no hit, then clear with Escape (page stays), then Escape closes.
            let search = text_rect(&out, icons::regular::MAGNIFYING_GLASS).expect("search field");
            click(
                &mut app,
                size,
                &mut time,
                search.center() + egui::vec2(60.0, 0.0),
            );
            type_text(&mut app, size, &mut time, "zzzz");
            let out = settle(&mut app, size, &mut time);
            assert!(
                has_text(&out, "No settings match \"zzzz\""),
                "search narrows the nav"
            );
            key(&mut app, size, &mut time, egui::Key::Escape);
            assert!(app.show_settings, "first Escape only clears the search");
            let out = settle(&mut app, size, &mut time);
            assert!(!texts(&out)
                .iter()
                .any(|(t, _)| t.starts_with("No settings match")));
            key(&mut app, size, &mut time, egui::Key::Escape);
            assert!(!app.show_settings, "second Escape closes");
            let out = settle(&mut app, size, &mut time);
            assert!(!has_text(&out, "USER SETTINGS"), "settings page is gone");
            if connected {
                assert!(has_text(&out, "channel 2"), "main page is back");
            }
        }
    }
}

#[test]
fn settings_close_button_returns_and_stops_mic_preview() {
    let devices = Devices::new();
    let mut app = app();
    let _rx = connect(&mut app);
    let size = SIZES[0];
    let mut time = 1.0;
    app.open_settings(SettingsPage::Voice);
    let out = settle(&mut app, size, &mut time);
    app.sync_mic_preview();
    assert!(
        app.mic_monitor.is_some(),
        "voice page previews the microphone"
    );
    let close = text_rect(&out, icons::regular::X).expect("close button");
    click(&mut app, size, &mut time, close.center());
    assert!(!app.show_settings);
    app.sync_mic_preview();
    assert!(
        app.mic_monitor.is_none(),
        "closing settings releases the microphone"
    );
    drop(app);
    drop(devices);
}

#[test]
fn escape_dismisses_poke_before_client_settings() {
    for size in SIZES {
        let _devices = Devices::new();
        let mut app = app();
        let _rx = connect(&mut app);
        let mut time = 1.0;
        app.open_settings(SettingsPage::Appearance);
        settle(&mut app, size, &mut time);
        app.receive_poke(app.srv.id, 2, "Friend".into(), "Hello".into());
        settle(&mut app, size, &mut time);
        key(&mut app, size, &mut time, egui::Key::Escape);
        assert!(app.poke_inbox.is_empty());
        assert!(app.show_settings, "foreground poke must not close settings");
        key(&mut app, size, &mut time, egui::Key::Escape);
        assert!(!app.show_settings);
    }
}
