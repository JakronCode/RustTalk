//! RustTalk-Client (Windows): egui-App mit Voice ueber Opus/UDP.
//!
//! Command line (for admins / scripts):
//!   --install [--desktop-shortcut] [--no-start]   install this exe for the user
//!   --uninstall [--quiet] [--remove-settings]     uninstall (Apps & features uses this)
//!   --update                                      install the latest release (installed app)

// Release builds: no console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod hotkey;
#[cfg(windows)]
mod install;
#[cfg(not(windows))]
#[path = "linux/install.rs"]
mod install;
mod logo;
mod media;
mod net;
mod notify;
mod update;
#[cfg(windows)]
mod video;
#[cfg(not(windows))]
#[path = "linux/video.rs"]
mod video;
mod video_codec;
mod video_common;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = run_cli(&args) {
        std::process::exit(code);
    }
    if cfg!(target_os = "linux") || install::mode() == install::Mode::Installed {
        install::refresh_installed();
    }

    // Debug builds: own window title for automated UI screenshots.
    let title = if cfg!(debug_assertions) {
        std::env::var("RUSTTALK_DEV_TITLE").unwrap_or_else(|_| "RustTalk".to_string())
    } else {
        "RustTalk".to_string()
    };
    let options = eframe::NativeOptions {
        // Wayland may stop frame callbacks for hidden windows; blocking swaps stall desktop replies.
        vsync: !cfg!(target_os = "linux"),
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id("org.rusttalk.RustTalk")
            .with_icon(logo::window_icon())
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RustTalk",
        options,
        Box::new(|cc| Ok(Box::new(app::RustTalkApp::new(cc)))),
    )
}

/// Command line actions. `Some(exit code)` = done, don't start the UI.
fn run_cli(args: &[String]) -> Option<i32> {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let fail = |quiet: bool, msg: String| {
        tracing::error!("{msg}");
        if !quiet {
            rfd::MessageDialog::new().set_title("RustTalk").set_description(&msg).set_level(rfd::MessageLevel::Error).show();
        }
        1
    };
    if has("--version") {
        println!("RustTalk {} {}-{}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
        return Some(0);
    }
    #[cfg(target_os = "linux")]
    if has("--check-runtime") {
        return Some(match video::check_runtime() {
            Ok(()) => { println!("RustTalk capture runtime ready"); 0 }
            Err(error) => fail(true, format!("Capture runtime unavailable: {error:#}")),
        });
    }
    if has("--install") {
        return Some(match install::install(has("--desktop-shortcut")) {
            Ok(exe) => {
                if !has("--no-start") {
                    let _ = std::process::Command::new(exe).spawn();
                }
                0
            }
            Err(e) => fail(true, format!("Installation failed: {e:#}")),
        });
    }
    if has("--uninstall") {
        let quiet = has("--quiet");
        let mut keep_settings = !has("--remove-settings");
        if !quiet {
            let yes = rfd::MessageDialog::new()
                .set_title("Uninstall RustTalk")
                .set_description("Do you want to remove RustTalk from this computer?")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if yes != rfd::MessageDialogResult::Yes {
                return Some(0);
            }
            let remove = rfd::MessageDialog::new()
                .set_title("Uninstall RustTalk")
                .set_description("Also delete your settings (saved servers, audio settings, volumes)?")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            keep_settings = remove != rfd::MessageDialogResult::Yes;
        }
        return Some(match install::uninstall(keep_settings) {
            Ok(()) => 0,
            Err(e) => fail(quiet, format!("Uninstall failed: {e:#}")),
        });
    }
    if has("--update") {
        if install::mode() != install::Mode::Installed {
            return Some(fail(true, "--update only works for the installed RustTalk".to_string()));
        }
        return Some(match update::check() {
            Ok(Some(release)) => {
                let progress = std::sync::atomic::AtomicU32::new(0);
                match update::download_and_install(&release, &progress) {
                    Ok(_) => 0,
                    Err(e) => fail(true, format!("Update failed: {e:#}")),
                }
            }
            Ok(None) => 0, // up to date
            Err(e) => fail(true, format!("Update check failed: {e:#}")),
        });
    }
    None
}
