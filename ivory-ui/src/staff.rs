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
/// past that. **Plus the sign's own HEIGHT**: it is centre-anchored text, so
/// budgeting only for where its baseline sits paints the top half of every
/// `8va` outside the rect the band was measured for — over whatever is above
/// it, which in the theory band is the panel's own title.
const OUTER_SPACES: f32 = 4.5;

/// How far the octave sign sits beyond the notes it covers, in half-spaces.
///
/// Two, not three. It is the difference between a staff whose visible lines
/// are half its band and one where they are more than half: the reserve above
/// the top staff is the sign's distance plus its glyph, and every half-space
/// of it is height the music does not get.
const OCTAVE_SIGN_GAP: i32 = 2;

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

/// How wide a staff needs to be, in staff spaces.
///
/// **Content, not the cell.** The clef, then the key signature — which is why
/// this takes the key and is not a constant: seven flats are five spaces of
/// accidentals that have to go somewhere — then the gap before the music, the
/// widest chord this band draws, and a short tail so the last notehead is not
/// against the end bar.
///
/// The chord allowance is three notehead columns plus three accidental
/// columns, which is the worst case `Column::place` can produce for a hand-sized
/// chord: a chromatic cluster with an accidental on every note.
fn staff_w_spaces(key: i32) -> f32 {
    let clef = 3.6;
    let n = key.unsigned_abs().min(MAX_KEY as u32) as f32;
    let sig = if n == 0.0 {
        0.0
    } else {
        n * if key > 0 { 0.95 } else { 0.78 } + 0.95
    };
    let chord = 3.0 * 1.05 + 3.0 * OFFSET_STEP + 1.5;
    clef + sig + 1.6 + chord + 2.5
}

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

// -- key signatures ---------------------------------------------------------

/// The natural pitch class of each letter, C first.
const NATURAL_PC: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// The order sharps are added to a signature, as letter indices: F C G D A E B.
const SHARP_ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];

/// The order flats are added: B E A D G C F -- the sharps' order reversed,
/// which is not a coincidence and is the whole reason one table generates both.
const FLAT_ORDER: [usize; 7] = [6, 2, 5, 1, 4, 0, 3];

/// The furthest a key signature goes in either direction.
///
/// **A signature is one integer, not a tonic.** It says nothing about major or
/// minor -- one sharp is G major and E minor and they are the same signature --
/// so storing a tonic would be storing a fact this app cannot know and has no
/// use for. The names below are for the menu; the number is the music.
pub const MAX_KEY: i32 = 7;

/// What each signature is called, from seven flats to seven sharps.
pub fn key_label(key: i32) -> &'static str {
    const NAMES: [&str; 15] = [
        "Cb / Abm  (7b)",
        "Gb / Ebm  (6b)",
        "Db / Bbm  (5b)",
        "Ab / Fm  (4b)",
        "Eb / Cm  (3b)",
        "Bb / Gm  (2b)",
        "F / Dm  (1b)",
        "C / Am",
        "G / Em  (1#)",
        "D / Bm  (2#)",
        "A / F#m  (3#)",
        "E / C#m  (4#)",
        "B / G#m  (5#)",
        "F# / D#m  (6#)",
        "C# / A#m  (7#)",
    ];
    NAMES[(key.clamp(-MAX_KEY, MAX_KEY) + MAX_KEY) as usize]
}

/// What the signature does to each letter: -1, 0 or +1.
fn key_alterations(key: i32) -> [i8; 7] {
    let mut out = [0_i8; 7];
    let n = (key.unsigned_abs().min(MAX_KEY as u32)) as usize;
    if key > 0 {
        for &l in SHARP_ORDER.iter().take(n) {
            out[l] = 1;
        }
    } else {
        for &l in FLAT_ORDER.iter().take(n) {
            out[l] = -1;
        }
    }
    out
}

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
    /// How far the letter is bent to reach this pitch: the note's actual
    /// alteration, which by itself decides nothing about what is drawn.
    pub alter: i8,
    /// **What is PRINTED in front of the note**, which is a different question.
    /// `None` when the key signature already says it -- that is what a key
    /// signature is for -- and `Some(0)` for a natural, the sign that exists
    /// precisely to contradict one.
    pub printed: Option<i8>,
    pub letter: &'static str,
}

