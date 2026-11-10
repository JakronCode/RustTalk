//! TCP-Steuerkanal: Listener + ein Task pro Verbindung.

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusttalk_shared::framing::{read_frame, write_frame};
use rusttalk_shared::protocol::*;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::state::SharedState;

/// Zeitlimit für das erste `Hello` nach dem Verbindungsaufbau.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run_listener(state: SharedState, bind: &str) -> Result<()> {
    let listener = TcpListener::bind(bind).await.with_context(|| format!("TCP-Bind an {bind}"))?;
    info!("TCP-Steuerkanal lauscht auf {bind}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            match handle_connection(stream, peer, state).await {
                Ok(()) => debug!("Verbindung {peer} beendet"),
                Err(e) => debug!("Verbindung {peer} beendet: {e:#}"),
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, peer: SocketAddr, state: SharedState) -> Result<()> {
    stream.set_nodelay(true).ok();
    let (mut reader, mut writer) = tokio::io::split(stream);

    // 1. Hello mit Timeout erwarten.
    let hello: Option<ClientMessage> = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader))
        .await
        .context("Hello-Timeout")??;
    let Some(ClientMessage::Hello { username, protocol_version, password }) = hello else {
        bail!("erste Nachricht war kein Hello");
    };

    // 2. Pre-Login-Checks. Fehler werden als Error-Frame gesendet, dann beendet.
    if protocol_version != PROTOCOL_VERSION {
        let msg = format!(
            "Protokollversion nicht unterstuetzt (Client: {protocol_version}, Server: {PROTOCOL_VERSION})"
        );
        let _ = write_frame(&mut writer, &ServerMessage::Error { message: msg }).await;
        bail!("Protokollversion {protocol_version} abgelehnt ({peer})");
    }
    // MutexGuard vor dem await wieder fallen lassen (Send-Safety).
    let login_error: Option<String> = {
        let st = state.lock().unwrap();
        if let Err(msg) = st.check_password(&password) {
            Some(msg.to_string())
        } else if st.is_full() {
            Some("Server ist voll".to_string())
        } else {
            None
        }
    };
    if let Some(message) = login_error {
        let _ = write_frame(&mut writer, &ServerMessage::Error { message: message.clone() }).await;
        bail!("Login abgelehnt ({peer}): {message}");
    }

    // 3. Registrierung + Welcome an den Neuen + UserJoined an alle.
    //    Reihenfolge pro Client ist durch die mpsc-Queue garantiert
    //    (Welcome landet vor UserJoined in der Queue des Neuen).
    let (tx, rx) = mpsc::unbounded_channel::<ServerMessage>();
    let (client_id, username) = {
        let mut st = state.lock().unwrap();
        let username = sanitize_username(&username, st.next_client_id_hint());
        let (id, token) = st.add_client(username.clone(), tx);
        let welcome = ServerMessage::Welcome {
            client_id: id,
            udp_token: token,
            channels: st.all_channels(),
            users: st.all_users(),
            spawn_log: st.spawn_log(),
            chat_history: st.chat_history(),
        };
        let user = st.client(id).expect("frisch registrierter Client").info();
        st.send_to(id, &welcome);
        st.broadcast(&ServerMessage::UserJoined { user });
        // Join im Spawn-Log dokumentieren (persistiert serverseitig).
        let entry = st.push_spawn_log(format!("{username} ist erschienen"));
        st.broadcast(&ServerMessage::SpawnLogEntry { entry });
        (id, username)
    };
    info!("Client {client_id} ({username}) von {peer} verbunden");

    // 4. Writer-Task uebernimmt ab hier den Sendeteil des Streams.
    let writer_handle = tokio::spawn(writer_task(writer, rx));

    // 5. Message-Loop bis Disconnect/Fehler.
    let result = message_loop(&mut reader, &state, client_id).await;

    // 6. Cleanup: Client entfernen, alle informieren, Writer beenden.
    {
        let mut st = state.lock().unwrap();
        st.remove_client(client_id);
        st.broadcast(&ServerMessage::UserLeft { client_id });
    }
    writer_handle.abort();
    info!("Client {client_id} ({username}) getrennt");
    result
}

