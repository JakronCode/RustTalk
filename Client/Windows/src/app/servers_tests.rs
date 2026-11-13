use super::*;
use crate::audio::test_devices::Devices;
use crate::net::{Connection, TestCommands};
use rusttalk_shared::protocol::{ChannelInfo, ChannelKind, ServerMessage, UserInfo};
use std::net::UdpSocket;

const SPAWN: u64 = 1;
const VOICE: u64 = 2;
const TEXT: u64 = 3;

pub(in crate::app) fn app() -> RustTalkApp {
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

pub(in crate::app) fn bookmark(address: &str) -> ServerBookmark {
    ServerBookmark {
        name: String::new(),
        address: address.into(),
        password: None,
    }
}

/// Server view with us (`my_id`) in the lobby.
pub(in crate::app) fn welcome(my_id: u64) -> NetEvent {
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
pub(in crate::app) fn login(
    app: &mut RustTalkApp,
    address: &str,
    my_id: u64,
) -> (SessionId, TestCommands) {
    let id = app.ensure_session(address).unwrap();
    let (conn, rx) = Connection::test_pair();
    app.session_mut(id).unwrap().cmd_tx = Some(conn);
    app.handle_event(id, welcome(my_id));
    (id, rx)
}

fn message(app: &mut RustTalkApp, id: SessionId, msg: ServerMessage) {
    app.handle_event(id, NetEvent::Message(msg));
}

/// Our explicit voice join on the viewed server, confirmed by the server.
fn join_voice(app: &mut RustTalkApp, channel_id: u64) {
    let id = app.srv.id;
    let me = app.srv.my_id;
    app.join_channel(channel_id);
    moved(app, id, me, channel_id);
}

fn channel(id: u64, kind: ChannelKind, name: String) -> ChannelInfo {
    ChannelInfo {
        id,
        name,
        kind,
        parent: None,
        position: 0,
        user_ids: vec![],
    }
}

pub(in crate::app) fn moved(app: &mut RustTalkApp, id: SessionId, client_id: u64, channel_id: u64) {
    app.handle_event(
        id,
        NetEvent::Message(ServerMessage::UserMoved {
            client_id,
            channel_id,
        }),
    );
}

pub(in crate::app) fn chat(app: &mut RustTalkApp, id: SessionId, channel_id: u64, text: &str) {
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

/// Commands sent so far, and "Disconnect" once the connection was dropped.
pub(in crate::app) fn drain(rx: &mut TestCommands) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(cmd) = rx.try_recv() {
        out.push(format!("{cmd:?}"));
    }
    if rx.cancelled() {
        out.push("Disconnect".into());
    }
    out
}

/// A local port nothing listens on: connection attempts fail fast.
pub(in crate::app) fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// Distinct local addresses nothing listens on: connection attempts are
/// refused at once. The ports stay bound (not listening) while the
/// returned sockets live, so no other test can open a server on one.
pub(in crate::app) fn closed_ports(n: usize) -> (Vec<String>, Vec<tokio::net::TcpSocket>) {
    let held: Vec<tokio::net::TcpSocket> = (0..n)
        .map(|_| {
            let s = tokio::net::TcpSocket::new_v4().unwrap();
            s.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            s
        })
        .collect();
    let addrs = held
        .iter()
        .map(|s| s.local_addr().unwrap().to_string())
        .collect();
    (addrs, held)
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
    assert_eq!(
        drain(&mut ra),
        vec![
            "SetMute { muted: false, deafened: false }",
            "JoinChannel(2)"
        ],
        "the voice server learns the mute state before capture"
    );
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
    assert!(
        to_a.ends_with(&[
            "JoinChannel(1)".into(),
            "SetMute { muted: true, deafened: false }".into()
        ]),
        "A: back to its lobby, shown muted from now on: {to_a:?}"
    );
    assert_eq!(
        drain(&mut rb),
        vec![
            "SetMute { muted: false, deafened: false }",
            "JoinChannel(2)"
        ]
    );
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
        old_a.try_send(welcome(1), 0).is_err(),
        "late events of A's old attempt have no receiver"
    );
    assert!(app.session(a).unwrap().reconnect.is_some());
    assert!(
        app.session(b).unwrap().connected,
        "A failing leaves B connected"
    );
    // B's queue only feeds B.
    let b_tx = app.session(b).unwrap().event_tx.clone();
    b_tx.try_send(
        NetEvent::Message(ServerMessage::SpawnLogEntry {
            entry: rusttalk_shared::protocol::LogEntry {
                text: "b joined".into(),
                timestamp: 1,
            },
        }),
        0,
    )
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
    let (ports, _held) = closed_ports(3);
    let [enrolled, unpinned, not_enrolled] = <[String; 3]>::try_from(ports).unwrap();
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
    let (addrs, _held) = closed_ports(MAX_ACTIVE_SESSIONS + 2);
    assert_eq!(
        addrs.iter().collect::<HashSet<_>>().len(),
        addrs.len(),
        "distinct"
    );
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
    app.connect_server(&extra, false);
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
fn only_connect_and_remember_enrolls_a_saved_server() {
    let _devices = Devices::new();
    let mut app = app();
    let (ports, _held) = closed_ports(3);
    let (once, remembered, via_row) = (ports[0].clone(), ports[1].clone(), ports[2].clone());
    app.config.bookmarks = vec![bookmark(&once), bookmark(&remembered), bookmark(&via_row)];
    // Connect once (form route): a successful login enrolls nothing.
    app.config.server_address = once.clone();
    app.start_connect(false);
    assert!(!app.srv.remember && app.srv.cmd_tx.is_some());
    app.handle_event(app.srv.id, welcome(1));
    assert!(app.config.auto_connect.is_empty());
    // Switcher route without the choice: same.
    app.connect_server(&via_row, false);
    let id = app.session_by_key(&via_row).unwrap();
    app.handle_event(id, welcome(1));
    assert!(app.config.auto_connect.is_empty());
    // Connect and remember: nothing before the login, enrolled after it.
    app.config.server_address = remembered.clone();
    app.start_connect(true);
    assert!(app.srv.remember && app.config.auto_connect.is_empty());
    app.handle_event(app.srv.id, welcome(1));
    assert_eq!(
        app.config.auto_connect,
        std::collections::BTreeSet::from([remembered.clone()])
    );
    // The choice is spent by the login: a later connect once does not renew it.
    assert!(!app.srv.remember);
    // An unsaved address is never enrolled, even asked to be remembered.
    let (u, _ru) = login(&mut app, "unsaved.invalid:7331", 1);
    app.session_mut(u).unwrap().remember = true;
    app.handle_event(u, welcome(1));
    assert_eq!(app.config.auto_connect.len(), 1);
}

#[test]
fn pending_voice_join_receives_toggles_without_starting_capture() {
    let devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (_b, mut rb) = login(&mut app, "b.invalid:7331", 1);
    app.view_session(a);
    app.toggle_deafen();
    app.join_channel(VOICE);
    assert_eq!(
        drain(&mut ra),
        vec!["SetMute { muted: true, deafened: true }", "JoinChannel(2)"]
    );
    app.toggle_deafen();
    app.toggle_mute();
    app.toggle_mute();
    assert_eq!(
        drain(&mut ra),
        vec![
            "SetMute { muted: false, deafened: false }",
            "SetMute { muted: true, deafened: false }",
            "SetMute { muted: false, deafened: false }",
        ]
    );
    assert_eq!(devices.0.borrow().input_starts, 0);
    assert!(app.voice().is_none());
    assert!(
        drain(&mut rb).is_empty(),
        "background chat receives no metadata"
    );
    moved(&mut app, a, 1, VOICE);
    assert_eq!(app.mute_state(), (false, false));
    assert_eq!(devices.0.borrow().input_starts, 1);
    assert!(drain(&mut rb).is_empty());
}

#[test]
fn mute_state_reaches_only_the_voice_server() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, mut rb) = login(&mut app, "b.invalid:7331", 1);
    app.toggle_mute();
    assert!(
        drain(&mut ra).is_empty() && drain(&mut rb).is_empty(),
        "chat-only servers learn nothing"
    );
    app.view_session(a);
    join_voice(&mut app, VOICE);
    assert_eq!(
        drain(&mut ra),
        vec!["SetMute { muted: true, deafened: false }", "JoinChannel(2)"]
    );
    app.toggle_mute();
    assert_eq!(
        drain(&mut ra),
        vec!["SetMute { muted: false, deafened: false }"]
    );
    assert!(
        drain(&mut rb).is_empty(),
        "no broadcast to the background server"
    );
    // Voice moves to B: A is told once that we are muted, then nothing more.
    app.view_session(b);
    app.toggle_deafen();
    assert_eq!(
        drain(&mut ra),
        vec!["SetMute { muted: true, deafened: true }"]
    );
    join_voice(&mut app, VOICE);
    assert_eq!(
        drain(&mut rb),
        vec!["SetMute { muted: true, deafened: true }", "JoinChannel(2)"]
    );
    assert_eq!(
        drain(&mut ra),
        vec!["JoinChannel(1)", "SetMute { muted: true, deafened: false }"]
    );
    app.toggle_deafen();
    app.toggle_mute();
    assert!(drain(&mut ra).is_empty(), "A learns no later toggles");
    assert_eq!(drain(&mut rb).len(), 2);
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

// ----- Voice authorization ---------------------------------------------------

#[test]
fn unsolicited_moves_never_open_the_microphone_and_cannot_make_us_flood() {
    let devices = Devices::new();
    let mut app = app();
    // A completed (startup) login of the viewed, idle server, then a hostile move.
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    assert_eq!(app.srv.id, a);
    for _ in 0..50 {
        moved(&mut app, a, 1, VOICE);
        moved(&mut app, a, 1, SPAWN);
    }
    moved(&mut app, a, 1, VOICE);
    assert_eq!(app.voice_owner, None);
    assert_eq!(devices.0.borrow().input_starts, 0);
    assert_eq!(
        drain(&mut ra),
        vec!["JoinChannel(1)"],
        "one lobby request per interval, however often the server moves us"
    );
    assert!(app.status_is_error);
    // Our own join still works.
    join_voice(&mut app, VOICE);
    assert_eq!(app.voice_owner, Some(a));
    assert_eq!(devices.0.borrow().inputs, 1);
}

#[test]
fn mirrored_voice_membership_cannot_confirm_a_pending_join() {
    for in_welcome in [false, true] {
        let devices = Devices::new();
        let mut app = app();
        let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
        app.view_session(a);
        if in_welcome {
            let mut ev = welcome(1);
            if let NetEvent::Connected { users, .. } = &mut ev {
                users.iter_mut().find(|u| u.id == 1).unwrap().channel_id = VOICE;
            }
            app.handle_event(a, ev);
        } else {
            moved(&mut app, a, 1, VOICE);
        }
        message(
            &mut app,
            a,
            ServerMessage::ChannelCreated {
                channel: channel(4, ChannelKind::Voice, "requested".into()),
            },
        );
        app.join_channel(4);
        assert!(app.voice().is_none());
        message(
            &mut app,
            a,
            ServerMessage::ChannelDeleted { channel_id: TEXT },
        );
        let (_b, _rb) = login(&mut app, "b.invalid:7331", 5);
        app.sync_voice_audio();
        assert_eq!(
            devices.0.borrow().input_starts,
            0,
            "no capture before matching acknowledgement"
        );
        moved(&mut app, a, 1, VOICE);
        assert_eq!(
            app.voice_owner, None,
            "old mirrored membership cannot confirm a different join"
        );
        assert_eq!(devices.0.borrow().input_starts, 0);
        app.view_session(a);
        app.join_channel(4);
        moved(&mut app, a, 1, 4);
        assert_eq!(
            devices.0.borrow().input_starts,
            1,
            "matching explicit acknowledgement permits capture"
        );
    }
}

#[test]
fn cancelling_pending_voice_stops_mute_metadata_before_and_after_lobby_ack() {
    let devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, mut rb) = login(&mut app, "b.invalid:7331", 1);
    app.view_session(a);
    moved(&mut app, a, 1, VOICE);
    message(
        &mut app,
        a,
        ServerMessage::ChannelCreated {
            channel: channel(4, ChannelKind::Voice, "requested".into()),
        },
    );
    app.join_channel(4);
    app.join_channel(SPAWN);
    drain(&mut ra);
    app.view_session(b);
    for acknowledged in [false, true] {
        if acknowledged {
            moved(&mut app, a, 1, SPAWN);
        }
        app.toggle_mute();
        app.toggle_deafen();
        assert!(
            drain(&mut ra).is_empty(),
            "cancelled join must not receive toggles"
        );
        assert!(
            drain(&mut rb).is_empty(),
            "viewing chat is not voice consent"
        );
        assert_eq!(devices.0.borrow().input_starts, 0);
    }
    moved(&mut app, a, 1, 4);
    assert!(
        app.voice().is_none(),
        "late voice acknowledgement stays unauthorized"
    );
    assert_eq!(devices.0.borrow().input_starts, 0);
}

