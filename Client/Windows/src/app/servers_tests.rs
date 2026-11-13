use super::*;
use crate::audio::test_devices::Devices;
use rusttalk_shared::protocol::{ChannelInfo, ChannelKind, ServerMessage, UserInfo};
use std::net::UdpSocket;
use tokio::sync::mpsc::UnboundedReceiver;

const SPAWN: u64 = 1;
const VOICE: u64 = 2;
const TEXT: u64 = 3;

fn app() -> RustTalkApp {
    RustTalkApp::with_config(
        egui::Context::default(),
        ClientConfig {
            announce_names: false,
            desktop_notifications: false,
            bookmarks: vec![],
            ..ClientConfig::default()
        },
        false,
    )
}

fn bookmark(address: &str) -> ServerBookmark {
    ServerBookmark {
        name: String::new(),
        address: address.into(),
        password: None,
    }
}

/// Server view with us (`my_id`) in the lobby.
fn welcome(my_id: u64) -> NetEvent {
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    udp.connect(udp.local_addr().unwrap()).unwrap();
    NetEvent::Connected {
        my_id,
        settings: Default::default(),
        users: vec![UserInfo {
            id: my_id,
            name: "me".into(),
            channel_id: SPAWN,
            muted: false,
            deafened: false,
            sharing: false,
            role: Default::default(),
            streams: vec![],
        }],
        channels: [
            (SPAWN, ChannelKind::Spawn),
            (VOICE, ChannelKind::Voice),
            (TEXT, ChannelKind::Text),
        ]
        .into_iter()
        .map(|(id, kind)| ChannelInfo {
            id,
            name: format!("channel {id}"),
            kind,
            parent: None,
            position: 0,
            user_ids: vec![],
        })
        .collect(),
        udp_socket: Some(udp),
        udp_cipher: Some(Arc::new(
            rusttalk_shared::secure::PacketCipher::new(&[7; 32]).unwrap(),
        )),
        fingerprint: String::new(),
        udp_token: 42,
        spawn_log: vec![],
        chat_history: vec![],
        gif_search: false,
        open_permissions: false,
    }
}

/// A logged-in session for `address` (a fake transport instead of the network).
fn login(
    app: &mut RustTalkApp,
    address: &str,
    my_id: u64,
) -> (SessionId, UnboundedReceiver<Command>) {
    let id = app.ensure_session(address).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_mut(id).unwrap().cmd_tx = Some(tx);
    app.handle_event(id, welcome(my_id));
    (id, rx)
}

fn moved(app: &mut RustTalkApp, id: SessionId, client_id: u64, channel_id: u64) {
    app.handle_event(
        id,
        NetEvent::Message(ServerMessage::UserMoved {
            client_id,
            channel_id,
        }),
    );
}

fn chat(app: &mut RustTalkApp, id: SessionId, channel_id: u64, text: &str) {
    app.handle_event(
        id,
        NetEvent::Message(ServerMessage::Chat {
            id: 100,
            channel_id,
            from_id: 77,
            from_name: "peer".into(),
            text: text.into(),
            timestamp: 1,
            image: None,
            reply_to: None,
        }),
    );
}

fn drain(rx: &mut UnboundedReceiver<Command>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        out.push(format!("{cmd:?}"));
    }
    out
}

/// A local port nothing listens on: connection attempts fail fast.
fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

