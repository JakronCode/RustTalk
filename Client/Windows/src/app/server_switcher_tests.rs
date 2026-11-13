use super::super::servers::tests::{
    app, bookmark, chat, closed_port, drain, login, moved, welcome,
};
use super::*;
use crate::audio::test_devices::Devices;

const SCREEN: egui::Vec2 = egui::vec2(360.0, 640.0);

fn run(app: &mut RustTalkApp, time: f64, events: Vec<egui::Event>) -> egui::FullOutput {
    let ctx = app.ctx.clone();
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ctx| {
            let tx = app.srv.tx().filter(|_| app.srv.connected);
            app.ui_sidebar(ctx, tx.as_ref());
            app.ui_remember_prompt(ctx);
        },
    )
}

/// Two frames so layout and hit-testing settle before the next input.
fn settle(app: &mut RustTalkApp, time: &mut f64) -> egui::FullOutput {
    run(app, *time, vec![]);
    *time += 0.05;
    run(app, *time, vec![])
}

fn text_at(out: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
    out.shapes.iter().find_map(|s| match &s.shape {
        egui::epaint::Shape::Text(t) if t.galley.text() == text => {
            Some(t.pos + t.galley.size() / 2.0)
        }
        _ => None,
    })
}

fn pos_of(out: &egui::FullOutput, text: &str) -> egui::Pos2 {
    text_at(out, text).unwrap_or_else(|| panic!("missing rendered text: {text}"))
}

fn click(app: &mut RustTalkApp, pos: egui::Pos2, time: &mut f64) {
    let ev = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    run(app, *time, vec![egui::Event::PointerMoved(pos), ev(true)]);
    *time += 0.05;
    run(app, *time, vec![ev(false)]);
    *time += 0.05;
}

/// Opens the dropdown by clicking the header.
fn open_menu(app: &mut RustTalkApp, time: &mut f64) -> egui::FullOutput {
    settle(app, time);
    click(app, egui::pos2(60.0, 24.0), time);
    settle(app, time)
}

/// A logged-in session whose server announces `name` (rows show it, not the address).
fn login_named(
    app: &mut RustTalkApp,
    address: &str,
    name: &str,
) -> (SessionId, crate::net::TestCommands) {
    let r = login(app, address, 1);
    app.session_mut(r.0).unwrap().server_settings.name = name.into();
    r
}

fn names(prefix: &str) -> [String; 2] {
    [
        format!("{prefix}-a.invalid:7331"),
        format!("{prefix}-b.invalid:7331"),
    ]
}

fn all_text(out: &egui::FullOutput) -> Vec<String> {
    out.shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn dropdown_has_servers_above_actions_and_spans_the_sidebar() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("w");
    let (ia, _ra) = login_named(&mut app, &a, "Alpha");
    login_named(&mut app, &b, "Beta");
    app.view_session(ia);
    let mut t = 0.0;
    let out = open_menu(&mut app, &mut t);
    let settings = pos_of(&out, "Server Settings");
    for name in ["Alpha", "Beta"] {
        assert!(
            pos_of(&out, name).y < settings.y,
            "{name} row above the actions"
        );
    }
    assert!(pos_of(&out, "Beta").y < pos_of(&out, "App Settings").y);
    // The actions' icons sit at the right edge of the full-width menu.
    let width = app.config.sidebar_width.clamp(220.0, 480.0);
    let icon = pos_of(&out, icons::regular::SHIELD_CHECK);
    assert!(
        icon.x > width - 40.0 && icon.x < width,
        "menu spans the sidebar: icon at {icon:?}, sidebar {width}"
    );
    assert!(icon.x < SCREEN.x);
    assert!(text_at(&out, "Report a Bug").is_some() && text_at(&out, "Create Category").is_some());
}

#[test]
fn clicking_a_row_only_views_that_server() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("v");
    let (ia, _ra) = login_named(&mut app, &a, "Alpha");
    let (ib, _rb) = login_named(&mut app, &b, "Beta");
    app.view_session(ia);
    let mut t = 0.0;
    let out = open_menu(&mut app, &mut t);
    let before = app.session(ib).unwrap().generation;
    click(&mut app, pos_of(&out, "Beta"), &mut t);
    assert_eq!(app.srv.id, ib, "row click shows the server");
    assert!(app.session(ia).unwrap().connected && app.session(ib).unwrap().connected);
    assert_eq!(app.session(ib).unwrap().generation, before, "no reconnect");
    assert!(
        text_at(&settle(&mut app, &mut t), "Server Settings").is_none(),
        "dropdown closed after selecting"
    );
}