#[test]
fn a_pending_join_only_accepts_the_requested_channel() {
    let devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    message(
        &mut app,
        a,
        ServerMessage::ChannelCreated {
            channel: channel(4, ChannelKind::Voice, "other".into()),
        },
    );
    app.join_channel(VOICE);
    drain(&mut ra);
    moved(&mut app, a, 1, 4);
    assert_eq!(app.voice_owner, None, "not what we asked for");
    assert_eq!(devices.0.borrow().input_starts, 0);
    assert!(drain(&mut ra).contains(&"JoinChannel(1)".to_string()));
    // Clicking the lobby cancels a voice join still on its way.
    moved(&mut app, a, 1, SPAWN);
    app.session_mut(a).unwrap().bounced_at = None;
    app.join_channel(VOICE);
    app.join_channel(SPAWN);
    moved(&mut app, a, 1, VOICE); // the late answer to the first request
    assert_eq!(app.voice_owner, None);
    assert_eq!(devices.0.borrow().input_starts, 0);
}

#[test]
fn voice_ends_with_its_channel_and_later_moves_need_a_new_join() {
    let devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, _rb) = login(&mut app, "b.invalid:7331", 5);
    app.view_session(a);
    message(
        &mut app,
        a,
        ServerMessage::ChannelCreated {
            channel: channel(4, ChannelKind::Voice, "other".into()),
        },
    );
    join_voice(&mut app, VOICE);
    moved(&mut app, a, 1, 4); // moved by a moderator while in voice: kept
    assert_eq!(devices.0.borrow().inputs, 1);
    assert_eq!(app.voice_channel(), Some(4));
    app.view_session(b); // A in the background
    message(&mut app, a, ServerMessage::ChannelDeleted { channel_id: 4 });
    assert_eq!(app.voice_owner, None);
    assert_eq!(devices.0.borrow().inputs, 0);
    drain(&mut ra);
    moved(&mut app, a, 1, VOICE);
    assert_eq!(
        app.voice_owner, None,
        "a deleted channel leaves no voice consent behind"
    );
    assert_eq!(devices.0.borrow().inputs, 0);
    assert_eq!(drain(&mut ra), vec!["JoinChannel(1)"]);
}