#[test]
fn two_servers_keep_separate_chat_unread_and_notifications() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, _rb) = login(&mut app, "b.invalid:7331", 1);
    app.view_session(a);
    app.select_channel(TEXT);
    chat(&mut app, a, TEXT, "open channel");
    chat(&mut app, b, TEXT, "background");
    assert!(
        app.srv.unread.is_empty(),
        "the open channel on the viewed server is read"
    );
    assert_eq!(app.session(b).unwrap().unread, HashSet::from([TEXT]));
    assert_eq!(app.session(a).unwrap().chat[&TEXT].len(), 1);
    assert_eq!(app.session(b).unwrap().chat[&TEXT][0].text, "background");
    let rows = app.server_rows();
    let row = |key: &str| rows.iter().find(|r| r.key == key).unwrap().clone();
    assert!(!row("a.invalid:7331").unread && row("a.invalid:7331").viewed);
    assert!(
        row("b.invalid:7331").unread && row("b.invalid:7331").status == ServerStatus::Connected
    );
    // A clicked toast of server B shows B and its channel, A stays connected.
    app.toast_tx.send((b, TEXT)).unwrap();
    app.poll_background(&egui::Context::default());
    assert_eq!(app.srv.id, b);
    assert_eq!(app.srv.selected_channel, Some(TEXT));
    assert!(app.srv.unread.is_empty());
    assert!(app.session(a).unwrap().connected);
    // Unknown channels from a server do not grow state.
    chat(&mut app, b, 999, "nowhere");
    assert!(!app.srv.chat.contains_key(&999) && !app.srv.unread.contains(&999));
}

#[test]
fn switching_view_keeps_voice_and_closes_server_bound_dialogs() {
    let devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, _rb) = login(&mut app, "b.invalid:7331", 5);
    app.view_session(a);
    app.join_channel(VOICE);
    assert!(drain(&mut ra).iter().any(|c| c == "JoinChannel(2)"));
    moved(&mut app, a, 1, VOICE);
    assert_eq!(devices.0.borrow().inputs, 1);
    app.chat_input = "draft for A".into();
    app.reply_to = Some(3);
    app.confirm_delete = Some(TEXT);
    app.view_session(b);
    assert_eq!(
        app.voice_owner,
        Some(a),
        "viewing another server keeps voice"
    );
    assert_eq!(devices.0.borrow().inputs, 1);
    assert_eq!(
        app.voice_server().map(|v| v.0),
        Some("a.invalid:7331".to_string())
    );
    assert!(
        app.reply_to.is_none() && app.confirm_delete.is_none(),
        "dialogs cannot act on B"
    );
    assert!(app.chat_input.is_empty());
    assert!(app
        .server_rows()
        .iter()
        .any(|r| r.key == "a.invalid:7331" && r.voice && !r.viewed));
    // Viewing B: A's talking map and mixer controls are not shown against B's ids.
    app.talking.lock().unwrap().insert(5, Instant::now());
    assert!(app.voice_maps().0.is_empty());
    app.view_session(a);
    assert_eq!(app.chat_input, "draft for A");
    assert!(!app.voice_maps().0.is_empty());
}

#[test]
fn joining_voice_elsewhere_leaves_the_old_voice_first() {
    let devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, mut rb) = login(&mut app, "b.invalid:7331", 5);
    app.view_session(a);
    app.join_channel(VOICE);
    moved(&mut app, a, 1, VOICE);
    app.audio_only = true; // sharing computer audio on A
    app.screen_audio_failed = false;
    app.sync_shared_audio();
    assert_eq!(devices.0.borrow().systems, 1);
    drain(&mut ra);
    let a_session = app.control.link.session();
    assert_eq!(app.control.link.accept(a_session, 7), Some(true));

    app.view_session(b);
    app.join_channel(VOICE);
    let to_a = drain(&mut ra);
    assert_eq!(
        to_a.last().map(String::as_str),
        Some("JoinChannel(1)"),
        "A: back to its lobby, {to_a:?}"
    );
    assert_eq!(drain(&mut rb), vec!["JoinChannel(2)".to_string()]);
    assert_eq!(devices.0.borrow().inputs, 0, "no capture until B confirms");
    assert_eq!(
        devices.0.borrow().systems,
        0,
        "sharing does not follow to B"
    );
    assert_eq!(app.voice_owner, Some(b));

    moved(&mut app, b, 5, VOICE);
    assert_eq!(devices.0.borrow().inputs, 1);
    assert_eq!(app.audio_owner, Some(b));
    assert_eq!(
        devices.0.borrow().systems,
        0,
        "never restarted automatically"
    );
    // A's late confirmation, and A pulling us back: B keeps the only voice.
    moved(&mut app, a, 1, SPAWN);
    moved(&mut app, a, 1, VOICE);
    assert_eq!(drain(&mut ra), vec!["JoinChannel(1)".to_string()]);
    assert_eq!(app.voice_owner, Some(b));
    assert_eq!(devices.0.borrow().inputs, 1);

    // Voice back to the same A connection: its old packets are still replays.
    moved(&mut app, a, 1, SPAWN);
    app.view_session(a);
    app.join_channel(VOICE);
    moved(&mut app, a, 1, VOICE);
    assert_eq!(app.audio_owner, Some(a));
    assert_eq!(
        app.control.link.accept(app.control.link.session(), 7),
        Some(false)
    );
}

