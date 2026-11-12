//! Netzwerk-Thread des Clients: TCP-Steuerkanal (tokio) + UDP-Socket-Uebergabe.
//!
//! Die UI kommuniziert ueber [`Command`] (raus) und [`NetEvent`] (rein).
//! Der UDP-Socket wird nach dem Login an die Audio-Engine uebergeben.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;
use std::time::Duration;

use rusttalk_shared::framing::{read_frame, write_frame};
use rusttalk_shared::protocol::*;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

use crate::config::ClientConfig;
use rusttalk_shared::secure;

/// Befehle der UI an den Netzwerk-Thread.
#[derive(Debug)]
pub enum Command {
    JoinChannel(u64),
    CreateChannel { name: String, kind: ChannelKind, parent: Option<u64> },
    DeleteChannel(u64),
    RenameChannel(u64, String),
    MoveChannel { channel_id: u64, parent: Option<u64>, index: u32 },
    UpdateServerSettings(ServerSettings),
    SendChat { channel_id: u64, text: String, image: Option<Vec<u8>>, reply_to: Option<u64> },
    EditChat { message_id: u64, text: String },
    DeleteChat(u64),
    /// GIF search through the server (empty query = trending).
    GifSearch { query: String, offset: u32 },
    /// Set / remove the server's GIPHY key.
    SetGifKey(Option<String>),
    /// Any other client message (moderation, roles, invites).
    Raw(ClientMessage),
    SetMute { muted: bool, deafened: bool },
    SetSharing(bool),
    VideoFrame(VideoKind, Vec<u8>),
    StopVideo(VideoKind),
    Disconnect,
}

/// Ereignisse des Netzwerk-Threads an die UI.
#[derive(Debug)]
pub enum NetEvent {
    /// Login erfolgreich; enthaelt den initialen Server-State und den UDP-Socket.
    Connected {
        my_id: u64,
        settings: ServerSettings,
        users: Vec<UserInfo>,
        channels: Vec<ChannelInfo>,
        udp_socket: Option<std::net::UdpSocket>,
        /// Key for the encrypted audio packets of this session.
        udp_cipher: Option<Arc<rusttalk_shared::secure::PacketCipher>>,
        /// SHA-256 fingerprint of the server's TLS certificate.
        fingerprint: String,
        udp_token: u64,
        spawn_log: Vec<LogEntry>,
        chat_history: Vec<ChatRecord>,
        /// The server offers GIF search.
        gif_search: bool,
        /// No admin on the server yet: everyone may do everything.
        open_permissions: bool,
    },
    Message(ServerMessage),
    /// Fehler/Infos aus lokalen Hintergrund-Threads (z. B. Kamera).
    Status(String),
    Disconnected { reason: String },
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Startet den Verbindungsaufbau in einem Hintergrund-Thread.
/// Rueckgabe: Sender fuer Befehle (beim Drop wird getrennt).
/// `ctx` wird nach jeder eingehenden Nachricht zum Neuzeichnen angestossen
/// (sonst erscheinen Video-Frames erst beim naechsten 100-ms-Tick).
/// `upload_backlog`: Video-Bytes in der Sende-Queue; der Writer zieht nach
/// jedem gesendeten Frame ab (die UI verwirft neue Frames, wenn es staut).
pub fn connect(
    cfg: ClientConfig,
    event_tx: StdSender<NetEvent>,
    ctx: eframe::egui::Context,
    upload_backlog: Arc<AtomicUsize>,
) -> UnboundedSender<Command> {
    let (cmd_tx, cmd_rx) = unbounded_channel::<Command>();
    std::thread::Builder::new()
        .name("net".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let mut cmd_rx = cmd_rx;
                if let Err(e) = run(cfg, &mut cmd_rx, &event_tx, &ctx, &upload_backlog).await {
                    let _ = event_tx.send(NetEvent::Disconnected { reason: format!("{e:#}") });
                }
            });
        })
        .expect("Netzwerk-Thread starten");
    cmd_tx
}

pub fn normalize_addr(addr: &str) -> String {
    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    }
}