#[test]
fn deleting_the_requested_channel_or_leaving_ends_the_voice_join() {
    let devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    message(
        &mut app,
        a,
        ServerMessage::ChannelCreated {
            channel: channel(4, ChannelKind::Voice, "other".into()),
        },
    );
    app.join_channel(4);
    message(&mut app, a, ServerMessage::ChannelDeleted { channel_id: 4 });
    assert_eq!(app.voice_owner, None);
    moved(&mut app, a, 1, VOICE);
    assert_eq!(devices.0.borrow().input_starts, 0);
    // The server says we left: voice ends.
    moved(&mut app, a, 1, SPAWN);
    app.session_mut(a).unwrap().bounced_at = None;
    join_voice(&mut app, VOICE);
    assert_eq!(devices.0.borrow().inputs, 1);
    message(&mut app, a, ServerMessage::UserLeft { client_id: 1 });
    assert_eq!(app.voice_owner, None);
    assert_eq!(devices.0.borrow().inputs, 0);
}

#[test]
fn reconnects_bring_back_chat_only() {
    let devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    join_voice(&mut app, VOICE);
    assert_eq!(devices.0.borrow().inputs, 1);
    app.handle_event(
        a,
        NetEvent::Disconnected {
            reason: "connection lost".into(),
        },
    );
    assert!(app.srv.reconnect.as_ref().is_some_and(|rc| rc.was_voice));
    assert_eq!(app.voice_owner, None);
    assert_eq!(devices.0.borrow().inputs, 0);
    // The automatic attempt succeeds (e.g. after sleep): no voice request, no capture.
    let (conn, mut rx) = Connection::test_pair();
    app.srv.cmd_tx = Some(conn);
    app.handle_event(a, welcome(1));
    assert!(
        drain(&mut rx).is_empty(),
        "nothing is sent on the user's behalf"
    );
    assert!(
        app.status.contains("join a voice channel again"),
        "{}",
        app.status
    );
    moved(&mut app, a, 1, VOICE); // a server that still has us in voice
    assert_eq!(
        devices.0.borrow().input_starts,
        1,
        "only the explicit join ever started capture"
    );
    assert_eq!(app.voice_owner, None);
}

