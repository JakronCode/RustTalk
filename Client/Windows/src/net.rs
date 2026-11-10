//! Netzwerk-Thread des Clients: TCP-Steuerkanal (tokio) + UDP-Socket-Uebergabe.
//!
//! Die UI kommuniziert ueber [`Command`] (raus) und [`NetEvent`] (rein).
//! Der UDP-Socket wird nach dem Login an die Audio-Engine uebergeben.

use std::sync::mpsc::Sender as StdSender;
use std::time::Duration;

use rusttalk_shared::framing::{read_frame, write_frame};
use rusttalk_shared::protocol::*;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

use crate::config::ClientConfig;

/// Befehle der UI an den Netzwerk-Thread.
#[derive(Debug)]
pub enum Command {
    JoinChannel(u64),
    CreateChannel { name: String, kind: ChannelKind, parent: Option<u64> },
    DeleteChannel(u64),
    RenameChannel(u64, String),
    SendChat { channel_id: u64, text: String, image: Option<Vec<u8>> },
    SetMute { muted: bool, deafened: bool },
    SetSharing(bool),
    VideoFrame(Vec<u8>),
    Disconnect,
}

/// Ereignisse des Netzwerk-Threads an die UI.
#[derive(Debug)]
pub enum NetEvent {
    /// Login erfolgreich; enthaelt den initialen Server-State und den UDP-Socket.
    Connected {
        my_id: u64,
        users: Vec<UserInfo>,
        channels: Vec<ChannelInfo>,
        udp_socket: Option<std::net::UdpSocket>,
        udp_token: u64,
        spawn_log: Vec<LogEntry>,
        chat_history: Vec<ChatRecord>,
    },
    Message(ServerMessage),
    /// Fehler/Infos aus lokalen Hintergrund-Threads (z. B. Kamera).
    Status(String),
    Disconnected { reason: String },
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Startet den Verbindungsaufbau in einem Hintergrund-Thread.
/// Rueckgabe: Sender fuer Befehle (beim Drop wird getrennt).
pub fn connect(cfg: ClientConfig, event_tx: StdSender<NetEvent>) -> UnboundedSender<Command> {
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
                if let Err(e) = run(cfg, &mut cmd_rx, &event_tx).await {
                    let _ = event_tx.send(NetEvent::Disconnected { reason: format!("{e:#}") });
                }
            });
        })
        .expect("Netzwerk-Thread starten");
    cmd_tx
}

fn normalize_addr(addr: &str) -> String {
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
) -> anyhow::Result<()> {
    let addr = normalize_addr(&cfg.server_address);
    let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("Zeitueberschreitung beim Verbinden mit {addr}"))??;
    stream.set_nodelay(true).ok();
    let peer = stream.peer_addr()?;
    let (mut reader, mut writer) = tokio::io::split(stream);

    write_frame(
        &mut writer,
        &ClientMessage::Hello {
            username: cfg.username.clone(),
            protocol_version: PROTOCOL_VERSION,
            password: cfg.server_password.clone(),
        },
    )
    .await?;

    let first: Option<ServerMessage> = timeout(CONNECT_TIMEOUT, read_frame(&mut reader))
        .await
        .map_err(|_| anyhow::anyhow!("Zeitueberschreitung beim Warten auf Welcome"))??;
    let (client_id, udp_token, channels, users, spawn_log, chat_history) = match first {
        Some(ServerMessage::Welcome { client_id, udp_token, channels, users, spawn_log, chat_history }) => {
            (client_id, udp_token, channels, users, spawn_log, chat_history)
        }
        Some(ServerMessage::Error { message }) => anyhow::bail!("{message}"),
        other => anyhow::bail!("Unerwartete Server-Antwort: {other:?}"),
    };

    // UDP-Socket (std) fuer die Audio-Threads vorbereiten.
    let udp = std::net::UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(peer)?;
    let _ = event_tx.send(NetEvent::Connected {
        my_id: client_id,
        users,
        channels,
        udp_socket: Some(udp),
        udp_token,
        spawn_log,
        chat_history,
    });

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::Disconnect) | None => {
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
                            Command::RenameChannel(channel_id, name) => {
                                ClientMessage::RenameChannel { channel_id, name }
                            }
                            Command::SendChat { channel_id, text, image } => {
                                ClientMessage::Chat { channel_id, text, image }
                            }
                            Command::SetMute { muted, deafened } => {
                                ClientMessage::SetMuteState { muted, deafened }
                            }
                            Command::SetSharing(sharing) => ClientMessage::SetSharing { sharing },
                            Command::VideoFrame(data) => ClientMessage::VideoFrame { data },
                            Command::Disconnect => unreachable!("oben behandelt"),
                        };
                        write_frame(&mut writer, &msg).await?;
                    }
                }
            }
            msg = read_frame::<_, ServerMessage>(&mut reader) => {
                match msg {
                    Ok(Some(m)) => {
                        if event_tx.send(NetEvent::Message(m)).is_err() {
                            return Ok(()); // UI ist weg
                        }
                    }
                    Ok(None) => anyhow::bail!("Server hat die Verbindung geschlossen"),
                    Err(e) => anyhow::bail!("Netzwerkfehler: {e}"),
                }
            }
        }
    }
}
