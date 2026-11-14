//! TCP-Steuerkanal: Listener + ein Task pro Verbindung.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusttalk_shared::framing::{read_frame, write_frame};
use rusttalk_shared::protocol::*;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::state::{SharedState, MAX_VIDEO_BACKLOG};

/// Zeitlimit für das erste `Hello` nach dem Verbindungsaufbau.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Haengt ein einzelner Schreibvorgang laenger, gilt die Verbindung als tot.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// Tote Verbindungen erkennen, auch wenn der Client sich nicht abmeldet
/// (Absturz, WLAN weg, Firewall verwirft Pakete still):
/// - Keepalive: nach 15 s Stille alle 5 s pruefen.
/// - Linux `TCP_USER_TIMEOUT`: 20 s unbestaetigte Daten -> Verbindung zu
///   (ohne das versucht der Kernel ~15 min lang neu zu senden).
fn configure_socket(stream: &TcpStream) {
    let sock = socket2::SockRef::from(stream);
    let ka = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(15))
        .with_interval(Duration::from_secs(5));
    #[cfg(target_os = "linux")]
    let ka = ka.with_retries(3);
    if let Err(e) = sock.set_tcp_keepalive(&ka) {
        debug!("Could not set keepalive: {e}");
    }
    #[cfg(target_os = "linux")]
    if let Err(e) = sock.set_tcp_user_timeout(Some(Duration::from_secs(20))) {
        debug!("Could not set TCP_USER_TIMEOUT: {e}");
    }
}

/// The TLS stream of one client.
type Conn = tokio_rustls::server::TlsStream<TcpStream>;

pub async fn run_listener(state: SharedState, bind: &str, tls: Arc<rustls::ServerConfig>) -> Result<()> {
    let listener = TcpListener::bind(bind).await.with_context(|| format!("Could not bind TCP to {bind}"))?;
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    info!("TCP control channel (TLS) listening on {bind}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            stream.set_nodelay(true).ok();
            configure_socket(&stream);
            let stream = match tokio::time::timeout(HELLO_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return debug!("TLS handshake with {peer} failed: {e}"),
                Err(_) => return debug!("TLS handshake with {peer}: timeout"),
            };
            match handle_connection(stream, peer, state).await {
                Ok(()) => debug!("Connection {peer} closed"),
                Err(e) => debug!("Connection {peer} closed: {e:#}"),
            }
        });
    }
}