#[test]
fn a_busy_connection_is_reported_not_claimed_sent() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    let tx = app.srv.tx().unwrap();
    while tx.send(Command::JoinChannel(SPAWN)).is_ok() {}
    app.poll_sessions();
    assert!(
        app.status_is_error && app.status.contains("busy"),
        "{}",
        app.status
    );
    app.set_status("");
    app.join_channel(VOICE);
    assert!(app.status.contains("busy"));
    assert_eq!(
        app.voice_owner, None,
        "a voice join that was not sent grants nothing"
    );
    app.chat_input = "keep me".into();
    app.send_chat(TEXT, &tx);
    assert_eq!(app.chat_input, "keep me", "the composer keeps unsent text");
}

// ----- Resource limits -----------------------------------------------------

#[test]
fn oversized_welcome_fails_closed_for_that_server_only() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    let cases: Vec<fn(&mut NetEvent)> = vec![
        |ev| {
            if let NetEvent::Connected { users, .. } = ev {
                let u = users[0].clone();
                users.extend((0..limits::MAX_USERS as u64).map(|i| UserInfo {
                    id: 1000 + i,
                    ..u.clone()
                }));
            }
        },
        |ev| {
            if let NetEvent::Connected { channels, .. } = ev {
                channels[0].name = "x".repeat(limits::MAX_LABEL_CHARS + 1);
            }
        },
        |ev| {
            if let NetEvent::Connected { chat_history, .. } = ev {
                chat_history.push(rusttalk_shared::protocol::ChatRecord {
                    id: 1,
                    channel_id: TEXT,
                    from_name: "x".into(),
                    text: "y".repeat(rusttalk_shared::protocol::MAX_CHAT_LEN + 1),
                    timestamp: 0,
                    image: None,
                    reply_to: None,
                    edited: false,
                });
            }
        },
        |ev| {
            if let NetEvent::Connected { settings, .. } = ev {
                settings.icon = Some(vec![0; rusttalk_shared::protocol::MAX_ICON_BYTES + 1]);
            }
        },
    ];
    for (i, change) in cases.iter().enumerate() {
        let id = app.ensure_session(&format!("bad{i}.invalid:7331")).unwrap();
        let (conn, rx) = Connection::test_pair();
        app.session_mut(id).unwrap().cmd_tx = Some(conn);
        let mut ev = welcome(1);
        change(&mut ev);
        app.handle_event(id, ev);
        let s = app.session(id).unwrap();
        assert!(
            !s.connected && s.cmd_tx.is_none() && s.reconnect.is_none(),
            "case {i}"
        );
        assert!(
            s.users.is_empty() && s.channels.is_empty() && s.chat.is_empty(),
            "case {i}: nothing kept"
        );
        assert!(s.join_error.is_some() && rx.cancelled(), "case {i}");
        assert!(
            app.status_is_error && app.status.contains("invalid data"),
            "case {i}: {}",
            app.status
        );
    }
    assert!(
        app.session(a).unwrap().connected,
        "other servers stay connected"
    );
}

