//! Voice connection popup: ping graph (last 4 minutes), packet loss and a
//! short hint. Opened by clicking "Voice Connected" in the voice panel.

use std::time::Instant;

use super::theme;
use super::*;
use crate::audio::link::{Summary, HISTORY};

const GRAPH_SIZE: egui::Vec2 = egui::vec2(340.0, 130.0);
/// Above this, delay in a conversation is noticeable.
const HIGH_PING_MS: f32 = 250.0;
/// Above this, voices start to sound robotic.
const HIGH_LOSS_PCT: f32 = 10.0;

impl RustTalkApp {
    /// Opens (or closes) the popup, anchored at `at` (bottom-left corner).
    pub(super) fn toggle_connection_panel(&mut self, at: egui::Pos2) {
        self.connection_panel = match self.connection_panel {
            Some(_) => None,
            None => Some(at),
        };
    }

    pub(super) fn ui_connection_panel(&mut self, ctx: &egui::Context) {
        let Some(at) = self.connection_panel else {
            return;
        };
        if self.audio.is_none() || self.in_lobby() {
            self.connection_panel = None;
            return;
        }
        let (points, summary) = self.control.link.view(Instant::now());
        let address = crate::net::normalize_addr(&self.config.server_address);
        let mut open = true;
        egui::Window::new(format!("{}  Voice Connection", icons::regular::CELL_SIGNAL_FULL))
            .id(egui::Id::new("connection_panel"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .default_pos(at)
            .fixed_size([GRAPH_SIZE.x, 0.0])
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(egui::RichText::new(&self.server_settings.name).strong().color(theme::HEADER));
                ui.label(egui::RichText::new(&address).size(12.0).color(theme::MUTED));
                ping_graph(ui, &points);
                if let Some(secs) = summary.stale_secs {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  No answer from the server for {secs:.0} s - voice traffic may be blocked",
                            icons::regular::WARNING
                        ))
                        .color(theme::YELLOW),
                    );
                }
                stats_grid(ui, &summary);
                ui.label(
                    egui::RichText::new(format!(
                        "A ping of {HIGH_PING_MS:.0} ms or more makes the delay noticeable. Packet loss above \
                         {HIGH_LOSS_PCT:.0} % makes voices sound robotic. If this persists, try reconnecting."
                    ))
                    .size(11.5)
                    .color(theme::MUTED),
                );
                encryption_badge(ui);
            });
        if !open {
            self.connection_panel = None;
        }
    }
}

fn fmt_ms(v: Option<f32>) -> String {
    v.map_or("-".to_string(), |v| format!("{v:.0} ms"))
}

fn fmt_pct(v: Option<f32>) -> String {
    v.map_or("-".to_string(), |v| format!("{v:.1} %"))
}

fn stats_grid(ui: &mut egui::Ui, s: &Summary) {
    let warn = |bad: bool| if bad { theme::YELLOW } else { theme::TEXT };
    let rows = [
        (
            "Average ping",
            fmt_ms(s.average_ms),
            warn(s.average_ms.is_some_and(|v| v >= HIGH_PING_MS)),
        ),
        (
            "Last ping",
            fmt_ms(s.last_ms),
            warn(s.last_ms.is_some_and(|v| v >= HIGH_PING_MS)),
        ),
        ("Jitter", fmt_ms(s.jitter_ms), theme::TEXT),
        (
            "Outbound packet loss",
            fmt_pct(s.outbound_loss),
            warn(s.outbound_loss.is_some_and(|v| v > HIGH_LOSS_PCT)),
        ),
        (
            "Inbound packet loss",
            fmt_pct(s.inbound_loss),
            warn(s.inbound_loss.is_some_and(|v| v > HIGH_LOSS_PCT)),
        ),
    ];
    egui::Grid::new("connection_stats")
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            for (label, value, color) in rows {
                ui.label(egui::RichText::new(label).color(theme::MUTED));
                ui.label(egui::RichText::new(value).strong().color(color));
                ui.end_row();
            }
        });
}

fn encryption_badge(ui: &mut egui::Ui) {
    let text = egui::RichText::new(format!(
        "{}  Encrypted (ChaCha20-Poly1305)",
        icons::regular::LOCK
    ))
    .size(11.5)
    .color(theme::GREEN);
    egui::Frame::new()
        .fill(theme::DARK)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| ui.label(text))
        .response
        .on_hover_text("Voice, screen audio and pings travel encrypted with a per-session key sent over the pinned TLS connection.");
}

/// Upper bound of the y axis: a round number above the highest ping.
fn y_max(points: &[(f32, f32)]) -> f32 {
    let top = points.iter().map(|p| p.1).fold(0.0f32, f32::max).max(50.0);
    let step = if top > 400.0 {
        200.0
    } else if top > 100.0 {
        100.0
    } else {
        25.0
    };
    (top / step).ceil() * step
}

/// Ping over [`HISTORY`]: x = seconds ago (right = now), y = ms.
fn ping_graph(ui: &mut egui::Ui, points: &[(f32, f32)]) {
    let (rect, _) = ui.allocate_exact_size(GRAPH_SIZE, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::DARK);
    let font = egui::FontId::proportional(10.0);
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(38.0, 8.0),
        rect.max - egui::vec2(8.0, 18.0),
    );
    let span = HISTORY.as_secs_f32();
    let top = y_max(points);
    let to_screen = |age: f32, ms: f32| {
        egui::pos2(
            plot.right() - (age / span).clamp(0.0, 1.0) * plot.width(),
            plot.bottom() - (ms / top).clamp(0.0, 1.0) * plot.height(),
        )
    };
    for i in 0..=4 {
        let ms = top * i as f32 / 4.0;
        let y = to_screen(0.0, ms).y;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0_f32, theme::BORDER),
        );
        painter.text(
            egui::pos2(plot.left() - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{ms:.0}"),
            font.clone(),
            theme::MUTED,
        );
    }
    painter.text(
        egui::pos2(rect.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        "ms",
        font.clone(),
        theme::DISABLED,
    );
    for min in 0..=4u32 {
        let x = to_screen(min as f32 * 60.0, 0.0).x;
        let label = if min == 0 {
            "now".to_string()
        } else {
            format!("-{min} min")
        };
        painter.text(
            egui::pos2(x, plot.bottom() + 3.0),
            egui::Align2::CENTER_TOP,
            label,
            font.clone(),
            theme::MUTED,
        );
    }
    if top >= HIGH_PING_MS {
        let y = to_screen(0.0, HIGH_PING_MS).y;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0_f32, theme::YELLOW.gamma_multiply(0.5)),
        );
    }
    if points.is_empty() {
        painter.text(
            plot.center(),
            egui::Align2::CENTER_CENTER,
            "Measuring...",
            egui::FontId::proportional(12.0),
            theme::MUTED,
        );
        return;
    }
    let line: Vec<egui::Pos2> = points.iter().map(|&(age, ms)| to_screen(age, ms)).collect();
    painter.add(egui::Shape::line(
        line.clone(),
        egui::Stroke::new(2.0_f32, theme::ACCENT),
    ));
    if let Some(&last) = line.last() {
        painter.circle_filled(last, 3.0, theme::ACCENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn y_axis_rounds_up() {
        assert_eq!(y_max(&[]), 50.0);
        assert_eq!(y_max(&[(0.0, 30.0)]), 50.0);
        assert_eq!(y_max(&[(0.0, 130.0)]), 200.0);
        assert_eq!(y_max(&[(0.0, 900.0)]), 1000.0);
    }
}
