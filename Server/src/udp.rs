//! UDP-Audio-Relay: ein Socket-Task, der Pakete an Channel-Mitglieder weiterleitet.
//!
//! Der Server mischt nicht – er leitet Opus-Pakete 1:1 weiter (TeamSpeak-Prinzip).
//! Beim Relay wird das Token-Feld durch die `client_id` des Sprechers ersetzt,
//! damit der Empfänger weiß, von wem das Paket stammt.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use rusttalk_shared::audio_packet::{decode, encode, AudioHeader, MAX_PACKET_LEN};
use tokio::net::UdpSocket;
use tracing::info;

use crate::state::SharedState;

pub async fn run_udp(state: SharedState, bind: &str) -> Result<()> {
    let socket = UdpSocket::bind(bind).await.with_context(|| format!("UDP-Bind an {bind}"))?;
    info!("UDP-Audio lauscht auf {bind}");

    let mut buf = vec![0u8; MAX_PACKET_LEN];
    let mut out: Vec<u8> = Vec::with_capacity(MAX_PACKET_LEN);

    loop {
        let (len, from) = socket.recv_from(&mut buf).await?;
        let Some((header, payload)) = decode(&buf[..len]) else {
            continue; // kein RustTalk-Paket
        };

        // State nur kurz sperren: Adresse lernen + Ziele einsammeln.
        let (speaker_id, targets): (u64, Vec<SocketAddr>) = {
            let mut st = state.lock().unwrap();
            match st.udp_targets(header.token, from) {
                Some(v) => v,
                None => continue, // unbekanntes Token
            }
        };
        if targets.is_empty() {
            continue;
        }

        // Header umschreiben: Token-Feld trägt beim Empfänger die Sprecher-ID.
        let fwd = AudioHeader {
            flags: header.flags,
            token: speaker_id,
            sequence: header.sequence,
            timestamp: header.timestamp,
        };
        encode(&fwd, payload, &mut out);

        for addr in &targets {
            // Einzelne Sendefehler (z. B. ICMP unreachable) ignorieren.
            let _ = socket.send_to(&out, addr).await;
        }
    }
}