async fn handle_connection(stream: Conn, peer: SocketAddr, state: SharedState) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);

    // 1. Hello mit Timeout erwarten.
    let hello: Option<ClientMessage> = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader))
        .await
        .context("Hello timeout")??;
    let Some(ClientMessage::Hello { username, protocol_version, password, identity, invite }) = hello else {
        bail!("first message was not Hello");
    };
    let ip = peer.ip().to_string();

    // 2. Pre-Login-Checks. Fehler werden als Error-Frame gesendet, dann beendet.
    if protocol_version != PROTOCOL_VERSION {
        let msg = format!(
            "Protocol version not supported (client: {protocol_version}, server: {PROTOCOL_VERSION}) - please update RustTalk"
        );
        let _ = write_frame(&mut writer, &ServerMessage::Error { message: msg }).await;
        bail!("Protocol version {protocol_version} refused ({peer})");
    }
    let Some(uid) = crate::moderation::uid_from_identity(&identity) else {
        let msg = "Invalid identity - please update RustTalk".to_string();
        let _ = write_frame(&mut writer, &ServerMessage::Error { message: msg }).await;
        bail!("Invalid identity ({peer})");
    };
    // MutexGuard vor dem await wieder fallen lassen (Send-Safety).
    let login_error: Option<String> = {
        let mut st = state.lock().unwrap();
        if let Err(msg) = st.check_login(&uid, Some(&ip), &password, invite.as_deref()) {
            Some(msg)
        } else if st.is_full() {
            Some("Server is full".to_string())
        } else {
            None
        }
    };
    if let Some(message) = login_error {
        let _ = write_frame(&mut writer, &ServerMessage::Error { message: message.clone() }).await;
        bail!("Login refused ({peer}): {message}");
    }

    // 3. Registrierung + Welcome an den Neuen + UserJoined an alle.
    //    Reihenfolge pro Client ist durch die mpsc-Queue garantiert
    //    (Welcome landet vor UserJoined in der Queue des Neuen).
    let (tx, rx) = mpsc::unbounded_channel::<ServerMessage>();
    let (client_id, username, video_backlog) = {
        let mut st = state.lock().unwrap();
        let username = sanitize_username(&username, st.next_client_id_hint());
        let (id, token) = st.add_client_identified(username.clone(), tx, uid, Some(ip));
        let welcome = ServerMessage::Welcome {
            settings: st.settings().clone(),
            client_id: id,
            udp_token: token,
            channels: st.all_channels(),
            users: st.all_users(),
            spawn_log: st.spawn_log(),
            chat_history: st.chat_history(),
            gif_search: st.gif_available(),
            open_permissions: st.permissions_open(),
            udp_key: Vec::new(), // filled below
        };
        let client = st.client(id).expect("frisch registrierter Client");
        let user = client.info();
        let video_backlog = client.video_backlog.clone();
        let mut welcome = welcome;
        if let ServerMessage::Welcome { udp_key, .. } = &mut welcome {
            *udp_key = st.client(id).map(|c| c.udp_key.clone()).unwrap_or_default();
        }
        st.send_to(id, &welcome);
        st.broadcast(&ServerMessage::UserJoined { user });
        // Join im Spawn-Log dokumentieren (persistiert serverseitig).
        let entry = st.push_spawn_log(format!("{username} joined the server"));
        st.broadcast(&ServerMessage::SpawnLogEntry { entry });
        let join_template = st.settings().join_message.clone();
        if let Some(msg) = st.announcement(&join_template, &username) {
            st.broadcast(&msg);
        }
        // Message of the day: only to the new client.
        let motd = st.settings().motd.trim().to_string();
        if !motd.is_empty() {
            st.send_to(id, &ServerMessage::Notice { message: motd });
        }
        if let Some(notice) = st.update_notice_for(id) {
            st.send_to(id, &notice);
        }
        (id, username, video_backlog)
    };
    info!("Client {client_id} ({username}) connected from {peer}");

    // 4. Writer-Task uebernimmt ab hier den Sendeteil des Streams.
    let mut writer_handle = tokio::spawn(writer_task(writer, rx, video_backlog));

    // 5. Message-Loop bis Disconnect/Fehler – oder bis das Senden scheitert
    //    (sonst wuerde der Server ewig auf einen toten Client warten).
    let result = tokio::select! {
        r = message_loop(&mut reader, &state, client_id) => r,
        _ = &mut writer_handle => Err(anyhow::anyhow!("Sending to client failed")),
    };

    // 6. Cleanup: Client entfernen, alle informieren, Writer beenden.
    {
        let mut st = state.lock().unwrap();
        st.remove_client(client_id);
        st.broadcast(&ServerMessage::UserLeft { client_id });
        let leave_template = st.settings().leave_message.clone();
        if let Some(msg) = st.announcement(&leave_template, &username) {
            st.broadcast(&msg);
        }
    }
    writer_handle.abort();
    match &result {
        Ok(()) => info!("Client {client_id} ({username}) disconnected"),
        Err(e) => info!("Client {client_id} ({username}) disconnected: {e:#}"),
    }
    result
}

/// Empfaengt ServerMessages aus der Queue und schreibt sie auf den Stream.
/// Endet bei Schreibfehler oder wenn ein Frame laenger als `WRITE_TIMEOUT`
/// haengt. Gibt nach jedem Video-Frame dessen Bytes im Backlog frei.
async fn writer_task(
    mut writer: WriteHalf<Conn>,
    mut rx: mpsc::UnboundedReceiver<ServerMessage>,
    video_backlog: Arc<AtomicUsize>,
) {
    while let Some(msg) = rx.recv().await {
        let video_len = match &msg {
            ServerMessage::VideoFrame { data, .. } => data.len(),
            _ => 0,
        };
        let res = tokio::time::timeout(WRITE_TIMEOUT, write_frame(&mut writer, &msg)).await;
        if video_len > 0 {
            video_backlog.fetch_sub(video_len, Ordering::Relaxed);
        }
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug!("Write failed: {e}");
                break;
            }
            Err(_) => {
                debug!("Write stuck for {WRITE_TIMEOUT:?}");
                break;
            }
        }
    }
}

