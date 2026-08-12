//! 88-key piano renderer + hit-testing, replicating Qt geometry exactly
//! (spec §4): 52 white keys, blackW = 0.7*whiteW, blackH = 0.65*H, black keys
//! centered on the white-key boundary via the whiteKeysBefore formula, with
//! Python's int() truncation. Sub-pixel background slivers between truncated
//! key rects are part of the look and are preserved.

use crate::settings::Settings;
use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind};
use std::collections::HashSet;

pub const NOTE_START: u8 = 21; // A0
pub const NOTE_END: u8 = 108; // C8
pub const WHITE_KEYS: usize = 52;

pub fn is_white(note: u8) -> bool {
    matches!(note % 12, 0 | 2 | 4 | 5 | 7 | 9 | 11)
}

/// Number of white keys strictly to the left of a black key (spec §4.3.4):
/// octave block note//12 == 1 (A#0) => 0; otherwise 2 (A0,B0) plus
/// 7*(note//12 - 2) plus a per-pitch-class offset.
pub fn white_keys_before(note: u8) -> Option<usize> {
    if is_white(note) {
        return None;
    }
    if note / 12 == 1 {
        return Some(0); // A#0
    }
    let offset = match note % 12 {
        1 => 0,  // C#
        3 => 1,  // D#
        6 => 3,  // F#
        8 => 4,  // G#
        10 => 5, // A#
        _ => return None,
    };
    Some(2 + 7 * (note as usize / 12 - 2) + offset)
}

/// Idle fill for a key, honoring the dark-mode swap (idle colors ONLY swap).
fn idle_colors(s: &Settings) -> (Color32, Color32) {
    let white_idle = s.white_key_idle_color.to_color32();
    let black_idle = s.black_key_idle_color.to_color32();
    if s.dark_mode {
        (black_idle, white_idle) // (white keys, black keys) swapped
    } else {
        (white_idle, black_idle)
    }
}

pub fn bg_color(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(0x1a, 0x1a, 0x1a)
    } else {
        Color32::from_rgb(0xE8, 0xE8, 0xE8)
    }
}

/// Draw the piano into `rect`. `display_notes` must already include manual
/// (keytoggle) notes. While the sustain pedal is down, EVERY active key is
/// drawn in the sustain color (spec §4.4).
pub fn draw(
    painter: &Painter,
    rect: Rect,
    display_notes: &HashSet<u8>,
    sustain_down: bool,
    s: &Settings,
    glow: bool,
) {
    let w = rect.width() as f64;
    let h = rect.height() as f64;
    let left = rect.left();
    let top = rect.top();

    // 1. Background fill (visible only through truncation slivers).
    painter.rect_filled(rect, 0.0, bg_color(s.dark_mode));

    let white_key_w = w / WHITE_KEYS as f64;
    let black_key_w = white_key_w * 0.7;
    let black_key_h = h * 0.65;

    // Held keys can carry a halo (supporter extra). It takes its color from
    // whatever the user set as the active/sustain color, so it always agrees
    // with their palette instead of imposing one.
    let mut lit_white: Vec<(Rect, Color32)> = Vec::new();
    let mut lit_black: Vec<(Rect, Color32)> = Vec::new();

    let (white_idle, black_idle) = idle_colors(s);
    let sustain = s.sustain_color.to_color32();
    let white_active = s.white_key_active_color.to_color32();
    let black_active = s.black_key_active_color.to_color32();

    let separator = if s.dark_mode {
        Color32::from_rgb(153, 153, 153)
    } else {
        Color32::from_rgb(92, 63, 31)
    };
    let outline = if s.dark_mode {
        Color32::from_rgb(204, 204, 204)
    } else {
        Color32::from_rgb(139, 115, 85)
    };

    // 2. White keys, left to right (int truncation per key, like Qt fillRect).
    // Fill every white key FIRST, then draw the separators on top — otherwise each
    // key's fill overpaints the previous key's separator line and they vanish.
    let mut idx = 0usize;
    for note in NOTE_START..=NOTE_END {
        if !is_white(note) {
            continue;
        }
        let x = idx as f64 * white_key_w;
        let fill = if display_notes.contains(&note) {
            if sustain_down {
                sustain
            } else {
                white_active
            }
        } else {
            white_idle
        };
        let key_rect = Rect::from_min_size(
            Pos2::new(left + x.trunc() as f32, top),
            egui::vec2(white_key_w.trunc() as f32, h.trunc() as f32),
        );
        painter.rect_filled(key_rect, 0.0, fill);
        if glow && display_notes.contains(&note) {
            lit_white.push((key_rect, fill));
        }
        idx += 1;
    }
    // White-key halos go here — before the separators and the black keys, so
    // the black keys occlude them exactly as they occlude the keys themselves.
    draw_glow(painter, &lit_white, w);

    // 1px separators between adjacent white keys (drawn after all fills).
    for sep in 1..WHITE_KEYS {
        let lx = left + (sep as f64 * white_key_w).trunc() as f32 + 0.5;
        painter.line_segment(
            [Pos2::new(lx, top), Pos2::new(lx, top + h.trunc() as f32)],
            Stroke::new(1.0, separator),
        );
    }

    // 3. Black keys on top.
    for note in NOTE_START..=NOTE_END {
        let Some(before) = white_keys_before(note) else {
            continue;
        };
        if !(before < WHITE_KEYS && before + 1 < WHITE_KEYS) {
            continue;
        }
        // gapCenter = (before + 1) * whiteKeyW (exactly on the separator line)
        let gap_center = (before as f64 + 1.0) * white_key_w;
        let x = gap_center - black_key_w / 2.0;

        let fill = if display_notes.contains(&note) {
            if sustain_down {
                sustain
            } else {
                black_active
            }
        } else {
            black_idle
        };
        let bx = x.trunc() as f32;
        let bw = black_key_w.trunc() as f32;
        let bh = black_key_h.trunc() as f32;
        let key_rect = Rect::from_min_size(Pos2::new(left + bx, top), egui::vec2(bw, bh));
        painter.rect_filled(key_rect, 0.0, fill);
        // 1px outline; Qt drawRect includes the far edge, so stroke the
        // half-pixel-centered boundary for a crisp 1px ring.
        let stroke_rect = Rect::from_min_max(
            Pos2::new(left + bx + 0.5, top + 0.5),
            Pos2::new(left + bx + bw + 0.5, top + bh + 0.5),
        );
        painter.rect_stroke(stroke_rect, 0.0, Stroke::new(1.0, outline), StrokeKind::Middle);
        if glow && display_notes.contains(&note) {
            lit_black.push((key_rect, fill));
        }
    }

    // Black keys are the frontmost layer, so their halos are too.
    draw_glow(painter, &lit_black, w);

}

