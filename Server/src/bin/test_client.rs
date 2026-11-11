//! Headless CLI-Testclient fÃ¼r End-to-End-Tests und manuelle PrÃ¼fungen.
//!
//! Beispiele:
//!   test-client --addr 127.0.0.1:7331 --name Bob --audio-secs 5
//!   test-client --addr 10.69.40.101 --name WinTest --chat "Hallo"
//!
//! Ausgabe (zeilenorientiert, fÃ¼r Skripte greppbar):
//!   WELCOME client_id=1 token=... channels=1 users=1
//!   EVENT <Debug-Format der ServerMessage>
//!   DISCONNECTED [reason=...]
//!   AUDIO_RX=<n> AUDIO_RX_FROM=<id,id,...>

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rusttalk_shared::audio_packet::{self, AudioHeader, FRAME_SAMPLES, MAX_PACKET_LEN};
use rusttalk_shared::framing::{read_frame, write_frame};
use rusttalk_shared::protocol::*;
use tokio::net::{TcpStream, UdpSocket};

#[derive(Debug)]
struct Args {
    addr: String,
    name: String,
    password: Option<String>,
    /// Wie viele Sekunden lang Audiopakete (Dummy-Payload) gesendet werden.
    audio_secs: u64,
    /// Gesamtlaufzeit des Tests.
    duration_secs: u64,
    chat: Option<String>,
    /// Channel-ID fuer --chat (Default: Join-Ziel oder 1).
    chat_channel: Option<u64>,
    create_channel: Option<String>,
    join_channel: Option<u64>,
    /// Ersten Channel dieser Art joinen (z. B. "voice").
    join_kind: Option<ChannelKind>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            addr: format!("127.0.0.1:{DEFAULT_PORT}"),
            name: "tester".to_string(),
            password: None,
            audio_secs: 0,
            duration_secs: 0,
            chat: None,
            chat_channel: None,
            create_channel: None,
            join_channel: None,
            join_kind: None,
        }
    }
}

fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| -> Result<String> {
            it.next().with_context(|| format!("{flag} braucht einen Wert"))
        };
        match arg.as_str() {
            "--addr" => args.addr = value("--addr")?,
            "--name" => args.name = value("--name")?,
            "--password" => args.password = Some(value("--password")?),
            "--audio-secs" => args.audio_secs = value("--audio-secs")?.parse()?,
            "--duration-secs" => args.duration_secs = value("--duration-secs")?.parse()?,
            "--chat" => args.chat = Some(value("--chat")?),
            "--chat-channel" => args.chat_channel = Some(value("--chat-channel")?.parse()?),
            "--create-channel" => args.create_channel = Some(value("--create-channel")?),
            "--join-channel" => args.join_channel = Some(value("--join-channel")?.parse()?),
            "--join-kind" => {
                args.join_kind = Some(match value("--join-kind")?.to_ascii_lowercase().as_str() {
                    "spawn" => ChannelKind::Spawn,
                    "category" => ChannelKind::Category,
                    "text" => ChannelKind::Text,
                    "voice" => ChannelKind::Voice,
                    other => bail!("unbekannte Channel-Art: {other}"),
                })
            }
            "-h" | "--help" => {
                println!(
                    "test-client [--addr host:port] [--name N] [--password P] \
                     [--audio-secs N] [--duration-secs N] [--chat TEXT] \
                     [--create-channel NAME] [--join-channel ID]"
                );
                std::process::exit(0);
            }
            other => bail!("unbekanntes Argument: {other}"),
        }
    }
    if args.duration_secs == 0 {
        args.duration_secs = args.audio_secs.max(1) + 2;
    }
    Ok(args)
}

