use super::*;

fn app() -> RustTalkApp {
    let config = ClientConfig {
        server_address: "unselected.invalid".into(),
        bookmarks: vec![ServerBookmark {
            name: "Example".into(),
            address: "example.invalid:7331".into(),
            password: Some("saved-password".into()),
        }],
        ..Default::default()
    };
    RustTalkApp::with_config(egui::Context::default(), config, false)
}

fn frame(app: &mut RustTalkApp, time: f64, events: Vec<egui::Event>) -> (bool, egui::FullOutput) {
    let ctx = app.ctx.clone();
    let mut connect = false;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(340.0, 440.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(300.0);
                connect = app.saved_servers(ui);
            });
        },
    );
    (connect, output)
}

fn text_position(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(t) if t.galley.text() == text => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing rendered text: {text}"))
}

fn click(app: &mut RustTalkApp, pos: egui::Pos2, time: f64, button: egui::PointerButton) -> bool {
    let event = |pressed| egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(app, time, vec![egui::Event::PointerMoved(pos), event(true)]);
    frame(app, time + 0.01, vec![event(false)]).0
}

#[test]
fn single_click_selects_and_double_click_requests_connection_without_opening_editor() {
    let mut app = app();
    let pos = text_position(&frame(&mut app, 0.0, vec![]).1, "Example");
    app.config.invite = Some("stale-invite".into());
    assert!(!click(&mut app, pos, 0.1, egui::PointerButton::Primary));
    assert_eq!(app.config.server_address, "example.invalid:7331");
    assert_eq!(
        app.config.server_password.as_deref(),
        Some("saved-password")
    );
    assert!(app.config.invite.is_none());
    assert!(!app.show_connect_form);
    assert!(app.srv.cmd_tx.is_none());
    assert!(click(&mut app, pos, 0.2, egui::PointerButton::Primary));
    assert!(!app.show_connect_form);
}

#[test]
fn footer_connect_requires_existing_selection_and_reloads_saved_credentials() {
    let mut app = app();
    frame(&mut app, 0.0, vec![]);
    let output = frame(&mut app, 0.01, vec![]).1;
    let button = text_position(&output, "Connect");
    let settings = text_position(&output, &format!("{}  Settings", icons::regular::GEAR_SIX));
    assert!(button.x > settings.x);
    assert!(
        (button.y - settings.y).abs() < 5.0,
        "Connect {button:?}, Settings {settings:?}"
    );
    assert!(!click(&mut app, button, 0.1, egui::PointerButton::Primary));
    let row = text_position(&output, "Example");
    assert!(!click(&mut app, row, 1.0, egui::PointerButton::Primary));
    app.config.server_password = Some("unsaved-password".into());
    assert!(click(&mut app, button, 2.0, egui::PointerButton::Primary));
    assert_eq!(
        app.config.server_password.as_deref(),
        Some("saved-password")
    );
    app.config.bookmarks.clear();
    frame(&mut app, 3.0, vec![]);
    assert!(!click(&mut app, button, 3.1, egui::PointerButton::Primary));
}

#[test]
fn right_click_edit_opens_details_without_connecting() {
    let mut app = app();
    let pos = text_position(&frame(&mut app, 0.0, vec![]).1, "Example");
    assert!(!click(&mut app, pos, 0.1, egui::PointerButton::Secondary));
    frame(&mut app, 0.2, vec![]);
    let output = frame(&mut app, 0.3, vec![]).1;
    let edit = text_position(&output, "Edit");
    assert!(!click(&mut app, edit, 0.4, egui::PointerButton::Primary));
    assert!(app.show_connect_form);
    assert_eq!(
        app.editing_bookmark.as_deref(),
        Some("example.invalid:7331")
    );
    assert_eq!(app.config.server_address, "example.invalid:7331");
    assert!(app.srv.cmd_tx.is_none());
}

#[test]
fn editing_address_updates_original_bookmark_and_rejects_collisions() {
    let mut app = app();
    app.editing_bookmark = Some(app.config.bookmarks[0].address.clone());
    app.config.server_address = " changed.invalid:7331 ".into();
    app.server_nickname = "Changed".into();
    app.config.server_password = None;
    app.update_saved_server().unwrap();
    assert_eq!(app.config.bookmarks.len(), 1);
    assert_eq!(app.config.bookmarks[0].address, "changed.invalid:7331");
    assert_eq!(app.config.bookmarks[0].nickname(), Some("Changed"));
    assert!(app.config.bookmarks[0].password.is_none());
    assert_eq!(app.config.server_address, "changed.invalid:7331");

    app.config.bookmarks.push(ServerBookmark {
        name: "Other".into(),
        address: "other.invalid:7331".into(),
        password: Some("other-password".into()),
    });
    app.config.server_address = "other.invalid:7331".into();
    assert!(app.update_saved_server().is_err());
    assert_eq!(app.config.bookmarks[0].address, "changed.invalid:7331");
    assert_eq!(app.config.bookmarks[1].nickname(), Some("Other"));
    assert_eq!(
        app.config.bookmarks[1].password.as_deref(),
        Some("other-password")
    );
}

#[test]
fn removing_another_bookmark_preserves_edit_collision_protection() {
    let mut app = app();
    for (name, address) in [
        ("Other", "other.invalid"),
        ("Unrelated", "unrelated.invalid"),
    ] {
        app.config.bookmarks.push(ServerBookmark {
            name: name.into(),
            address: address.into(),
            password: Some(name.into()),
        });
    }
    app.editing_bookmark = Some("example.invalid:7331".into());
    app.show_connect_form = true;
    app.config.server_address = "other.invalid".into();
    app.server_nickname = "Must not overwrite Other".into();
    app.config.server_password = None;
    let row = text_position(&frame(&mut app, 0.0, vec![]).1, "Unrelated");
    assert!(!click(&mut app, row, 0.1, egui::PointerButton::Secondary));
    frame(&mut app, 0.2, vec![]);
    let remove = text_position(&frame(&mut app, 0.3, vec![]).1, "Remove");
    assert!(!click(&mut app, remove, 0.4, egui::PointerButton::Primary));
    assert_eq!(app.config.bookmarks.len(), 2);
    assert_eq!(
        app.editing_bookmark.as_deref(),
        Some("example.invalid:7331")
    );
    assert!(app.update_saved_server().is_err());
    assert_eq!(app.config.bookmarks[1].nickname(), Some("Other"));
    assert_eq!(app.config.bookmarks[1].password.as_deref(), Some("Other"));

    // Deletion through Settings must not turn a stale edit into an overwrite either.
    app.config.bookmarks.remove(0);
    assert!(app.update_saved_server().is_err());
    assert_eq!(app.config.bookmarks[0].nickname(), Some("Other"));
    assert_eq!(app.config.bookmarks[0].password.as_deref(), Some("Other"));
}
