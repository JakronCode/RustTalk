//! UDP-Audio-Relay: ein Socket-Task, der Pakete an Channel-Mitglieder weiterleitet.
//!
//! Der Server mischt nicht – er leitet Opus-Pakete 1:1 weiter (TeamSpeak-Prinzip).
//! Beim Relay wird das Token-Feld durch die `client_id` des Sprechers ersetzt,
//! damit der Empfänger weiß, von wem das Paket stammt. Pakete sind verschluesselt:
//! geoeffnet mit dem Schluessel des Senders, neu versiegelt pro Empfaenger.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use rusttalk_shared::audio_packet::{decode, open_packet, seal_packet, AudioHeader, MAX_PACKET_LEN};
use rusttalk_shared::secure::{DIR_TO_CLIENT, DIR_TO_SERVER};
use tokio::net::UdpSocket;
use tracing::info;

use crate::state::SharedState;

pub async fn run_udp(state: SharedState, bind: &str) -> Result<()> {
    let socket = UdpSocket::bind(bind).await.with_context(|| format!("UDP-Bind an {bind}"))?;
    info!("UDP-Audio lauscht auf {bind}");

    let mut buf = vec![0u8; MAX_PACKET_LEN];

    loop {
        let (len, from) = socket.recv_from(&mut buf).await?;
        let Some((header, _)) = decode(&buf[..len]) else {
            continue; // kein RustTalk-Paket
        };

        // Authenticate FIRST (the address of a client is only learned from a
        // genuine packet), then collect the receivers with their keys.
        let Some(sender_key) = state.lock().unwrap().udp_cipher(header.token) else {
            continue; // unbekanntes Token
        };
        let Some(plain) = open_packet(&header, &buf[..len], &sender_key, DIR_TO_SERVER) else {
            continue; // gefaelscht / falscher Schluessel
        };
        let (speaker_id, targets) = {
            let mut st = state.lock().unwrap();
            match st.udp_relay_targets(header.token, from) {
                Some(v) => v,
                None => continue,
            }
        };
        if plain.is_empty() {
            continue; // Registrierungs-Ping: Adresse ist gelernt
        }

        // Token-Feld traegt beim Empfaenger die Sprecher-ID; pro Empfaenger neu versiegelt.
        let fwd = AudioHeader { flags: header.flags, token: speaker_id, sequence: header.sequence, timestamp: header.timestamp, counter: 0 };
        for (addr, key) in &targets {
            let out = seal_packet(&fwd, key, DIR_TO_CLIENT, &plain);
            // Einzelne Sendefehler (z. B. ICMP unreachable) ignorieren.
            let _ = socket.send_to(&out, addr).await;
        }
    }
}
