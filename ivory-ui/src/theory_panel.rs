//! The theory band (D-UI-17): what you are playing, drawn as geometry.
//!
//! Three pictures of the same twelve notes, any combination of which can be on
//! at once. They share one band above everything else and split its width.
//!
//!   * the **circle of fifths**, which arranges keys so that neighbours share
//!     six of seven notes — the reason a modulation to the next sector sounds
//!     like a step and one to the opposite sector sounds like a shove,
//!   * the **Tonnetz**, a lattice where the three axes are the fifth, the major
//!     third and the minor third, so every triad is a triangle and two chords
//!     that share two notes are two triangles that share an edge, and
//!   * the **harmonic triangles**, I-IV-V pointing up with i-iv-v inverted
//!     through the same centre, which is the tonic-subdominant-dominant
//!     relationship as one shape you can slide to a new key.
//!
//! Like `piano.rs` and `fretboard_panel.rs`, this module is dumb: it is handed
//! a set of sounding pitch classes and it draws. It does not detect anything.
//! What it adds over those two is that a pitch class here has no octave and no
//! string — the whole point of these diagrams is what survives forgetting both.

use crate::fonts;
use crate::settings::Settings;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use std::time::{Duration, Instant};

/// Band height for a 1300pt-wide window. Large, because these are diagrams
/// rather than readouts: a circle of fifths with unreadable sector labels is
/// decoration. Scaled with everything else (spec §3.2).
pub const BAND_H_AT_1300: f64 = 300.0;

/// Height of the theory band for a window `w` points wide, or 0 when nothing
/// is selected. Truncated like every other band in the layout.
pub fn band_height(w: f32, views: &Views) -> f32 {
    if views.count() == 0 {
        return 0.0;
    }
    (BAND_H_AT_1300 * w as f64 / 1300.0).trunc() as f32
}

// ── the twelve notes ───────────────────────────────────────────────────────

const SHARP_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

fn pc_name(pc: u8, prefer_flats: bool) -> &'static str {
    let i = (pc % 12) as usize;
    if prefer_flats {
        FLAT_NAMES[i]
    } else {
        SHARP_NAMES[i]
    }
}

/// The real accidental characters. JetBrains Mono carries both and sits at the
/// bottom of every font chain, so they always render.
const FLAT: char = '\u{266D}';
const SHARP: char = '\u{266F}';

/// Draw a note name with its accidental raised to the top right, the way
/// music is set rather than the way a string concatenates.
///
/// `♭` and `♯` on the baseline at full size read as a second letter — which
/// is exactly the complaint that started this: a lower-case "Bb" was two b's,
/// one a note and one a flat, and no reader should have to work out which.
/// Raised and smaller, the accidental is unmistakably a modifier.
///
/// The pair is centred as a WHOLE: the letter shifts left by half the
/// accidental's width so "B♭" sits where "B" would, or a ring of note names
/// wobbles.
///
/// `suffix` is appended after the accidental at full size — the "m" of a
/// minor chord, which is part of the NAME rather than a modifier of the
/// letter, and so is not raised with it.
fn draw_note(
    painter: &Painter,
    center: Pos2,
    pc: u8,
    prefer_flats: bool,
    size: f32,
    color: Color32,
    lower: bool,
    suffix: &str,
) {
    let name = pc_name(pc, prefer_flats);
    let letter = if lower {
        name[..1].to_lowercase()
    } else {
        name[..1].to_owned()
    };
    let acc = match name.as_bytes().get(1) {
        Some(b'b') => Some(FLAT),
        Some(b'#') => Some(SHARP),
        _ => None,
    };
    let Some(acc) = acc else {
        painter.text(
            center,
            Align2::CENTER_CENTER,
            &format!("{letter}{suffix}"),
            font(size),
            color,
        );
        return;
    };
    let acc_size = size * 0.72;
    // Monospace-ish advance; close enough to centre the pair by eye and it
    // cannot be measured without laying the text out first.
    let acc_w = acc_size * 0.6 + suffix.chars().count() as f32 * size * 0.6;
    let r = painter.text(
        center - Vec2::new(acc_w * 0.5, 0.0),
        Align2::CENTER_CENTER,
        &letter,
        font(size),
        color,
    );
    let a = painter.text(
        Pos2::new(r.right(), r.top() + acc_size * 0.34),
        Align2::LEFT_CENTER,
        acc.to_string(),
        font(acc_size),
        color,
    );
    if !suffix.is_empty() {
        painter.text(
            Pos2::new(a.right(), center.y),
            Align2::LEFT_CENTER,
            suffix,
            font(size),
            color,
        );
    }
}

/// The twelve major keys in ascending fifths from C. Index is also the number
/// of sharps, up to the enharmonic seam at six.
const FIFTHS: [u8; 12] = [0, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10, 5];

/// The key signature at position `i` around the circle, as (sharps, flats).
/// Exactly one of the two is non-zero except at C, which has neither, and at
/// the seam, which is six of either depending on how you spell it.
fn signature(i: usize) -> (u8, u8) {
    match i {
        0 => (0, 0),
        1..=6 => (i as u8, 0),
        _ => (0, (12 - i) as u8),
    }
}

/// The major scale on `root`, as a 12-bit pitch-class mask.
pub fn major_scale(root: u8) -> u16 {
    const STEPS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
    STEPS
        .iter()
        .fold(0u16, |m, s| m | 1 << ((root as u16 + *s as u16) % 12))
}

/// A major triad as a pitch-class mask.
pub fn major_triad(root: u8) -> u16 {
    (1 << (root % 12)) | (1 << ((root + 4) % 12)) | (1 << ((root + 7) % 12))
}

/// A minor triad as a pitch-class mask.
pub fn minor_triad(root: u8) -> u16 {
    (1 << (root % 12)) | (1 << ((root + 3) % 12)) | (1 << ((root + 7) % 12))
}

fn has(mask: u16, pc: u8) -> bool {
    mask & (1 << (pc % 12)) != 0
}

/// Read the root and quality back off one of the app's own chord labels.
///
/// Deliberately narrow. These labels are generated by `ivory-core`, so the
/// leading note name is always one of the twelve spellings above, and a
/// lower-case `m` that is not the start of `maj` is always minor quality. It
/// returns None rather than guessing on anything else — a diagram that points
/// at the wrong tonic is worse than one that points nowhere, because there is
/// no way to tell from looking at it.
pub fn parse_label(label: &str) -> Option<(u8, bool)> {
    let b = label.as_bytes();
    let letter = *b.first()?;
    let base = match letter {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let (root, rest) = match b.get(1) {
        Some(b'#') => ((base + 1) % 12, &label[2..]),
        Some(b'b') => ((base + 11) % 12, &label[2..]),
        _ => (base, &label[1..]),
    };
    // "m" is minor; "maj" is not, and neither is the "m" inside "dim".
    let minor = rest.starts_with('m') && !rest.starts_with("maj");
    Some((root, minor))
}

// ── what is selected ───────────────────────────────────────────────────────

/// **Which elements are in the band, IN ORDER.**
///
/// An ordered list and not a set of flags, and that is the whole design. The
/// band used to be three bools, which can say what is showing and cannot say
/// where — so the arrangement was whatever order the enum happened to be
/// written in and there was no way for anybody to change it.
///
/// With a list, one interaction does both jobs: a number key removes its
/// element if it is showing and APPENDS it if it is not. Turning something off
/// and on again therefore moves it to the right, which is the whole of the
/// reordering vocabulary and takes no second control to learn. See
/// [`Views::toggled`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Views(Vec<View>);

impl Views {
    /// Every element, in the order the number keys are numbered.
    pub fn all() -> Views {
        Views(View::ALL.to_vec())
    }

    pub fn of(order: Vec<View>) -> Views {
        let mut seen = Vec::new();
        for v in order {
            if !seen.contains(&v) {
                seen.push(v);
            }
        }
        Views(seen)
    }

    pub fn order(&self) -> &[View] {
        &self.0
    }

    pub fn count(&self) -> usize {
        self.0.len()
    }

    pub fn any(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn contains(&self, v: View) -> bool {
        self.0.contains(&v)
    }

    /// Where `v` sits in the band, if it is in it. 0 is the leftmost.
    pub fn position(&self, v: View) -> Option<usize> {
        self.0.iter().position(|x| *x == v)
    }

    /// **Off if it is on; on at the RIGHT-HAND END if it is off.**
    ///
    /// Appending rather than restoring the element to its numbered place is the
    /// entire reordering mechanism: press a number twice and that element moves
    /// to the end, so any arrangement is reachable with nothing but the number
    /// row. Restoring it to a fixed slot would make the band's order permanent
    /// and the second press a no-op.
    pub fn toggled(&self, v: View) -> Views {
        let mut out = self.0.clone();
        match out.iter().position(|x| *x == v) {
            Some(i) => {
                out.remove(i);
            }
            None => out.push(v),
        }
        Views(out)
    }

    /// The order, as it goes in the settings file.
    pub fn keys(&self) -> String {
        self.0
            .iter()
            .map(|v| v.key())
            .collect::<Vec<_>>()
            .join("+")
    }

    /// Read back what `keys` wrote. Unknown names are dropped rather than
    /// refused, so a file written by a LATER build — one that knows a fifth
    /// diagram — still opens here with the four this build understands.
    pub fn from_keys(text: &str) -> Views {
        Views::of(text.split('+').filter_map(View::from_key).collect())
    }

}

/// One element of the theory band, for the menu and for the number keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Circle,
    Tonnetz,
    Triangles,
    /// The sheet music. A diagram like the others: it says what you are playing
    /// in the notation everybody already reads, and it is the most concrete of
    /// the four — which is why it is worth having beside the abstractions
    /// rather than in a band of its own. One chord at a time does not need the
    /// width of a window.
    Staff,
}

impl View {
    /// **The order the number keys are numbered in, and it must not be
    /// reshuffled.** `1` is this array's first element on every machine
    /// forever; moving one would silently rebind everybody's fingers. New
    /// elements go on the END.
    pub const ALL: [View; 4] = [View::Circle, View::Tonnetz, View::Triangles, View::Staff];

    pub fn label(self) -> &'static str {
        match self {
            View::Circle => "Circle of Fifths",
            View::Tonnetz => "Tonnetz",
            View::Triangles => "Harmonic Triangles",
            View::Staff => "Sheet Music",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            View::Circle => "circle",
            View::Tonnetz => "tonnetz",
            View::Triangles => "triangles",
            View::Staff => "staff",
        }
    }

    pub fn from_key(k: &str) -> Option<View> {
        View::ALL.into_iter().find(|v| v.key() == k)
    }

    /// The number key that toggles this element, 1-based.
    pub fn number(self) -> usize {
        View::ALL
            .iter()
            .position(|v| *v == self)
            .expect("every view is in ALL")
            + 1
    }

    /// Which element a number key means, if any.
    pub fn from_number(n: usize) -> Option<View> {
        (n >= 1).then(|| View::ALL.get(n - 1).copied()).flatten()
    }

    pub fn is_on(self, v: &Views) -> bool {
        v.contains(self)
    }
}

/// What the diagrams are drawing.
#[derive(Clone, Copy, Default, Debug)]
pub struct Input {
    /// Sounding pitch classes, bit `i` for pitch class `i`. Octaves are gone
    /// by design: none of these diagrams has an axis for them.
    pub pcs: u16,
    /// Lowest sounding pitch class. The fallback tonic when the detector has
    /// no name for what is being played, because a bass note is a better guess
    /// than C.
    pub bass: Option<u8>,
    /// Root and quality from the detected chord label, when there is one.
    pub root: Option<u8>,
    pub minor: bool,
}

impl Input {
    /// The tonic the key-centred diagrams orient themselves around: the
    /// detected root, else the bass, else C so the picture is still drawn.
    pub fn tonic(&self) -> u8 {
        self.root.or(self.bass).unwrap_or(0)
    }

    pub fn contains(&self, mask: u16) -> bool {
        self.pcs != 0 && (self.pcs & mask) == mask
    }
}

// ── palette ────────────────────────────────────────────────────────────────

struct Palette {
    bg: Color32,
    ink: Color32,
    faint: Color32,
    line: Color32,
    lit: Color32,
    lit_text: Color32,
    root: Color32,
}

/// The theory band's own background.
///
/// Public so that the camera pane, which sits INSIDE this band, can letterbox
/// against it. A camera surrounded by the Recorder's leather in the middle of a
/// cream band reads as a hole cut in the window rather than as part of it.
pub fn band_bg(s: &Settings) -> Color32 {
    palette(s).bg
}

fn palette(s: &Settings) -> Palette {
    let lit = s.white_key_active_color.to_color32();
    // `faint` and `line` carry the outlines, the roman numerals and the key
    // signatures — everything that says what the shapes MEAN. They were
    // pitched as background texture and were hard to read in both modes: a
    // 0x55 grey on near-black, and a 0xC8 on cream, are both about 2:1
    // against their background where text wants 4.5:1. Darkened on light and
    // lightened on dark until the numerals read at a glance, while staying
    // clearly below the note names in weight.
    if s.dark_mode {
        Palette {
            bg: Color32::from_rgb(0x0a, 0x0a, 0x0a),
            ink: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            faint: Color32::from_rgb(0x9a, 0x92, 0x80),
            line: Color32::from_rgb(0x62, 0x5c, 0x50),
            lit,
            lit_text: Color32::BLACK,
            root: s.sustain_color.to_color32(),
        }
    } else {
        Palette {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            ink: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            faint: Color32::from_rgb(0x6b, 0x60, 0x4a),
            line: Color32::from_rgb(0x9c, 0x8f, 0x74),
            lit,
            lit_text: Color32::WHITE,
            root: s.sustain_color.to_color32(),
        }
    }
}

fn font(size: f32) -> FontId {
    FontId::new(size, fonts::courier_bold())
}

fn font_light(size: f32) -> FontId {
    FontId::new(size, fonts::courier())
}

// ── the band ───────────────────────────────────────────────────────────────