#[test]
fn background_server_cannot_open_the_microphone() {
    let devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, mut rb) = login(&mut app, "b.invalid:7331", 5);
    app.view_session(a);
    moved(&mut app, b, 5, VOICE);
    assert_eq!(drain(&mut rb), vec!["JoinChannel(1)".to_string()]);
    assert_eq!(app.voice_owner, None);
    assert_eq!(devices.0.borrow().inputs, 0);
    assert!(app.status_is_error);
}

#[test]
fn stale_events_cannot_touch_another_or_a_replaced_session() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, _rb) = login(&mut app, "b.invalid:7331", 1);
    let old_a = app.session(a).unwrap().event_tx.clone();
    app.handle_event(
        a,
        NetEvent::Disconnected {
            reason: "connection lost".into(),
        },
    );
    assert!(
        old_a.send(welcome(1)).is_err(),
        "late events of A's old attempt have no receiver"
    );
    assert!(app.session(a).unwrap().reconnect.is_some());
    assert!(
        app.session(b).unwrap().connected,
        "A failing leaves B connected"
    );
    // B's queue only feeds B.
    let b_tx = app.session(b).unwrap().event_tx.clone();
    b_tx.send(NetEvent::Message(ServerMessage::SpawnLogEntry {
        entry: rusttalk_shared::protocol::LogEntry {
            text: "b joined".into(),
            timestamp: 1,
        },
    }))
    .unwrap();
    app.poll_sessions();
    assert_eq!(app.session(b).unwrap().chat[&SPAWN].len(), 1);
    assert!(app.session(a).unwrap().chat.is_empty());
}

#[test]
fn fatal_and_certificate_failures_stop_retrying() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, _rb) = login(&mut app, "b.invalid:7331", 1);
    app.view_session(a);
    app.handle_event(
        b,
        NetEvent::Disconnected {
            reason: "Wrong or missing server password".into(),
        },
    );
    let s = app.session(b).unwrap();
    assert!(s.reconnect.is_none() && s.status() == ServerStatus::Failed);
    let (c, _rc) = login(&mut app, "c.invalid:7331", 1);
    app.handle_event(
        c,
        NetEvent::CertCheck {
            addr: "c.invalid:7331".into(),
            fingerprint: "AA".into(),
            changed: true,
        },
    );
    let s = app.session(c).unwrap();
    assert!(s.reconnect.is_none() && s.cert_check.is_some() && s.cmd_tx.is_none());
    assert!(
        app.status_is_error && app.status.starts_with("c.invalid:7331"),
        "{}",
        app.status
    );
    // Startup/resume leaves servers alone that wait for the user.
    app.config.bookmarks = vec![bookmark("b.invalid:7331"), bookmark("c.invalid:7331")];
    for key in ["b.invalid:7331", "c.invalid:7331"] {
        app.config.auto_connect.insert(key.into());
        app.config.known_servers.insert(key.into(), "AA".into());
    }
    app.connect_enrolled_servers();
    assert!(app.session(b).unwrap().cmd_tx.is_none() && app.session(c).unwrap().cmd_tx.is_none());
    assert!(app.session(a).unwrap().connected);
}