#[test]
fn row_icon_disconnects_without_selecting() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("d");
    for k in [&a, &b] {
        app.config.bookmarks.push(bookmark(k));
        app.config.auto_connect.insert(k.clone());
    }
    let (ia, _ra) = login_named(&mut app, &a, "Alpha");
    let (ib, _rb) = login_named(&mut app, &b, "Beta");
    app.view_session(ia);
    let mut t = 0.0;
    let out = open_menu(&mut app, &mut t);
    let row = pos_of(&out, "Beta");
    let icon = egui::pos2(
        app.config.sidebar_width.clamp(220.0, 480.0) - 7.0 - 22.0 - 6.0,
        row.y + 8.0,
    );
    click(&mut app, icon, &mut t);
    assert_eq!(app.srv.id, ia, "the icon does not select the row");
    assert!(
        !app.session(ib).is_some_and(|s| s.connected),
        "B disconnected"
    );
    assert!(app.session(ia).unwrap().connected, "A untouched");
    assert!(
        !app.config.auto_connect.contains(&b) && app.config.auto_connect.contains(&a),
        "explicit disconnect persists, only for B"
    );
}

#[test]
fn connect_icon_asks_before_remembering_and_connects_once_on_answer() {
    let _devices = Devices::new();
    let mut app = app();
    let saved = closed_port();
    app.config.bookmarks = vec![bookmark(&saved)];
    let (ia, _ra) = login_named(&mut app, "live.invalid:7331", "Live");
    app.view_session(ia);
    let mut t = 0.0;
    let out = open_menu(&mut app, &mut t);
    let row = pos_of(&out, &saved);
    let icon = egui::pos2(
        app.config.sidebar_width.clamp(220.0, 480.0) - 7.0 - 22.0 - 6.0,
        row.y + 8.0,
    );
    click(&mut app, icon, &mut t);
    assert_eq!(app.srv.id, ia, "the icon does not select");
    let id = app.session_by_key(&saved);
    assert!(
        id.is_none_or(|id| app.session(id).unwrap().cmd_tx.is_none()),
        "nothing connects before the answer"
    );
    assert!(app.remember_prompt.is_some());
    let out = settle(&mut app, &mut t);
    assert!(text_at(&out, "Just now").is_some() && text_at(&out, "Reconnect on start").is_some());
    click(&mut app, pos_of(&out, "Just now"), &mut t);
    let s = app.session(app.session_by_key(&saved).unwrap()).unwrap();
    assert!(
        s.cmd_tx.is_some() && s.generation == 1,
        "connected once, not twice"
    );
    assert!(!s.remember && app.remember_prompt.is_none());
    assert_eq!(app.srv.id, ia, "viewed server unchanged");
    app.handle_event(s.id, welcome(1));
    assert!(
        app.config.auto_connect.is_empty(),
        "connect once never enrolls"
    );
}

#[test]
fn affirmative_answer_enrolls_after_login_and_cancel_does_nothing() {
    let _devices = Devices::new();
    let mut app = app();
    let saved = closed_port();
    app.config.bookmarks = vec![bookmark(&saved)];
    app.request_connect_row(&saved);
    let mut t = 0.0;
    let out = settle(&mut app, &mut t);
    click(&mut app, pos_of(&out, "Cancel"), &mut t);
    assert!(
        app.remember_prompt.is_none()
            && app.session_by_key(&saved).is_none_or(|id| app
                .session(id)
                .unwrap()
                .cmd_tx
                .is_none())
    );
    app.request_connect_row(&saved);
    let out = settle(&mut app, &mut t);
    click(&mut app, pos_of(&out, "Reconnect on start"), &mut t);
    let id = app.session_by_key(&saved).unwrap();
    assert!(app.session(id).unwrap().cmd_tx.is_some());
    assert!(app.config.auto_connect.is_empty(), "not before the login");
    app.handle_event(id, welcome(1));
    assert!(app.config.auto_connect.contains(&saved));
    // Remembered servers connect without asking again.
    app.disconnect_server(&saved);
    app.config.auto_connect.insert(saved.clone());
    app.request_connect_row(&saved);
    assert!(app.remember_prompt.is_none());
    assert!(app.session(id).unwrap().cmd_tx.is_some());
}

