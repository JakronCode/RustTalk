//! Netzwerk-Thread des Clients: TCP-Steuerkanal (tokio) + UDP-Socket-Uebergabe.
//!
//! Die UI kommuniziert ueber [`Command`] (raus) und [`NetEvent`] (rein).
//! Der UDP-Socket wird nach dem Login an die Audio-Engine uebergeben.
//! Both queues are bounded by count and bytes. Dropping the [`Connection`]
//! ends the network thread at once, whatever it is waiting for.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rusttalk_shared::framing::{frame_len, read_frame_sized, write_frame};
use rusttalk_shared::protocol::*;
use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio::time::timeout;

use crate::config::ClientConfig;
use rusttalk_shared::secure;

/// Commands waiting for the network thread of one connection.
pub const COMMAND_QUEUE: usize = 512;
/// Their payload bytes (chat images, video, settings).
pub const COMMAND_QUEUE_BYTES: usize = 16 * 1024 * 1024;
/// Events waiting for the UI per connection. Further video frames are
/// dropped; other messages wait, so the server's TCP stream backs up.
pub const EVENT_QUEUE: usize = 256;
/// Their payload bytes; at least one frame of `MAX_FRAME_LEN` fits.
pub const EVENT_QUEUE_BYTES: usize = 24 * 1024 * 1024;
/// Counted for every queued item on top of its payload.
const ITEM_OVERHEAD: usize = 64;
/// A frame that is not written within this (plus `WRITE_MIN_RATE`) ends the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 15 });
/// Slowest upload in bytes per second before a frame counts as stalled.
const WRITE_MIN_RATE: usize = 32 * 1024;
/// Goodbye to the server on a user disconnect, never waited for longer.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(250);

/// Befehle der UI an den Netzwerk-Thread.
#[derive(Debug)]
pub enum Command {
    JoinChannel(u64),
    CreateChannel {
        name: String,
        kind: ChannelKind,
        parent: Option<u64>,
    },
    DeleteChannel(u64),
    RenameChannel(u64, String),
    MoveChannel {
        channel_id: u64,
        parent: Option<u64>,
        index: u32,
    },
    UpdateServerSettings(ServerSettings),
    SendChat {
        channel_id: u64,
        text: String,
        image: Option<Vec<u8>>,
        reply_to: Option<u64>,
    },
    EditChat {
        message_id: u64,
        text: String,
    },
    DeleteChat(u64),
    /// GIF search through the server (empty query = trending).
    GifSearch {
        query: String,
        offset: u32,
    },
    /// Set / remove the server's GIPHY key.
    SetGifKey(Option<String>),
    /// Any other client message (moderation, roles, invites).
    Raw(ClientMessage),
    SetMute {
        muted: bool,
        deafened: bool,
    },
    SetSharing(bool),
    SetAudioSharing(bool),
    SetAudioStreams(Vec<rusttalk_shared::protocol::AudioStreamInfo>),
    VideoFrame(VideoKind, Vec<u8>),
    StopVideo(VideoKind),
}

impl Command {
    fn into_message(self) -> ClientMessage {
        match self {
            Command::JoinChannel(channel_id) => ClientMessage::JoinChannel { channel_id },
            Command::CreateChannel { name, kind, parent } => {
                ClientMessage::CreateChannel { name, kind, parent }
            }
            Command::DeleteChannel(channel_id) => ClientMessage::DeleteChannel { channel_id },
            Command::UpdateServerSettings(settings) => {
                ClientMessage::UpdateServerSettings { settings }
            }
            Command::MoveChannel {
                channel_id,
                parent,
                index,
            } => ClientMessage::MoveChannel {
                channel_id,
                parent,
                index,
            },
            Command::RenameChannel(channel_id, name) => {
                ClientMessage::RenameChannel { channel_id, name }
            }
            Command::SendChat {
                channel_id,
                text,
                image,
                reply_to,
            } => ClientMessage::Chat {
                channel_id,
                text,
                image,
                reply_to,
            },
            Command::EditChat { message_id, text } => ClientMessage::EditChat { message_id, text },
            Command::DeleteChat(message_id) => ClientMessage::DeleteChat { message_id },
            Command::GifSearch { query, offset } => ClientMessage::GifSearch { query, offset },
            Command::SetGifKey(key) => ClientMessage::SetGifKey { key },
            Command::Raw(msg) => msg,
            Command::SetMute { muted, deafened } => ClientMessage::SetMuteState { muted, deafened },
            Command::SetSharing(sharing) => ClientMessage::SetSharing { sharing },
            Command::SetAudioStreams(streams) => ClientMessage::SetAudioStreams { streams },
            Command::SetAudioSharing(sharing) => ClientMessage::SetAudioSharing { sharing },
            Command::VideoFrame(kind, data) => ClientMessage::VideoFrame { kind, data },
            Command::StopVideo(kind) => ClientMessage::StopVideo { kind },
        }
    }

