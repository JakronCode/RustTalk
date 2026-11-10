//! Prozedural generiertes RustTalk-Logo (keine Asset-Datei noetig):
//! abgerundetes Quadrat in Akzentfarbe mit weisser Klangwellen-Form.

use eframe::egui;

pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x58, 0x65, 0xF2);
const ACCENT_DARK: egui::Color32 = egui::Color32::from_rgb(0x40, 0x4A, 0xC8);

/// Balkenhoehen der Klangwelle (relativ zur Kantenlaenge), 5 Balken.
const BARS: [f32; 5] = [0.30, 0.52, 0.78, 0.52, 0.30];

/// Malt das Logo in die UI (Connect-Screen etc.).
pub fn paint_logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, size * 0.22, ACCENT);
    let bw = size * 0.095;
    let gap = size * 0.05;
    let total_w = BARS.len() as f32 * bw + (BARS.len() - 1) as f32 * gap;
    let mut x = rect.left() + (size - total_w) / 2.0;
    for &h in &BARS {
        let bh = size * h;
        let bar = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - bh / 2.0),
            egui::vec2(bw, bh),
        );
        p.rect_filled(bar, bw / 2.0, egui::Color32::WHITE);
        x += bw + gap;
    }
}

/// Fenster-Icon (64x64 RGBA, prozedural gerendert).
pub fn window_icon() -> egui::IconData {
    const N: usize = 64;
    let size = N as f32;
    let mut rgba = vec![0u8; N * N * 4];

    let bw = size * 0.095;
    let gap = size * 0.05;
    let total_w = BARS.len() as f32 * bw + (BARS.len() - 1) as f32 * gap;
    let bar_left = (size - total_w) / 2.0;

    for y in 0..N {
        for x in 0..N {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let mut color = [0u8; 4];
            // Abgerundetes Grundquadrat mit vertikalem Farbverlauf.
            if in_rounded_rect(px, py, 0.0, 0.0, size, size, size * 0.22) {
                let t = py / size;
                color = lerp_rgba(ACCENT, ACCENT_DARK, t);
                // Klangwellen-Balken.
                let mut bx = bar_left;
                for &h in &BARS {
                    let bh = size * h;
                    let top = (size - bh) / 2.0;
                    if in_rounded_rect(px, py, bx, top, bw, bh, bw / 2.0) {
                        color = [255, 255, 255, 255];
                        break;
                    }
                    bx += bw + gap;
                }
            }
            let i = (y * N + x) * 4;
            rgba[i..i + 4].copy_from_slice(&color);
        }
    }
    egui::IconData { rgba, width: N as u32, height: N as u32 }
}

/// Punkt-in-abgerundetem-Rechteck-Test.
fn in_rounded_rect(px: f32, py: f32, rx: f32, ry: f32, rw: f32, rh: f32, corner: f32) -> bool {
    if px < rx || px > rx + rw || py < ry || py > ry + rh {
        return false;
    }
    // Abstand zur naechsten inneren Ecke pruefen (nur in Eck-Zonen relevant).
    let cx = (px - rx - rw / 2.0).signum();
    let cy = (py - ry - rh / 2.0).signum();
    let corner_center_x = rx + rw / 2.0 + cx * (rw / 2.0 - corner);
    let corner_center_y = ry + rh / 2.0 + cy * (rh / 2.0 - corner);
    let dx = px - corner_center_x;
    let dy = py - corner_center_y;
    // Nur wenn der Punkt ausserhalb des achsparallelen Kernbereichs liegt,
    // muss er innerhalb des Ecken-Kreises sein.
    let in_core_x = (rx + corner..=rx + rw - corner).contains(&px);
    let in_core_y = (ry + corner..=ry + rh - corner).contains(&py);
    if in_core_x || in_core_y {
        return true;
    }
    dx * dx + dy * dy <= corner * corner
}

fn lerp_rgba(a: egui::Color32, b: egui::Color32, t: f32) -> [u8; 4] {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_correct_size_and_content() {
        let icon = window_icon();
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        // Mitte ist weiss (mittlerer Balken), Ecke transparent.
        let center = &icon.rgba[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4];
        assert_eq!(center, &[255, 255, 255, 255]);
        let corner = &icon.rgba[0..4];
        assert_eq!(corner[3], 0);
        // Akzentpixel existieren (oben links im Quadrat).
        let accent = &icon.rgba[(5 * 64 + 10) * 4..(5 * 64 + 10) * 4 + 4];
        assert_eq!(accent[3], 255);
        assert!(accent[2] > 150, "Akzent hat Blauanteil");
    }

    #[test]
    fn rounded_rect_geometry() {
        assert!(in_rounded_rect(10.0, 10.0, 0.0, 0.0, 100.0, 100.0, 20.0));
        assert!(!in_rounded_rect(2.0, 2.0, 0.0, 0.0, 100.0, 100.0, 20.0), "Ecke ausserhalb");
        assert!(!in_rounded_rect(101.0, 50.0, 0.0, 0.0, 100.0, 100.0, 20.0), "ausserhalb");
    }
}