#[test]
fn form_route_asks_for_saved_servers_only() {
    let _devices = Devices::new();
    let mut app = app();
    let saved = closed_port();
    app.config.bookmarks = vec![bookmark(&saved)];
    app.config.server_address = closed_port();
    app.request_connect();
    assert!(
        app.remember_prompt.is_none() && app.srv.cmd_tx.is_some(),
        "unsaved address connects once"
    );
    app.srv.cmd_tx = None;
    app.config.server_address = saved.clone();
    app.request_connect();
    assert_eq!(
        app.remember_prompt
            .as_ref()
            .map(|p| (p.key.clone(), p.from_form)),
        Some((saved, true))
    );
}

#[test]
fn disconnected_view_keeps_switcher_voice_and_disables_server_actions() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("x");
    let (ia, mut ra) = login_named(&mut app, &a, "Alpha");
    let (ib, _rb) = login_named(&mut app, &b, "Beta");
    app.view_session(ia);
    app.join_channel(2);
    app.join_channel(2);
    moved(&mut app, ia, 1, 2);
    app.disconnect_server(&b);
    app.config.bookmarks.push(bookmark(&b));
    app.select_server(&b);
    assert_eq!(app.srv.id, ib);
    assert!(!app.is_connected());
    let mut t = 0.0;
    let out = settle(&mut app, &mut t);
    assert!(text_at(&out, "Voice Connected").is_some() || out.shapes.iter().any(|s| matches!(&s.shape, egui::epaint::Shape::Text(t) if t.galley.text().ends_with("Voice Connected"))), "voice control shown while viewing another server");
    assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::epaint::Shape::Text(t) if t.galley.text().contains("on Alpha"))), "names the voice server");
    let out = open_menu(&mut app, &mut t);
    click(&mut app, pos_of(&out, "Server Settings"), &mut t);
    assert!(
        app.server_settings_dialog.is_none(),
        "server action disabled while disconnected"
    );
    assert!(
        text_at(&settle(&mut app, &mut t), "Alpha").is_some(),
        "switcher still open and listing servers"
    );
    // Hang up leaves voice only; the server stays connected.
    app.leave_voice(ia);
    assert!(app.voice_owner.is_none() && app.session(ia).unwrap().connected);
    assert!(drain(&mut ra).iter().any(|c| c == "JoinChannel(1)"));
}

#[test]
fn rows_show_status_unread_and_voice_and_widgets_have_labels() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("s");
    app.config.bookmarks.push(bookmark(&b));
    let (ia, _ra) = login_named(&mut app, &a, "Alpha");
    app.view_session(ia);
    let rows = app.server_rows();
    let ra = rows.iter().find(|r| r.key == a).unwrap();
    let rb = rows.iter().find(|r| r.key == b).unwrap();
    assert_eq!(status_text(ra).0, "Connected");
    assert_eq!(status_text(rb).0, "Disconnected");
    let (live, tip) = toggle_label(ra);
    assert!(
        live && tip.starts_with("Disconnect from") && tip.contains("no messages or notifications")
    );
    let (live, tip) = toggle_label(rb);
    assert!(!live && tip.starts_with("Connect to"));
    // Screen readers get a label per row and per icon.
    app.ctx.enable_accesskit();
    let mut t = 0.0;
    let out = open_menu(&mut app, &mut t);
    let labels: Vec<String> = out
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|u| u.nodes.iter())
        .filter_map(|(_, n)| n.label().map(str::to_string))
        .collect();
    for want in [
        "Alpha, Connected".to_string(),
        "Disconnect from Alpha".to_string(),
        format!("Connect to {b}"),
    ] {
        assert!(
            labels.iter().any(|l| l.starts_with(&want)),
            "missing label {want}: {labels:?}"
        );
    }
}

