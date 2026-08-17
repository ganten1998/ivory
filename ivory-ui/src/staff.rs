//! The sheet music band: what you are playing, engraved.
//!
//! It sits between the theory diagrams and the chord name, which is where it
//! belongs in the chain of increasing abstraction the window already is —
//! keys at the bottom, then the notes on a staff, then the chord's name, then
//! the diagrams that say what the name means.
//!
//! # What it draws, and what it deliberately does not
//!
//! **Whole notes, and no rhythm.** This band shows the pitches that are
//! sounding right now. It knows nothing about duration and it must not imply
//! any: a filled notehead with a stem is a crotchet, and a crotchet is a claim
//! about time that this app has no basis for. Semibreves say "these pitches"
//! and nothing else, which is exactly what a harmony textbook uses them for.
//! It also means no stems, no flags, no beams and no stem-direction rules —
//! the three hardest parts of engraving are not skipped here, they are
//! genuinely absent from the problem.
//!
//! **No key signature**, so no naturals. Accidentals are per-note and follow
//! `prefer_flats`, exactly like every other note name in the app.
//!
//! **Seconds are offset**, which is the one engraving rule that is not
//! optional: two noteheads a diatonic second apart cannot share a side, and a
//! chord drawn without that rule is a smear rather than an interval. See
//! [`Column::place`].
//!
//! # Clefs
//!
//! A clef is three facts — which pitch it names, which line that pitch sits on,
//! and whether the written note sounds where it is written — and every clef in
//! [`Clef`] is those three numbers plus a drawing. That is why adding the
//! octave-down treble a guitarist reads was a one-line change and why alto and
//! tenor are the same glyph at two heights.
//!
//! # The class view
//!
//! [`StaffSet::Custom`] stacks any combination of clefs, and **every staff
//! shows every note**. That is the difference between it and the grand staff,
//! which splits the notes between its two staves the way a pianist reads them.
//! The custom set is for a room with a violist and a cellist in it: they are
//! reading the same chord, each in their own clef, and a note that vanished
//! from the alto staff because it happened to sit low would be the one thing
//! that makes the view useless to the person reading it.
//!
//! # 8va
//!
//! Ledger lines are countable up to about three. Past that a reader is
//! arithmetic, not sight-reading, so the notes move an octave (or two) and take
//! a bracket with them — which is what an engraver does and why `8va` exists.
//! See [`OCTAVE_AT`].

use crate::fonts;
use crate::settings::Settings;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};
use std::collections::HashSet;

// ── the band ───────────────────────────────────────────────────────────────

/// The staff itself: five lines are four spaces.
const STAFF_SPACES: f32 = 4.0;

/// Room above the top staff and below the bottom one, in spaces.
///
/// Not decoration. [`OCTAVE_AT`] lets a note reach three ledger lines past the
/// staff, which is three spaces, and the `8va` sign sits a space and a half
/// past that — so a band with less than this draws its ledger lines over the
/// band above it.
const OUTER_SPACES: f32 = 4.5;

/// The gap BETWEEN two staves of a grand staff.
///
/// Four spaces, which is what an engraver leaves, and it is not the same
/// question as `OUTER_SPACES`: the two staves of a grand staff share the region
/// between them — middle C's ledger line lives there — and a pianist's notes go
/// to the other staff rather than deeper into the gap. Ten spaces of air with
/// one ledger line in the middle does not read as a grand staff, it reads as
/// two staves that happen to be near each other.
const GRAND_GAP: f32 = 4.0;

/// The gap between two INDEPENDENT staves.
///
/// Wider, because they are different instruments: each one shows every note, so
/// each needs its own ledger room on both sides and neither can borrow the
/// other's.
const SCORE_GAP: f32 = 7.0;

/// The staff space at a 1300pt-wide window, in points. Everything else in this
/// module is measured in staff spaces, which is how engraving works and why a
/// staff drawn at any size looks like the same staff.
const SPACE_AT_1300: f64 = 7.5;

/// The band's whole height in staff spaces, which is the one place the vertical
/// arrangement is decided. The painter reads it back to recover the space size,
/// so the drawing cannot disagree with the height the window was sized to.
fn total_spaces(staves: usize, splits: bool) -> f32 {
    if staves == 0 {
        return 0.0;
    }
    let gap = if splits { GRAND_GAP } else { SCORE_GAP };
    staves as f32 * STAFF_SPACES + (staves as f32 - 1.0) * gap + OUTER_SPACES * 2.0
}

/// Height of the sheet music band for a window `w` wide, or 0 when it is off.
/// Truncated like every other band in the layout.
pub fn band_height(w: f32, set: &StaffSet) -> f32 {
    let staves = set.clefs().len();
    if staves == 0 {
        return 0.0;
    }
    let space = SPACE_AT_1300 * f64::from(w) / 1300.0;
    (space * f64::from(total_spaces(staves, set.splits()))).trunc() as f32
}

// ── pitch, spelled ─────────────────────────────────────────────────────────

/// The seven letters, as an offset from C within an octave.
const LETTERS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "B"];

/// How a pitch class is spelled with sharps: `(letter index, alteration)`.
const SHARP_SPELLING: [(i32, i8); 12] = [
    (0, 0),
    (0, 1),
    (1, 0),
    (1, 1),
    (2, 0),
    (3, 0),
    (3, 1),
    (4, 0),
    (4, 1),
    (5, 0),
    (5, 1),
    (6, 0),
];

/// The same twelve with flats. Note that the naturals are identical — only the
/// five black keys differ, which is the whole of what `prefer_flats` decides.
const FLAT_SPELLING: [(i32, i8); 12] = [
    (0, 0),
    (1, -1),
    (1, 0),
    (2, -1),
    (2, 0),
    (3, 0),
    (4, -1),
    (4, 0),
    (5, -1),
    (5, 0),
    (6, -1),
    (6, 0),
];

/// A MIDI note, spelled: which line-or-space it lives on and what has to be
/// printed in front of it.
///
/// `step` counts DIATONIC steps, seven to the octave, so that a third is two
/// steps whichever notes it is made of — which is the only counting under which
/// a staff makes sense. Middle C is 28.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spelled {
    pub midi: u8,
    pub step: i32,
    pub alter: i8,
    pub letter: &'static str,
}

/// Spell a MIDI note. `prefer_flats` decides the five black keys and nothing
/// else.
pub fn spell(midi: u8, prefer_flats: bool) -> Spelled {
    let table = if prefer_flats {
        FLAT_SPELLING
    } else {
        SHARP_SPELLING
    };
    let (letter, alter) = table[(midi % 12) as usize];
    // MIDI 60 is C4, so the octave is `midi / 12 - 1`. Middle C therefore
    // lands on step 4 * 7 + 0 = 28.
    let octave = i32::from(midi) / 12 - 1;
    Spelled {
        midi,
        step: octave * 7 + letter,
        alter,
        letter: LETTERS[letter as usize],
    }
}