/// Divide `rect` into one cell per selected view, left to right in a fixed
/// order so turning one off never reshuffles the others.
pub fn cells(rect: Rect, views: &Views) -> Vec<(View, Rect)> {
    let on = views.order().to_vec();
    if on.is_empty() {
        return Vec::new();
    }
    let w = rect.width() / on.len() as f32;
    on.into_iter()
        .enumerate()
        .map(|(i, v)| {
            (
                v,
                Rect::from_min_size(
                    Pos2::new(rect.min.x + w * i as f32, rect.min.y),
                    Vec2::new(w, rect.height()),
                ),
            )
        })
        .collect()
}

/// What a click landed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// One pitch class: a name on the circle, a node on the lattice.
    Pc(u8),
    /// A whole chord: a vertex of the harmonic triangles, which is a triad
    /// rather than a note and is much more useful placed all at once.
    Triad { root: u8, minor: bool },
}

/// What is under `pos`, if anything.
///
/// The exact inverse of `draw`, and deliberately in the same file a few lines
/// from it: a hit test that lives somewhere else is a hit test that stops
/// matching the picture the first time the picture moves.
pub fn hit_test(rect: Rect, views: &Views, input: Input, pos: Pos2) -> Option<Hit> {
    let (view, cell) = cells(rect, &views)
        .into_iter()
        .find(|(_, c)| c.contains(pos))?;
    let body = body_rect(cell.shrink(8.0));
    match view {
        View::Circle => circle_hit(body, pos).map(Hit::Pc),
        View::Tonnetz => tonnetz_hit(body, pos).map(Hit::Pc),
        // The sheet music is a READOUT, not a control. There is nothing on a
        // staff to click that would mean anything — a note on it is a note you
        // are already holding — and a cell that swallowed clicks would make the
        // band's behaviour depend on which panel happened to be under the
        // pointer.
        View::Staff => None,
        // The only one that needs to know what is playing: I, IV and V are
        // relative to a tonic, and the tonic comes from the notes.
        View::Triangles => triangles_hit(body, input.tonic(), pos),
    }
}

pub fn draw(
    painter: &Painter,
    rect: Rect,
    views: &Views,
    input: Input,
    notes: &std::collections::HashSet<u8>,
    readout: Option<crate::staff::Readout<'_>>,
    s: &Settings,
) {
    let p = palette(s);
    painter.rect_filled(rect, 0.0, p.bg);

    let cells = cells(rect, &views);
    for (i, (view, cell)) in cells.iter().enumerate() {
        // A hairline between panes, so three diagrams read as three and not as
        // one crowded one. Not drawn on the first, which has the band edge.
        if i > 0 {
            painter.line_segment(
                [
                    Pos2::new(cell.min.x, cell.min.y + 6.0),
                    Pos2::new(cell.min.x, cell.max.y - 6.0),
                ],
                Stroke::new(1.0_f32, p.line),
            );
        }
        let inner = cell.shrink(8.0);
        match view {
            View::Circle => draw_circle(painter, inner, input, &p, s),
            View::Tonnetz => draw_tonnetz(painter, inner, input, &p, s),
            View::Triangles => draw_triangles(painter, inner, input, &p, s),
            // The one element that needs the notes themselves rather than
            // their pitch classes: an octave is exactly what a staff is FOR,
            // and `Input` has thrown it away by design.
            //
            // Its title is drawn HERE rather than inside `staff::draw`, so the
            // four panels share one title in one font at one height. The legend
            // says which staves and which key, because those are two settings
            // that live in a right-click menu and would otherwise be invisible
            // — a band showing E flat major with nothing saying so is a band
            // that looks wrong to anybody who did not set it.
            View::Staff => {
                let legend = format!(
                    "{}  \u{2014}  {}",
                    s.staff_set().label(),
                    crate::staff::key_label(s.staff_key)
                );
                let body = title(painter, inner, "SHEET MUSIC", &legend, &p);
                crate::staff::draw(painter, body, notes, readout, s);
            }
        }
    }

    // Bottom edge, the same idea as the fretboard's top edge: the bands are
    // one window and need a seam or they read as one tall picture.
    painter.line_segment(
        [
            Pos2::new(rect.min.x, rect.max.y - 0.5),
            Pos2::new(rect.max.x, rect.max.y - 0.5),
        ],
        Stroke::new(1.0_f32, p.line),
    );
}

/// A title over each pane, in the space the diagrams leave for it.
/// A title and a one-line key to what the colours mean.
///
/// The legend is not decoration. Three diagrams showing three different things
/// in the same two colours is genuinely hard to read, and a viewer who has to
/// work out from context whether a lit shape means "this note is sounding" or
/// "this key fits what you are playing" is being asked to do the diagram's
/// job. One line each, in the faint ink, costs about 11 points of height.
/// What is left of a pane once its title and legend have taken their share.
///
/// Shared by the drawing and the hit-testing so the two cannot disagree about
/// where the diagram starts — the class of bug where a click lands one row
/// above the thing it looks like it is on.
fn body_rect(rect: Rect) -> Rect {
    let h = (rect.height() * 0.075).clamp(9.0, 14.0);
    let used = if rect.height() > 150.0 {
        h + h * 0.82 + 2.0
    } else {
        h + 2.0
    };
    Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + used), rect.max)
}

/// Advance width of one character as a fraction of the font size, for the
/// bundled monospaced faces. The same number `recorder_panel` uses, for the
/// same reason: sizing text to its box without laying it out first.
const LEGEND_ADV: f32 = 0.62;

fn title(painter: &Painter, rect: Rect, text: &str, legend: &str, p: &Palette) -> Rect {
    let h = (rect.height() * 0.075).clamp(9.0, 14.0);
    painter.text(
        Pos2::new(rect.center().x, rect.min.y + h * 0.5),
        Align2::CENTER_CENTER,
        text,
        font(h * 0.92),
        p.faint,
    );
    let lh = h * 0.82;
    // Dropped rather than crammed when the pane is too short for it: an
    // unreadable legend is worse than none, and it is the first thing that
    // should give up its space.
    //
    // **And too NARROW, which is the case that used to be missed.** These are
    // forty-odd characters of centred text; in a pane half the window's width
    // they ran out of both sides and collided with the legends either side of
    // them, so three diagrams read as one smear of overlapping type. Shrink to
    // fit, then give up — the same bargain the height already made.
    let fitted = (rect.width() * 0.98 / (legend.chars().count().max(1) as f32 * LEGEND_ADV))
        .min(lh * 0.88);
    if rect.height() > 150.0 && !legend.is_empty() && fitted >= 6.0 {
        painter.text(
            Pos2::new(rect.center().x, rect.min.y + h + lh * 0.5),
            Align2::CENTER_CENTER,
            legend,
            font_light(fitted),
            p.faint,
        );
    }
    body_rect(rect)
}

// ── circle of fifths ───────────────────────────────────────────────────────

/// Angle of circle position `i`, with C at twelve o'clock and fifths
/// clockwise. Screen y grows downward, so this is the usual convention with
/// the sign of the sine left alone.
fn circle_angle(i: usize) -> f32 {
    -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 12.0
}

fn draw_circle(painter: &Painter, rect: Rect, input: Input, p: &Palette, s: &Settings) {
    let rect = title(
        painter,
        rect,
        "CIRCLE OF FIFTHS",
        "filled = note sounding    shaded = key that fits",
        p,
    );
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5 - 2.0;
    if r < 20.0 {
        return;
    }

    let r_sig = r;
    let r_maj = r * 0.80;
    let r_min = r * 0.56;
    let r_hub = r * 0.34;
    let step = std::f32::consts::TAU / 12.0;

    // ── the wedges: which KEYS contain everything being played ─────────────
    //
    // One meaning, one treatment. This used to share the note colour and to
    // shade partial fits as well, so a single chord washed most of the circle
    // in the same blue that meant "this note is sounding" — two different
    // questions answered in one colour, which is what made it unreadable.
    //
    // Now: a key that contains ALL of it gets a neutral warm wash and a
    // boundary; a key that does not gets nothing. Nothing is a real answer.
    let full: Vec<bool> = FIFTHS
        .iter()
        .map(|&pc| input.pcs != 0 && input.pcs & !major_scale(pc) == 0)
        .collect();

    for (i, &fits) in full.iter().enumerate() {
        if !fits {
            continue;
        }
        let a0 = circle_angle(i) - step * 0.5;
        let mut poly = Vec::with_capacity(18);
        for k in 0..=8 {
            let t = a0 + step * k as f32 / 8.0;
            poly.push(c + Vec2::new(t.cos(), t.sin()) * r_sig);
        }
        for k in (0..=8).rev() {
            let t = a0 + step * k as f32 / 8.0;
            poly.push(c + Vec2::new(t.cos(), t.sin()) * r_hub);
        }
        painter.add(egui::Shape::convex_polygon(
            poly,
            p.ink.gamma_multiply(0.10),
            Stroke::NONE,
        ));
    }

    // ── the spokes and rings ───────────────────────────────────────────────
    for i in 0..12usize {
        let a0 = circle_angle(i) - step * 0.5;
        painter.line_segment(
            [
                c + Vec2::new(a0.cos(), a0.sin()) * r_hub,
                c + Vec2::new(a0.cos(), a0.sin()) * r_sig,
            ],
            Stroke::new(1.0_f32, p.line),
        );
    }
    for ring in [r_sig, r_maj, r_min, r_hub] {
        painter.circle_stroke(c, ring, Stroke::new(1.0_f32, p.line));
    }

    // ── the names ──────────────────────────────────────────────────────────
    for (i, &pc) in FIFTHS.iter().enumerate() {
        let a = circle_angle(i);
        let dir = Vec2::new(a.cos(), a.sin());
        let sounding = has(input.pcs, pc);

        // The major name, on a filled disc when THAT PITCH CLASS is sounding.
        // The disc is the same colour the piano paints a held key, which is
        // the whole point: the two displays agree by construction.
        let at = c + dir * ((r_maj + r_min) * 0.5);
        let name_size = r * 0.135;
        if sounding {
            painter.circle_filled(at, name_size * 0.95, p.lit);
            // The root is a sounding note WITH a ring, not a different fill —
            // so it reads as "and this one is the root" rather than as a
            // fourth unrelated colour.
            if input.root == Some(pc) {
                painter.circle_stroke(at, name_size * 0.95, Stroke::new(2.5_f32, p.root));
            }
        }
        draw_note(
            painter,
            at,
            pc,
            s.prefer_flats,
            name_size,
            if sounding { p.lit_text } else { p.ink },
            false,
            "",
        );

        // The relative minor, lower case, which is how a circle of fifths has
        // always been labelled and which also fits the inner wedge.
        //
        // A LABEL, not a second set of lights. Every pitch class appears twice
        // on this circle — the minor at position i is the major at position
        // i+3 — so lighting both rings put eight discs on the screen for a
        // four-note chord and said nothing the outer ring had not already
        // said. The wedge behind it still shades when that key fits.
        let rel = (pc + 9) % 12;
        draw_note(
            painter,
            c + dir * ((r_min + r_hub) * 0.5),
            rel,
            s.prefer_flats,
            r * 0.115,
            p.ink.gamma_multiply(0.8),
            true,
            "",
        );

        // Key signature, outermost. ASCII accidentals: neither U+266F nor
        // U+266D is in any bundled face, and both drew as tofu boxes.
        let (sharps, flats) = signature(i);
        let sig = match (sharps, flats) {
            (0, 0) => String::new(),
            (n, 0) => format!("{n}{SHARP}"),
            (0, n) => format!("{n}{FLAT}"),
            _ => String::new(),
        };
        if !sig.is_empty() {
            painter.text(
                c + dir * ((r_sig + r_maj) * 0.5),
                Align2::CENTER_CENTER,
                &sig,
                font_light(r * 0.095),
                p.faint,
            );
        }
    }

    // ── the hub: what the picture is of ────────────────────────────────────
    if input.pcs != 0 {
        let tonic = input.tonic();
        draw_note(
            painter,
            Pos2::new(c.x, c.y - r_hub * 0.28),
            tonic,
            s.prefer_flats,
            r * 0.19,
            p.ink,
            false,
            if input.minor { "m" } else { "" },
        );
        // Zero is the useful reading, not an error: it means what you are
        // playing belongs to no single major key.
        let n = full.iter().filter(|f| **f).count();
        painter.text(
            Pos2::new(c.x, c.y + r_hub * 0.38),
            Align2::CENTER_CENTER,
            &match n {
                0 => "no key fits".to_owned(),
                1 => "1 key".to_owned(),
                n => format!("{n} keys"),
            },
            font_light(r * 0.085),
            p.faint,
        );
    }
}

/// Which pitch class the point is on, if any. The inverse of the name ring
/// above, and kept next to it so the two cannot drift.
fn circle_hit(rect: Rect, pos: Pos2) -> Option<u8> {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5 - 2.0;
    if r < 20.0 {
        return None;
    }
    let v = pos - c;
    let d = v.length();
    // The major ring only. The signature ring, the hub and the relative-minor
    // ring are all labels rather than controls — and since every pitch class
    // already appears on the major ring, nothing is unreachable.
    if !(r * 0.56..=r * 0.80).contains(&d) {
        return None;
    }
    let step = std::f32::consts::TAU / 12.0;
    // Undo `circle_angle`: subtract the -90 degree offset and round to the
    // nearest twelfth.
    let a = v.y.atan2(v.x) + std::f32::consts::FRAC_PI_2;
    let i = (a / step).round().rem_euclid(12.0) as usize;
    Some(FIFTHS[i])
}

// ── Tonnetz ────────────────────────────────────────────────────────────────

/// The pitch class at lattice position (u, v): `u` fifths and `v` major thirds
/// from the origin. This one line is the whole Tonnetz — every property of the
/// diagram falls out of it.
pub fn tonnetz_pc(u: i32, v: i32, origin: u8) -> u8 {
    (((7 * u + 4 * v) % 12 + 12) % 12 + origin as i32) as u8 % 12
}