    /// Bytes this command holds while it waits in the queue.
    fn queued_len(&self) -> usize {
        match self {
            Command::SendChat { text, image, .. } => {
                text.len() + image.as_ref().map_or(0, Vec::len)
            }
            Command::VideoFrame(_, data) => data.len(),
            Command::EditChat { text: s, .. }
            | Command::CreateChannel { name: s, .. }
            | Command::RenameChannel(_, s) => s.len(),
            Command::GifSearch { query, .. } => query.len(),
            Command::SetGifKey(key) => key.as_ref().map_or(0, String::len),
            Command::UpdateServerSettings(settings) => {
                frame_len(settings).unwrap_or(COMMAND_QUEUE_BYTES)
            }
            Command::Raw(msg) => frame_len(msg).unwrap_or(COMMAND_QUEUE_BYTES),
            _ => 0,
        }
    }
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
    Disconnected {
        reason: String,
    },
    /// The user must decide about the server's certificate: unknown address
    /// (nothing was sent yet) or a pinned fingerprint that no longer matches.
    /// Only raised by our own TLS check, never from server-supplied text.
    CertCheck {
        addr: String,
        fingerprint: String,
        changed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The queue is at its count or byte limit.
    Full,
    /// The connection is gone.
    Closed,
}

/// An item and the queue bytes it holds until it is taken out.
struct Queued<T> {
    item: T,
    _bytes: OwnedSemaphorePermit,
}

fn permits(len: usize, budget: usize) -> Result<u32, SendError> {
    let len = len.checked_add(ITEM_OVERHEAD).ok_or(SendError::Full)?;
    if len > budget {
        return Err(SendError::Full);
    }
    u32::try_from(len).map_err(|_| SendError::Full)
}

fn take_bytes(
    bytes: &Arc<Semaphore>,
    len: usize,
    budget: usize,
) -> Result<OwnedSemaphorePermit, SendError> {
    bytes
        .clone()
        .try_acquire_many_owned(permits(len, budget)?)
        .map_err(|e| match e {
            TryAcquireError::NoPermits => SendError::Full,
            TryAcquireError::Closed => SendError::Closed,
        })
}

fn try_queue<T>(
    tx: &mpsc::Sender<Queued<T>>,
    item: T,
    bytes: OwnedSemaphorePermit,
) -> Result<(), SendError> {
    tx.try_send(Queued {
        item,
        _bytes: bytes,
    })
    .map_err(|e| match e {
        mpsc::error::TrySendError::Full(_) => SendError::Full,
        mpsc::error::TrySendError::Closed(_) => SendError::Closed,
    })
}

/// Sends commands to one connection without ever blocking the caller.
#[derive(Clone)]
pub struct CommandSender {
    pub traffic: Arc<crate::bandwidth::Traffic>,
    tx: mpsc::Sender<Queued<Command>>,
    bytes: Arc<Semaphore>,
    /// A command (not video) was refused because the queue was full.
    refused: Arc<AtomicBool>,
}

impl CommandSender {
    fn new() -> (Self, mpsc::Receiver<Queued<Command>>) {
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
        let bytes = Arc::new(Semaphore::new(COMMAND_QUEUE_BYTES));
        (
            Self {
                tx,
                traffic: Default::default(),
                bytes,
                refused: Default::default(),
            },
            rx,
        )
    }

    /// Queues `cmd`. A full queue refuses it (the connection is stalled):
    /// nothing is sent, the caller and `Connection::take_refused` learn it.
    pub fn send(&self, cmd: Command) -> Result<(), SendError> {
        if self.tx.is_closed() {
            return Err(SendError::Closed);
        }
        let video = matches!(cmd, Command::VideoFrame(..));
        let res = take_bytes(&self.bytes, cmd.queued_len(), COMMAND_QUEUE_BYTES)
            .and_then(|b| try_queue(&self.tx, cmd, b));
        if res == Err(SendError::Full) && !video {
            self.refused.store(true, Ordering::Relaxed);
        }
        res
    }
}

/// The UI's handle on one connection. Dropping it ends the connection at
/// once (connecting, handshake, a stalled write): nothing waits for a queue.
pub struct Connection {
    tx: CommandSender,
    _cancel: watch::Sender<()>,
}

impl std::ops::Deref for Connection {
    type Target = CommandSender;
    fn deref(&self) -> &CommandSender {
        &self.tx
    }
}

impl Connection {
    /// A sender for code that outlives this borrow (capture threads, dialogs).
    pub fn sender(&self) -> CommandSender {
        self.tx.clone()
    }

    /// Some command was refused since the last call.
    pub fn take_refused(&self) -> bool {
        self.tx.refused.swap(false, Ordering::Relaxed)
    }

    /// A connection without a network thread: tests read its commands.
    #[cfg(test)]
    pub fn test_pair() -> (Self, TestCommands) {
        let (tx, rx) = CommandSender::new();
        let (cancel, cancelled) = watch::channel(());
        (
            Self {
                tx,
                _cancel: cancel,
            },
            TestCommands { rx, cancelled },
        )
    }
}

#[cfg(test)]
pub struct TestCommands {
    rx: mpsc::Receiver<Queued<Command>>,
    cancelled: watch::Receiver<()>,
}

#[cfg(test)]
impl TestCommands {
    pub fn try_recv(&mut self) -> Option<Command> {
        self.rx.try_recv().ok().map(|q| q.item)
    }

    /// The `Connection` was dropped (the network thread would have ended).
    pub fn cancelled(&self) -> bool {
        self.cancelled.has_changed().is_err()
    }
}

/// Network events of one connection on their way to the UI.
#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<Queued<NetEvent>>,
    bytes: Arc<Semaphore>,
}

pub struct EventReceiver {
    rx: mpsc::Receiver<Queued<NetEvent>>,
    bytes: Arc<Semaphore>,
}