#[test]
fn oversized_or_excess_incremental_state_fails_closed() {
    let _devices = Devices::new();
    let long_chat = "y".repeat(rusttalk_shared::protocol::MAX_CHAT_LEN + 1);
    let cases: Vec<ServerMessage> = vec![
        ServerMessage::Chat {
            id: 1,
            channel_id: TEXT,
            from_id: 2,
            from_name: "x".into(),
            text: long_chat.clone(),
            timestamp: 0,
            image: None,
            reply_to: None,
        },
        ServerMessage::Chat {
            id: 1,
            channel_id: TEXT,
            from_id: 2,
            from_name: "x".into(),
            text: String::new(),
            timestamp: 0,
            image: Some(vec![0; MAX_MEDIA_BYTES + 1]),
            reply_to: None,
        },
        ServerMessage::ChatEdited {
            channel_id: TEXT,
            message_id: 1,
            text: long_chat,
        },
        ServerMessage::ChannelRenamed {
            channel_id: TEXT,
            name: "n".repeat(limits::MAX_LABEL_CHARS + 1),
        },
        ServerMessage::Notice {
            message: "n".repeat(limits::MAX_TEXT_LEN + 1),
        },
        ServerMessage::StreamsChanged {
            client_id: 1,
            streams: vec![VideoKind::Camera; 3],
        },
    ];
    for (i, msg) in cases.into_iter().enumerate() {
        let mut app = app();
        let (a, ra) = login(&mut app, "a.invalid:7331", 1);
        let (b, _rb) = login(&mut app, "b.invalid:7331", 1);
        message(&mut app, a, msg);
        assert!(
            !app.session(a).unwrap().connected && ra.cancelled(),
            "case {i}"
        );
        assert!(app.session(b).unwrap().connected, "case {i}");
    }
    // Counts: users and channels beyond the caps.
    let mut app = app();
    let (a, ra) = login(&mut app, "a.invalid:7331", 1);
    let user = app.session(a).unwrap().users[&1].clone();
    for i in 1..limits::MAX_USERS as u64 {
        message(
            &mut app,
            a,
            ServerMessage::UserJoined {
                user: UserInfo {
                    id: 100 + i,
                    channel_id: SPAWN,
                    ..user.clone()
                },
            },
        );
    }
    assert!(
        app.session(a).unwrap().connected
            && app.session(a).unwrap().users.len() == limits::MAX_USERS
    );
    message(
        &mut app,
        a,
        ServerMessage::UserJoined {
            user: UserInfo {
                id: 99_999,
                ..user.clone()
            },
        },
    );
    assert!(!app.session(a).unwrap().connected && ra.cancelled());
    let (c, rc) = login(&mut app, "c.invalid:7331", 1);
    for i in 3..limits::MAX_CHANNELS as u64 {
        message(
            &mut app,
            c,
            ServerMessage::ChannelCreated {
                channel: channel(100 + i, ChannelKind::Text, "t".into()),
            },
        );
    }
    assert!(app.session(c).unwrap().connected);
    message(
        &mut app,
        c,
        ServerMessage::ChannelCreated {
            channel: channel(99_999, ChannelKind::Text, "t".into()),
        },
    );
    assert!(!app.session(c).unwrap().connected && rc.cancelled());
}