#[test]
fn startup_connects_only_enrolled_trusted_saved_servers_without_media() {
    let devices = Devices::new();
    let mut app = app();
    let enrolled = closed_port();
    let unpinned = closed_port();
    let not_enrolled = closed_port();
    app.config.bookmarks = vec![
        ServerBookmark {
            name: "official".into(),
            address: "public.rusttalk.com:7331".into(),
            password: None,
        },
        bookmark(&enrolled),
        bookmark(&unpinned),
        bookmark(&not_enrolled),
    ];
    app.config
        .known_servers
        .insert(enrolled.clone(), "AA".into());
    app.config
        .known_servers
        .insert(not_enrolled.clone(), "AA".into());
    app.config.auto_connect = [
        enrolled.clone(),
        unpinned.clone(),
        "gone.invalid:7331".to_string(),
    ]
    .into();
    app.connect_enrolled_servers();
    let started: Vec<String> = app
        .all_sessions()
        .filter(|s| s.is_active())
        .map(|s| s.key.clone())
        .collect();
    assert_eq!(started, vec![enrolled.clone()]);
    assert!(
        !app.config.auto_connect.contains("gone.invalid:7331"),
        "removed servers lose consent"
    );
    assert_eq!(app.srv.key, enrolled, "the started server is shown");
    let generation = app.srv.generation;
    app.connect_enrolled_servers(); // duplicate launch / resume
    assert_eq!(app.srv.generation, generation, "no second attempt");
    assert!(app.voice_owner.is_none() && app.audio.is_none());
    assert_eq!(devices.0.borrow().input_starts, 0);
}

#[test]
fn startup_and_retries_respect_the_session_and_attempt_limits() {
    let mut app = app();
    let addrs: Vec<String> = (0..MAX_ACTIVE_SESSIONS + 2)
        .map(|_| closed_port())
        .collect();
    for a in &addrs {
        app.config.bookmarks.push(bookmark(a));
        app.config.known_servers.insert(a.clone(), "AA".into());
        app.config.auto_connect.insert(a.clone());
    }
    app.connect_enrolled_servers();
    let active = app.all_sessions().filter(|s| s.is_active()).count();
    let overflow: Vec<&ServerSession> = app.all_sessions().filter(|s| s.overflow).collect();
    assert_eq!(active, MAX_ACTIVE_SESSIONS);
    assert_eq!(
        overflow.len(),
        2,
        "servers over the limit are shown, not dropped"
    );
    assert!(overflow
        .iter()
        .all(|s| s.status() == ServerStatus::LimitReached && s.last_error.is_some()));
    assert_eq!(
        app.all_sessions().filter(|s| s.is_connecting()).count(),
        MAX_AUTO_CONNECTING
    );
    assert!(
        app.server_rows()
            .iter()
            .filter(|r| r.status == ServerStatus::Waiting)
            .count()
            >= MAX_ACTIVE_SESSIONS - MAX_AUTO_CONNECTING
    );
    // An explicit connect beyond the limit fails with a reason instead of opening a ninth.
    let extra = overflow[0].key.clone();
    app.connect_server(&extra);
    assert_eq!(
        app.all_sessions().filter(|s| s.is_active()).count(),
        MAX_ACTIVE_SESSIONS
    );
    assert!(app.status_is_error);
}

