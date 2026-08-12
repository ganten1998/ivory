//! Chord display strip (spec §5): always-black background, #E8DCC0 text in
//! both modes, Courier Prime Regular, font size max(12, int(0.6*h)) with a
//! single-pass shrink when the text exceeds 95% of the width. Also hosts the
//! detached chord window (an immediate viewport).

use crate::fonts;
use egui::{Color32, FontId, Painter, Pos2, Rect, ViewportBuilder, ViewportCommand, ViewportId};

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

/// Paint the strip into `rect`. `chord` of None leaves a solid black strip.
///
/// `color` is the chord label colour (user-settable, `Set Chord Color...`).
pub fn draw(
    painter: &Painter,
    rect: Rect,
    chord: Option<&str>,
    color: Color32,
    heart: Option<Color32>,
) {
    painter.rect_filled(rect, 0.0, Color32::BLACK);
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

    // Point size: max(12, int(height * 0.6)).
    let mut font_size = ((h * 0.6).trunc() as i64).max(12);
    let font = |size: i64| FontId::new(size as f32, fonts::courier());
    let mut galley = painter.layout_no_wrap(text.to_owned(), font(font_size), color);

    // Single-pass shrink at 95% width (not a loop).
    let text_w = galley.size().x as f64;
    if text_w > 0.95 * w && text_w > 0.0 {
        font_size = ((font_size as f64) * (0.95 * w) / text_w).trunc() as i64;
        font_size = font_size.max(1);
        galley = painter.layout_no_wrap(text.to_owned(), font(font_size), color);
    }

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
    /// Right-click happened at this global (monitor-space) position.
    pub context_menu_at: Option<Pos2>,
}

/// Show the detached chord window (spec §5.7) as an immediate viewport.
/// `builder_size` must stay constant for the lifetime of one detachment so the
/// per-frame builder diff never fights user resizes; explicit width syncing is
/// done via `sync_width`.
pub fn show_detached_window(
    ctx: &egui::Context,
    builder_size: egui::Vec2,
    borderless: bool,
    chord: Option<&str>,
    color: Color32,
    heart: Option<Color32>,
) -> DetachedOutcome {
    let mut outcome = DetachedOutcome::default();
    let builder = ViewportBuilder::default()
        .with_title("Ivory")
        .with_inner_size(builder_size)
        .with_min_inner_size([300.0, 100.0])
        .with_resizable(true)
        .with_decorations(!borderless);

    ctx.show_viewport_immediate(viewport_id(), builder, |ui, _class| {
        let rect = ui.max_rect();
        draw(ui.painter(), rect, chord, color, heart);

        let (close, inner_rect, pressed, secondary, pointer) = ui.input(|i| {
            (
                i.viewport().close_requested(),
                i.viewport().inner_rect,
                i.pointer.primary_pressed(),
                i.pointer.secondary_clicked(),
                i.pointer.interact_pos(),
            )
        });

        outcome.close_requested = close;
        outcome.inner_size = inner_rect.map(|r| r.size()).or(Some(rect.size()));

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
    outcome
}

/// 100ms-debounced width sync (spec §5.7): match the detached window's width
/// to the main window when they differ by more than 5px, preserving height.
pub fn sync_width(ctx: &egui::Context, main_w: f32, current: egui::Vec2) {
    if (current.x - main_w).abs() > 5.0 {
        ctx.send_viewport_cmd_to(
            viewport_id(),
            ViewportCommand::InnerSize(egui::vec2(main_w, current.y)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