// ── clefs ──────────────────────────────────────────────────────────────────

/// A clef: the pitch it names, the line it names it on, and whether what is
/// written is what sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Clef {
    /// G on the second line. What everybody means by "the treble clef".
    Treble,
    /// F on the fourth line.
    Bass,
    /// C on the middle line. Viola, and the one clef most players have never
    /// had to read.
    Alto,
    /// C on the fourth line. Cello, bassoon and trombone in their upper range.
    Tenor,
    /// Treble, sounding an octave below what is written. Guitar reads this, and
    /// so does every tenor in every choir — and both of them are used to being
    /// handed music that lies about its octave.
    Treble8vb,
    /// Bass, sounding an octave below. Double bass and contrabassoon.
    Bass8vb,
}

impl Clef {
    pub const ALL: [Clef; 6] = [
        Clef::Treble,
        Clef::Bass,
        Clef::Alto,
        Clef::Tenor,
        Clef::Treble8vb,
        Clef::Bass8vb,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Clef::Treble => "Treble",
            Clef::Bass => "Bass",
            Clef::Alto => "Alto",
            Clef::Tenor => "Tenor",
            Clef::Treble8vb => "Treble 8vb (guitar, tenor)",
            Clef::Bass8vb => "Bass 8vb (double bass)",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Clef::Treble => "treble",
            Clef::Bass => "bass",
            Clef::Alto => "alto",
            Clef::Tenor => "tenor",
            Clef::Treble8vb => "treble8vb",
            Clef::Bass8vb => "bass8vb",
        }
    }

    pub fn from_key(k: &str) -> Option<Clef> {
        Clef::ALL.into_iter().find(|c| c.key() == k)
    }

    /// The diatonic step this clef names, and the line it names it on —
    /// 0 for the bottom line, 4 for the top.
    ///
    /// The 8vb clefs name a pitch an octave lower than their plain form: a note
    /// written where G4 goes in the treble clef SOUNDS G3 in the octave-down
    /// one, so a sounding G3 is drawn there.
    fn reference(self) -> (i32, i32) {
        match self {
            // G4 = step 32, second line.
            Clef::Treble => (32, 1),
            // G3 = step 25.
            Clef::Treble8vb => (25, 1),
            // F3 = step 24, fourth line.
            Clef::Bass => (24, 3),
            // F2 = step 17.
            Clef::Bass8vb => (17, 3),
            // C4 = step 28, middle line.
            Clef::Alto => (28, 2),
            // C4 on the fourth line.
            Clef::Tenor => (28, 3),
        }
    }

    /// Half-space position of a diatonic step: 0 is the bottom line, 8 the top,
    /// odd numbers the spaces between.
    fn position(self, step: i32) -> i32 {
        let (ref_step, ref_line) = self.reference();
        (step - ref_step) + ref_line * 2
    }

    /// The step that sits in the middle of this staff, for deciding which staff
    /// of a grand pair a note belongs to.
    fn centre_step(self) -> i32 {
        let (ref_step, ref_line) = self.reference();
        ref_step + (2 - ref_line)
    }
}

// ── what is on screen ──────────────────────────────────────────────────────

/// Which staves the band is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaffSet {
    /// Treble over bass, notes split between them at middle C the way a pianist
    /// reads them. The default, because this app is a piano first.
    Grand,
    /// One staff, showing everything.
    Single(Clef),
    /// Any stack of clefs, each showing EVERY note. See the module docs.
    Custom(Vec<Clef>),
}

impl StaffSet {
    /// The clefs to draw, top to bottom.
    pub fn clefs(&self) -> Vec<Clef> {
        match self {
            StaffSet::Grand => vec![Clef::Treble, Clef::Bass],
            StaffSet::Single(c) => vec![*c],
            StaffSet::Custom(v) => v.clone(),
        }
    }

    /// Whether the notes are divided between the staves rather than shown on
    /// all of them.
    pub fn splits(&self) -> bool {
        matches!(self, StaffSet::Grand)
    }

    pub fn key(&self) -> String {
        match self {
            StaffSet::Grand => "grand".to_owned(),
            StaffSet::Single(c) => c.key().to_owned(),
            StaffSet::Custom(v) => {
                let names: Vec<&str> = v.iter().map(|c| c.key()).collect();
                format!("custom:{}", names.join("+"))
            }
        }
    }

    /// Parse what `key` wrote. Anything unreadable is the grand staff, which is
    /// the same rule every other setting in this app follows: a hand-edited
    /// file gets the default, not an error.
    pub fn from_key(k: &str) -> StaffSet {
        if let Some(rest) = k.strip_prefix("custom:") {
            let clefs: Vec<Clef> = rest.split('+').filter_map(Clef::from_key).collect();
            if !clefs.is_empty() {
                return StaffSet::Custom(clefs);
            }
            return StaffSet::Grand;
        }
        match k {
            "grand" => StaffSet::Grand,
            other => Clef::from_key(other).map_or(StaffSet::Grand, StaffSet::Single),
        }
    }

    /// The order `I` walks: the grand staff, then each single clef. A custom
    /// set is not in the cycle — it is a thing the user built, and cycling off
    /// it and back would need somewhere to keep it.
    pub fn next(&self) -> StaffSet {
        let order: Vec<StaffSet> = std::iter::once(StaffSet::Grand)
            .chain(Clef::ALL.into_iter().map(StaffSet::Single))
            .collect();
        match order.iter().position(|s| s == self) {
            Some(i) => order[(i + 1) % order.len()].clone(),
            // Coming off a custom set lands on the grand staff.
            None => StaffSet::Grand,
        }
    }

    /// Add or remove `clef` from a custom stack, keeping it in SCORE order —
    /// highest instrument at the top, which is where a reader expects to find
    /// the flute and not the double bass.
    ///
    /// Returns `None` when the last clef was removed: a set with no staves in
    /// it is not a view, and the caller falls back to the grand staff rather
    /// than drawing an empty band.
    pub fn with_clef_toggled(&self, clef: Clef) -> Option<StaffSet> {
        let mut clefs = match self {
            StaffSet::Custom(v) => v.clone(),
            other => other.clefs(),
        };
        if let Some(i) = clefs.iter().position(|c| *c == clef) {
            clefs.remove(i);
        } else {
            clefs.push(clef);
        }
        if clefs.is_empty() {
            return None;
        }
        clefs.sort_by_key(|c| std::cmp::Reverse(c.centre_step()));
        clefs.dedup();
        Some(StaffSet::Custom(clefs))
    }