#[test]
fn add_server_keeps_connections_and_voice_and_leaves_without_disconnecting() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("add");
    let (ia, mut ra) = login_named(&mut app, &a, "Alpha");
    let (ib, _rb) = login_named(&mut app, &b, "Beta");
    app.view_session(ia);
    app.join_channel(2);
    app.join_channel(2);
    moved(&mut app, ia, 1, 2);
    let mut t = 0.0;
    let out = open_menu(&mut app, &mut t);
    assert!(
        pos_of(&out, "Add server").y < pos_of(&out, "Server Settings").y,
        "Add server sits before the settings"
    );
    assert!(
        !all_text(&out)
            .iter()
            .any(|t| t.contains("reconnects a saved") || t.contains("Disconnect stops")),
        "no boilerplate in the menu"
    );
    click(&mut app, pos_of(&out, "Add server"), &mut t);
    assert!(
        app.srv.is_placeholder() && app.show_connect_form,
        "blank connect form"
    );
    assert!(app.session(ia).unwrap().connected && app.session(ib).unwrap().connected);
    assert_eq!(app.voice_owner, Some(ia));
    assert!(app.in_voice());
    let _ = drain(&mut ra);
    // The switcher stays reachable; Alpha's row returns to it.
    let out = open_menu(&mut app, &mut t);
    assert!(
        text_at(&out, "Add server").is_none(),
        "no second blank form"
    );
    click(&mut app, pos_of(&out, "Alpha"), &mut t);
    assert_eq!(app.srv.id, ia);
    assert!(
        app.others.iter().all(|s| !s.is_placeholder()),
        "cancelled form leaves no session behind"
    );
    assert!(app.session(ib).unwrap().connected && app.voice_owner == Some(ia) && app.in_voice());
    assert!(
        drain(&mut ra).is_empty(),
        "nothing sent to the voice server"
    );
    // Add a third server from the form.
    app.add_server();
    app.config.server_address = closed_port();
    app.start_connect(false);
    let third = app.srv.id;
    assert!(![ia, ib].contains(&third));
    assert!(app.session(ia).unwrap().connected && app.session(ib).unwrap().connected);
    assert_eq!(app.voice_owner, Some(ia));
    assert_eq!(app.all_sessions().filter(|s| s.is_placeholder()).count(), 0);
    assert_eq!(app.all_sessions().count(), 3);
}

#[test]
fn viewing_a_server_marks_its_shown_channel_read_only() {
    let _devices = Devices::new();
    let mut app = app();
    let [a, b] = names("unr");
    let (ia, _ra) = login_named(&mut app, &a, "Alpha");
    let (ib, _rb) = login_named(&mut app, &b, "Beta");
    app.view_session(ia);
    let lobby = app.session(ib).unwrap().selected_channel.unwrap();
    chat(&mut app, ib, lobby, "hi");
    chat(&mut app, ib, 3, "elsewhere");
    assert!(app.session(ib).unwrap().unread.contains(&lobby));
    app.view_session(ib);
    assert!(!app.srv.unread.contains(&lobby), "shown channel is read");
    assert!(app.srv.unread.contains(&3), "hidden channel stays unread");
    chat(&mut app, ia, 1, "later");
    assert!(
        app.session(ia).unwrap().unread.contains(&1),
        "other server keeps its unread"
    );
}

#[test]
fn unread_is_plain_text_only_when_unread_and_state_stays_readable_when_narrow() {
    let _devices = Devices::new();
    for width in [220.0_f32, 260.0] {
        let mut app = app();
        app.config.sidebar_width = width;
        let [a, b] = names("lbl");
        let (ia, _ra) = login_named(&mut app, &a, "A very long community name that must elide");
        let (ib, _rb) = login_named(&mut app, &b, "Beta");
        app.view_session(ia);
        let lobby = app.session(ib).unwrap().selected_channel.unwrap();
        let mut t = 0.0;
        let out = open_menu(&mut app, &mut t);
        assert!(
            text_at(&out, "New messages").is_none(),
            "no label without unread"
        );
        assert!(text_at(&out, "Connected").is_some());
        chat(&mut app, ib, lobby, "hi");
        let out = settle(&mut app, &mut t);
        assert!(text_at(&out, "New messages").is_some(), "label at {width}");
        assert_eq!(
            all_text(&out)
                .iter()
                .filter(|text| text.as_str() == "Connected")
                .count(),
            2,
            "both rows retain their complete connection state at {width}"
        );
        assert!(text_at(&out, "Alpha").is_none());
        let pos = pos_of(&out, "New messages");
        assert!(
            pos.x < width - ACTION_W,
            "label clear of the connect icon at {width}: {pos:?}"
        );
    }
}
