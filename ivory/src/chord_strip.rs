//! Chord display strip (spec §5): always-black background, #E8DCC0 text in
//! both modes, Courier Prime Regular, font size max(12, int(0.6*h)) with a
//! single-pass shrink when the text exceeds 95% of the width. Also hosts the
//! detached chord window (an immediate viewport).

use crate::fonts;
use egui::{Color32, FontId, Painter, Pos2, Rect, ViewportBuilder, ViewportCommand, ViewportId};

/// Chord label color for the active theme. The strip is mode-independent
/// (spec §5.3), so both variants carry the same value and either works.
pub fn text_color() -> Color32 {
    crate::theme::active().dark.chord_text
}

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("ivory-chord-window")
}

/// Paint the strip into `rect`. `chord` of None leaves a solid black strip.
pub fn draw(painter: &Painter, rect: Rect, chord: Option<&str>) {
    painter.rect_filled(rect, 0.0, crate::theme::active().dark.chord_bg);
    let Some(text) = chord else { return };
    if text.is_empty() {
        return;
    }

    let w = rect.width() as f64;
    let h = rect.height() as f64;

    // Point size: max(12, int(height * 0.6)).
    let mut font_size = ((h * 0.6).trunc() as i64).max(12);
    let font = |size: i64| FontId::new(size as f32, fonts::courier());
    let mut galley = painter.layout_no_wrap(text.to_owned(), font(font_size), text_color());

    // Single-pass shrink at 95% width (not a loop).
    let text_w = galley.size().x as f64;
    if text_w > 0.95 * w && text_w > 0.0 {
        font_size = ((font_size as f64) * (0.95 * w) / text_w).trunc() as i64;
        font_size = font_size.max(1);
        galley = painter.layout_no_wrap(text.to_owned(), font(font_size), text_color());
    }

    // Centered, integer positions (Qt draws at int baseline coordinates).
    let tw = galley.size().x as f64;
    let th = galley.size().y as f64;
    let x = ((w - tw) / 2.0).trunc() as f32;
    let y = ((h - th) / 2.0).trunc() as f32;
    painter.galley(
        Pos2::new(rect.left() + x, rect.top() + y),
        galley,
        text_color(),
    );
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
        draw(ui.painter(), rect, chord);

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