/// Spell a MIDI note in a key.
///
/// **The signature does two jobs here and they are easy to confuse.** It
/// chooses the SPELLING -- in E flat major a black key is A flat and never G
/// sharp -- and it decides what is PRINTED, because a note the signature
/// already alters needs no accidental in front of it, and a note the signature
/// alters and this one does not needs a natural. Both fall out of one
/// comparison: the alteration the letter needs, against the alteration the
/// signature gives it.
///
/// `prefer_flats` decides the five black keys when there is no signature to
/// decide them, and nothing else.
pub fn spell_in_key(midi: u8, key: i32, prefer_flats: bool) -> Spelled {
    let pc = i32::from(midi % 12);
    let alters = key_alterations(key);
    // Flats when the signature has flats, sharps when it has sharps, and the
    // user's own preference only when the signature is silent.
    let use_flats = if key == 0 { prefer_flats } else { key < 0 };

    let mut best = (0_usize, 0_i32);
    let mut best_score = i32::MAX;
    for letter in 0..7_usize {
        // The alteration this letter would need, brought into -6..=5 so that
        // "B sharp for C" scores as +1 rather than as -11.
        let alter = (pc - NATURAL_PC[letter] + 6).rem_euclid(12) - 6;
        if alter.abs() > 1 {
            continue;
        }
        let printed = alter != i32::from(alters[letter]);
        let wrong_way = printed && alter != 0 && ((alter > 0) == use_flats);
        // Printing nothing is best; then going the way the key goes; then the
        // smaller alteration. A natural counts as an alteration of zero, which
        // is why it beats a sharp in a key full of flats.
        let score = i32::from(printed) * 4 + i32::from(wrong_way) * 2 + alter.abs();
        if score < best_score {
            best_score = score;
            best = (letter, alter);
        }
    }
    let (letter, alter) = best;
    let printed = (alter != i32::from(alters[letter])).then_some(alter as i8);
    // MIDI 60 is C4, so the octave is `midi / 12 - 1` -- and a B sharp below
    // middle C belongs to the octave BELOW it, which is what taking the
    // alteration off the pitch first corrects for.
    // `div_euclid`, not `/`: Rust truncates toward zero, so the one case where
    // this goes negative -- MIDI 0 spelled as a B sharp, which the sharpest
    // keys really do produce -- would round the wrong way and engrave the note
    // a full octave above where it belongs.
    let octave = (i32::from(midi) - alter).div_euclid(12) - 1;
    Spelled {
        midi,
        step: octave * 7 + letter as i32,
        alter: alter as i8,
        printed,
        letter: LETTERS[letter],
    }
}