    pub fn contains(&self, clef: Clef) -> bool {
        self.clefs().contains(&clef)
    }

    pub fn label(&self) -> String {
        match self {
            StaffSet::Grand => "Grand staff".to_owned(),
            StaffSet::Single(c) => c.label().to_owned(),
            StaffSet::Custom(v) => {
                let names: Vec<&str> = v.iter().map(|c| c.label()).collect();
                names.join(" + ")
            }
        }
    }
}

/// How many ledger lines a reader will count before an engraver gives up and
/// writes `8va` instead.
///
/// Three is the usual limit and it is not arbitrary: up to three, the eye reads
/// the gaps; past that it counts, and counting is not sight-reading.
const OCTAVE_AT: i32 = 3;

/// One note, placed: where it sits, what octave sign it is under, and which
/// side of the column it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placed {
    note: Spelled,
    /// Half-spaces from the bottom line, AFTER any octave transposition.
    pos: i32,
    /// -1 for `8vb`, 0 for none, +1 for `8va`, ±2 for the 15ths.
    octave: i32,
    /// Notes a second apart cannot share a side of the column.
    offset: bool,
    /// Which column of accidentals this note's sharp or flat sits in, counted
    /// leftwards from the noteheads. Zero for a note that needs none.
    acc_col: usize,
}

/// How far apart two accidentals must be, in half-spaces, to share a column.
///
/// A flat is about two spaces tall and a sharp a little more, so anything
/// closer than three spaces overlaps. Getting this wrong does not look like a
/// bug — it looks like a chord with a smear in front of it, which is precisely
/// the failure a reader blames on themselves.
const ACC_CLEARANCE: i32 = 6;

/// The notes on one staff, resolved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Column {
    notes: Vec<Placed>,
}

impl Column {
    /// Place `notes` on `clef`: transpose anything too far out into an octave
    /// sign, then offset the seconds.
    fn place(clef: Clef, notes: &[Spelled]) -> Column {
        let mut placed: Vec<Placed> = notes
            .iter()
            .map(|n| {
                let mut pos = clef.position(n.step);
                let mut octave = 0;
                // Above the top line by more than `OCTAVE_AT` ledger lines, or
                // below the bottom by the same. Two octaves at most: past 15ma
                // an engraver would change clef, and so should the user.
                while pos > 8 + OCTAVE_AT * 2 && octave < 2 {
                    pos -= 7;
                    octave += 1;
                }
                while pos < -(OCTAVE_AT * 2) && octave > -2 {
                    pos += 7;
                    octave -= 1;
                }
                Placed {
                    note: *n,
                    pos,
                    octave,
                    offset: false,
                    acc_col: 0,
                }
            })
            .collect();
        placed.sort_by_key(|p| (p.pos, p.note.midi));

        // **Seconds go on opposite sides of the column.** Two noteheads one
        // step apart overlap by most of their width; printed on the same side
        // they are an inkblot, and an interval nobody can name is the one thing
        // this band exists to prevent. Walk upwards and offset a note whenever
        // its neighbour below is a second away AND was not itself offset — so a
        // cluster alternates rather than every note after the first drifting
        // right.
        for i in 1..placed.len() {
            if placed[i].pos - placed[i - 1].pos == 1 && !placed[i - 1].offset {
                placed[i].offset = true;
            }
        }

        // **Accidentals stack into columns, from the top down.** Four flats in
        // one column is four flats on top of each other: a flat is two spaces
        // tall and the notes of a seventh chord are one space apart. An
        // engraver works down from the highest note, putting each accidental in
        // the rightmost column that has room for it, which is what this is.
        let mut columns: Vec<Vec<i32>> = Vec::new();
        for i in (0..placed.len()).rev() {
            if placed[i].note.alter == 0 {
                continue;
            }
            let pos = placed[i].pos;
            let col = columns
                .iter()
                .position(|used: &Vec<i32>| {
                    used.iter().all(|p| (p - pos).abs() >= ACC_CLEARANCE)
                })
                .unwrap_or_else(|| {
                    columns.push(Vec::new());
                    columns.len() - 1
                });
            columns[col].push(pos);
            placed[i].acc_col = col;
        }
        Column { notes: placed }
    }

}

/// Split `notes` between the staves of `set`, or give every staff all of them.
///
/// The split rule is the pianist's one: a note goes to the staff whose middle
/// it is nearest, and a tie goes UP — middle C is written on a ledger line
/// under the treble staff far more often than over the bass one.
fn distribute(set: &StaffSet, notes: &[Spelled]) -> Vec<Vec<Spelled>> {
    let clefs = set.clefs();
    if !set.splits() || clefs.len() < 2 {
        return clefs.iter().map(|_| notes.to_vec()).collect();
    }
    let mut out = vec![Vec::new(); clefs.len()];
    for n in notes {
        let mut best = 0;
        let mut best_d = i32::MAX;
        for (i, c) in clefs.iter().enumerate() {
            let d = (n.step - c.centre_step()).abs();
            // `<` and not `<=`, so the earlier — that is, the higher — staff
            // keeps a tie.
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        out[best].push(*n);
    }
    out
}

// ── palette ────────────────────────────────────────────────────────────────

struct Palette {
    bg: Color32,
    ink: Color32,
    lit: Color32,
    lit_text: Color32,
}

/// Black on ivory, and its inverse in dark mode — the same two colours the
/// theory band and the chord strip use, because a page of music that did not
/// match the app it is in would read as a screenshot of something else.
fn palette(s: &Settings) -> Palette {
    if s.dark_mode {
        Palette {
            bg: Color32::from_rgb(0x0a, 0x0a, 0x0a),
            ink: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            lit: s.white_key_active_color.to_color32(),
            lit_text: Color32::BLACK,
        }
    } else {
        Palette {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            ink: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            lit: s.white_key_active_color.to_color32(),
            lit_text: Color32::BLACK,
        }
    }
}

// ── drawing ────────────────────────────────────────────────────────────────

/// One staff's geometry, in points.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    /// The space between two staff lines.
    space: f32,
    /// The y of the BOTTOM line.
    bottom: f32,
    left: f32,
    right: f32,
}

impl Geometry {
    /// The y of a half-space position: 0 is the bottom line, 8 the top.
    fn y(&self, pos: i32) -> f32 {
        self.bottom - pos as f32 * self.space * 0.5
    }
}