/// Halo under held keys. Many 1px strokes with an exponential falloff — a few
/// thick bands read as a hard-edged selection box, which is what a glow must
/// not look like.
///
/// Called at two different depths on purpose. A white key sits BEHIND the black
/// keys, so its halo has to be painted before them or it draws visible lines
/// across the black keys in front of it; the black keys' own halos go last.
fn draw_glow(painter: &Painter, lit: &[(Rect, Color32)], width: f64) {
    if lit.is_empty() {
        return;
    }
    let scale = ((width / 1300.0) as f32).clamp(0.5, 2.0);
    let radius = (16.0 * scale).max(2.0);
    let steps = radius.round() as i32;
    for i in 0..steps {
        let t = (i as f32 + 0.5) / steps as f32;
        let alpha = 0.40 * (-2.6 * t).exp();
        if alpha < 0.004 {
            break;
        }
        let outset = t * radius;
        for (rect, color) in lit {
            painter.rect_stroke(
                rect.expand(outset),
                outset.min(6.0),
                Stroke::new(1.0, color.gamma_multiply(alpha)),
                StrokeKind::Middle,
            );
        }
    }
}

/// Hit-test a point (widget-local) against the keys, matching the drawing math
/// (spec §4.5): black keys first when y < blackKeyH, else white by index.
pub fn hit_test(x: f32, y: f32, w: f32, h: f32) -> Option<u8> {
    if x < 0.0 || y < 0.0 || x >= w || y >= h {
        return None;
    }
    let x = x as f64;
    let y = y as f64;
    let w = w as f64;
    let h = h as f64;
    let white_key_w = w / WHITE_KEYS as f64;
    let black_key_w = white_key_w * 0.7;
    let black_key_h = h * 0.65;

    if y < black_key_h {
        for note in NOTE_START..=NOTE_END {
            let Some(before) = white_keys_before(note) else {
                continue;
            };
            if !(before < WHITE_KEYS && before + 1 < WHITE_KEYS) {
                continue;
            }
            let key_x = (before as f64 + 1.0) * white_key_w - black_key_w / 2.0;
            if x >= key_x && x <= key_x + black_key_w {
                return Some(note);
            }
        }
    }

    // White key by index. (All black keys were already checked above, so the
    // Python "not within an adjacent black key" re-check is vacuous here.)
    let idx = (x / white_key_w) as usize;
    if idx >= WHITE_KEYS {
        return None;
    }
    let mut wi = 0usize;
    for note in NOTE_START..=NOTE_END {
        if !is_white(note) {
            continue;
        }
        if wi == idx {
            return Some(note);
        }
        wi += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_52_white_and_36_black_keys() {
        let whites = (NOTE_START..=NOTE_END).filter(|&n| is_white(n)).count();
        let blacks = (NOTE_START..=NOTE_END)
            .filter_map(white_keys_before)
            .count();
        assert_eq!(whites, 52);
        assert_eq!(blacks, 36);
    }

    #[test]
    fn white_keys_before_formula_matches_counting() {
        // `whiteKeysBefore` is the index of the white key to the black key's
        // LEFT, i.e. (number of white notes strictly below it) - 1.
        for note in NOTE_START..=NOTE_END {
            if is_white(note) {
                continue;
            }
            let below = (NOTE_START..note).filter(|&n| is_white(n)).count();
            assert_eq!(
                white_keys_before(note),
                Some(below - 1),
                "whiteKeysBefore mismatch for note {note}"
            );
        }
    }

    #[test]
    fn hit_test_matches_black_first_semantics() {
        let (w, h) = (1300.0_f32, 150.0_f32);
        let wkw = w as f64 / 52.0;
        // Dead center of the boundary between white keys 0 (A0) and 1 (B0)
        // above black-key height => A#0 (note 22).
        let bx = 1.0 * wkw;
        assert_eq!(hit_test(bx as f32, 10.0, w, h), Some(22));
        // Same x below black-key height => the white key under it (B0, idx 1).
        assert_eq!(hit_test(bx as f32 + 1.0, h * 0.9, w, h), Some(23));
        // Far left bottom => A0.
        assert_eq!(hit_test(1.0, h - 1.0, w, h), Some(21));
        // Far right => C8.
        assert_eq!(hit_test(w - 1.0, h - 1.0, w, h), Some(108));
        // Outside.
        assert_eq!(hit_test(-1.0, 10.0, w, h), None);
        assert_eq!(hit_test(w + 1.0, 10.0, w, h), None);
    }
}