/// A bounded event queue (`EVENT_QUEUE` items, `EVENT_QUEUE_BYTES`).
pub fn event_queue() -> (EventSender, EventReceiver) {
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    let bytes = Arc::new(Semaphore::new(EVENT_QUEUE_BYTES));
    (
        EventSender {
            tx,
            bytes: bytes.clone(),
        },
        EventReceiver { rx, bytes },
    )
}

impl EventSender {
    /// Waits (asynchronously) until the event and its `len` bytes fit.
    pub async fn send(&self, event: NetEvent, len: usize) -> Result<(), SendError> {
        let bytes = self
            .bytes
            .clone()
            .acquire_many_owned(permits(len, EVENT_QUEUE_BYTES)?)
            .await
            .map_err(|_| SendError::Closed)?;
        self.tx
            .send(Queued {
                item: event,
                _bytes: bytes,
            })
            .await
            .map_err(|_| SendError::Closed)
    }

    /// Never waits: a full queue refuses the event (video, local errors).
    pub fn try_send(&self, event: NetEvent, len: usize) -> Result<(), SendError> {
        take_bytes(&self.bytes, len, EVENT_QUEUE_BYTES).and_then(|b| try_queue(&self.tx, event, b))
    }
}

impl EventReceiver {
    /// The next event; its queue bytes are free again once it is taken.
    pub fn try_recv(&mut self) -> Result<NetEvent, mpsc::error::TryRecvError> {
        self.rx.try_recv().map(|q| q.item)
    }

    /// Bytes waiting in the queue.
    #[cfg(test)]
    pub fn queued_bytes(&self) -> usize {
        EVENT_QUEUE_BYTES - self.bytes.available_permits()
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        self.bytes.close(); // a sender waiting for room gives up at once
    }
}

/// Typed so a server error message can never pose as a certificate decision.
#[derive(Debug)]
struct CertDecision {
    addr: String,
    fingerprint: String,
    changed: bool,
}

impl std::fmt::Display for CertDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = if self.changed {
            secure::CERT_CHANGED
        } else {
            "UNKNOWN CERTIFICATE"
        };
        write!(f, "{what}: {}", self.fingerprint)
    }
}

impl std::error::Error for CertDecision {}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Startet den Verbindungsaufbau in einem Hintergrund-Thread.
/// `ctx` wird nach jeder eingehenden Nachricht zum Neuzeichnen angestossen
/// (sonst erscheinen Video-Frames erst beim naechsten 100-ms-Tick).
/// `upload_backlog`: Video-Bytes in der Sende-Queue; der Writer zieht nach
/// jedem gesendeten Frame ab (die UI verwirft neue Frames, wenn es staut).
pub fn connect(
    cfg: ClientConfig,
    events: EventSender,
    ctx: eframe::egui::Context,
    upload_backlog: Arc<AtomicUsize>,
) -> Connection {
    let (tx, commands) = CommandSender::new();
    let traffic = tx.traffic.clone();
    let (cancel, mut cancelled_rx) = watch::channel(());
    std::thread::Builder::new()
        .name("net".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                if let Err(e) = run(
                    cfg,
                    commands,
                    &events,
                    &ctx,
                    &upload_backlog,
                    &mut cancelled_rx,
                    traffic,
                )
                .await
                {
                    let event = match e.downcast::<CertDecision>() {
                        Ok(CertDecision {
                            addr,
                            fingerprint,
                            changed,
                        }) => NetEvent::CertCheck {
                            addr,
                            fingerprint,
                            changed,
                        },
                        Err(e) => NetEvent::Disconnected {
                            reason: format!("{e:#}"),
                        },
                    };
                    tokio::select! {
                        _ = events.send(event, 0) => ctx.request_repaint(),
                        _ = cancelled(&mut cancelled_rx) => {}
                    }
                }
            });
        })
        .expect("Netzwerk-Thread starten");
    Connection {
        tx,
        _cancel: cancel,
    }
}

/// Resolves once the UI dropped its `Connection`.
async fn cancelled(cancel: &mut watch::Receiver<()>) {
    while cancel.changed().await.is_ok() {}
}

pub fn normalize_addr(addr: &str) -> String {
    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    }
}