/// Draw the band.
pub fn draw(painter: &Painter, rect: Rect, notes: &HashSet<u8>, s: &Settings) {
    if !rect.is_positive() {
        return;
    }
    let p = palette(s);
    painter.rect_filled(rect, 0.0, p.bg);

    let set = s.staff_set();
    let clefs = set.clefs();
    if clefs.is_empty() {
        return;
    }
    let mut sorted: Vec<u8> = notes.iter().copied().collect();
    sorted.sort_unstable();
    let spelled: Vec<Spelled> = sorted
        .iter()
        .map(|n| spell(*n, s.prefer_flats))
        .collect();
    let per_staff = distribute(&set, &spelled);

    let spaces = total_spaces(clefs.len(), set.splits());
    let space = (rect.height() / spaces).max(1.0);
    let gap = if set.splits() { GRAND_GAP } else { SCORE_GAP };
    // A margin that grows with the staff rather than with the window, so the
    // clef sits the same distance from the edge at every size.
    let margin = space * 2.2;
    // The bottom line of each staff, top to bottom. One list, so the brace and
    // the staves cannot end up in different places.
    let bottoms: Vec<f32> = (0..clefs.len())
        .map(|i| {
            rect.top()
                + space * (OUTER_SPACES + STAFF_SPACES + i as f32 * (STAFF_SPACES + gap))
        })
        .collect();

    for (i, clef) in clefs.iter().enumerate() {
        let g = Geometry {
            space,
            bottom: bottoms[i],
            left: rect.left() + margin,
            right: rect.right() - margin,
        };
        draw_staff(painter, g, *clef, &per_staff[i], s, &p);
    }

    // The brace and the end bars that make two staves one instrument. Only for
    // the grand staff: a stack of independent clefs is a score, and joining a
    // violist to a cellist with a piano brace would be saying something untrue.
    if set.splits() && clefs.len() == 2 {
        let top = bottoms[0] - space * STAFF_SPACES;
        let bottom = bottoms[1];
        let left = rect.left() + margin;
        let right = rect.right() - margin;
        draw_brace(painter, left - space * 1.1, top, bottom, space, &p);
        for x in [left, right] {
            painter.line_segment(
                [Pos2::new(x, top), Pos2::new(x, bottom)],
                Stroke::new((space * 0.14).max(1.0), p.ink),
            );
        }
    }
}

/// The curly brace that says "one player, two hands".
///
/// Drawn rather than typeset, and drawn as a real brace rather than the
/// straight bracket that is quicker: a straight bracket means a GROUP of
/// instruments — a section of a score — and using one here would be saying
/// something about the music that is not true.
fn draw_brace(painter: &Painter, x: f32, top: f32, bottom: f32, space: f32, p: &Palette) {
    let mid = (top + bottom) * 0.5;
    let w = space * 0.85;
    let stroke = Stroke::new((space * 0.16).max(1.0), p.ink);
    for (from, to) in [(top, mid), (bottom, mid)] {
        let dir = (to - from).signum();
        let path = bezier(
            Pos2::new(x + w * 0.55, from),
            Pos2::new(x - w * 0.55, from + dir * space * 1.1),
            Pos2::new(x + w * 0.95, to - dir * space * 1.4),
            Pos2::new(x - w * 0.15, to),
            26,
        );
        let mut full = vec![Pos2::new(x + w * 0.55, from)];
        full.extend(path);
        painter.add(egui::Shape::line(full, stroke));
    }
}

/// One staff: its lines, its clef, and whatever is on it.
fn draw_staff(
    painter: &Painter,
    g: Geometry,
    clef: Clef,
    notes: &[Spelled],
    s: &Settings,
    p: &Palette,
) {
    let hair = (g.space * 0.09).max(1.0);
    for line in 0..5 {
        let y = g.y(line * 2);
        painter.line_segment(
            [Pos2::new(g.left, y), Pos2::new(g.right, y)],
            Stroke::new(hair, p.ink),
        );
    }
    let after_clef = draw_clef(painter, g, clef, p);

    let column = Column::place(clef, notes);
    if column.notes.is_empty() {
        return;
    }
    // The chord sits a little right of the clef rather than in the middle of
    // the staff: a reader's eye starts at the clef, and a chord adrift in the
    // centre of an otherwise empty system looks like a mistake.
    let head_w = g.space * 1.18;
    let x = (after_clef + g.space * 3.0).min(g.right - head_w * 2.5);
    draw_column(painter, g, &column, x, s, p);
}

/// The chord: accidentals, noteheads, ledger lines and any octave bracket.
fn draw_column(
    painter: &Painter,
    g: Geometry,
    column: &Column,
    x: f32,
    s: &Settings,
    p: &Palette,
) {
    let head_w = g.space * 1.18;
    // Accidentals stack leftwards from the noteheads, in as many columns as the
    // chord needs. See `Column::place`.
    let acc_step = g.space * 1.05;

    for placed in &column.notes {
        let y = g.y(placed.pos);
        let cx = if placed.offset { x + head_w } else { x };

        // Ledger lines first, so a notehead sits ON them rather than under
        // them — a ledger line drawn over a hollow head fills it in.
        draw_ledgers(painter, g, placed.pos, cx, head_w, p);

        draw_notehead(
            painter,
            Pos2::new(cx, y),
            g.space,
            p.lit,
            s.staff_note_names,
            p,
        );

        if placed.note.alter != 0 {
            draw_accidental(
                painter,
                Pos2::new(x - g.space * 1.15 - placed.acc_col as f32 * acc_step, y),
                g.space,
                placed.note.alter,
                p.ink,
            );
        }
        if s.staff_note_names {
            // Inside the head, which is where a teaching edition puts it and
            // the only place it does not collide with the note above.
            painter.text(
                Pos2::new(cx, y),
                Align2::CENTER_CENTER,
                placed.note.letter,
                FontId::new(g.space * 0.86, fonts::courier_bold()),
                p.lit_text,
            );
        }
    }
    draw_octave_signs(painter, g, column, x, head_w, p);
}