#[test]
fn member_lists_stay_bounded_by_known_users() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    for i in 0..1000 {
        moved(&mut app, a, 10_000 + i, SPAWN); // unknown users
    }
    let user = app.session(a).unwrap().users[&1].clone();
    for ch in [SPAWN, VOICE, TEXT, SPAWN] {
        message(
            &mut app,
            a,
            ServerMessage::UserJoined {
                user: UserInfo {
                    id: 7,
                    channel_id: ch,
                    ..user.clone()
                },
            },
        );
    }
    let s = app.session(a).unwrap();
    assert_eq!(
        s.member_count(),
        1,
        "one entry for user 7, none for unknown ids"
    );
}

#[test]
fn chat_text_budget_counts_edits_and_recovers_on_removal() {
    let mut s = ServerSession::new(1, "a.invalid");
    for id in [TEXT, VOICE] {
        s.channels
            .insert(id, channel(id, ChannelKind::Text, "t".into()));
    }
    let held = |s: &ServerSession| {
        s.chat
            .values()
            .flatten()
            .map(session::line_bytes)
            .sum::<usize>()
    };
    let text = "🙂".repeat(rusttalk_shared::protocol::MAX_CHAT_LEN);
    for n in 0..4000u64 {
        let ch = if n % 2 == 0 { TEXT } else { VOICE };
        let mut line = ChatLine::system("w".repeat(2000), n);
        line.id = n + 1;
        s.push_line(ch, line);
    }
    assert_eq!(s.text_bytes, held(&s));
    // Edits grow the text: they count, and the oldest lines make room.
    let ids: Vec<u64> = s.chat[&TEXT]
        .iter()
        .rev()
        .take(2000)
        .map(|l| l.id)
        .collect();
    for id in ids {
        s.edit_line(TEXT, id, text.clone());
        assert!(s.text_bytes <= session::MAX_TEXT_BYTES);
    }
    assert_eq!(s.text_bytes, held(&s));
    assert!(
        s.chat[&TEXT]
            .last()
            .is_some_and(|l| l.edited && l.text == text),
        "newest kept"
    );
    let first = s.chat[&TEXT][0].id;
    s.remove_line(TEXT, first);
    assert_eq!(s.text_bytes, held(&s));
    s.remove_channel_chat(TEXT);
    assert_eq!(s.text_bytes, held(&s));
    s.clear_chat();
    assert_eq!((s.text_bytes, s.image_bytes), (0, 0));
}

#[test]
fn a_flooding_server_stays_within_the_retained_budget() {
    let _devices = Devices::new();
    assert!(limits::worst_case_bytes() <= limits::MAX_RETAINED_BYTES);
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    let name = "é".repeat(limits::MAX_LABEL_CHARS);
    let user = app.session(a).unwrap().users[&1].clone();
    for i in 1..limits::MAX_USERS as u64 {
        message(
            &mut app,
            a,
            ServerMessage::UserJoined {
                user: UserInfo {
                    id: 100 + i,
                    name: name.clone(),
                    ..user.clone()
                },
            },
        );
    }
    for i in 3..limits::MAX_CHANNELS as u64 {
        message(
            &mut app,
            a,
            ServerMessage::ChannelCreated {
                channel: channel(100 + i, ChannelKind::Text, name.clone()),
            },
        );
    }
    let text = "é".repeat(rusttalk_shared::protocol::MAX_CHAT_LEN);
    for i in 0..3000u64 {
        message(
            &mut app,
            a,
            ServerMessage::Chat {
                id: i + 1,
                channel_id: 103 + i % 50,
                from_id: 101,
                from_name: name.clone(),
                text: text.clone(),
                timestamp: i,
                image: (i % 100 == 0).then(|| vec![0; MAX_MEDIA_BYTES]),
                reply_to: None,
            },
        );
    }
    let s = app.session(a).unwrap();
    assert!(s.connected);
    let budget = limits::MAX_RETAINED_BYTES - crate::net::EVENT_QUEUE_BYTES;
    assert!(
        s.retained_bytes() <= budget,
        "{} > {budget}",
        s.retained_bytes()
    );
    assert!(s.text_bytes <= session::MAX_TEXT_BYTES && s.image_bytes <= session::MAX_IMAGE_BYTES);
}