/// Aborts the task when the connection ends on any path.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run(
    cfg: ClientConfig,
    mut commands: mpsc::Receiver<Queued<Command>>,
    events: &EventSender,
    ctx: &eframe::egui::Context,
    upload_backlog: &AtomicUsize,
    cancel: &mut watch::Receiver<()>,
    traffic: Arc<crate::bandwidth::Traffic>,
) -> anyhow::Result<()> {
    // Connecting, TLS and login stop as soon as the UI lets go: dropping
    // the unfinished stream closes it, nothing of it is reused.
    let (reader, mut writer, connected, welcome_len) = tokio::select! {
        r = login(&cfg) => r?,
        _ = cancelled(cancel) => return Ok(()),
    };
    tokio::select! {
        r = events.send(connected, welcome_len) => if r.is_err() { return Ok(()) },
        _ = cancelled(cancel) => return Ok(()),
    }
    ctx.request_repaint();

    // Lesen in eigenem Task: `read_frame` ist NICHT abbruchsicher. Lief es
    // frueher als Zweig im `select!`, wurde es bei jedem ausgehenden Befehl
    // (25 fps Video!) mitten in einem Frame abgebrochen -> Stream verschoben
    // -> "Netzwerkfehler". Der Task liest immer komplette Frames und wird nur
    // abgebrochen, wenn die ganze Verbindung endet.
    let mut reader_task = AbortOnDrop(tokio::spawn(read_loop(
        reader,
        events.clone(),
        ctx.clone(),
        traffic.clone(),
    )));

    loop {
        tokio::select! {
            biased;
            _ = cancelled(cancel) => {
                // Between two frames: say goodbye, but never wait long for it.
                let _ = timeout(CLOSE_TIMEOUT, write_frame(&mut writer, &ClientMessage::Disconnect)).await;
                return Ok(());
            }
            res = &mut reader_task.0 => {
                return match res {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("Receive task: {e}")),
                };
            }
            queued = commands.recv() => {
                let Some(Queued { item, _bytes }) = queued else { return Ok(()) };
                let len = item.queued_len();
                let msg = item.into_message();
                let video_len = match &msg {
                    ClientMessage::VideoFrame { data, .. } => data.len(),
                    _ => 0,
                };
                let limit = WRITE_TIMEOUT + Duration::from_secs((len / WRITE_MIN_RATE) as u64);
                // A frame cut off by a cancel or a timeout is half written:
                // the connection ends with it, the stream is never reused.
                let res = tokio::select! {
                    r = timeout(limit, write_frame(&mut writer, &msg)) => r,
                    _ = cancelled(cancel) => return Ok(()),
                };
                drop(_bytes);
                upload_backlog.fetch_sub(video_len, Ordering::Relaxed);
                match res {
                    Ok(Ok(())) => traffic.sent(&msg),
                    Ok(Err(e)) => anyhow::bail!("Network error while sending: {e}"),
                    Err(_) => anyhow::bail!("Timeout while sending to the server"),
                }
            }
        }
    }
}

type Reader = tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>;
type Writer = tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>;

/// TCP, TLS with the pinned certificate, Hello and Welcome.
async fn login(cfg: &ClientConfig) -> anyhow::Result<(Reader, Writer, NetEvent, usize)> {
    let addr = normalize_addr(&cfg.server_address);
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout while connecting to {addr}"))??;
    tcp.set_nodelay(true).ok();
    let peer = tcp.peer_addr()?;

    // TLS with the server certificate pinned; an unknown server needs the user's OK first.
    let expected = cfg.known_servers.get(&addr).cloned();
    let verifier = secure::PinnedVerifier::new(expected.clone());
    let connector = tokio_rustls::TlsConnector::from(secure::client_config(verifier.clone()));
    let name = rustls::pki_types::ServerName::try_from(secure::TLS_SERVER_NAME)?;
    let mut stream = match timeout(CONNECT_TIMEOUT, connector.connect(name, tcp)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let seen = verifier.seen.lock().unwrap().clone();
            if let (Some(seen), Some(want)) = (seen, expected) {
                if !seen.eq_ignore_ascii_case(&want) {
                    return Err(CertDecision {
                        addr,
                        fingerprint: seen,
                        changed: true,
                    }
                    .into());
                }
            }
            anyhow::bail!("Secure connection failed: {e} (old server version?)");
        }
        Err(_) => anyhow::bail!("Timeout during the secure handshake with {addr}"),
    };
    let Some(fingerprint) = verifier.seen.lock().unwrap().clone() else {
        anyhow::bail!("Secure connection failed: the server sent no certificate");
    };
    if expected.is_none() {
        // Send TLS close_notify before dropping the socket; never send application data.
        use tokio::io::AsyncWriteExt;
        let _ = timeout(Duration::from_secs(1), stream.shutdown()).await;
        return Err(CertDecision {
            addr,
            fingerprint,
            changed: false,
        }
        .into());
    }
    let (mut reader, mut writer) = tokio::io::split(stream);

    let hello = ClientMessage::Hello {
        username: cfg.username.clone(),
        protocol_version: PROTOCOL_VERSION,
        password: cfg.server_password.clone(),
        identity: cfg.identity.clone(),
        invite: cfg.invite.clone(),
    };
    timeout(CONNECT_TIMEOUT, write_frame(&mut writer, &hello))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout while logging in to {addr}"))??;

    let first: Option<(ServerMessage, usize)> =
        timeout(CONNECT_TIMEOUT, read_frame_sized(&mut reader))
            .await
            .map_err(|_| anyhow::anyhow!("Timeout while waiting for the server"))??;
    let (
        client_id,
        settings,
        udp_token,
        channels,
        users,
        spawn_log,
        chat_history,
        gif_search,
        open_permissions,
        udp_key,
        len,
    ) = match first {
        Some((
            ServerMessage::Welcome {
                client_id,
                udp_token,
                channels,
                users,
                spawn_log,
                chat_history,
                settings,
                gif_search,
                open_permissions,
                udp_key,
            },
            len,
        )) => (
            client_id,
            settings,
            udp_token,
            channels,
            users,
            spawn_log,
            chat_history,
            gif_search,
            open_permissions,
            udp_key,
            len,
        ),
        Some((ServerMessage::Error { message }, _)) => anyhow::bail!("{message}"),
        Some(_) => anyhow::bail!("Unexpected server response"),
        None => anyhow::bail!("The server closed the connection"),
    };

    // UDP-Socket (std) fuer die Audio-Threads vorbereiten.
    let udp = std::net::UdpSocket::bind("0.0.0.0:0")?;
    udp.connect(peer)?;
    let connected = NetEvent::Connected {
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
    };
    Ok((reader, writer, connected, len))
}