/// Where the Tonnetz nodes go inside a pane.
///
/// Pulled out of the drawing so it can be tested. The lattice is unbounded by
/// nature and a pane is not; every mistake this diagram can make is a mistake
/// about that boundary, and none of them is visible in a passing paint test.
struct Lattice {
    a: f32,
    h: f32,
    node_r: f32,
    rows: i32,
    cols: i32,
    x0: f32,
    y0: f32,
}

impl Lattice {
    /// Five rows shows the vertical period — the lattice repeats every three
    /// major thirds — with a row of context above and below it.
    const ROWS: i32 = 5;
    /// Breathing room at every edge, so no node ever sits exactly on it.
    const SLACK: f32 = 1.5;

    fn fit(rect: Rect) -> Option<Self> {
        let rows = Self::ROWS;
        let vspan = (rows - 1) as f32 * 0.8660254 + 0.6; // + a node either end
                                                         // A band this small is not a diagram. Bailing here also keeps `a_h`
                                                         // away from zero, which is what made the column count divide into
                                                         // infinity and then overflow `i32` on the way back — found by the
                                                         // every-size test, at a 40pt-wide window nobody would open on purpose.
        if !(rect.width() >= 32.0 && rect.height() >= vspan * 16.0) {
            return None;
        }
        // A pixel of slack, top and bottom.
        //
        // Without it the lattice is sized so its extent lands EXACTLY on the
        // pane edge — `a * vspan == rect.height()` by construction — and a
        // node whose bounding box touches the boundary fails `contains_rect`
        // by a rounding error, so `shows()` drops it. The whole bottom row
        // disappears and the diagram reads as cut off. It only showed below
        // 100% window size, because that is where the arithmetic stopped
        // landing on whole pixels.
        let a_h = (rect.height() - Self::SLACK * 2.0).max(1.0) / vspan;
        // Columns are whatever the width affords at that spacing, never fewer
        // than five — below that the horizontal axis stops showing a run of
        // fifths — and never so many that the names stop being readable.
        let shear = (rows - 1) as f32 * 0.5;
        let cols = ((((rect.width() - Self::SLACK * 2.0) / a_h) - shear - 0.6).floor() + 1.0)
            .clamp(5.0, 13.0) as i32;
        let a_w = (rect.width() - Self::SLACK * 2.0).max(1.0) / ((cols - 1) as f32 + shear + 0.6);
        let a = a_h.min(a_w);
        if a < 16.0 {
            return None;
        }
        let h = a * 0.8660254; // equilateral: a * sqrt(3)/2
                               // The lattice is a sheared rhombus, so its bounding box is wider than
                               // the node grid by the shear. Centre the BOX, not the grid, or it
                               // sits left.
        let w_used = (cols - 1) as f32 * a + shear * a;
        let h_used = (rows - 1) as f32 * h;
        Some(Self {
            a,
            h,
            node_r: a * 0.30,
            rows,
            cols,
            x0: rect.center().x - w_used * 0.5,
            y0: rect.center().y + h_used * 0.5,
        })
    }

    fn at(&self, u: i32, v: i32) -> Pos2 {
        Pos2::new(
            self.x0 + u as f32 * self.a + v as f32 * self.a * 0.5,
            self.y0 - v as f32 * self.h,
        )
    }

    /// Whether a node at (u, v) is drawn.
    ///
    /// Two conditions, and the second is easy to forget. It has to be INSIDE
    /// THE GRID — the triangle loop walks `-1..cols` so it can reach the cells
    /// on the left edge, and a corner outside `0..cols` is a node that is
    /// never drawn however well it fits. And it has to fit WHOLE: a node
    /// clipped in half by the pane edge is a note name that is not a note
    /// name, which is how an "Eb" once appeared as "Kb".
    fn shows(&self, rect: Rect, u: i32, v: i32) -> bool {
        (0..self.cols).contains(&u)
            && (0..self.rows).contains(&v)
            && rect.contains_rect(Rect::from_center_size(
                self.at(u, v),
                Vec2::splat(self.node_r * 2.0),
            ))
    }
}

fn draw_tonnetz(painter: &Painter, rect: Rect, input: Input, p: &Palette, s: &Settings) {
    let rect = title(
        painter,
        rect,
        "TONNETZ",
        "solid triangle = major    outlined = minor",
        p,
    );
    // Everything below stays inside the pane. The lattice is unbounded by
    // nature, so without this it happily draws over the neighbouring diagram.
    let painter = &painter.with_clip_rect(rect);

    let Some(l) = Lattice::fit(rect) else {
        return;
    };
    let (rows, cols, node_r) = (l.rows, l.cols, l.node_r);
    // The lattice is ANCHORED at C rather than re-centred on the tonic.
    //
    // Re-centring looked clever and is wrong once the diagram is clickable:
    // placing a note changes the tonic, which slides the whole lattice, so the
    // node you just clicked is no longer under your finger. Nothing is lost by
    // anchoring — every pitch class appears somewhere on the grid, which
    // `the_tonnetz_still_has_a_lattice_left` asserts — and the tonic is marked
    // by the same ring it gets on the other two diagrams.
    let origin = 0u8;
    let at = |u: i32, v: i32| l.at(u, v);

    // Triads first, so the nodes sit on top of their triangles.
    //
    // Two triangles per cell, and they are the two chord qualities: the one
    // pointing up is {p, p+7, p+4}, a major triad on p; the one pointing down
    // is {p+7, p+4, p+11}, a minor triad on p+4. That is the fact the whole
    // diagram is for, and it needs no table.
    for v in 0..rows {
        for u in -1..cols {
            // Major and minor differ by TREATMENT, not by hue: solid versus
            // outlined, both in the note colour. They used to be blue and
            // orange, and orange was also the root marker — so a lit minor
            // triad and a root note said the same thing in the same colour
            // while meaning different things.
            for (corners, minor) in [
                ([(u, v), (u + 1, v), (u, v + 1)], false),
                ([(u + 1, v), (u, v + 1), (u + 1, v + 1)], true),
            ] {
                let mask = corners
                    .iter()
                    .fold(0u16, |m, &(u, v)| m | 1 << tonnetz_pc(u, v, origin));
                if !input.contains(mask) {
                    continue;
                }
                // Only between nodes that are actually drawn. The lattice is
                // clipped to the pane, so a triangle reaching past the last
                // visible column used to be cut off mid-edge and read as an
                // unfinished shape rather than a chord.
                if !corners.iter().all(|&(u, v)| l.shows(rect, u, v)) {
                    continue;
                }
                let pts: Vec<Pos2> = corners.iter().map(|&(u, v)| at(u, v)).collect();
                if minor {
                    painter.add(egui::Shape::convex_polygon(
                        pts.clone(),
                        p.lit.gamma_multiply(0.16),
                        Stroke::NONE,
                    ));
                    for k in 0..3 {
                        painter
                            .line_segment([pts[k], pts[(k + 1) % 3]], Stroke::new(2.0_f32, p.lit));
                    }
                } else {
                    painter.add(egui::Shape::convex_polygon(
                        pts,
                        p.lit.gamma_multiply(0.50),
                        Stroke::NONE,
                    ));
                }
            }
        }
    }

    // The three axes as edges. Every edge is an interval: horizontal a fifth,
    // up-right a major third, up-left a minor third.
    for v in 0..=rows {
        for u in -1..=cols {
            let from = at(u, v);
            for (du, dv) in [(1, 0), (0, 1), (-1, 1)] {
                painter.line_segment([from, at(u + du, v + dv)], Stroke::new(1.3_f32, p.line));
            }
        }
    }

    for v in 0..rows {
        for u in 0..cols {
            if !l.shows(rect, u, v) {
                continue;
            }
            let c = at(u, v);
            let pc = tonnetz_pc(u, v, origin);
            let lit = has(input.pcs, pc);
            let fill = if lit { p.lit } else { p.bg };
            painter.circle(
                c,
                node_r,
                fill,
                Stroke::new(1.4_f32, if lit { fill } else { p.faint }),
            );
            // The root, ringed rather than recoloured — the same mark it gets
            // on the circle of fifths, so one glance learns it once.
            if input.root == Some(pc) || (input.root.is_none() && lit && pc == origin) {
                painter.circle_stroke(c, node_r, Stroke::new(2.5_f32, p.root));
            }
            draw_note(
                painter,
                c,
                pc,
                s.prefer_flats,
                node_r * 0.95,
                if lit { p.lit_text } else { p.ink },
                false,
                "",
            );
        }
    }
}

/// The lattice node under `pos`, if any.
fn tonnetz_hit(rect: Rect, pos: Pos2) -> Option<u8> {
    let l = Lattice::fit(rect)?;
    for v in 0..l.rows {
        for u in 0..l.cols {
            if l.shows(rect, u, v) && (l.at(u, v) - pos).length() <= l.node_r {
                return Some(tonnetz_pc(u, v, 0));
            }
        }
    }
    None
}

// ── harmonic triangles ─────────────────────────────────────────────────────

/// Vertex `k` of the upward or downward triangle.
///
/// One definition, used by the drawing and by the hit test, because a click
/// that lands next to the circle it looks like it is on is the failure this
/// shape is most prone to.
fn hex_vertex(c: Pos2, r: f32, k: usize, down: bool) -> Pos2 {
    let base = if down {
        std::f32::consts::FRAC_PI_2
    } else {
        -std::f32::consts::FRAC_PI_2
    };
    let a = base + k as f32 * std::f32::consts::TAU / 3.0;
    c + Vec2::new(a.cos(), a.sin()) * r
}

/// Vertex k of the upward triangle is I at the apex, then V and IV going
/// clockwise, so the dominant is on the right where it is usually drawn.
const HEX_UP_ORDER: [usize; 3] = [0, 2, 1];
const HEX_DOWN_ORDER: [usize; 3] = [0, 1, 2];

/// Distance from the centre of the harmonic-triangle figure to a vertex.
///
/// The drawn shape is `r` to a vertex PLUS a node radius beyond it, and the
/// node radius is 0.30r, so the real extent is 1.30r and the pane has to hold
/// 2.60r. Sizing on `r` alone put the tonic node through the pane's own title.
fn hexagram_radius(rect: Rect) -> f32 {
    rect.width().min(rect.height()) * 0.5 / 1.32
}

fn draw_triangles(painter: &Painter, rect: Rect, input: Input, p: &Palette, s: &Settings) {
    let rect = title(
        painter,
        rect,
        "HARMONIC TRIANGLES",
        "filled = you are playing that chord",
        p,
    );
    let c = rect.center();
    let r = hexagram_radius(rect);
    if r < 20.0 {
        return;
    }

    let tonic = input.tonic();
    // I, IV, V — the tonic, the fifth below it and the fifth above it. Written
    // as -5 and +7 rather than +5 and +7 so the arithmetic says what the
    // relationship is.
    let roots = [tonic, (tonic + 5) % 12, (tonic + 7) % 12];
    let numerals_major = ["I", "IV", "V"];
    let numerals_minor = ["i", "iv", "v"];

    // The upward triangle carries the major chords, the downward one the
    // minor. Sharing a centre makes them one shape rather than two diagrams,
    // which is the point: sliding it is a modulation.
    let up = |k: usize| hex_vertex(c, r, k, false);
    let down = |k: usize| hex_vertex(c, r, k, true);
    let up_order = HEX_UP_ORDER;
    let down_order = HEX_DOWN_ORDER;

    let tri = |f: &dyn Fn(usize) -> Pos2| [f(0), f(1), f(2)];

    for (pts, minor) in [(tri(&up), false), (tri(&down), true)] {
        let stroke = Stroke::new(if minor { 1.4_f32 } else { 2.0 }, p.faint);
        for k in 0..3 {
            painter.line_segment([pts[k], pts[(k + 1) % 3]], stroke);
        }
    }

    let node_r = r * 0.30;
    let draw_vertex = |pos: Pos2, root: u8, minor: bool, numeral: &str| {
        let mask = if minor {
            minor_triad(root)
        } else {
            major_triad(root)
        };
        let lit = input.contains(mask);
        let fill = if lit { p.lit } else { p.bg };
        painter.circle(
            pos,
            node_r,
            fill,
            Stroke::new(
                if lit { 2.0_f32 } else { 1.0 },
                if lit { p.lit } else { p.faint },
            ),
        );
        // The tonic, ringed, exactly as on the other two diagrams — but only
        // on the triangle whose QUALITY matches. These vertices are CHORDS,
        // so ringing both C and Cm said "the root is C" twice, and the unlit
        // one read as important-but-silent.
        if input.root == Some(root) && input.minor == minor {
            painter.circle_stroke(pos, node_r, Stroke::new(2.5_f32, p.root));
        }
        draw_note(
            painter,
            Pos2::new(pos.x, pos.y - node_r * 0.22),
            root,
            s.prefer_flats,
            node_r * 0.62,
            if lit { p.lit_text } else { p.ink },
            false,
            if minor { "m" } else { "" },
        );
        painter.text(
            Pos2::new(pos.x, pos.y + node_r * 0.42),
            Align2::CENTER_CENTER,
            numeral,
            font(node_r * 0.46),
            if lit {
                p.lit_text
            } else {
                p.ink.gamma_multiply(0.85)
            },
        );
    };

    for (k, &oi) in up_order.iter().enumerate() {
        draw_vertex(up(k), roots[oi], false, numerals_major[oi]);
    }
    for (k, &oi) in down_order.iter().enumerate() {
        draw_vertex(down(k), roots[oi], true, numerals_minor[oi]);
    }

    // The key in the middle of its own triangle.
    draw_note(
        painter,
        c,
        tonic,
        s.prefer_flats,
        r * 0.24,
        p.ink.gamma_multiply(0.55),
        false,
        "",
    );
}

