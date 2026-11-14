//! End-to-End-Tests: echter Server-Prozess + synthetische Clients.
//!
//! Deckt ab: Handshake mit Spawn-Log, Channel-Typen-Regeln (Join/Chat),
//! Kategorie-Gruppierung, Mute-Broadcast, Passwort-Abweisung, UDP-Audio-Relay.

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use rusttalk_shared::framing::{read_frame, write_frame};
use rusttalk_shared::protocol::*;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn write_config(port: u16, extra: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rusttalk-it-{port}.toml"));
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "bind_address = \"127.0.0.1:{port}\"").unwrap();
    writeln!(f, "server_name = \"Integrationstest\"").unwrap();
    if !extra.contains("check_updates") {
        writeln!(f, "check_updates = false").unwrap(); // tests never ask GitHub
    }
    write!(f, "{extra}").unwrap();
    path
}

fn spawn_server(port: u16, extra_config: &str) -> ServerGuard {
    spawn_server_with_env(port, extra_config, &[])
}

fn spawn_server_with_env(port: u16, extra_config: &str, env: &[(&str, String)]) -> ServerGuard {
    let config = write_config(port, extra_config);
    let child = Command::new(env!("CARGO_BIN_EXE_rusttalk-server"))
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Server-Prozess starten");
    ServerGuard(child)
}

fn wait_for_port(port: u16) {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("Server hat Port {port} nicht geoeffnet");
}

type Tls = tokio_rustls::client::TlsStream<TcpStream>;
type Conn = (ReadHalf<Tls>, WriteHalf<Tls>);

/// TLS connection like the client makes it (pinning off: any certificate).
async fn tls_connect(port: u16) -> Tls {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.expect("TCP connect");
    let verifier = rusttalk_shared::secure::PinnedVerifier::new(None);
    let connector = tokio_rustls::TlsConnector::from(rusttalk_shared::secure::client_config(verifier));
    let name = rustls::pki_types::ServerName::try_from(rusttalk_shared::secure::TLS_SERVER_NAME).unwrap();
    connector.connect(name, tcp).await.expect("TLS handshake")
}

struct Welcome {
    client_id: u64,
    channels: Vec<ChannelInfo>,
    users: Vec<UserInfo>,
    spawn_log: Vec<LogEntry>,
    chat_history: Vec<ChatRecord>,
    settings: ServerSettings,
    gif_search: bool,
    open_permissions: bool,
    udp_token: u64,
    udp_key: Vec<u8>,
}

/// Stable test identity per name (64 hex chars).
fn identity_for(name: &str) -> String {
    let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{hex:0<64}").chars().take(64).collect()
}

async fn connect(port: u16, name: &str, password: Option<String>) -> (Conn, Welcome) {
    connect_with(port, name, password, None).await.expect("login")
}

/// Login; `Err(message)` if the server refuses.
async fn connect_with(port: u16, name: &str, password: Option<String>, invite: Option<String>) -> Result<(Conn, Welcome), String> {
    let mut stream = tls_connect(port).await;
    write_frame(
        &mut stream,
        &ClientMessage::Hello { username: name.to_string(), protocol_version: PROTOCOL_VERSION, password, identity: identity_for(name), invite },
    )
    .await
    .unwrap();
    let (reader, writer) = tokio::io::split(stream);
    let mut conn = (reader, writer);
    let first: ServerMessage = read_frame(&mut conn.0).await.unwrap().expect("erste Nachricht");
    match first {
        ServerMessage::Welcome { client_id, channels, users, spawn_log, chat_history, settings, gif_search, open_permissions, udp_token, udp_key } => {
            Ok((conn, Welcome { client_id, channels, users, spawn_log, chat_history, settings, gif_search, open_permissions, udp_token, udp_key }))
        }
        ServerMessage::Error { message } => Err(message),
        other => panic!("kein Welcome erhalten: {other:?}"),
    }
}

async fn recv(conn: &mut Conn, timeout_ms: u64) -> Option<ServerMessage> {
    match tokio::time::timeout(Duration::from_millis(timeout_ms), read_frame(&mut conn.0)).await {
        Ok(Ok(msg)) => msg,
        _ => None,
    }
}

async fn send(conn: &mut Conn, msg: &ClientMessage) {
    write_frame(&mut conn.1, msg).await.unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handshake_spawn_und_join_log() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);

    // Erster Client: Welcome mit genau einem Spawn-Channel, User im Spawn.
    let (mut a, wa) = connect(port, "alice", None).await;
    assert_eq!(wa.client_id, 1);
    assert_eq!(wa.channels.len(), 1, "nur der Spawn-Channel");
    assert_eq!(wa.channels[0].kind, ChannelKind::Spawn);
    assert_eq!(wa.channels[0].name, "Lobby");
    assert_eq!(wa.users.len(), 1);
    assert_eq!(wa.users[0].channel_id, wa.channels[0].id, "landet im Spawn");

    // Eigener UserJoined + eigener Spawn-Log-Eintrag kommen als Events nach.
    let ev1 = recv(&mut a, 2000).await.expect("UserJoined");
    assert!(matches!(ev1, ServerMessage::UserJoined { .. }), "bekam: {ev1:?}");
    let ev2 = recv(&mut a, 2000).await.expect("SpawnLogEntry");
    match ev2 {
        ServerMessage::SpawnLogEntry { entry } => assert!(entry.text.contains("alice")),
        other => panic!("SpawnLogEntry erwartet, bekam: {other:?}"),
    }

    // Zweiter Client: Welcome enthaelt das bisherige Join-Log (alice).
    let (mut b, wb) = connect(port, "bob", None).await;
    assert_eq!(wb.spawn_log.len(), 1, "alices Join im Log");
    assert!(wb.spawn_log[0].text.contains("alice"));
    assert_eq!(wb.users.len(), 2);

    // alice bekommt bobs Join + Log-Eintrag live mit.
    let ev = recv(&mut a, 2000).await.unwrap();
    match ev {
        ServerMessage::UserJoined { user } => assert_eq!(user.name, "bob"),
        other => panic!("UserJoined(bob) erwartet, bekam: {other:?}"),
    }
    let ev = recv(&mut a, 2000).await.unwrap();
    assert!(matches!(ev, ServerMessage::SpawnLogEntry { .. }));
    recv(&mut b, 2000).await; // bobs eigener UserJoined
    recv(&mut b, 2000).await; // bobs eigener Log-Eintrag

    // bob trennt sich -> alice bekommt UserLeft.
    send(&mut b, &ClientMessage::Disconnect).await;
    match recv(&mut a, 2000).await {
        Some(ServerMessage::UserLeft { client_id }) => assert_eq!(client_id, wb.client_id),
        other => panic!("UserLeft erwartet, bekam: {other:?}"),
    }
}

#[tokio::test]
async fn channel_typen_regeln() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);

    let (mut a, _wa) = connect(port, "alice", None).await;
    let (mut b, _wb) = connect(port, "bob", None).await;
    for _ in 0..2 {
        recv(&mut a, 2000).await;
        recv(&mut a, 2000).await;
    }
    recv(&mut b, 2000).await;
    recv(&mut b, 2000).await;

    // Kategorie erstellen.
    send(&mut a, &ClientMessage::CreateChannel {
        name: "Talk".into(),
        kind: ChannelKind::Category,
        parent: None,
    })
    .await;
    let cat_id = match recv(&mut a, 2000).await {
        Some(ServerMessage::ChannelCreated { channel }) => {
            assert_eq!(channel.kind, ChannelKind::Category);
            channel.id
        }
        other => panic!("ChannelCreated erwartet, bekam: {other:?}"),
    };
    assert!(matches!(
        recv(&mut b, 2000).await,
        Some(ServerMessage::ChannelCreated { .. })
    ));

    // Sprach-Channel in der Kategorie.
    send(&mut a, &ClientMessage::CreateChannel {
        name: "Funkturm".into(),
        kind: ChannelKind::Voice,
        parent: Some(cat_id),
    })
    .await;
    let voice_id = match recv(&mut a, 2000).await {
        Some(ServerMessage::ChannelCreated { channel }) => {
            assert_eq!(channel.kind, ChannelKind::Voice);
            assert_eq!(channel.parent, Some(cat_id));
            channel.id
        }
        other => panic!("ChannelCreated(Voice) erwartet, bekam: {other:?}"),
    };
    recv(&mut b, 2000).await;

    // Text-Channel in der Kategorie.
    send(&mut a, &ClientMessage::CreateChannel {
        name: "info".into(),
        kind: ChannelKind::Text,
        parent: Some(cat_id),
    })
    .await;
    let text_id = match recv(&mut a, 2000).await {
        Some(ServerMessage::ChannelCreated { channel }) => channel.id,
        other => panic!("ChannelCreated(Text) erwartet, bekam: {other:?}"),
    };
    recv(&mut b, 2000).await;

    // Zweiter Spawn verboten.
    send(&mut a, &ClientMessage::CreateChannel {
        name: "Spawn2".into(),
        kind: ChannelKind::Spawn,
        parent: None,
    })
    .await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));

    // Join-Regeln: Voice ok, Text/Kategorie verboten.
    send(&mut a, &ClientMessage::JoinChannel { channel_id: voice_id }).await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::UserMoved { channel_id, .. }) => assert_eq!(channel_id, voice_id),
            other => panic!("UserMoved erwartet, bekam: {other:?}"),
        }
    }
    send(&mut a, &ClientMessage::JoinChannel { channel_id: text_id }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));
    send(&mut a, &ClientMessage::JoinChannel { channel_id: cat_id }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));

    // Chat-Regeln: in Text UND Voice erlaubt (Voice = Media-Feed), Spawn nicht.
    send(&mut a, &ClientMessage::Chat { channel_id: text_id, text: "hallo info".into(), image: None, reply_to: None }).await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::Chat { channel_id, text, .. }) => {
                assert_eq!(channel_id, text_id);
                assert_eq!(text, "hallo info");
            }
            other => panic!("Chat erwartet, bekam: {other:?}"),
        }
    }
    // Bild im Voice-Channel:
    send(&mut a, &ClientMessage::Chat {
        channel_id: voice_id,
        text: String::new(),
        image: Some(vec![0x89, 0x50, 0x4E, 0x47]),
        reply_to: None,
    })
    .await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::Chat { channel_id, image, .. }) => {
                assert_eq!(channel_id, voice_id);
                assert_eq!(image.as_deref(), Some(&[0x89, 0x50, 0x4E, 0x47][..]));
            }
            other => panic!("Bild-Chat erwartet, bekam: {other:?}"),
        }
    }
    // Spawn bleibt verboten:
    send(&mut a, &ClientMessage::Chat { channel_id: 1, text: "x".into(), image: None, reply_to: None }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));

    // Kategorie loeschen: Kinder werden Top-Level, Spawn unloeschbar.
    send(&mut b, &ClientMessage::DeleteChannel { channel_id: 1 }).await;
    assert!(matches!(recv(&mut b, 2000).await, Some(ServerMessage::Error { .. })));
    send(&mut b, &ClientMessage::DeleteChannel { channel_id: cat_id }).await;
    assert!(matches!(recv(&mut b, 2000).await, Some(ServerMessage::ChannelDeleted { .. })));
}

