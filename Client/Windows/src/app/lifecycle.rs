//! Install screen (exe not installed yet) and updates (check, install,
//! restart) plus clicks on toast notifications.

use super::theme;
use super::*;
use crate::install::{self, Mode};
use crate::update::{self, Release};

/// Install screen state (only when the exe is not installed).
pub(super) struct InstallScreen {
    desktop_shortcut: bool,
    error: Option<String>,
}

/// Messages from update threads.
pub(super) enum UpdateMsg {
    Checked(Result<Option<Release>, String>),
    Installed(Result<std::path::PathBuf, String>),
}

/// Update state of the running app.
pub(super) struct UpdateState {
    pub(super) checking: bool,
    pub(super) available: Option<Release>,
    pub(super) error: Option<String>,
    pub(super) installing: bool,
    pub(super) progress: Arc<std::sync::atomic::AtomicU32>,
    /// Last check finished without a newer version.
    pub(super) up_to_date: bool,
    pub(super) tx: std::sync::mpsc::Sender<UpdateMsg>,
    pub(super) rx: std::sync::mpsc::Receiver<UpdateMsg>,
}

impl UpdateState {
    pub(super) fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            checking: false,
            available: None,
            error: None,
            installing: false,
            progress: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            up_to_date: false,
            tx,
            rx,
        }
    }
}

impl InstallScreen {
    pub(super) fn new() -> Self {
        Self { desktop_shortcut: true, error: None }
    }
}

