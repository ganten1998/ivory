//! Chord display strip (spec §5): always-black background, #E8DCC0 text in
//! both modes, Courier Prime Regular, font size max(12, int(0.6*h)) with a
//! single-pass shrink when the text exceeds 95% of the width. Also hosts the
//! detached chord window (an immediate viewport).

use crate::fonts;
use egui::{
    Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, ViewportBuilder, ViewportCommand,
    ViewportId,
};

/// The pre-2.2 chord colour (#E8DCC0). Kept only so `Settings::reset_to_default`
/// and the docs have a name for what the label used to be; the live colour now
/// comes from `settings.chord_text_color`, which defaults to display green.
#[allow(dead_code)]
pub const LEGACY_TEXT_COLOR: Color32 = Color32::from_rgb(232, 220, 192);

/// Supporter heart: a tiny pixel-art sprite in the top-right of the chord view.
/// Purely decorative, shown only with a valid licence, and clickable to recolour.
/// It is drawn from a bitmap rather than a glyph so it stays crisp and unmistakably
/// pixel-art at any window size.
const HEART: [&str; 6] = [
    ".XX.XX.",
    "XXXXXXX",
    "XXXXXXX",
    ".XXXXX.",
    "..XXX..",
    "...X...",
];

/// Colours the heart cycles through when clicked.
pub const HEART_COLORS: [Color32; 7] = [
    Color32::from_rgb(0xE8, 0x3A, 0x4E), // red
    Color32::from_rgb(0xFF, 0x8F, 0xC4), // pink
    Color32::from_rgb(0xE8, 0xC4, 0x6A), // gold
    Color32::from_rgb(0x6C, 0x9B, 0xD2), // blue
    Color32::from_rgb(0x5E, 0xD6, 0x8A), // green
    Color32::from_rgb(0xB8, 0x8F, 0xE0), // violet
    Color32::from_rgb(0xE8, 0xDC, 0xC0), // ivory
];

/// Where the heart sits, given the chord rect. Shared by the renderer and the
/// hit-test so the two can never disagree about what is clickable.
pub fn heart_rect(rect: Rect) -> Rect {
    // One "pixel" scales with the strip but stays chunky and integral, so the
    // sprite never lands on half-pixels and blurs.
    let px = (rect.height() * 0.055).round().max(2.0);
    let w = px * HEART[0].len() as f32;
    let h = px * HEART.len() as f32;
    let margin = px * 2.0;
    Rect::from_min_size(
        Pos2::new(rect.right() - w - margin, rect.top() + margin),
        egui::vec2(w, h),
    )
}

fn draw_heart(painter: &Painter, rect: Rect, color: Color32) {
    let hr = heart_rect(rect);
    let px = hr.height() / HEART.len() as f32;
    for (row, line) in HEART.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            if ch != 'X' {
                continue;
            }
            let p = Pos2::new(hr.left() + col as f32 * px, hr.top() + row as f32 * px);
            painter.rect_filled(Rect::from_min_size(p, egui::vec2(px, px)), 0.0, color);
        }
    }
}

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("ivory-chord-window")
}

/// Outline for the detached window. The strip is always black, so on a dark
/// desktop the window has no edge at all and bleeds into whatever is behind
/// it. One neutral grey reads against both a dark and a light desktop, so
/// this does not need to follow the theme.
pub const BORDER_COLOR: Color32 = Color32::from_gray(0x5A);

const MIN_FONT: i64 = 12;

fn font(size: i64) -> FontId {
    FontId::new(size as f32, fonts::courier())
}