#[tokio::test]
async fn mute_state_wird_verteilt() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);

    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _wb) = connect(port, "bob", None).await;
    for _ in 0..2 {
        recv(&mut a, 2000).await;
        recv(&mut a, 2000).await;
    }
    recv(&mut b, 2000).await;
    recv(&mut b, 2000).await;

    send(&mut a, &ClientMessage::SetMuteState { muted: true, deafened: false }).await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::MuteStateChanged { client_id, muted, deafened }) => {
                assert_eq!(client_id, wa.client_id);
                assert!(muted);
                assert!(!deafened);
            }
            other => panic!("MuteStateChanged erwartet, bekam: {other:?}"),
        }
    }
}

/// Poke: kommt nur beim Ziel an (mit Absender + Nachricht), Spam wird abgelehnt.
#[tokio::test]
async fn poke_erreicht_nur_das_ziel() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);

    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, wb) = connect(port, "bob", None).await;
    let (mut c, _wc) = connect(port, "carol", None).await;
    for conn in [&mut a, &mut b, &mut c] {
        while recv(conn, 300).await.is_some() {}
    }

    send(&mut a, &ClientMessage::Poke { target: wb.client_id, message: "Gaming?".into() }).await;
    match recv(&mut b, 2000).await {
        Some(ServerMessage::Poked { from_id, from_name, message }) => {
            assert_eq!(from_id, wa.client_id);
            assert_eq!(from_name, "alice");
            assert_eq!(message, "Gaming?");
        }
        other => panic!("Poked erwartet, bekam: {other:?}"),
    }
    assert!(recv(&mut c, 400).await.is_none(), "Unbeteiligte bekommen nichts");
    assert!(recv(&mut a, 400).await.is_none(), "Absender bekommt kein Echo");

    // Sofort nochmal: Cooldown -> Fehler nur an den Absender.
    send(&mut a, &ClientMessage::Poke { target: wb.client_id, message: String::new() }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));
    assert!(recv(&mut b, 400).await.is_none());
}

/// Servereinstellungen: Aenderung wird an alle verteilt, ungueltige abgelehnt.
#[tokio::test]
async fn server_settings_werden_verteilt() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);

    let (mut a, _) = connect(port, "alice", None).await;
    let (mut b, _) = connect(port, "bob", None).await;
    // Join-Events wegraeumen.
    while recv(&mut a, 300).await.is_some() {}
    while recv(&mut b, 300).await.is_some() {}

    let new = ServerSettings {
        name: "LAN Party".into(),
        welcome_message: "Hallo!".into(),
        afk_timeout_mins: 5,
        ..ServerSettings::default()
    };
    send(&mut a, &ClientMessage::UpdateServerSettings { settings: new.clone() }).await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::ServerSettingsChanged { settings }) => assert_eq!(settings, new),
            other => panic!("ServerSettingsChanged erwartet, bekam: {other:?}"),
        }
    }

    // Ungueltig (Timeout 0) -> Fehler nur an den Absender, keine Aenderung.
    let bad = ServerSettings { afk_timeout_mins: 0, ..new.clone() };
    send(&mut a, &ClientMessage::UpdateServerSettings { settings: bad }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));
    assert!(recv(&mut b, 400).await.is_none(), "b bekommt nichts");

    // Neuer Client sieht die Einstellungen im Welcome.
    let (_c, wc) = connect(port, "carol", None).await;
    assert_eq!(wc.settings, new);
}

/// Collects messages until the connection is quiet for `quiet_ms`.
async fn drain(conn: &mut Conn, quiet_ms: u64) -> Vec<ServerMessage> {
    let mut out = Vec::new();
    while let Some(m) = recv(conn, quiet_ms).await {
        out.push(m);
    }
    out
}

fn chat_texts(msgs: &[ServerMessage]) -> Vec<(u64, String, String)> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMessage::Chat { channel_id, from_name, text, .. } => Some((*channel_id, from_name.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn join_leave_announcements_and_motd() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);

    let (mut a, _) = connect(port, "alice", None).await;
    drain(&mut a, 300).await;
    send(&mut a, &ClientMessage::CreateChannel { name: "news".into(), kind: ChannelKind::Text, parent: None }).await;
    let news = match recv(&mut a, 2000).await {
        Some(ServerMessage::ChannelCreated { channel }) => channel.id,
        other => panic!("ChannelCreated erwartet, bekam: {other:?}"),
    };
    let settings = ServerSettings {
        announce_channel: Some(news),
        join_message: "hi {user}".into(),
        leave_message: "bye {user}".into(),
        motd: "Game night".into(),
        rules: "Be nice".into(),
        ..ServerSettings::default()
    };
    send(&mut a, &ClientMessage::UpdateServerSettings { settings: settings.clone() }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::ServerSettingsChanged { .. })));

    // bob: rules in the Welcome, MOTD as notice, join announcement for everyone.
    let (mut b, wb) = connect(port, "bob", None).await;
    assert_eq!(wb.settings.rules, "Be nice");
    let bob_msgs = drain(&mut b, 500).await;
    assert!(
        bob_msgs.iter().any(|m| matches!(m, ServerMessage::Notice { message } if message == "Game night")),
        "MOTD fehlt: {bob_msgs:?}"
    );
    let expected_join = (news, "RustTalk".to_string(), "hi bob".to_string());
    assert!(chat_texts(&bob_msgs).contains(&expected_join), "bob: {bob_msgs:?}");
    let alice_msgs = drain(&mut a, 500).await;
    assert!(chat_texts(&alice_msgs).contains(&expected_join), "alice: {alice_msgs:?}");
    assert!(
        !alice_msgs.iter().any(|m| matches!(m, ServerMessage::Notice { .. })),
        "MOTD nur an den Neuen"
    );

    // bob leaves -> leave announcement.
    drop(b);
    let alice_msgs = drain(&mut a, 800).await;
    assert!(
        chat_texts(&alice_msgs).contains(&(news, "RustTalk".to_string(), "bye bob".to_string())),
        "leave fehlt: {alice_msgs:?}"
    );

    // Announcement channel deleted -> switched off, no more messages.
    send(&mut a, &ClientMessage::DeleteChannel { channel_id: news }).await;
    drain(&mut a, 400).await;
    let (_c, wc) = connect(port, "carol", None).await;
    assert_eq!(wc.settings.announce_channel, None);
    let alice_msgs = drain(&mut a, 500).await;
    assert!(chat_texts(&alice_msgs).is_empty(), "keine Ansage mehr: {alice_msgs:?}");
}