/// A semibreve: a hollow ellipse leaning the way a broad-nibbed pen leaves it.
///
/// Drawn as a polygon rather than a circle, because a notehead is an ellipse at
/// about -20 degrees and a circle reads as a bullet point. The hole is a second
/// polygon in the background colour rather than a stroke, so the head keeps its
/// thick-thin weight — which is the whole visual signature of a notehead.
fn draw_notehead(painter: &Painter, c: Pos2, space: f32, fill: Color32, named: bool, p: &Palette) {
    let outer = ellipse(c, space * 0.72, space * 0.5, -0.35);
    painter.add(egui::Shape::convex_polygon(outer, fill, Stroke::NONE));
    // **No counter when the head is carrying a letter.** A semibreve's hole is
    // a narrow slot — three or four points across at the size this band draws —
    // and a letter printed into it is a smudge with a ring round it. A filled
    // head with a letter on it is what every teaching edition uses, and nobody
    // reads a labelled notehead as a claim about rhythm.
    if !named {
        let inner = ellipse(c, space * 0.44, space * 0.22, -0.35);
        painter.add(egui::Shape::convex_polygon(inner, p.bg, Stroke::NONE));
    }
    // A hairline round the outside, so a lit notehead in the accent colour
    // still reads as a note on a page rather than as a coloured blob.
    let edge = ellipse(c, space * 0.72, space * 0.5, -0.35);
    painter.add(egui::Shape::closed_line(
        edge,
        Stroke::new((space * 0.07).max(0.8), p.ink),
    ));
}

/// A rotated ellipse as a polygon. Sixteen points is enough that the eye cannot
/// find the corners at any size this band draws.
fn ellipse(c: Pos2, rx: f32, ry: f32, tilt: f32) -> Vec<Pos2> {
    let (sin, cos) = tilt.sin_cos();
    (0..16)
        .map(|i| {
            let t = std::f32::consts::TAU * f32::from(i as u8) / 16.0;
            let (x, y) = (rx * t.cos(), ry * t.sin());
            Pos2::new(c.x + x * cos - y * sin, c.y + x * sin + y * cos)
        })
        .collect()
}

/// The short lines a note stands on when it has left the staff.
fn draw_ledgers(painter: &Painter, g: Geometry, pos: i32, cx: f32, head_w: f32, p: &Palette) {
    let hair = (g.space * 0.11).max(1.0);
    let half = head_w * 0.78;
    let line = |pos: i32| {
        let y = g.y(pos);
        painter.line_segment(
            [Pos2::new(cx - half, y), Pos2::new(cx + half, y)],
            Stroke::new(hair, p.ink),
        );
    };
    // Every LINE position outside the staff up to the note. A note in the space
    // above a ledger line does not get one of its own, which is why this counts
    // to `pos` rather than through it.
    let mut l = 10;
    while l <= pos {
        line(l);
        l += 2;
    }
    let mut l = -2;
    while l >= pos {
        line(l);
        l -= 2;
    }
}

/// `8va` / `8vb` and their dashed brackets.
fn draw_octave_signs(
    painter: &Painter,
    g: Geometry,
    column: &Column,
    x: f32,
    head_w: f32,
    p: &Palette,
) {
    for dir in [1_i32, -1] {
        let group: Vec<&Placed> = column
            .notes
            .iter()
            .filter(|n| n.octave.signum() == dir)
            .collect();
        if group.is_empty() {
            continue;
        }
        let steps = group.iter().map(|n| n.octave.abs()).max().unwrap_or(1);
        let text = match (dir, steps) {
            (1, 1) => "8va",
            (1, _) => "15ma",
            (_, 1) => "8vb",
            _ => "15mb",
        };
        // Clear of the notes it covers, and of the staff itself: above the
        // highest thing in the group, or below the lowest.
        let edge = if dir > 0 {
            group.iter().map(|n| n.pos).max().unwrap_or(8).max(8) + 3
        } else {
            group.iter().map(|n| n.pos).min().unwrap_or(0).min(0) - 3
        };
        let y = g.y(edge);
        let size = g.space * 1.15;
        painter.text(
            Pos2::new(x - head_w * 1.2, y),
            Align2::RIGHT_CENTER,
            text,
            FontId::new(size, fonts::courier_bold()),
            p.ink,
        );
        // The dashed line that says how far the sign reaches, with the hook at
        // its end pointing back at the notes it applies to.
        let from = x - head_w * 0.9;
        let to = x + head_w * 2.2;
        let dash = g.space * 0.5;
        let mut dx = from;
        while dx < to {
            let end = (dx + dash).min(to);
            painter.line_segment(
                [Pos2::new(dx, y), Pos2::new(end, y)],
                Stroke::new((g.space * 0.1).max(1.0), p.ink),
            );
            dx += dash * 2.0;
        }
        painter.line_segment(
            [
                Pos2::new(to, y),
                Pos2::new(to, y + dir as f32 * g.space * 0.7),
            ],
            Stroke::new((g.space * 0.1).max(1.0), p.ink),
        );
    }
}

/// A sharp or a flat, drawn rather than typeset — the same bargain the whole
/// app makes with glyphs it cannot rely on a font to carry.
fn draw_accidental(painter: &Painter, c: Pos2, space: f32, alter: i8, ink: Color32) {
    let thin = Stroke::new((space * 0.1).max(1.0), ink);
    let thick = Stroke::new((space * 0.22).max(1.2), ink);
    if alter > 0 {
        // Two uprights and two thick slanted crossbars — the slant is what
        // distinguishes a sharp from a hash at ten points.
        for dx in [-0.22_f32, 0.22] {
            painter.line_segment(
                [
                    Pos2::new(c.x + dx * space, c.y - space * 0.85),
                    Pos2::new(c.x + dx * space, c.y + space * 0.85),
                ],
                thin,
            );
        }
        for dy in [-0.28_f32, 0.28] {
            painter.line_segment(
                [
                    Pos2::new(c.x - space * 0.42, c.y + (dy + 0.12) * space),
                    Pos2::new(c.x + space * 0.42, c.y + (dy - 0.12) * space),
                ],
                thick,
            );
        }
    } else {
        // A flat is a stem with a bowl on its lower right, and the bowl comes
        // to a point where it meets the stem.
        let top = Pos2::new(c.x - space * 0.22, c.y - space * 1.0);
        let foot = Pos2::new(c.x - space * 0.22, c.y + space * 0.55);
        painter.line_segment([top, foot], thin);
        let bowl = vec![
            foot,
            Pos2::new(c.x + space * 0.05, c.y + space * 0.45),
            Pos2::new(c.x + space * 0.38, c.y + space * 0.1),
            Pos2::new(c.x + space * 0.2, c.y - space * 0.18),
            Pos2::new(c.x - space * 0.22, c.y + space * 0.02),
        ];
        painter.add(egui::Shape::line(bowl, thick));
    }
}

// ── the clefs ──────────────────────────────────────────────────────────────

