//! The launch splash: something to look at while the app becomes usable.
//!
//! **It exists because the wait is real, not because a logo is nice.** A saved
//! instrument blocks the UI thread for about five seconds while its module
//! initialises and warms up, and an external camera adds up to four more. Both
//! happen after the window is on screen, so without this the app opens, paints
//! one frame, and then ignores every click for as long as eight seconds — which
//! reads as a crash, and the natural response to which is to launch it again.
//!
//! **The wordmark is the app's own Tonnetz.** Seven letters on the triangular
//! lattice — TANG over ENT, a row of four over an offset row of three, exactly
//! the arrangement the theory band draws its pitch classes in — with the
//! lattice lines running off past the letters the way a crop of an infinite
//! lattice does. No status line: the splash used to explain what it was
//! waiting for, and the explanation outlived its usefulness the moment the
//! logo said what the app IS.
//!
//! A pure painter, like every other band in this crate: it is handed a rect
//! and draws. **What "ready" means is not decided here** — that is the host's
//! question, because the host is what owns the devices being waited on.

use crate::fonts;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};

/// Ink, on the app's own black. Not the theme's: the splash is up before
/// anybody has seen the theme, and a light-mode splash that flashes white for
/// half a second at midnight is worse than one that is always dark.
const INK: Color32 = Color32::from_rgb(0xE8, 0xDC, 0xC0);
const DIM: Color32 = Color32::from_rgb(0x8A, 0x82, 0x74);

/// The seven letters, as lattice coordinates.
///
/// `(col, row)` on the triangular grid: row 0 is the top row of four, row 1
/// sits below it shifted half a step right, which is what makes the triangles
/// equilateral and the whole thing read as the Tonnetz rather than as two
/// lines of type. Reading order is the ordinary one — along the top, then
/// along the bottom — so the lattice spells the name without being told how
/// to be read.
const LETTERS: [(f32, i32, char); 7] = [
    (0.0, 0, 'T'),
    (1.0, 0, 'A'),
    (2.0, 0, 'N'),
    (3.0, 0, 'G'),
    (0.5, 1, 'E'),
    (1.5, 1, 'N'),
    (2.5, 1, 'T'),
];

/// Node radius as a fraction of the lattice spacing — the same 0.30 the theory
/// band's own `Lattice` uses, so the splash and the diagram are one drawing at
/// two moments rather than a logo and a thing it vaguely resembles.
const NODE_R: f32 = 0.30;

/// How far a lattice line runs toward a neighbour that is not there, as a
/// fraction of the spacing. Past the node's edge but well short of where the
/// phantom neighbour's circle would begin, which is what makes the seven
/// letters read as a CROP of the infinite lattice rather than as a diagram
/// with loose ends.
const STUB: f32 = 0.62;

/// Draw the splash over `rect`.
///
/// `fade` is 1.0 while it is up and falls to 0.0 as it leaves, so the window
/// underneath is revealed rather than appearing all at once. At 0.0 nothing is
/// drawn at all.
pub fn draw(painter: &Painter, rect: Rect, fade: f32) {
    let fade = fade.clamp(0.0, 1.0);
    if fade <= 0.0 || !rect.is_positive() {
        return;
    }
    let alpha = |c: Color32| c.gamma_multiply(fade);
    // Opaque while it is up. A translucent splash over a half-drawn window is
    // how a loading screen ends up looking like a rendering fault.
    painter.rect_filled(rect, 0.0, alpha(Color32::BLACK));

    // The spacing, from the window. Modest on purpose — a splash is a
    // courtesy, not an announcement — and floored where the circles would
    // collapse into unreadable dots, at which point the lattice is not drawn
    // at all rather than drawn as a smudge.
    // Smaller than the first cut: at 13% of the width the logo was an
    // announcement, and a splash is a courtesy. This is the whole change.
    let a = (rect.width() * 0.09).min(rect.height() * 0.155).min(50.0);
    if a < 18.0 {
        return;
    }
    let h = a * 0.866_025_4; // equilateral: a * sqrt(3)/2
    let r = a * NODE_R;

    // Centre the seven-node bounding box; the stubs extend symmetrically.
    let centre = rect.center();
    let origin = Pos2::new(centre.x - a * 1.5, centre.y - h * 0.5);
    let at = |col: f32, row: i32| Pos2::new(origin.x + col * a, origin.y + row as f32 * h);
    let nodes: Vec<(Pos2, char)> = LETTERS
        .iter()
        .map(|(col, row, ch)| (at(*col, *row), *ch))
        .collect();

    // The six lattice directions out of any node.
    let dirs: [Vec2; 6] = [
        Vec2::new(a, 0.0),
        Vec2::new(-a, 0.0),
        Vec2::new(a * 0.5, h),
        Vec2::new(-a * 0.5, h),
        Vec2::new(a * 0.5, -h),
        Vec2::new(-a * 0.5, -h),
    ];

    // Lines first, nodes on top — the same order the Tonnetz itself draws in,
    // and for the same reason: the circles are filled, so they trim every line
    // back to their own edge without any arithmetic here.
    let stroke = Stroke::new(1.0_f32, alpha(DIM));
    for (c, _) in &nodes {
        for d in dirs {
            let n = *c + d;
            let real = nodes.iter().any(|(o, _)| (*o - n).length() < a * 0.25);
            if real {
                // A real neighbour: the full edge. Drawn from both ends, which
                // costs one duplicate line and saves a visited-set.
                painter.line_segment([*c, n], stroke);
            } else {
                // No neighbour: a stub running off the crop.
                painter.line_segment([*c, *c + d * STUB], stroke);
            }
        }
    }
    for (c, ch) in &nodes {
        painter.circle_filled(*c, r, alpha(Color32::BLACK));
        painter.circle_stroke(*c, r, Stroke::new(1.2_f32, alpha(INK)));
        painter.text(
            *c,
            Align2::CENTER_CENTER,
            *ch,
            FontId::new(r * 1.25, fonts::courier_bold()),
            alpha(INK),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// It must draw at every size a window can be, including the degenerate
    /// ones — a splash that panics is a launch that fails.
    #[test]
    fn it_draws_at_every_size_without_panicking() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        for (w, h) in [(0.0_f32, 0.0_f32), (1.0, 1.0), (320.0, 200.0), (3840.0, 2160.0)] {
            for fade in [0.0_f32, 0.5, 1.0] {
                let r = Rect::from_min_size(Pos2::ZERO, egui::vec2(w, h));
                let _ = ctx.run(Default::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw(ui.painter(), r, fade);
                    });
                });
            }
        }
    }

    /// The lattice spells the name. Four over three, offset by half a step,
    /// one row of height apart — the Tonnetz's own arrangement, asserted so a
    /// rearrangement cannot quietly turn the logo into two lines of type.
    #[test]
    fn the_lattice_spells_tangent_in_tonnetz_geometry() {
        let word: String = LETTERS.iter().map(|(_, _, c)| *c).collect();
        assert_eq!(word, "TANGENT");
        let top: Vec<f32> = LETTERS.iter().filter(|l| l.1 == 0).map(|l| l.0).collect();
        let bottom: Vec<f32> = LETTERS.iter().filter(|l| l.1 == 1).map(|l| l.0).collect();
        assert_eq!(top, vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(bottom, vec![0.5, 1.5, 2.5]);
        // Every bottom letter sits exactly between two top letters, which is
        // the equilateral offset and the whole reason it reads as the Tonnetz.
        for b in bottom {
            assert!(top.contains(&(b - 0.5)) && top.contains(&(b + 0.5)));
        }
    }
}