#[tokio::test]
async fn chat_reply_edit_delete() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);
    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _) = connect(port, "bob", None).await;
    drain(&mut a, 300).await;
    drain(&mut b, 300).await;
    let text_id = wa.channels.iter().find(|c| c.kind == ChannelKind::Text).map(|c| c.id);
    let text_id = match text_id {
        Some(id) => id,
        None => {
            send(&mut a, &ClientMessage::CreateChannel { name: "chat".into(), kind: ChannelKind::Text, parent: None }).await;
            let id = match recv(&mut a, 2000).await {
                Some(ServerMessage::ChannelCreated { channel }) => channel.id,
                other => panic!("ChannelCreated erwartet, bekam: {other:?}"),
            };
            drain(&mut b, 300).await;
            id
        }
    };

    send(&mut a, &ClientMessage::Chat { channel_id: text_id, text: "frage?".into(), image: None, reply_to: None }).await;
    let first = match recv(&mut b, 2000).await {
        Some(ServerMessage::Chat { id, reply_to: None, .. }) => id,
        other => panic!("Chat erwartet, bekam: {other:?}"),
    };
    drain(&mut a, 300).await;

    // bob answers; an invalid reply id is dropped by the server.
    send(&mut b, &ClientMessage::Chat { channel_id: text_id, text: "antwort".into(), image: None, reply_to: Some(first) }).await;
    let answer = match recv(&mut a, 2000).await {
        Some(ServerMessage::Chat { id, reply_to, .. }) => {
            assert_eq!(reply_to, Some(first));
            id
        }
        other => panic!("Antwort erwartet, bekam: {other:?}"),
    };
    drain(&mut b, 300).await;
    send(&mut b, &ClientMessage::Chat { channel_id: text_id, text: "x".into(), image: None, reply_to: Some(999_999) }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Chat { reply_to: None, .. })));
    drain(&mut b, 300).await;

    // alice may not edit bob's message, but her own.
    send(&mut a, &ClientMessage::EditChat { message_id: answer, text: "gehackt".into() }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));
    assert!(recv(&mut b, 400).await.is_none(), "kein Edit verteilt");
    send(&mut a, &ClientMessage::EditChat { message_id: first, text: "frage (korrigiert)?".into() }).await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::ChatEdited { channel_id, message_id, text }) => {
                assert_eq!((channel_id, message_id, text.as_str()), (text_id, first, "frage (korrigiert)?"));
            }
            other => panic!("ChatEdited erwartet, bekam: {other:?}"),
        }
    }

    // bob deletes his answer; deleting it again fails.
    send(&mut b, &ClientMessage::DeleteChat { message_id: answer }).await;
    for conn in [&mut a, &mut b] {
        assert!(matches!(
            recv(conn, 2000).await,
            Some(ServerMessage::ChatDeleted { message_id, .. }) if message_id == answer
        ));
    }
    send(&mut b, &ClientMessage::DeleteChat { message_id: answer }).await;
    assert!(matches!(recv(&mut b, 2000).await, Some(ServerMessage::Error { .. })));
}

#[tokio::test]
async fn gif_search_needs_a_server_key() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);
    let (mut a, _) = connect(port, "alice", None).await;
    drain(&mut a, 300).await;

    // No key: answer with an error, nothing goes to GIPHY.
    send(&mut a, &ClientMessage::GifSearch { query: "cat".into(), offset: 0 }).await;
    match recv(&mut a, 2000).await {
        Some(ServerMessage::GifResults { query, results, error: Some(e), .. }) => {
            assert_eq!(query, "cat");
            assert!(results.is_empty());
            assert!(e.contains("not set up"), "{e}");
        }
        other => panic!("GifResults mit Fehler erwartet, bekam: {other:?}"),
    }
    // Obviously invalid key: rejected locally.
    send(&mut a, &ClientMessage::SetGifKey { key: Some("not a key!".into()) }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));
    // Removing the key is broadcast.
    send(&mut a, &ClientMessage::SetGifKey { key: None }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::GifSearchChanged { available: false })));
}

/// Minimal fake GIPHY API on a local port: accepts the key "goodkey", answers
/// with two GIFs and counts the API requests.
fn fake_giphy() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead, BufReader};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut line = String::new();
            while reader.read_line(&mut line).map(|n| n > 2).unwrap_or(false) {
                line.clear(); // skip headers
            }
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (status, body) = if request_line.contains("api_key=goodkey") {
                let gif = |id: &str| format!(
                    r#"{{"id":"{id}","title":"t {id}","images":{{"downsized":{{"url":"http://x/{id}.gif","width":"100","height":"50"}}}}}}"#
                );
                ("200 OK", format!(r#"{{"data":[{},{}]}}"#, gif("a"), gif("b")))
            } else {
                ("401 Unauthorized", String::new())
            };
            let mut s = stream;
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, hits)
}

#[tokio::test]
async fn gif_key_is_checked_and_search_is_cached() {
    let (giphy_port, hits) = fake_giphy();
    let port = free_port();
    let _server = spawn_server_with_env(port, "", &[("RUSTTALK_GIPHY_API", format!("http://127.0.0.1:{giphy_port}"))]);
    wait_for_port(port);
    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _) = connect(port, "bob", None).await;
    assert!(!wa.settings.name.is_empty());
    drain(&mut a, 300).await;
    drain(&mut b, 300).await;

    // Wrong key: rejected by "GIPHY", nothing changes.
    send(&mut a, &ClientMessage::SetGifKey { key: Some("badkey".into()) }).await;
    match recv(&mut a, 5000).await {
        Some(ServerMessage::Error { message }) => assert!(message.contains("rejected"), "{message}"),
        other => panic!("Error erwartet, bekam: {other:?}"),
    }
    assert!(recv(&mut b, 400).await.is_none());

    // Good key: everyone learns that GIFs work, the setter gets a notice.
    send(&mut a, &ClientMessage::SetGifKey { key: Some("goodkey".into()) }).await;
    let msgs = drain(&mut a, 1500).await;
    assert!(msgs.iter().any(|m| matches!(m, ServerMessage::GifSearchChanged { available: true })), "{msgs:?}");
    assert!(matches!(recv(&mut b, 2000).await, Some(ServerMessage::GifSearchChanged { available: true })));
    let after_check = hits.load(std::sync::atomic::Ordering::SeqCst);

    // Trending was fetched while checking the key -> answered from the cache.
    send(&mut b, &ClientMessage::GifSearch { query: String::new(), offset: 0 }).await;
    match recv(&mut b, 3000).await {
        Some(ServerMessage::GifResults { results, error: None, .. }) => {
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].url, "http://x/a.gif");
        }
        other => panic!("GifResults erwartet, bekam: {other:?}"),
    }
    // A search goes to GIPHY once, the repeat comes from the cache.
    for _ in 0..2 {
        send(&mut b, &ClientMessage::GifSearch { query: "Cat".into(), offset: 0 }).await;
        assert!(matches!(recv(&mut b, 3000).await, Some(ServerMessage::GifResults { error: None, .. })));
    }
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), after_check + 1, "one request for 'Cat', rest cached");

    // A new client sees GIFs as available.
    let (_c, wc) = connect(port, "carol", None).await;
    assert!(wc.gif_search);
}

/// Waits for a message matching `pred` (skips others).
async fn expect<F: Fn(&ServerMessage) -> bool>(conn: &mut Conn, what: &str, pred: F) -> ServerMessage {
    for _ in 0..30 {
        match recv(conn, 3000).await {
            Some(m) if pred(&m) => return m,
            Some(_) => continue,
            None => break,
        }
    }
    panic!("{what} nicht erhalten");
}