/// The chord vertex under `pos`, if any.
///
/// Returns a whole triad rather than a note: these vertices ARE chords, and
/// placing one note of a chord you pointed at would be a strange thing for a
/// diagram of chords to do.
fn triangles_hit(rect: Rect, tonic: u8, pos: Pos2) -> Option<Hit> {
    let c = rect.center();
    let r = hexagram_radius(rect);
    if r < 20.0 {
        return None;
    }
    let node_r = r * 0.30;
    let roots = [tonic, (tonic + 5) % 12, (tonic + 7) % 12];
    for (k, &oi) in HEX_UP_ORDER.iter().enumerate() {
        if (hex_vertex(c, r, k, false) - pos).length() <= node_r {
            return Some(Hit::Triad {
                root: roots[oi],
                minor: false,
            });
        }
    }
    for (k, &oi) in HEX_DOWN_ORDER.iter().enumerate() {
        if (hex_vertex(c, r, k, true) - pos).length() <= node_r {
            return Some(Hit::Triad {
                root: roots[oi],
                minor: true,
            });
        }
    }
    None
}

/// A one-pixel seam over the top of the band, matching the fretboard's.
pub fn draw_bottom_edge(painter: &Painter, rect: Rect, s: &Settings) {
    let p = palette(s);
    painter.rect_stroke(
        Rect::from_min_max(
            Pos2::new(rect.min.x, rect.max.y - 1.0),
            Pos2::new(rect.max.x, rect.max.y),
        ),
        0.0,
        Stroke::new(1.0_f32, p.line),
        StrokeKind::Inside,
    );
}

// ── the popped-out window ──────────────────────────────────────────────────
//
// The third detachable band, and deliberately the same shape of code as the
// other two (`chord_strip::show_detached_window`, `fretboard_panel::
// show_detached_window`): same close-to-reattach, same right-click-anywhere
// menu, same borderless drag-anywhere, same window level. Three popouts with
// three sets of habits would be worse than none.
//
// What is NOT the same is that this one is an INPUT. The chord strip is a
// readout and the neck's detached window is a picture; a click on a circle
// name or a chord vertex places notes. Everything below that differs from the
// other two windows — the hit reported in the outcome, the drag that has to
// stand aside for it, the ctrl-click guard — exists for that one reason.

/// Default size for the popped-out theory window when nothing is remembered.
///
/// Three panes of roughly 300x380 rather than a slice of the band. At 100% the
/// band is 1300x300, so a pane is 433 wide and 300 tall — a shape chosen by the
/// piano above it rather than by the diagrams. A circle and a hexagram both
/// want to be square, and the Tonnetz wants height for its five rows. In its
/// own window it should be legible, not a slice of the main one, which is the
/// same argument `fretboard_panel::DETACHED_DEFAULT` makes for the neck.
///
/// Note this is not `band_height(900.0, ..)` (207) and must not become it: the
/// window's height is the user's to choose and the band's is not.
pub const DETACHED_DEFAULT: Vec2 = Vec2::new(900.0, 380.0);

/// The smallest window that still holds all three diagrams at once.
///
/// Measured rather than guessed, and pinned by the test of the same name below.
/// With three panes the pane is `w/3 - 16` wide, and `Lattice::fit` gives up
/// below `a = 16`, which happens at about 374pt of window; the circle and the
/// hexagram both give up below `r = 20` and reach it later. 400x240 clears all
/// three with a little room to spare.
///
/// Below this a selected pane is a title over an empty rectangle, which reads
/// as a rendering fault rather than as "too small" — the same failure the
/// Tonnetz's own `Lattice::fit` bail-out produces, and the reason to put a
/// floor under the window instead of letting the user find it.
/// **Four panes, not three.** It was 400 when the band held three diagrams,
/// and the sheet music made four without the constant moving — so the shipped
/// default selection at the minimum size drew the Tonnetz as a title over an
/// empty rectangle, which is precisely the failure this number exists to stop.
pub const DETACHED_MIN: Vec2 = Vec2::new(540.0, 240.0);

pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("ivory-theory-window")
}

/// Outline for the popped-out window. The band's own background is near-black
/// in dark mode and cream in light, so neither theme's fill can be relied on to
/// separate the window from the desktop behind it. One neutral grey reads
/// against both, exactly as it does for the other two popouts.
pub const BORDER_COLOR: Color32 = Color32::from_gray(0x5A);

/// What the app has to act on after showing the window.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct DetachedOutcome {
    /// The user closed the window. Close-to-reattach: the band comes back.
    pub close_requested: bool,
    /// Live inner size in points, recorded every frame. Feed it to
    /// `GeometryGuard::observe` rather than to settings directly.
    pub inner_size: Option<Vec2>,
    /// Live outer position in monitor coordinates, recorded every frame.
    pub outer_pos: Option<Pos2>,
    /// Right-click (or ctrl-click on macOS) happened at this monitor-space
    /// position, for the app context menu.
    pub context_menu_at: Option<Pos2>,
    /// A click landed on something. The window is an instrument like the band
    /// is: `Hit::Pc` places one note, `Hit::Triad` places a whole chord. Feed
    /// it to the same handler the band's clicks go to, with no extra gating —
    /// keytoggle has already been consulted (see `show_detached_window`).
    pub hit: Option<Hit>,
}

/// The two lines shown when no diagram is selected.
///
/// The band's answer to an empty selection is to take zero height and vanish,
/// which is right for a band and impossible for a window: a window that is
/// already open cannot un-exist, and shrinking it to nothing or leaving it
/// blank both read as the app having crashed in that corner of the screen.
/// So it says what happened, and how to undo it.
const EMPTY_TITLE: &str = "NO DIAGRAMS SELECTED";
const EMPTY_HINT: &str = "right-click here for the Theory menu";

/// The window's contents.
///
/// **This calls the band's own `draw` with the window's rect and nothing else.**
/// That is the whole defence for requirement that a click keeps landing on what
/// it looks like it is on: `hit_test` is the inverse of `draw`, so the moment
/// the window gets a second drawing path — its own spacing, its own title
/// height, a "detached" layout tweak — the inverse stops being an inverse and
/// every click in the window lands somewhere slightly wrong, invisibly. There
/// is one picture, drawn at whatever rect it is handed.
fn draw_detached(
    painter: &Painter,
    rect: Rect,
    views: &Views,
    input: Input,
    notes: &std::collections::HashSet<u8>,
    readout: Option<crate::staff::Readout<'_>>,
    s: &Settings,
) {
    if views.any() {
        draw(painter, rect, views, input, notes, readout, s);
    } else {
        let p = palette(s);
        painter.rect_filled(rect, 0.0, p.bg);
        // Sized off the window rather than fixed: this window can be dragged
        // from 240pt tall to a full screen, and a 14pt notice is a whisper at
        // one end and lost at the other.
        let size = (rect.height() * 0.055).clamp(11.0, 22.0);
        painter.text(
            Pos2::new(rect.center().x, rect.center().y - size * 0.9),
            Align2::CENTER_CENTER,
            EMPTY_TITLE,
            font(size),
            p.ink.gamma_multiply(0.75),
        );
        painter.text(
            Pos2::new(rect.center().x, rect.center().y + size * 0.9),
            Align2::CENTER_CENTER,
            EMPTY_HINT,
            font_light(size * 0.8),
            p.faint,
        );
    }
}

/// The theory diagrams in their own window.
///
/// `builder_size` and `builder_pos` must stay constant for the lifetime of one
/// detachment, exactly as for the other two windows: egui diffs this builder
/// against the previous frame's builder rather than against the window's real
/// geometry, so a value that changed every frame would fight the user's own
/// resizes and drags.
///
/// `main_focused` decides the window LEVEL. A detached window is a piece of the
/// same app, so it rises and falls WITH the piano rather than being left
/// wherever the window stack last put it. By level rather than by raising:
/// always-on-top while we are frontmost is exactly "above our own window", and
/// dropping to Normal when we are not means it never floats over other
/// applications.
#[allow(clippy::too_many_arguments)]
pub fn show_detached_window(
    ctx: &egui::Context,
    builder_size: Vec2,
    builder_pos: Option<Pos2>,
    borderless: bool,
    main_focused: bool,
    views: &Views,
    input: Input,
    notes: &std::collections::HashSet<u8>,
    readout: Option<crate::staff::Readout<'_>>,
    s: &Settings,
) -> DetachedOutcome {
    let mut outcome = DetachedOutcome::default();
    let mut builder = egui::ViewportBuilder::default()
        .with_title("Tangent")
        .with_inner_size(builder_size)
        .with_min_inner_size(DETACHED_MIN)
        .with_resizable(true)
        .with_decorations(!borderless)
        .with_window_level(if main_focused {
            egui::viewport::WindowLevel::AlwaysOnTop
        } else {
            egui::viewport::WindowLevel::Normal
        });
    if let Some(pos) = builder_pos {
        builder = builder.with_position(pos);
    }

    ctx.show_viewport_immediate(viewport_id(), builder, |vp, _class| {
        crate::shell::viewport_ui(vp, |ui| {
            // THE WINDOW'S OWN RECT, used for both the drawing and the hit
            // test below. The band's rect is a completely different rectangle
            // — different width, different height, different origin — and a
            // hit test run against it would return the note that is under
            // that point IN THE MAIN WINDOW, which is a note the user cannot
            // see and did not point at. Nothing here may reach for a band
            // rect, which is why none is passed in.
            let rect = ui.max_rect();
            draw_detached(ui.painter(), rect, views, input, notes, readout, s);
            painter_border(ui.painter(), rect);

            let (close, inner_rect, outer_rect, pressed, secondary, pointer, ctrl) =
                ui.input(|i| {
                    (
                        i.viewport().close_requested(),
                        i.viewport().inner_rect,
                        i.viewport().outer_rect,
                        i.pointer.primary_pressed(),
                        i.pointer.secondary_clicked(),
                        i.pointer.interact_pos(),
                        i.modifiers.ctrl,
                    )
                });

            outcome.close_requested = close;
            // The fallback matters in a host that reports no window geometry
            // at all: without it the guard sees `None` forever and the window
            // is never remembered, rather than never poisoned.
            outcome.inner_size = inner_rect.map(|r| r.size()).or(Some(rect.size()));
            outcome.outer_pos = outer_rect.map(|r| r.min);

            // Ctrl-click IS the right-click on macOS, and this window is the
            // only detachable one where getting that wrong does damage: the
            // main window guards it (`app.rs`: `primary_pressed &&
            // !ctrl_as_context`) because a ctrl-click reaching the keytoggle
            // hit test enters a note instead of opening the menu. The chord
            // strip and the neck have no click targets, so they never needed
            // it. This window has six chord vertices and twelve note names.
            let ctrl_as_context = cfg!(target_os = "macos") && ctrl;
            let menu = secondary || (pressed && ctrl_as_context);
            if menu {
                if let (Some(pos), Some(inner)) = (pointer, inner_rect) {
                    outcome.context_menu_at = Some(inner.min + pos.to_vec2());
                }
            }

            // Keytoggle is consulted HERE rather than by the caller, and the
            // reason is the drag below. If the window reported a hit that the
            // app then discarded because keytoggle is off, a borderless press
            // on a chord vertex would stand aside for a drag that never
            // happens and do nothing at all — a dead patch of window.
            if pressed && !menu && s.keytoggle_enabled {
                if let Some(p) = pointer {
                    outcome.hit = hit_test(rect, &views, input, p);
                }
            }

            // Borderless drag-anywhere, minus the parts that are not "anywhere".
            // With no title bar the whole window is the drag handle, so an
            // unguarded StartDrag means clicking a chord vertex picks the
            // window up and carries it off instead of placing the triad —
            // which looks exactly like a broken diagram, since the note never
            // appears. The press only becomes a drag when it hit nothing.
            if borderless && pressed && !menu && outcome.hit.is_none() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        });
    });
    outcome
}

fn painter_border(painter: &Painter, rect: Rect) {
    painter.rect_stroke(
        rect.shrink(0.5),
        0.0,
        Stroke::new(1.0_f32, BORDER_COLOR),
        StrokeKind::Middle,
    );
}

// ── whose size is it anyway ────────────────────────────────────────────────

/// Slack when comparing the size a window actually has against the size it was
/// asked for. Rounding and DPI scaling move things by a point or two.
pub const WM_SIZE_TOLERANCE: f32 = 8.0;

/// How long after the window appears a size mismatch still counts as the window
/// manager's doing rather than the user's.
///
/// A tiling WM overrules the requested size at creation, within a frame or two.
/// A person reaching for a window edge takes far longer than this. Size alone
/// cannot tell the two apart — the observed size is just a number either way —
/// so only timing can.
pub const WM_GRACE: Duration = Duration::from_millis(600);

/// Whether something other than us is sizing this window.
///
/// Under a tiling window manager (AeroSpace, yabai, i3) the size we ask for is
/// simply overruled. Recording the result as if the user had chosen it is worse
/// than recording nothing: the tiled geometry then follows them into every
/// later session, including ones where nothing is tiling. That is exactly how a
/// `detached_chord_height` of 1377 ended up in a settings file — a chord strip
/// the height of a monitor, restored faithfully on a machine with no tiling WM
/// running.
pub fn wm_overrode_size(observed: Vec2, requested: Vec2) -> bool {
    (observed.x - requested.x).abs() > WM_SIZE_TOLERANCE
        || (observed.y - requested.y).abs() > WM_SIZE_TOLERANCE
}

/// Whether this window's geometry is the user's to remember.
///
/// The rule `app.rs` already applies to the chord and fretboard windows, made
/// into a thing that can be TESTED. There it is six lines of `if let` inline in
/// a 3000-line update loop, exercised only by running the app under a tiling
/// WM; here the same rule takes an `Instant` as an argument, so
/// `the_window_manager_cannot_write_its_own_size_into_settings` can drive it
/// through both sides of the grace period in a millisecond.
///
/// The safety property is in the return types, not in the caller's discipline:
/// `remembered_size` and `remembered_pos` yield `None` for the whole session
/// once the WM has been caught sizing the window, so there is no path by which
/// a tiled geometry reaches settings.json. A caller that forgets to ask stores
/// nothing, rather than storing 1377.
#[derive(Debug, Clone, Copy)]
pub struct GeometryGuard {
    /// The size the builder asked for, which is what the observed size is
    /// judged against.
    requested: Vec2,
    /// When the window appeared. Cleared once the grace period is over, at
    /// which point mismatches are the user resizing.
    shown_at: Option<Instant>,
    wm_managed: bool,
    live_size: Option<Vec2>,
    live_pos: Option<Pos2>,
}

