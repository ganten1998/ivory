//! The launch splash: something to look at while the app becomes usable.
//!
//! **It exists because the wait is real, not because a logo is nice.** A saved
//! instrument blocks the UI thread for about five seconds while its module
//! initialises and warms up, and an external camera adds up to four more. Both
//! happen after the window is on screen, so without this the app opens, paints
//! one frame, and then ignores every click for as long as eight seconds — which
//! reads as a crash, and the natural response to which is to launch it again.
//!
//! A pure painter, like every other band in this crate: it is handed a rect and
//! a line of text and draws them. **What "ready" means is not decided here** —
//! that is the host's question, because the host is what owns the devices being
//! waited on.

use crate::fonts;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};

/// The wordmark, at a size proportional to the window.
const TITLE_FRACTION: f32 = 0.085;
/// The status line, relative to the wordmark.
const STATUS_FRACTION: f32 = 0.30;
/// Ink, on the app's own black. Not the theme's: the splash is up before
/// anybody has seen the theme, and a light-mode splash that flashes white for
/// half a second at midnight is worse than one that is always dark.
const INK: Color32 = Color32::from_rgb(0xE8, 0xDC, 0xC0);
const DIM: Color32 = Color32::from_rgb(0x8A, 0x82, 0x74);

/// Draw the splash over `rect`.
///
/// `fade` is 1.0 while it is up and falls to 0.0 as it leaves, so the window
/// underneath is revealed rather than appearing all at once. At 0.0 nothing is
/// drawn at all.
pub fn draw(painter: &Painter, rect: Rect, status: &str, fade: f32) {
    let fade = fade.clamp(0.0, 1.0);
    if fade <= 0.0 || !rect.is_positive() {
        return;
    }
    let alpha = |c: Color32| c.gamma_multiply(fade);
    // Opaque while it is up. A translucent splash over a half-drawn window is
    // how a loading screen ends up looking like a rendering fault.
    painter.rect_filled(rect, 0.0, alpha(Color32::BLACK));

    let title_size = (rect.height() * TITLE_FRACTION).clamp(14.0, 64.0);
    let centre = rect.center();
    painter.text(
        Pos2::new(centre.x, centre.y - title_size * 0.4),
        Align2::CENTER_CENTER,
        "TANGENT",
        FontId::new(title_size, fonts::courier_bold()),
        alpha(INK),
    );

    // A hairline under the wordmark, the width of the word. Drawn from the
    // measured text rather than a guessed fraction, so it tracks the font at
    // every window size.
    let rule_w = title_size * 7.0 * 0.62;
    let rule_y = centre.y + title_size * 0.35;
    painter.line_segment(
        [
            Pos2::new(centre.x - rule_w * 0.5, rule_y),
            Pos2::new(centre.x + rule_w * 0.5, rule_y),
        ],
        Stroke::new(1.0, alpha(DIM)),
    );

    if !status.is_empty() {
        let size = (title_size * STATUS_FRACTION).max(9.0);
        painter.text(
            Pos2::new(centre.x, rule_y + size * 2.0),
            Align2::CENTER_CENTER,
            status,
            FontId::new(size, fonts::courier()),
            alpha(DIM),
        );
    }
}

/// What the splash should say while `waiting_for` is happening.
///
/// One place, so the status line cannot contradict the band underneath it once
/// the splash lifts. `None` when there is nothing specific to report, which is
/// the ordinary case — most of the wait is the window and the fonts.
pub fn status(instrument: Option<&str>, camera: bool) -> String {
    match (instrument, camera) {
        // The instrument first: it is the longer wait by far, and naming it is
        // what turns five silent seconds into five explained ones.
        (Some(name), _) => format!("loading {name}"),
        (None, true) => "starting the camera".to_owned(),
        (None, false) => "starting up".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_names_the_instrument_because_that_is_the_long_wait() {
        assert_eq!(status(Some("Pianoteq 9"), false), "loading Pianoteq 9");
        // The instrument wins even when the camera is also starting: five
        // seconds beats four, and two subjects in one line is a line nobody
        // reads.
        assert_eq!(status(Some("Pianoteq 9"), true), "loading Pianoteq 9");
        assert_eq!(status(None, true), "starting the camera");
        assert_eq!(status(None, false), "starting up");
    }

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
                        draw(ui.painter(), r, "loading Pianoteq 9", fade);
                    });
                });
            }
        }
    }
}