#[test]
fn explicit_disconnect_persists_and_beats_reconnects() {
    let _devices = Devices::new();
    let mut app = app();
    let key = "a.invalid:7331";
    app.config.bookmarks = vec![bookmark(key)];
    app.config.known_servers.insert(key.into(), "AA".into());
    app.config.auto_connect.insert(key.into());
    let (a, mut ra) = login(&mut app, key, 1);
    app.view_session(a);
    app.handle_event(
        a,
        NetEvent::Disconnected {
            reason: "connection lost".into(),
        },
    );
    assert!(app.srv.reconnect.is_some());
    app.disconnect_server(key);
    assert!(app.srv.reconnect.is_none() && app.srv.cmd_tx.is_none());
    assert!(
        !app.config.auto_connect.contains(key),
        "stays disconnected after a restart"
    );
    assert!(drain(&mut ra).iter().any(|c| c == "Disconnect"));
    app.connect_enrolled_servers();
    app.poll_reconnect();
    assert!(app.srv.cmd_tx.is_none() && app.srv.reconnect.is_none());
}

#[test]
fn only_an_explicit_login_to_a_saved_server_enrolls_it() {
    let _devices = Devices::new();
    let mut app = app();
    let saved = closed_port();
    app.config.bookmarks = vec![bookmark(&saved)];
    app.config.known_servers.insert(saved.clone(), "AA".into());
    app.config.server_address = saved.clone();
    app.start_connect();
    assert!(app.srv.explicit && app.srv.cmd_tx.is_some());
    assert!(
        app.config.auto_connect.is_empty(),
        "not before the login succeeded"
    );
    app.handle_event(app.srv.id, welcome(1));
    assert!(app.config.auto_connect.contains(&saved));
    // An unsaved address is never enrolled.
    let (u, _ru) = login(&mut app, "unsaved.invalid:7331", 1);
    app.session_mut(u).unwrap().explicit = true;
    app.handle_event(u, welcome(1));
    assert_eq!(app.config.auto_connect.len(), 1);
}

#[test]
fn mute_state_is_global_and_reaches_every_server() {
    let _devices = Devices::new();
    let mut app = app();
    let (_a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    app.toggle_mute();
    let (_b, mut rb) = login(&mut app, "b.invalid:7331", 1);
    assert!(drain(&mut ra)
        .iter()
        .any(|c| c.contains("SetMute { muted: true")));
    assert!(
        drain(&mut rb)
            .iter()
            .any(|c| c.contains("SetMute { muted: true")),
        "a new server learns it at login"
    );
    app.toggle_mute();
    assert!(drain(&mut rb)
        .iter()
        .any(|c| c.contains("SetMute { muted: false")));
}

#[test]
fn chat_memory_per_session_is_bounded() {
    let mut s = ServerSession::new(1, "a.invalid");
    s.channels.insert(
        TEXT,
        ChannelInfo {
            id: TEXT,
            name: "t".into(),
            kind: ChannelKind::Text,
            parent: None,
            position: 0,
            user_ids: vec![],
        },
    );
    for i in 0..session::MAX_LINES_PER_CHANNEL as u64 + 10 {
        s.push_line(TEXT, ChatLine::system(format!("{i}"), i));
    }
    assert_eq!(s.chat[&TEXT].len(), session::MAX_LINES_PER_CHANNEL);
    assert_eq!(s.chat[&TEXT][0].text, "10", "oldest lines go first");
    let big = rusttalk_shared::protocol::MAX_MEDIA_BYTES;
    for i in 0..(session::MAX_IMAGE_BYTES / big + 3) as u64 {
        let mut line = ChatLine::system(String::new(), 10_000 + i);
        line.image = Some(vec![0; big]);
        s.push_line(TEXT, line);
    }
    assert!(s.image_bytes <= session::MAX_IMAGE_BYTES);
    let kept: usize = s.chat[&TEXT]
        .iter()
        .filter_map(|l| l.image.as_ref())
        .map(Vec::len)
        .sum();
    assert_eq!(kept, s.image_bytes, "counter matches what is held");
    assert!(
        s.chat[&TEXT]
            .iter()
            .rev()
            .take(3)
            .all(|l| l.image.is_some()),
        "newest images stay"
    );
    assert!(
        !s.push_line(42, ChatLine::system("x".into(), 0)),
        "unknown channel"
    );
}