#[tokio::test]
async fn roles_kick_ban_and_audit() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);
    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _) = connect(port, "bob", None).await;
    let (mut c, wc) = connect(port, "carol", None).await;
    assert!(wa.open_permissions, "no admin yet: open");
    let (alice, bob, carol) = (wa.client_id, wa.client_id + 1, wc.client_id);
    for conn in [&mut a, &mut b, &mut c] {
        drain(conn, 300).await;
    }

    // alice becomes the first admin -> everyone learns it, the server is closed now.
    send(&mut a, &ClientMessage::ClaimAdmin { password: None }).await;
    for conn in [&mut a, &mut b] {
        expect(conn, "UserRoleChanged", |m| matches!(m, ServerMessage::UserRoleChanged { client_id, role: Role::Admin } if *client_id == alice)).await;
        expect(conn, "PermissionsChanged", |m| matches!(m, ServerMessage::PermissionsChanged { open: false })).await;
    }
    drain(&mut c, 300).await;
    // bob cannot claim (no admin_password) and may not manage channels anymore.
    send(&mut b, &ClientMessage::ClaimAdmin { password: None }).await;
    expect(&mut b, "Error", |m| matches!(m, ServerMessage::Error { .. })).await;
    send(&mut b, &ClientMessage::CreateChannel { name: "x".into(), kind: ChannelKind::Text, parent: None }).await;
    match expect(&mut b, "Error", |m| matches!(m, ServerMessage::Error { .. })).await {
        ServerMessage::Error { message } => assert!(message.contains("Admin"), "{message}"),
        _ => unreachable!(),
    }

    // bob -> moderator; he may kick carol but not alice.
    send(&mut a, &ClientMessage::SetRole { client_id: bob, role: Role::Moderator }).await;
    expect(&mut b, "bob Moderator", |m| matches!(m, ServerMessage::UserRoleChanged { client_id, role: Role::Moderator } if *client_id == bob)).await;
    send(&mut b, &ClientMessage::Kick { client_id: alice, reason: String::new() }).await;
    expect(&mut b, "Error (higher role)", |m| matches!(m, ServerMessage::Error { .. })).await;
    send(&mut b, &ClientMessage::Kick { client_id: carol, reason: "spam".into() }).await;
    match expect(&mut c, "Removed", |m| matches!(m, ServerMessage::Removed { .. })).await {
        ServerMessage::Removed { reason, banned } => {
            assert!(!banned);
            assert!(reason.contains("bob") && reason.contains("spam"), "{reason}");
        }
        _ => unreachable!(),
    }
    expect(&mut a, "UserLeft carol", |m| matches!(m, ServerMessage::UserLeft { client_id } if *client_id == carol)).await;
    assert!(recv(&mut c, 2000).await.is_none(), "carol's connection is closed");

    // alice bans bob for 60 min -> he cannot come back until unbanned.
    send(&mut a, &ClientMessage::Ban { client_id: bob, reason: "trolling".into(), minutes: Some(60) }).await;
    expect(&mut b, "Removed (ban)", |m| matches!(m, ServerMessage::Removed { banned: true, .. })).await;
    let err = connect_with(port, "bob", None, None).await.err().expect("banned");
    assert!(err.contains("banned") && err.contains("trolling"), "{err}");

    // Admin data: ban + audit log.
    drain(&mut a, 300).await;
    send(&mut a, &ClientMessage::RequestAdminData).await;
    let ban_id = match expect(&mut a, "AdminData", |m| matches!(m, ServerMessage::AdminData { .. })).await {
        ServerMessage::AdminData { roles, bans, audit, .. } => {
            assert!(roles.iter().any(|r| r.name == "bob" && r.role == Role::Moderator), "{roles:?}");
            assert_eq!(bans.len(), 1);
            assert_eq!(bans[0].name, "bob");
            let actions: Vec<String> = audit.iter().map(|e| format!("{}: {}", e.actor, e.action)).collect();
            for want in ["alice: became admin", "bob: kicked carol (spam)", "alice: banned bob for 60 min (trolling)"] {
                assert!(actions.iter().any(|a| a == want), "{want} fehlt in {actions:?}");
            }
            bans[0].id
        }
        _ => unreachable!(),
    };
    send(&mut a, &ClientMessage::Unban { ban_id }).await;
    expect(&mut a, "AdminData after unban", |m| matches!(m, ServerMessage::AdminData { bans, .. } if bans.is_empty())).await;
    // bob is back - and still moderator (the role hangs on his identity, not the session).
    let (_b2, wb) = connect_with(port, "bob", None, None).await.expect("unbanned");
    assert!(wb.users.iter().any(|u| u.name == "bob" && u.role == Role::Moderator));
    assert!(!wb.open_permissions);
}

#[tokio::test]
async fn invites_replace_the_password_and_admin_password_works() {
    let port = free_port();
    let _server = spawn_server(port, "password = \"pw\"
admin_password = \"boss\"
");
    wait_for_port(port);
    let (mut a, wa) = connect(port, "alice", Some("pw".into())).await;
    drain(&mut a, 300).await;
    send(&mut a, &ClientMessage::ClaimAdmin { password: None }).await; // open -> works
    expect(&mut a, "admin", |m| matches!(m, ServerMessage::UserRoleChanged { client_id, role: Role::Admin } if *client_id == wa.client_id)).await;

    send(&mut a, &ClientMessage::CreateInvite { max_uses: Some(1), hours: Some(24) }).await;
    let code = match expect(&mut a, "InviteCreated", |m| matches!(m, ServerMessage::InviteCreated { .. })).await {
        ServerMessage::InviteCreated { invite } => invite.code,
        _ => unreachable!(),
    };
    // Invite instead of password - once.
    let (_d, _) = connect_with(port, "dave", None, Some(code.to_lowercase())).await.expect("invite works");
    let err = connect_with(port, "erin", None, Some(code.clone())).await.err().expect("used up");
    assert!(err.contains("invite"), "{err}");
    assert!(connect_with(port, "erin", None, None).await.is_err(), "no password");

    // admin_password from server.toml makes erin admin even though alice already is.
    let (mut e, we) = connect(port, "erin", Some("pw".into())).await;
    drain(&mut e, 300).await;
    send(&mut e, &ClientMessage::ClaimAdmin { password: Some("wrong".into()) }).await;
    expect(&mut e, "Error", |m| matches!(m, ServerMessage::Error { .. })).await;
    // A reconnect with a new identity from the same IP must not reset the guessing limit.
    let (mut retry, _) = connect(port, "retry", Some("pw".into())).await;
    drain(&mut retry, 50).await;
    send(&mut retry, &ClientMessage::ClaimAdmin { password: Some("boss".into()) }).await;
    match expect(&mut retry, "throttled", |m| matches!(m, ServerMessage::Error { .. })).await {
        ServerMessage::Error { message } => assert!(message.contains("Too many admin attempts")),
        _ => unreachable!(),
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    send(&mut e, &ClientMessage::ClaimAdmin { password: Some("boss".into()) }).await;
    expect(&mut e, "erin admin", |m| matches!(m, ServerMessage::UserRoleChanged { client_id, role: Role::Admin } if *client_id == we.client_id)).await;
}

/// Fake GitHub API: release list includes v99.0.0 with a server binary.
fn fake_github() -> u16 {
    fake_github_with_checksum(true)
}

fn fake_github_with_checksum(checksum: bool) -> u16 {
    use std::io::{BufRead, BufReader};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                line,
                "GET /repos/rusttalkcrew/RustTalk/releases?per_page=100&page=1 HTTP/1.1\r\n"
            );
            line.clear();
            while reader.read_line(&mut line).map(|n| n > 2).unwrap_or(false) {
                line.clear();
            }
            let asset = match (std::env::consts::OS, std::env::consts::ARCH) {
                ("windows", "x86_64") => "rusttalk-server-windows.exe",
                ("windows", "aarch64") => "rusttalk-server-windows-arm64.exe",
                ("linux", "x86_64") => "rusttalk-server-linux-x86_64",
                ("linux", "aarch64") => "rusttalk-server-linux-aarch64",
                target => panic!("Unsupported update-test target: {target:?}"),
            };
            let body = format!(
                r#"[{{"tag_name":"v99.0.0+server.abcdef1","draft":false,"prerelease":false,"body":null,"html_url":"http://x/v99","assets":[{{"name":"{asset}","browser_download_url":"http://x/bin"}},{{"name":"{asset}.sha256","browser_download_url":"http://x/sha"}}]}}]"#
            );
            let mut releases: serde_json::Value = serde_json::from_str(&body).unwrap();
            if !checksum {
                releases[0]["assets"].as_array_mut().unwrap().pop();
            }
            let body = releases.to_string();
            let mut s = stream;
            let _ = write!(s, "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{body}", body.len());
        }
    });
    port
}

