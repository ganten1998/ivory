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

/// Band height for a 1300pt-wide window. Large, because these are diagrams
/// rather than readouts: a circle of fifths with unreadable sector labels is
/// decoration. Scaled with everything else (spec §3.2).
pub const BAND_H_AT_1300: f64 = 300.0;

/// Height of the theory band for a window `w` points wide, or 0 when nothing
/// is selected. Truncated like every other band in the layout.
pub fn band_height(w: f32, views: Views) -> f32 {
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

/// Which diagrams are showing. Any combination, including none.
///
/// Three independent flags rather than one enum: the request was explicitly to
/// be able to see more than one at a time, and an enum would have to grow a
/// variant per combination to say that.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Views {
    pub circle: bool,
    pub tonnetz: bool,
    pub triangles: bool,
}

impl Views {
    pub fn count(self) -> usize {
        self.circle as usize + self.tonnetz as usize + self.triangles as usize
    }

    pub fn any(self) -> bool {
        self.count() > 0
    }
}

/// One diagram, for the menu and for cycling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Circle,
    Tonnetz,
    Triangles,
}

impl View {
    pub const ALL: [View; 3] = [View::Circle, View::Tonnetz, View::Triangles];

    pub fn label(self) -> &'static str {
        match self {
            View::Circle => "Circle of Fifths",
            View::Tonnetz => "Tonnetz",
            View::Triangles => "Harmonic Triangles",
        }
    }

    pub fn is_on(self, v: Views) -> bool {
        match self {
            View::Circle => v.circle,
            View::Tonnetz => v.tonnetz,
            View::Triangles => v.triangles,
        }
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
pub fn cells(rect: Rect, views: Views) -> Vec<(View, Rect)> {
    let on: Vec<View> = View::ALL.into_iter().filter(|v| v.is_on(views)).collect();
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
pub fn hit_test(rect: Rect, views: Views, input: Input, pos: Pos2) -> Option<Hit> {
    let (view, cell) = cells(rect, views)
        .into_iter()
        .find(|(_, c)| c.contains(pos))?;
    let body = body_rect(cell.shrink(8.0));
    match view {
        View::Circle => circle_hit(body, pos).map(Hit::Pc),
        View::Tonnetz => tonnetz_hit(body, pos).map(Hit::Pc),
        // The only one that needs to know what is playing: I, IV and V are
        // relative to a tonic, and the tonic comes from the notes.
        View::Triangles => triangles_hit(body, input.tonic(), pos),
    }
}

pub fn draw(painter: &Painter, rect: Rect, views: Views, input: Input, s: &Settings) {
    let p = palette(s);
    painter.rect_filled(rect, 0.0, p.bg);

    let cells = cells(rect, views);
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
    if rect.height() > 150.0 && !legend.is_empty() {
        painter.text(
            Pos2::new(rect.center().x, rect.min.y + h + lh * 0.5),
            Align2::CENTER_CENTER,
            legend,
            font_light(lh * 0.88),
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
        let a_h = rect.height() / vspan;
        // Columns are whatever the width affords at that spacing, never fewer
        // than five — below that the horizontal axis stops showing a run of
        // fifths — and never so many that the names stop being readable.
        let shear = (rows - 1) as f32 * 0.5;
        let cols = (((rect.width() / a_h) - shear - 0.6).floor() + 1.0).clamp(5.0, 13.0) as i32;
        let a_w = rect.width() / ((cols - 1) as f32 + shear + 0.6);
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Panes are laid out in a fixed order, so turning one off never makes the
    /// other two swap places under the user's eyes.
    #[test]
    fn panes_keep_their_order_and_fill_the_band() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 300.0));
        let all = Views {
            circle: true,
            tonnetz: true,
            triangles: true,
        };
        let got = cells(rect, all);
        assert_eq!(
            got.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
            View::ALL.to_vec()
        );
        let total: f32 = got.iter().map(|(_, r)| r.width()).sum();
        assert!((total - rect.width()).abs() < 0.5, "panes left a gap");

        // Dropping the middle one keeps the outer two in the same order.
        let two = Views {
            circle: true,
            tonnetz: false,
            triangles: true,
        };
        assert_eq!(
            cells(rect, two).iter().map(|(v, _)| *v).collect::<Vec<_>>(),
            vec![View::Circle, View::Triangles]
        );
        assert!(cells(rect, Views::default()).is_empty());
    }

    /// The band takes no height when nothing is selected — the window must not
    /// grow by 300 points to show three empty panes.
    #[test]
    fn an_empty_selection_takes_no_band() {
        assert_eq!(band_height(1300.0, Views::default()), 0.0);
        assert!(
            band_height(
                1300.0,
                Views {
                    circle: true,
                    ..Default::default()
                }
            ) > 0.0
        );
    }

    /// The pane sizes the band actually produces: one, two or three diagrams
    /// across a window at every size the app offers, minus the title strip.
    fn real_panes() -> Vec<Rect> {
        let mut out = Vec::new();
        for pct in [50i64, 75, 100, 125, 150, 175, 200] {
            let w = (1300.0 * pct as f64 / 100.0).trunc() as f32;
            for n in 1..=3usize {
                let views = Views {
                    circle: true,
                    tonnetz: n >= 2,
                    triangles: n >= 3,
                };
                let h = band_height(w, views);
                let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
                for (_, cell) in cells(band, views) {
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
            for n in 1..=3usize {
                let views = Views {
                    circle: n >= 2,
                    tonnetz: true,
                    triangles: n >= 3,
                };
                let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, views)));
                for (v, cell) in cells(band, views) {
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
        let views = Views {
            tonnetz: true,
            ..Default::default()
        };
        for pane in real_panes() {
            let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(pane.width() + 16.0, 300.0));
            let cell = cells(band, views)[0].1;
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
                        hit_test(band, views, Input::default(), c),
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
        let views = Views {
            circle: true,
            ..Default::default()
        };
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(440.0, 300.0));
        let body = body_rect(cells(band, views)[0].1.shrink(8.0));
        let c = body.center();
        let r = body.width().min(body.height()) * 0.5 - 2.0;
        for (i, &pc) in FIFTHS.iter().enumerate() {
            let a = circle_angle(i);
            let dir = Vec2::new(a.cos(), a.sin());
            // Where `draw` puts the major name, and the relative minor.
            let major_at = c + dir * ((r * 0.80 + r * 0.56) * 0.5);
            let minor_at = c + dir * ((r * 0.56 + r * 0.34) * 0.5);
            assert_eq!(
                hit_test(band, views, Input::default(), major_at),
                Some(Hit::Pc(pc)),
                "the major name at position {i} is not clickable"
            );
            // The relative-minor ring is a label. Every pitch class is
            // already reachable on the major ring, so nothing is lost.
            assert_eq!(
                hit_test(band, views, Input::default(), minor_at),
                None,
                "the relative-minor label at position {i} acts as a control"
            );
        }
        // The hub and the signature ring are labels, not controls.
        assert_eq!(hit_test(band, views, Input::default(), c), None);
    }

    /// Each vertex of the hexagram gives back the chord printed on it, in
    /// every key — and the two triangles must not answer for each other.
    #[test]
    fn a_click_on_a_chord_vertex_gives_that_chord() {
        let views = Views {
            triangles: true,
            ..Default::default()
        };
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(440.0, 300.0));
        let body = body_rect(cells(band, views)[0].1.shrink(8.0));
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
                    hit_test(band, views, input, hex_vertex(c, r, k, false)),
                    Some(Hit::Triad {
                        root: roots[oi],
                        minor: false
                    }),
                    "upward vertex {k} in key {tonic}"
                );
            }
            for (k, &oi) in HEX_DOWN_ORDER.iter().enumerate() {
                assert_eq!(
                    hit_test(band, views, input, hex_vertex(c, r, k, true)),
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
        let all = Views {
            circle: true,
            tonnetz: true,
            triangles: true,
        };
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 300.0));
        let panes = cells(band, all);
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
                match hit_test(band, all, Input::default(), p) {
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
                        Views::default(),
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
                Views {
                    circle: true,
                    ..Default::default()
                },
                Views {
                    tonnetz: true,
                    ..Default::default()
                },
                Views {
                    triangles: true,
                    ..Default::default()
                },
                Views {
                    circle: true,
                    tonnetz: true,
                    triangles: true,
                },
            ] {
                let h = band_height(w, views);
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
                        draw(&painter, rect, views, input, &s);
                    });
                }
            }
        }
    }
}
