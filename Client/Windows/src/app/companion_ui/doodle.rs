//! The "doodle" style: flat colors without outlines, simple faces drawn
//! with a thick pen. The dog
//! peeks over an edge with its paws on it - above the user panel that edge
//! is the panel itself.
//!
//! Five breeds, picked by the companion's coat (which comes from the user's
//! identity): beagle, shiba, corgi, spaniel and a fluffy cream one.

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Shape, Stroke};

use super::{bone, group, heart, rgb, tint, Blob, Grid, Look};
use crate::companion::Emotion;

/// Distance of the edge the paws rest on from the top of the box (0..1).
pub const LEDGE: f32 = 0.86;

#[derive(Clone, Copy, PartialEq)]
enum Breed {
    Beagle,
    Shiba,
    Corgi,
    Spaniel,
    Fluffy,
}

const BREEDS: [Breed; 5] = [Breed::Beagle, Breed::Shiba, Breed::Corgi, Breed::Spaniel, Breed::Fluffy];

/// Name of the breed a coat number gives (for labels).
pub fn breed_name(coat: u8) -> &'static str {
    match BREEDS[coat as usize % BREEDS.len()] {
        Breed::Beagle => "Beagle",
        Breed::Shiba => "Shiba",
        Breed::Corgi => "Corgi",
        Breed::Spaniel => "Spaniel",
        Breed::Fluffy => "Fluffy pup",
    }
}

