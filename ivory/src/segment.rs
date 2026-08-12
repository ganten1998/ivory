//! A virtual 16-segment display for the chord readout (supporter extra).
//!
//! Modelled on real alphanumeric LED/VFD modules: the display has a FIXED
//! number of character cells and every segment is always drawn — unlit ones at
//! a low alpha, exactly as you can see the dark segments of a real module. So
//! an idle display shows the whole grid faintly rather than going blank, and a
//! chord lights a subset of it.
//!
//! Sixteen segments (the classic layout) are enough for the full chord
//! vocabulary, including the symbols Ivory renders — `Δ` is the two upper
//! diagonals over the baseline, `+` is the centre cross, `ø` is a zero with its
//! slash, `°` is the upper cell.
//!
//! ```text
//!    a1  a2        segment indices
//!   f i j b        0:a1 1:a2 2:b  3:c  4:d1 5:d2 6:e  7:f
//!    g1  g2        8:g1 9:g2 10:h 11:i 12:j 13:k 14:l 15:m
//!   e k l m c
//!    d1  d2
//! ```

use egui::{Color32, Painter, Pos2, Rect, Stroke};

const A1: u16 = 1 << 0;
const A2: u16 = 1 << 1;
const B: u16 = 1 << 2;
const C: u16 = 1 << 3;
const D1: u16 = 1 << 4;
const D2: u16 = 1 << 5;
const E: u16 = 1 << 6;
const F: u16 = 1 << 7;
const G1: u16 = 1 << 8;
const G2: u16 = 1 << 9;
const H: u16 = 1 << 10;
const I: u16 = 1 << 11;
const J: u16 = 1 << 12;
const K: u16 = 1 << 13;
const L: u16 = 1 << 14;
const M: u16 = 1 << 15;

const OUTER: u16 = A1 | A2 | B | C | D1 | D2 | E | F;

/// Which segments light for a character. Unknown characters render blank
/// (all segments unlit) rather than a placeholder, so an unmapped symbol
/// degrades quietly instead of drawing garbage.
fn mask(ch: char) -> u16 {
    match ch {
        '0' => OUTER | J | K,
        '1' => B | C,
        '2' => A1 | A2 | B | G1 | G2 | E | D1 | D2,
        '3' => A1 | A2 | B | G1 | G2 | C | D1 | D2,
        '4' => F | G1 | G2 | B | C,
        '5' => A1 | A2 | F | G1 | G2 | C | D1 | D2,
        '6' => A1 | A2 | F | G1 | G2 | E | C | D1 | D2,
        '7' => A1 | A2 | B | C,
        '8' => OUTER | G1 | G2,
        '9' => A1 | A2 | F | B | G1 | G2 | C | D1 | D2,

        'A' => A1 | A2 | F | B | G1 | G2 | E | C,
        'B' => A1 | A2 | B | C | D1 | D2 | I | L | G2,
        'C' => A1 | A2 | F | E | D1 | D2,
        'D' => A1 | A2 | B | C | D1 | D2 | I | L,
        'E' => A1 | A2 | F | G1 | E | D1 | D2,
        'F' => A1 | A2 | F | G1 | E,
        'G' => A1 | A2 | F | E | D1 | D2 | C | G2,

        // Lowercase forms used by chord names: b (flat), m, s(us), a(dd), d, u,
        // n, o, i, t, l, e, r, h, j, g, y.
        'a' => G1 | G2 | E | D1 | D2 | C,
        'b' => F | E | D1 | D2 | C | G1 | G2,
        'd' => B | C | D1 | D2 | E | G1 | G2,
        'e' => A1 | A2 | F | G1 | G2 | E | D1 | D2,
        'g' => A1 | A2 | F | B | G1 | G2 | C | D1 | D2,
        'h' => F | E | G1 | G2 | C,
        'i' => L,
        'j' => B | C | D1 | D2,
        'l' => F | E,
        'm' => E | G1 | I | G2 | C,
        'n' => E | G1 | G2 | C,
        'o' => G1 | G2 | E | D1 | D2 | C,
        'r' => E | G1,
        's' => A1 | A2 | F | G1 | G2 | C | D1 | D2,
        't' => F | E | G1 | D1 | D2,
        'u' => E | D1 | D2 | C,
        'y' => F | G1 | G2 | B | C | D1 | D2,

        // Chord symbols.
        // Apex UP: the LOWER diagonals converge on the centre (K: bottom-left
        // -> centre, M: bottom-right -> centre), with the baseline closing it.
        // The upper pair (H|J) converge downward and render an upside-down V.
        'Δ' => K | M | D1 | D2,
        '°' => A1 | A2 | F | B | G1 | G2, // small ring in the upper cell
        'ø' => OUTER | J | K,             // zero with its slash
        '+' => G1 | G2 | I | L,           // centre cross
        '#' => G1 | G2 | I | L | B | E,   // cross plus the outer uprights
        '-' => G1 | G2,
        '/' => J | K,
        '\\' => H | M,
        '(' => H | K,
        ')' => J | M,
        ',' => K,
        '.' => L,
        ' ' => 0,
        _ => 0,
    }
}