/// The largest legible layout for `text` inside a `w` x `h` strip.
///
/// One line at the natural size whenever it fits. When it does not, the old
/// behaviour was a single-pass shrink until the whole label fitted on one
/// line, which is why "Eb Minor Pentatonic" rendered a fraction of the size of
/// "EbM4" in the same window. Now a two-line layout competes with the shrunken
/// one line and the larger glyphs win.
///
/// The search starts at the shrunken size, so the result is never smaller than
/// what the old code would have produced.
///
/// Returns the chosen point size alongside the galley so tests can assert on
/// the decision rather than on rendered pixels.
fn layout_label(
    painter: &Painter,
    text: &str,
    w: f64,
    h: f64,
    color: Color32,
) -> (std::sync::Arc<egui::Galley>, i64) {
    let natural = ((h * 0.6).trunc() as i64).max(MIN_FONT);
    let one_line = painter.layout_no_wrap(text.to_owned(), font(natural), color);
    let max_w = 0.95 * w;
    let natural_w = one_line.size().x as f64;
    if natural_w <= max_w || natural_w <= 0.0 {
        return (one_line, natural);
    }

    // Candidate A: the historical single-pass shrink to 95% of the width.
    let shrunk = (((natural as f64) * max_w / natural_w).trunc() as i64).max(1);

    // Candidate B: the largest size that still fits on two lines. Only for
    // text with a space in it. egui breaks mid-word to honour a wrap width,
    // and "Ebmaj" / "7#11" across two lines is worse than small.
    if text.trim().contains(char::is_whitespace) {
        let (mut lo, mut hi) = (shrunk, natural);
        let mut best = None;
        while lo <= hi {
            let mid = lo + (hi - lo) / 2;
            let g = painter.layout(text.to_owned(), font(mid), color, max_w as f32);
            if g.rows.len() <= 2 && (g.size().y as f64) <= h {
                best = Some((g, mid));
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        // Always Some: at `shrunk` the text fits on one line by construction.
        if let Some(found) = best {
            return found;
        }
    }

    (
        painter.layout_no_wrap(text.to_owned(), font(shrunk), color),
        shrunk,
    )
}

/// Paint the strip into `rect`. `chord` of None leaves a solid black strip.
///
/// `color` is the chord label colour (user-settable, `Set Chord Color...`).
pub fn draw(
    painter: &Painter,
    rect: Rect,
    chord: Option<&str>,
    color: Color32,
    heart: Option<Color32>,
    border: Option<Color32>,
) {
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    // Inside, so the outline is never clipped away at the window edge.
    if let Some(bc) = border {
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, bc), StrokeKind::Inside);
    }
    // Drawn before the early return so it shows even with no chord sounding.
    if let Some(hc) = heart {
        draw_heart(painter, rect, hc);
    }
    let Some(text) = chord else { return };
    if text.is_empty() {
        return;
    }

    let w = rect.width() as f64;
    let h = rect.height() as f64;
    let (galley, _size) = layout_label(painter, text, w, h, color);

    // Centered, integer positions (Qt draws at int baseline coordinates).
    let tw = galley.size().x as f64;
    let th = galley.size().y as f64;
    let x = ((w - tw) / 2.0).trunc() as f32;
    let y = ((h - th) / 2.0).trunc() as f32;
    let pos = Pos2::new(rect.left() + x, rect.top() + y);

    // Bloom: stamp the same galley around rings of increasing radius at
    // decaying alpha, then the crisp label on top. egui has no blur, and this
    // is what a lit display actually looks like — the glyph stays sharp while
    // light spreads around it. Eight points per ring is enough that the ring
    // structure disappears at these sizes.
    painter.galley(pos, galley, color);
}

/// Everything the app needs to know after showing the detached window.
#[derive(Default)]
pub struct DetachedOutcome {
    /// User closed the window (close-to-reattach).
    pub close_requested: bool,
    /// Live inner size in points, recorded every frame.
    pub inner_size: Option<egui::Vec2>,
    /// Live outer position in monitor coordinates, recorded every frame, so
    /// where the user drags the window can be remembered for next time.
    pub outer_pos: Option<Pos2>,
    /// Right-click happened at this global (monitor-space) position.
    pub context_menu_at: Option<Pos2>,
}