fn normalize_addr(addr: &str) -> String {
    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = parse_args()?;
    if let Err(e) = run(&args).await {
        println!("FAILED {e:#}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run(args: &Args) -> Result<()> {
    let addr = normalize_addr(&args.addr);
    let mut stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("Verbindung zu {addr} fehlgeschlagen"))?;
    stream.set_nodelay(true).ok();

    write_frame(
        &mut stream,
        &ClientMessage::Hello {
            username: args.name.clone(),
            protocol_version: PROTOCOL_VERSION,
            password: args.password.clone(),
            identity: format!("{:032x}{:032x}", rand::random::<u128>(), rand::random::<u128>()),
            invite: None,
        },
    )
    .await?;

    let first: Option<ServerMessage> = read_frame(&mut stream).await?;
    let (my_id, udp_token, mut join_target) = match first {
        Some(ServerMessage::Welcome { client_id, udp_token, channels, users, spawn_log, chat_history, .. }) => {
            println!(
                "WELCOME client_id={client_id} token={udp_token} channels={} users={} spawn_log={} history={}",
                channels.len(),
                users.len(),
                spawn_log.len(),
                chat_history.len()
            );
            for ch in &channels {
                println!(
                    "CHANNEL id={} kind={:?} name={:?} parent={:?} users={}",
                    ch.id,
                    ch.kind,
                    ch.name,
                    ch.parent,
                    ch.user_ids.len()
                );
            }
            // --join-kind aufloesen: ersten Channel der gewuenschten Art.
            let by_kind = args.join_kind.and_then(|kind| {
                channels.iter().filter(|c| c.kind == kind).min_by_key(|c| c.id).map(|c| c.id)
            });
            (client_id, udp_token, args.join_channel.or(by_kind))
        }
        Some(ServerMessage::Error { message }) => {
            println!("ERROR {message}");
            bail!("Server-Fehler: {message}");
        }
        other => bail!("unerwartete erste Antwort: {other:?}"),
    };

    let peer = stream.peer_addr()?;
    let (mut reader, mut writer) = tokio::io::split(stream);

    // UDP vorbereiten + Registrierungs-Ping (leere Payload).
    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    udp.connect(peer).await?;
    udp.send(&audio_packet::encode_to_vec(&AudioHeader::new(udp_token, 0, 0), &[])).await?;

    // Optionale Aktionen direkt nach dem Login.
    if let Some(name) = &args.create_channel {
        write_frame(
            &mut writer,
            &ClientMessage::CreateChannel { name: name.clone(), kind: ChannelKind::Text, parent: None },
        )
        .await?;
    }
    if let Some(id) = join_target {
        write_frame(&mut writer, &ClientMessage::JoinChannel { channel_id: id }).await?;
    }
    if let Some(text) = &args.chat {
        let channel_id = args.chat_channel.or(join_target).unwrap_or(1);
        write_frame(&mut writer, &ClientMessage::Chat { channel_id, text: text.clone(), image: None, reply_to: None }).await?;
    }

    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.duration_secs);
    let audio_until = start + Duration::from_secs(args.audio_secs);
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    let mut udp_buf = [0u8; MAX_PACKET_LEN];
    let dummy_payload = vec![0xABu8; 120]; // Server relayed payload-transparent
    let mut seq: u32 = 1;
    let mut timestamp: u32 = 0;
    let mut audio_rx: u64 = 0;
    let mut senders: HashSet<u64> = HashSet::new();

    loop {
        tokio::select! {
            msg = read_frame::<_, ServerMessage>(&mut reader) => {
                match msg {
                    Ok(Some(m)) => println!("EVENT {m:?}"),
                    Ok(None) => {
                        println!("DISCONNECTED");
                        break;
                    }
                    Err(e) => {
                        println!("DISCONNECTED reason={e}");
                        break;
                    }
                }
            }
            pkt = udp.recv(&mut udp_buf) => {
                if let Ok(n) = pkt {
                    if let Some((h, payload)) = audio_packet::decode(&udp_buf[..n]) {
                        if !payload.is_empty() {
                            audio_rx += 1;
                            senders.insert(h.token); // Relay: Token = Sprecher-ID
                        }
                    }
                }
            }
            _ = interval.tick() => {
                let now = Instant::now();
                if now < audio_until {
                    let pkt = audio_packet::encode_to_vec(
                        &AudioHeader::new(udp_token, seq, timestamp),
                        &dummy_payload,
                    );
                    udp.send(&pkt).await?;
                    seq = seq.wrapping_add(1);
                    timestamp = timestamp.wrapping_add(FRAME_SAMPLES as u32);
                }
                if now >= deadline {
                    break;
                }
            }
        }
    }

    let _ = write_frame(&mut writer, &ClientMessage::Disconnect).await;
    let sender_list: Vec<String> = senders.iter().map(u64::to_string).collect();
    println!("AUDIO_RX={audio_rx} AUDIO_RX_FROM={}", sender_list.join(","));
    let _ = my_id;
    Ok(())
}
// wdac-dice: 4