/// Draw `clef` at the left of the staff and return the x its right edge
/// reached, so the music can start after it.
fn draw_clef(painter: &Painter, g: Geometry, clef: Clef, p: &Palette) -> f32 {
    let x = g.left + g.space * 0.8;
    let w = match clef {
        Clef::Treble | Clef::Treble8vb => {
            draw_treble(painter, g, x, p);
            g.space * 2.6
        }
        Clef::Bass | Clef::Bass8vb => {
            draw_bass(painter, g, x, p);
            g.space * 2.8
        }
        Clef::Alto | Clef::Tenor => {
            let line = if clef == Clef::Alto { 2 } else { 3 };
            draw_c_clef(painter, g, x, line, p);
            g.space * 2.6
        }
    };
    if matches!(clef, Clef::Treble8vb | Clef::Bass8vb) {
        // The little 8. Below for both, because both sound an octave DOWN and
        // an 8 above would be saying the opposite.
        let (anchor, dy) = match clef {
            Clef::Treble8vb => (g.y(0), g.space * 1.5),
            _ => (g.y(0), g.space * 1.2),
        };
        painter.text(
            Pos2::new(x + w * 0.45, anchor + dy),
            Align2::CENTER_CENTER,
            "8",
            FontId::new(g.space * 1.4, fonts::courier_bold()),
            p.ink,
        );
    }
    x + w
}

/// The G clef: a spiral round the second line, a bowl, a tall curl and a hook
/// below the staff.
///
/// Built out of a logarithmic spiral and four cubics rather than copied from a
/// font, for the reason [`draw_accidental`] gives — and because a clef is the
/// one thing on this page that everybody recognises instantly, so it has to be
/// right rather than merely present.
fn draw_treble(painter: &Painter, g: Geometry, x: f32, p: &Palette) {
    let s = g.space;
    // The spiral's eye sits ON the second line, which is what makes it the G
    // clef rather than a decoration.
    let eye = Pos2::new(x + s * 0.95, g.y(2));
    let stroke = Stroke::new((s * 0.26).max(1.2), p.ink);

    // The scroll: a logarithmic spiral opening anticlockwise from the eye.
    let mut path: Vec<Pos2> = Vec::new();
    let turns = 1.45_f32;
    let steps = 90;
    let r0 = s * 0.14;
    let k = (s * 1.3 / r0).ln() / (std::f32::consts::TAU * turns);
    for i in 0..=steps {
        let t = std::f32::consts::TAU * turns * i as f32 / steps as f32;
        let r = r0 * (k * t).exp();
        // Starts pointing right and opens upwards, so the outermost sweep
        // passes under the eye and up the left side.
        let a = t + std::f32::consts::FRAC_PI_2;
        path.push(Pos2::new(eye.x + r * a.cos(), eye.y + r * a.sin()));
    }
    // Out of the scroll and up to the top curl.
    let end = *path.last().expect("the spiral has points");
    path.extend(bezier(
        end,
        Pos2::new(eye.x - s * 1.55, eye.y - s * 2.2),
        Pos2::new(eye.x - s * 1.0, eye.y - s * 4.3),
        Pos2::new(eye.x + s * 0.12, eye.y - s * 4.55),
        26,
    ));
    // The curl over the top and back down the right.
    path.extend(bezier(
        Pos2::new(eye.x + s * 0.12, eye.y - s * 4.55),
        Pos2::new(eye.x + s * 0.86, eye.y - s * 4.6),
        Pos2::new(eye.x + s * 0.72, eye.y - s * 3.2),
        Pos2::new(eye.x + s * 0.36, eye.y - s * 2.0),
        20,
    ));
    // The long descent through the eye to below the staff.
    path.extend(bezier(
        Pos2::new(eye.x + s * 0.36, eye.y - s * 2.0),
        Pos2::new(eye.x + s * 0.02, eye.y + s * 0.2),
        Pos2::new(eye.x - s * 0.14, eye.y + s * 1.8),
        Pos2::new(eye.x - s * 0.06, eye.y + s * 2.85),
        28,
    ));
    // The hook, which curls left and comes to its point under the staff.
    path.extend(bezier(
        Pos2::new(eye.x - s * 0.06, eye.y + s * 2.85),
        Pos2::new(eye.x + s * 0.08, eye.y + s * 3.8),
        Pos2::new(eye.x - s * 0.95, eye.y + s * 3.75),
        Pos2::new(eye.x - s * 0.82, eye.y + s * 2.85),
        22,
    ));
    painter.add(egui::Shape::line(path, stroke));
}

/// The F clef: a filled head on the fourth line with a tail curling up and
/// over, and the two dots that say WHICH line.
fn draw_bass(painter: &Painter, g: Geometry, x: f32, p: &Palette) {
    let s = g.space;
    let f = Pos2::new(x + s * 0.9, g.y(6));
    painter.circle_filled(f, s * 0.52, p.ink);
    let stroke = Stroke::new((s * 0.3).max(1.2), p.ink);
    // The tail: up over the head and down to the left, ending under the staff.
    let mut path = vec![Pos2::new(f.x + s * 0.02, f.y - s * 0.52)];
    path.extend(bezier(
        Pos2::new(f.x + s * 0.02, f.y - s * 0.52),
        Pos2::new(f.x + s * 1.5, f.y - s * 0.6),
        Pos2::new(f.x + s * 1.25, f.y + s * 1.55),
        Pos2::new(f.x - s * 0.85, f.y + s * 2.5),
        30,
    ));
    painter.add(egui::Shape::line(path, stroke));
    // The dots straddle the F line and are the whole point of the clef.
    for dy in [-0.5_f32, 0.5] {
        painter.circle_filled(
            Pos2::new(f.x + s * 1.35, f.y + dy * s),
            s * 0.16,
            p.ink,
        );
    }
}

/// The C clef: two bulbs meeting at the line the clef names, against a pair of
/// uprights. `line` is 2 for alto, 3 for tenor — the same glyph, moved.
fn draw_c_clef(painter: &Painter, g: Geometry, x: f32, line: i32, p: &Palette) {
    let s = g.space;
    let c = Pos2::new(x + s * 0.3, g.y(line * 2));
    let top = g.y(8);
    let bottom = g.y(0);
    painter.line_segment(
        [Pos2::new(c.x, top), Pos2::new(c.x, bottom)],
        Stroke::new((s * 0.3).max(1.2), p.ink),
    );
    painter.line_segment(
        [
            Pos2::new(c.x + s * 0.34, top),
            Pos2::new(c.x + s * 0.34, bottom),
        ],
        Stroke::new((s * 0.12).max(1.0), p.ink),
    );
    let stroke = Stroke::new((s * 0.3).max(1.2), p.ink);
    // Two mirrored bulbs, each starting at the staff's edge and coming to a
    // POINT on the named line — which is the whole information content of the
    // glyph, and the reason a C clef is drawn with an arrowhead in the middle
    // rather than a decoration.
    for dir in [-1.0_f32, 1.0] {
        let far = if dir < 0.0 { top } else { bottom };
        let mut path = vec![Pos2::new(c.x + s * 0.42, far)];
        path.extend(bezier(
            Pos2::new(c.x + s * 0.42, far),
            Pos2::new(c.x + s * 1.9, far - dir * s * 0.05),
            Pos2::new(c.x + s * 1.75, c.y + dir * s * 1.35),
            Pos2::new(c.x + s * 0.44, c.y),
            26,
        ));
        painter.add(egui::Shape::line(path, stroke));
    }
    // The arrowhead where they meet. Filled, so the eye lands on the line the
    // clef names rather than on the two curves that point at it.
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(c.x + s * 0.42, c.y - s * 0.5),
            Pos2::new(c.x + s * 1.05, c.y),
            Pos2::new(c.x + s * 0.42, c.y + s * 0.5),
        ],
        p.ink,
        Stroke::NONE,
    ));
}