// ----- Async results bound to their server -----------------------------------

#[test]
fn a_late_gif_download_never_reaches_another_server() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, mut rb) = login(&mut app, "b.invalid:7331", 1);
    app.view_session(a);
    app.select_channel(TEXT);
    let on_a = app.new_gif_target().unwrap();
    app.gif_target = Some(on_a);
    app.giphy.open = true;
    let old_epoch = app.giphy.epoch;
    app.view_session(b);
    app.select_channel(TEXT);
    let on_b = app.new_gif_target().unwrap();
    app.gif_target = Some(on_b);
    app.giphy.sending = true;
    app.giphy.open = true;
    // A's late results: neither sent anywhere nor clearing B's pending one.
    app.on_giphy_msg(GiphyMsg::Full {
        request: on_a.request,
        bytes: vec![1],
    });
    app.on_giphy_msg(GiphyMsg::FullFailed {
        request: on_a.request,
        error: "late".into(),
    });
    app.on_giphy_msg(GiphyMsg::Preview {
        epoch: old_epoch,
        url: "u".into(),
        bytes: vec![1],
    });
    assert!(drain(&mut ra).is_empty() && drain(&mut rb).is_empty());
    assert_eq!(app.gif_target, Some(on_b));
    assert!(app.giphy.sending && app.giphy.error.is_none() && app.giphy.previews.is_empty());
    app.on_giphy_msg(GiphyMsg::Full {
        request: on_b.request,
        bytes: vec![2],
    });
    let to_b = drain(&mut rb);
    assert_eq!(to_b.len(), 1);
    assert!(to_b[0].starts_with("SendChat { channel_id: 3"), "{to_b:?}");
    assert!(drain(&mut ra).is_empty());
    // A target of an older connection of the same server is not sent either.
    let stale = GifTarget {
        generation: on_b.generation + 1,
        request: 99,
        ..on_b
    };
    app.gif_target = Some(stale);
    app.on_giphy_msg(GiphyMsg::Full {
        request: 99,
        bytes: vec![3],
    });
    assert!(drain(&mut rb).is_empty() && app.status_is_error);
}

#[test]
fn stale_decoder_results_are_ignored_across_voice_servers_and_restarts() {
    let _devices = Devices::new();
    let mut app = app();
    let ctx = egui::Context::default();
    let (a, mut ra) = login(&mut app, "a.invalid:7331", 1);
    let (b, mut rb) = login(&mut app, "b.invalid:7331", 1);
    app.view_session(a);
    join_voice(&mut app, VOICE);
    let key = (5, VideoKind::Screen);
    let view = |app: &mut RustTalkApp| {
        app.next_video_view += 1;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::mem::forget(rx); // the "decoder" never answers by itself
        app.video_frames.insert(
            key,
            media_ui::VideoView {
                id: app.next_video_view,
                tx,
                tex: None,
                last_request: None,
            },
        );
        app.next_video_view
    };
    let picture = || egui::ColorImage::new([1, 1], egui::Color32::RED);
    let old = view(&mut app);
    // The stream restarts on the same voice server: the old decoder's results are void.
    app.clear_video_of(5, None);
    let restarted = view(&mut app);
    app.video_tx
        .send(media_ui::DecodeMsg::Picture(key, old, picture()))
        .unwrap();
    app.video_tx
        .send(media_ui::DecodeMsg::NeedKeyframe(key, old))
        .unwrap();
    app.poll_video(&ctx);
    assert!(app.video_frames[&key].tex.is_none());
    drain(&mut ra);
    // Voice moves to B, which has its own user 5.
    app.view_session(b);
    join_voice(&mut app, VOICE);
    assert!(
        app.video_frames.is_empty(),
        "tiles belong to the old voice server"
    );
    let on_b = view(&mut app);
    for id in [old, restarted] {
        app.video_tx
            .send(media_ui::DecodeMsg::Picture(key, id, picture()))
            .unwrap();
        app.video_tx
            .send(media_ui::DecodeMsg::NeedKeyframe(key, id))
            .unwrap();
    }
    drain(&mut rb);
    drain(&mut ra); // the legitimate voice handoff has finished
    app.poll_video(&ctx);
    assert!(
        app.video_frames[&key].tex.is_none(),
        "A's picture never shows in B's tile"
    );
    assert!(
        drain(&mut rb).is_empty() && drain(&mut ra).is_empty(),
        "no keyframe request for A's stream"
    );
    app.video_tx
        .send(media_ui::DecodeMsg::Picture(key, on_b, picture()))
        .unwrap();
    app.video_tx
        .send(media_ui::DecodeMsg::NeedKeyframe(key, on_b))
        .unwrap();
    app.poll_video(&ctx);
    assert!(app.video_frames[&key].tex.is_some());
    assert_eq!(
        drain(&mut rb),
        vec!["Raw(RequestKeyframe { from_id: 5, kind: Screen })"]
    );
}