/// Empfaengt ServerMessages aus der Queue und schreibt sie auf den Stream.
async fn writer_task(mut writer: WriteHalf<TcpStream>, mut rx: mpsc::UnboundedReceiver<ServerMessage>) {
    while let Some(msg) = rx.recv().await {
        if write_frame(&mut writer, &msg).await.is_err() {
            break;
        }
    }
}

async fn message_loop(
    reader: &mut ReadHalf<TcpStream>,
    state: &SharedState,
    client_id: u64,
) -> Result<()> {
    loop {
        let msg: Option<ClientMessage> = read_frame(reader).await?;
        let Some(msg) = msg else { return Ok(()) };

        // Lock nur fuer die State-Aenderung halten (Sends sind synchron, kein await).
        let mut st = state.lock().unwrap();
        match msg {
            ClientMessage::Hello { .. } => {
                st.send_to(client_id, &ServerMessage::Error {
                    message: "Bereits angemeldet".to_string(),
                });
            }
            ClientMessage::Disconnect => return Ok(()),
            ClientMessage::JoinChannel { channel_id } => match st.move_client(client_id, channel_id) {
                Ok(()) => st.broadcast(&ServerMessage::UserMoved { client_id, channel_id }),
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::CreateChannel { name, kind, parent } => {
                match st.create_channel(&name, kind, parent) {
                    Ok(channel) => st.broadcast(&ServerMessage::ChannelCreated { channel }),
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
                }
            }
            ClientMessage::DeleteChannel { channel_id } => match st.delete_channel(channel_id) {
                Ok(moved) => {
                    let spawn_id = st.spawn_channel_id();
                    for uid in moved {
                        st.broadcast(&ServerMessage::UserMoved { client_id: uid, channel_id: spawn_id });
                    }
                    st.broadcast(&ServerMessage::ChannelDeleted { channel_id });
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::RenameChannel { channel_id, name } => {
                match st.rename_channel(channel_id, &name) {
                    Ok(()) => st.broadcast(&ServerMessage::ChannelRenamed { channel_id, name }),
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
                }
            }
            ClientMessage::Chat { channel_id, text, image } => {
                let img_len = image.as_ref().map(|i| i.len()).unwrap_or(0);
                if text.chars().count() > MAX_CHAT_LEN {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: format!("Nachricht zu lang (max. {MAX_CHAT_LEN} Zeichen)"),
                    });
                } else if text.trim().is_empty() && image.is_none() {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: "Leere Nachricht".to_string(),
                    });
                } else if img_len > MAX_MEDIA_BYTES {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: format!("Bild zu gross (max. {} MiB)", MAX_MEDIA_BYTES / 1024 / 1024),
                    });
                } else if let Err(e) = st.chat_allowed(channel_id) {
                    st.send_to(client_id, &ServerMessage::Error { message: e });
                } else if let Some(from_name) = st.client(client_id).map(|c| c.name.clone()) {
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    // Persistenz (no-op, wenn deaktiviert) + globaler Broadcast.
                    st.persist_message(channel_id, &from_name, &text, timestamp, image.as_deref());
                    st.broadcast(
                        &ServerMessage::Chat { channel_id, from_id: client_id, from_name, text, timestamp, image },
                    );
                }
            }
            ClientMessage::SetMuteState { muted, deafened } => {
                st.set_mute_state(client_id, muted, deafened);
                st.broadcast(&ServerMessage::MuteStateChanged { client_id, muted, deafened });
            }
            ClientMessage::SetSharing { sharing } => {
                if st.set_sharing(client_id, sharing) {
                    st.broadcast(&ServerMessage::SharingChanged { client_id, sharing });
                } else {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: "Teilen nur in Sprach-Channels moeglich".to_string(),
                    });
                }
            }
            ClientMessage::VideoFrame { data } => {
                if data.len() > MAX_MEDIA_BYTES {
                    // Uebergrosse Frames verwerfen (kein Fehler-Spam pro Frame).
                    continue;
                }
                let targets = st.video_targets(client_id);
                if !targets.is_empty() {
                    let msg = ServerMessage::VideoFrame { from_id: client_id, data };
                    for tx in targets {
                        let _ = tx.send(msg.clone());
                    }
                }
            }
        }
    }
}

fn sanitize_username(name: &str, fallback_id: u64) -> String {
    let trimmed = name.trim();
    let mut s: String = trimmed.chars().take(MAX_NAME_LEN).collect();
    if s.is_empty() {
        s = format!("User{fallback_id}");
    }
    s
}