/// Spell with no key signature at all.
pub fn spell(midi: u8, prefer_flats: bool) -> Spelled {
    spell_in_key(midi, 0, prefer_flats)
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
        // The middle line is position 4, and one LINE is two positions, so the
        // distance from the reference is `4 - 2 * ref_line` steps -- not
        // `2 - ref_line`, which happens to be right for the alto clef and
        // wrong, in both directions, for the other five.
        ref_step + 4 - 2 * ref_line
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

/// A notehead's horizontal semi-axis, in staff spaces. One number, because the
/// ellipse, its outline, and the offset that keeps two seconds apart all have
/// to agree on how wide a head IS — three copies of 0.72 is how the offset
/// ends up measured against a head that has quietly grown.
const HEAD_RX: f32 = 0.72;

/// How far an offset notehead sits to the right of the column, in staff
/// spaces.
///
/// **More than two semi-axes, or the heads overlap.** Engraving convention
/// lets the two heads of a second touch; the owner's requirement is stricter —
/// never — so the step is a full head width plus a sliver of daylight. The
/// test `seconds_are_offset_and_larger_intervals_are_not` holds this
/// inequality so the daylight cannot be optimised away.
/// **The outline counts.** Each head wears a hairline of 0.07 spaces centred
/// on its edge, so the visible width is `2*(HEAD_RX + 0.035)`; a step sized to
/// the bare ellipse left a third of a pixel of daylight, which at screen size
/// reads as touching.
const OFFSET_STEP: f32 = HEAD_RX * 2.0 + 0.26;

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
    /// Which notehead column this note sits in: 0 against the stem line, 1
    /// one step right, and so on. Almost always 0 or 1 — it takes a chromatic
    /// cluster to need a third.
    col: u8,
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
    /// Place `notes` on `clef`: move the WHOLE column under an octave sign if
    /// it needs one, then offset the seconds.
    ///
    /// **The shift is one decision for the whole chord, and it has to be.** The
    /// first version decided it per note, which is wrong in a way that reads as
    /// plausible until you actually read the staff: given E6, G6 and C7 in the
    /// treble clef the E stays put and the other two come down an octave, so
    /// the printed chord is G, C, E from the bottom -- the lowest sounding note
    /// printed highest, under a bracket claiming the whole thing is up an
    /// octave. Two notes an octave apart could land on the same line and print
    /// as one. An `8va` applies to what is under it; anything else is not
    /// notation.
    fn place(clef: Clef, notes: &[Spelled]) -> Column {
        if notes.is_empty() {
            return Column::default();
        }
        let raw: Vec<i32> = notes.iter().map(|n| clef.position(n.step)).collect();
        let octave = Self::shift_for(&raw);
        let mut placed: Vec<Placed> = notes
            .iter()
            .zip(&raw)
            .map(|(n, pos)| Placed {
                note: *n,
                pos: pos - octave * 7,
                octave,
                col: 0,
                acc_col: 0,
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
        // **Noteheads a second — or a unison — apart cannot share a column.**
        // Two heads one step apart overlap by most of their width, and an
        // augmented unison puts two different pitches on the SAME line; either
        // way, one of the two is not on the page.
        //
        // Columns, not a boolean, and the difference is a chromatic cluster:
        // in {C, Db, D} every pair clashes, so alternating sides puts the D
        // back in the C's column a second above it — which is precisely the
        // overlap the alternation was for. Greedy colouring on the sorted
        // notes gives each one the leftmost column where nothing sits within
        // a step of it, which is minimal and is what an engraver does.
        for i in 0..placed.len() {
            let mut col = 0_u8;
            'find: loop {
                for j in 0..i {
                    if placed[j].col == col && (placed[i].pos - placed[j].pos).abs() <= 1 {
                        col += 1;
                        continue 'find;
                    }
                }
                break;
            }
            placed[i].col = col;
        }

        // **Accidentals stack into columns, from the top down.** Four flats in
        // one column is four flats on top of each other: a flat is two spaces
        // tall and the notes of a seventh chord are one space apart. An
        // engraver works down from the highest note, putting each accidental in
        // the rightmost column that has room for it, which is what this is.
        let mut columns: Vec<Vec<i32>> = Vec::new();
        for i in (0..placed.len()).rev() {
            if placed[i].note.printed.is_none() {
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

    /// How many octaves the whole column moves, if any.
    ///
    /// **All of it or none of it.** The shift is chosen so that every note in
    /// the chord still fits after it; if no shift does -- a chord genuinely
    /// spanning more than the staff plus its ledger lines can hold -- the
    /// answer is none, and the reader gets ledger lines, which is what an
    /// engraver would give them too. Ledger lines are honest; a bracket over a
    /// chord that is only half transposed is not.
    fn shift_for(positions: &[i32]) -> i32 {
        let (lo, hi) = match (positions.iter().min(), positions.iter().max()) {
            (Some(lo), Some(hi)) => (*lo, *hi),
            _ => return 0,
        };
        let top = 8 + OCTAVE_AT * 2;
        let bottom = -(OCTAVE_AT * 2);
        if hi <= top && lo >= bottom {
            return 0;
        }
        // Two octaves at most: past 15ma an engraver changes clef, and so
        // should the user. Nearest first, so a chord that only just leaves the
        // range gets 8va rather than 15ma.
        for k in [1, -1, 2, -2] {
            if hi - k * 7 <= top && lo - k * 7 >= bottom {
                return k;
            }
        }
        0
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
    /// The alternates and other second-rank text — the same values the theory
    /// band uses for its legends, because this panel lives inside that band.
    faint: Color32,
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
            faint: Color32::from_rgb(0x9a, 0x92, 0x80),
            lit: s.white_key_active_color.to_color32(),
            lit_text: Color32::BLACK,
        }
    } else {
        Palette {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            ink: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            faint: Color32::from_rgb(0x6b, 0x60, 0x4a),
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

/// The chord readout the staff carries: what the detector calls the notes, and
/// the runners-up it nearly called them.
///
/// **This panel is the chord view now.** The chord strip band auto-hides while
/// the staff is on screen — one name in two places is two places for them to
/// disagree — so what used to be a band of its own is a strip at the top of
/// this panel, with up to two next-best readings under the winner. The
/// alternates are the teaching feature: a chord that is C6 and Am7 at once is
/// a fact about harmony, and the strip could never say it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Readout<'a> {
    /// The winning name, or nothing while nothing is sounding.
    pub chord: Option<&'a str>,
    /// The next-best distinct names, best first. At most two are drawn.
    pub alternates: &'a [String],
}

/// The readout strip's height as a fraction of the panel, and its bounds.
///
/// **Reserved whenever a readout is passed at all**, sounding or silent: a
/// strip that appeared on the first chord would drop the staves a quarter of
/// an inch mid-performance, and a staff that jumps when you play is worse than
/// any amount of idle headroom.
const READOUT_FRACTION: f32 = 0.20;

/// The name's box beside the staff, and the gap before it, in staff spaces.
///
/// In spaces, so the name is proportionate to the music. Sized to the gutter
/// instead, it grew to a hundred points beside a staff a fifth of that: a
/// chord symbol is a label on the music, not a headline over it.
const NAME_BOX_SPACES: f32 = 13.0;
const NAME_GAP_SPACES: f32 = 1.5;

/// The tallest the winning name may be drawn, in staff spaces.
const NAME_CAP_SPACES: f32 = 2.6;


/// Draw the band. `readout` is `None` when chord detection is off entirely —
/// only then does the staff take the whole rect.
pub fn draw(
    painter: &Painter,
    rect: Rect,
    notes: &HashSet<u8>,
    readout: Option<Readout<'_>>,
    s: &Settings,
) {
    if !rect.is_positive() {
        return;
    }
    let p = palette(s);
    painter.rect_filled(rect, 0.0, p.bg);

    // **Where the readout goes depends on the shape of the panel.**
    //
    // A grand staff needs twenty-two units of height and seventeen of width,
    // so it is a TALL object: in a wide, short panel it is limited by the
    // height and can never fill the width however it is placed. Stacking the
    // readout on top of it there takes a quarter of the only dimension that
    // was scarce and leaves the width, which was abundant, still empty.
    //
    // So in a wide panel the readout moves into the gutter beside the staff —
    // where there is room for it to be genuinely large — and the staff keeps
    // the full height and is centred in the whole width. In a narrow one, a
    // cell of the four-panel band, there is no gutter and it stacks as before.
    let wide = rect.width() > rect.height() * 2.0;
    // **The staff is centred in the whole panel either way, and never moves.**
    // Giving the readout its own column pushed the staff into the right-hand
    // share, which looks deliberate while a chord is sounding and looks broken
    // the rest of the time: an empty column and a staff shoved off centre is
    // what the app shows for every second nobody is playing. The name goes in
    // the gutter the staff leaves, and appears and disappears there without
    // moving anything.
    let staff_rect = match readout {
        Some(_) if wide => rect,
        Some(_) => {
            let h = (rect.height() * READOUT_FRACTION).clamp(24.0, 64.0);
            Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + h), rect.max)
        }
        None => rect,
    };
    if !staff_rect.is_positive() {
        return;
    }

    let set = s.staff_set();
    let clefs = set.clefs();
    if clefs.is_empty() {
        return;
    }
    let mut sorted: Vec<u8> = notes.iter().copied().collect();
    sorted.sort_unstable();
    let spelled: Vec<Spelled> = sorted
        .iter()
        .map(|n| spell_in_key(*n, s.staff_key, s.prefer_flats))
        .collect();
    let per_staff = distribute(&set, &spelled);

    let spaces = total_spaces(clefs.len(), set.splits());
    // **The staff is as wide as what is ON it, and centred.** It used to run
    // the full width of its cell, which put the chord in the first fifth and
    // left four fifths of ruled emptiness after it — a page waiting for music
    // that is never coming, since this band shows one chord at a time.
    //
    // Sized in SPACES, so the whole drawing scales as one thing: the width
    // needed for the clef, the key signature (which is why this is not a
    // constant), the chord, and a short tail after it. See `staff_w_spaces`.
    let w_spaces = staff_w_spaces(s.staff_key);
    // Whichever budget runs out first. The height decides in a wide cell; the
    // WIDTH decides in a narrow one, and that is what stops a seven-flat
    // signature being engraved over its own clef in a cell too small for it.
    let space = (staff_rect.height() / spaces)
        .min(staff_rect.width() / w_spaces)
        .max(1.0);
    let gap = if set.splits() { GRAND_GAP } else { SCORE_GAP };
    // Centred horizontally in whatever is left over, and vertically too — with
    // the width capped, a tall cell would otherwise hang the staves from the
    // top edge with the slack all underneath.
    let used_w = space * w_spaces;
    let used_h = space * spaces;
    // **In a wide panel the staff and the name are one block, centred.** The
    // name sits to the RIGHT of the music, in a box reserved whether or not
    // anything is sounding, so the staff does not shuffle sideways every time
    // a chord starts. The box is sized in staff spaces like everything else,
    // which is what keeps the name proportionate to the music rather than
    // swollen to fill whatever gutter happened to be left over.
    let name_box = if wide && readout.is_some() {
        space * (NAME_BOX_SPACES + NAME_GAP_SPACES)
    } else {
        0.0
    };
    let left = staff_rect.left() + (staff_rect.width() - used_w - name_box) * 0.5;
    let top = staff_rect.top() + (staff_rect.height() - used_h) * 0.5;
    // A margin that grows with the staff rather than with the window, so the
    // clef sits the same distance from the edge at every size.
    let margin = space * 1.1;
    // The bottom line of each staff, top to bottom. One list, so the brace and
    // the staves cannot end up in different places.
    let bottoms: Vec<f32> = (0..clefs.len())
        .map(|i| {
            top + space * (OUTER_SPACES + STAFF_SPACES + i as f32 * (STAFF_SPACES + gap))
        })
        .collect();

    if let Some(r) = readout {
        let strip = if wide {
            Rect::from_min_max(
                Pos2::new(left + used_w + space * NAME_GAP_SPACES, rect.top()),
                Pos2::new(
                    left + used_w + space * (NAME_GAP_SPACES + NAME_BOX_SPACES),
                    rect.bottom(),
                ),
            )
        } else {
            Rect::from_min_max(
                rect.min,
                Pos2::new(rect.right(), staff_rect.top()),
            )
        };
        // Never taller than a few staff spaces. The name is a label on the
        // music; letting it fill its box put a hundred-point chord symbol
        // beside a staff a fifth of that height.
        draw_readout(painter, strip, r, space * NAME_CAP_SPACES, &p);
    }

    for (i, clef) in clefs.iter().enumerate() {
        let g = Geometry {
            space,
            bottom: bottoms[i],
            left: left + margin,
            right: left + used_w - margin,
        };
        draw_staff(painter, g, *clef, &per_staff[i], s, &p);
    }

    // The brace and the end bars that make two staves one instrument. Only for
    // the grand staff: a stack of independent clefs is a score, and joining a
    // violist to a cellist with a piano brace would be saying something untrue.
    if set.splits() && clefs.len() == 2 {
        let brace_top = bottoms[0] - space * STAFF_SPACES;
        let bottom = bottoms[1];
        let left = left + margin;
        let right = left + used_w - margin * 2.0;
        draw_brace(painter, left - space * 1.1, brace_top, bottom, space, &p);
        for x in [left, right] {
            painter.line_segment(
                [Pos2::new(x, brace_top), Pos2::new(x, bottom)],
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

/// The winner, large, and up to two runners-up under it.
///
/// **Shape-agnostic**, because it is handed two very different rectangles: a
/// wide short strip above the staff in a narrow panel, and a tall narrow
/// gutter beside it in a wide one. So it sizes the type to whichever of the
/// two dimensions runs out first and centres the block, rather than placing
/// each line at a fraction of the height — which spreads two lines to opposite
/// ends of a tall gutter.
fn draw_readout(painter: &Painter, strip: Rect, r: Readout<'_>, cap: f32, p: &Palette) {
    if !strip.is_positive() {
        return;
    }
    let Some(chord) = r.chord else {
        return;
    };
    let names: Vec<&str> = r.alternates.iter().take(2).map(String::as_str).collect();
    let alts = (!names.is_empty()).then(|| format!("or  {}", names.join("   ")));

    // 0.62 is the bundled monospace advance, the same number every other band
    // uses to size text to a box without laying it out first.
    let fit = |text: &str, w: f32| w * 0.94 / (text.chars().count().max(1) as f32 * 0.62);
    // The two lines together must fit the height: the alternates are just over
    // half the main size, and there is a gap between them.
    let stacked = if alts.is_some() { 1.9 } else { 1.15 };
    let main = fit(chord, strip.width())
        .min(alts.as_deref().map_or(f32::MAX, |a| fit(a, strip.width()) / 0.52))
        .min(strip.height() / stacked)
        .min(cap);
    if main < 5.0 {
        return;
    }
    let alt_size = main * 0.52;
    let block = if alts.is_some() { main + alt_size * 1.5 } else { main };
    let top = strip.center().y - block * 0.5;
    painter.text(
        Pos2::new(strip.center().x, top + main * 0.5),
        Align2::CENTER_CENTER,
        chord,
        FontId::new(main, fonts::courier_bold()),
        p.ink,
    );
    if let Some(a) = alts {
        // "or", because that is what an alternate reading IS: not a second
        // answer, a second way of hearing the first one.
        painter.text(
            Pos2::new(strip.center().x, top + main + alt_size * 0.75),
            Align2::CENTER_CENTER,
            &a,
            FontId::new(alt_size, fonts::courier()),
            p.faint,
        );
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
    let after_clef = draw_key_signature(
        painter,
        g,
        clef,
        s.staff_key,
        draw_clef(painter, g, clef, p),
        p,
    );

    let column = Column::place(clef, notes);
    if column.notes.is_empty() {
        return;
    }
    // The chord sits a little right of the clef rather than in the middle of
    // the staff: a reader's eye starts at the clef, and a chord adrift in the
    // centre of an otherwise empty system looks like a mistake.
    let head_w = g.space * 1.18;
    // **Bounded at BOTH ends.** The `min` keeps the chord off the right edge;
    // without the `max` a staff too narrow to hold clef, signature and chord
    // silently pulled the notes back to the LEFT of the clef and engraved the
    // music on top of it. Running off the right is a layout that ran out of
    // room; printing over the clef is a layout that lied about having any.
    let x = (after_clef + g.space * 3.0)
        .min(g.right - head_w * 2.5)
        .max(after_clef + g.space * 1.6);
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
        let cx = x + g.space * OFFSET_STEP * f32::from(placed.col);

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

        if let Some(sign) = placed.note.printed {
            draw_accidental(
                painter,
                Pos2::new(x - g.space * 1.15 - placed.acc_col as f32 * acc_step, y),
                g.space,
                sign,
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
    let outer = ellipse(c, space * HEAD_RX, space * 0.5, -0.35);
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
    let edge = ellipse(c, space * HEAD_RX, space * 0.5, -0.35);
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
            group.iter().map(|n| n.pos).max().unwrap_or(8).max(8) + OCTAVE_SIGN_GAP
        } else {
            group.iter().map(|n| n.pos).min().unwrap_or(0).min(0) - OCTAVE_SIGN_GAP
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
    if alter == 0 {
        // A natural: two uprights that do not meet, joined by two thick bars.
        // The gaps are the point -- a natural whose stems ran the whole way
        // would be a sharp with the slant taken out of it.
        painter.line_segment(
            [
                Pos2::new(c.x - space * 0.22, c.y - space * 0.9),
                Pos2::new(c.x - space * 0.22, c.y + space * 0.42),
            ],
            thin,
        );
        painter.line_segment(
            [
                Pos2::new(c.x + space * 0.22, c.y - space * 0.42),
                Pos2::new(c.x + space * 0.22, c.y + space * 0.9),
            ],
            thin,
        );
        for dy in [-0.34_f32, 0.2] {
            painter.line_segment(
                [
                    Pos2::new(c.x - space * 0.24, c.y + (dy + 0.1) * space),
                    Pos2::new(c.x + space * 0.24, c.y + (dy - 0.06) * space),
                ],
                thick,
            );
        }
        return;
    }
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

/// Where the accidentals of a key signature go, per clef family.
///
/// **Tabled rather than derived**, and that is deliberate. There IS a rule --
/// each accidental is a fourth up or a fifth down from the last, whichever
/// keeps it inside the clef's own band -- but the band is a different height
/// for every clef and the tenor clef breaks the pattern outright, putting its
/// first sharp at the BOTTOM of the staff so that the rest have somewhere to
/// go. A table states the four answers; the test below proves every entry
/// spells the letter it is supposed to, which is the part a typo would break.
fn key_positions(clef: Clef, sharps: bool) -> [i32; 7] {
    match (clef, sharps) {
        (Clef::Treble | Clef::Treble8vb, true) => [8, 5, 9, 6, 3, 7, 4],
        (Clef::Treble | Clef::Treble8vb, false) => [4, 7, 3, 6, 2, 5, 1],
        (Clef::Bass | Clef::Bass8vb, true) => [6, 3, 7, 4, 1, 5, 2],
        (Clef::Bass | Clef::Bass8vb, false) => [2, 5, 1, 4, 0, 3, -1],
        (Clef::Alto, true) => [7, 4, 8, 5, 2, 6, 3],
        (Clef::Alto, false) => [3, 6, 2, 5, 1, 4, 0],
        // **The tenor exception, and only where it really applies.** F sharp
        // and G sharp drop an octave because at their ordinary height they
        // would need ledger lines; E sharp does NOT -- it sits on the top line
        // like everywhere else. Dropping it too, as the first version did,
        // printed a signature that falls a seventh and jumps back up a fifth,
        // which no engraver has ever printed.
        (Clef::Tenor, true) => [2, 6, 3, 7, 4, 8, 5],
        // And the FLATS are not irregular at all. Flats descend, so starting
        // from B flat on the third space nothing ever runs off the top; the
        // first version applied the sharps' exception to them and put three of
        // the seven an octave low, with the C flat below the staff entirely.
        (Clef::Tenor, false) => [5, 8, 4, 7, 3, 6, 2],
    }
}

/// The key signature, after the clef. Returns the x it reached.
fn draw_key_signature(painter: &Painter, g: Geometry, clef: Clef, key: i32, x: f32, p: &Palette) -> f32 {
    let n = (key.unsigned_abs().min(MAX_KEY as u32)) as usize;
    if n == 0 {
        return x;
    }
    let sharps = key > 0;
    let positions = key_positions(clef, sharps);
    // Sharps are wider than flats and need more room between them, which is
    // why a seven-sharp signature is visibly longer than a seven-flat one on
    // real paper too.
    let step = g.space * if sharps { 0.95 } else { 0.78 };
    // Clear of the clef before the first one. A signature that touches the
    // clef reads as part of it, and the first thing a reader does with a staff
    // is separate those two.
    let from = x + g.space * 0.45;
    for (i, pos) in positions.iter().take(n).enumerate() {
        draw_accidental(
            painter,
            Pos2::new(from + step * (i as f32 + 0.5), g.y(*pos)),
            g.space,
            if sharps { 1 } else { -1 },
            p.ink,
        );
    }
    from + step * n as f32 + g.space * 0.5
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// **A key signature decides both the spelling and what is printed.**
    ///
    /// Two jobs, one comparison, and the failures look completely different: a
    /// spelling mistake puts the note on the wrong line, and a printing mistake
    /// puts an accidental in front of a note that the signature already covers
    /// — which is not wrong so much as illiterate, and is the thing a musician
    /// notices in the first half-second.
    #[test]
    fn a_key_signature_spells_the_notes_and_silences_the_accidentals() {
        // E flat major: three flats. B flat, E flat and A flat are in the key,
        // so they are spelled with those letters and printed bare.
        for (midi, letter) in [(63_u8, "E"), (70, "B"), (68, "A")] {
            let sp = spell_in_key(midi, -3, false);
            assert_eq!(sp.letter, letter, "{midi} is spelled wrong in E flat");
            assert_eq!(sp.alter, -1);
            assert_eq!(
                sp.printed, None,
                "{midi} printed an accidental the signature already gives it"
            );
        }
        // And the naturals of that key print nothing either.
        for midi in [60_u8, 62, 65, 67] {
            assert_eq!(spell_in_key(midi, -3, false).printed, None);
        }
        // E natural in E flat major is a note OUTSIDE the key, and the sign
        // that says so is a natural — not a sharp on D, and not nothing.
        let e = spell_in_key(64, -3, false);
        assert_eq!(e.letter, "E");
        assert_eq!(e.alter, 0);
        assert_eq!(e.printed, Some(0), "E natural in E flat needs a natural");

        // A major: three sharps. F sharp is bare; F natural needs a natural.
        assert_eq!(spell_in_key(66, 3, false).printed, None);
        let f = spell_in_key(65, 3, false);
        assert_eq!(f.letter, "F");
        assert_eq!(f.printed, Some(0));

        // With no signature at all, nothing changes from what the band did
        // before key signatures existed: naturals bare, black keys spelled by
        // the user's own preference.
        assert_eq!(spell_in_key(60, 0, false).printed, None);
        assert_eq!(spell_in_key(61, 0, false).letter, "C");
        assert_eq!(spell_in_key(61, 0, false).printed, Some(1));
        assert_eq!(spell_in_key(61, 0, true).letter, "D");
        assert_eq!(spell_in_key(61, 0, true).printed, Some(-1));
    }

    /// **Every pitch is spellable in every key, on the right line.**
    ///
    /// The sweep that a hand-written table cannot replace: 128 notes times
    /// fifteen signatures, checking that what comes back really is the pitch
    /// that went in and that the step it lands on agrees with the letter it
    /// claims. A spelling that is off by an octave — the B-sharp-below-middle-C
    /// case — passes every spot check and fails this.
    #[test]
    fn every_note_in_every_key_lands_on_the_line_its_letter_names() {
        for key in -MAX_KEY..=MAX_KEY {
            for midi in 0..=127_u8 {
                let sp = spell_in_key(midi, key, false);
                // The letter and the alteration have to add back up to the
                // pitch that was asked for.
                let letter = LETTERS
                    .iter()
                    .position(|l| *l == sp.letter)
                    .expect("a letter this module knows");
                let pc = (NATURAL_PC[letter] + i32::from(sp.alter)).rem_euclid(12);
                assert_eq!(
                    pc,
                    i32::from(midi % 12),
                    "{midi} in key {key} came back as {}{}",
                    sp.letter,
                    sp.alter
                );
                // And the step it sits on has to be that letter, in the octave
                // the pitch really belongs to.
                assert_eq!(
                    sp.step.rem_euclid(7),
                    letter as i32,
                    "{midi} in key {key} is spelled {} and sits on a {} line",
                    sp.letter,
                    LETTERS[sp.step.rem_euclid(7) as usize]
                );
                assert!(sp.alter.abs() <= 1, "{midi} in key {key} needs a double");
            }
        }
    }

    /// **Every accidental in a key signature is the letter it is supposed to
    /// be, on every clef.**
    ///
    /// `key_positions` is a table, and a table is exactly the thing a typo
    /// hides in: one wrong number draws a G sharp where an F sharp goes, which
    /// is a signature nobody can read and which looks, at a glance, completely
    /// normal.
    #[test]
    fn every_key_signature_accidental_lands_on_its_own_letter() {
        for clef in Clef::ALL {
            for sharps in [true, false] {
                let order = if sharps { SHARP_ORDER } else { FLAT_ORDER };
                for (i, pos) in key_positions(clef, sharps).iter().enumerate() {
                    // Undo `Clef::position` to get back to a diatonic step,
                    // then to a letter.
                    let (ref_step, ref_line) = clef.reference();
                    let step = pos - ref_line * 2 + ref_step;
                    assert_eq!(
                        step.rem_euclid(7) as usize,
                        order[i],
                        "{clef:?} {} number {} is on a {} and should be a {}",
                        if sharps { "sharp" } else { "flat" },
                        i + 1,
                        LETTERS[step.rem_euclid(7) as usize],
                        LETTERS[order[i]]
                    );
                    // And it has to be ON the staff, or near enough: a
                    // signature that needs ledger lines is one nobody prints.
                    assert!(
                        (-1..=9).contains(pos),
                        "{clef:?} {} number {} is at {pos}, off the staff",
                        if sharps { "sharp" } else { "flat" },
                        i + 1
                    );
                    // **And it is as HIGH as that clef allows.** This is the
                    // assertion the letter check cannot make and the one that
                    // matters: an accidental put an octave below where it
                    // belongs is still the right letter, still on the staff,
                    // and still completely wrong. Both tenor tables shipped
                    // broken past a test that checked only the letter.
                    //
                    // The ceilings are the top of each clef's own signature
                    // band. They are stated here rather than read from the
                    // table, so a typo in the table cannot also be a typo in
                    // what the table is checked against.
                    let ceiling = match (clef, sharps) {
                        (Clef::Treble | Clef::Treble8vb, true) => 9,
                        (Clef::Treble | Clef::Treble8vb, false) => 7,
                        (Clef::Bass | Clef::Bass8vb, true) => 7,
                        (Clef::Bass | Clef::Bass8vb, false) => 5,
                        (Clef::Alto, true) => 8,
                        (Clef::Alto, false) => 6,
                        (Clef::Tenor, _) => 8,
                    };
                    assert!(*pos <= ceiling, "{clef:?} number {} is above its band", i + 1);
                    assert!(
                        pos + 7 > ceiling,
                        "{clef:?} {} number {} sits at {pos} when {} is free",
                        if sharps { "sharp" } else { "flat" },
                        i + 1,
                        pos + 7
                    );
                }
            }
        }
    }

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

    /// **No two noteheads may overlap, in any chord whatsoever.**
    ///
    /// The rule is geometric and this asserts the geometry: any two notes in
    /// the same column must be more than a second apart, and the step between
    /// columns must clear a full head width. The boolean version of this rule
    /// shipped a chromatic cluster — {C, Db, D} — with the D "alternated" back
    /// into the C's column, one step above it, overlapping.
    #[test]
    fn seconds_are_offset_and_larger_intervals_are_not() {
        // C D E: the middle note moves right, the outer two stay.
        let notes: Vec<Spelled> = [60_u8, 62, 64].iter().map(|n| spell(*n, false)).collect();
        let col = Column::place(Clef::Treble, &notes);
        let cols: Vec<u8> = col.notes.iter().map(|p| p.col).collect();
        assert_eq!(cols, vec![0, 1, 0], "a cluster did not alternate");

        // **The offset clears the head.** Two semi-axes is where the ellipses
        // touch; anything less is the overlap this rule exists to prevent, and
        // it shipped once at 1.18 spaces against a 1.44-space head.
        assert!(
            OFFSET_STEP > (HEAD_RX + 0.035) * 2.0 + 0.15,
            "an offset second has no daylight: step {OFFSET_STEP} vs outlined width {}",
            (HEAD_RX + 0.035) * 2.0
        );

        // A triad has nothing a second apart in it and stays in one column.
        let triad: Vec<Spelled> = [60_u8, 64, 67].iter().map(|n| spell(*n, false)).collect();
        let col = Column::place(Clef::Treble, &triad);
        assert!(
            col.notes.iter().all(|p| p.col == 0),
            "a plain triad was split across the stem"
        );

        // The case the boolean shipped wrong: a chromatic cluster needs a
        // THIRD column, because its outer notes are themselves a second apart
        // (C and D share a staff position with a sharp between them, or sit
        // one step apart as here with Db between).
        let cluster: Vec<Spelled> = [60_u8, 61, 62].iter().map(|n| spell(*n, true)).collect();
        let col = Column::place(Clef::Treble, &cluster);
        for i in 0..col.notes.len() {
            for j in (i + 1)..col.notes.len() {
                let (a, b) = (col.notes[i], col.notes[j]);
                assert!(
                    a.col != b.col || (a.pos - b.pos).abs() > 1,
                    "{} and {} share column {} only {} apart",
                    a.note.midi,
                    b.note.midi,
                    a.col,
                    (a.pos - b.pos).abs()
                );
            }
        }

        // And exhaustively: every chromatic run up to five notes, at several
        // roots, obeys the rule. Five simultaneous semitones is beyond what a
        // hand plays as a chord; past that the drawing may do what it likes.
        for root in [36_u8, 59, 60, 71] {
            for len in 2..=5_u8 {
                let ns: Vec<Spelled> =
                    (0..len).map(|k| spell(root + k, false)).collect();
                let col = Column::place(Clef::Bass, &ns);
                for i in 0..col.notes.len() {
                    for j in (i + 1)..col.notes.len() {
                        let (a, b) = (col.notes[i], col.notes[j]);
                        assert!(
                            a.col != b.col || (a.pos - b.pos).abs() > 1,
                            "run at {root} len {len}: {} and {} clash",
                            a.note.midi,
                            b.note.midi
                        );
                    }
                }
            }
        }
    }

    /// **The readout goes where there is room for it.**
    ///
    /// A grand staff wants twenty-two units of height and seventeen of width,
    /// so in a wide short panel it is limited by the height and can never fill
    /// the width. Stacking the readout above it there spends a quarter of the
    /// scarce dimension and leaves the abundant one empty; beside it, the name
    /// grows to fill the space that was going to waste and the staff keeps its
    /// full height. This pins which way round that decision goes, because it
    /// is invisible in a passing paint test.
    #[test]
    fn the_readout_sits_beside_the_staff_only_when_there_is_a_gutter_for_it() {
        // A cell of the four-panel band: taller than it is wide, so the
        // readout stacks and the staff gets what is left.
        let narrow = Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 280.0));
        assert!(!(narrow.width() > narrow.height() * 2.0));
        // The solo panel: five times wider than tall, so it splits in two.
        let wide = Rect::from_min_size(Pos2::ZERO, egui::vec2(1250.0, 258.0));
        assert!(wide.width() > wide.height() * 2.0);
        // And the staff is centred in the WHOLE panel either way, so it does
        // not move when a chord starts or stops sounding.
        // Whichever way it lands, the staff has to be able to fit: the width
        // budget must not starve the space below what the height would give.
        for r in [narrow, wide] {
            let space = (r.height() / total_spaces(2, true)).min(r.width() / staff_w_spaces(0));
            assert!(space > 4.0, "{r:?} leaves a {space}pt staff space");
        }
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

    /// **An octave sign covers the WHOLE chord or none of it.**
    ///
    /// Deciding it per note is wrong in a way that looks right until you read
    /// the result: the notes cross each other, two an octave apart can land on
    /// one line, and the bracket claims the untransposed ones are transposed
    /// too. This is the test that pins the property that actually matters --
    /// the printed intervals are the sounding intervals.
    #[test]
    fn an_octave_sign_moves_the_whole_chord_and_never_reorders_it() {
        let mk = |ns: &[u8]| -> Vec<Spelled> { ns.iter().map(|n| spell(*n, false)).collect() };

        // E6 G6 C7 in the treble: the top two are past the limit and the E is
        // not. Per-note shifting printed G, C, E bottom to top.
        let col = Column::place(Clef::Treble, &mk(&[88, 91, 96]));
        let octs: Vec<i32> = col.notes.iter().map(|p| p.octave).collect();
        assert!(
            octs.windows(2).all(|w| w[0] == w[1]),
            "the chord was split across two octave signs: {octs:?}"
        );
        // The printed order is the sounding order, always.
        let by_pos: Vec<u8> = col.notes.iter().map(|p| p.note.midi).collect();
        let mut sorted = by_pos.clone();
        sorted.sort_unstable();
        assert_eq!(by_pos, sorted, "the printed chord is not in pitch order");

        // C6 and C7 together: an octave, and it has to stay an octave.
        let col = Column::place(Clef::Treble, &mk(&[84, 96]));
        assert_eq!(col.notes.len(), 2);
        assert_eq!(
            col.notes[1].pos - col.notes[0].pos,
            7,
            "an octave was printed as a unison"
        );

        // Two pitches must never land on the same spot with the same offset,
        // in any chord, in any clef.
        for clef in Clef::ALL {
            for chord in [
                vec![60_u8, 61],
                vec![84, 96],
                vec![96, 108],
                vec![88, 91, 96],
                vec![24, 36, 48],
                vec![21, 108],
                vec![60, 62, 64, 65, 67],
            ] {
                let col = Column::place(clef, &mk(&chord));
                for i in 0..col.notes.len() {
                    for j in (i + 1)..col.notes.len() {
                        let (a, b) = (col.notes[i], col.notes[j]);
                        assert!(
                            a.pos != b.pos || a.col != b.col,
                            "{clef:?} {chord:?}: {} and {} print in the same place",
                            a.note.midi,
                            b.note.midi
                        );
                    }
                }
                // And nothing ends up outside the room the band reserved.
                for n in &col.notes {
                    assert!(
                        (-(OCTAVE_AT * 2)..=8 + OCTAVE_AT * 2).contains(&n.pos)
                            || Column::shift_for(
                                &chord
                                    .iter()
                                    .map(|m| clef.position(spell(*m, false).step))
                                    .collect::<Vec<_>>()
                            ) == 0,
                        "{clef:?} {chord:?}: {} is at {} , outside the band",
                        n.note.midi,
                        n.pos
                    );
                }
            }
        }
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
            // The sign's LINE, plus half its glyph: it is centre-anchored
            // text at 1.15 spaces, so the top of it reaches another 0.6 spaces
            // past where the dashes are.
            let reach = (8 + OCTAVE_AT * 2 + OCTAVE_SIGN_GAP) as f32 * 0.5 - STAFF_SPACES + 0.5;
            assert!(
                OUTER_SPACES >= reach,
                "a note at the top of its range and its 8va need {reach} spaces, not {OUTER_SPACES}"
            );
            assert!(space > 0.0);
        }
    }
}
