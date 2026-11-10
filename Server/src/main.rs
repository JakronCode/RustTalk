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

    // TCP und UDP laufen auf derselben Port-Nummer (wie TeamSpeak).
    let tcp = tcp::run_listener(state.clone(), &cfg.bind_address);
    let udp = udp::run_udp(state.clone(), &cfg.bind_address);
    tokio::try_join!(tcp, udp)?;
    Ok(())
}
