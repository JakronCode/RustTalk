//! RustTalk-Server: TeamSpeak-artiger Voice-Server.
//!
//! Start: `rusttalk-server [pfad/zur/server.toml]`
//! Update: `sudo rusttalk-server update [--check] [--no-restart]` (see [`update`])
//! Version: `rusttalk-server --version`
//! Health: `rusttalk-server --healthcheck [pfad/zur/server.toml]` (see [`container`])

mod admin_auth;
mod config;
mod container;
mod db;
mod gif;
mod link;
mod moderation;
mod private_file;
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
        Some("--healthcheck") => {
            let path = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "server.toml".to_string());
            std::process::exit(container::healthcheck(&path).await);
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path = args.first().cloned().unwrap_or_else(|| "server.toml".to_string());
    let in_container = container::enabled();
    // Read the secret before anything is created, so a bad one leaves no state behind.
    let admin_from_file = std::env::var_os(config::ADMIN_PASSWORD_FILE_ENV)
        .map(|path| config::read_secret_file(std::path::Path::new(&path)))
        .transpose()
        .with_context(|| format!("Admin password from {}", config::ADMIN_PASSWORD_FILE_ENV))?;
    if in_container && container::write_default_config(std::path::Path::new(&config_path))? {
        info!("Created default config {config_path}");
    }
    let mut cfg = ServerConfig::load(&config_path)
        .with_context(|| format!("Could not load config '{config_path}'"))?;
    if let Some(password) = admin_from_file {
        cfg.admin_password = Some(password);
        info!(
            "Admin password read from {}",
            config::ADMIN_PASSWORD_FILE_ENV
        );
    }
    if in_container {
        // Without this, the first stranger to connect could make themselves admin.
        anyhow::ensure!(
            cfg.admin_password.as_deref().is_some_and(|p| !p.is_empty()),
            "No admin password: point {} at a mounted secret file",
            config::ADMIN_PASSWORD_FILE_ENV
        );
        cfg.admin_password_required = true;
    }

    info!(
        "Starting RustTalk server {} '{}' (max. {} clients)",
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
                        let notice = update::notice(&release, in_container);
                        if state.lock().unwrap().set_update_notice(notice.clone()) {
                            tracing::warn!("{notice}");
                        }
                    }
                    Ok(Ok(None)) => info!("Server is up to date ({})", update::VERSION),
                    Ok(Err(e)) => tracing::warn!("Update check failed: {e:#}"),
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
    info!("TLS fingerprint: {}", rusttalk_shared::secure::fingerprint(&cert));
    let tls = rusttalk_shared::secure::server_config(cert, key).context("TLS configuration")?;
    let tcp = tcp::run_listener(state.clone(), &cfg.bind_address, tls);
    let udp = udp::run_udp(state.clone(), &cfg.bind_address);
    tokio::select! {
        res = async { tokio::try_join!(tcp, udp) } => {
            res?;
        }
        res = shutdown_signal() => {
            res.context("Signal handler")?;
            info!("Shutting down");
            // Waits for a running change to finish, then closes SQLite (WAL checkpoint).
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            guard.close_db();
            info!("Shutdown complete");
            // Keep the lock until exit: no worker may acknowledge memory-only mutations.
            std::process::exit(0);
        }
    }
    // Do not wait for blocking tasks (update check, GIF search) that may still run.
    std::process::exit(0);
}

/// SIGTERM (`docker stop`, `systemctl stop`) or Ctrl+C / SIGINT.
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let terminate = term.recv();
    #[cfg(not(unix))]
    let terminate = std::future::pending::<Option<()>>();
    tokio::select! {
        _ = terminate => Ok(()),
        res = tokio::signal::ctrl_c() => res,
    }
}
