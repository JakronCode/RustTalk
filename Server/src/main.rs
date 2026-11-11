//! RustTalk-Server: TeamSpeak-artiger Voice-Server.
//!
//! Start: `rusttalk-server [pfad/zur/server.toml]`

mod config;
mod db;
mod state;
mod tcp;
mod udp;

use std::sync::{Arc, Mutex};

use anyhow::Context;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::ServerConfig;
use crate::state::State;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path =
        std::env::args().nth(1).unwrap_or_else(|| "server.toml".to_string());
    let cfg = ServerConfig::load(&config_path)
        .with_context(|| format!("Konfiguration '{config_path}' konnte nicht geladen werden"))?;

    info!(
        "Starte RustTalk-Server '{}' (max. {} Clients)",
        cfg.server_name, cfg.max_clients
    );
    let state = Arc::new(Mutex::new(State::new(&cfg)?));

    // Auto-AFK: alle 15 s stumme Clients verschieben (Einstellung im Client).
    tokio::spawn({
        let state = state.clone();
        async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                tick.tick().await;
                let mut st = state.lock().unwrap();
                let moved = st.afk_sweep(std::time::Instant::now());
                for (client_id, channel_id) in moved {
                    let target = st.channel(channel_id).map(|c| c.name.clone()).unwrap_or_default();
                    let message = st.afk_notice(&target);
                    info!("AFK: Client {client_id} -> {target}");
                    st.broadcast(&rusttalk_shared::protocol::ServerMessage::UserMoved { client_id, channel_id });
                    st.send_to(
                        client_id,
                        &rusttalk_shared::protocol::ServerMessage::Notice {
                            message,
                        },
                    );
                }
            }
        }
    });

    // TCP und UDP laufen auf derselben Port-Nummer (wie TeamSpeak).
    let tcp = tcp::run_listener(state.clone(), &cfg.bind_address);
    let udp = udp::run_udp(state.clone(), &cfg.bind_address);
    tokio::try_join!(tcp, udp)?;
    Ok(())
}