/// A cubic bezier, sampled. Excludes its first point so segments can be chained
/// without doubling the joins.
fn bezier(a: Pos2, b: Pos2, c: Pos2, d: Pos2, steps: usize) -> Vec<Pos2> {
    (1..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            Pos2::new(
                a.x * w0 + b.x * w1 + c.x * w2 + d.x * w3,
                a.y * w0 + b.y * w1 + c.y * w2 + d.y * w3,
            )
        })
        .collect()
}

/// What the band is showing, or nothing when it is off.
pub fn visible_set(s: &Settings) -> Option<StaffSet> {
    s.show_staff.then(|| s.staff_set())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The clefs name the pitches they are named after.**
    ///
    /// Every clef in this module is three numbers, and getting one of them
    /// wrong produces a staff that is wrong by exactly one step everywhere —
    /// which looks perfectly plausible and is the single worst thing this band
    /// could do, because a reader would trust it.
    #[test]
    fn each_clef_puts_its_own_pitch_on_its_own_line() {
        // Treble: G4 on the second line, so position 2.
        assert_eq!(Clef::Treble.position(spell(67, false).step), 2);
        // Bass: F3 on the fourth line, position 6.
        assert_eq!(Clef::Bass.position(spell(53, false).step), 6);
        // Alto: middle C on the middle line, position 4.
        assert_eq!(Clef::Alto.position(spell(60, false).step), 4);
        // Tenor: middle C on the fourth line, position 6.
        assert_eq!(Clef::Tenor.position(spell(60, false).step), 6);
        // The octave-down clefs put the SOUNDING pitch where the written one
        // would go: a guitarist's low E sounds E2 and is written E3.
        assert_eq!(
            Clef::Treble8vb.position(spell(55, false).step),
            Clef::Treble.position(spell(67, false).step),
            "the octave-down treble is not an octave down"
        );
        assert_eq!(
            Clef::Bass8vb.position(spell(41, false).step),
            Clef::Bass.position(spell(53, false).step)
        );
    }

    /// Middle C is one ledger line below the treble staff and one above the
    /// bass staff. If this is wrong, everything is.
    #[test]
    fn middle_c_sits_where_every_musician_expects_it() {
        assert_eq!(Clef::Treble.position(spell(60, false).step), -2);
        assert_eq!(Clef::Bass.position(spell(60, false).step), 10);
    }

    /// A staff is a diatonic ladder: the same interval is the same distance
    /// wherever it is, which is the property that makes the whole thing
    /// readable and the one a semitone-based layout would break.
    #[test]
    fn a_third_is_a_third_wherever_it_starts() {
        for (low, high) in [(60_u8, 64_u8), (62, 65), (67, 71), (53, 57)] {
            let a = Clef::Treble.position(spell(low, false).step);
            let b = Clef::Treble.position(spell(high, false).step);
            assert_eq!(b - a, 2, "a third from {low} to {high} did not span two steps");
        }
        // And enharmonics land in DIFFERENT places, which is the point of
        // spelling: C sharp and D flat are one key and two notes.
        assert_ne!(
            spell(61, false).step,
            spell(61, true).step,
            "C# and Db were engraved on the same line"
        );
    }

    /// Notes a second apart must not share a side of the column, or a chord is
    /// an inkblot.
    #[test]
    fn seconds_are_offset_and_larger_intervals_are_not() {
        // C D E: the D is a second above the C and moves; the E is a second
        // above the D but the D already moved, so the E comes back.
        let notes: Vec<Spelled> = [60_u8, 62, 64].iter().map(|n| spell(*n, false)).collect();
        let col = Column::place(Clef::Treble, &notes);
        let offsets: Vec<bool> = col.notes.iter().map(|p| p.offset).collect();
        assert_eq!(offsets, vec![false, true, false], "a cluster did not alternate");

        // A triad has nothing a second apart in it and stays in one column.
        let triad: Vec<Spelled> = [60_u8, 64, 67].iter().map(|n| spell(*n, false)).collect();
        let col = Column::place(Clef::Treble, &triad);
        assert!(
            col.notes.iter().all(|p| !p.offset),
            "a plain triad was split across the stem"
        );
    }

    /// **Accidentals stack into columns rather than on top of each other.**
    ///
    /// A flat is two spaces tall and the notes of a seventh chord are one space
    /// apart, so a chord of four flats printed in one column is four flats in
    /// the same place. It does not look like a bug — it looks like a smear, and
    /// a reader blames themselves for not being able to read it.
    #[test]
    fn accidentals_that_would_collide_are_put_in_separate_columns() {
        // C, Eb, Gb, Bb: four flats, each a third apart. Thirds are two
        // half-spaces, well inside the clearance, so no two of the flats can
        // share a column with their neighbour.
        let notes: Vec<Spelled> = [60_u8, 63, 66, 70].iter().map(|n| spell(*n, true)).collect();
        let col = Column::place(Clef::Treble, &notes);
        let flats: Vec<&Placed> = col.notes.iter().filter(|p| p.note.alter != 0).collect();
        assert_eq!(flats.len(), 3, "the spelling changed under this test");
        for a in &flats {
            for b in &flats {
                if a.note.midi != b.note.midi && a.acc_col == b.acc_col {
                    assert!(
                        (a.pos - b.pos).abs() >= ACC_CLEARANCE,
                        "{} and {} share a column {} apart",
                        a.note.midi,
                        b.note.midi,
                        (a.pos - b.pos).abs()
                    );
                }
            }
        }
        // Far apart, and one column is enough: the stacking must not push
        // accidentals outwards when there is no reason to.
        let spread: Vec<Spelled> = [61_u8, 82].iter().map(|n| spell(*n, false)).collect();
        let col = Column::place(Clef::Treble, &spread);
        assert!(
            col.notes.iter().all(|p| p.acc_col == 0),
            "two accidentals an octave and a half apart were split into columns"
        );
        // A note with no accidental never claims a column.
        let plain: Vec<Spelled> = [60_u8, 64, 67].iter().map(|n| spell(*n, false)).collect();
        let col = Column::place(Clef::Treble, &plain);
        assert!(col.notes.iter().all(|p| p.acc_col == 0));
    }

    /// **Past three ledger lines the note moves and takes a sign with it.**
    #[test]
    fn a_note_too_high_to_read_is_written_an_octave_down() {
        // C7 in the treble clef is five ledger lines up — unreadable.
        let high = spell(96, false);
        let plain = Clef::Treble.position(high.step);
        assert!(plain > 8 + OCTAVE_AT * 2, "C7 was not out of range to begin with");

        let col = Column::place(Clef::Treble, &[high]);
        let placed = col.notes[0];
        assert_eq!(placed.octave, 1, "no 8va");
        assert_eq!(placed.pos, plain - 7, "the note did not move a full octave");
        assert!(placed.pos <= 8 + OCTAVE_AT * 2, "it is still off the top");

        // And the same downwards, in the bass clef.
        let low = spell(24, false);
        let col = Column::place(Clef::Bass, &[low]);
        assert_eq!(col.notes[0].octave, -1);
        assert!(col.notes[0].pos >= -(OCTAVE_AT * 2));

        // A note comfortably on the staff is left alone. This is the assertion
        // that fails if the thresholds are ever made too tight, which would put
        // an 8va over ordinary music.
        // C3 is NOT in this list: four ledger lines below the treble staff is
        // exactly the case the sign exists for, and asserting it stayed put
        // would be asserting the feature does not work.
        for n in [60_u8, 64, 67, 72, 55] {
            let col = Column::place(Clef::Treble, &[spell(n, false)]);
            assert_eq!(col.notes[0].octave, 0, "{n} was moved and should not have been");
        }
    }

    /// The grand staff splits at the middle, and every other set shows
    /// everything on every staff — which is what makes the custom set useful to
    /// a room full of people reading different clefs.
    #[test]
    fn the_grand_staff_splits_and_a_class_view_does_not() {
        let notes: Vec<Spelled> = [48_u8, 60, 72].iter().map(|n| spell(*n, false)).collect();
        let split = distribute(&StaffSet::Grand, &notes);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].len() + split[1].len(), 3, "a note was lost or doubled");
        assert!(split[1].iter().any(|n| n.midi == 48), "C3 is not in the bass");
        assert!(split[0].iter().any(|n| n.midi == 72), "C5 is not in the treble");
        // Middle C goes UP: it is written under the treble staff far more often
        // than over the bass one.
        assert!(split[0].iter().any(|n| n.midi == 60), "middle C went down");

        let class = StaffSet::Custom(vec![Clef::Treble, Clef::Alto, Clef::Bass]);
        let all = distribute(&class, &notes);
        assert_eq!(all.len(), 3);
        for staff in &all {
            assert_eq!(staff.len(), 3, "a player lost a note that was being played");
        }
    }

    /// Every set round-trips through the string that goes in the settings file,
    /// and anything unreadable lands on the grand staff rather than on nothing.
    #[test]
    fn a_staff_set_survives_the_settings_file() {
        let sets = [
            StaffSet::Grand,
            StaffSet::Single(Clef::Alto),
            StaffSet::Single(Clef::Treble8vb),
            StaffSet::Custom(vec![Clef::Treble, Clef::Alto, Clef::Bass]),
        ];
        for set in sets {
            assert_eq!(StaffSet::from_key(&set.key()), set, "{} broke", set.key());
        }
        for junk in ["", "nonsense", "custom:", "custom:zzz", "custom:+"] {
            assert_eq!(StaffSet::from_key(junk), StaffSet::Grand, "{junk:?}");
        }
    }

    /// The cycle visits every preset and comes back, and never lands on a
    /// custom set — which has no place in a cycle because nothing could put it
    /// back together afterwards.
    #[test]
    fn the_clef_cycle_visits_everything_once_and_returns() {
        let mut seen = vec![StaffSet::Grand];
        let mut cur = StaffSet::Grand;
        for _ in 0..Clef::ALL.len() {
            cur = cur.next();
            assert!(!seen.contains(&cur), "the cycle repeated at {}", cur.key());
            seen.push(cur.clone());
        }
        assert_eq!(cur.next(), StaffSet::Grand, "the cycle does not come home");
        assert_eq!(seen.len(), Clef::ALL.len() + 1);
        assert_eq!(
            StaffSet::Custom(vec![Clef::Alto]).next(),
            StaffSet::Grand,
            "a custom set should step off to the grand staff"
        );
    }

    /// **The band is as tall as what is in it, and a grand staff is tighter
    /// than a score.**
    ///
    /// The two staves of a grand staff share the region between them — middle
    /// C's ledger line lives there — so they sit four spaces apart. Two
    /// independent clefs cannot share anything: each shows every note, so each
    /// needs its own ledger room and the gap is wider. Getting this backwards
    /// produces a grand staff with a chasm down the middle, which is what the
    /// first version drew.
    #[test]
    fn the_band_is_tall_enough_for_what_is_on_it() {
        let empty = StaffSet::Custom(Vec::new());
        for w in [500.0_f32, 900.0, 1300.0, 2600.0] {
            assert_eq!(band_height(w, &empty), 0.0, "an empty set has height");

            let one = band_height(w, &StaffSet::Single(Clef::Treble));
            let grand = band_height(w, &StaffSet::Grand);
            let pair = band_height(w, &StaffSet::Custom(vec![Clef::Treble, Clef::Bass]));
            let three =
                band_height(w, &StaffSet::Custom(vec![Clef::Treble, Clef::Alto, Clef::Bass]));

            assert!(one > 0.0, "a staff with no height at {w}");
            assert!(grand > one, "a grand staff is not taller than one staff");
            assert!(
                pair > grand,
                "two independent staves are not more spread out than a grand staff at {w}"
            );
            assert!(three > pair, "a third staff added no height at {w}");

            // Every staff has room for its ledger lines and its octave sign:
            // the whole point of `OUTER_SPACES`.
            let space = one / total_spaces(1, false);
            let reach = (8 + OCTAVE_AT * 2 + 3) as f32 * 0.5 - STAFF_SPACES;
            assert!(
                OUTER_SPACES >= reach,
                "a note at the top of its range and its 8va need {reach} spaces, not {OUTER_SPACES}"
            );
            assert!(space > 0.0);
        }
    }
}
