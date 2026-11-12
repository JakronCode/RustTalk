//! RustTalk-Server: TeamSpeak-artiger Voice-Server.
//!
//! Start: `rusttalk-server [pfad/zur/server.toml]`
//! Update: `sudo rusttalk-server update [--check] [--no-restart]` (see [`update`])
//! Version: `rusttalk-server --version`

mod config;
mod db;
mod gif;
mod link;
mod moderation;
mod state;
mod tls;
mod tcp;
mod udp;
mod update;

use std::sync::{Arc, Mutex};

use anyhow::Context;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::ServerConfig;
use crate::state::State;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("rusttalk-server {}", update::VERSION);
            return Ok(());
        }
        Some("update") => {
            let code = tokio::task::spawn_blocking(move || update::run_cli(&args[1..])).await?;
            std::process::exit(code);
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path = args.first().cloned().unwrap_or_else(|| "server.toml".to_string());
    let cfg = ServerConfig::load(&config_path)
        .with_context(|| format!("Konfiguration '{config_path}' konnte nicht geladen werden"))?;

    info!(
        "Starte RustTalk-Server {} '{}' (max. {} Clients)",
        update::VERSION, cfg.server_name, cfg.max_clients
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

    // Update check: on start and once a day; admins get a notice in the client.
    if cfg.check_updates {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
            loop {
                tick.tick().await;
                match tokio::task::spawn_blocking(update::check).await {
                    Ok(Ok(Some(release))) => {
                        let notice = update::notice(&release);
                        if state.lock().unwrap().set_update_notice(notice.clone()) {
                            tracing::warn!("{notice}");
                        }
                    }
                    Ok(Ok(None)) => info!("Server ist aktuell ({})", update::VERSION),
                    Ok(Err(e)) => tracing::warn!("Update-Pruefung fehlgeschlagen: {e:#}"),
                    Err(_) => {}
                }
            }
        });
    }

    // TCP und UDP laufen auf derselben Port-Nummer (wie TeamSpeak).
    // TLS certificate next to the database (stable fingerprint for the clients).
    let cert_dir = std::path::Path::new(&cfg.db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let (cert, key) = tls::load_or_create(&cert_dir)?;
    info!("TLS-Fingerprint: {}", rusttalk_shared::secure::fingerprint(&cert));
    let tls = rusttalk_shared::secure::server_config(cert, key).context("TLS-Konfiguration")?;
    let tcp = tcp::run_listener(state.clone(), &cfg.bind_address, tls);
    let udp = udp::run_udp(state.clone(), &cfg.bind_address);
    tokio::try_join!(tcp, udp)?;
    Ok(())
}