async fn read_loop<R: AsyncRead + Unpin>(
    mut reader: R,
    events: EventSender,
    ctx: eframe::egui::Context,
    traffic: Arc<crate::bandwidth::Traffic>,
) -> anyhow::Result<()> {
    loop {
        match read_frame_sized::<_, ServerMessage>(&mut reader).await {
            Ok(Some((m, len))) => {
                traffic.received(&m, len);
                // A full queue drops video frames (the decoder asks for a
                // keyframe); the rest waits, which stops reading from the server.
                let sent = match m {
                    ServerMessage::VideoFrame { .. } => {
                        events.try_send(NetEvent::Message(m), len) != Err(SendError::Closed)
                    }
                    m => events.send(NetEvent::Message(m), len).await.is_ok(),
                };
                if !sent {
                    return Ok(()); // UI ist weg
                }
                ctx.request_repaint();
            }
            Ok(None) => anyhow::bail!("The server closed the connection"),
            Err(e) => anyhow::bail!("Network error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn oversized_queue_items_are_refused_without_consuming_budget() {
        let (connection, mut received) = super::Connection::test_pair();
        let result = connection.send(super::Command::SendChat {
            channel_id: 1,
            text: "x".repeat(super::COMMAND_QUEUE_BYTES),
            image: None,
            reply_to: None,
        });
        assert_eq!(result, Err(super::SendError::Full));
        assert!(connection.take_refused());
        assert!(received.try_recv().is_none());
        assert_eq!(
            connection.traffic.snapshot(),
            [[0; crate::bandwidth::MEDIA]; 2]
        );
        assert_eq!(
            connection.tx.bytes.available_permits(),
            super::COMMAND_QUEUE_BYTES
        );
        assert_eq!(
            super::permits(usize::MAX, super::EVENT_QUEUE_BYTES),
            Err(super::SendError::Full)
        );
    }

    use super::*;
    use rusttalk_shared::framing::read_frame;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::error::TryRecvError;

    const N: usize = 150;
    const SIZE: usize = 180 * 1024;

    type ServerStream = tokio_rustls::server::TlsStream<TcpStream>;

    /// A local TLS server running `f` on its first connection (own thread and runtime).
    fn tls_server<F, Fut>(f: F) -> std::net::SocketAddr
    where
        F: FnOnce(ServerStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = TcpListener::from_std(std_listener).unwrap();
                let (tcp, _) = listener.accept().await.unwrap();
                let (c, k) = rcgen_free_cert();
                let tls = rusttalk_shared::secure::server_config(c, k).unwrap();
                let stream = tokio_rustls::TlsAcceptor::from(tls)
                    .accept(tcp)
                    .await
                    .unwrap();
                f(stream).await;
            });
        });
        addr
    }

    fn welcome() -> ServerMessage {
        ServerMessage::Welcome {
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
        }
    }

    fn pinned(addr: std::net::SocketAddr) -> ClientConfig {
        let mut cfg = ClientConfig {
            server_address: addr.to_string(),
            ..ClientConfig::default()
        };
        cfg.known_servers
            .insert(addr.to_string(), secure::fingerprint(&rcgen_free_cert().0));
        cfg
    }

    /// The next event within `within` (`Err(Empty)` = none came).
    fn next_event(rx: &mut EventReceiver, within: Duration) -> Result<NetEvent, TryRecvError> {
        let deadline = Instant::now() + within;
        loop {
            match rx.try_recv() {
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                other => return other,
            }
        }
    }

    /// Every event until the network thread is gone (its senders dropped).
    fn until_closed(rx: &mut EventReceiver, within: Duration) -> Vec<NetEvent> {
        let deadline = Instant::now() + within;
        let mut seen = Vec::new();
        loop {
            match next_event(rx, deadline.saturating_duration_since(Instant::now())) {
                Ok(ev) => seen.push(ev),
                Err(TryRecvError::Disconnected) => return seen,
                Err(TryRecvError::Empty) => panic!("network thread still running after {within:?}"),
            }
        }
    }

    fn chat(text: &str, image: Option<Vec<u8>>) -> Command {
        Command::SendChat {
            channel_id: 1,
            text: text.into(),
            image,
            reply_to: None,
        }
    }

    #[tokio::test]
    async fn bandwidth_counts_received_video_even_when_ui_queue_is_full() {
        let (mut writer, reader) = tokio::io::duplex(4096);
        let (events, _rx) = event_queue();
        let traffic = Arc::new(crate::bandwidth::Traffic::default());
        let write = async move {
            for _ in 0..EVENT_QUEUE + 10 {
                write_frame(
                    &mut writer,
                    &ServerMessage::VideoFrame {
                        from_id: 9,
                        kind: VideoKind::Screen,
                        data: vec![7; 40],
                    },
                )
                .await
                .unwrap();
            }
        };
        let read = read_loop(reader, events, Default::default(), traffic.clone());
        let (_, result) = tokio::join!(write, read);
        assert!(result.is_err()); // EOF after complete frames.
        assert_eq!(
            traffic.snapshot()[crate::bandwidth::IN],
            [((EVENT_QUEUE + 10) * 40) as u64, 0, 0, 0, 0]
        );
    }

    /// Regression: Frueher lief `read_frame` als Zweig im `select!` und wurde
    /// bei jedem ausgehenden Befehl mitten im Frame abgebrochen -> Stream
    /// verschoben -> "Netzwerkfehler". Hier gehen grosse Video-Frames
    /// gleichzeitig in beide Richtungen.
    #[test]
    fn large_frames_in_both_directions_stay_intact() {
        let received_by_server = Arc::new(AtomicUsize::new(0));
        let received = received_by_server.clone();
        let addr = tls_server(move |stream| async move {
            let (mut r, mut w) = tokio::io::split(stream);
            let _hello: Option<ClientMessage> = read_frame(&mut r).await.unwrap();
            tokio::spawn(async move {
                while let Ok(Some(msg)) = read_frame::<_, ClientMessage>(&mut r).await {
                    if matches!(msg, ClientMessage::VideoFrame { .. }) {
                        received.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
            write_frame(&mut w, &welcome()).await.unwrap();
            for i in 0..N {
                let data = vec![(i % 251) as u8; SIZE];
                let msg = ServerMessage::Chat {
                    id: i as u64,
                    channel_id: 1,
                    from_id: 2,
                    from_name: "peer".into(),
                    text: String::new(),
                    timestamp: 0,
                    image: Some(data),
                    reply_to: None,
                };
                write_frame(&mut w, &msg).await.unwrap();
            }
            // Verbindung offen halten, bis der Test fertig ist.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let (events, mut event_rx) = event_queue();
        let backlog = Arc::new(AtomicUsize::new(0));
        let conn = connect(
            pinned(addr),
            events,
            eframe::egui::Context::default(),
            backlog.clone(),
        );

        // Client laedt gleichzeitig Video hoch (a full queue is retried, as the capture threads would drop).
        for _ in 0..N {
            loop {
                backlog.fetch_add(SIZE, Ordering::Relaxed);
                match conn.send(Command::VideoFrame(VideoKind::Camera, vec![9u8; SIZE])) {
                    Ok(()) => break,
                    Err(e) => {
                        backlog.fetch_sub(SIZE, Ordering::Relaxed);
                        assert_eq!(e, SendError::Full);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        }
        assert!(
            !conn.take_refused(),
            "video frames never count as refused commands"
        );

        let mut frames = 0;
        let deadline = Instant::now() + Duration::from_secs(20);
        while frames < N {
            match next_event(
                &mut event_rx,
                deadline.saturating_duration_since(Instant::now()),
            ) {
                Ok(NetEvent::Message(ServerMessage::Chat {
                    image: Some(data), ..
                })) => {
                    assert_eq!(data.len(), SIZE);
                    assert!(
                        data.iter().all(|&b| b == (frames % 251) as u8),
                        "Frame {frames} korrupt"
                    );
                    frames += 1;
                }
                Ok(NetEvent::Disconnected { reason }) => panic!("Verbindung verloren: {reason}"),
                Ok(_) => {}
                Err(_) => panic!("Timeout: nur {frames}/{N} Frames empfangen"),
            }
        }
        // Upload kam komplett an und der Zaehler ist wieder bei 0.
        let deadline = Instant::now() + Duration::from_secs(10);
        while received_by_server.load(Ordering::Relaxed) < N && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(received_by_server.load(Ordering::Relaxed), N);
        assert_eq!(backlog.load(Ordering::Relaxed), 0);
        let counts = conn.traffic.snapshot();
        assert_eq!(
            counts[crate::bandwidth::OUT][crate::bandwidth::Media::Camera as usize],
            (N * SIZE) as u64
        );
        assert!(
            counts[crate::bandwidth::IN][crate::bandwidth::Media::Other as usize]
                > (N * SIZE) as u64
        );
    }

    /// A server flooding big messages fills the event queue up to its byte
    /// budget and then waits; commands still go out, and dropping the
    /// connection ends it promptly, even if unread flood data resets TCP.
    #[test]
    fn saturated_inbound_queue_neither_blocks_commands_nor_disconnect() {
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
        let addr = tls_server(move |stream| async move {
            let (mut r, mut w) = tokio::io::split(stream);
            let _hello: Option<ClientMessage> = read_frame(&mut r).await.unwrap();
            let reader = tokio::spawn(async move {
                loop {
                    let seen = match read_frame::<_, ClientMessage>(&mut r).await {
                        Ok(Some(ClientMessage::Chat { text, .. })) => text,
                        Ok(Some(ClientMessage::Disconnect)) => "goodbye".into(),
                        Ok(Some(_)) => continue,
                        _ => "closed".into(),
                    };
                    let end = seen != "ping";
                    let _ = seen_tx.send(seen);
                    if end {
                        return;
                    }
                }
            });
            write_frame(&mut w, &welcome()).await.unwrap();
            for i in 0..64u64 {
                let msg = ServerMessage::Chat {
                    id: i,
                    channel_id: 1,
                    from_id: 2,
                    from_name: "flood".into(),
                    text: String::new(),
                    timestamp: 0,
                    image: Some(vec![0; 1024 * 1024]),
                    reply_to: None,
                };
                if write_frame(&mut w, &msg).await.is_err() {
                    break;
                }
            }
            // Keep the runtime alive until the reader reports the client disconnect.
            tokio::time::timeout(Duration::from_secs(10), reader)
                .await
                .unwrap()
                .unwrap();
        });
        let (events, mut event_rx) = event_queue();
        let conn = connect(
            pinned(addr),
            events,
            eframe::egui::Context::default(),
            Default::default(),
        );
        // The queue fills up to its budget and stays there (nobody drains it).
        let deadline = Instant::now() + Duration::from_secs(10);
        while event_rx.queued_bytes() < EVENT_QUEUE_BYTES - 2 * 1024 * 1024
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(200));
        let queued = event_rx.queued_bytes();
        assert!(
            queued >= EVENT_QUEUE_BYTES - 2 * 1024 * 1024,
            "queue filled: {queued}"
        );
        assert!(
            queued <= EVENT_QUEUE_BYTES,
            "never above the byte budget: {queued}"
        );
        conn.send(chat("ping", None)).unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "ping",
            "commands are not starved"
        );
        assert_eq!(seen_rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
        let started = Instant::now();
        drop(conn);
        let terminal = seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Disconnect is best-effort: closing with unread flood data can reset TCP.
        assert!(matches!(terminal.as_str(), "goodbye" | "closed"));
        let events = until_closed(&mut event_rx, Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            events.len() < 64,
            "bounded by bytes, not only by count: {}",
            events.len()
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, NetEvent::Disconnected { .. })),
            "a cancelled connection reports nothing"
        );
        assert_eq!(
            event_rx.queued_bytes(),
            0,
            "taking events frees their bytes"
        );
    }

    /// A server that stops reading: the bounded command queue refuses
    /// instead of growing, and the stalled write times out and ends the
    /// connection (a half written frame is never reused).
    #[test]
    fn stalled_writer_refuses_commands_and_times_out() {
        let addr = tls_server(|stream| async move {
            let (mut r, mut w) = tokio::io::split(stream);
            let _hello: Option<ClientMessage> = read_frame(&mut r).await.unwrap();
            write_frame(&mut w, &welcome()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await; // never reads again
        });
        let (events, mut event_rx) = event_queue();
        let conn = connect(
            pinned(addr),
            events,
            eframe::egui::Context::default(),
            Default::default(),
        );
        assert!(matches!(
            next_event(&mut event_rx, Duration::from_secs(5)),
            Ok(NetEvent::Connected { .. })
        ));
        let refused = fill_until_refused(&conn);
        assert_eq!(refused, SendError::Full);
        assert!(conn.take_refused() && !conn.take_refused(), "reported once");
        match next_event(&mut event_rx, Duration::from_secs(10)) {
            Ok(NetEvent::Disconnected { reason }) => {
                assert!(reason.contains("Timeout while sending"), "{reason}")
            }
            other => panic!("expected the stalled write to time out, got {other:?}"),
        }
        assert_eq!(conn.send(chat("late", None)), Err(SendError::Closed));
    }

    /// Dropping the connection while a write is stuck ends the thread at
    /// once, long before the write timeout, and reports nothing.
    #[test]
    fn disconnect_does_not_wait_for_a_stalled_writer() {
        let addr = tls_server(|stream| async move {
            let (mut r, mut w) = tokio::io::split(stream);
            let _hello: Option<ClientMessage> = read_frame(&mut r).await.unwrap();
            write_frame(&mut w, &welcome()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let (events, mut event_rx) = event_queue();
        let conn = connect(
            pinned(addr),
            events,
            eframe::egui::Context::default(),
            Default::default(),
        );
        assert!(matches!(
            next_event(&mut event_rx, Duration::from_secs(5)),
            Ok(NetEvent::Connected { .. })
        ));
        fill_until_refused(&conn);
        let started = Instant::now();
        drop(conn);
        let events = until_closed(&mut event_rx, Duration::from_millis(800));
        assert!(events.is_empty(), "{events:?}");
        assert!(started.elapsed() < WRITE_TIMEOUT);
    }

    /// Cancelling while the TLS handshake hangs ends the attempt at once.
    #[test]
    fn disconnect_cancels_a_hanging_handshake() {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = std_listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let _held = std_listener.accept(); // accepts TCP, never answers TLS
            std::thread::sleep(Duration::from_secs(10));
        });
        let (events, mut event_rx) = event_queue();
        let conn = connect(
            pinned(addr),
            events,
            eframe::egui::Context::default(),
            Default::default(),
        );
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        drop(conn);
        let events = until_closed(&mut event_rx, Duration::from_secs(1));
        assert!(events.is_empty(), "{events:?}");
        assert!(started.elapsed() < CONNECT_TIMEOUT);
    }

    /// Queues 60 KiB chat images until the queue refuses one.
    fn fill_until_refused(conn: &Connection) -> SendError {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Err(e) = conn.send(chat("x", Some(vec![1; 60 * 1024]))) {
                return e;
            }
        }
        panic!("the command queue never filled up");
    }

    #[test]
    fn command_queue_is_bounded_by_bytes_and_count() {
        let (conn, mut rx) = Connection::test_pair();
        let image = vec![0u8; MAX_MEDIA_BYTES];
        let mut sent = 0;
        while conn.send(chat("", Some(image.clone()))).is_ok() {
            sent += 1;
        }
        assert_eq!(
            sent,
            COMMAND_QUEUE_BYTES / (MAX_MEDIA_BYTES + ITEM_OVERHEAD)
        );
        assert!(conn.take_refused());
        while rx.try_recv().is_some() {}
        assert!(
            conn.send(chat("again", Some(image))).is_ok(),
            "taken commands free their bytes"
        );
        while rx.try_recv().is_some() {}
        let small = (0..)
            .take_while(|_| conn.send(Command::JoinChannel(1)).is_ok())
            .count();
        assert_eq!(small, COMMAND_QUEUE);
        assert!(!rx.cancelled());
        drop(conn);
        assert!(rx.cancelled());
    }

    /// A remembered fingerprint that does not match -> refused with CERT_CHANGED
    /// (no Hello is sent, nothing reaches the server).
    #[test]
    fn changed_certificate_is_refused() {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = TcpListener::from_std(std_listener).unwrap();
                let (tcp, _) = listener.accept().await.unwrap();
                let (c, k) = rcgen_free_cert();
                let tls = rusttalk_shared::secure::server_config(c, k).unwrap();
                let _ = tokio_rustls::TlsAcceptor::from(tls).accept(tcp).await; // fails: client aborts
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
        });
        let mut cfg = ClientConfig {
            server_address: addr.to_string(),
            ..ClientConfig::default()
        };
        cfg.known_servers
            .insert(addr.to_string(), "00:11:22".into());
        let (event_tx, mut event_rx) = event_queue();
        let _tx = connect(
            cfg,
            event_tx,
            eframe::egui::Context::default(),
            Arc::new(AtomicUsize::new(0)),
        );
        match next_event(&mut event_rx, Duration::from_secs(10)) {
            Ok(NetEvent::CertCheck {
                addr: reported,
                fingerprint,
                changed,
            }) => {
                assert!(changed);
                assert_eq!(reported, addr.to_string());
                assert_eq!(
                    fingerprint,
                    secure::fingerprint(&rcgen_free_cert().0),
                    "new fingerprint is reported"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// First contact: the client stops after the TLS handshake and sends no Hello
    /// until the user trusted the fingerprint; then it logs in with it pinned.
    #[test]
    fn unknown_server_gets_no_hello_until_trusted() {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        // What the server read on each connection: Some(true) = a Hello arrived.
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = TcpListener::from_std(std_listener).unwrap();
                let (c, k) = rcgen_free_cert();
                let tls = tokio_rustls::TlsAcceptor::from(rusttalk_shared::secure::server_config(c, k).unwrap());
                for attempt in 0..2 {
                    let (tcp, _) = listener.accept().await.unwrap();
                    let stream = tls.accept(tcp).await.unwrap();
                    let (mut r, mut w) = tokio::io::split(stream);
                    if attempt == 0 {
                        use tokio::io::AsyncReadExt;
                        let mut byte = [0u8; 1];
                        match timeout(Duration::from_secs(5), r.read(&mut byte)).await {
                            Ok(Ok(0)) => {},
                            Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {},
                            other => panic!("expected connection closure with zero application bytes before trust, got {other:?}"),
                        }
                        seen_tx.send(false).unwrap();
                        continue;
                    }
                    let first = timeout(Duration::from_secs(5), read_frame::<_, ClientMessage>(&mut r)).await;
                    let hello = matches!(first, Ok(Ok(Some(ClientMessage::Hello { .. }))));
                    seen_tx.send(hello).unwrap();
                    if hello {
                        let _ = write_frame(&mut w, &ServerMessage::Error { message: "test done".into() }).await;
                    }
                }
            });
        });

        let mut cfg = ClientConfig {
            server_address: addr.to_string(),
            server_password: Some("pw".into()),
            ..ClientConfig::default()
        };
        cfg.identity = "ab".repeat(32);
        let (event_tx, mut event_rx) = event_queue();
        let _tx = connect(
            cfg.clone(),
            event_tx.clone(),
            eframe::egui::Context::default(),
            Arc::new(AtomicUsize::new(0)),
        );
        let (pinned_addr, fp) = match next_event(&mut event_rx, Duration::from_secs(10)) {
            Ok(NetEvent::CertCheck {
                addr: a,
                fingerprint,
                changed: false,
            }) => (a, fingerprint),
            other => panic!("expected a trust question, got {other:?}"),
        };
        assert_eq!(pinned_addr, addr.to_string());
        assert_eq!(fp, secure::fingerprint(&rcgen_free_cert().0));
        assert!(
            !seen_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            "no Hello before the user trusts the server"
        );

        cfg.known_servers.insert(pinned_addr, fp);
        let _tx2 = connect(
            cfg,
            event_tx,
            eframe::egui::Context::default(),
            Arc::new(AtomicUsize::new(0)),
        );
        assert!(
            seen_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            "Hello once the fingerprint is pinned"
        );
        match next_event(&mut event_rx, Duration::from_secs(10)) {
            Ok(NetEvent::Disconnected { reason }) => assert_eq!(reason, "test done"),
            other => panic!("expected the server's answer, got {other:?}"),
        }
    }

    /// Self-signed test certificate (DER cert, PKCS#8 key), generated once per test run.
    fn rcgen_free_cert() -> (Vec<u8>, Vec<u8>) {
        static CERT: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
        CERT.get_or_init(|| {
            let ck = rcgen::generate_simple_self_signed(vec![secure::TLS_SERVER_NAME.to_string()])
                .unwrap();
            (ck.cert.der().to_vec(), ck.signing_key.serialize_der())
        })
        .clone()
    }
}