impl GeometryGuard {
    /// Call when the window is opened — on detach, and again on the frame that
    /// recreates it at startup. `requested` is the `builder_size` handed to
    /// `show_detached_window`, and must be the same value for as long as this
    /// guard lives, for the same reason the builder itself must be.
    pub fn opened(requested: Vec2, now: Instant) -> Self {
        Self {
            requested,
            shown_at: Some(now),
            wm_managed: false,
            live_size: None,
            live_pos: None,
        }
    }

    /// Feed one frame's outcome. Returns true when the geometry CHANGED and is
    /// ours to remember, which is the app's cue to arm its debounced save.
    pub fn observe(&mut self, outcome: &DetachedOutcome, now: Instant) -> bool {
        self.observe_geometry(outcome.inner_size, outcome.outer_pos, now)
    }

    /// The same thing, for a popout whose outcome type is not this module's.
    ///
    /// The guard is about window managers, not about theory diagrams — the
    /// module it happens to live in is an accident of which popout was written
    /// first. The Recorder band is the fourth surface to need exactly this
    /// dance, and taking the two fields rather than a foreign struct is what
    /// stops the fifth one from copying it again.
    pub fn observe_geometry(
        &mut self,
        inner_size: Option<Vec2>,
        outer_pos: Option<Pos2>,
        now: Instant,
    ) -> bool {
        if let (Some(shown), Some(size)) = (self.shown_at, inner_size) {
            if now.duration_since(shown) < WM_GRACE {
                // `|=`, not `=`. A tiling WM can hand back the requested size
                // on one frame and its own on the next (AeroSpace resizes
                // after the window is mapped), and a guard that could be
                // un-set by a single agreeing frame would let the next frame's
                // tiled size through.
                self.wm_managed |= wm_overrode_size(size, self.requested);
            } else {
                self.shown_at = None;
            }
        }
        let moved = inner_size != self.live_size
            || (outer_pos.is_some() && outer_pos != self.live_pos);
        if let Some(size) = inner_size {
            self.live_size = Some(size);
        }
        if let Some(pos) = outer_pos {
            self.live_pos = Some(pos);
        }
        moved && !self.wm_managed
    }

    /// The last size the window reported, if any.
    pub fn live_size(&self) -> Option<Vec2> {
        self.live_size
    }

    /// The last position the window reported, if any.
    pub fn live_pos(&self) -> Option<Pos2> {
        self.live_pos
    }

    /// Whether a window manager has been caught sizing this window.
    pub fn wm_managed(&self) -> bool {
        self.wm_managed
    }

    /// The size to write to settings, or `None` when it is not ours to write.
    pub fn remembered_size(&self) -> Option<Vec2> {
        (!self.wm_managed).then_some(self.live_size).flatten()
    }