async fn run(
    cfg: ClientConfig,
    cmd_rx: &mut UnboundedReceiver<Command>,
    event_tx: &StdSender<NetEvent>,
    ctx: &eframe::egui::Context,
    upload_backlog: &AtomicUsize,
) -> anyhow::Result<()> {
    let addr = normalize_addr(&cfg.server_address);
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout while connecting to {addr}"))??;
    tcp.set_nodelay(true).ok();
    let peer = tcp.peer_addr()?;

    // TLS with the server certificate pinned on first use (like SSH).
    let expected = cfg.known_servers.get(&addr).cloned();
    let verifier = secure::PinnedVerifier::new(expected.clone());
    let connector = tokio_rustls::TlsConnector::from(secure::client_config(verifier.clone()));
    let name = rustls::pki_types::ServerName::try_from(secure::TLS_SERVER_NAME)?;
    let stream = match timeout(CONNECT_TIMEOUT, connector.connect(name, tcp)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let seen = verifier.seen.lock().unwrap().clone();
            if let (Some(seen), Some(want)) = (seen, expected) {
                if !seen.eq_ignore_ascii_case(&want) {
                    anyhow::bail!("{}: {seen}", secure::CERT_CHANGED);
                }
            }
            anyhow::bail!("Secure connection failed: {e} (old server version?)");
        }
        Err(_) => anyhow::bail!("Timeout during the secure handshake with {addr}"),
    };
    let fingerprint = verifier.seen.lock().unwrap().clone().unwrap_or_default();
    let (mut reader, mut writer) = tokio::io::split(stream);

    write_frame(
        &mut writer,
        &ClientMessage::Hello {
            username: cfg.username.clone(),
            protocol_version: PROTOCOL_VERSION,
            password: cfg.server_password.clone(),
            identity: cfg.identity.clone(),
            invite: cfg.invite.clone(),
        },
    )
    .await?;

    let first: Option<ServerMessage> = timeout(CONNECT_TIMEOUT, read_frame(&mut reader))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout while waiting for the server"))??;
    let (client_id, settings, udp_token, channels, users, spawn_log, chat_history, gif_search, open_permissions, udp_key) = match first {
        Some(ServerMessage::Welcome { client_id, udp_token, channels, users, spawn_log, chat_history, settings, gif_search, open_permissions, udp_key }) => {
            (client_id, settings, udp_token, channels, users, spawn_log, chat_history, gif_search, open_permissions, udp_key)
        }
        Some(ServerMessage::Error { message }) => anyhow::bail!("{message}"),
        other => anyhow::bail!("Unexpected server response: {other:?}"),
    };

    // UDP-Socket (std) fuer die Audio-Threads vorbereiten.
    let udp = std::net::UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(peer)?;
    let _ = event_tx.send(NetEvent::Connected {
        my_id: client_id,
        settings,
        users,
        channels,
        udp_socket: Some(udp),
        udp_cipher: secure::PacketCipher::new(&udp_key).map(Arc::new),
        fingerprint,
        udp_token,
        spawn_log,
        chat_history,
        gif_search,
        open_permissions,
    });

    // Lesen in eigenem Task: `read_frame` ist NICHT abbruchsicher. Lief es
    // frueher als Zweig im `select!`, wurde es bei jedem ausgehenden Befehl
    // (25 fps Video!) mitten in einem Frame abgebrochen -> Stream verschoben
    // -> "Netzwerkfehler". Der Task liest immer komplette Frames.
    let mut reader_task = tokio::spawn({
        let event_tx = event_tx.clone();
        let ctx = ctx.clone();
        async move {
            loop {
                match read_frame::<_, ServerMessage>(&mut reader).await {
                    Ok(Some(m)) => {
                        if event_tx.send(NetEvent::Message(m)).is_err() {
                            return Ok(()); // UI ist weg
                        }
                        ctx.request_repaint();
                    }
                    Ok(None) => anyhow::bail!("The server closed the connection"),
                    Err(e) => anyhow::bail!("Network error: {e}"),
                }
            }
        }
    });

    loop {
        tokio::select! {
            res = &mut reader_task => {
                return match res {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("Receive task: {e}")),
                };
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::Disconnect) | None => {
                        reader_task.abort();
                        let _ = write_frame(&mut writer, &ClientMessage::Disconnect).await;
                        return Ok(());
                    }
                    Some(cmd) => {
                        let msg = match cmd {
                            Command::JoinChannel(channel_id) => ClientMessage::JoinChannel { channel_id },
                            Command::CreateChannel { name, kind, parent } => {
                                ClientMessage::CreateChannel { name, kind, parent }
                            }
                            Command::DeleteChannel(channel_id) => ClientMessage::DeleteChannel { channel_id },
                            Command::UpdateServerSettings(settings) => {
                                ClientMessage::UpdateServerSettings { settings }
                            }
                            Command::MoveChannel { channel_id, parent, index } => {
                                ClientMessage::MoveChannel { channel_id, parent, index }
                            }
                            Command::RenameChannel(channel_id, name) => {
                                ClientMessage::RenameChannel { channel_id, name }
                            }
                            Command::SendChat { channel_id, text, image, reply_to } => {
                                ClientMessage::Chat { channel_id, text, image, reply_to }
                            }
                            Command::EditChat { message_id, text } => ClientMessage::EditChat { message_id, text },
                            Command::DeleteChat(message_id) => ClientMessage::DeleteChat { message_id },
                            Command::GifSearch { query, offset } => ClientMessage::GifSearch { query, offset },
                            Command::SetGifKey(key) => ClientMessage::SetGifKey { key },
                            Command::Raw(msg) => msg,
                            Command::SetMute { muted, deafened } => {
                                ClientMessage::SetMuteState { muted, deafened }
                            }
                            Command::SetSharing(sharing) => ClientMessage::SetSharing { sharing },
                            Command::VideoFrame(kind, data) => ClientMessage::VideoFrame { kind, data },
                            Command::StopVideo(kind) => ClientMessage::StopVideo { kind },
                            Command::Disconnect => unreachable!("oben behandelt"),
                        };
                        let video_len = match &msg {
                            ClientMessage::VideoFrame { data, .. } => data.len(),
                            _ => 0,
                        };
                        // Dieser Zweig laeuft immer zu Ende (nur das Warten auf
                        // `recv` wird von select! abgebrochen) -> Frames bleiben ganz.
                        let res = write_frame(&mut writer, &msg).await;
                        upload_backlog.fetch_sub(video_len, Ordering::Relaxed);
                        if let Err(e) = res {
                            reader_task.abort();
                            anyhow::bail!("Network error while sending: {e}");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::net::TcpListener;

    const N: usize = 150;
    const SIZE: usize = 180 * 1024;

    /// Fake-Server: Welcome, dann N grosse Video-Frames; liest parallel mit
    /// und zaehlt die vom Client hochgeladenen Video-Frames.
    async fn fake_server(listener: TcpListener, received: Arc<AtomicUsize>) {
        let (tcp, _) = listener.accept().await.unwrap();
        let ck = rcgen_free_cert();
        let tls = rusttalk_shared::secure::server_config(ck.0, ck.1).unwrap();
        let stream = tokio_rustls::TlsAcceptor::from(tls).accept(tcp).await.unwrap();
        let (mut r, mut w) = tokio::io::split(stream);
        let _hello: Option<ClientMessage> = read_frame(&mut r).await.unwrap();
        tokio::spawn(async move {
            while let Ok(Some(msg)) = read_frame::<_, ClientMessage>(&mut r).await {
                if matches!(msg, ClientMessage::VideoFrame { .. }) {
                    received.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        let welcome = ServerMessage::Welcome {
            settings: ServerSettings::default(),
            client_id: 1,
            udp_token: 7,
            channels: vec![],
            users: vec![],
            spawn_log: vec![],
            chat_history: vec![],
            gif_search: false,
            open_permissions: true,
            udp_key: vec![1; 32],
        };
        write_frame(&mut w, &welcome).await.unwrap();
        for i in 0..N {
            let data = vec![(i % 251) as u8; SIZE];
            let msg = ServerMessage::VideoFrame { from_id: 2, kind: VideoKind::Screen, data };
            write_frame(&mut w, &msg).await.unwrap();
        }
        // Verbindung offen halten, bis der Test fertig ist.
        tokio::time::sleep(Duration::from_secs(30)).await;
    }

    /// Regression: Frueher lief `read_frame` als Zweig im `select!` und wurde
    /// bei jedem ausgehenden Befehl mitten im Frame abgebrochen -> Stream
    /// verschoben -> "Netzwerkfehler". Hier gehen grosse Video-Frames
    /// gleichzeitig in beide Richtungen.
    #[test]
    fn large_frames_in_both_directions_stay_intact() {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let received_by_server = Arc::new(AtomicUsize::new(0));
        let received = received_by_server.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = TcpListener::from_std(std_listener).unwrap();
                fake_server(listener, received).await;
            });
        });

        let cfg = ClientConfig { server_address: addr.to_string(), ..ClientConfig::default() };
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        let cmd_tx = connect(cfg, event_tx, eframe::egui::Context::default(), backlog.clone());

        // Client laedt gleichzeitig Video hoch.
        for _ in 0..N {
            backlog.fetch_add(SIZE, Ordering::Relaxed);
            cmd_tx.send(Command::VideoFrame(VideoKind::Camera, vec![9u8; SIZE])).unwrap();
        }

        let mut frames = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while frames < N {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match event_rx.recv_timeout(left) {
                Ok(NetEvent::Message(ServerMessage::VideoFrame { data, .. })) => {
                    assert_eq!(data.len(), SIZE);
                    assert!(data.iter().all(|&b| b == (frames % 251) as u8), "Frame {frames} korrupt");
                    frames += 1;
                }
                Ok(NetEvent::Disconnected { reason }) => panic!("Verbindung verloren: {reason}"),
                Ok(_) => {}
                Err(_) => panic!("Timeout: nur {frames}/{N} Frames empfangen"),
            }
        }
        // Upload kam komplett an und der Zaehler ist wieder bei 0.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while received_by_server.load(Ordering::Relaxed) < N && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(received_by_server.load(Ordering::Relaxed), N);
        assert_eq!(backlog.load(Ordering::Relaxed), 0);
        let _ = cmd_tx.send(Command::Disconnect);
    }

    /// A remembered fingerprint that does not match -> refused with CERT_CHANGED
    /// (no Hello is sent, nothing reaches the server).
    #[test]
    fn changed_certificate_is_refused() {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = TcpListener::from_std(std_listener).unwrap();
                let (tcp, _) = listener.accept().await.unwrap();
                let (c, k) = rcgen_free_cert();
                let tls = rusttalk_shared::secure::server_config(c, k).unwrap();
                let _ = tokio_rustls::TlsAcceptor::from(tls).accept(tcp).await; // fails: client aborts
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
        });
        let mut cfg = ClientConfig { server_address: addr.to_string(), ..ClientConfig::default() };
        cfg.known_servers.insert(addr.to_string(), "00:11:22".into());
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let _tx = connect(cfg, event_tx, eframe::egui::Context::default(), Arc::new(AtomicUsize::new(0)));
        match event_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(NetEvent::Disconnected { reason }) => {
                assert!(reason.starts_with(secure::CERT_CHANGED), "{reason}");
                let real = secure::fingerprint(&rcgen_free_cert().0);
                assert!(reason.contains(&real), "new fingerprint is reported: {reason}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// Self-signed test certificate (DER cert, PKCS#8 key), generated once per test run.
    fn rcgen_free_cert() -> (Vec<u8>, Vec<u8>) {
        static CERT: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
        CERT.get_or_init(|| {
            let ck = rcgen::generate_simple_self_signed(vec![secure::TLS_SERVER_NAME.to_string()]).unwrap();
            (ck.cert.der().to_vec(), ck.signing_key.serialize_der())
        })
        .clone()
    }
}
