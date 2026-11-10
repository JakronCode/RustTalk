//! RustTalk-Client (Windows): egui-App mit Voice ueber Opus/UDP.

mod app;
mod audio;
mod config;
mod logo;
mod media;
mod net;
mod video;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    // Debug builds: own window title for automated UI screenshots.
    let title = if cfg!(debug_assertions) {
        std::env::var("RUSTTALK_DEV_TITLE").unwrap_or_else(|_| "RustTalk".to_string())
    } else {
        "RustTalk".to_string()
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(title)
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