impl RustTalkApp {
    /// Full-window install screen of the not yet installed exe.
    pub(super) fn ui_install(&mut self, ctx: &egui::Context) {
        let Some(mut screen) = self.install_screen.take() else { return };
        let mut done = false;
        egui::CentralPanel::default().frame(egui::Frame::default().fill(theme::DARK)).show(ctx, |ui| {
            let avail = ui.available_size();
            ui.add_space(((avail.y - 420.0) / 2.0).max(16.0));
            ui.vertical_centered(|ui| {
                logo::paint_logo(ui, 72.0);
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Install RustTalk").size(28.0).strong().color(theme::HEADER));
                ui.label(egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(theme::MUTED));
                ui.add_space(16.0);
                egui::Frame::default()
                    .fill(theme::BG)
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_width(440.0);
                        ui.vertical(|ui| {
                            let dir = install::install_dir().map(|d| d.display().to_string()).unwrap_or_default();
                            ui.label(egui::RichText::new("RustTalk will be installed for your user (no admin rights needed):").color(theme::TEXT));
                            ui.label(egui::RichText::new(dir).monospace().size(12.0).color(theme::MUTED));
                            ui.add_space(8.0);
                            for line in [
                                "Start menu entry, uninstall via Windows \"Apps & features\"",
                                "Settings are kept in your user profile",
                                "Updates are installed from inside the app",
                            ] {
                                ui.label(egui::RichText::new(format!("{}  {line}", icons::regular::CHECK)).color(theme::TEXT));
                            }
                            ui.add_space(8.0);
                            ui.checkbox(&mut screen.desktop_shortcut, "Create a desktop shortcut");
                            if let Some(e) = &screen.error {
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(e).color(theme::RED));
                            }
                            ui.add_space(12.0);
                            let install = ui.add_sized(
                                [ui.available_width(), 38.0],
                                egui::Button::new(egui::RichText::new(format!("{}  Install", icons::regular::DOWNLOAD_SIMPLE)).size(15.0).strong().color(egui::Color32::WHITE))
                                    .fill(theme::ACCENT),
                            );
                            if install.clicked() {
                                match install::install(screen.desktop_shortcut) {
                                    Ok(exe) => match update::restart(&exe) {
                                        Ok(_) => done = true,
                                        Err(e) => screen.error = Some(format!("Installed, but could not start it: {e}")),
                                    },
                                    Err(e) => screen.error = Some(format!("{e:#}")),
                                }
                            }
                        });
                    });
            });
        });
        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            self.install_screen = Some(screen);
        }
    }

    /// Starts an update check in the background.
    pub(super) fn check_for_updates(&mut self) {
        if self.update.checking || self.update.installing {
            return;
        }
        self.update.checking = true;
        self.update.error = None;
        self.update.up_to_date = false;
        let tx = self.update.tx.clone();
        let ctx = self.ctx.clone();
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let _ = tx.send(UpdateMsg::Checked(update::check().map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Downloads + installs the available update (installed app only).
    pub(super) fn install_update(&mut self) {
        let Some(release) = self.update.available.clone() else { return };
        if install::mode() != Mode::Installed || self.update.installing {
            return;
        }
        self.update.installing = true;
        self.update.error = None;
        self.update.progress.store(0, std::sync::atomic::Ordering::Relaxed);
        let tx = self.update.tx.clone();
        let progress = self.update.progress.clone();
        let ctx = self.ctx.clone();
        let _ = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            let res = update::download_and_install(&release, &progress).map_err(|e| format!("{e:#}"));
            let _ = tx.send(UpdateMsg::Installed(res));
            ctx.request_repaint();
        });
    }

    /// Update results + toast clicks (every frame).
    pub(super) fn poll_background(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.update.rx.try_recv() {
            match msg {
                UpdateMsg::Checked(Ok(Some(r))) => {
                    self.update.checking = false;
                    self.set_status(format!("Update {} available - Settings > About", r.version));
                    self.update.available = Some(r);
                }
                UpdateMsg::Checked(Ok(None)) => {
                    self.update.checking = false;
                    self.update.up_to_date = true;
                }
                UpdateMsg::Checked(Err(e)) => {
                    self.update.checking = false;
                    self.update.error = Some(e);
                }
                UpdateMsg::Installed(Ok(exe)) => {
                    // Restart into the new version.
                    self.config.save();
                    self.disconnect();
                    match update::restart(&exe) {
                        Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.update.installing = false;
                            self.update.error = Some(format!("Installed - please restart RustTalk ({e})"));
                        }
                    }
                }
                UpdateMsg::Installed(Err(e)) => {
                    self.update.installing = false;
                    self.update.error = Some(e);
                }
            }
        }
        if self.update.installing {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // Toast clicked: bring the window to the front, open the channel.
        while let Ok(channel_id) = self.toast_rx.try_recv() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if self.channels.contains_key(&channel_id) {
                self.select_channel(channel_id);
            }
        }
    }

    /// Windows pop-up for a new message (while RustTalk is in the background).
    pub(super) fn message_toast(&mut self, channel_id: u64, from: &str, text: &str, has_image: bool, mentioned: bool) {
        if !self.config.desktop_notifications {
            return;
        }
        // Bursts: at most one toast every 2 s (mentions always).
        if !mentioned && self.last_toast.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.last_toast = Some(Instant::now());
        let channel = self.channel_name(channel_id);
        let (mut title, body) = crate::notify::message_text(from, &channel, text, has_image, self.config.notification_preview);
        if mentioned {
            title = format!("@ {title}");
        }
        let tx = self.toast_tx.clone();
        let ctx = self.ctx.clone();
        crate::notify::show(title, body, move || {
            let _ = tx.send(channel_id);
            ctx.request_repaint();
        });
    }

    /// "Updates" card for Settings > About.
    pub(super) fn updates_card(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, |ui| {
            theme::caption(ui, "Updates");
            let installed = install::mode() == Mode::Installed;
            if let Some(r) = self.update.available.clone() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icons::regular::ARROW_CIRCLE_UP).size(18.0).color(theme::GREEN));
                    ui.label(egui::RichText::new(format!("Version {} is available (you have {})", r.version, env!("CARGO_PKG_VERSION"))).strong().color(theme::HEADER));
                });
                if !r.notes.trim().is_empty() {
                    let notes: String = r.notes.chars().take(600).collect();
                    egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                        ui.label(egui::RichText::new(notes).size(12.0).color(theme::MUTED));
                    });
                }
                ui.horizontal(|ui| {
                    if self.update.installing {
                        let p = self.update.progress.load(std::sync::atomic::Ordering::Relaxed);
                        ui.add(egui::ProgressBar::new(p as f32 / 100.0).desired_width(220.0).text(format!("Downloading... {p} %")));
                    } else if installed {
                        if theme::primary_button(ui, &format!("{}  Install update & restart", icons::regular::DOWNLOAD_SIMPLE)).clicked() {
                            self.install_update();
                        }
                    } else {
                        ui.label(egui::RichText::new("Updates install automatically only in the installed version.").color(theme::MUTED));
                    }
                    if !r.page_url.is_empty() && ui.link("Release notes").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&r.page_url));
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    let text = if self.update.checking {
                        "Checking for updates...".to_string()
                    } else if self.update.up_to_date {
                        format!("{}  RustTalk is up to date", icons::regular::CHECK_CIRCLE)
                    } else {
                        "Not checked yet".to_string()
                    };
                    ui.label(egui::RichText::new(text).color(theme::TEXT));
                    if self.update.checking {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!self.update.checking, egui::Button::new(format!("{}  Check now", icons::regular::ARROWS_CLOCKWISE))).clicked() {
                            self.check_for_updates();
                        }
                    });
                });
            }
            if let Some(e) = &self.update.error {
                ui.label(egui::RichText::new(e).color(theme::RED));
            }
            ui.separator();
            theme::setting_row(ui, "Check for updates on start", Some("Asks GitHub for the latest release (no personal data is sent)"), false, |ui| {
                theme::toggle(ui, &mut self.config.check_updates);
            });
            if !installed {
                ui.label(
                    egui::RichText::new(format!("{}  This copy runs without installation (development build).", icons::regular::INFO))
                        .size(12.0)
                        .color(theme::MUTED),
                );
            }
        });
    }
}