#[test]
fn a_late_screen_failure_cannot_stop_a_newer_share() {
    let _devices = Devices::new();
    let mut app = app();
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    join_voice(&mut app, VOICE);
    let mut old_sink = app.make_screen_error_sink();
    // A new share starts (start_screen_share bumps the epoch first).
    app.video_epoch[1].fetch_add(1, Ordering::Relaxed);
    let mut new_sink = app.make_screen_error_sink();
    old_sink("old capture ended".into());
    assert!(!app.take_screen_failure());
    // Voice ends: the current share's sink is stale too.
    let mut current = app.make_screen_error_sink();
    app.revoke_voice(a);
    current("after voice ended".into());
    assert!(!app.take_screen_failure());
    // Only a failure of the running share counts.
    let mut running = app.make_screen_error_sink();
    running("broken".into());
    assert!(app.take_screen_failure());
    new_sink("ignored".into());
    assert!(!app.take_screen_failure());
}

#[test]
fn mixer_volumes_are_per_stream_saved_by_name_and_follow_the_voice_server() {
    use crate::audio::StreamKind;
    let _devices = Devices::new();
    let mut app = app();
    app.config.user_volumes.insert("anna".into(), 1.5);
    app.config.share_volumes.insert("anna".into(), 0.25);
    let (a, _ra) = login(&mut app, "a.invalid:7331", 1);
    app.view_session(a);
    join_voice(&mut app, VOICE);
    let anna = UserInfo {
        id: 5,
        name: "anna".into(),
        channel_id: VOICE,
        muted: false,
        deafened: false,
        sharing: false,
        role: Default::default(),
        streams: vec![],
    };
    message(&mut app, a, ServerMessage::UserJoined { user: anna.clone() });
    let gain = |app: &RustTalkApp, kind: StreamKind| app.user_volumes.lock().unwrap().get(&kind.key(5)).copied();
    assert_eq!(gain(&app, StreamKind::Voice), Some(1.5), "saved voice volume reaches playback");
    assert_eq!(gain(&app, StreamKind::Shared), Some(0.25), "shared audio has its own volume");

    // One source of truth: whatever slider changes it, config and playback agree.
    app.set_user_volume(5, "anna", StreamKind::Shared, 1.8);
    assert_eq!(app.user_volume("anna", StreamKind::Shared), 1.8);
    assert_eq!(gain(&app, StreamKind::Shared), Some(1.8));
    assert_eq!(gain(&app, StreamKind::Voice), Some(1.5), "voice untouched");
    app.set_user_volume(5, "anna", StreamKind::Voice, 1.0);
    assert!(!app.config.user_volumes.contains_key("anna"), "100 % is not stored");
    assert_eq!(gain(&app, StreamKind::Voice), None);

    let back: ClientConfig = toml::from_str(&toml::to_string_pretty(&app.config).unwrap()).unwrap();
    assert_eq!(back.share_volumes.get("anna"), Some(&1.8), "survives a restart");

    // Leaving forgets her client id, the saved volume stays.
    app.control.muted_streams.lock().unwrap().insert(StreamKind::Shared.key(5));
    message(&mut app, a, ServerMessage::UserLeft { client_id: 5 });
    assert_eq!(gain(&app, StreamKind::Shared), None);
    assert!(app.control.muted_streams.lock().unwrap().is_empty());
    assert_eq!(app.config.share_volumes.get("anna"), Some(&1.8));
}
