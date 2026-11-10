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
    write!(f, "{extra}").unwrap();
    path
}

fn spawn_server(port: u16, extra_config: &str) -> ServerGuard {
    let config = write_config(port, extra_config);
    let child = Command::new(env!("CARGO_BIN_EXE_rusttalk-server"))
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

type Conn = (ReadHalf<TcpStream>, WriteHalf<TcpStream>);

struct Welcome {
    client_id: u64,
    channels: Vec<ChannelInfo>,
    users: Vec<UserInfo>,
    spawn_log: Vec<LogEntry>,
    chat_history: Vec<ChatRecord>,
}

async fn connect(port: u16, name: &str, password: Option<String>) -> (Conn, Welcome) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.expect("TCP connect");
    write_frame(
        &mut stream,
        &ClientMessage::Hello { username: name.to_string(), protocol_version: PROTOCOL_VERSION, password },
    )
    .await
    .unwrap();
    let (reader, writer) = tokio::io::split(stream);
    let mut conn = (reader, writer);
    let first: ServerMessage = read_frame(&mut conn.0).await.unwrap().expect("erste Nachricht");
    match first {
        ServerMessage::Welcome { client_id, channels, users, spawn_log, chat_history, .. } => {
            (conn, Welcome { client_id, channels, users, spawn_log, chat_history })
        }
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
    assert_eq!(wa.channels[0].name, "Spawn");
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
    send(&mut a, &ClientMessage::Chat { channel_id: text_id, text: "hallo info".into(), image: None }).await;
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
    send(&mut a, &ClientMessage::Chat { channel_id: 1, text: "x".into(), image: None }).await;
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

#[tokio::test]
async fn passwort_schutz() {
    let port = free_port();
    let _server = spawn_server(port, "password = \"geheim\"\n");
    wait_for_port(port);

    // Falsches Passwort -> Error-Frame, dann Schluss.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    write_frame(
        &mut stream,
        &ClientMessage::Hello {
            username: "eve".into(),
            protocol_version: PROTOCOL_VERSION,
            password: Some("falsch".into()),
        },
    )
    .await
    .unwrap();
    let (mut r, _w) = tokio::io::split(stream);
    match read_frame(&mut r).await.unwrap() {
        Some(ServerMessage::Error { message }) => {
            assert!(message.contains("Passwort"), "unerwartete Fehlermeldung: {message}")
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
        send(&mut a, &ClientMessage::Chat { channel_id, text: "ueberlebt das?".into(), image: None }).await;
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

    // Video-Frame von a kommt bei b an (nicht bei a selbst).
    let frame = vec![0xFF, 0xD8, 0xFF, 1, 2, 3, 4, 5];
    // Kamera und Bildschirm gleichzeitig: beide Arten kommen getrennt an.
    for kind in [VideoKind::Camera, VideoKind::Screen] {
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

    // Einzelnen Stream beenden -> b bekommt VideoStopped.
    send(&mut a, &ClientMessage::StopVideo { kind: VideoKind::Camera }).await;
    match recv(&mut b, 2000).await {
        Some(ServerMessage::VideoStopped { from_id, kind }) => {
            assert_eq!(from_id, wa.client_id);
            assert_eq!(kind, VideoKind::Camera);
        }
        other => panic!("VideoStopped erwartet, bekam: {other:?}"),
    }

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
// wdac-dice: 2