/// The daily update check tells admins (and only admins) about a new version.
#[tokio::test]
async fn update_notice_reaches_only_admins() {
    let gh = fake_github();
    let port = free_port();
    let _server = spawn_server_with_env(port, "check_updates = true
", &[("RUSTTALK_UPDATE_API", format!("http://127.0.0.1:{gh}"))]);
    wait_for_port(port);
    let (mut a, wa) = connect(port, "alice", None).await;
    send(&mut a, &ClientMessage::ClaimAdmin { password: None }).await;
    // Open server at first = alice may see it right away (at login or when the
    // check finishes), so the notice and the role change come in any order.
    let (mut admin, mut notice) = (false, false);
    while !(admin && notice) {
        match recv(&mut a, 5000).await {
            Some(ServerMessage::UserRoleChanged { client_id, role: Role::Admin }) if client_id == wa.client_id => admin = true,
            Some(ServerMessage::Notice { message }) if message.contains("99.0.0") && message.contains("rusttalk-server update") => notice = true,
            Some(_) => {}
            None => panic!("admin={admin} notice={notice}"),
        }
    }

    // A later admin gets it on login, a member never.
    let (mut b, _) = connect(port, "bob", None).await;
    let bob_msgs = drain(&mut b, 500).await;
    assert!(!bob_msgs.iter().any(|m| matches!(m, ServerMessage::Notice { message } if message.contains("update"))), "{bob_msgs:?}");
}

/// Without TLS nothing gets through: a plain Hello is not answered.
#[tokio::test]
async fn plaintext_clients_are_refused() {
    let port = free_port();
    let _server = spawn_server(port, "");
    wait_for_port(port);
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let hello = ClientMessage::Hello {
        username: "plain".into(),
        protocol_version: PROTOCOL_VERSION,
        password: None,
        identity: identity_for("plain"),
        invite: None,
    };
    let _ = write_frame(&mut stream, &hello).await;
    let answer = tokio::time::timeout(Duration::from_secs(5), read_frame::<_, ServerMessage>(&mut stream)).await;
    assert!(!matches!(answer, Ok(Ok(Some(ServerMessage::Welcome { .. })))), "no Welcome without TLS");
}

/// A viewer's keyframe request reaches the sharer - only within the channel.
#[tokio::test]
async fn keyframe_requests_reach_the_sharer() {
    let port = free_port();
    let _server = spawn_server(port, "[[channels]]
name = \"Funkturm\"
kind = \"voice\"
");
    wait_for_port(port);
    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _) = connect(port, "bob", None).await;
    let (mut c, _) = connect(port, "carol", None).await;
    for conn in [&mut a, &mut b] {
        send(conn, &ClientMessage::JoinChannel { channel_id: 2 }).await;
    }
    send(&mut a, &ClientMessage::SetSharing { sharing: true }).await;
    for conn in [&mut a, &mut b, &mut c] {
        drain(conn, 500).await;
    }
    // carol is in the lobby, not watching: her request is ignored.
    send(&mut c, &ClientMessage::RequestKeyframe { from_id: wa.client_id, kind: VideoKind::Screen }).await;
    assert!(recv(&mut a, 600).await.is_none(), "no request from outside the channel");
    // bob watches: alice is asked for a keyframe.
    send(&mut b, &ClientMessage::RequestKeyframe { from_id: wa.client_id, kind: VideoKind::Screen }).await;
    match recv(&mut a, 2000).await {
        Some(ServerMessage::KeyframeRequest { kind: VideoKind::Screen }) => {}
        other => panic!("KeyframeRequest erwartet, bekam: {other:?}"),
    }
}

#[tokio::test]
async fn passwort_schutz() {
    let port = free_port();
    let _server = spawn_server(port, "password = \"geheim\"\n");
    wait_for_port(port);

    // Falsches Passwort -> Error-Frame, dann Schluss.
    let mut stream = tls_connect(port).await;
    write_frame(
        &mut stream,
        &ClientMessage::Hello {
            username: "eve".into(),
            protocol_version: PROTOCOL_VERSION,
            password: Some("falsch".into()),
            identity: identity_for("eve"),
            invite: None,
        },
    )
    .await
    .unwrap();
    let (mut r, _w) = tokio::io::split(stream);
    match read_frame(&mut r).await.unwrap() {
        Some(ServerMessage::Error { message }) => {
            assert!(message.contains("password"), "unerwartete Fehlermeldung: {message}")
        }
        other => panic!("Error erwartet, bekam: {other:?}"),
    }

    // Richtiges Passwort -> Welcome.
    let (_conn, _welcome) = connect(port, "friend", Some("geheim".into())).await;
}

/// Audio-Relay: Clients im Spawn duerfen NICHT hoeren/sprechen;
/// nach Join in einen Voice-Channel fliesst Audio beidseitig.
#[test]
fn audio_nur_in_voice_channels() {
    let port = free_port();
    // Config: Spawn (auto, id 1) + Voice-Channel (id 2).
    let extra = "[[channels]]\nname = \"Funkturm\"\nkind = \"voice\"\n";
    let server = spawn_server(port, extra);
    wait_for_port(port);

    let test_client = env!("CARGO_BIN_EXE_test_client");
    let addr = format!("127.0.0.1:{port}");

    // Phase 1: beide im Spawn -> duerfen NICHTS empfangen.
    let spawn_args = |name: &str| -> Vec<String> {
        vec![
            "--addr".into(), addr.clone(),
            "--name".into(), name.to_string(),
            "--audio-secs".into(), "2".into(),
            "--duration-secs".into(), "3".into(),
        ]
    };
    let mut a1 = Command::new(test_client)
        .args(spawn_args("spawn-a"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn-a");
    std::thread::sleep(Duration::from_millis(300));
    let mut b1 = Command::new(test_client)
        .args(spawn_args("spawn-b"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn-b");
    let out_a1 = a1.wait_with_output().expect("wait spawn-a");
    let out_b1 = b1.wait_with_output().expect("wait spawn-b");
    let sa = String::from_utf8_lossy(&out_a1.stdout);
    let sb = String::from_utf8_lossy(&out_b1.stdout);
    assert_eq!(parse_audio_rx(&sa), 0, "Spawn darf nichts empfangen:\n{sa}");
    assert_eq!(parse_audio_rx(&sb), 0, "Spawn darf nichts empfangen:\n{sb}");

    // Phase 2: beide joinen Voice-Channel (id 2) -> Audio fliesst.
    let voice_args = |name: &str| -> Vec<String> {
        vec![
            "--addr".into(), addr.clone(),
            "--name".into(), name.to_string(),
            "--join-channel".into(), "2".into(),
            "--audio-secs".into(), "3".into(),
            "--duration-secs".into(), "5".into(),
        ]
    };
    let mut a = Command::new(test_client)
        .args(voice_args("voice-a"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("voice-a");
    std::thread::sleep(Duration::from_millis(300));
    let mut b = Command::new(test_client)
        .args(voice_args("voice-b"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("voice-b");
    let out_a = a.wait_with_output().expect("wait a");
    let out_b = b.wait_with_output().expect("wait b");
    drop(server);

    let sa = String::from_utf8_lossy(&out_a.stdout);
    let sb = String::from_utf8_lossy(&out_b.stdout);
    assert!(out_a.status.success(), "voice-a fehlgeschlagen:\n{sa}");
    assert!(out_b.status.success(), "voice-b fehlgeschlagen:\n{sb}");
    let rx_a = parse_audio_rx(&sa);
    let rx_b = parse_audio_rx(&sb);
    assert!(rx_a >= 10, "a hat zu wenig Audio empfangen ({rx_a}):\n{sa}");
    assert!(rx_b >= 10, "b hat zu wenig Audio empfangen ({rx_b}):\n{sb}");
}

/// Persistenz: Channel + Chat-Verlauf + Spawn-Log ueberleben einen Server-Neustart.
#[tokio::test]
async fn server_restart_keeps_state() {
    let port = free_port();
    let db = std::env::temp_dir().join(format!("rusttalk-it-persist-{port}.db"));
    let _ = std::fs::remove_file(&db);
    let extra = format!(
        "persist_state = true\ndb_path = '{}'\n[[channels]]\nname = \"allgemein\"\nkind = \"text\"\n",
        db.to_string_lossy()
    );

    // --- Lauf 1: alice erstellt Channel + schreibt ---
    let channel_id;
    {
        let _server = spawn_server(port, &extra);
        wait_for_port(port);
        let (mut a, _wa) = connect(port, "alice", None).await;
        recv(&mut a, 2000).await; // eigener UserJoined
        recv(&mut a, 2000).await; // eigener SpawnLogEntry
        send(&mut a, &ClientMessage::CreateChannel {
            name: "Memo".into(),
            kind: ChannelKind::Text,
            parent: None,
        })
        .await;
        channel_id = match recv(&mut a, 2000).await {
            Some(ServerMessage::ChannelCreated { channel }) => channel.id,
            other => panic!("ChannelCreated erwartet, bekam: {other:?}"),
        };
        send(&mut a, &ClientMessage::Chat { channel_id, text: "ueberlebt das?".into(), image: None, reply_to: None }).await;
        assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Chat { .. })));
        send(&mut a, &ClientMessage::Disconnect).await;
    } // ServerGuard drop -> Prozess beendet

    // Kurz warten, bis der Port frei ist.
    std::thread::sleep(Duration::from_millis(300));

    // --- Lauf 2: gleiche Config -> gleiche DB ---
    {
        let _server = spawn_server(port, &extra);
        wait_for_port(port);
        let (_b, wb) = connect(port, "bob", None).await;
        assert!(
            wb.channels.iter().any(|c| c.id == channel_id && c.name == "Memo"),
            "Channel 'Memo' ueberlebt Neustart: {:?}",
            wb.channels.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(
            wb.chat_history.iter().any(|m| m.text == "ueberlebt das?"),
            "Chat-Verlauf ueberlebt Neustart: {:?}",
            wb.chat_history
        );
        assert!(
            wb.spawn_log.iter().any(|e| e.text.contains("alice")),
            "Spawn-Log ueberlebt Neustart"
        );
    }
    std::fs::remove_file(&db).ok();
}

/// Audio sharing status: voice only, announced once per change, ends when leaving the channel.
#[tokio::test]
async fn audio_sharing_status_follows_voice_membership() {
    let port = free_port();
    let _server = spawn_server(port, "[[channels]]\nname = \"Funkturm\"\nkind = \"voice\"\n");
    wait_for_port(port);

    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _wb) = connect(port, "bob", None).await;
    drain(&mut a, 500).await;
    drain(&mut b, 500).await;

    send(&mut a, &ClientMessage::SetAudioSharing { sharing: true }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })), "not in voice");
    assert!(drain(&mut b, 300).await.is_empty(), "a refused request is not announced");

    for conn in [&mut a, &mut b] {
        send(conn, &ClientMessage::JoinChannel { channel_id: 2 }).await;
    }
    drain(&mut a, 500).await;
    drain(&mut b, 500).await;

    send(&mut a, &ClientMessage::SetAudioSharing { sharing: true }).await;
    send(&mut a, &ClientMessage::SetAudioSharing { sharing: true }).await;
    for conn in [&mut a, &mut b] {
        let msgs = drain(conn, 500).await;
        let changes: Vec<_> = msgs
            .iter()
            .filter(|m| matches!(m, ServerMessage::AudioSharingChanged { client_id, sharing: true } if *client_id == wa.client_id))
            .collect();
        assert_eq!(changes.len(), 1, "announced once: {msgs:?}");
    }

    send(&mut a, &ClientMessage::JoinChannel { channel_id: 1 }).await;
    for conn in [&mut a, &mut b] {
        let msgs = drain(conn, 500).await;
        assert!(
            msgs.iter().any(|m| matches!(m, ServerMessage::AudioSharingChanged { client_id, sharing: false } if *client_id == wa.client_id)),
            "leaving voice ends audio sharing: {msgs:?}"
        );
    }

    // Flipping it over and over is cut off instead of flooding everyone.
    send(&mut a, &ClientMessage::JoinChannel { channel_id: 2 }).await;
    drain(&mut a, 300).await;
    drain(&mut b, 300).await;
    for i in 0..40 {
        send(&mut a, &ClientMessage::SetAudioSharing { sharing: i % 2 == 0 }).await;
    }
    let msgs = drain(&mut b, 1000).await;
    let changes = msgs.iter().filter(|m| matches!(m, ServerMessage::AudioSharingChanged { .. })).count();
    assert!(changes <= 10, "bounded broadcasts, got {changes}");
    let errors = drain(&mut a, 300).await;
    assert!(errors.iter().any(|m| matches!(m, ServerMessage::Error { .. })), "the flood is refused with an error");
}

/// Video-Relay: Sharing-Status + JPEG-Frames kommen bei Channel-Mitgliedern an.
#[tokio::test]
async fn video_relay_und_sharing() {
    let port = free_port();
    let _server = spawn_server(port, "[[channels]]\nname = \"Funkturm\"\nkind = \"voice\"\n");
    wait_for_port(port);

    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _wb) = connect(port, "bob", None).await;
    // Join-Events abarbeiten (a: 4, b: 2).
    for _ in 0..4 {
        recv(&mut a, 2000).await;
    }
    recv(&mut b, 2000).await;
    recv(&mut b, 2000).await;

    // Sharing im Spawn -> Fehler.
    send(&mut a, &ClientMessage::SetSharing { sharing: true }).await;
    assert!(matches!(recv(&mut a, 2000).await, Some(ServerMessage::Error { .. })));

    // Beide in Voice (id 2).
    for conn in [&mut a, &mut b] {
        send(conn, &ClientMessage::JoinChannel { channel_id: 2 }).await;
    }
    for _ in 0..2 {
        recv(&mut a, 2000).await;
        recv(&mut b, 2000).await;
    }

    // a teilt -> beide sehen SharingChanged.
    send(&mut a, &ClientMessage::SetSharing { sharing: true }).await;
    for conn in [&mut a, &mut b] {
        match recv(conn, 2000).await {
            Some(ServerMessage::SharingChanged { client_id, sharing }) => {
                assert_eq!(client_id, wa.client_id);
                assert!(sharing);
            }
            other => panic!("SharingChanged erwartet, bekam: {other:?}"),
        }
    }

    // Opt-in: the first frame only announces the stream - b gets no video yet.
    let frame = vec![0xFF, 0xD8, 0xFF, 1, 2, 3, 4, 5];
    for kind in [VideoKind::Camera, VideoKind::Screen] {
        send(&mut a, &ClientMessage::VideoFrame { kind, data: frame.clone() }).await;
    }
    let b_msgs = drain(&mut b, 500).await;
    assert!(!b_msgs.iter().any(|m| matches!(m, ServerMessage::VideoFrame { .. })), "no video before Watch: {b_msgs:?}");
    assert!(
        b_msgs.iter().any(|m| matches!(m, ServerMessage::StreamsChanged { client_id, streams } if *client_id == wa.client_id && streams.len() == 2)),
        "streams announced: {b_msgs:?}"
    );
    drain(&mut a, 300).await;

    // b watches both -> a is asked for a keyframe, then frames reach b (not a itself).
    for kind in [VideoKind::Camera, VideoKind::Screen] {
        send(&mut b, &ClientMessage::Watch { from_id: wa.client_id, kind, watching: true }).await;
        expect(&mut a, "KeyframeRequest", |m| matches!(m, ServerMessage::KeyframeRequest { kind: k } if *k == kind)).await;
        send(&mut a, &ClientMessage::VideoFrame { kind, data: frame.clone() }).await;
        match recv(&mut b, 2000).await {
            Some(ServerMessage::VideoFrame { from_id, kind: got, data }) => {
                assert_eq!(from_id, wa.client_id);
                assert_eq!(got, kind);
                assert_eq!(data, frame);
            }
            other => panic!("VideoFrame erwartet, bekam: {other:?}"),
        }
    }
    assert!(recv(&mut a, 400).await.is_none(), "Sender bekommt kein Echo");

    // Einzelnen Stream beenden -> b bekommt VideoStopped (and the shorter list).
    send(&mut a, &ClientMessage::StopVideo { kind: VideoKind::Camera }).await;
    match recv(&mut b, 2000).await {
        Some(ServerMessage::VideoStopped { from_id, kind }) => {
            assert_eq!(from_id, wa.client_id);
            assert_eq!(kind, VideoKind::Camera);
        }
        other => panic!("VideoStopped erwartet, bekam: {other:?}"),
    }
    expect(&mut b, "StreamsChanged", |m| matches!(m, ServerMessage::StreamsChanged { streams, .. } if streams == &vec![VideoKind::Screen])).await;
    // b stops watching the screen -> no more frames for b.
    send(&mut b, &ClientMessage::Watch { from_id: wa.client_id, kind: VideoKind::Screen, watching: false }).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    send(&mut a, &ClientMessage::VideoFrame { kind: VideoKind::Screen, data: frame.clone() }).await;
    assert!(recv(&mut b, 400).await.is_none(), "unwatched: no frames");
    drain(&mut a, 200).await;

    // b teilt nicht -> Frame von b erreicht niemanden.
    send(&mut b, &ClientMessage::VideoFrame { kind: VideoKind::Camera, data: frame }).await;
    assert!(recv(&mut a, 400).await.is_none(), "Nicht-Sharer wird nicht relayed");
}

fn parse_audio_rx(stdout: &str) -> u64 {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("AUDIO_RX="))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// UDP ping/pong + connection counters (protocol v15)
// ---------------------------------------------------------------------------

mod udp_stats {
    use super::*;
    use rusttalk_shared::audio_packet::{decode, open_packet, seal_packet, AudioHeader, Control, CONTROL_LEN, FLAG_CONTROL, MAX_PACKET_LEN};
    use rusttalk_shared::secure::{PacketCipher, DIR_TO_CLIENT, DIR_TO_SERVER};
    use tokio::net::UdpSocket;

    struct Udp {
        sock: UdpSocket,
        token: u64,
        cipher: PacketCipher,
    }

    impl Udp {
        async fn new(port: u16, w: &Welcome) -> Self {
            let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            sock.connect(("127.0.0.1", port)).await.unwrap();
            let u = Self { sock, token: w.udp_token, cipher: PacketCipher::new(&w.udp_key).unwrap() };
            u.send(&seal_packet(&AudioHeader::new(u.token, 0, 0), &u.cipher, DIR_TO_SERVER, &[])).await;
            u
        }

        async fn send(&self, pkt: &[u8]) {
            self.sock.send(pkt).await.unwrap();
        }

        fn audio(&self, seq: u32) -> Vec<u8> {
            seal_packet(&AudioHeader::new(self.token, seq, seq * 960), &self.cipher, DIR_TO_SERVER, b"opus")
        }

        fn ping(&self, id: u32) -> Vec<u8> {
            self.control(id, &Control::Ping.encode())
        }

        fn control(&self, id: u32, payload: &[u8]) -> Vec<u8> {
            let mut h = AudioHeader::new(self.token, id, 0);
            h.flags = FLAG_CONTROL;
            seal_packet(&h, &self.cipher, DIR_TO_SERVER, payload)
        }

        /// Next authenticated packet: (header, plaintext).
        async fn recv(&self, ms: u64) -> Option<(AudioHeader, Vec<u8>)> {
            let mut buf = [0u8; MAX_PACKET_LEN];
            loop {
                let n = tokio::time::timeout(Duration::from_millis(ms), self.sock.recv(&mut buf)).await.ok()?.ok()?;
                let Some((h, _)) = decode(&buf[..n]) else { continue };
                let plain = open_packet(&h, &buf[..n], &self.cipher, DIR_TO_CLIENT).expect("server packets open with our key");
                return Some((h, plain));
            }
        }

        /// All pongs until it is quiet: (ping id, audio_received, audio_relayed).
        async fn pongs(&self, ms: u64) -> Vec<(u32, u32, u32)> {
            let mut out = Vec::new();
            while let Some((h, plain)) = self.recv(ms).await {
                if h.flags & FLAG_CONTROL == 0 {
                    continue;
                }
                assert_eq!(h.token, 0, "pongs come from the server");
                match Control::decode(&plain) {
                    Some(Control::Pong { audio_received, audio_relayed }) => out.push((h.sequence, audio_received, audio_relayed)),
                    other => panic!("pong expected, got {other:?}"),
                }
            }
            out
        }
    }

    async fn two_in_voice(port: u16) -> ((Conn, Welcome), (Conn, Welcome)) {
        let (mut a, wa) = connect(port, "alice", None).await;
        let (mut b, wb) = connect(port, "bob", None).await;
        for conn in [&mut a, &mut b] {
            send(conn, &ClientMessage::JoinChannel { channel_id: 2 }).await;
        }
        for conn in [&mut a, &mut b] {
            drain(conn, 300).await;
        }
        ((a, wa), (b, wb))
    }

    const VOICE: &str = "[[channels]]\nname = \"Funkturm\"\nkind = \"voice\"\n";

    #[tokio::test]
    async fn ping_pong_reports_packet_counters() {
        let port = free_port();
        let _server = spawn_server(port, VOICE);
        wait_for_port(port);
        let ((_a, wa), (_b, wb)) = two_in_voice(port).await;
        let ua = Udp::new(port, &wa).await;
        let ub = Udp::new(port, &wb).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        for seq in 1..=5 {
            ua.send(&ua.audio(seq)).await;
        }
        let mut got = 0;
        while let Some((h, plain)) = ub.recv(500).await {
            assert_eq!(h.flags & FLAG_CONTROL, 0);
            assert_eq!(plain, b"opus");
            got += 1;
        }
        assert_eq!(got, 5);

        ua.send(&ua.ping(41)).await;
        assert_eq!(ua.pongs(500).await, vec![(41, 5, 0)], "alice: 5 received from her, nothing relayed to her");
        ub.send(&ub.ping(7)).await;
        assert_eq!(ub.pongs(500).await, vec![(7, 0, 5)], "bob: 5 relayed to him");
        assert!(ub.recv(300).await.is_none(), "alice's ping is never relayed to bob");
    }

    #[tokio::test]
    async fn malformed_forged_and_replayed_packets_get_no_pong() {
        let port = free_port();
        let _server = spawn_server(port, VOICE);
        wait_for_port(port);
        let ((_a, wa), (_b, wb)) = two_in_voice(port).await;
        let ua = Udp::new(port, &wa).await;
        let ub = Udp::new(port, &wb).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        let ping = ua.ping(1);
        ua.send(&ping[..ping.len() - 1]).await; // truncated
        ua.send(&ua.control(2, &[0u8; CONTROL_LEN + 1])).await; // oversized control payload
        ua.send(&ua.control(3, &[1u8; CONTROL_LEN])).await; // bad ping padding
        ua.send(&ua.control(4, &Control::Pong { audio_received: 1, audio_relayed: 1 }.encode())).await; // a client cannot send pongs
        let forged = PacketCipher::new(&PacketCipher::random_key()).unwrap();
        let mut h = AudioHeader::new(ua.token, 5, 0);
        h.flags = FLAG_CONTROL;
        ua.send(&seal_packet(&h, &forged, DIR_TO_SERVER, &Control::Ping.encode())).await; // wrong key
        let mut h = AudioHeader::new(ua.token ^ 1, 6, 0);
        h.flags = FLAG_CONTROL;
        ua.send(&seal_packet(&h, &ua.cipher, DIR_TO_SERVER, &Control::Ping.encode())).await; // unknown token
        ua.send(&[0u8; MAX_PACKET_LEN]).await; // garbage, full size
        assert!(ua.pongs(500).await.is_empty(), "no pong for bad packets");

        // Replay: the same ping and the same audio packet count once.
        ua.send(&ping).await;
        ua.send(&ping).await;
        assert_eq!(ua.pongs(500).await.len(), 1, "replayed ping is dropped");
        let audio = ua.audio(1);
        ua.send(&audio).await;
        ua.send(&audio).await;
        let mut got = 0;
        while ub.recv(400).await.is_some() {
            got += 1;
        }
        assert_eq!(got, 1, "replayed audio is not relayed");
        tokio::time::sleep(Duration::from_millis(1100)).await; // refill one ping token
        ua.send(&ua.ping(9)).await;
        assert_eq!(ua.pongs(500).await, vec![(9, 1, 0)], "replayed audio is not counted");
    }

    #[tokio::test]
    async fn audio_only_relay_needs_voice_membership_and_authentication() {
        use rusttalk_shared::audio_packet::FLAG_SCREEN_AUDIO;
        let port = free_port();
        let _server = spawn_server(port, VOICE);
        wait_for_port(port);
        let ((mut a, wa), (mut b, wb)) = two_in_voice(port).await;
        let ua = Udp::new(port, &wa).await;
        let ub = Udp::new(port, &wb).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!wa.users.iter().any(|u| u.sharing));
        let mut header = AudioHeader::new(ua.token, 1, 960);
        header.flags = FLAG_SCREEN_AUDIO;
        let packet = seal_packet(&header, &ua.cipher, DIR_TO_SERVER, b"audio only");
        ua.send(&packet).await;
        let (got, plain) = ub.recv(500).await.expect("audio without video sharing");
        assert_eq!(got.flags, FLAG_SCREEN_AUDIO);
        assert_eq!(got.token, wa.client_id);
        assert_eq!(plain, b"audio only");
        ua.send(&packet).await; // replay
        let mut forged = seal_packet(&header, &ua.cipher, DIR_TO_SERVER, b"forged");
        *forged.last_mut().unwrap() ^= 1;
        ua.send(&forged).await;
        let truncated = seal_packet(&header, &ua.cipher, DIR_TO_SERVER, b"truncated");
        ua.send(&truncated[..truncated.len() - 1]).await;
        assert!(ub.recv(200).await.is_none());
        send(&mut b, &ClientMessage::SetMuteState { muted: false, deafened: true }).await;
        drain(&mut b, 200).await;
        ua.send(&seal_packet(&header, &ua.cipher, DIR_TO_SERVER, b"deafened")).await;
        assert!(ub.recv(200).await.is_none());
        send(&mut b, &ClientMessage::SetMuteState { muted: false, deafened: false }).await;
        send(&mut a, &ClientMessage::JoinChannel { channel_id: 1 }).await;
        drain(&mut a, 200).await;
        drain(&mut b, 200).await;
        ua.send(&seal_packet(&header, &ua.cipher, DIR_TO_SERVER, b"lobby")).await;
        assert!(ub.recv(200).await.is_none(), "lobby cannot send system audio");
    }

    #[tokio::test]
    async fn ping_flood_is_rate_limited() {
        let port = free_port();
        let _server = spawn_server(port, VOICE);
        wait_for_port(port);
        let ((_a, wa), _b) = two_in_voice(port).await;
        let ua = Udp::new(port, &wa).await;
        for id in 1..=50 {
            ua.send(&ua.ping(id)).await;
        }
        let pongs = ua.pongs(500).await;
        assert!((1..=4).contains(&pongs.len()), "burst of at most 3 (+1 refill), got {}", pongs.len());
        tokio::time::sleep(Duration::from_millis(1200)).await;
        ua.send(&ua.ping(99)).await;
        assert_eq!(ua.pongs(500).await.len(), 1, "answers again after the limit refills");
    }

    #[tokio::test]
    async fn old_protocol_version_is_refused() {
        let port = free_port();
        let _server = spawn_server(port, "");
        wait_for_port(port);
        let mut stream = tls_connect(port).await;
        let hello = ClientMessage::Hello { username: "old".into(), protocol_version: PROTOCOL_VERSION - 1, password: None, identity: identity_for("old"), invite: None };
        write_frame(&mut stream, &hello).await.unwrap();
        match read_frame::<_, ServerMessage>(&mut stream).await {
            Ok(Some(ServerMessage::Error { message })) => assert!(message.contains("Protocol version"), "{message}"),
            other => panic!("refusal expected, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Container image behaviour (RUSTTALK_CONTAINER=1), health check, shutdown
// ---------------------------------------------------------------------------

const ADMIN_SECRET: &str = "integration-admin-secret-0123456789";

/// Fresh directory per test (the "volume").
fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rusttalk-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the server binary to its exit (at most 15 s); returns (success, stdout, stderr).
fn run_to_exit(args: &[&str], env: &[(&str, String)]) -> (bool, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rusttalk-server"))
        .args(args)
        .env_remove("RUSTTALK_CONTAINER")
        .env_remove("RUSTTALK_ADMIN_PASSWORD_FILE")
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start server process");
    for _ in 0..150 {
        if child.try_wait().unwrap().is_some() {
            let out = child.wait_with_output().unwrap();
            return (out.status.success(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    panic!("server did not exit: {args:?}");
}

fn healthy(config: &std::path::Path) -> bool {
    run_to_exit(&["--healthcheck", config.to_str().unwrap()], &[]).0
}

#[tokio::test]
async fn healthcheck_follows_the_listener() {
    let port = free_port();
    let config = std::env::temp_dir().join(format!("rusttalk-it-{port}.toml"));
    {
        let _server = spawn_server(port, "");
        wait_for_port(port);
        assert!(healthy(&config), "listener is up");
    }
    // ServerGuard killed it: the port is closed again.
    assert!(!healthy(&config), "listener is down");
    let (ok, _, err) = run_to_exit(&["--healthcheck", "/nonexistent/rusttalk/server.toml"], &[]);
    assert!(!ok && err.contains("unhealthy"), "{err}");
}

#[test]
fn bad_admin_secrets_fail_closed_without_leaking() {
    let dir = data_dir("badsecret");
    let config = dir.join("server.toml");
    let marker = "LEAKMARKER";
    let short = dir.join("short");
    std::fs::write(&short, format!("{marker}\n")).unwrap();
    let big = dir.join("big");
    std::fs::write(&big, format!("{marker}{}", "x".repeat(2000))).unwrap();
    let cases = [(short, "shorter than"), (big, "larger than"), (dir.join("missing"), "Cannot read")];
    for container in [false, true] {
        for (secret, want) in &cases {
            let mut env = vec![("RUSTTALK_ADMIN_PASSWORD_FILE", secret.to_string_lossy().into_owned())];
            if container {
                env.push(("RUSTTALK_CONTAINER", "1".into()));
            }
            let (ok, out, err) = run_to_exit(&[config.to_str().unwrap()], &env);
            assert!(!ok, "must not start ({secret:?}, container={container})");
            assert!(err.contains(want) && err.contains("RUSTTALK_ADMIN_PASSWORD_FILE"), "{err}");
            assert!(!out.contains(marker) && !err.contains(marker), "secret leaked: {out}{err}");
            assert!(!config.exists(), "nothing is created before the secret is valid");
        }
    }

    // Container without any admin secret: refuses, no database or key is created.
    let (ok, _, err) = run_to_exit(&[config.to_str().unwrap()], &[("RUSTTALK_CONTAINER", "1".into())]);
    assert!(!ok && err.contains("No admin password"), "{err}");
    assert!(config.exists(), "first run wrote the default config");
    for name in ["rusttalk.db", "tls-key.der", "tls-cert.der"] {
        assert!(!dir.join(name).exists(), "{name} must not exist yet");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn container_mode_is_closed_until_the_admin_logs_in() {
    let dir = data_dir("container");
    let port = free_port();
    let secret = dir.join("admin_password");
    std::fs::write(&secret, format!("{ADMIN_SECRET}\n")).unwrap();
    let extra = format!("persist_state = true\ndb_path = {:?}\n", dir.join("rusttalk.db").to_string_lossy());
    let server = spawn_server_with_env(
        port,
        &extra,
        &[("RUSTTALK_CONTAINER", "1".into()), ("RUSTTALK_ADMIN_PASSWORD_FILE", secret.to_string_lossy().into_owned())],
    );
    wait_for_port(port);
    #[cfg(unix)]
    for name in ["tls-key.der", "tls-cert.der"] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
    }

    let (mut a, wa) = connect(port, "alice", None).await;
    let (mut b, _) = connect(port, "bob", None).await;
    assert!(!wa.open_permissions, "no open mode without an admin");
    drain(&mut a, 300).await;
    drain(&mut b, 300).await;
    send(&mut b, &ClientMessage::ClaimAdmin { password: None }).await;
    match expect(&mut b, "Error", |m| matches!(m, ServerMessage::Error { .. })).await {
        ServerMessage::Error { message } => assert!(message.contains("Wrong admin password"), "{message}"),
        _ => unreachable!(),
    }
    send(&mut b, &ClientMessage::ClaimAdmin { password: Some("guess".into()) }).await;
    expect(&mut b, "Error", |m| matches!(m, ServerMessage::Error { .. })).await;
    send(&mut b, &ClientMessage::CreateChannel { name: "x".into(), kind: ChannelKind::Text, parent: None }).await;
    expect(&mut b, "Error", |m| matches!(m, ServerMessage::Error { .. })).await;

    // Both test clients share an IP; failed guesses back off across reconnects/identities.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    send(&mut a, &ClientMessage::ClaimAdmin { password: Some(ADMIN_SECRET.into()) }).await;
    expect(&mut a, "alice admin", |m| matches!(m, ServerMessage::UserRoleChanged { client_id, role: Role::Admin } if *client_id == wa.client_id)).await;
    send(&mut a, &ClientMessage::CreateChannel { name: "ops".into(), kind: ChannelKind::Text, parent: None }).await;
    expect(&mut a, "ChannelCreated", |m| matches!(m, ServerMessage::ChannelCreated { .. })).await;
    drop(server);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn container_refuses_update_installs_but_still_checks() {
    let gh = fake_github();
    let api = ("RUSTTALK_UPDATE_API", format!("http://127.0.0.1:{gh}"));
    let container = ("RUSTTALK_CONTAINER", "1".to_string());

    let (ok, out, err) = run_to_exit(&["update"], &[api.clone(), container.clone()]);
    assert!(!ok, "install refused");
    assert!(err.contains("pulling the new image"), "{err}");
    assert!(!out.contains("New version"), "refused before asking GitHub: {out}");

    let (ok, out, _) = run_to_exit(&["update", "--check"], &[api, container]);
    assert!(ok && out.contains("New version available") && out.contains("99.0.0"), "{out}");
}

#[test]
fn update_missing_checksum_refuses_before_downloading_or_installing() {
    let gh = fake_github_with_checksum(false);
    let (ok, _, err) = run_to_exit(
        &["update", "--no-restart"],
        &[
            ("RUSTTALK_UPDATE_API", format!("http://127.0.0.1:{gh}")),
            ("RUSTTALK_CONTAINER", "0".to_string()),
        ],
    );
    assert!(!ok);
    assert!(err.contains("no checksum file"), "{err}");
}

/// docker stop / systemctl stop: SIGTERM closes SQLite cleanly, data and key survive.
#[cfg(unix)]
#[tokio::test]
async fn sigterm_shuts_down_cleanly() {
    let dir = data_dir("sigterm");
    let port = free_port();
    let db = dir.join("rusttalk.db");
    let extra = format!("persist_state = true\ndb_path = {:?}\n", db.to_string_lossy());

    let mut server = spawn_server(port, &extra);
    wait_for_port(port);
    let (mut a, _) = connect(port, "alice", None).await;
    drain(&mut a, 300).await;
    send(&mut a, &ClientMessage::CreateChannel { name: "kept".into(), kind: ChannelKind::Text, parent: None }).await;
    expect(&mut a, "ChannelCreated", |m| matches!(m, ServerMessage::ChannelCreated { .. })).await;
    let cert = std::fs::read(dir.join("tls-cert.der")).unwrap();

    let pid = server.0.id().to_string();
    assert!(Command::new("kill").args(["-TERM", &pid]).status().unwrap().success());
    let mut status = None;
    for _ in 0..50 {
        status = server.0.try_wait().unwrap();
        if status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = status.expect("server stops within 5 s after SIGTERM");
    assert!(status.success(), "clean exit, not killed: {status:?}");
    let wal = dir.join("rusttalk.db-wal");
    assert!(std::fs::metadata(&wal).map(|m| m.len() == 0).unwrap_or(true), "WAL checkpointed on close");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(check, "ok");
    drop(conn);

    let _server = spawn_server(port, &extra);
    wait_for_port(port);
    let (_a, wa) = connect(port, "alice", None).await;
    assert!(wa.channels.iter().any(|c| c.name == "kept"), "{:?}", wa.channels);
    assert_eq!(std::fs::read(dir.join("tls-cert.der")).unwrap(), cert, "same certificate (pinned fingerprint)");
    std::fs::remove_dir_all(&dir).ok();
}
// wdac-dice: 2

#[test]
fn config_errors_never_print_secret_values() {
    let dir = data_dir("config-redaction");
    let config = dir.join("server.toml");
    let marker = "PRIVATE_CONFIG_VALUE_0123456789";
    for text in [
        format!("password = [\"{marker}\"]"),
        format!("admin_password = \"{marker}"),
    ] {
        std::fs::write(&config, text).unwrap();
        for args in [
            vec![config.to_str().unwrap()],
            vec!["--healthcheck", config.to_str().unwrap()],
        ] {
            let (ok, out, err) = run_to_exit(&args, &[]);
            assert!(!ok);
            assert!(
                err.contains("Invalid server configuration near line"),
                "{err}"
            );
            assert!(!out.contains(marker) && !err.contains(marker));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_writes_acknowledged_during_shutdown_are_persisted() {
    let dir = data_dir("shutdown-writes");
    let port = free_port();
    let db = dir.join("rusttalk.db");
    let extra = format!(
        "persist_state = true\ndb_path = {:?}\n",
        db.to_string_lossy()
    );
    let mut server = spawn_server(port, &extra);
    wait_for_port(port);
    let ((mut reader, mut writer), _) = connect(port, "writer", None).await;
    let writing = tokio::spawn(async move {
        for i in 0..100 {
            let msg = ClientMessage::CreateChannel {
                name: format!("concurrent-{i}"),
                kind: ChannelKind::Text,
                parent: None,
            };
            if write_frame(&mut writer, &msg).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    let mut acknowledged = Vec::new();
    let mut signalled = false;
    loop {
        match tokio::time::timeout(
            Duration::from_secs(5),
            read_frame::<_, ServerMessage>(&mut reader),
        )
        .await
        {
            Ok(Ok(Some(ServerMessage::ChannelCreated { channel }))) => {
                acknowledged.push(channel.name);
                if acknowledged.len() == 5 {
                    assert!(Command::new("kill")
                        .args(["-TERM", &server.0.id().to_string()])
                        .status()
                        .unwrap()
                        .success());
                    signalled = true;
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(_) => break,
            Err(_) => panic!("shutdown did not close the writer connection"),
        }
    }
    writing.await.unwrap();
    assert!(signalled);
    assert!(server.0.wait().unwrap().success());
    let conn = rusqlite::Connection::open(&db).unwrap();
    for name in acknowledged {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channels WHERE name = ?1",
                [&name],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "acknowledged channel {name} was lost");
    }
    drop(conn);
    std::fs::remove_dir_all(dir).unwrap();
}
