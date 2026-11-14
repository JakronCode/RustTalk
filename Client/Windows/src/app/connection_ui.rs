//! Voice connection popup: bounded ping and payload bandwidth graphs,
//! packet loss and call totals. Opened by clicking "Voice Connected" in the voice panel.

use std::time::Instant;

use super::theme;
use super::*;
use crate::audio::link::{Summary, HISTORY};

/// Preferred width of the popup; shrinks to fit a small window.
const PANEL_WIDTH: f32 = 420.0;
const PING_GRAPH_HEIGHT: f32 = 110.0;
const BANDWIDTH_GRAPH_HEIGHT: f32 = 84.0;
/// Above this, delay in a conversation is noticeable.
const HIGH_PING_MS: f32 = 250.0;
/// Above this, voices start to sound robotic.
const HIGH_LOSS_PCT: f32 = 10.0;

impl RustTalkApp {
    pub(super) fn bandwidth_counts(&self) -> Option<crate::bandwidth::Counts> {
        let id = self.voice_owner?;
        let server = self
            .session(id)
            .filter(|s| s.connected && s.generation == self.voice_generation)?;
        let tcp = &server.cmd_tx.as_ref()?.traffic;
        Some(tcp.snapshot())
    }

    pub(super) fn sample_bandwidth(&mut self, now: Instant) {
        let Some(counts) = self.bandwidth_counts() else {
            self.bandwidth = None;
            return;
        };
        if let Some(call) = &mut self.bandwidth {
            call.sample(now, counts);
        }
    }

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
        // The audio link belongs to the voice server.
        let Some((address, name)) = self.voice().map(|s| (s.key.clone(), s.server_settings.name.clone())) else {
            self.connection_panel = None;
            return;
        };
        if self.audio.is_none() {
            self.connection_panel = None;
            return;
        }
        let (points, summary) = self.control.link.view(Instant::now());
        let mut open = true;
        let size = theme::dialog_size(ctx, egui::vec2(PANEL_WIDTH, f32::MAX));
        // Window chrome (margins + header) that the scroll area must leave free.
        let body_max = (size.y - 64.0).max(120.0);
        theme::dialog_window("connection_panel")
            .pivot(egui::Align2::LEFT_BOTTOM)
            .default_pos(at)
            // Not `fixed_size([w, 0.0])`: that pins the height to 0 and left only a sliver.
            .min_width(size.x)
            .max_width(size.x)
            .show(ctx, |ui| {
                theme::dialog_header(ui, &format!("{}  Voice Connection", icons::regular::CELL_SIGNAL_FULL), &mut open);
                // Height is left to the content (up to the window), only the body scrolls.
                egui::ScrollArea::vertical().max_height(body_max).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.label(egui::RichText::new(&name).strong().color(theme::HEADER));
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
                    if let Some(call) = &self.bandwidth {
                        bandwidth_ui(ui, call);
                    }
                    encryption_badge(ui);
                });
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
        .num_columns(4)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            for (i, (label, value, color)) in rows.into_iter().enumerate() {
                ui.label(egui::RichText::new(label).color(theme::MUTED));
                ui.label(egui::RichText::new(value).strong().color(color));
                if i % 2 == 1 {
                    ui.end_row();
                }
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
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), PING_GRAPH_HEIGHT), egui::Sense::hover());
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
        egui::Stroke::new(2.0_f32, theme::accent()),
    ));
    if let Some(&last) = line.last() {
        painter.circle_filled(last, 3.0, theme::accent());
    }
}

fn bandwidth_ui(ui: &mut egui::Ui, call: &crate::bandwidth::Call) {
    let now = Instant::now();
    ui.separator();
    ui.label(egui::RichText::new("Application payload bandwidth").strong());
    for (direction, name) in [
        (crate::bandwidth::IN, "Incoming"),
        (crate::bandwidth::OUT, "Outgoing"),
    ] {
        let rate: f64 = call.rates[direction].iter().sum();
        let total = call.totals[direction]
            .iter()
            .fold(0u64, |sum, n| sum.saturating_add(*n));
        ui.label(format!(
            "{name}: {:.1} KiB/s · {:.2} MiB since join",
            rate / 1024.,
            total as f64 / 1_048_576.
        ));
        bandwidth_graph(ui, call, direction, now);
    }
    ui.horizontal_wrapped(|ui| {
        for (name, color) in bandwidth_legend() {
            ui.colored_label(color, name);
        }
    });
    ui.label(egui::RichText::new("Media payloads + Other (serialized chat/control and UDP control). Excludes framing, encryption, IP/TCP/UDP overhead, retransmits, incomplete TCP frames and rejected UDP packets. Local only.").size(11.5).color(theme::MUTED));
}

fn bandwidth_legend() -> [(&'static str, egui::Color32); 5] {
    [
        ("Screen", theme::GREEN),
        ("Camera", theme::YELLOW),
        ("Microphone", egui::Color32::from_rgb(100, 180, 255)),
        ("Shared audio", egui::Color32::from_rgb(220, 130, 230)),
        ("Other", theme::MUTED),
    ]
}

fn bandwidth_graph(
    ui: &mut egui::Ui,
    call: &crate::bandwidth::Call,
    direction: usize,
    now: Instant,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), BANDWIDTH_GRAPH_HEIGHT), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4., theme::DARK);
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(45., 15.),
        rect.max - egui::vec2(8., 18.),
    );
    let top = call
        .history
        .iter()
        .flat_map(|(_, rates)| rates[direction])
        .fold(1024_f64, f64::max);
    let font = egui::FontId::proportional(10.);
    for i in 0..=2 {
        let y = plot.bottom() - plot.height() * i as f32 / 2.;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0_f32, theme::BORDER),
        );
        painter.text(
            egui::pos2(plot.left() - 4., y),
            egui::Align2::RIGHT_CENTER,
            format!("{:.1}", top / 1024. * i as f64 / 2.),
            font.clone(),
            theme::MUTED,
        );
    }
    painter.text(
        rect.min + egui::vec2(4., 2.),
        egui::Align2::LEFT_TOP,
        "KiB/s",
        font.clone(),
        theme::MUTED,
    );
    for minute in 0..=4 {
        painter.text(
            egui::pos2(
                plot.right() - minute as f32 / 4. * plot.width(),
                plot.bottom() + 3.,
            ),
            egui::Align2::CENTER_TOP,
            if minute == 0 {
                "now".into()
            } else {
                format!("-{minute} min")
            },
            font.clone(),
            theme::MUTED,
        );
    }
    for (media, (_, color)) in bandwidth_legend().iter().enumerate() {
        let points: Vec<_> = call
            .history
            .iter()
            .filter_map(|(at, rates)| {
                let age = now.saturating_duration_since(*at).as_secs_f32();
                (age <= 240.).then(|| {
                    egui::pos2(
                        plot.right() - age / 240. * plot.width(),
                        plot.bottom() - (rates[direction][media] / top) as f32 * plot.height(),
                    )
                })
            })
            .collect();
        if let Some(last) = points.last() {
            painter.circle_filled(*last, 2., *color);
        }
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5_f32, *color),
        ));
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
