//! Update notice in the top right corner and the confirmation dialog that
//! starts the (checksum-verified) install from the existing update path.

use super::lifecycle::Banner;
use super::theme;
use super::*;
use crate::install::{self, Mode};
use crate::update::{Release, REPO};

/// Release page to open: the one GitHub reported, if it belongs to our repo.
pub(super) fn release_page(r: &Release) -> String {
    let ours = format!("https://github.com/{REPO}/releases/");
    if r.page_url.starts_with(&ours) {
        r.page_url.clone()
    } else {
        format!("https://github.com/{REPO}/releases/latest")
    }
}

impl RustTalkApp {
    /// Small pill for a top bar (right to left layout). Nothing without an update.
    pub(super) fn update_banner(&mut self, ui: &mut egui::Ui) {
        let (text, color, tip) = match self.update.banner() {
            Banner::Hidden => return,
            Banner::Available(v) => (
                format!("{}  Update {v}", icons::regular::ARROW_CIRCLE_UP),
                theme::GREEN,
                format!("RustTalk {v} is available"),
            ),
            Banner::Installing(p) => (
                format!("{}  Updating {p} %", icons::regular::DOWNLOAD_SIMPLE),
                theme::GREEN,
                "Downloading the update".to_string(),
            ),
            Banner::Failed(v) => (
                format!("{}  Update failed", icons::regular::WARNING_CIRCLE),
                theme::RED,
                format!("Installing RustTalk {v} failed - click for details"),
            ),
        };
        let resp = ui
            .add(
                egui::Button::new(egui::RichText::new(text).size(12.5).strong().color(color))
                    .fill(color.gamma_multiply(0.16))
                    .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.5)))
                    .corner_radius(12.0)
                    .min_size(egui::vec2(0.0, 24.0)),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tip);
        if resp.clicked() {
            self.update.open_dialog();
        }
    }

    /// [`Self::update_banner`] floating in the window's top right corner
    /// (views without a header bar).
    pub(super) fn update_banner_overlay(&mut self, ctx: &egui::Context) {
        if self.update.banner() == Banner::Hidden {
            return;
        }
        egui::Area::new(egui::Id::new("update_banner_overlay"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 12.0))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.update_banner(ui)
                });
            });
    }

    /// Confirmation + progress, a modal over whatever page is showing.
    pub(super) fn ui_update_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.update.dialog.clone() else {
            return;
        };
        let Some(release) = self.update.dialog_release().cloned() else {
            // A newer check replaced the release: never install the old confirmation.
            self.update.dialog = None;
            return;
        };
        let installable = install::mode() == Mode::Installed;
        let installing = self.update.installing;
        let mut close = false;
        let mut confirm = false;
        let response = egui::Modal::new(egui::Id::new("update_dialog"))
            .frame(egui::Frame::default().fill(theme::BG()).corner_radius(10.0).inner_margin(egui::Margin::same(20)))
            .backdrop_color(egui::Color32::from_black_alpha(160))
            .show(ctx, |ui| {
                ui.set_width(420.0_f32.min(ctx.screen_rect().width() - 72.0).max(160.0));
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icons::regular::ARROW_CIRCLE_UP).size(22.0).color(theme::GREEN));
                    ui.label(egui::RichText::new("Update RustTalk").size(18.0).strong().color(theme::HEADER()));
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Version {} is available. You have {}.", release.version, env!("CARGO_PKG_VERSION")))
                        .color(theme::TEXT()),
                );
                ui.add_space(4.0);
                if installable {
                    ui.label(
                        egui::RichText::new("RustTalk downloads the update from GitHub, checks its SHA-256 checksum, installs it and restarts.")
                            .color(theme::MUTED()),
                    );
                    let restart = if self.any_connected() {
                        "Restarting disconnects you from the server, including any voice call or screen share."
                    } else {
                        "RustTalk closes and starts again."
                    };
                    ui.label(egui::RichText::new(restart).color(theme::MUTED()));
                } else {
                    ui.label(
                        egui::RichText::new(
                            "This copy runs without installation (development build), so it cannot update itself. \
                             Download the new version from the release page instead.",
                        )
                        .color(theme::MUTED()),
                    );
                }
                if ui.link("Release notes").clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(release_page(&release)));
                }
                if installing {
                    ui.add_space(8.0);
                    let p = self.update.progress.load(std::sync::atomic::Ordering::Relaxed).min(100);
                    ui.add(egui::ProgressBar::new(p as f32 / 100.0).desired_width(ui.available_width()).text(format!("Downloading... {p} %")));
                }
                if let Some(e) = &self.update.install_error {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(format!("Update failed: {e}")).color(theme::RED));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if installable {
                        let label = if self.update.install_error.is_some() { "Try again" } else { "Update and restart" };
                        let ready = !installing && !self.update.checking;
                        ui.add_enabled_ui(ready, |ui| {
                            if theme::primary_button(ui, &format!("{}  {label}", icons::regular::DOWNLOAD_SIMPLE)).clicked() {
                                confirm = true;
                            }
                        });
                    } else if theme::primary_button(ui, &format!("{}  Open release page", icons::regular::ARROW_SQUARE_OUT)).clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(release_page(&release)));
                        close = true;
                    }
                    // The download cannot be cancelled; while it runs this only hides the dialog.
                    if theme::ghost_button(ui, if installing { "Hide" } else { "Cancel" }).clicked() {
                        close = true;
                    }
                });
            });
        if confirm {
            self.start_install(&dialog.version);
        }
        if close || response.should_close() {
            self.update.dialog = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::lifecycle::{UpdateDialog, UpdateState};
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            notes: String::new(),
            page_url: format!("https://github.com/{REPO}/releases/tag/v{version}"),
            exe_url: "https://example.invalid/rusttalk".into(),
            sha_url: Some("https://example.invalid/rusttalk.sha256".into()),
        }
    }

    fn with_update(version: &str) -> UpdateState {
        let mut s = UpdateState::new();
        assert!(s.apply_check(Ok(Some(release(version)))).is_some());
        s
    }

    #[test]
    fn no_update_no_banner_no_dialog() {
        let mut s = UpdateState::new();
        assert_eq!(s.banner(), Banner::Hidden);
        assert!(!s.open_dialog());
        assert_eq!(s.dialog, None);
        assert_eq!(s.begin_install("1.0.0", true), None);
    }

    #[test]
    fn install_needs_confirmation_of_that_version() {
        let mut s = with_update("1.0.0");
        assert_eq!(s.begin_install("1.0.0", true), None, "dialog never opened");
        assert!(s.open_dialog());
        assert_eq!(s.begin_install("0.9.0", true), None, "other version");
        assert!(!s.installing);
        assert_eq!(
            s.begin_install("1.0.0", true).map(|r| r.version),
            Some("1.0.0".into())
        );
        assert!(s.installing);
    }

    #[test]
    fn repeated_confirm_starts_one_install() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        assert!(s.begin_install("1.0.0", true).is_some());
        assert_eq!(s.begin_install("1.0.0", true), None);
        assert_eq!(s.banner(), Banner::Installing(0));
        s.progress.store(250, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(s.banner(), Banner::Installing(100), "progress is clamped");
    }

    #[test]
    fn dev_build_never_installs() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        assert_eq!(s.begin_install("1.0.0", false), None);
        assert!(!s.installing);
        assert_eq!(s.banner(), Banner::Available("1.0.0".into()));
    }

    #[test]
    fn no_install_while_a_check_runs() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        s.checking = true;
        assert_eq!(s.begin_install("1.0.0", true), None);
    }

    #[test]
    fn newer_check_result_invalidates_open_confirmation() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        s.apply_check(Ok(Some(release("1.1.0"))));
        assert_eq!(s.dialog, None);
        assert_eq!(s.begin_install("1.0.0", true), None);
        assert_eq!(
            s.begin_install("1.1.0", true),
            None,
            "new version not confirmed yet"
        );
        assert_eq!(s.banner(), Banner::Available("1.1.0".into()));
    }

    #[test]
    fn stale_dialog_is_not_installable() {
        let mut s = with_update("1.1.0");
        s.dialog = Some(UpdateDialog {
            version: "1.0.0".into(),
        });
        assert_eq!(s.dialog_release(), None);
        assert_eq!(s.begin_install("1.0.0", true), None);
    }

    #[test]
    fn same_version_check_keeps_dialog() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        s.apply_check(Ok(Some(release("1.0.0"))));
        assert_eq!(
            s.dialog,
            Some(UpdateDialog {
                version: "1.0.0".into(),
            })
        );
    }

    #[test]
    fn up_to_date_check_clears_update() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        assert!(s.apply_check(Ok(None)).is_none());
        assert_eq!(s.banner(), Banner::Hidden);
        assert_eq!(s.dialog, None);
        assert!(s.up_to_date);
    }

    #[test]
    fn check_error_keeps_update_and_banner() {
        let mut s = with_update("1.0.0");
        s.checking = true;
        assert!(s.apply_check(Err("offline".into())).is_none());
        assert!(!s.checking);
        assert_eq!(s.error.as_deref(), Some("offline"));
        assert_eq!(
            s.banner(),
            Banner::Available("1.0.0".into()),
            "a check error is not an install failure"
        );
    }

    #[test]
    fn failed_install_can_be_retried() {
        let mut s = with_update("1.0.0");
        s.open_dialog();
        assert!(s.begin_install("1.0.0", true).is_some());
        // What poll_background does on UpdateMsg::Installed(Err(..)).
        s.installing = false;
        s.install_error = Some("checksum mismatch".into());
        assert_eq!(s.banner(), Banner::Failed("1.0.0".into()));
        assert!(s.begin_install("1.0.0", true).is_some());
        assert_eq!(s.install_error, None);
    }

    #[test]
    fn release_page_only_opens_our_repo() {
        let mut r = release("1.0.0");
        assert_eq!(release_page(&r), r.page_url);
        let latest = format!("https://github.com/{REPO}/releases/latest");
        for bad in [
            "",
            "https://evil.example/releases/",
            "https://github.com/someone/else/releases/tag/v1",
            "javascript:alert(1)",
        ] {
            r.page_url = bad.into();
            assert_eq!(release_page(&r), latest, "{bad}");
        }
    }
}