/// Draws the dog into `rect`. With `ledge`, the edge it leans on is drawn
/// too (where there is no panel edge to lean on).
pub fn paint(p: &egui::Painter, rect: egui::Rect, coat: u8, collar: Color32, genes: u32, l: &Look, ledge: bool) {
    let breed = BREEDS[coat as usize % BREEDS.len()];
    let e = l.emotion;
    let t = l.now;
    let ink = rgb(0x1C1512);
    let white = rgb(0xFFFDF8);

    let s = rect.width() / 100.0;
    let w = (2.0 * s).max(1.5); // the thick doodle line
    let pen = Stroke::new(w, ink);
    // No outlines: shapes are told apart by color and a shadow tone.
    let edge = Color32::TRANSPARENT;
    let fine = Stroke::new((1.5 * s).max(1.2), ink);
    let ledge_y = rect.top() + rect.height() * LEDGE;

    // Body language.
    let bounce = match e {
        Emotion::Happy => (t * 10.0).sin().abs() * 3.0,
        Emotion::Petted => (t * 6.0).sin() * 1.0,
        _ => 0.0,
    };
    let sink = match e {
        Emotion::Asleep => 9.0,
        Emotion::Sleepy => 4.0,
        Emotion::Sad => 3.0,
        _ => 0.0,
    };
    let lean = match e {
        Emotion::Full => -4.0,
        Emotion::Hungry => (t * 1.6).sin() * 1.5,
        _ => 0.0,
    };
    let ear_flop = match e {
        Emotion::Happy | Emotion::Petted => (t * 12.0).sin() * 1.6,
        Emotion::Angry | Emotion::Sad => -2.0,
        _ => 0.0,
    };

    // Head grid: origin in the middle of the face.
    let g = Grid { o: pos2(rect.center().x + lean * s, ledge_y - (33.0 + bounce - sink) * s), s };
    let fill = |pts: Vec<Pos2>, c: Color32| {
        p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
    };
    // Mirrors a list of circles to the other side.
    let side_pts = |side: f32, pts: &[(f32, f32, f32)]| -> Vec<Pos2> { g.soft(&pts.iter().map(|&(x, y, r)| (side * x, y, r)).collect::<Vec<_>>()) };

    // Colors per breed: every color has a range (hue in degrees, saturation,
    // brightness) and the genes pick a spot in it, so no two look the same.
    let fur = |n: u32, base: u32, hue: f32, sat: f32, val: f32| vary(genes, n, base, hue, sat, val);
    // White fur: from paper white to cream.
    let white = mix(white, rgb(0xF3E2C6), roll(genes, 90) * 0.7);
    // (head, ears, paws, patch)
    let (head_c, ear_c, paw_c, patch_c) = match breed {
        Breed::Beagle => (fur(0, 0xC97B4E, 12.0, 0.22, 0.14), fur(3, 0x7C4330, 10.0, 0.2, 0.2), white, white),
        Breed::Shiba => {
            let c = fur(0, 0xF2A93B, 14.0, 0.18, 0.1);
            (c, c, white, white)
        }
        Breed::Corgi => {
            let c = fur(0, 0xF28C28, 12.0, 0.2, 0.12);
            (c, c, white, white)
        }
        Breed::Spaniel => {
            let patch = fur(0, 0x8C5E3C, 14.0, 0.3, 0.2);
            (white, mix(patch, rgb(0x2A1810), 0.35 + roll(genes, 3) * 0.3), patch, patch)
        }
        Breed::Fluffy => {
            let c = fur(0, 0xF8E3A8, 12.0, 0.45, 0.06);
            (c, mix(c, fur(3, 0xC98A4B, 10.0, 0.2, 0.15), 0.75), c, mix(c, rgb(0xC98A4B), 0.3))
        }
    };
    // Inside of upright ears: the coat, a good bit lighter.
    let inner_c = mix(ear_c, white, 0.5);

    // Shoulders behind the edge, with the collar. Clipped at the edge.
    {
        let clip = egui::Rect::from_min_max(p.clip_rect().min, pos2(p.clip_rect().max.x, ledge_y));
        let pc = p.with_clip_rect(clip.intersect(p.clip_rect()));
        let body_c = if breed == Breed::Spaniel { patch_c } else { head_c };
        let body = egui::Rect::from_min_max(pos2(g.o.x - 21.0 * s, ledge_y - 16.0 * s), pos2(g.o.x + 21.0 * s, ledge_y + 10.0 * s));
        pc.rect_filled(body, 12.0 * s, tint(body_c, 0.9));
        if !matches!(breed, Breed::Spaniel | Breed::Fluffy) {
            pc.rect_filled(body.shrink2(vec2(8.0 * s, 0.0)), 8.0 * s, tint(white, 0.88));
        }
        let neck = ledge_y - 10.0 * s;
        let band = vec![pos2(g.o.x - 20.0 * s, neck - 2.0 * s), pos2(g.o.x, neck + 2.0 * s), pos2(g.o.x + 20.0 * s, neck - 2.0 * s)];
        pc.add(Shape::line(band, Stroke::new(5.0 * s, collar)));
        pc.circle_filled(pos2(g.o.x, neck + 6.0 * s), 3.4 * s, rgb(0xF2C94C));
        pc.circle_filled(pos2(g.o.x - 1.0 * s, neck + 5.0 * s), 1.0 * s, Color32::from_white_alpha(200));
    }

    // Upright ears go behind the head.
    match breed {
        Breed::Shiba => {
            for side in [-1.0f32, 1.0] {
                group(p, &[(Blob::Poly(side_pts(side, &[(9.0, -17.0, 3.0), (23.0, -12.0, 3.0), (20.5 + ear_flop * 0.3, -29.0, 2.2)])), ear_c)], edge, 0.0);
                fill(side_pts(side, &[(13.5, -17.0, 1.6), (20.0, -15.0, 1.6), (19.0, -23.5, 1.2)]), inner_c);
            }
        }
        Breed::Corgi => {
            for side in [-1.0f32, 1.0] {
                group(p, &[(Blob::Poly(side_pts(side, &[(6.0, -15.0, 4.0), (24.0, -9.0, 4.0), (28.0 + ear_flop, -37.0, 3.6)])), ear_c)], edge, 0.0);
                fill(side_pts(side, &[(12.0, -15.0, 2.2), (21.5, -12.0, 2.2), (24.5 + ear_flop, -30.0, 2.0)]), inner_c);
            }
        }
        _ => {}
    }

    // Head.
    let head: Vec<(Blob, Color32)> = match breed {
        Breed::Beagle => vec![(Blob::Poly(g.soft(&[(0.0, -5.0, 20.0), (-10.0, 10.0, 14.0), (10.0, 10.0, 14.0)])), head_c)],
        Breed::Shiba => vec![(Blob::Poly(g.soft(&[(-11.0, -4.0, 15.0), (11.0, -4.0, 15.0), (-14.0, 10.0, 13.0), (14.0, 10.0, 13.0)])), head_c)],
        Breed::Corgi => vec![(Blob::Poly(g.soft(&[(0.0, -6.0, 18.0), (-10.0, 10.0, 13.5), (10.0, 10.0, 13.5)])), head_c)],
        Breed::Spaniel => vec![(Blob::Poly(g.soft(&[(0.0, -6.0, 18.5), (-8.0, 11.0, 13.0), (8.0, 11.0, 13.0)])), head_c)],
        // A cloud of curls.
        Breed::Fluffy => [(0.0, 0.0, 19.0), (-12.0, -14.0, 8.5), (0.0, -19.0, 9.0), (12.0, -14.0, 8.5), (-20.0, -3.0, 8.0), (20.0, -3.0, 8.0), (-16.0, 11.0, 9.5), (16.0, 11.0, 9.5), (0.0, 15.0, 10.5)]
            .iter()
            .map(|&(x, y, r)| (Blob::Circle(g.p(x, y), g.r(r)), head_c))
            .collect(),
    };
    group(p, &head, edge, 0.0);

    // Patches inside the head (no lines).
    match breed {
        Breed::Beagle => {
            fill(g.soft(&[(0.0, -22.0, 2.4), (0.0, -6.0, 4.5)]), patch_c);
            fill(g.soft(&[(-8.5, 11.0, 9.5), (8.5, 11.0, 9.5), (0.0, 3.0, 7.5)]), patch_c);
        }
        Breed::Shiba => {
            fill(g.soft(&[(-14.0, 10.0, 10.0), (14.0, 10.0, 10.0), (0.0, 4.0, 8.5)]), patch_c);
            for side in [-1.0f32, 1.0] {
                fill(side_pts(side, &[(7.5, -10.5, 2.3), (11.0, -11.5, 2.0)]), patch_c);
            }
        }
        Breed::Corgi => {
            fill(g.soft(&[(0.0, -22.0, 2.0), (0.0, -6.0, 5.0)]), patch_c);
            fill(g.soft(&[(-9.5, 11.0, 10.0), (9.5, 11.0, 10.0), (0.0, 2.0, 8.0)]), patch_c);
        }
        Breed::Spaniel => {
            // Brown over both eyes, a white blaze between; freckles later.
            for side in [-1.0f32, 1.0] {
                fill(side_pts(side, &[(9.5, -8.0, 8.0), (11.5, -1.0, 6.0), (8.0, -17.0, 5.0)]), patch_c);
            }
        }
        Breed::Fluffy => {
            fill(g.soft(&[(-3.0, 8.5, 7.0), (3.0, 8.5, 7.0)]), patch_c);
        }
    }

    // Floppy ears hang in front of the head.
    let floppy: Option<[(f32, f32, f32); 3]> = match breed {
        Breed::Beagle => Some([(16.0, -15.0, 6.0), (25.0, 0.0, 8.5), (22.0, 15.0, 6.5)]),
        Breed::Spaniel => Some([(15.0, -14.0, 6.0), (25.0, 4.0, 9.0), (23.0, 22.0, 7.0)]),
        Breed::Fluffy => Some([(19.0, -9.0, 5.5), (27.5, 0.0, 6.5), (27.0, 9.0, 6.0)]),
        _ => None,
    };
    if let Some(ear) = floppy {
        for side in [-1.0f32, 1.0] {
            let pts: Vec<(f32, f32, f32)> = ear.iter().enumerate().map(|(i, &(x, y, r))| (x + if i > 0 { ear_flop * side * 0.6 } else { 0.0 }, y, r)).collect();
            group(p, &[(Blob::Poly(side_pts(side, &pts)), ear_c)], edge, 0.0);
        }
    }

    // Blush marks when it's pleased.
    if matches!(e, Emotion::Happy | Emotion::Petted) {
        for side in [-1.0f32, 1.0] {
            p.add(Shape::ellipse_filled(g.p(side * 14.5, 6.5), g.v(4.0, 2.4), Color32::from_rgba_unmultiplied(0xFF, 0x7E, 0x96, 150)));
        }
    }

    // Eyes: dots, arcs and lines.
    let arc = |c: Pos2, wdt: f32, hgt: f32, up: bool| -> Vec<Pos2> {
        (0..=8)
            .map(|i| {
                let a = std::f32::consts::PI * i as f32 / 8.0;
                c + g.v(-wdt * a.cos(), if up { -hgt * a.sin() } else { hgt * a.sin() })
            })
            .collect()
    };
    for side in [-1.0f32, 1.0] {
        let c = g.p(side * 8.5, -2.0);
        match e {
            Emotion::Happy | Emotion::Eating => {
                p.add(Shape::line(arc(c + g.v(0.0, 1.5), 3.4, 3.0, true), pen));
            }
            Emotion::Petted | Emotion::Asleep => {
                p.add(Shape::line(arc(c + g.v(0.0, -0.5), 3.4, 2.4, false), pen));
            }
            Emotion::Sleepy => {
                p.line_segment([c + g.v(-3.2, 0.0), c + g.v(3.2, 0.0)], pen);
            }
            _ if l.blink => {
                p.line_segment([c + g.v(-2.8, 0.2), c + g.v(2.8, 0.2)], pen);
            }
            Emotion::Hungry => {
                p.circle_filled(c, g.r(3.3), ink);
                p.circle_filled(c + g.v(-1.0, -1.2), g.r(1.3), Color32::WHITE);
            }
            Emotion::Angry => {
                p.circle_filled(c + g.v(0.0, 1.0), g.r(2.0), ink);
                p.line_segment([c + g.v(-side * 4.5, -5.0), c + g.v(side * 3.5, -2.0)], pen);
            }
            Emotion::Full => {
                // One eye shut, one looking away.
                if side < 0.0 {
                    p.add(Shape::line(vec![c + g.v(-3.0, -2.0), c + g.v(1.5, 0.0), c + g.v(-3.0, 2.0)], pen));
                } else {
                    p.circle_filled(c + g.v(-1.0, 0.0), g.r(2.0), ink);
                }
            }
            Emotion::Sad => {
                // Big wet eyes under worried brows.
                p.circle_filled(c + g.v(0.0, 0.6), g.r(2.6), ink);
                p.circle_filled(c + g.v(-0.8, -0.4), g.r(1.0), Color32::WHITE);
                p.line_segment([c + g.v(-side * 3.0, -6.2), c + g.v(side * 3.6, -4.2)], fine);
            }
            Emotion::Content => {
                p.circle_filled(c, g.r(2.2), ink);
                p.circle_filled(c + g.v(-0.7, -0.8), g.r(0.7), Color32::WHITE);
            }
        }
    }

    // Nose.
    fill(g.soft(&[(-2.4, 5.0, 2.2), (2.4, 5.0, 2.2), (0.0, 7.0, 1.9)]), ink);
    p.circle_filled(g.p(-1.0, 4.5), g.r(0.7), Color32::from_white_alpha(170));

    // Mouth.
    let smile = |p: &egui::Painter| {
        p.line_segment([g.p(0.0, 8.6), g.p(0.0, 10.4)], fine);
        for side in [-1.0f32, 1.0] {
            let pts: Vec<Pos2> = (0..=6)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 6.0;
                    g.p(side * 3.2 - 3.2 * a.cos(), 10.4 + 2.6 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, fine));
        }
    };
    let tongue = |p: &egui::Painter, len: f32| {
        p.add(Shape::convex_polygon(g.soft(&[(-1.5, 12.5, 2.2), (1.5, 12.5, 2.2), (0.6, 12.5 + len, 2.6)]), rgb(0xF27E93), Stroke::NONE));
    };
    match e {
        Emotion::Happy => {
            tongue(p, 5.0 + (t * 10.0).sin());
            smile(p);
        }
        Emotion::Petted => {
            tongue(p, 3.0);
            smile(p);
        }
        Emotion::Eating => {
            if (t * 13.0).sin() > 0.0 {
                p.add(Shape::ellipse_filled(g.p(0.0, 12.0), g.v(3.2, 2.6), rgb(0x7A2A35)));
                p.add(Shape::ellipse_stroke(g.p(0.0, 12.0), g.v(3.2, 2.6), fine));
            } else {
                smile(p);
            }
        }
        Emotion::Angry => {
            p.add(Shape::line(vec![g.p(-5.0, 12.5), g.p(-2.5, 10.6), g.p(0.0, 12.5), g.p(2.5, 10.6), g.p(5.0, 12.5)], fine));
        }
        Emotion::Sleepy if (t % 9.0) < 1.5 => {
            p.add(Shape::ellipse_filled(g.p(0.0, 12.5), g.v(2.8, 3.4), rgb(0x7A2A35)));
            p.add(Shape::ellipse_stroke(g.p(0.0, 12.5), g.v(2.8, 3.4), fine));
        }
        Emotion::Full => {
            p.add(Shape::line(vec![g.p(-4.0, 11.6), g.p(0.0, 10.6), g.p(4.0, 11.2)], fine));
        }
        Emotion::Sad => {
            // A little frown.
            let pts: Vec<Pos2> = (0..=6)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 6.0;
                    g.p(-3.4 * a.cos(), 12.4 - 1.8 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, fine));
        }
        Emotion::Hungry => {
            smile(p);
            // A drop of drool.
            let d = (t * 1.2) % 1.0;
            p.add(Shape::convex_polygon(g.soft(&[(4.5, 13.0 + d * 4.0, 0.6), (4.5, 15.0 + d * 4.0, 1.3)]), rgb(0xBFE3F7), Stroke::NONE));
        }
        _ => smile(p),
    }
    // Freckles.
    if breed == Breed::Spaniel {
        for (x, y) in [(-5.5f32, 9.0f32), (-7.0, 11.5), (5.5, 9.0), (7.0, 11.5)] {
            p.circle_filled(g.p(x, y), g.r(0.6), patch_c);
        }
    }
    if breed == Breed::Shiba || breed == Breed::Corgi {
        for (x, y) in [(-2.5f32, 17.5f32), (2.0, 18.5)] {
            p.line_segment([g.p(x, y), g.p(x + 1.2, y + 1.6)], fine);
        }
    }

    // The edge, then the paws resting on it.
    if ledge {
        p.line_segment([pos2(rect.left(), ledge_y), pos2(rect.right(), ledge_y)], Stroke::new(w, theme_edge()));
    }
    let paw_grid = Grid { o: pos2(rect.center().x + lean * s * 0.3, ledge_y), s };
    for side in [-1.0f32, 1.0] {
        let lift = if e == Emotion::Happy && side > 0.0 { (t * 10.0).sin().abs() * 2.0 } else { 0.0 };
        let x = side * 17.0;
        let pts: Vec<Pos2> = paw_grid.soft(&[(x - 3.6, -1.5 - lift, 6.0), (x + 3.6, -1.5 - lift, 6.0), (x, 3.5 - lift, 6.0)]);
        group(p, &[(Blob::Poly(pts), paw_c)], edge, 0.0);
        for dx in [-2.6f32, 2.6] {
            p.line_segment([paw_grid.p(x + dx, 3.2 - lift), paw_grid.p(x + dx, 8.0 - lift)], fine);
        }
    }

    // Extras around it.
    match e {
        Emotion::Petted | Emotion::Happy => {
            for (i, (x, delay)) in [(-34.0f32, 0.0f32), (33.0, 0.5)].into_iter().enumerate() {
                let phase = ((t + delay) % 1.4) / 1.4;
                let c = g.p(x + (t * 3.0 + i as f32).sin() * 2.0, -22.0 - phase * 12.0);
                heart(p, c, g.r(4.2), rgb(0xFF6FAE).gamma_multiply(1.0 - phase));
            }
        }
        Emotion::Asleep => {
            for i in 0..3 {
                let phase = ((t * 0.5 + i as f32 / 3.0) % 1.0).clamp(0.0, 1.0);
                let pos = g.p(26.0 + phase * 9.0, -22.0 - phase * 14.0);
                let alpha = (1.0 - phase) * (phase * 4.0).min(1.0);
                p.text(pos, egui::Align2::CENTER_CENTER, "z", egui::FontId::proportional(g.r(9.0 + phase * 6.0)), super::theme::MUTED.gamma_multiply(alpha));
            }
        }
        Emotion::Angry => {
            let c = g.p(26.0, -24.0);
            let st = Stroke::new(g.r(2.4), rgb(0xFF4D6D));
            for (dx, dy) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                let corner = c + g.v(dx * 2.0, dy * 2.0);
                p.add(Shape::line(vec![corner + g.v(dx * 4.0, 0.0), corner, corner + g.v(0.0, dy * 4.0)], st));
            }
        }
        Emotion::Hungry => {
            let c = g.p(-33.0, -30.0);
            let bubble = super::theme::HEADER.gamma_multiply(0.94);
            p.circle_filled(g.p(-22.0, -19.0), g.r(1.8), bubble);
            p.circle_filled(g.p(-26.0, -23.5), g.r(2.6), bubble);
            p.add(Shape::ellipse_filled(c, g.v(12.0, 8.5), bubble));
            bone(p, c, g.r(7.0), 0.0);
        }
        _ => {}
    }
    if let Some(progress) = l.eating {
        let left = (1.0 - progress).max(0.0);
        if left > 0.05 {
            bone(p, g.p(8.0, 15.0), g.r(8.0) * left, 0.3);
        }
    }
}

/// A number from 0 to 1 out of the genes; every `n` gives another one.
fn roll(genes: u32, n: u32) -> f32 {
    let mut x = genes ^ n.wrapping_add(1).wrapping_mul(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x & 0xFFFF) as f32 / 65535.0
}

/// `base` moved within its range: up to `hue` degrees either way, and
/// saturation and brightness up to those fractions more or less. Uses the
/// rolls `n`, `n + 1` and `n + 2`.
fn vary(genes: u32, n: u32, base: u32, hue: f32, sat: f32, val: f32) -> Color32 {
    let spot = |i: u32| roll(genes, n + i) * 2.0 - 1.0;
    let mut c = egui::epaint::Hsva::from(rgb(base));
    c.h = (c.h + spot(0) * hue / 360.0).rem_euclid(1.0);
    c.s = (c.s * (1.0 + spot(1) * sat)).clamp(0.0, 1.0);
    c.v = (c.v * (1.0 + spot(2) * val)).clamp(0.0, 1.0);
    Color32::from(c)
}

/// `a` with `t` (0..1) of `b` mixed in.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Color of the drawn edge where there is no panel to lean on.
fn theme_edge() -> Color32 {
    super::theme::MUTED.gamma_multiply(0.6)
}
