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

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("ivory-chord-window")
}

/// Paint the strip into `rect`. `chord` of None leaves a solid black strip.
///
/// `color` is the chord label colour (user-settable). With `glow` set — a
/// supporter extra — the label is bloomed: the same galley is stamped around a
/// ring at decaying alpha before the crisp text goes on top, which reads like a
/// lit display rather than like flat type.
pub fn draw(painter: &Painter, rect: Rect, chord: Option<&str>, color: Color32, glow: bool) {
    painter.rect_filled(rect, 0.0, Color32::BLACK);
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
    if glow {
        // Per-stamp alpha must be TINY: with RINGS*POINTS overlapping copies the
        // opacities compound (1 - (1-a)^n), so a "modest" 0.3 per stamp renders
        // a solid blob instead of a halo. These values put the accumulated peak
        // near 0.25 right at the glyph edge.
        const RINGS: usize = 3;
        const POINTS: usize = 8;
        let radius = (th as f32 * 0.11).clamp(1.5, 9.0);
        for ring in 1..=RINGS {
            let t = ring as f32 / RINGS as f32;
            let r = radius * t;
            let alpha = 0.055 * (-2.5 * t).exp();
            let tint = color.gamma_multiply(alpha);
            for p in 0..POINTS {
                let a = std::f32::consts::TAU * (p as f32 / POINTS as f32);
                let off = egui::vec2(r * a.cos(), r * a.sin());
                painter.galley(pos + off, galley.clone(), tint);
            }
        }
    }

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
    glow: bool,
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
        draw(ui.painter(), rect, chord, color, glow);

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
