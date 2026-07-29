use std::collections::HashMap;
use egui::{Color32, Painter, Rect, Stroke, Vec2};
use crate::settings::Settings;

/// 88-key piano: A0 (21) to C8 (108).
const START: u8 = 21;
const END: u8 = 108;
const WHITE_KEYS: usize = 52;
const PIANO_ASPECT: f32 = 1300.0 / 150.0; // ~8.67

fn is_white(note: u8) -> bool {
    matches!(note % 12, 0 | 2 | 4 | 5 | 7 | 9 | 11)
}

/// Returns the x-offset (0..WHITE_KEYS) of a white key.
#[allow(dead_code)]
fn white_key_index(note: u8) -> Option<usize> {
    if !is_white(note) { return None; }
    let octave = note / 12;
    let pc = note % 12;
    let white_map = [0usize, 99, 1, 99, 2, 3, 99, 4, 99, 5, 99, 6]; // 99 = black
    let oct_offset = white_map[pc as usize];

    Some(if octave == 1 {
        // A0=0, B0=1
        match pc { 9 => 0, 11 => 1, _ => return None }
    } else {
        let base = 2 + (octave as usize - 2) * 7;
        base + oct_offset
    })
}

/// Returns the white-key index to the LEFT of a black key (so it sits between idx and idx+1).
fn black_key_left_white(note: u8) -> Option<usize> {
    if is_white(note) { return None; }
    let octave = note / 12;
    let pc = note % 12;
    // black_key_map: pc → position within octave (white keys before it)
    let oct_offset: usize = match pc {
        1 => 0,   // C#: between C(0) and D(1)
        3 => 1,   // D#: between D(1) and E(2)
        6 => 3,   // F#: between F(3) and G(4)
        8 => 4,   // G#: between G(4) and A(5)
        10 => 5,  // A#: between A(5) and B(6)
        _ => return None,
    };

    Some(if octave == 1 && pc == 10 {
        // A#0 sits between A0(0) and B0(1)
        0
    } else {
        let base = 2 + (octave as usize - 2) * 7;
        base + oct_offset
    })
}

/// Draw the 88-key piano into `rect`.
/// `active_notes` maps MIDI note → dummy value (we only need the keys).
/// `manual_notes` are keyboard-toggled notes (keytoggle mode).
/// Returns a list of notes whose key was clicked this frame.
pub fn draw_piano(
    painter: &Painter,
    rect: Rect,
    active_notes: &HashMap<u8, ()>,
    sustain_active: bool,
    settings: &Settings,
    _keytoggle_enabled: bool,
) {
    let w = rect.width();
    let h = rect.height();
    let wkw = w / WHITE_KEYS as f32; // white key width
    let bkw = wkw * 0.7;            // black key width
    let bkh = h * 0.65;             // black key height

    // ── White keys ───────────────────────────────────────────────────────────
    let white_idle = settings.white_key_idle_color.to_egui();
    let white_active = settings.white_key_active_color.to_egui();
    let sustain_col = settings.sustain_color.to_egui();
    let black_idle = settings.black_key_idle_color.to_egui();
    let sep_col = if settings.dark_mode {
        Color32::from_rgb(153, 153, 153)
    } else {
        Color32::from_rgb(92, 63, 31)
    };

    let mut white_idx = 0usize;
    for note in START..=END {
        if !is_white(note) { continue; }
        let x = rect.left() + white_idx as f32 * wkw;
        let key_rect = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(wkw, h));

        let fill = if active_notes.contains_key(&note) {
            if sustain_active { sustain_col } else { white_active }
        } else if settings.dark_mode { black_idle } else { white_idle };

        painter.rect_filled(key_rect, 0.0, fill);
        if white_idx + 1 < WHITE_KEYS {
            let lx = x + wkw;
            painter.line_segment(
                [egui::pos2(lx, rect.top()), egui::pos2(lx, rect.bottom())],
                Stroke::new(1.0, sep_col),
            );
        }
        white_idx += 1;
    }

    // ── Black keys ───────────────────────────────────────────────────────────
    let outline_col = if settings.dark_mode {
        Color32::from_rgb(204, 204, 204)
    } else {
        Color32::from_rgb(139, 115, 85)
    };

    for note in START..=END {
        if is_white(note) { continue; }
        let left_idx = match black_key_left_white(note) {
            Some(i) => i,
            None => continue,
        };
        if left_idx + 1 >= WHITE_KEYS { continue; }

        let x1 = rect.left() + left_idx as f32 * wkw;
        let x2 = rect.left() + (left_idx + 1) as f32 * wkw;
        let gap_center = (x1 + wkw + x2) / 2.0;
        let bx = gap_center - bkw / 2.0;

        let fill = if active_notes.contains_key(&note) {
            if sustain_active { sustain_col } else {
                settings.black_key_active_color.to_egui()
            }
        } else if settings.dark_mode { white_idle } else { black_idle };

        let bk_rect = Rect::from_min_size(egui::pos2(bx, rect.top()), Vec2::new(bkw, bkh));
        painter.rect_filled(bk_rect, 0.0, fill);
        painter.rect_stroke(bk_rect, 0.0, Stroke::new(1.0, outline_col));
    }
}

/// Given a click position within `rect`, return the MIDI note number hit (if any).
pub fn hit_test(pos: egui::Pos2, rect: Rect) -> Option<u8> {
    if !rect.contains(pos) { return None; }
    let x = pos.x - rect.left();
    let y = pos.y - rect.top();
    let w = rect.width();
    let h = rect.height();
    let wkw = w / WHITE_KEYS as f32;
    let bkw = wkw * 0.7;
    let bkh = h * 0.65;

    // Check black keys first (they overlap white keys)
    if y < bkh {
        for note in START..=END {
            if is_white(note) { continue; }
            let left_idx = match black_key_left_white(note) {
                Some(i) => i,
                None => continue,
            };
            if left_idx + 1 >= WHITE_KEYS { continue; }
            let x1 = left_idx as f32 * wkw;
            let x2 = (left_idx + 1) as f32 * wkw;
            let gap_center = (x1 + wkw + x2) / 2.0;
            let bx = gap_center - bkw / 2.0;
            if x >= bx && x <= bx + bkw {
                return Some(note);
            }
        }
    }

    // White key
    let idx = (x / wkw) as usize;
    let mut wi = 0usize;
    for note in START..=END {
        if !is_white(note) { continue; }
        if wi == idx { return Some(note); }
        wi += 1;
    }
    None
}

/// Return the preferred height for a given piano width.
pub fn height_for_width(width: f32) -> f32 {
    (width / PIANO_ASPECT).max(50.0)
}