/// Show the detached chord window (spec §5.7) as an immediate viewport.
///
/// `builder_size` and `builder_pos` must stay constant for the lifetime of one
/// detachment. egui diffs this builder against the previous frame's builder,
/// not against the window's real geometry, so a value that changes every frame
/// would fight the user's own resizes and drags.
pub fn show_detached_window(
    ctx: &egui::Context,
    builder_size: egui::Vec2,
    builder_pos: Option<Pos2>,
    borderless: bool,
    chord: Option<&str>,
    color: Color32,
    heart: Option<Color32>,
) -> DetachedOutcome {
    let mut outcome = DetachedOutcome::default();
    let mut builder = ViewportBuilder::default()
        .with_title("Tangent")
        .with_inner_size(builder_size)
        .with_min_inner_size([220.0, 80.0])
        .with_resizable(true)
        .with_decorations(!borderless);
    if let Some(p) = builder_pos {
        builder = builder.with_position(p);
    }

    ctx.show_viewport_immediate(viewport_id(), builder, |vp, _class| {
        crate::shell::viewport_ui(vp, |ui| {
        let rect = ui.max_rect();
        draw(ui.painter(), rect, chord, color, heart, Some(BORDER_COLOR));

        let (close, inner_rect, outer_rect, pressed, secondary, pointer) = ui.input(|i| {
            (
                i.viewport().close_requested(),
                i.viewport().inner_rect,
                i.viewport().outer_rect,
                i.pointer.primary_pressed(),
                i.pointer.secondary_clicked(),
                i.pointer.interact_pos(),
            )
        });

        outcome.close_requested = close;
        outcome.inner_size = inner_rect.map(|r| r.size()).or(Some(rect.size()));
        outcome.outer_pos = outer_rect.map(|r| r.min);

        // Right-click anywhere opens the app context menu at the cursor.
        if secondary {
            if let (Some(pos), Some(inner)) = (pointer, inner_rect) {
                outcome.context_menu_at = Some(inner.min + pos.to_vec2());
            }
        }

        // Borderless drag-anywhere: StartDrag from the press handler.
        if borderless && pressed && !secondary {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        });
    });
    outcome
}

// The 100ms-debounced width sync that used to live here is gone on purpose.
// Spec §5.7 (and the Python app before it) forced the detached window's width
// to match the main window's, re-asserting it on every size change. A chord
// readout has no reason to inherit the keyboard's 8.6667:1 proportions, and
// the re-assert made remembering a user-chosen width impossible: any restored
// width was overwritten within 100ms of the next size change.

#[cfg(test)]
mod tests {
    use super::*;

    /// A headless context with Tangent's fonts actually installed. Laying text
    /// out against the default font set would measure the wrong glyphs, and
    /// `fonts::courier()` names a family that only `fonts::install` binds.
    fn test_painter() -> (egui::Context, Painter) {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        // One pass, so the font atlas exists before anything is laid out.
        let _ = ctx.run(Default::default(), |_| {});
        let painter = Painter::new(ctx.clone(), egui::LayerId::debug(), Rect::EVERYTHING);
        (ctx, painter)
    }

    /// What the pre-2.3 single-pass shrink would have chosen.
    fn legacy_shrunk_size(painter: &Painter, text: &str, w: f64, h: f64) -> i64 {
        let natural = ((h * 0.6).trunc() as i64).max(MIN_FONT);
        let g = painter.layout_no_wrap(text.to_owned(), font(natural), Color32::WHITE);
        let text_w = g.size().x as f64;
        if text_w <= 0.95 * w {
            natural
        } else {
            (((natural as f64) * (0.95 * w) / text_w).trunc() as i64).max(1)
        }
    }

    #[test]
    fn short_labels_are_untouched_and_stay_on_one_line() {
        let (_ctx, painter) = test_painter();
        let (g, size) = layout_label(&painter, "EbM4", 460.0, 150.0, Color32::WHITE);
        assert_eq!(g.rows.len(), 1, "a short label must not wrap");
        // 0.6 * height, exactly as before: no regression for the common case.
        assert_eq!(size, 90);
    }

    #[test]
    fn long_scale_names_wrap_rather_than_shrink_to_nothing() {
        let (_ctx, painter) = test_painter();
        let (w, h) = (460.0, 150.0);
        let text = "Eb Minor Pentatonic";
        let legacy = legacy_shrunk_size(&painter, text, w, h);
        let (g, size) = layout_label(&painter, text, w, h, Color32::WHITE);
        assert_eq!(g.rows.len(), 2, "a long name should use the second line");
        assert!(
            size > legacy,
            "wrapping must buy glyph size back: got {size}pt, old behaviour was {legacy}pt"
        );
        assert!(
            (g.size().y as f64) <= h,
            "two lines must still fit inside the window"
        );
    }

    #[test]
    fn label_without_spaces_cannot_wrap_and_falls_back_to_shrinking() {
        let (_ctx, painter) = test_painter();
        // egui would happily break this mid-token to honour a wrap width, and
        // "Cmaj7" split across two lines reads worse than small.
        let text = "Cmaj7#11b13sus4addBnonsense";
        let (w, h) = (200.0, 150.0);
        let (g, size) = layout_label(&painter, text, w, h, Color32::WHITE);
        assert_eq!(g.rows.len(), 1);
        assert_eq!(size, legacy_shrunk_size(&painter, text, w, h));
    }

    #[test]
    fn wrapping_never_returns_less_than_the_old_single_line_shrink() {
        let (_ctx, painter) = test_painter();
        // The binary search starts at the shrunken size, so this is a property,
        // not a coincidence. Swept across shapes the real window takes.
        for (w, h) in [(460.0, 150.0), (1300.0, 50.0), (220.0, 80.0), (650.0, 25.0)] {
            for text in [
                "C",
                "EbM4",
                "C7(b9,#11)",
                "Eb Minor Pentatonic",
                "Bb Half-Whole Diminished",
                "F# Melodic Minor",
            ] {
                let (_, size) = layout_label(&painter, text, w, h, Color32::WHITE);
                let legacy = legacy_shrunk_size(&painter, text, w, h);
                assert!(
                    size >= legacy,
                    "{text} at {w}x{h}: {size}pt is worse than the old {legacy}pt"
                );
            }
        }
    }

    #[test]
    fn heart_bitmap_is_rectangular_and_non_empty() {
        let w = HEART[0].len();
        assert!(w > 0);
        for row in HEART {
            assert_eq!(row.len(), w, "heart rows must be equal length");
        }
        assert!(HEART.iter().any(|r| r.contains('X')), "heart is blank");
    }

    #[test]
    fn any_stored_heart_index_is_in_range() {
        // settings.json is hand-editable and carries a raw i64; a negative or
        // huge value must wrap rather than panic on indexing.
        let n = HEART_COLORS.len() as i64;
        for stored in [-9_999i64, -1, 0, 3, n, n * 7 + 2, i64::MAX] {
            let idx = stored.rem_euclid(n) as usize;
            assert!(HEART_COLORS.get(idx).is_some(), "index {stored} escaped range");
        }
    }

    #[test]
    fn heart_sits_inside_the_strip_at_realistic_sizes() {
        for (w, h) in [(300.0f32, 40.0f32), (1300.0, 46.0), (2600.0, 120.0)] {
            let strip = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(w, h));
            let hr = heart_rect(strip);
            assert!(strip.contains_rect(hr), "heart escapes the strip at {w}x{h}");
            // Top-right, not centred: it must not collide with the chord label.
            assert!(hr.center().x > strip.center().x, "heart should hug the right");
            assert!(hr.center().y < strip.center().y, "heart should hug the top");
        }
    }
}