    /// The position to write to settings, or `None` when it is not ours.
    ///
    /// Tied to the same flag as the size, deliberately. A tiling WM moves a
    /// window as well as resizing it, and remembering "the size is not mine
    /// but the position is" would restore the window to the corner the tiler
    /// happened to put it in, on a machine where nothing tiles.
    pub fn remembered_pos(&self) -> Option<Pos2> {
        (!self.wm_managed).then_some(self.live_pos).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The circle really is fifths, all the way round and back to the start.
    /// A typo in that table would still look like a circle of fifths.
    #[test]
    fn the_circle_ascends_by_fifths_and_closes() {
        for i in 0..12 {
            assert_eq!(
                FIFTHS[(i + 1) % 12],
                (FIFTHS[i] + 7) % 12,
                "position {i} is not a fifth above its neighbour"
            );
        }
        let mut seen = [false; 12];
        for pc in FIFTHS {
            assert!(!seen[pc as usize], "pitch class {pc} appears twice");
            seen[pc as usize] = true;
        }
    }

    /// Adjacent keys share six of seven notes and opposite keys share two.
    /// This is the fact the diagram exists to show, so it is worth asserting
    /// rather than trusting the drawing to make it look true.
    #[test]
    fn neighbouring_keys_are_the_closest_ones() {
        for i in 0..12usize {
            let here = major_scale(FIFTHS[i]);
            let next = major_scale(FIFTHS[(i + 1) % 12]);
            let opposite = major_scale(FIFTHS[(i + 6) % 12]);
            assert_eq!(
                (here & next).count_ones(),
                6,
                "neighbours of {i} do not share six notes"
            );
            assert_eq!(
                (here & opposite).count_ones(),
                2,
                "opposites of {i} do not share two notes"
            );
        }
    }

    /// The relative minor sits on the same spoke because it has the same key
    /// signature. Same signature means, exactly, the same seven notes.
    #[test]
    fn the_relative_minor_shares_its_majors_notes() {
        for pc in 0..12u8 {
            // A natural minor scale is its relative major's, started later.
            let rel = (pc + 9) % 12;
            let minor = major_scale(pc); // same set, different tonic
            assert_eq!(
                major_scale((rel + 3) % 12),
                minor,
                "the relative minor of {pc} does not share its notes"
            );
        }
    }

    /// Signatures run 0..6 sharps clockwise and 6..1 flats coming back.
    #[test]
    fn key_signatures_count_out_and_back() {
        let got: Vec<(u8, u8)> = (0..12).map(signature).collect();
        assert_eq!(
            got,
            vec![
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (5, 0),
                (6, 0),
                (0, 5),
                (0, 4),
                (0, 3),
                (0, 2),
                (0, 1),
            ]
        );
    }

    /// Every upward triangle in the lattice is a major triad and every
    /// downward one is a minor triad, everywhere, for any origin. This is the
    /// Tonnetz's defining property; if it fails the diagram is a decoration.
    #[test]
    fn every_tonnetz_triangle_is_a_triad() {
        for origin in 0..12u8 {
            for u in -4..4 {
                for v in -4..4 {
                    let p = tonnetz_pc(u, v, origin);
                    let up = [(u, v), (u + 1, v), (u, v + 1)]
                        .iter()
                        .fold(0u16, |m, &(u, v)| m | 1 << tonnetz_pc(u, v, origin));
                    assert_eq!(
                        up,
                        major_triad(p),
                        "the up-triangle at ({u},{v}) origin {origin} is not major on {p}"
                    );

                    let dn = [(u + 1, v), (u, v + 1), (u + 1, v + 1)]
                        .iter()
                        .fold(0u16, |m, &(u, v)| m | 1 << tonnetz_pc(u, v, origin));
                    // The minor triad shares the up-triangle's third and fifth.
                    assert_eq!(
                        dn,
                        minor_triad((p + 4) % 12),
                        "the down-triangle at ({u},{v}) origin {origin} is not minor"
                    );
                }
            }
        }
    }

    /// Neighbouring triangles share an edge, which means the two chords share
    /// two notes. That is why the Tonnetz shows voice leading at a glance.
    #[test]
    fn adjacent_triads_share_two_notes() {
        for pc in 0..12u8 {
            assert_eq!(
                (major_triad(pc) & minor_triad((pc + 4) % 12)).count_ones(),
                2
            );
            assert_eq!(
                (major_triad(pc) & minor_triad((pc + 9) % 12)).count_ones(),
                2
            );
        }
    }

    /// I, IV and V between them use every note of the major scale and no
    /// other. It is why three chords are enough for so many songs, and it
    /// keeps the triangle honest about which key it is drawing.
    #[test]
    fn one_four_and_five_spell_the_whole_key() {
        for tonic in 0..12u8 {
            let three =
                major_triad(tonic) | major_triad((tonic + 5) % 12) | major_triad((tonic + 7) % 12);
            assert_eq!(
                three,
                major_scale(tonic),
                "I-IV-V in {tonic} is not the major scale"
            );
        }
    }

    #[test]
    fn labels_parse_to_a_root_and_a_quality() {
        let cases = [
            ("C", Some((0, false))),
            ("Cm", Some((0, true))),
            ("Cmaj7", Some((0, false))),
            ("F#m7", Some((6, true))),
            ("Bb", Some((10, false))),
            ("Bbm", Some((10, true))),
            ("Ebmaj9", Some((3, false))),
            ("G7", Some((7, false))),
            ("Adim", Some((9, false))),
            ("", None),
            ("?", None),
            ("Hm", None),
        ];
        for (label, want) in cases {
            assert_eq!(parse_label(label), want, "parsing {label:?}");
        }
    }

    /// **The whole interaction, walked through as somebody would.**
    ///
    /// Four number keys, and between them they choose what is showing AND
    /// arrange it — because a toggle that appends is a reorder. This is the
    /// sequence the owner described, asserted step by step, and it is the one
    /// test in this file that would catch "on" quietly meaning "back in its
    /// numbered slot", which would make the band's order permanent and the
    /// second press of a number do nothing.
    #[test]
    fn the_number_keys_both_choose_and_arrange_the_band() {
        // Everything, in the numbered order.
        let mut band = Views::all();
        assert_eq!(band.order(), View::ALL);

        // 1 takes the first element away and the other three close up.
        band = band.toggled(View::Circle);
        assert_eq!(band.order(), &[View::Tonnetz, View::Triangles, View::Staff]);

        // Empty it entirely. This is a real state: the band collapses and the
        // window gives its height back.
        for v in [View::Tonnetz, View::Triangles, View::Staff] {
            band = band.toggled(v);
        }
        assert!(!band.any(), "the band did not collapse");
        assert_eq!(band.count(), 0);

        // From empty, ANY number puts that one element in full view.
        band = band.toggled(View::Triangles);
        assert_eq!(band.order(), &[View::Triangles]);

        // And the next one goes to its RIGHT, whatever its number is. This is
        // the reordering: 3 then 2 puts the Tonnetz after the triangles, which
        // no arrangement of flags could have said.
        band = band.toggled(View::Tonnetz);
        assert_eq!(band.order(), &[View::Triangles, View::Tonnetz]);
        band = band.toggled(View::Staff);
        assert_eq!(band.order(), &[View::Triangles, View::Tonnetz, View::Staff]);

        // Pressing a number twice moves that element to the end, which is the
        // whole vocabulary: any arrangement is reachable from any other.
        band = band.toggled(View::Triangles).toggled(View::Triangles);
        assert_eq!(band.order(), &[View::Tonnetz, View::Staff, View::Triangles]);

        // The numbers are FIXED to elements, not to positions. If this ever
        // became "the nth showing panel", every rearrangement would rebind the
        // keys under the user's fingers.
        for (i, v) in View::ALL.iter().enumerate() {
            assert_eq!(View::from_number(i + 1), Some(*v));
            assert_eq!(v.number(), i + 1);
        }
        assert_eq!(View::from_number(0), None);
        assert_eq!(View::from_number(View::ALL.len() + 1), None);
    }

    /// The order survives the settings file, and a file from a build that knows
    /// a fifth element still opens here.
    #[test]
    fn the_band_order_round_trips_and_tolerates_what_it_does_not_know() {
        for band in every_selection() {
            assert_eq!(Views::from_keys(&band.keys()), band, "{}", band.keys());
        }
        // A name this build has never heard of is DROPPED, not refused: the
        // alternative is a file written by a later version resetting somebody's
        // whole arrangement because one word in it was unfamiliar.
        assert_eq!(
            Views::from_keys("staff+spectrogram+circle"),
            Views::of(vec![View::Staff, View::Circle])
        );
        assert_eq!(Views::from_keys(""), Views::default());
        // And a name repeated is not two panels.
        assert_eq!(
            Views::from_keys("circle+circle"),
            Views::of(vec![View::Circle])
        );
    }

    /// **The cells divide the whole band and keep the order they were given.**
    #[test]
    fn the_cells_follow_the_order_and_leave_no_gap() {
        let rect = Rect::from_min_size(Pos2::new(11.0, 7.0), Vec2::new(900.0, 300.0));
        for band in every_selection() {
            let cells = cells(rect, &band);
            assert_eq!(
                cells.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
                band.order(),
                "the cells came out in a different order from the band"
            );
            if band.any() {
                let total: f32 = cells.iter().map(|(_, r)| r.width()).sum();
                assert!((total - rect.width()).abs() < 0.01, "the band has a gap");
                assert!((cells[0].1.left() - rect.left()).abs() < 0.01);
                let last = cells.last().expect("a cell").1;
                assert!((last.right() - rect.right()).abs() < 0.01);
            }
        }
    }

    /// Panes are laid out in a fixed order, so turning one off never makes the
    /// other two swap places under the user's eyes.
    #[test]
    fn panes_keep_their_order_and_fill_the_band() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 300.0));
        let all = Views::all();
        let got = cells(rect, &all);
        assert_eq!(
            got.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
            View::ALL.to_vec()
        );
        let total: f32 = got.iter().map(|(_, r)| r.width()).sum();
        assert!((total - rect.width()).abs() < 0.5, "panes left a gap");

        // Dropping the middle one keeps the outer two in the same order.
        let two = Views::of(vec![View::Circle, View::Triangles]);
        assert_eq!(
            cells(rect, &two).iter().map(|(v, _)| *v).collect::<Vec<_>>(),
            vec![View::Circle, View::Triangles]
        );
        assert!(cells(rect, &Views::default()).is_empty());
    }

    /// The band takes no height when nothing is selected — the window must not
    /// grow by 300 points to show three empty panes.
    #[test]
    fn zz_tmp_octave_sign_bounds() {
        for set in [
            crate::staff::StaffSet::Single(crate::staff::Clef::Treble),
            crate::staff::StaffSet::Grand,
        ] {
            for note in [100u8, 113, 41, 29] {
                let mut s = crate::settings::Settings::first_launch();
                s.staff_set = set.key();
                let views = s.theory_views();
                let w = 1300.0_f32;
                let band =
                    Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, &views)));
                let cells = cells(band, &views);
                let Some((_, staff_cell)) = cells.iter().find(|(v, _)| *v == View::Staff)
                else {
                    continue;
                };
                let inner = staff_cell.shrink(8.0);
                let body = body_rect(inner);
                let h = (inner.height() * 0.075).clamp(9.0, 14.0);
                let lh = h * 0.82;
                let legend_centre_y = inner.min.y + h + lh * 0.5;
                let notes: std::collections::HashSet<u8> = [note].into_iter().collect();
                let ctx = egui::Context::default();
                crate::fonts::install(&ctx, crate::fonts::FontChoice::default(), None);
                let out = ctx.run(egui::RawInput::default(), |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::debug());
                    draw(
                        &painter,
                        band,
                        &views,
                        Input {
                            pcs: 1 << (note % 12),
                            bass: Some(note % 12),
                            root: None,
                            minor: false,
                        },
                        &notes,
                        None,
                        &s,
                    );
                });
                let mut worst_top = f32::INFINITY;
                let mut worst_bot = f32::NEG_INFINITY;
                let mut texts = Vec::new();
                for cs in &out.shapes {
                    if let egui::Shape::Text(t) = &cs.shape {
                        let r = t.visual_bounding_rect();
                        texts.push((t.galley.job.text.clone(), r));
                    }
                    let r = cs.shape.visual_bounding_rect();
                    if r.is_positive() && r.width() < 1e6 {
                        worst_top = worst_top.min(r.top());
                        worst_bot = worst_bot.max(r.bottom());
                    }
                }
                eprintln!(
                    "SET={:?} note={note} body={:?} legend_cy={:.2} inner_top={:.2} worst_top={:.2} worst_bot={:.2}",
                    set.key(), body, legend_centre_y, inner.min.y, worst_top, worst_bot
                );
                for (txt, r) in texts {
                    eprintln!("   text {txt:?} rect={r:?} above_body={:.2}", body.top() - r.top());
                }
            }
        }
    }

    #[test]
    fn an_empty_selection_takes_no_band() {
        assert_eq!(band_height(1300.0, &Views::default()), 0.0);
        assert!(
            band_height(1300.0, &Views::of(vec![View::Circle])) > 0.0
        );
    }

    /// The pane sizes the band actually produces: one, two or three diagrams
    /// across a window at every size the app offers, minus the title strip.
    fn real_panes() -> Vec<Rect> {
        let mut out = Vec::new();
        for pct in [50i64, 75, 100, 125, 150, 175, 200] {
            let w = (1300.0 * pct as f64 / 100.0).trunc() as f32;
            for n in 1..=View::ALL.len() {
                // **Actually n panes.** This loop used to build a one-element
                // band whatever `n` was, so every width in it was measured
                // three times and the multi-pane split — the one that can
                // starve a diagram — was never measured at all.
                let views = Views::of(View::ALL.into_iter().take(n).collect());
                let h = band_height(w, &views);
                let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
                for (_, cell) in cells(band, &views) {
                    // What draw() hands each diagram, and then what title()
                    // leaves below itself.
                    let inner = cell.shrink(8.0);
                    let th = (inner.height() * 0.09).clamp(9.0, 15.0);
                    out.push(Rect::from_min_max(
                        Pos2::new(inner.min.x, inner.min.y + th + 2.0),
                        inner.max,
                    ));
                }
            }
        }
        out
    }

    /// Not one Tonnetz node may cross its pane. The lattice is unbounded and
    /// the pane is not; the first render put a column through the neighbouring
    /// diagram and clipped an "Eb" down to "Kb", which reads as a note name
    /// and is not one.
    #[test]
    fn no_tonnetz_node_escapes_its_pane() {
        for pane in real_panes() {
            let Some(l) = Lattice::fit(pane) else {
                continue;
            };
            for v in 0..l.rows {
                for u in 0..l.cols {
                    if !l.shows(pane, u, v) {
                        continue;
                    }
                    let node = Rect::from_center_size(l.at(u, v), Vec2::splat(l.node_r * 2.0));
                    assert!(
                        pane.contains_rect(node),
                        "node ({u},{v}) at {node:?} escapes {pane:?}"
                    );
                }
            }
        }
    }

    /// ...and the fix for that must not have emptied the diagram. A guard that
    /// hides everything also satisfies "nothing escapes", so the useful half of
    /// the property is that a real pane still shows a lattice worth looking at.
    #[test]
    fn the_tonnetz_still_has_a_lattice_left() {
        for pane in real_panes() {
            let l = Lattice::fit(pane).expect("a real pane must fit a lattice");
            let shown = (0..l.rows)
                .flat_map(|v| (0..l.cols).map(move |u| (u, v)))
                .filter(|&(u, v)| l.shows(pane, u, v))
                .count();
            assert!(
                shown >= 20,
                "only {shown} nodes survive in {pane:?} ({}x{})",
                l.cols,
                l.rows
            );

            // EVERY row and EVERY column, whole. The lattice used to be sized
            // so its extent landed exactly on the pane edge, and a node whose
            // box touches the boundary fails `contains_rect` by a rounding
            // error — so the bottom row vanished and the diagram read as cut
            // off. It only happened below 100% window size, which is where
            // the arithmetic stopped landing on whole pixels, so a test that
            // checks one size proves nothing.
            for v in 0..l.rows {
                let in_row = (0..l.cols).filter(|&u| l.shows(pane, u, v)).count();
                assert!(
                    in_row >= 3,
                    "row {v} of {} has only {in_row} whole nodes in {pane:?} — \
                     the lattice is clipped",
                    l.rows
                );
            }
            // Every pitch class reachable, so the diagram can always light the
            // note you are playing.
            let seen: u16 = (0..l.rows)
                .flat_map(|v| (0..l.cols).map(move |u| (u, v)))
                .filter(|&(u, v)| l.shows(pane, u, v))
                .fold(0u16, |m, (u, v)| m | 1 << tonnetz_pc(u, v, 0));
            assert_eq!(seen, 0xFFF, "some pitch class has no node in {pane:?}");
        }
    }

    /// The harmonic-triangle figure, nodes included, fits the pane it is given.
    /// It did not: the tonic node was drawn over the pane's own title.
    #[test]
    fn the_hexagram_fits_under_its_own_title() {
        for pane in real_panes() {
            let r = hexagram_radius(pane);
            let extent = r * 1.30 * 2.0;
            assert!(
                extent <= pane.width().min(pane.height()) + 0.5,
                "the figure is {extent} across in a {:?} pane",
                pane.size()
            );
        }
    }

    /// Every character the band draws must reach the screen. The key
    /// signatures were written with U+266F and U+266D, which neither bundled
    /// face nor the fallback chain has, so they drew as tofu boxes — and a
    /// missing accidental turns "2 sharps" into "2 something".
    #[test]
    fn every_glyph_the_band_draws_exists() {
        // EVERY bundled face, not just the default: the typeface is a user
        // choice, so a glyph missing from one of them is a glyph missing for
        // whoever picked it.
        //
        // The two music accidentals are the exception, and deliberately: only
        // JetBrains Mono has them, and `fonts::install` puts JetBrains at the
        // bottom of BOTH families for exactly that reason. They are checked
        // separately below, against that one face.
        let bundled: &[(&str, &[u8])] = &[
            ("Courier Prime", fonts::COURIER_PRIME_REGULAR),
            ("Courier Prime Bold", fonts::COURIER_PRIME_BOLD),
            ("Terminess", fonts::TERMINESS_REGULAR),
            ("Terminess Bold", fonts::TERMINESS_BOLD),
            ("JetBrains Mono", fonts::JETBRAINS_REGULAR),
            ("JetBrains Mono Bold", fonts::JETBRAINS_BOLD),
        ];
        let mut drawn: Vec<char> = Vec::new();
        for pc in 0..12u8 {
            drawn.extend(pc_name(pc, true).chars());
            drawn.extend(pc_name(pc, false).chars());
            drawn.extend(pc_name(pc, true).to_lowercase().chars());
        }
        for i in 0..12usize {
            let (sharps, flats) = signature(i);
            drawn.extend(format!("{sharps}#{flats}b").chars());
        }
        drawn.extend("CIRCLE OF FIFTHS TONNETZ HARMONIC TRIANGLES".chars());
        drawn.extend("I IV V i iv v no key fits keys 0123456789m".chars());
        // The empty detached window's notice. It is the only thing on screen
        // when no diagram is selected, so a glyph missing from the chosen face
        // would leave the user with a blank window and no way to know why.
        drawn.extend(EMPTY_TITLE.chars());
        drawn.extend(EMPTY_HINT.chars());
        drawn.sort_unstable();
        drawn.dedup();

        for c in drawn {
            for (name, bytes) in bundled {
                assert!(
                    ttf_parser::Face::parse(bytes, 0)
                        .map(|f| f.glyph_index(c).is_some())
                        .unwrap_or(false),
                    "{name} has no glyph for {c:?} (U+{:04X})",
                    c as u32
                );
            }
        }

        // The accidentals, which reach the screen through the JetBrains
        // fallback rather than the chosen face. If this face ever loses them
        // the circle silently goes back to drawing tofu boxes, which is how
        // this started.
        for c in [FLAT, SHARP] {
            assert!(
                ttf_parser::Face::parse(fonts::JETBRAINS_REGULAR, 0)
                    .map(|f| f.glyph_index(c).is_some())
                    .unwrap_or(false),
                "JetBrains Mono has no {c:?} (U+{:04X}), so nothing does",
                c as u32
            );
        }
    }

    /// Every Tonnetz cell the band can actually produce.
    fn real_tonnetz_cells() -> Vec<(Rect, Rect)> {
        let mut out = Vec::new();
        for pct in [50i64, 75, 100, 125, 150, 175, 200] {
            let w = (1300.0 * pct as f64 / 100.0).trunc() as f32;
            for n in 1..=View::ALL.len() {
                // The Tonnetz in a band of `n` panes, not in a band of one
                // pane `n` times — same bug as `real_panes` had.
                let mut order = vec![View::Tonnetz];
                order.extend(View::ALL.into_iter().filter(|v| *v != View::Tonnetz).take(n - 1));
                let views = Views::of(order);
                let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, &views)));
                for (v, cell) in cells(band, &views) {
                    if v == View::Tonnetz {
                        out.push((band, cell));
                    }
                }
            }
        }
        out
    }

    /// No lit triangle may have a corner on a node that is not drawn.
    ///
    /// The triangle loop walks `-1..cols` so it can reach the cells along the
    /// left edge, which means it can also name corners one column past the
    /// right edge. Those produced a shape hanging off the grid with a vertex
    /// on nothing — it reads as a rendering fault, not as a chord.
    #[test]
    fn no_lit_triangle_hangs_off_the_grid() {
        // Cm7, which is what showed the bug, plus a spread of others.
        for pcs in [
            0b0000_1000_1001_u16 | (1 << 3) | (1 << 10), // C Eb G Bb
            0b0000_1001_0001,                            // C E G
            0xFFF,
            0b1000_0100_0001,
        ] {
            let input = Input {
                pcs,
                bass: Some(0),
                root: Some(0),
                minor: false,
            };
            // THE REAL PANES, all three combinations, because a cell in the
            // middle of three is not the same rectangle as a band of one and
            // this bug only showed in the three-pane case.
            for (band, cell) in real_tonnetz_cells() {
                let w = band.width();
                let body = body_rect(cell.shrink(8.0));
                let Some(l) = Lattice::fit(body) else {
                    continue;
                };
                for v in 0..l.rows {
                    for u in -1..l.cols {
                        for corners in [
                            [(u, v), (u + 1, v), (u, v + 1)],
                            [(u + 1, v), (u, v + 1), (u + 1, v + 1)],
                        ] {
                            let mask = corners
                                .iter()
                                .fold(0u16, |m, &(u, v)| m | 1 << tonnetz_pc(u, v, 0));
                            if !input.contains(mask) {
                                continue;
                            }
                            let drawn = corners.iter().all(|&(u, v)| l.shows(body, u, v));
                            if !drawn {
                                // This is the case the renderer must skip. If
                                // it did not, the assertion below would be the
                                // one to change — it is asserting the SHAPE of
                                // the guard, so it has to name it.
                                continue;
                            }
                            for &(cu, cv) in &corners {
                                assert!(
                                    l.shows(body, cu, cv),
                                    "a lit triangle at ({u},{v}) in a {w}pt band \
                                     has a corner at ({cu},{cv}) that is never drawn"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A click on a Tonnetz node must return the pitch class written on it.
    ///
    /// Hit tests drift away from the pictures they belong to, and the drift is
    /// invisible: the diagram still looks right and the clicks land on the
    /// wrong thing. So this walks every node the lattice actually shows and
    /// asks the hit test what is at its exact centre.
    #[test]
    fn a_click_on_a_tonnetz_node_finds_that_note() {
        let views = Views::of(vec![View::Tonnetz]);
        for pane in real_panes() {
            let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(pane.width() + 16.0, 300.0));
            let cell = cells(band, &views)[0].1;
            let body = body_rect(cell.shrink(8.0));
            let Some(l) = Lattice::fit(body) else {
                continue;
            };
            let mut checked = 0;
            for v in 0..l.rows {
                for u in 0..l.cols {
                    if !l.shows(body, u, v) {
                        continue;
                    }
                    let c = l.at(u, v);
                    assert_eq!(
                        hit_test(band, &views, Input::default(), c),
                        Some(Hit::Pc(tonnetz_pc(u, v, 0))),
                        "clicking the centre of node ({u},{v}) missed it"
                    );
                    checked += 1;
                }
            }
            assert!(checked >= 20, "only {checked} nodes were reachable at all");
        }
    }

    /// The same for the circle: the name you click is the note you get, on
    /// both rings, all the way round.
    #[test]
    fn a_click_on_a_circle_name_finds_that_note() {
        let views = Views::of(vec![View::Circle]);
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(440.0, 300.0));
        let body = body_rect(cells(band, &views)[0].1.shrink(8.0));
        let c = body.center();
        let r = body.width().min(body.height()) * 0.5 - 2.0;
        for (i, &pc) in FIFTHS.iter().enumerate() {
            let a = circle_angle(i);
            let dir = Vec2::new(a.cos(), a.sin());
            // Where `draw` puts the major name, and the relative minor.
            let major_at = c + dir * ((r * 0.80 + r * 0.56) * 0.5);
            let minor_at = c + dir * ((r * 0.56 + r * 0.34) * 0.5);
            assert_eq!(
                hit_test(band, &views, Input::default(), major_at),
                Some(Hit::Pc(pc)),
                "the major name at position {i} is not clickable"
            );
            // The relative-minor ring is a label. Every pitch class is
            // already reachable on the major ring, so nothing is lost.
            assert_eq!(
                hit_test(band, &views, Input::default(), minor_at),
                None,
                "the relative-minor label at position {i} acts as a control"
            );
        }
        // The hub and the signature ring are labels, not controls.
        assert_eq!(hit_test(band, &views, Input::default(), c), None);
    }

    /// Each vertex of the hexagram gives back the chord printed on it, in
    /// every key — and the two triangles must not answer for each other.
    #[test]
    fn a_click_on_a_chord_vertex_gives_that_chord() {
        let views = Views::of(vec![View::Triangles]);
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(440.0, 300.0));
        let body = body_rect(cells(band, &views)[0].1.shrink(8.0));
        let c = body.center();
        let r = hexagram_radius(body);
        for tonic in 0..12u8 {
            let input = Input {
                pcs: 1 << tonic,
                bass: Some(tonic),
                root: Some(tonic),
                minor: false,
            };
            let roots = [tonic, (tonic + 5) % 12, (tonic + 7) % 12];
            for (k, &oi) in HEX_UP_ORDER.iter().enumerate() {
                assert_eq!(
                    hit_test(band, &views, input, hex_vertex(c, r, k, false)),
                    Some(Hit::Triad {
                        root: roots[oi],
                        minor: false
                    }),
                    "upward vertex {k} in key {tonic}"
                );
            }
            for (k, &oi) in HEX_DOWN_ORDER.iter().enumerate() {
                assert_eq!(
                    hit_test(band, &views, input, hex_vertex(c, r, k, true)),
                    Some(Hit::Triad {
                        root: roots[oi],
                        minor: true
                    }),
                    "downward vertex {k} in key {tonic}"
                );
            }
        }
    }

    /// A click belongs to the pane it landed in, and to no other.
    ///
    /// Three diagrams side by side share one rectangle, and each one's
    /// geometry is happy to answer for a point anywhere on the plane — the
    /// circle's rings and the lattice both extend past their cell by
    /// arithmetic if not by drawing. `hit_test` picks the cell FIRST for
    /// exactly that reason.
    #[test]
    fn a_click_belongs_to_the_pane_it_landed_in() {
        let all = Views::of(vec![View::Circle, View::Tonnetz, View::Triangles]);
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 300.0));
        let panes = cells(band, &all);
        let triangles_cell = panes
            .iter()
            .find(|(v, _)| *v == View::Triangles)
            .map(|(_, c)| *c)
            .expect("the triangles pane");

        // Sweep the whole band. Only the triangles produce chords, and only
        // from inside their own cell.
        let mut triads = 0;
        let mut hits = 0;
        for x in (0..1300).step_by(7) {
            for y in (0..300).step_by(7) {
                let p = Pos2::new(x as f32, y as f32);
                match hit_test(band, &all, Input::default(), p) {
                    Some(Hit::Triad { .. }) => {
                        assert!(
                            triangles_cell.contains(p),
                            "a chord came back from {p:?}, outside the triangles pane"
                        );
                        triads += 1;
                        hits += 1;
                    }
                    Some(Hit::Pc(_)) => {
                        assert!(
                            !triangles_cell.contains(p),
                            "a bare note came back from inside the triangles pane at {p:?}"
                        );
                        hits += 1;
                    }
                    None => {}
                }
            }
        }
        assert!(triads > 0, "no chord vertex was reachable anywhere");
        assert!(
            hits > 100,
            "only {hits} points in the whole band are clickable"
        );

        // Nothing selected, nothing to click.
        for x in (0..1300).step_by(53) {
            for y in (0..300).step_by(29) {
                assert_eq!(
                    hit_test(
                        band,
                        &Views::default(),
                        Input::default(),
                        Pos2::new(x as f32, y as f32)
                    ),
                    None
                );
            }
        }
    }

    /// Every pane must draw without panicking at any size the layout can hand
    /// it, holding anything from nothing to all twelve notes. These are
    /// painters full of trigonometry and integer row counts; a zero-width band
    /// or a degenerate lattice is exactly where that breaks.
    #[test]
    fn every_pane_draws_at_every_size() {
        let ctx = egui::Context::default();
        // Every label here is a real glyph, and a bare Context has no font
        // families bound, so painting one panics rather than failing.
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        let s = Settings::default();
        for w in [40.0_f32, 120.0, 400.0, 1300.0, 2600.0] {
            for views in [
                Views::of(vec![View::Circle]),
                Views::of(vec![View::Tonnetz]),
                Views::of(vec![View::Triangles]),
                Views::of(vec![View::Circle, View::Tonnetz, View::Triangles]),
            ] {
                let h = band_height(w, &views);
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
                for pcs in [0u16, 0b0000_1001_0001, 0xFFF, 0b1000_0000_0001] {
                    let input = Input {
                        pcs,
                        bass: (pcs != 0).then_some(0),
                        root: (pcs != 0).then_some(0),
                        minor: false,
                    };
                    let _ = ctx.run(Default::default(), |ctx| {
                        let painter = ctx.layer_painter(egui::LayerId::background());
                        draw(&painter, rect, &views, input, &HashSet::new(), None, &s);
                    });
                }
            }
        }
    }

    // ── the popped-out window ──────────────────────────────────────────────

    /// All eight selections, the empty one included. Every test below runs the
    /// whole set: the window has to survive combinations the band never shows,
    /// because the band with nothing selected is zero points tall and simply
    /// is not there, while the window is still on the user's screen.
    /// **Every ordered subset of the band**, which is what the band can now
    /// be: sixty-five of them, from empty to all four in each of their
    /// twenty-four arrangements. It used to be the eight combinations three
    /// flags could express, and order was not one of the things it could
    /// express — so nothing here could have caught a hit test that was right
    /// about WHICH panels and wrong about where they were.
    fn every_selection() -> Vec<Views> {
        fn grow(prefix: Vec<View>, out: &mut Vec<Views>) {
            out.push(Views::of(prefix.clone()));
            for v in View::ALL {
                if !prefix.contains(&v) {
                    let mut next = prefix.clone();
                    next.push(v);
                    grow(next, out);
                }
            }
        }
        let mut out = Vec::new();
        grow(Vec::new(), &mut out);
        out
    }

    /// Where the drawing puts every control inside `rect`, and what clicking it
    /// must give back.
    ///
    /// Re-derived from the drawing code rather than read back out of
    /// `hit_test`, exactly like the band's own click tests: a hit test compared
    /// against itself proves nothing. If `draw_circle` moves the name ring or
    /// `draw_triangles` moves a vertex, this goes stale and the assertions
    /// below fail, which is the point.
    fn controls(rect: Rect, views: &Views, input: Input) -> Vec<(View, Pos2, Hit)> {
        let mut out = Vec::new();
        for (view, cell) in cells(rect, views) {
            let body = body_rect(cell.shrink(8.0));
            match view {
                View::Circle => {
                    let c = body.center();
                    let r = body.width().min(body.height()) * 0.5 - 2.0;
                    if r < 20.0 {
                        continue;
                    }
                    for (i, &pc) in FIFTHS.iter().enumerate() {
                        let a = circle_angle(i);
                        let dir = Vec2::new(a.cos(), a.sin());
                        // Where `draw_circle` centres the major name.
                        out.push((view, c + dir * ((r * 0.80 + r * 0.56) * 0.5), Hit::Pc(pc)));
                    }
                }
                // The sheet music has nothing to click on: it is a readout.
                View::Staff => {}
                View::Tonnetz => {
                    let Some(l) = Lattice::fit(body) else {
                        continue;
                    };
                    for v in 0..l.rows {
                        for u in 0..l.cols {
                            if l.shows(body, u, v) {
                                out.push((view, l.at(u, v), Hit::Pc(tonnetz_pc(u, v, 0))));
                            }
                        }
                    }
                }
                View::Triangles => {
                    let c = body.center();
                    let r = hexagram_radius(body);
                    if r < 20.0 {
                        continue;
                    }
                    let tonic = input.tonic();
                    let roots = [tonic, (tonic + 5) % 12, (tonic + 7) % 12];
                    for (k, &oi) in HEX_UP_ORDER.iter().enumerate() {
                        let hit = Hit::Triad {
                            root: roots[oi],
                            minor: false,
                        };
                        out.push((view, hex_vertex(c, r, k, false), hit));
                    }
                    for (k, &oi) in HEX_DOWN_ORDER.iter().enumerate() {
                        let hit = Hit::Triad {
                            root: roots[oi],
                            minor: true,
                        };
                        out.push((view, hex_vertex(c, r, k, true), hit));
                    }
                }
            }
        }
        out
    }

    /// C major sounding, so the diagrams have something to light and the
    /// triangles have a tonic to orient themselves around.
    fn c_major() -> Input {
        Input {
            pcs: major_triad(0),
            bass: Some(0),
            root: Some(0),
            minor: false,
        }
    }

    /// Sizes the window can be that the BAND can never be.
    ///
    /// `band_height` is `trunc(300 * w / 1300)`, so a band 900 points wide is
    /// 207 tall and nothing else. Every size here is deliberately off that
    /// curve, which is what makes the test below able to tell "hit-tested
    /// against the window" apart from "hit-tested against the band".
    fn detached_sizes() -> Vec<Vec2> {
        vec![
            DETACHED_DEFAULT,
            DETACHED_MIN,
            Vec2::new(640.0, 300.0),
            Vec2::new(1180.0, 520.0),
            Vec2::new(520.0, 700.0), // taller than wide: a window can be
        ]
    }

    /// The hit test must invert the drawing at the WINDOW's size, which is a
    /// different rectangle from the band's in every dimension.
    ///
    /// This is the failure the detached window is most prone to and the least
    /// visible when it happens: the picture still looks right, and the clicks
    /// land on the note that would have been under that point in the main
    /// window — a note that is not on screen. So the test walks every control
    /// the drawing produces, at five window sizes no band ever has, and asks
    /// the hit test what is there.
    #[test]
    fn the_detached_hit_test_inverts_the_detached_drawing_at_the_windows_own_size() {
        let input = c_major();
        let mut checked = 0;
        for size in detached_sizes() {
            // Proof that these really are shapes the band cannot take, so a
            // hit test that quietly used band geometry could not pass.
            for views in every_selection() {
                assert_ne!(
                    band_height(size.x, &views),
                    size.y,
                    "{size:?} is a size the band can have, so this test cannot \
                     tell the window's geometry from the band's"
                );
            }
            // Off the origin as well: a viewport's content rect starts at zero,
            // but nothing in the geometry may ASSUME that.
            for origin in [Pos2::ZERO, Pos2::new(37.0, 11.0)] {
                let rect = Rect::from_min_size(origin, size);
                for views in every_selection() {
                    let controls = controls(rect, &views, input);
                    for (view, at, want) in &controls {
                        assert_eq!(
                            hit_test(rect, &views, input, *at),
                            Some(*want),
                            "{view:?} in a {size:?} window at {origin:?}: the \
                             control drawn at {at:?} is not clickable"
                        );
                        checked += 1;
                    }
                    // A band with something CLICKABLE in it must produce
                    // controls. The sheet music is a readout and has none, so a
                    // band of nothing but sheet music legitimately produces
                    // none — which is a different thing from a diagram that
                    // silently stopped drawing.
                    let clickable = views.order().iter().any(|v| *v != View::Staff);
                    if clickable && size.x >= DETACHED_MIN.x {
                        assert!(
                            !controls.is_empty(),
                            "{views:?} produced no controls at all in a \
                             {size:?} window"
                        );
                    }
                }
            }
        }
        assert!(checked > 300, "only {checked} controls were checked");
    }

    /// ...and the band would have answered differently, which is what makes the
    /// test above worth having.
    ///
    /// A hit test wired to the band's rect instead of the window's is not a
    /// crash and not a blank pane: it is a click that places the wrong note.
    /// This pins the two geometries apart, so the mistake cannot be invisible.
    #[test]
    fn the_band_and_the_window_do_not_agree_about_what_is_under_a_point() {
        let input = c_major();
        let views = Views::of(vec![View::Circle, View::Tonnetz, View::Triangles]);
        let window = Rect::from_min_size(Pos2::ZERO, DETACHED_DEFAULT);
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, band_height(1300.0, &views)));
        let controls = controls(window, &views, input);
        let wrong = controls
            .iter()
            .filter(|(_, at, want)| hit_test(band, &views, input, *at) != Some(*want))
            .count();
        assert!(
            wrong * 2 > controls.len(),
            "only {wrong} of {} window controls would answer differently \
             against the band, so a hit test run against the wrong rect would \
             mostly still look correct",
            controls.len()
        );
    }

    /// The smallest window we let the user make must still hold three diagrams.
    ///
    /// `Lattice::fit` returns None below `a = 16`, and `draw_circle` and
    /// `draw_triangles` both return early below `r = 20`. All three failures
    /// look identical from outside: a pane title over an empty rectangle, which
    /// reads as a rendering fault rather than as "the window is too small".
    /// `DETACHED_MIN` is where that stops happening, so it is asserted rather
    /// than remembered.
    #[test]
    fn the_smallest_window_still_holds_three_diagrams() {
        // **Every element the app ships with**, not the three it used to have.
        // `DETACHED_MIN` was measured for three panes; a fourth arrived and the
        // constant did not move, so the shipped default selection at the
        // minimum size drew the Tonnetz as a title over an empty rectangle —
        // exactly the failure this constant exists to prevent.
        let views = Views::all();
        let rect = Rect::from_min_size(Pos2::ZERO, DETACHED_MIN);
        let at_min = controls(rect, &views, c_major());
        // The sheet music is not in this list and cannot be: it has no
        // controls to count. It is a readout, and `controls` only knows about
        // things you can click.
        for view in View::ALL.into_iter().filter(|v| *v != View::Staff) {
            let n = at_min.iter().filter(|(v, _, _)| *v == view).count();
            assert!(
                n > 0,
                "{view:?} draws nothing at the minimum window size {:?}",
                DETACHED_MIN
            );
        }
        // And the floor is not wastefully high: a little under it, something
        // does give up. Otherwise the minimum is just a number nobody checked.
        let tighter = Rect::from_min_size(Pos2::ZERO, DETACHED_MIN - Vec2::new(60.0, 0.0));
        let shrunk = controls(tighter, &views, c_major());
        assert!(
            shrunk.len() < at_min.len(),
            "nothing was lost 60pt below the minimum, so the minimum is set \
             higher than it needs to be"
        );
    }

    /// Where the test window sits on its monitor. Non-zero and not round, so a
    /// context-menu position that forgot to add the window origin is obvious.
    const WINDOW_ORIGIN: Pos2 = Pos2::new(120.0, 64.0);

    /// One frame of the real detached window, headless.
    ///
    /// A bare `egui::Context` has `embed_viewports` set, so
    /// `show_viewport_immediate` runs its closure inline against this same
    /// context — which is the trap `host.rs` warns about in a plugin, and here
    /// it is exactly what makes the window testable without a window manager.
    fn run_detached(
        size: Vec2,
        views: &Views,
        input: Input,
        s: &Settings,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
        closing: bool,
    ) -> (DetachedOutcome, egui::FullOutput) {
        let raw = |events: Vec<egui::Event>, modifiers, closing| {
            let mut info = egui::ViewportInfo {
                inner_rect: Some(Rect::from_min_size(WINDOW_ORIGIN, size)),
                outer_rect: Some(Rect::from_min_size(
                    WINDOW_ORIGIN - Vec2::new(0.0, 28.0),
                    size + Vec2::new(0.0, 28.0),
                )),
                ..Default::default()
            };
            if closing {
                info.events.push(egui::ViewportEvent::Close);
            }
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                viewports: std::iter::once((egui::ViewportId::ROOT, info)).collect(),
                events,
                modifiers,
                ..Default::default()
            }
        };

        let ctx = egui::Context::default();
        // The window draws real glyphs, and a bare Context has no font families
        // bound: painting one panics rather than failing an assertion.
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        // A warm-up pass, so the atlas exists before any text is laid out.
        let _ = ctx.run(raw(Vec::new(), Default::default(), false), |_| {});

        let mut outcome = DetachedOutcome::default();
        let out = ctx.run(raw(events, modifiers, closing), |ctx| {
            outcome =
                show_detached_window(
                    ctx,
                    size,
                    Some(WINDOW_ORIGIN),
                    false,
                    true,
                    &views,
                    input,
                    &HashSet::new(),
                    None,
                    s,
                );
        });
        (outcome, out)
    }

    fn click_at(
        pos: Pos2,
        button: egui::PointerButton,
        modifiers: egui::Modifiers,
    ) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers,
            },
            egui::Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers,
            },
        ]
    }

    fn keytoggle_on() -> Settings {
        Settings {
            keytoggle_enabled: true,
            ..Default::default()
        }
    }

    /// A real click in the real window reports the chord printed on the vertex
    /// it landed on — and reports the WINDOW's answer, not the band's.
    ///
    /// The end-to-end version of the geometry tests above: the position goes in
    /// as a pointer event and comes back as a `Hit` through the outcome, so a
    /// wiring mistake between the two (the wrong rect, the wrong pointer
    /// position, a hit read on release instead of press) fails here.
    #[test]
    fn a_click_on_a_chord_vertex_in_the_window_places_that_triad() {
        let s = keytoggle_on();
        let input = c_major();
        let views = Views::of(vec![View::Circle, View::Tonnetz, View::Triangles]);
        let size = DETACHED_DEFAULT;
        let rect = Rect::from_min_size(Pos2::ZERO, size);
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, band_height(1300.0, &views)));

        let mut checked = 0;
        for (view, at, want) in controls(rect, &views, input) {
            if view != View::Triangles {
                continue;
            }
            let (outcome, _) = run_detached(
                size,
                &views,
                input,
                &s,
                click_at(at, egui::PointerButton::Primary, Default::default()),
                Default::default(),
                false,
            );
            assert_eq!(
                outcome.hit,
                Some(want),
                "a click at {at:?} in a {size:?} window did not place the \
                 chord drawn there"
            );
            // The same point in the band is a different pane entirely — the
            // triangles sit at x 866..1300 there and 600..900 here — so this
            // is the assertion that would fail if the window ever hit-tested
            // against band geometry.
            assert_ne!(
                hit_test(band, &views, input, at),
                Some(want),
                "the band happens to answer the same at {at:?}, so this case \
                 cannot tell the two rects apart"
            );
            checked += 1;
        }
        assert_eq!(checked, 6, "the hexagram has six vertices");
    }

    /// Keytoggle off is keytoggle off, in the window as in the band.
    ///
    /// It also decides whether a borderless press becomes a window drag: the
    /// drag stands aside only for a press that hit something, so a hit reported
    /// here and then discarded by the app would leave a patch of window that
    /// neither places a note nor drags.
    #[test]
    fn a_click_is_not_a_note_when_keytoggle_is_off() {
        let input = c_major();
        let views = Views::of(vec![View::Triangles]);
        let size = DETACHED_DEFAULT;
        let rect = Rect::from_min_size(Pos2::ZERO, size);
        let at = controls(rect, &views, input)[0].1;
        let (outcome, _) = run_detached(
            size,
            &views,
            input,
            &Settings::default(),
            click_at(at, egui::PointerButton::Primary, Default::default()),
            Default::default(),
            false,
        );
        assert!(
            !Settings::default().keytoggle_enabled,
            "this test is about the shipped default"
        );
        assert_eq!(outcome.hit, None, "a chord was placed with keytoggle off");
    }

    /// Right-click anywhere opens the app menu, at a position given in monitor
    /// coordinates — the window's own origin plus where the pointer was.
    ///
    /// And on macOS, so does ctrl-click, WITHOUT also placing a note. The main
    /// window guards that gesture for exactly this reason; this window is the
    /// only popout with click targets for it to damage.
    #[test]
    fn a_right_click_asks_for_the_menu_in_monitor_coordinates_and_places_nothing() {
        let s = keytoggle_on();
        let input = c_major();
        let views = Views::of(vec![View::Triangles]);
        let size = DETACHED_DEFAULT;
        let rect = Rect::from_min_size(Pos2::ZERO, size);
        let at = controls(rect, &views, input)[0].1;

        let (outcome, _) = run_detached(
            size,
            &views,
            input,
            &s,
            click_at(at, egui::PointerButton::Secondary, Default::default()),
            Default::default(),
            false,
        );
        assert_eq!(
            outcome.context_menu_at,
            Some(WINDOW_ORIGIN + at.to_vec2()),
            "the menu position must be in monitor coordinates"
        );
        assert_eq!(
            outcome.hit, None,
            "a right-click on a chord vertex also placed the chord"
        );

        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let (outcome, _) = run_detached(
            size,
            &views,
            input,
            &s,
            click_at(at, egui::PointerButton::Primary, ctrl),
            ctrl,
            false,
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                outcome.context_menu_at,
                Some(WINDOW_ORIGIN + at.to_vec2()),
                "ctrl-click is the right-click on macOS"
            );
            assert_eq!(
                outcome.hit, None,
                "ctrl-click placed a chord instead of opening the menu, which \
                 is the bug app.rs guards with `!ctrl_as_context`"
            );
        } else {
            assert_eq!(
                outcome.hit,
                Some(controls(rect, &views, input)[0].2),
                "off macOS, ctrl is not a context-menu gesture and the click \
                 is an ordinary one"
            );
        }
    }

    /// Closing the window is the reattach signal, exactly as for the other two
    /// popouts. Nothing else in the outcome may be disturbed by it.
    #[test]
    fn closing_the_window_is_how_the_band_comes_back() {
        let (outcome, _) = run_detached(
            DETACHED_DEFAULT,
            &Views::of(vec![View::Circle]),
            c_major(),
            &Settings::default(),
            Vec::new(),
            Default::default(),
            true,
        );
        assert!(outcome.close_requested, "the close was not reported");
        assert_eq!(outcome.hit, None);
        assert_eq!(outcome.context_menu_at, None);
    }

    /// Every selection makes a window with something in it — including the
    /// empty selection, which is the one the band cannot show at all.
    ///
    /// The band's answer to "nothing selected" is to be zero points tall and
    /// disappear. A window cannot do that: it is already open, and a blank one
    /// reads as the app having crashed in that corner of the screen. So the
    /// empty window says what happened and how to undo it, and this counts the
    /// text shapes to prove it rather than trusting the code to have drawn any.
    #[test]
    fn every_selection_including_none_makes_a_window_worth_looking_at() {
        let s = keytoggle_on();
        for views in every_selection() {
            for size in detached_sizes() {
                for input in [Input::default(), c_major()] {
                    let (outcome, out) = run_detached(
                        size,
                        &views,
                        input,
                        &s,
                        Vec::new(),
                        Default::default(),
                        false,
                    );
                    let texts = out
                        .shapes
                        .iter()
                        .filter(|s| matches!(s.shape, egui::Shape::Text(_)))
                        .count();
                    assert!(
                        texts > 0,
                        "{views:?} at {size:?} painted no text at all — a \
                         blank window reads as a crash, not as a choice"
                    );
                    if !views.any() {
                        // The notice is two lines, and both must survive: the
                        // hint is the only thing that tells the user the
                        // window is not broken but empty.
                        assert!(
                            texts >= 2,
                            "the empty window drew {texts} lines, not the \
                             title and the hint"
                        );
                    }
                    assert_eq!(
                        outcome.inner_size,
                        Some(size),
                        "the window reported a size that is not its own"
                    );
                    assert!(!outcome.close_requested);
                }
            }
        }
    }

    /// Nothing in an empty window is clickable, at any size. The notice is a
    /// notice, not a control, and a stray hit there would place a note the user
    /// cannot see the name of.
    #[test]
    fn an_empty_window_has_nothing_to_click() {
        for size in detached_sizes() {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            for x in 0..24 {
                for y in 0..24 {
                    let p = Pos2::new(
                        rect.min.x + size.x * x as f32 / 23.0,
                        rect.min.y + size.y * y as f32 / 23.0,
                    );
                    assert_eq!(
                        hit_test(rect, &Views::default(), c_major(), p),
                        None,
                        "something in the empty window at {p:?} is clickable"
                    );
                }
            }
        }
    }

    /// A tiling window manager must not be able to write its own idea of the
    /// window's size into settings.json.
    ///
    /// The owner runs AeroSpace, which overrules the size we ask for the moment
    /// the window is mapped. Recording that as a user preference is how a
    /// `detached_chord_height` of 1377 got into a settings file and then
    /// followed the user onto machines with nothing tiling. The guard is a
    /// timing test, because size alone cannot tell a tiler from a person: a
    /// mismatch inside the grace period is the WM's, after it is the user's.
    #[test]
    fn the_window_manager_cannot_write_its_own_size_into_settings() {
        let t0 = Instant::now();
        let asked = DETACHED_DEFAULT;
        let tiled = |size: Vec2| DetachedOutcome {
            inner_size: Some(size),
            outer_pos: Some(Pos2::new(0.0, 0.0)),
            ..Default::default()
        };

        // The tiler: the very first frame comes back the height of a monitor.
        let mut g = GeometryGuard::opened(asked, t0);
        let armed = g.observe(&tiled(Vec2::new(asked.x, 1377.0)), t0);
        assert!(!armed, "a tiled window armed the geometry save");
        assert!(g.wm_managed());
        assert_eq!(g.remembered_size(), None, "1377 reached settings.json");
        assert_eq!(g.remembered_pos(), None, "the tiled position came with it");
        // ...and it stays true for the session, even once the tiler hands back
        // the size we asked for. A guard that could be cleared by one agreeing
        // frame would let the next frame's tiled size through.
        let _ = g.observe(&tiled(asked), t0 + Duration::from_millis(200));
        assert!(g.wm_managed(), "one agreeing frame cleared the guard");
        assert_eq!(g.remembered_size(), None);

        // A window nobody interfered with: its geometry is the user's.
        let mut g = GeometryGuard::opened(asked, t0);
        assert!(g.observe(&tiled(asked), t0), "the first frame is a change");
        assert!(!g.wm_managed());
        assert_eq!(g.remembered_size(), Some(asked));
        assert_eq!(g.remembered_pos(), Some(Pos2::ZERO));
        // Rounding and DPI scaling move things by a point or two, and that is
        // not a window manager.
        let nudged = asked + Vec2::new(WM_SIZE_TOLERANCE - 1.0, 0.0);
        let _ = g.observe(&tiled(nudged), t0 + Duration::from_millis(100));
        assert!(!g.wm_managed(), "a {WM_SIZE_TOLERANCE}pt slack was refused");

        // The user, dragging the window edge long after it appeared. Same
        // numbers as the tiler, opposite meaning, and only the clock says so.
        let mut g = GeometryGuard::opened(asked, t0);
        let _ = g.observe(&tiled(asked), t0);
        let later = t0 + WM_GRACE + Duration::from_millis(1);
        let dragged = Vec2::new(asked.x, 1377.0);
        assert!(
            g.observe(&tiled(dragged), later),
            "a real resize did not arm the save"
        );
        assert!(
            !g.wm_managed(),
            "a resize after the grace period was blamed on the WM"
        );
        assert_eq!(g.remembered_size(), Some(dragged));

        // A frame in which nothing moved must not arm the save; otherwise the
        // debounced write never settles and settings.json is rewritten forever.
        assert!(
            !g.observe(&tiled(dragged), later + Duration::from_millis(16)),
            "an unchanged frame armed the geometry save"
        );
    }
}