/// Endpoints of each segment in a unit cell (x 0..1, y 0..2).
fn geometry(seg: u16) -> ((f32, f32), (f32, f32)) {
    match seg {
        A1 => ((0.0, 0.0), (0.5, 0.0)),
        A2 => ((0.5, 0.0), (1.0, 0.0)),
        F => ((0.0, 0.0), (0.0, 1.0)),
        B => ((1.0, 0.0), (1.0, 1.0)),
        G1 => ((0.0, 1.0), (0.5, 1.0)),
        G2 => ((0.5, 1.0), (1.0, 1.0)),
        E => ((0.0, 1.0), (0.0, 2.0)),
        C => ((1.0, 1.0), (1.0, 2.0)),
        D1 => ((0.0, 2.0), (0.5, 2.0)),
        D2 => ((0.5, 2.0), (1.0, 2.0)),
        I => ((0.5, 0.0), (0.5, 1.0)),
        L => ((0.5, 1.0), (0.5, 2.0)),
        H => ((0.0, 0.0), (0.5, 1.0)),
        J => ((1.0, 0.0), (0.5, 1.0)),
        K => ((0.0, 2.0), (0.5, 1.0)),
        M => ((1.0, 2.0), (0.5, 1.0)),
        _ => ((0.0, 0.0), (0.0, 0.0)),
    }
}

const SEGMENTS: [u16; 16] = [A1, A2, B, C, D1, D2, E, F, G1, G2, H, I, J, K, L, M];

/// Number of character cells. Fixed, like a real module — the display does not
/// resize with the text, and the unused cells stay visible but unlit.
pub const CELLS: usize = 8;

/// Draw `text` right-aligned-to-centre across a fixed grid of cells.
///
/// `color` lights a segment; every other segment is drawn at `unlit_alpha` of
/// the same hue, which is what makes it read as a physical display rather than
/// as text. Passing `None` (or a string that does not fit) still draws the full
/// unlit grid.
pub fn draw(painter: &Painter, rect: Rect, text: Option<&str>, color: Color32, unlit_alpha: f32) {
    let chars: Vec<char> = text.unwrap_or("").chars().take(CELLS).collect();
    // Centre the message in the grid so it reads like a centred readout.
    let offset = (CELLS - chars.len()) / 2;

    // Cell metrics: 1:2 aspect, with a gap between cells. Height drives size so
    // the display always fits its strip.
    // Real modules are a little wider than 1:2 and fill their panel; a strict
    // 1:2 on a wide, short strip left the display looking like a stamp.
    const ASPECT: f32 = 1.55; // height / width
    let gap_frac = 0.30; // of a cell width
    let total_units = CELLS as f32 * (1.0 + gap_frac) - gap_frac;
    // Vertical padding: a module sits INSIDE its panel with a visible margin
    // above and below, it does not run to the edges of the strip.
    let cell_h = rect.height() * 0.62;
    let cell_w = (cell_h / ASPECT).min(rect.width() * 0.92 / total_units.max(0.001));
    let cell_h = cell_w * ASPECT;
    let step = cell_w * (1.0 + gap_frac);
    let grid_w = CELLS as f32 * step - cell_w * gap_frac;

    let x0 = rect.center().x - grid_w / 2.0;
    let y0 = rect.center().y - cell_h / 2.0;
    // Segment thickness: a real module's segments are chunky relative to the
    // glyph. Slight rounding keeps the ends from looking like bare lines.
    let thick = (cell_w * 0.17).max(1.5);

    let dim = Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (255.0 * unlit_alpha).clamp(0.0, 255.0) as u8,
    );

    for cell in 0..CELLS {
        let lit = chars
            .get(cell.wrapping_sub(offset))
            .filter(|_| cell >= offset)
            .map(|&c| mask(c))
            .unwrap_or(0);
        let cx = x0 + cell as f32 * step;
        for seg in SEGMENTS {
            let ((ax, ay), (bx, by)) = geometry(seg);
            let a = Pos2::new(cx + ax * cell_w, y0 + ay * (cell_h / 2.0));
            let b = Pos2::new(cx + bx * cell_w, y0 + by * (cell_h / 2.0));
            let on = lit & seg != 0;
            painter.line_segment([a, b], Stroke::new(thick, if on { color } else { dim }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chord_character_ivory_emits_is_mapped() {
        // Sampled from real detector output across the whole vocabulary:
        // roots, qualities, tensions, slash basses, scales are excluded (they
        // are words, and the strip falls back to text for those).
        let vocabulary = "ABCDEFG0123456789bm#Δ°ø+/(),-adeghijlnorstuy ";
        for ch in vocabulary.chars() {
            assert!(
                mask(ch) != 0 || ch == ' ',
                "no segment mask for {ch:?} — it would render blank"
            );
        }
    }

    /// Orientation guard: `Δ` must be apex-UP. Built from the upper diagonals
    /// it renders as a V with a line under it, which is what shipped first.
    #[test]
    fn delta_is_a_triangle_not_an_upside_down_v() {
        let d = mask('Δ');
        assert!(d & K != 0 && d & M != 0, "needs the lower (upward) diagonals");
        assert!(d & H == 0 && d & J == 0, "upper diagonals point the wrong way");
        assert!(d & D1 != 0 && d & D2 != 0, "needs its baseline");
    }

    #[test]
    fn distinct_symbols_have_distinct_shapes() {
        // These are the ones most at risk of collapsing into each other.
        let pairs = [
            ('+', '#'),
            ('0', 'o'),
            ('Δ', 'A'),
            ('°', '0'),
            ('/', '\\'),
            ('(', ')'),
            ('b', '6'),
        ];
        for (a, b) in pairs {
            assert_ne!(mask(a), mask(b), "{a:?} and {b:?} render identically");
        }
    }

    #[test]
    fn unknown_characters_are_blank_not_garbage() {
        for ch in ['~', '\u{1F600}', '\t'] {
            assert_eq!(mask(ch), 0);
        }
    }

    #[test]
    fn every_segment_has_geometry() {
        for seg in SEGMENTS {
            let (a, b) = geometry(seg);
            assert_ne!(a, b, "segment {seg:#x} has zero length");
        }
    }
}