async fn message_loop(
    reader: &mut ReadHalf<Conn>,
    state: &SharedState,
    client_id: u64,
) -> Result<()> {
    loop {
        let msg: Option<ClientMessage> = read_frame(reader).await?;
        let Some(msg) = msg else { return Ok(()) };

        // GIF requests talk to GIPHY (HTTP): in their own task, never under the lock.
        let msg = match msg {
            ClientMessage::GifSearch { query, offset } => {
                spawn_gif_search(state.clone(), client_id, query, offset);
                continue;
            }
            ClientMessage::SetGifKey { key } => {
                spawn_set_gif_key(state.clone(), client_id, key);
                continue;
            }
            other => other,
        };

        // Lock nur fuer die State-Aenderung halten (Sends sind synchron, kein await).
        let mut st = state.lock().unwrap();
        if let Some(need) = required_role(&msg) {
            if !st.can(client_id, need) {
                st.send_to(client_id, &ServerMessage::Error { message: format!("You need the {} role for that", need.label()) });
                continue;
            }
        }
        match msg {
            // Handled above (outside the lock).
            ClientMessage::GifSearch { .. } | ClientMessage::SetGifKey { .. } => {}
            ClientMessage::ClaimAdmin { password } => match st.claim_admin(client_id, password.as_deref()) {
                Ok(change) => broadcast_roles(&st, change),
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::SetRole { client_id: target, role } => match st.set_role_of_client(client_id, target, role) {
                Ok(change) => broadcast_roles(&st, change),
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::SetRoleByUid { uid, role } => match st.set_role_of_uid(client_id, &uid, role) {
                Ok(change) => {
                    broadcast_roles(&st, change);
                    let data = st.admin_data();
                    st.send_to(client_id, &data);
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::MoveUser { client_id: target, channel_id } => match st.move_user(client_id, target, channel_id) {
                Ok((_, channel)) => {
                    st.broadcast(&ServerMessage::UserMoved { client_id: target, channel_id });
                    let by = st.client(client_id).map(|c| c.name.clone()).unwrap_or_default();
                    st.send_to(target, &ServerMessage::Notice { message: format!("{by} moved you to {channel}") });
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::Kick { client_id: target, reason } => {
                if let Err(e) = st.kick(client_id, target, &reason) {
                    st.send_to(client_id, &ServerMessage::Error { message: e });
                }
            }
            ClientMessage::Ban { client_id: target, reason, minutes } => {
                if let Err(e) = st.ban(client_id, target, &reason, minutes) {
                    st.send_to(client_id, &ServerMessage::Error { message: e });
                }
            }
            ClientMessage::Unban { ban_id } => match st.unban(client_id, ban_id) {
                Ok(()) => {
                    let data = st.admin_data();
                    st.send_to(client_id, &data);
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::CreateInvite { max_uses, hours } => {
                let invite = st.create_invite(client_id, max_uses, hours);
                st.send_to(client_id, &ServerMessage::InviteCreated { invite });
            }
            ClientMessage::DeleteInvite { code } => match st.delete_invite(client_id, &code) {
                Ok(()) => {
                    let data = st.admin_data();
                    st.send_to(client_id, &data);
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::RequestAdminData => {
                let data = st.admin_data();
                st.send_to(client_id, &data);
            }
            ClientMessage::Hello { .. } => {
                st.send_to(client_id, &ServerMessage::Error {
                    message: "Already logged in".to_string(),
                });
            }
            ClientMessage::Disconnect => return Ok(()),
            ClientMessage::JoinChannel { channel_id } => match st.move_client(client_id, channel_id) {
                Ok(()) => st.broadcast(&ServerMessage::UserMoved { client_id, channel_id }),
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::CreateChannel { name, kind, parent } => {
                match st.create_channel(&name, kind, parent) {
                    Ok(channel) => {
                        st.audit(client_id, format!("created channel {}", channel.name));
                        st.broadcast(&ServerMessage::ChannelCreated { channel });
                    }
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
                }
            }
            ClientMessage::DeleteChannel { channel_id } => match st.delete_channel(channel_id) {
                Ok(moved) => {
                    st.audit(client_id, format!("deleted channel #{channel_id}"));
                    let spawn_id = st.spawn_channel_id();
                    for uid in moved {
                        st.broadcast(&ServerMessage::UserMoved { client_id: uid, channel_id: spawn_id });
                    }
                    st.broadcast(&ServerMessage::ChannelDeleted { channel_id });
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::MoveChannel { channel_id, parent, index } => {
                match st.move_channel(channel_id, parent, index) {
                    Ok(placements) => st.broadcast(&ServerMessage::ChannelsReordered { placements }),
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
                }
            }
            ClientMessage::UpdateServerSettings { settings } => match st.update_settings(settings) {
                Ok(settings) => {
                    st.audit(client_id, "changed the server settings".to_string());
                    st.broadcast(&ServerMessage::ServerSettingsChanged { settings });
                }
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::RenameChannel { channel_id, name } => {
                match st.rename_channel(channel_id, &name) {
                    Ok(()) => {
                        st.audit(client_id, format!("renamed a channel to {name}"));
                        st.broadcast(&ServerMessage::ChannelRenamed { channel_id, name });
                    }
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
                }
            }
            ClientMessage::Chat { channel_id, text, image, reply_to } => {
                let img_len = image.as_ref().map(|i| i.len()).unwrap_or(0);
                if text.chars().count() > MAX_CHAT_LEN {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: format!("Message too long (max. {MAX_CHAT_LEN} characters)"),
                    });
                } else if text.trim().is_empty() && image.is_none() {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: "Empty message".to_string(),
                    });
                } else if img_len > MAX_MEDIA_BYTES {
                    st.send_to(client_id, &ServerMessage::Error {
                        message: format!("Image too large (max. {} MiB)", MAX_MEDIA_BYTES / 1024 / 1024),
                    });
                } else if let Err(e) = st.chat_allowed(channel_id) {
                    st.send_to(client_id, &ServerMessage::Error { message: e });
                } else if let Some(from_name) = st.client(client_id).map(|c| c.name.clone()) {
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    // Speichern (DB oder Memory) + globaler Broadcast.
                    let reply_to = st.valid_reply(channel_id, reply_to);
                    let id = st.post_message(channel_id, &from_name, &text, timestamp, image.as_deref(), reply_to);
                    st.broadcast(&ServerMessage::Chat {
                        id,
                        channel_id,
                        from_id: client_id,
                        from_name,
                        text,
                        timestamp,
                        image,
                        reply_to,
                    });
                }
            }
            ClientMessage::EditChat { message_id, text } => {
                let name = st.client(client_id).map(|c| c.name.clone()).unwrap_or_default();
                match st.edit_message(&name, message_id, &text) {
                    Ok((channel_id, text)) => st.broadcast(&ServerMessage::ChatEdited { channel_id, message_id, text }),
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
                }
            }
            ClientMessage::DeleteChat { message_id } => {
                let name = st.client(client_id).map(|c| c.name.clone()).unwrap_or_default();
                let any = st.can(client_id, Role::Moderator);
                match st.delete_message(&name, message_id, any) {
                    Ok(channel_id) => st.broadcast(&ServerMessage::ChatDeleted { channel_id, message_id }),
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
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
                        message: "Sharing is only possible in voice channels".to_string(),
                    });
                }
            }
            ClientMessage::SetAudioStreams { streams } => {
                if let Err(message) = st.set_audio_streams(client_id, streams) { st.send_to(client_id, &ServerMessage::Error { message }); }
            }
            ClientMessage::SetAudioSharing { sharing } => match st.set_audio_sharing(client_id, sharing) {
                Ok(true) => st.broadcast(&ServerMessage::AudioSharingChanged { client_id, sharing }),
                Ok(false) => {}
                Err(message) => st.send_to(client_id, &ServerMessage::Error { message }),
            },
            ClientMessage::VideoFrame { kind, data } => {
                if data.len() > MAX_MEDIA_BYTES {
                    // Uebergrosse Frames verwerfen (kein Fehler-Spam pro Frame).
                    continue;
                }
                // Only to viewers who watch this stream (opt-in, saves bandwidth).
                let targets = st.stream_frame(client_id, kind);
                if !targets.is_empty() {
                    let len = data.len();
                    let msg = ServerMessage::VideoFrame { from_id: client_id, kind, data };
                    for (tx, backlog) in targets {
                        // Zuschauer haengt hinterher -> diesen Frame auslassen.
                        if backlog.load(Ordering::Relaxed) + len > MAX_VIDEO_BACKLOG {
                            continue;
                        }
                        backlog.fetch_add(len, Ordering::Relaxed);
                        if tx.send(msg.clone()).is_err() {
                            backlog.fetch_sub(len, Ordering::Relaxed);
                        }
                    }
                }
            }
            ClientMessage::RequestKeyframe { from_id, kind } => {
                // Only between people in the same channel, only to someone sharing.
                let same = st.client(from_id).zip(st.client(client_id)).is_some_and(|(s, v)| s.channel_id == v.channel_id && s.sharing);
                if same {
                    st.send_to(from_id, &ServerMessage::KeyframeRequest { kind });
                }
            }
            ClientMessage::StopVideo { kind } => st.stop_stream(client_id, kind),
            ClientMessage::Watch { from_id, kind, watching } => match st.set_watching(client_id, from_id, kind, watching) {
                // New viewer: the sender sends a full picture right away.
                Ok(true) => st.send_to(from_id, &ServerMessage::KeyframeRequest { kind }),
                Ok(false) => {}
                Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
            },
            ClientMessage::Poke { target, message } => {
                match st.poke(client_id, target, &message, std::time::Instant::now()) {
                    Ok(poked) => st.send_to(target, &poked),
                    Err(e) => st.send_to(client_id, &ServerMessage::Error { message: e }),
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

/// GIF search: cached page right away, otherwise ask GIPHY in a blocking task.
fn spawn_gif_search(state: SharedState, client_id: u64, query: String, offset: u32) {
    let query: String = query.trim().chars().take(MAX_GIF_QUERY_LEN).collect();
    let offset = offset.min(crate::gif::MAX_OFFSET);
    let reply = move |results: Vec<GifInfo>, error: Option<String>, query: &str| ServerMessage::GifResults {
        query: query.to_string(),
        offset,
        results,
        error,
    };
    let key = {
        let st = state.lock().unwrap();
        if let Some(results) = st.gif_cached(&query, offset) {
            st.send_to(client_id, &reply(results, None, &query));
            return;
        }
        st.gif_key()
    };
    let Some(key) = key else {
        let st = state.lock().unwrap();
        let msg = "GIF search is not set up on this server (Server Settings > Integrations)".to_string();
        st.send_to(client_id, &reply(Vec::new(), Some(msg), &query));
        return;
    };
    tokio::spawn(async move {
        let q = query.clone();
        let res = tokio::task::spawn_blocking(move || crate::gif::fetch(&key, &q, offset))
            .await
            .unwrap_or_else(|e| Err(format!("GIF search failed: {e}")));
        let mut st = state.lock().unwrap();
        match res {
            Ok(results) => {
                st.gif_store(&query, offset, results.clone());
                st.send_to(client_id, &reply(results, None, &query));
            }
            Err(e) => {
                tracing::warn!("GIF search failed: {e}");
                st.send_to(client_id, &reply(Vec::new(), Some(e), &query));
            }
        }
    });
}

/// Sets or removes the GIPHY key. A new key is tested with one request first.
fn spawn_set_gif_key(state: SharedState, client_id: u64, key: Option<String>) {
    {
        let st = state.lock().unwrap();
        if !st.can(client_id, Role::Admin) {
            st.send_to(client_id, &ServerMessage::Error { message: "You need the Admin role for that".to_string() });
            return;
        }
    }
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let Some(key) = key else {
        let mut st = state.lock().unwrap();
        st.set_gif_key(None);
        st.audit(client_id, "removed the GIPHY key".to_string());
        info!("GIF search disabled (client {client_id})");
        st.broadcast(&ServerMessage::GifSearchChanged { available: false });
        return;
    };
    if key.len() > 100 || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        let st = state.lock().unwrap();
        st.send_to(client_id, &ServerMessage::Error { message: "That does not look like a GIPHY API key".to_string() });
        return;
    }
    tokio::spawn(async move {
        let k = key.clone();
        let res = tokio::task::spawn_blocking(move || crate::gif::fetch(&k, "", 0))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let mut st = state.lock().unwrap();
        match res {
            Ok(trending) => {
                st.set_gif_key(Some(key));
                st.gif_store("", 0, trending);
                st.audit(client_id, "set the GIPHY key".to_string());
                info!("GIF search enabled (client {client_id})");
                st.broadcast(&ServerMessage::GifSearchChanged { available: true });
                st.send_to(client_id, &ServerMessage::Notice { message: "GIF search is now active for everyone".to_string() });
            }
            Err(e) => st.send_to(client_id, &ServerMessage::Error { message: format!("Key not saved: {e}") }),
        }
    });
}

/// Role a message needs (None = everyone).
fn required_role(msg: &ClientMessage) -> Option<Role> {
    use ClientMessage as M;
    match msg {
        M::CreateChannel { .. }
        | M::DeleteChannel { .. }
        | M::MoveChannel { .. }
        | M::RenameChannel { .. }
        | M::UpdateServerSettings { .. }
        | M::SetRole { .. }
        | M::SetRoleByUid { .. }
        | M::Ban { .. }
        | M::Unban { .. }
        | M::CreateInvite { .. }
        | M::DeleteInvite { .. }
        | M::RequestAdminData => Some(Role::Admin),
        M::MoveUser { .. } | M::Kick { .. } => Some(Role::Moderator),
        _ => None,
    }
}

/// Tells everyone about changed roles (and the open/closed switch).
fn broadcast_roles(st: &crate::state::State, (changed, open): (Vec<(u64, Role)>, Option<bool>)) {
    for (client_id, role) in changed {
        st.broadcast(&ServerMessage::UserRoleChanged { client_id, role });
    }
    if let Some(open) = open {
        st.broadcast(&ServerMessage::PermissionsChanged { open });
    }
}
