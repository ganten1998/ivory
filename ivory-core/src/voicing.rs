//! Which fretboard position lights up for each held pitch.
//!
//! `fretboard.rs` says where a pitch CAN go. This says where it DOES go, and,
//! through [`VoicingSession`], where it stays from one solve to the next.
//!
//! Pure: no GUI, no I/O, no clock. `ivory-core` contains no `std::time` and
//! keeps it that way, so the caller passes elapsed milliseconds rather than
//! this module reading one.
//!
//! # The shape of the problem
//!
//! Middle C is five places on a standard board and the high E string cannot
//! reach it at all. A four-note chord is hundreds of combinations. Something
//! has to choose, and the choice has to look like what a guitarist would
//! actually play.
//!
//! Two structural decisions make that tractable, and both are load-bearing:
//!
//! 1. **Assignment is monotone**: ascending sounding pitch goes on ascending
//!    strings. This is not a heuristic, it is the search itself — it gives
//!    "at most one note per string" for free, forbids voice crossings by
//!    construction, and collapses the solution space to `C(n + strings,
//!    strings)`, at most 3003 complete shapes for six strings. That is small
//!    enough to enumerate *exhaustively*, so there are no windows, no beam, no
//!    dynamic-programming table and no approximation anywhere below.
//! 2. **Costs are `i32`, never floats.** Float addition is not associative, so
//!    a reordered sum can flip a near-tie, and determinism here is a shipped
//!    contract with tests behind it. [`REACH_MILLI`] is a frozen table rather
//!    than a runtime `powf` for the same reason: a one-ULP libm difference
//!    between two machines must not be able to change which shape ships.
//!
//! Everything else is a number in [`Weights`], gathered in one struct because
//! the numbers are the part most likely to be argued with.
//!
//! # Known limitations, said out loud
//!
//! * **Monotone is structural.** Deliberate crossing voicings (a fretted 9th
//!   under an open B) and thumb-over-the-top bass notes cannot be represented.
//!   Changing that means rewriting the search, which is why it is decided here
//!   rather than discovered later.
//! * **Finger count is modelled; finger assignment is not.** `OneHand` is an
//!   anatomical claim this module has not checked. Partial ring-finger barres
//!   and index-tip mutes are not modelled, so a few real jazz shapes score one
//!   finger too expensive.
//! * **The solver knows nothing about the chord.** Notes are shed by register
//!   and doubling, not by function: a jazz player drops the 5th, we drop an
//!   octave double. [`History::root_pc`] is the hook for changing that and is
//!   deliberately unwired — parsing a display name back out of the detector
//!   would let a Teach override move the dots on the fretboard.
//! * **Every number here was executed against the real candidate tables, and
//!   none of it was measured against a guitarist.** The relative ordering
//!   (drop >> fingers > stretch > position > skip > open) is the part to
//!   defend; the exact integers are the part to lose an argument about.

use crate::fretboard::{candidates, reachable, FretboardSpec, Position, Tuning};
use std::collections::HashSet;

/// Strings past this are reported `Unused`. No shipped tuning has more than 6;
/// this exists to bound the tie key and the search on a hand-written spec.
pub const MAX_STRINGS: usize = 12;

/// Working-list ceiling. The pre-cap keeps at most `strings + note_slack`
/// notes, and this clamps that even if someone sets `note_slack` to 100.
const MAX_NOTES: usize = MAX_STRINGS + 4;

/// Sentinel for "this string sounds nothing" inside the fixed-size search
/// arrays. A real fret is 0..=254, and 255 sorts last in the tie key, so a
/// shape that uses a string beats one that mutes it at equal cost.
const MUTED: u8 = u8::MAX;

/// `round(1000 * 2^(-fret/12))`, indexed by ABSOLUTE fret, clamped at 24.
///
/// The same rule of 18 the geometry is drawn with: a five-fret reach at the
/// 12th fret is physically half the reach it is at the 1st. Frozen as a table
/// rather than computed, so no two machines can disagree by a rounding step.
const REACH_MILLI: [i32; 25] = [
    1000, 944, 891, 841, 794, 749, 707, 667, 630, 595, 561, 530, 500, 472, 445, 420, 397, 375, 354,
    334, 315, 297, 281, 265, 250,
];

// ── output ──────────────────────────────────────────────────────────────────

/// What became of one distinct held pitch.
///
/// Every variant is a different PICTURE, not merely a different `Option` arm.
/// A user looking at the board has to be able to tell "the guitar cannot play
/// this" from "not at the same time as the others" from "the app is broken",
/// and only the first two are things we ever mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// On the board. `sounding == pitch` unless `octave_shift != 0`, and
    /// `pos.folded == (octave_shift != 0)`.
    Placed {
        pos: Position,
        sounding: u8,
        octave_shift: i8,
    },
    /// The board CAN make this note; every position it has sits on a string
    /// another note took. Drawn as hollow rings at `wanted`.
    Conflict { wanted: Vec<Position> },
    /// Reachable, a free string exists, and the solver chose not to show it.
    Dropped { reason: DropReason },
    /// No octave of this pitch class exists on this board at all.
    Unreachable,
}

/// Why a note is not on the board. This is what the status line explains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// Its pitch class is already sounding elsewhere in the shape.
    Doubled,
    /// It folded onto a pitch another note already occupies.
    OctaveMerged,
    /// A free string could carry it, but only by crossing another placed note.
    Ordering,
    /// Cut by the pre-cap, or lost on cost. More notes than strings.
    Excess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotePlacement {
    pub pitch: u8,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringState {
    /// Outside the sounding span. Drawn `x`, costs nothing.
    Unused,
    /// Inside the span and silent: a real string skip the player must damp.
    Skipped,
    Sounding { fret: u8, pitch: u8, folded: bool },
}

/// One finger laid flat across the neck at `fret`, covering
/// `lo_string..=hi_string`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Barre {
    pub fret: u8,
    pub lo_string: usize,
    pub hi_string: usize,
}

/// A label, never a gate. The solver will happily show a shape no hand can
/// hold, because an honest unplayable diagram beats a pretty lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Playability {
    #[default]
    Silent,
    Open,
    OneHand,
    Stretch,
    TwoHands,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shape {
    /// Lowest FRETTED fret: where the hand is. `None` when nothing is fretted.
    pub anchor: Option<u8>,
    /// `max fretted - min fretted`. 0 when nothing is fretted.
    pub span: u8,
    /// After the barre discount. May exceed 4; that is reported, not prevented.
    pub fingers: u8,
    pub barre: Option<Barre>,
    pub playability: Playability,
    pub open_count: u8,
    pub folded_count: u8,
    pub dropped_count: u8,
    pub conflict_count: u8,
    pub unreachable_count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Voicing {
    /// One entry per DISTINCT held pitch, ascending by pitch. Nothing the
    /// player pressed is ever missing from this list, whatever became of it.
    pub notes: Vec<NotePlacement>,
    /// `len() == min(tuning.strings(), MAX_STRINGS)`, indexed by string.
    pub strings: Vec<StringState>,
    pub shape: Shape,
    /// Objective value of the winner. For tests, the probe and a debug
    /// overlay. Never shown to a user.
    pub cost: i32,
    /// Board this was solved for. A `Voicing` whose fingerprint no longer
    /// matches is ignored as hysteresis input, so a forgotten capo change
    /// cannot pin a note to a fret that no longer exists.
    pub spec_fp: u64,
    /// The leaf budget cut the search short. Still deterministic.
    pub truncated: bool,
    /// Carried so [`Voicing::caption`] can explain an impossible board without
    /// the caller handing the spec back in. Two bytes, and it keeps the one
    /// message that matters most self-contained.
    pub capo: u8,
    pub frets: u8,
}

impl Voicing {
    /// Every held pitch that made it onto the board, with where it went.
    pub fn placed(&self) -> impl Iterator<Item = (u8, Position)> + '_ {
        self.notes.iter().filter_map(|n| match n.outcome {
            Outcome::Placed { pos, .. } => Some((n.pitch, pos)),
            _ => None,
        })
    }

    pub fn position_of(&self, pitch: u8) -> Option<Position> {
        self.notes.iter().find_map(|n| match n.outcome {
            Outcome::Placed { pos, .. } if n.pitch == pitch => Some(pos),
            _ => None,
        })
    }

    /// The status line: `"6 of 10 notes  ·  1 an octave up  ·  two hands"`.
    ///
    /// `None` when there is nothing to explain, which is the common case. A
    /// message on an ordinary board reads as an error.
    pub fn caption(&self) -> Option<String> {
        if self.notes.is_empty() {
            return None;
        }
        if self.shape.unreachable_count as usize == self.notes.len() {
            return Some(if self.capo > self.frets {
                format!("capo {} is past fret {}", self.capo, self.frets)
            } else {
                "outside this instrument's range".to_owned()
            });
        }
        let mut parts: Vec<String> = Vec::new();
        // Counted, not derived by subtracting the `Shape` counters: those are
        // `u8` and saturate, so a 300-note set would have made this say "1 of
        // 300 notes" with nothing on the board at all. Counting cannot drift
        // from what is actually drawn.
        let placed = self.placed().count();
        if placed < self.notes.len() {
            parts.push(format!("{placed} of {} notes", self.notes.len()));
        }
        let (up, down) = self.notes.iter().fold((0u32, 0u32), |(u, d), n| match n.outcome {
            Outcome::Placed { octave_shift, .. } if octave_shift > 0 => (u + 1, d),
            Outcome::Placed { octave_shift, .. } if octave_shift < 0 => (u, d + 1),
            _ => (u, d),
        });
        if up > 0 {
            parts.push(format!("{up} an octave up"));
        }
        if down > 0 {
            parts.push(format!("{down} an octave down"));
        }
        match self.shape.playability {
            Playability::Stretch => parts.push("stretch".to_owned()),
            Playability::TwoHands => parts.push("two hands".to_owned()),
            _ => {}
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("  \u{b7}  "))
        }
    }
}

// ── input ───────────────────────────────────────────────────────────────────

/// The previous solve, as a cost bias rather than a constraint. Carried by
/// [`VoicingSession`]; a caller driving [`solve`] directly may leave it empty.
#[derive(Debug, Clone, Copy, Default)]
pub struct History<'a> {
    pub prev: Option<&'a Voicing>,
    /// Arrival ordinal per MIDI pitch; lower means held longer, 0 is unknown.
    /// `None` means "everything arrived at once".
    pub arrival: Option<&'a [u32; 128]>,
    /// Reserved hook to the chord engine. MUST be `None`: see the module doc.
    pub root_pc: Option<u8>,
}

impl History<'_> {
    pub const NONE: History<'static> = History {
        prev: None,
        arrival: None,
        root_pc: None,
    };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SolveStats {
    pub leaves: u32,
    pub pruned: u32,
    pub truncated: bool,
}

// ── the dials ───────────────────────────────────────────────────────────────

/// Every tunable number in the solver, in one struct, because these are the
/// ones the owner will want to turn while play-testing.
///
/// Calibration unit: **one finger = 80 points**. The tiers, from loudest to
/// quietest, are `drop >> fingers > stretch > position > skip > open`. That
/// ordering is the part to defend in an argument; the integers are the part to
/// lose one about.
///
/// Invariant assumed by the search's prune (asserted in the tests): every term
/// except `open_*` and the hysteresis pair is non-negative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weights {
    // ── hand ───────────────────────────────────────────────────────────────
    /// CUMULATIVE cost of using k fingers, indexed by k, clamped at 8.
    /// Marginals 80, 80, 80, 80, **300**, 600, 900, 1200: the fifth finger is
    /// expensive because it does not exist, but it is priced, not forbidden.
    pub finger_cum: [i32; 9],
    pub barre_setup: i32,
    /// Indexed by `span = max fretted - min fretted`, clamped at 8. Convex.
    pub stretch: [i32; 9],
    /// Percent. 100 is a 25.5" guitar, 140 a 34" bass. Scales `stretch`.
    pub stretch_scale: i32,

    // ── neck position ──────────────────────────────────────────────────────
    /// Times `(anchor - capo)`. Capo-relative: a capo at 5 must not tax every
    /// shape five frets.
    pub position_per_fret: i32,
    pub position_high_fret: u8,
    /// Extra per fret above `position_high_fret`, measured ABSOLUTELY: the
    /// 12th fret is a physical landmark, not a capo-relative one.
    pub position_high_per_fret: i32,
    /// Also what keeps `FretboardSpec { frets: 255 }` harmless.
    pub position_cap: i32,
    pub open_min: i32,
    /// Per fret of `(anchor - capo)`. The sign flips 7 frets above the capo:
    /// an open string with the hand at the 12th is a real device, but on a
    /// monitor it reads as an error.
    pub open_slope: i32,
    pub open_max: i32,
    /// Per silent string strictly BETWEEN the lowest and highest sounding
    /// one. Leading and trailing mutes are free, which is why `x-x-0-2-3-2`
    /// pays nothing for its two dead low strings.
    pub skip_interior: i32,

    // ── folding ────────────────────────────────────────────────────────────
    pub fold_place: i32,
    pub fold_per_extra_octave: i32,

    // ── drops: a tier of their own, see `min_unique_drop` ───────────────────
    pub drop_base: i32,
    /// The bass defines the chord.
    pub drop_lowest: i32,
    /// The melody is what you hear.
    pub drop_highest: i32,
    /// Applied only to the NON-lowest copy of a pitch class.
    pub drop_doubled: i32,
    /// Flat, and it REPLACES every modifier above. A ghost is cheap to lose.
    pub drop_folded: i32,
    /// Per held note that arrived strictly later: an older voice is cheaper to
    /// shed than the one just played.
    pub recency_per: i32,
    pub recency_cap: i32,
    /// Added if the pitch was placed last solve, subtracted if it was not.
    /// Lives in the drop tier deliberately: it only reorders WHICH of two
    /// similar notes is shed, which is the sustain-pedal chatter case.
    pub drop_stick: i32,

    // ── hysteresis ─────────────────────────────────────────────────────────
    pub sticky_anchor: i32,
    /// Within two frets of the previous anchor. Not cumulative with the above.
    pub sticky_near: i32,
    pub retain_note: i32,
    pub retain_cap: i32,
    /// Hard floor on `sticky + retain` combined. Inertia can never hide a note
    /// (a drop costs 15000+) and can never survive this much genuine
    /// improvement. This is the master switch for how stubborn the display
    /// feels.
    pub hyst_clamp: i32,

    // ── limits ─────────────────────────────────────────────────────────────
    /// LABEL ONLY, used by `Playability`. Never a gate.
    pub max_fingers: u8,
    /// The pre-cap keeps `strings + note_slack` notes and lets the SEARCH
    /// decide the rest against real string conflicts, rather than a list walk
    /// deciding them on static cost alone.
    ///
    /// This was 2, and 2 was wrong: measured over 14,500 random chords big
    /// enough to trigger it, the pre-cap threw away a note the search could
    /// have placed **11.5% of the time** — the board drew five dots where six
    /// would fit. At 8 that is 0 of 14,500, for a mean of 928 leaves instead
    /// of 213 (about 20us, still a fiftieth of a GUI frame) and a worst case
    /// of 12,645, a third of `leaf_budget`. The pre-cap now only exists to
    /// stop a pathological set from reaching the search at all.
    pub note_slack: usize,
    pub leaf_budget: u32,
    pub idle_decay_ms: u32,
}

impl Weights {
    pub const DEFAULT: Weights = Weights {
        finger_cum: [0, 80, 160, 240, 320, 620, 1220, 2120, 3320],
        barre_setup: 110,
        stretch: [0, 15, 50, 120, 260, 450, 900, 1400, 1600],
        stretch_scale: 100,

        position_per_fret: 30,
        position_high_fret: 12,
        position_high_per_fret: 45,
        position_cap: 1400,
        open_min: -55,
        open_slope: 9,
        open_max: 200,
        skip_interior: 45,

        fold_place: 900,
        fold_per_extra_octave: 150,

        drop_base: 20_000,
        drop_lowest: 10_000,
        drop_highest: 6_000,
        drop_doubled: -12_000,
        drop_folded: 6_000,
        recency_per: 500,
        recency_cap: 4_000,
        drop_stick: 1_000,

        sticky_anchor: -120,
        sticky_near: -60,
        retain_note: -35,
        retain_cap: -200,
        hyst_clamp: -250,

        max_fingers: 4,
        note_slack: 8,
        leaf_budget: 40_000,
        idle_decay_ms: 1_200,
    };

    /// A 34" neck: the same hand covers about 1.4x fewer frets.
    pub const BASS: Weights = Weights {
        stretch_scale: 140,
        ..Weights::DEFAULT
    };

    /// The ONLY place the "Bass (4) means a 34-inch neck" coupling lives.
    /// `FretboardSpec` has no scale-length field to derive it from.
    pub fn for_tuning(t: &Tuning) -> Weights {
        if t.name == "Bass (4)" {
            Weights::BASS
        } else {
            Weights::DEFAULT
        }
    }

    /// Upper bound on the shape terms (everything but folds and drops).
    pub fn max_shape_cost(&self, strings: usize) -> i32 {
        let s = strings.min(MAX_STRINGS) as i32;
        self.finger_cum[strings.min(8)]
            .saturating_add(self.barre_setup.max(0))
            .saturating_add(
                self.stretch[8].max(0).saturating_mul(self.stretch_scale.clamp(0, 10_000)) / 100,
            )
            .saturating_add(self.position_cap.max(0))
            .saturating_add(self.open_max.max(0).saturating_mul(s))
            .saturating_add(self.skip_interior.max(0).saturating_mul((s - 2).max(0)))
    }

    /// Lower bound on the same terms. Only the open-string bonus and the
    /// hysteresis pair can go negative, and the prune below depends on that
    /// staying true — `tests::every_positive_term_really_is_positive` says so.
    pub fn min_shape_cost(&self, strings: usize) -> i32 {
        let s = strings.min(MAX_STRINGS) as i32;
        self.open_min
            .min(0)
            .saturating_mul(s)
            .saturating_add(self.hyst_clamp.min(0))
    }

    /// The cheapest a note whose pitch class sounds nowhere else can ever be
    /// dropped for. Compare with `max_shape_cost - min_shape_cost`: while the
    /// gap holds, no such note is ever shed to buy a nicer shape.
    pub fn min_unique_drop(&self) -> i32 {
        self.drop_base
            .saturating_sub(self.recency_cap)
            .saturating_sub(self.drop_stick)
    }
}

impl Default for Weights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ── fingerprint ─────────────────────────────────────────────────────────────

/// FNV-1a over the tuning name, then each open pitch, then frets, then capo.
///
/// The exact algorithm is part of the contract, not an implementation detail:
/// two builds that disagree here would disagree about whether a remembered
/// shape is stale.
pub fn spec_fingerprint(spec: &FretboardSpec) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x100_0000_01b3;
    let mut h = OFFSET;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    };
    for b in spec.tuning.name.as_bytes() {
        eat(*b);
    }
    eat(0);
    for &o in spec.tuning.open {
        eat(o);
    }
    eat(0);
    eat(spec.frets);
    eat(spec.capo);
    h
}

// ── entry points ────────────────────────────────────────────────────────────

/// Stateless, deterministic, total. The test entry point.
///
/// `held` may be unsorted and may contain duplicates: this sorts and dedups at
/// the door. That is not politeness — `app.rs::display_notes()` builds a FRESH
/// `HashSet<u8>` on every call and `RandomState` is seeded per instance, so
/// iteration order genuinely varies call to call.
pub fn solve_cold(spec: &FretboardSpec, held: &[u8]) -> Voicing {
    solve(spec, held, &History::NONE, &Weights::for_tuning(spec.tuning))
}

pub fn solve(spec: &FretboardSpec, held: &[u8], hist: &History<'_>, w: &Weights) -> Voicing {
    run(spec, held, hist, w, false).0
}

/// The same search plus the numbers `voiceprobe` prints, mirroring
/// `detect_chord_debug`. Runners-up are best first and capped at eight.
pub fn solve_debug(
    spec: &FretboardSpec,
    held: &[u8],
    hist: &History<'_>,
    w: &Weights,
) -> (Voicing, SolveStats, Vec<(i32, Voicing)>) {
    run(spec, held, hist, w, true)
}

// ── the search ──────────────────────────────────────────────────────────────

/// One held pitch on its way through the pipeline.
#[derive(Clone, Copy)]
struct Entry {
    source: u8,
    target: u8,
    shift: i8,
    state: PreState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreState {
    /// Index into the working list the search actually sees.
    Live(usize),
    Unreachable,
    Merged,
    PreCapped,
}

struct Search<'a> {
    spec: &'a FretboardSpec,
    w: &'a Weights,
    strings: usize,
    n: usize,
    target: [u8; MAX_NOTES],
    shift: [i8; MAX_NOTES],
    /// `cand[note][string]` is the single fret producing that target on that
    /// string, or `MUTED`. There is at most one, because
    /// `fret = target - open[string]` has one solution.
    cand: [[u8; MAX_NOTES]; MAX_STRINGS],
    drop_cost: [i32; MAX_NOTES],
    /// `drop_suffix[i] == sum(drop_cost[i..n])`, for the notes a leaf never
    /// reached.
    drop_suffix: [i32; MAX_NOTES + 1],

    hyst: bool,
    prev_anchor: Option<u8>,
    /// `1 + string * 256 + fret` per MIDI pitch placed in the previous solve,
    /// 0 for "was not placed".
    prev_pos: [u16; 128],

    sel: [u8; MAX_STRINGS],
    sel_note: [u8; MAX_STRINGS],

    leaves: u32,
    pruned: u32,
    truncated: bool,
    have_best: bool,
    best_cost: i32,
    best_tie: [u8; MAX_STRINGS],
    best_sel: [u8; MAX_STRINGS],
    best_note: [u8; MAX_STRINGS],

    collect: bool,
    seen: Vec<(i32, [u8; MAX_STRINGS], [u8; MAX_STRINGS])>,
}

fn run(
    spec: &FretboardSpec,
    held: &[u8],
    hist: &History<'_>,
    w: &Weights,
    collect: bool,
) -> (Voicing, SolveStats, Vec<(i32, Voicing)>) {
    let fp = spec_fingerprint(spec);
    let strings = spec.tuning.strings().min(MAX_STRINGS);

    // Step 0 — normalise. Sorted and deduped, always, whatever the caller did.
    let mut pitches: Vec<u8> = held.to_vec();
    pitches.sort_unstable();
    pitches.dedup();

    let blank = |notes: Vec<NotePlacement>, shape: Shape| Voicing {
        notes,
        strings: vec![StringState::Unused; strings],
        shape,
        cost: 0,
        spec_fp: fp,
        truncated: false,
        capo: spec.capo,
        frets: spec.frets,
    };

    if pitches.is_empty() {
        return (blank(Vec::new(), Shape::default()), SolveStats::default(), Vec::new());
    }

    // Step 1 — spec sanity. Never panics, and never consults `range()`, which
    // returns min > max at capo >= 47 and would underflow on subtraction.
    if strings == 0 || spec.capo > spec.frets {
        let notes = pitches
            .iter()
            .map(|&p| NotePlacement { pitch: p, outcome: Outcome::Unreachable })
            .collect::<Vec<_>>();
        let shape = Shape {
            unreachable_count: notes.len().min(u8::MAX as usize) as u8,
            ..Shape::default()
        };
        return (blank(notes, shape), SolveStats::default(), Vec::new());
    }

    // Step 2 — fold each pitch to ONE target, up preferred on ties. A too-low
    // bass is the common case and folding it up reads better than down. The
    // sweep runs to +-21 rather than a comfortable +-10 because
    // `IVORY_DEMO_NOTES` parses with `parse::<u8>()` and can inject 200.
    let mut entries: Vec<Entry> = Vec::with_capacity(pitches.len());
    for &p in &pitches {
        if reachable(spec, p) {
            entries.push(Entry { source: p, target: p, shift: 0, state: PreState::Live(0) });
            continue;
        }
        let mut found = None;
        'octaves: for k in 1..=21i32 {
            for sgn in [1i32, -1] {
                let t = p as i32 + 12 * sgn * k;
                if !(0..=255).contains(&t) {
                    continue;
                }
                if reachable(spec, t as u8) {
                    found = Some((t as u8, (sgn * k) as i8));
                    break 'octaves;
                }
            }
        }
        match found {
            Some((target, shift)) => {
                entries.push(Entry { source: p, target, shift, state: PreState::Live(0) });
            }
            None => entries.push(Entry {
                source: p,
                target: p,
                shift: 0,
                state: PreState::Unreachable,
            }),
        }
    }

    // Step 3 — merge collisions. The unfolded note beats a ghost that landed
    // on it, and the smaller fold beats the larger.
    let mut order: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].state != PreState::Unreachable)
        .collect();
    order.sort_by_key(|&i| {
        let e = entries[i];
        (e.target, e.shift.unsigned_abs(), e.source)
    });
    let mut live: Vec<usize> = Vec::with_capacity(order.len());
    let mut last_target: Option<u8> = None;
    for i in order {
        if last_target == Some(entries[i].target) {
            entries[i].state = PreState::Merged;
        } else {
            last_target = Some(entries[i].target);
            live.push(i);
        }
    }
    // Back to ascending target order for the monotone search.
    live.sort_by_key(|&i| entries[i].target);

    // Steps 4 and 5 — static drop costs, then the pre-cap. Drop cost never
    // depends on the assignment, which is what makes both the pre-cap and the
    // search's prune sound.
    let keep = strings.saturating_add(w.note_slack).min(MAX_NOTES);
    while live.len() > keep {
        let victim = (0..live.len())
            .min_by_key(|&k| {
                (
                    drop_cost_of(&live, &entries, k, hist, w),
                    std::cmp::Reverse(entries[live[k]].target),
                )
            })
            .unwrap_or(0);
        entries[live[victim]].state = PreState::PreCapped;
        live.remove(victim);
    }

    let n = live.len();
    let mut s = Search {
        spec,
        w,
        strings,
        n,
        target: [0; MAX_NOTES],
        shift: [0; MAX_NOTES],
        cand: [[MUTED; MAX_NOTES]; MAX_STRINGS],
        drop_cost: [0; MAX_NOTES],
        drop_suffix: [0; MAX_NOTES + 1],
        hyst: false,
        prev_anchor: None,
        prev_pos: [0; 128],
        sel: [MUTED; MAX_STRINGS],
        sel_note: [MUTED; MAX_STRINGS],
        leaves: 0,
        pruned: 0,
        truncated: false,
        have_best: false,
        best_cost: i32::MAX,
        best_tie: [MUTED; MAX_STRINGS],
        best_sel: [MUTED; MAX_STRINGS],
        best_note: [MUTED; MAX_STRINGS],
        collect,
        seen: Vec::new(),
    };
    for k in 0..n {
        s.target[k] = entries[live[k]].target;
        s.shift[k] = entries[live[k]].shift;
        s.drop_cost[k] = drop_cost_of(&live, &entries, k, hist, w);
    }
    for (k, &i) in live.iter().enumerate() {
        entries[i].state = PreState::Live(k);
    }
    for k in (0..n).rev() {
        s.drop_suffix[k] = s.drop_suffix[k + 1].saturating_add(s.drop_cost[k]);
    }

    // Step 6 — the candidate table. One fret per (note, string) at most.
    for st in 0..strings {
        let open = spec.tuning.open[st];
        for k in 0..n {
            if s.target[k] >= open {
                let f = s.target[k] - open;
                if f >= spec.capo && f <= spec.frets {
                    s.cand[st][k] = f;
                }
            }
        }
    }

    // Hysteresis, gated on a matching board AND a shared pitch. A clean jump
    // to an unrelated chord gets no inertia, because the player's hand moved.
    if let Some(prev) = hist.prev {
        let shared = prev.spec_fp == fp
            && prev.notes.iter().any(|pn| pitches.binary_search(&pn.pitch).is_ok());
        if shared {
            s.hyst = true;
            s.prev_anchor = prev.shape.anchor;
            for pn in &prev.notes {
                if let Outcome::Placed { pos, .. } = pn.outcome {
                    if pos.string < MAX_STRINGS {
                        s.prev_pos[pn.pitch as usize % 128] =
                            1 + (pos.string as u16) * 256 + pos.fret as u16;
                    }
                }
            }
        }
    }

    // Step 7 — the search.
    s.rec(0, 0, 0);

    let stats = SolveStats { leaves: s.leaves, pruned: s.pruned, truncated: s.truncated };
    let winner = s.finalize(&entries, s.best_cost, s.best_sel, s.best_note);

    let runners = if collect {
        let mut all = std::mem::take(&mut s.seen);
        all.sort_unstable_by_key(|&(cost, sel, _)| (cost, sel));
        all.dedup_by(|a, b| a.1 == b.1);
        all.into_iter()
            .skip(1)
            .take(8)
            .map(|(c, sel, note)| (c, s.finalize(&entries, c, sel, note)))
            .collect()
    } else {
        Vec::new()
    };

    (winner, stats, runners)
}

/// What it costs to leave the note at working-index `k` off the board.
///
/// Static: it depends on the note's place in the held chord, not on the
/// assignment. That is exactly what makes both the pre-cap and the search's
/// prune sound — a bound computed from committed drops stays a bound.
///
/// The resulting hierarchy IS the whole drop policy, in one line:
/// `folded 6000 < doubled 8000 < plain 20000 < melody 26000 < bass 30000`.
fn drop_cost_of(
    live: &[usize],
    entries: &[Entry],
    k: usize,
    hist: &History<'_>,
    w: &Weights,
) -> i32 {
    let e = entries[live[k]];
    // Saturating throughout, here and in `shape_cost_of`. Every number below
    // is a DIAL the owner is invited to turn while play-testing, and turning
    // one to something silly must produce a silly shape, not a debug-build
    // panic. Release builds would wrap instead, which is worse: a cost that
    // wrapped negative would make the solver prefer exactly the shape the dial
    // was meant to discourage.
    let mut c = if e.shift != 0 {
        // A ghost is cheap to lose, and none of the modifiers below apply: it
        // is not really the bass or the melody, it is a stand-in for one.
        w.drop_folded
    } else {
        let mut c = w.drop_base;
        if k == 0 {
            c = c.saturating_add(w.drop_lowest);
        }
        if k + 1 == live.len() {
            c = c.saturating_add(w.drop_highest);
        }
        if live[..k].iter().any(|&j| entries[j].target % 12 == e.target % 12) {
            c = c.saturating_add(w.drop_doubled);
        }
        c
    };
    if let Some(a) = hist.arrival {
        let mine = a[e.source as usize % 128];
        let later = live
            .iter()
            .filter(|&&i| a[entries[i].source as usize % 128] > mine)
            .count() as i32;
        c = c.saturating_sub(w.recency_per.saturating_mul(later).min(w.recency_cap));
    }
    // Stickiness is a hysteresis term, so it only exists once there is a
    // previous solve to be sticky about. With no history it is silent, not
    // "everything was unplaced" — otherwise a cold solve would quietly shed
    // notes a warm one keeps.
    if let Some(prev) = hist.prev {
        let was_placed = prev
            .notes
            .iter()
            .any(|n| n.pitch == e.source && matches!(n.outcome, Outcome::Placed { .. }));
        c = if was_placed {
            c.saturating_add(w.drop_stick)
        } else {
            c.saturating_sub(w.drop_stick)
        };
    }
    c
}

impl Search<'_> {
    fn rec(&mut self, st: usize, i: usize, committed: i32) {
        // Admissible prune. `>` not `>=`: an equal-cost shape with a smaller
        // tie key must still be reachable, or the argmin would depend on the
        // DFS order rather than on the key.
        if self.have_best
            && committed.saturating_add(self.w.min_shape_cost(self.strings)) > self.best_cost
        {
            self.pruned += 1;
            return;
        }
        if self.truncated {
            return;
        }
        if st == self.strings {
            if self.leaves >= self.w.leaf_budget {
                self.truncated = true;
                return;
            }
            self.leaves += 1;
            let total = committed
                .saturating_add(self.drop_suffix[i])
                .saturating_add(self.shape_cost());
            self.consider(total);
            return;
        }
        // Mute this string. The cursor does not advance: muting a string is
        // not the same as giving up on a note.
        self.sel[st] = MUTED;
        self.sel_note[st] = MUTED;
        self.rec(st + 1, i, committed);
        // Or sound one of the remaining notes on it. Taking note `j` and
        // moving the cursor to `j + 1` is what makes the assignment monotone:
        // one note per string, ascending pitch on ascending strings, and
        // skipping a note means dropping it — all three by construction, with
        // no checks anywhere.
        let mut skipped = 0i32;
        for j in i..self.n {
            let f = self.cand[st][j];
            if f != MUTED {
                self.sel[st] = f;
                self.sel_note[st] = j as u8;
                self.rec(st + 1, j + 1, committed.saturating_add(skipped));
            }
            skipped = skipped.saturating_add(self.drop_cost[j]);
        }
        self.sel[st] = MUTED;
        self.sel_note[st] = MUTED;
    }

    fn consider(&mut self, cost: i32) {
        let tie = self.tie_key();
        if self.collect && self.seen.len() < 60_000 {
            self.seen.push((cost, self.sel, self.sel_note));
        }
        if !self.have_best || (cost, tie) < (self.best_cost, self.best_tie) {
            self.have_best = true;
            self.best_cost = cost;
            self.best_tie = tie;
            self.best_sel = self.sel;
            self.best_note = self.sel_note;
        }
    }

    /// The fret sounding on each string, `MUTED` where silent. Injective over
    /// shapes, because targets are distinct and each (note, string) pair has
    /// one fret — so the argmin under `(cost, tie)` is unique, and a later
    /// refactor of the DFS cannot silently change which shape ships.
    fn tie_key(&self) -> [u8; MAX_STRINGS] {
        let mut t = [MUTED; MAX_STRINGS];
        t[..self.strings].copy_from_slice(&self.sel[..self.strings]);
        t
    }

    /// Everything but the drops: fingers, stretch, position, opens, skips,
    /// folds and inertia, scored from the current `sel` arrays.
    fn shape_cost(&self) -> i32 {
        shape_cost_of(self.spec, self.w, self.strings, &self.sel, |st| {
            self.shift[self.sel_note[st] as usize]
        }, self.hyst, self.prev_anchor, |st| {
            let k = self.sel_note[st] as usize;
            // Keyed by SOURCE pitch, which is the target undone by the shift.
            let src = self.target[k] as i32 - 12 * self.shift[k] as i32;
            if !(0..128).contains(&src) {
                return 0;
            }
            self.prev_pos[src as usize]
        })
    }

    /// Turn a `sel`/`sel_note` pair back into the public `Voicing`.
    fn finalize(
        &self,
        entries: &[Entry],
        cost: i32,
        sel: [u8; MAX_STRINGS],
        sel_note: [u8; MAX_STRINGS],
    ) -> Voicing {
        let spec = self.spec;
        let capo = spec.capo;
        let (barre, fingers) = barre_and_fingers(&sel, self.strings, capo);
        let (anchor, span) = anchor_and_span(&sel, self.strings, capo);

        // Which strings sound, and the interior gaps between them.
        let mut lo = usize::MAX;
        let mut hi = 0usize;
        for (st, &f) in sel.iter().enumerate().take(self.strings) {
            if f != MUTED {
                if lo == usize::MAX {
                    lo = st;
                }
                hi = st;
            }
        }
        let mut strings = vec![StringState::Unused; self.strings];
        let mut open_count = 0u8;
        let mut folded_count = 0u8;
        for st in 0..self.strings {
            if sel[st] == MUTED {
                if lo != usize::MAX && st > lo && st < hi {
                    strings[st] = StringState::Skipped;
                }
                continue;
            }
            let k = sel_note[st] as usize;
            let folded = self.shift[k] != 0;
            strings[st] = StringState::Sounding { fret: sel[st], pitch: self.target[k], folded };
            if sel[st] == capo {
                open_count = open_count.saturating_add(1);
            }
            if folded {
                folded_count = folded_count.saturating_add(1);
            }
        }

        // Where each live note ended up, by working index.
        let mut placed_at = [MUTED; MAX_NOTES];
        for st in 0..self.strings {
            if sel_note[st] != MUTED {
                placed_at[sel_note[st] as usize] = st as u8;
            }
        }
        let sounding_pitches: Vec<u8> = (0..self.strings)
            .filter(|&st| sel[st] != MUTED)
            .map(|st| self.target[sel_note[st] as usize])
            .collect();
        let sounding_pcs: Vec<u8> = sounding_pitches.iter().map(|p| p % 12).collect();

        let mut notes = Vec::with_capacity(entries.len());
        let mut dropped_count = 0u8;
        let mut conflict_count = 0u8;
        let mut unreachable_count = 0u8;
        for e in entries {
            let outcome = match e.state {
                PreState::Unreachable => {
                    unreachable_count = unreachable_count.saturating_add(1);
                    Outcome::Unreachable
                }
                // "It folded onto a pitch another note already occupies" is
                // only true if that other note actually made it onto the board.
                // When the survivor was itself dropped, the merge is not the
                // reason this one is missing, so fall through and say what is.
                PreState::Merged if sounding_pitches.contains(&e.target) => {
                    dropped_count = dropped_count.saturating_add(1);
                    Outcome::Dropped { reason: DropReason::OctaveMerged }
                }
                PreState::Live(k) if placed_at[k] != MUTED => {
                    let st = placed_at[k] as usize;
                    Outcome::Placed {
                        pos: Position { string: st, fret: sel[st], folded: e.shift != 0 },
                        sounding: e.target,
                        octave_shift: e.shift,
                    }
                }
                PreState::Live(_) | PreState::PreCapped | PreState::Merged => {
                    // Step 10, first match wins. `Doubled` is checked ahead of
                    // `Conflict` on purpose: when a note is already sounding an
                    // octave away, "already there" is the useful explanation
                    // and "not at the same time" is a technically-true one.
                    if sounding_pcs.contains(&(e.target % 12)) {
                        dropped_count = dropped_count.saturating_add(1);
                        Outcome::Dropped { reason: DropReason::Doubled }
                    } else {
                        // `candidates` reports the FOLDED target's positions and
                        // stamps them `folded: false`, which would have the view
                        // draw a ghost's ring exactly like a real note's. The
                        // ring is at the right place for the octave being shown;
                        // it just has to say that is what it is.
                        let mut wanted = candidates(spec, e.target);
                        if e.shift != 0 {
                            for w in &mut wanted {
                                w.folded = true;
                            }
                        }
                        let free: Vec<&Position> = wanted
                            .iter()
                            .filter(|p| p.string >= self.strings || sel[p.string] == MUTED)
                            .collect();
                        if wanted.is_empty() {
                            dropped_count = dropped_count.saturating_add(1);
                            Outcome::Dropped { reason: DropReason::Excess }
                        } else if free.is_empty() {
                            conflict_count = conflict_count.saturating_add(1);
                            Outcome::Conflict { wanted }
                        } else if free.iter().all(|p| {
                            breaks_order(&sel, &sel_note, &self.target, self.strings, p.string, e.target)
                        }) {
                            dropped_count = dropped_count.saturating_add(1);
                            Outcome::Dropped { reason: DropReason::Ordering }
                        } else {
                            dropped_count = dropped_count.saturating_add(1);
                            Outcome::Dropped { reason: DropReason::Excess }
                        }
                    }
                }
            };
            notes.push(NotePlacement { pitch: e.source, outcome });
        }
        notes.sort_by_key(|n| n.pitch);

        let sounds = lo != usize::MAX;
        let playability = if !sounds {
            Playability::Silent
        } else if anchor.is_none() {
            Playability::Open
        } else {
            let allow = span_allow(anchor.unwrap_or(0));
            if fingers <= self.w.max_fingers && span <= allow {
                Playability::OneHand
            } else if fingers <= self.w.max_fingers && span <= allow.saturating_add(2) {
                Playability::Stretch
            } else {
                Playability::TwoHands
            }
        };

        Voicing {
            notes,
            strings,
            shape: Shape {
                anchor,
                span,
                fingers,
                barre,
                playability,
                open_count,
                folded_count,
                dropped_count,
                conflict_count,
                unreachable_count,
            },
            cost: if self.have_best { cost } else { 0 },
            spec_fp: spec_fingerprint(spec),
            truncated: self.truncated,
            capo,
            frets: spec.frets,
        }
    }
}

/// Would sounding `target` on `string` break the winner's ascending order?
fn breaks_order(
    sel: &[u8; MAX_STRINGS],
    sel_note: &[u8; MAX_STRINGS],
    targets: &[u8; MAX_NOTES],
    strings: usize,
    string: usize,
    target: u8,
) -> bool {
    (0..strings).any(|st| {
        if sel[st] == MUTED {
            return false;
        }
        let p = targets[sel_note[st] as usize];
        (st < string && p > target) || (st > string && p < target)
    })
}

/// `4` frets at the nut, `5` in the middle, `6` up where they are narrow. A
/// free function rather than an array so no fret value can index out of range.
fn span_allow(anchor: u8) -> u8 {
    match anchor {
        0..=4 => 4,
        5..=11 => 5,
        _ => 6,
    }
}

/// Lowest fretted fret and the distance to the highest. A string stopped at
/// the capo is open, so it is neither.
fn anchor_and_span(sel: &[u8; MAX_STRINGS], strings: usize, capo: u8) -> (Option<u8>, u8) {
    let mut min = u8::MAX;
    let mut max = 0u8;
    let mut any = false;
    for &f in sel.iter().take(strings) {
        if f == MUTED || f == capo {
            continue;
        }
        any = true;
        min = min.min(f);
        max = max.max(f);
    }
    if any {
        (Some(min), max - min)
    } else {
        (None, 0)
    }
}

/// A barre, and the finger count after its discount.
///
/// The rule is fussier than it looks and every clause earns its place. Two
/// strings at the lowest fret is only a barre if they are ADJACENT (otherwise
/// open D's `x-x-0-2-3-2` reads as a two-string barre it is not), and a
/// sounding OPEN string strictly inside the span rules one out (a flat finger
/// would stop it) — but an open string OUTSIDE the span is fine, which is what
/// makes Dm7's `x-x-0-2-1-1` the mini-barre it really is.
fn barre_and_fingers(sel: &[u8; MAX_STRINGS], strings: usize, capo: u8) -> (Option<Barre>, u8) {
    let fretted = |st: usize| sel[st] != MUTED && sel[st] != capo;
    let mut count = 0u8;
    let mut min = u8::MAX;
    for (st, &f) in sel.iter().enumerate().take(strings) {
        if fretted(st) {
            count += 1;
            min = min.min(f);
        }
    }
    if count == 0 {
        return (None, 0);
    }
    let bar: Vec<usize> = (0..strings).filter(|&st| fretted(st) && sel[st] == min).collect();
    let (lo, hi) = (bar[0], bar[bar.len() - 1]);
    let contiguous_enough = bar.len() >= 3 || (bar.len() == 2 && hi - lo == 1);
    let nothing_open_inside = ((lo + 1)..hi).all(|st| sel[st] == MUTED || sel[st] != capo);
    if bar.len() >= 2 && contiguous_enough && nothing_open_inside {
        let above = (0..strings).filter(|&st| fretted(st) && sel[st] > min).count() as u8;
        (Some(Barre { fret: min, lo_string: lo, hi_string: hi }), 1 + above)
    } else {
        (None, count)
    }
}

/// The shape half of the objective. Split out so the acceptance tests can
/// re-derive a cost independently of the search that produced it.
#[allow(clippy::too_many_arguments)]
fn shape_cost_of(
    spec: &FretboardSpec,
    w: &Weights,
    strings: usize,
    sel: &[u8; MAX_STRINGS],
    shift_of: impl Fn(usize) -> i8,
    hyst: bool,
    prev_anchor: Option<u8>,
    prev_pos_of: impl Fn(usize) -> u16,
) -> i32 {
    let capo = spec.capo;
    let (anchor, span) = anchor_and_span(sel, strings, capo);
    let (barre, fingers) = barre_and_fingers(sel, strings, capo);

    let mut cost = w.finger_cum[(fingers as usize).min(8)];
    if barre.is_some() {
        cost = cost.saturating_add(w.barre_setup);
    }

    if let Some(a) = anchor {
        // Stretch, scaled by the neck's own geometry: the same five frets is a
        // different hand at fret 1 and at fret 12.
        cost = cost.saturating_add(
            w.stretch[(span as usize).min(8)]
                .saturating_mul(REACH_MILLI[(a as usize).min(24)])
                / 1000
                * w.stretch_scale.clamp(-10_000, 10_000)
                / 100,
        );
        // Position: capo-relative below the 12th, absolute above it.
        let mut pos = w.position_per_fret.saturating_mul(a as i32 - capo as i32);
        if a > w.position_high_fret {
            pos = pos.saturating_add(
                w.position_high_per_fret.saturating_mul(a as i32 - w.position_high_fret as i32),
            );
        }
        cost = cost.saturating_add(pos.min(w.position_cap));
    }

    let anchor_rel = anchor.map_or(0, |a| a as i32 - capo as i32);
    // `.max().min()` rather than `.clamp()`: clamp panics when min > max, and
    // these are two independent dials that nothing stops the owner inverting.
    let per_open = w
        .open_min
        .saturating_add(w.open_slope.saturating_mul(anchor_rel))
        .max(w.open_min)
        .min(w.open_max);

    let mut lo = usize::MAX;
    let mut hi = 0usize;
    for (st, &f) in sel.iter().enumerate().take(strings) {
        if f == MUTED {
            continue;
        }
        if lo == usize::MAX {
            lo = st;
        }
        hi = st;
        if f == capo {
            cost = cost.saturating_add(per_open);
        }
        let shift = shift_of(st);
        if shift != 0 {
            cost = cost.saturating_add(w.fold_place).saturating_add(
                w.fold_per_extra_octave
                    .saturating_mul(shift.unsigned_abs() as i32 - 1),
            );
        }
    }
    if lo != usize::MAX {
        cost = cost.saturating_add(
            w.skip_interior
                .saturating_mul(((lo + 1)..hi).filter(|&st| sel[st] == MUTED).count() as i32),
        );
    }

    if hyst {
        let mut h = match (anchor, prev_anchor) {
            (Some(a), Some(p)) if a == p => w.sticky_anchor,
            (Some(a), Some(p)) if a.abs_diff(p) <= 2 => w.sticky_near,
            (None, None) => w.sticky_anchor,
            _ => 0,
        };
        let mut retained = 0i32;
        for (st, &f) in sel.iter().enumerate().take(strings) {
            if f == MUTED {
                continue;
            }
            if prev_pos_of(st) == 1 + (st as u16) * 256 + f as u16 {
                retained = retained.saturating_add(w.retain_note);
            }
        }
        h = h.saturating_add(retained.max(w.retain_cap));
        cost = cost.saturating_add(h.max(w.hyst_clamp));
    }

    cost
}

// ── the stateful layer ──────────────────────────────────────────────────────

/// Owns the memo cache, the previous shape, arrival ordinals and idle decay.
///
/// The app owns exactly ONE of these and clones the resulting [`Voicing`] to
/// the main and detached views, so the two windows cannot show different
/// shapes. It carries no clock: `dt_ms` comes from the caller.
pub struct VoicingSession {
    spec: FretboardSpec,
    w: Weights,
    out: Voicing,
    /// The last non-empty result, used as the hysteresis anchor. Empty `notes`
    /// means there is no memory to lean on.
    memory: Voicing,
    solved_for: Vec<u8>,
    scratch: Vec<u8>,
    seen_at: [u32; 128],
    next_ord: u32,
    idle_ms: u32,
    stats: SolveStats,
    solves: u64,
}

impl VoicingSession {
    pub fn new(spec: FretboardSpec, w: Weights) -> Self {
        let out = solve(&spec, &[], &History::NONE, &w);
        Self {
            spec,
            w,
            memory: out.clone(),
            out,
            solved_for: Vec::new(),
            scratch: Vec::new(),
            seen_at: [0; 128],
            next_ord: 0,
            idle_ms: 0,
            stats: SolveStats::default(),
            solves: 0,
        }
    }

    /// Discards the memory AND the cache. A shape the new rules could not have
    /// produced must never survive a rule change.
    pub fn set_spec(&mut self, spec: FretboardSpec) {
        self.spec = spec;
        self.reset();
    }

    pub fn set_weights(&mut self, w: Weights) {
        self.w = w;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.out = solve(&self.spec, &[], &History::NONE, &self.w);
        self.memory = self.out.clone();
        self.solved_for.clear();
        self.seen_at = [0; 128];
        self.next_ord = 0;
        self.idle_ms = 0;
        self.stats = SolveStats::default();
    }

    pub fn spec(&self) -> &FretboardSpec {
        &self.spec
    }

    pub fn weights(&self) -> &Weights {
        &self.w
    }

    pub fn current(&self) -> &Voicing {
        &self.out
    }

    pub fn stats(&self) -> SolveStats {
        self.stats
    }

    /// How many real searches have run. The cache is only worth having if this
    /// stays flat while a chord is held, which a test asserts.
    pub fn solves(&self) -> u64 {
        self.solves
    }

    /// The one call the GUI makes. `dt_ms` is the time since the last one.
    pub fn update(&mut self, held: &HashSet<u8>, dt_ms: u32) -> &Voicing {
        let mut v: Vec<u8> = held.iter().copied().collect();
        v.sort_unstable();
        self.update_sorted(&v, dt_ms)
    }

    pub fn update_sorted(&mut self, held: &[u8], dt_ms: u32) -> &Voicing {
        self.scratch.clear();
        self.scratch.extend_from_slice(held);
        self.scratch.sort_unstable();
        self.scratch.dedup();

        if self.scratch.is_empty() {
            self.idle_ms = self.idle_ms.saturating_add(dt_ms);
            if self.idle_ms >= self.w.idle_decay_ms {
                self.memory = solve(&self.spec, &[], &History::NONE, &self.w);
                self.seen_at = [0; 128];
                self.next_ord = 0;
            }
            if !self.solved_for.is_empty() {
                self.solved_for.clear();
                self.out = solve(&self.spec, &[], &History::NONE, &self.w);
                self.stats = SolveStats::default();
            }
            return &self.out;
        }

        if self.scratch == self.solved_for && self.out.spec_fp == spec_fingerprint(&self.spec) {
            return &self.out;
        }

        self.idle_ms = 0;
        // ONE ordinal per update, shared by everything new in it. Handing out
        // a separate ordinal per note would rank a chord struck as one gesture
        // by pitch order, and the drop policy reads that ranking as age: a
        // ten-note voicing would then shed its lowest voices because they
        // "arrived first", and the session would draw a different shape from
        // `solve_cold` on the same notes. Arrival is a property of the tick,
        // not of the note.
        if self
            .scratch
            .iter()
            .any(|&p| self.seen_at[p as usize % 128] == 0)
        {
            self.next_ord = self.next_ord.saturating_add(1);
            for &p in &self.scratch {
                let slot = &mut self.seen_at[p as usize % 128];
                if *slot == 0 {
                    *slot = self.next_ord;
                }
            }
        }
        // Ordinals only ever grow; on the (theoretical) wrap, start clean
        // rather than let a stale ordering decide which voice is shed.
        if self.next_ord == u32::MAX {
            self.seen_at = [0; 128];
            self.next_ord = 0;
        }
        for (p, slot) in self.seen_at.iter_mut().enumerate() {
            if *slot != 0 && self.scratch.binary_search(&(p as u8)).is_err() {
                *slot = 0;
            }
        }

        let fp = spec_fingerprint(&self.spec);
        let hist = History {
            prev: if self.memory.notes.is_empty() || self.memory.spec_fp != fp {
                None
            } else {
                Some(&self.memory)
            },
            arrival: Some(&self.seen_at),
            root_pc: None,
        };
        let (out, stats, _) = run(&self.spec, &self.scratch, &hist, &self.w, false);
        self.solves += 1;
        self.stats = stats;
        self.out = out;
        self.memory = self.out.clone();
        self.solved_for.clear();
        self.solved_for.extend_from_slice(&self.scratch);
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fretboard::TUNINGS;

    fn std_spec() -> FretboardSpec {
        FretboardSpec::default()
    }

    /// The shape as a guitarist writes it: one entry per string low to high,
    /// `None` for a muted one.
    fn frets(v: &Voicing) -> Vec<Option<u8>> {
        v.strings
            .iter()
            .map(|s| match s {
                StringState::Sounding { fret, .. } => Some(*fret),
                _ => None,
            })
            .collect()
    }

    fn shape(spec: &FretboardSpec, held: &[u8]) -> (Vec<Option<u8>>, Voicing) {
        let v = solve_cold(spec, held);
        (frets(&v), v)
    }

    /// Deterministic pseudo-random note sets: a fixed-seed LCG, so a failure
    /// is reproducible rather than "it went red on CI once".
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        fn set(&mut self, lo: u8, hi: u8, max_len: usize) -> Vec<u8> {
            let len = (self.next() as usize % max_len) + 1;
            let mut v: Vec<u8> = (0..len)
                .map(|_| lo + (self.next() as u8 % (hi - lo + 1)))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        }
    }

    // ── the shapes ──────────────────────────────────────────────────────────

    #[test]
    fn the_open_chords_come_out_as_open_chords() {
        let s = std_spec();
        let e = shape(&s, &[40, 47, 52, 56, 59, 64]);
        assert_eq!(e.0, [Some(0), Some(2), Some(2), Some(1), Some(0), Some(0)]);
        assert_eq!(e.1.shape.fingers, 3, "open E is three fingers, not a barre");
        assert_eq!(e.1.shape.barre, None);
        assert_eq!(e.1.cost, 146);

        let g = shape(&s, &[43, 47, 50, 55, 59, 67]);
        assert_eq!(g.0, [Some(3), Some(2), Some(0), Some(0), Some(0), Some(3)]);
        assert_eq!(g.1.shape.fingers, 3);

        let d = shape(&s, &[50, 57, 62, 66]);
        assert_eq!(d.0, [None, None, Some(0), Some(2), Some(3), Some(2)]);
        // The two strings at fret 2 are NOT adjacent, so this is three fingers.
        assert_eq!(d.1.shape.fingers, 3);
        assert_eq!(d.1.shape.barre, None);

        let c = shape(&s, &[48, 52, 55, 60, 64]);
        assert_eq!(c.0, [None, Some(3), Some(2), Some(0), Some(1), Some(0)]);
        assert_eq!(c.1.shape.fingers, 3);
    }

    #[test]
    fn the_barres_are_recognised_as_barres() {
        let s = std_spec();
        let f = shape(&s, &[41, 48, 53, 57, 60, 65]);
        assert_eq!(f.0, [Some(1), Some(3), Some(3), Some(2), Some(1), Some(1)]);
        assert_eq!(f.1.shape.barre, Some(Barre { fret: 1, lo_string: 0, hi_string: 5 }));
        assert_eq!(f.1.shape.fingers, 4, "index bars, three fingers above it");

        let am7 = shape(&s, &[45, 52, 55, 60, 64, 69]);
        assert_eq!(am7.0, [Some(5), Some(7), Some(5), Some(5), Some(5), Some(5)]);
        assert_eq!(am7.1.shape.barre, Some(Barre { fret: 5, lo_string: 0, hi_string: 5 }));
        assert_eq!(am7.1.shape.fingers, 2);

        // A mini-barre with an OPEN string outside its span is still a barre.
        let dm7 = shape(&s, &[50, 57, 60, 65]);
        assert_eq!(dm7.0, [None, None, Some(0), Some(2), Some(1), Some(1)]);
        assert_eq!(dm7.1.shape.barre, Some(Barre { fret: 1, lo_string: 4, hi_string: 5 }));
        assert_eq!(dm7.1.shape.fingers, 2);
    }

    #[test]
    fn one_pitch_five_places_and_it_picks_the_low_one() {
        let s = std_spec();
        let (f, v) = shape(&s, &[60]);
        assert_eq!(f, [None, None, None, None, Some(1), None]);
        assert_eq!(v.cost, 110);
        // The high E string is tuned above middle C and can never hold it.
        assert!(v.placed().all(|(_, p)| p.string != 5));
    }

    #[test]
    fn an_open_string_beats_the_same_note_higher_up() {
        let s = std_spec();
        let (f, v) = shape(&s, &[45, 55, 60, 64]);
        assert_eq!(f, [None, Some(0), None, Some(0), Some(1), Some(0)]);
        assert_eq!(v.shape.open_count, 3);
        assert_eq!(v.cost, 17);
    }

    // ── the constraints ─────────────────────────────────────────────────────

    #[test]
    fn one_note_per_string_and_never_a_crossing() {
        let mut rng = Lcg(0x1234_5678);
        for t in TUNINGS {
            for capo in [0u8, 2, 5] {
                let spec = FretboardSpec { tuning: t, frets: 22, capo };
                for _ in 0..300 {
                    let held = rng.set(30, 100, 9);
                    let v = solve_cold(&spec, &held);
                    let mut used = Vec::new();
                    for (_, p) in v.placed() {
                        assert!(!used.contains(&p.string), "two notes on string {}", p.string);
                        used.push(p.string);
                    }
                    // Ascending string implies ascending sounding pitch.
                    let mut last: Option<u8> = None;
                    for st in &v.strings {
                        if let StringState::Sounding { pitch, .. } = st {
                            if let Some(prev) = last {
                                assert!(*pitch > prev, "voice crossing: {prev} then {pitch}");
                            }
                            last = Some(*pitch);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_placement_is_a_fret_that_actually_makes_that_note() {
        let mut rng = Lcg(0xfeed_face);
        for t in TUNINGS {
            for capo in [0u8, 3, 7] {
                let spec = FretboardSpec { tuning: t, frets: 22, capo };
                for _ in 0..300 {
                    let held = rng.set(20, 110, 8);
                    let v = solve_cold(&spec, &held);
                    for n in &v.notes {
                        if let Outcome::Placed { pos, sounding, octave_shift } = n.outcome {
                            assert_eq!(spec.pitch_at(pos.string, pos.fret), Some(sounding));
                            assert_eq!(sounding as i32, n.pitch as i32 + 12 * octave_shift as i32);
                            assert_eq!(pos.folded, octave_shift != 0);
                            assert!(pos.fret >= spec.capo && pos.fret <= spec.frets);
                            if octave_shift != 0 {
                                assert!(
                                    candidates(&spec, n.pitch).is_empty(),
                                    "pitch {} folded although it was playable",
                                    n.pitch
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_the_player_pressed_ever_vanishes() {
        let mut rng = Lcg(0xdead_beef);
        for t in TUNINGS {
            let spec = FretboardSpec { tuning: t, frets: 22, capo: 0 };
            for _ in 0..400 {
                let held = rng.set(0, 127, 16);
                let v = solve_cold(&spec, &held);
                assert_eq!(v.notes.len(), held.len());
                let got: Vec<u8> = v.notes.iter().map(|n| n.pitch).collect();
                assert_eq!(got, held, "notes must be one per held pitch, ascending");
                let placed = v.placed().count();
                let sum = placed
                    + v.shape.dropped_count as usize
                    + v.shape.conflict_count as usize
                    + v.shape.unreachable_count as usize;
                assert_eq!(sum, v.notes.len(), "every note needs exactly one outcome");
                assert_eq!(v.strings.len(), t.strings().min(MAX_STRINGS));
            }
        }
    }

    #[test]
    fn the_reported_shape_matches_the_strings_it_reports() {
        let mut rng = Lcg(0x0bad_c0de);
        for capo in [0u8, 4] {
            let spec = FretboardSpec { capo, ..Default::default() };
            for _ in 0..500 {
                let held = rng.set(35, 95, 8);
                let v = solve_cold(&spec, &held);
                let mut sel = [MUTED; MAX_STRINGS];
                for (st, s) in v.strings.iter().enumerate() {
                    if let StringState::Sounding { fret, .. } = s {
                        sel[st] = *fret;
                    }
                }
                let (barre, fingers) = barre_and_fingers(&sel, v.strings.len(), capo);
                let (anchor, span) = anchor_and_span(&sel, v.strings.len(), capo);
                assert_eq!(v.shape.barre, barre);
                assert_eq!(v.shape.fingers, fingers);
                assert_eq!(v.shape.anchor, anchor);
                assert_eq!(v.shape.span, span);
                let opens = v.strings.iter().filter(|s| matches!(s, StringState::Sounding { fret, .. } if *fret == capo)).count();
                assert_eq!(v.shape.open_count as usize, opens);
                if let Some(b) = barre {
                    let at: Vec<usize> = (0..v.strings.len())
                        .filter(|&st| sel[st] == b.fret && sel[st] != capo)
                        .collect();
                    assert!(at.len() >= 2);
                    if at.len() == 2 {
                        assert_eq!(at[1] - at[0], 1, "a two-string barre must be adjacent");
                    }
                    for st in (b.lo_string + 1)..b.hi_string {
                        if sel[st] != MUTED {
                            assert!(sel[st] > capo, "a barre cannot cross a sounding open string");
                            assert!(sel[st] >= b.fret, "a finger cannot reach behind the barre");
                        }
                    }
                }
            }
        }
    }

    // ── determinism ─────────────────────────────────────────────────────────

    #[test]
    fn the_same_notes_always_draw_the_same_shape() {
        let mut rng = Lcg(0xabcd_1234);
        for t in TUNINGS {
            for capo in 0..=7u8 {
                let spec = FretboardSpec { tuning: t, frets: 22, capo };
                for _ in 0..120 {
                    let held = rng.set(30, 100, 8);
                    let first = solve_cold(&spec, &held);
                    assert_eq!(first, solve_cold(&spec, &held));
                    // Order and duplicates at the door must not matter.
                    let mut shuffled = held.clone();
                    shuffled.reverse();
                    shuffled.extend_from_slice(&held);
                    assert_eq!(first, solve_cold(&spec, &shuffled));
                }
            }
        }
    }

    #[test]
    fn a_fresh_hashset_cannot_change_the_answer() {
        // The real trap: `display_notes()` builds a new HashSet every call and
        // RandomState is seeded per instance, so iteration order genuinely
        // varies. If the solver ever depends on it, this goes red.
        let spec = std_spec();
        let held = [36u8, 45, 52, 60, 64, 67, 72];
        let mut seen: Option<Voicing> = None;
        for _ in 0..100 {
            let set: HashSet<u8> = held.iter().copied().collect();
            let v: Vec<u8> = set.iter().copied().collect();
            let got = solve_cold(&spec, &v);
            match &seen {
                None => seen = Some(got),
                Some(first) => assert_eq!(*first, got, "HashSet order leaked into the shape"),
            }
        }
    }

    #[test]
    fn the_winner_is_the_true_minimum_not_just_the_first_one_found() {
        // Brute force on a short neck: enumerate every monotone assignment in
        // the test, score it with the same terms, and demand the solver agree
        // on both the value and the argmin. This is the test that catches a
        // term charged twice, or charged on the wrong branch.
        let spec = FretboardSpec { frets: 7, ..Default::default() };
        let w = Weights::DEFAULT;
        let mut rng = Lcg(0x5eed_5eed);
        let mut checked = 0;
        for _ in 0..600 {
            let held = rng.set(40, 71, 4);
            let v = solve_cold(&spec, &held);
            if v.truncated {
                continue;
            }
            // Rebuild the working list the way the solver does. Only sets with
            // no folding and no merging are compared, so the brute force stays
            // a few lines rather than a second implementation.
            if held.iter().any(|&p| !reachable(&spec, p)) {
                continue;
            }
            let n = held.len();
            let strings = 6usize;
            let mut best: Option<(i32, [u8; MAX_STRINGS])> = None;
            // Every monotone assignment: for each string, the next note or a mute.
            let mut sel = [MUTED; MAX_STRINGS];
            fn walk(
                st: usize,
                i: usize,
                dropped: i32,
                sel: &mut [u8; MAX_STRINGS],
                held: &[u8],
                spec: &FretboardSpec,
                w: &Weights,
                strings: usize,
                drop_cost: &[i32],
                best: &mut Option<(i32, [u8; MAX_STRINGS])>,
            ) {
                if st == strings {
                    let rest: i32 = drop_cost[i..].iter().sum();
                    let c = dropped
                        + rest
                        + shape_cost_of(spec, w, strings, sel, |_| 0, false, None, |_| 0);
                    let mut tie = [MUTED; MAX_STRINGS];
                    tie[..strings].copy_from_slice(&sel[..strings]);
                    if best.as_ref().is_none_or(|(bc, bt)| (c, tie) < (*bc, *bt)) {
                        *best = Some((c, tie));
                    }
                    return;
                }
                sel[st] = MUTED;
                walk(st + 1, i, dropped, sel, held, spec, w, strings, drop_cost, best);
                let mut skipped = 0;
                for j in i..held.len() {
                    if held[j] >= spec.tuning.open[st] {
                        let f = held[j] - spec.tuning.open[st];
                        if f >= spec.capo && f <= spec.frets {
                            sel[st] = f;
                            walk(
                                st + 1,
                                j + 1,
                                dropped + skipped,
                                sel,
                                held,
                                spec,
                                w,
                                strings,
                                drop_cost,
                                best,
                            );
                        }
                    }
                    skipped += drop_cost[j];
                }
                sel[st] = MUTED;
            }
            let drop_cost: Vec<i32> = (0..n)
                .map(|k| {
                    let mut c = w.drop_base;
                    if k == 0 {
                        c += w.drop_lowest;
                    }
                    if k + 1 == n {
                        c += w.drop_highest;
                    }
                    if held[..k].iter().any(|&q| q % 12 == held[k] % 12) {
                        c += w.drop_doubled;
                    }
                    c
                })
                .collect();
            walk(0, 0, 0, &mut sel, &held, &spec, &w, strings, &drop_cost, &mut best);
            let (bc, bt) = best.expect("the all-muted assignment is always a leaf");
            assert_eq!(v.cost, bc, "cost mismatch for {held:?}");
            let mut got = [MUTED; MAX_STRINGS];
            for (st, s) in v.strings.iter().enumerate() {
                if let StringState::Sounding { fret, .. } = s {
                    got[st] = *fret;
                }
            }
            assert_eq!(got, bt, "argmin mismatch for {held:?}");
            checked += 1;
        }
        assert!(checked > 100, "the sweep skipped almost everything ({checked} checked)");
    }

    // ── the drop policy ─────────────────────────────────────────────────────

    #[test]
    fn a_note_with_nothing_doubling_it_is_never_shed_for_a_prettier_shape() {
        // The theorem the prune and the whole drop tier rest on: the entire
        // spread of shape costs is smaller than the cheapest possible drop of
        // a note whose pitch class sounds nowhere else.
        for (w, strings) in [(Weights::DEFAULT, 6usize), (Weights::BASS, 4)] {
            let spread = w.max_shape_cost(strings) - w.min_shape_cost(strings);
            assert!(
                spread < w.min_unique_drop(),
                "shape spread {spread} has caught up with the drop tier {}",
                w.min_unique_drop()
            );
        }
    }

    #[test]
    fn every_positive_term_really_is_positive() {
        // `min_shape_cost` is only a lower bound while these hold, and the
        // search's prune is only admissible while `min_shape_cost` is a bound.
        let w = Weights::DEFAULT;
        assert!(w.finger_cum.iter().all(|&c| c >= 0));
        assert!(w.stretch.iter().all(|&c| c >= 0));
        assert!(w.barre_setup >= 0 && w.position_cap >= 0 && w.skip_interior >= 0);
        assert!(w.fold_place >= 0 && w.fold_per_extra_octave >= 0);
        assert!(w.finger_cum.windows(2).all(|p| p[1] >= p[0]), "finger cost must not fall");
        assert!(w.stretch.windows(2).all(|p| p[1] >= p[0]), "stretch cost must not fall");
    }

    #[test]
    fn more_notes_than_strings_says_so_instead_of_lying() {
        let spec = std_spec();
        let held = [36u8, 43, 48, 52, 55, 58, 60, 64, 67, 72];
        let v = solve_cold(&spec, &held);
        assert_eq!(v.notes.len(), 10);
        assert_eq!(v.placed().count(), 6);
        assert!(v.shape.dropped_count >= 4);
        let cap = v.caption().expect("a partial board must explain itself");
        assert!(cap.starts_with("6 of 10 notes"), "{cap}");
        // The bass survives; it is what defines the chord.
        assert!(matches!(
            v.notes[0].outcome,
            Outcome::Dropped { reason: DropReason::OctaveMerged }
        ));
        assert!(v.placed().any(|(p, _)| p == 43), "the lowest real note must survive");
    }

    #[test]
    fn an_octave_double_is_the_first_thing_to_go() {
        let spec = std_spec();
        // Open E plus a G4: seven notes, six strings. E4 is already sounding
        // as E2 and E3, so it is the honest one to lose.
        let v = solve_cold(&spec, &[40, 47, 52, 56, 59, 64, 67]);
        assert_eq!(frets(&v), [Some(0), Some(2), Some(2), Some(1), Some(0), Some(3)]);
        let e4 = v.notes.iter().find(|n| n.pitch == 64).unwrap();
        assert_eq!(e4.outcome, Outcome::Dropped { reason: DropReason::Doubled });
        assert_eq!(v.cost, 8305);
    }

    #[test]
    fn notes_that_cannot_sound_together_get_rings_not_silence() {
        let spec = std_spec();
        // 84, 85 and 86 each exist in exactly one place, all on the high E.
        let v = solve_cold(&spec, &[84, 85, 86]);
        assert_eq!(v.placed().count(), 1);
        assert_eq!(v.shape.conflict_count, 2);
        for n in &v.notes {
            if let Outcome::Conflict { wanted } = &n.outcome {
                assert!(!wanted.is_empty());
                assert_eq!(*wanted, candidates(&spec, n.pitch));
            }
        }
    }

    // ── folding ─────────────────────────────────────────────────────────────

    #[test]
    fn a_note_below_the_guitar_becomes_a_ghost_an_octave_up() {
        let spec = std_spec();
        let v = solve_cold(&spec, &[36, 60, 64, 67]);
        assert_eq!(frets(&v), [None, Some(3), None, Some(5), Some(5), Some(3)]);
        let c2 = v.notes.iter().find(|n| n.pitch == 36).unwrap();
        match c2.outcome {
            Outcome::Placed { octave_shift, sounding, pos } => {
                assert_eq!(octave_shift, 1, "a low bass folds UP");
                assert_eq!(sounding, 48);
                assert!(pos.folded);
            }
            ref other => panic!("expected a folded placement, got {other:?}"),
        }
        assert_eq!(v.shape.folded_count, 1);
        assert_eq!(v.caption().as_deref(), Some("1 an octave up"));
        assert_eq!(v.cost, 1397);
    }

    #[test]
    fn a_note_above_a_bass_guitar_becomes_a_ghost_an_octave_down() {
        let spec = FretboardSpec {
            tuning: Tuning::by_name("Bass (4)").unwrap(),
            ..Default::default()
        };
        let v = solve_cold(&spec, &[43, 50, 55, 59, 62, 67]);
        let g4 = v.notes.iter().find(|n| n.pitch == 67).unwrap();
        // G4 is above this board and folds down onto the G3 already held. That
        // G3 is then itself dropped for want of a string, so "merged onto
        // another note" would be a lie — G is still sounding (as G2), and that
        // is what it says.
        assert_eq!(g4.outcome, Outcome::Dropped { reason: DropReason::Doubled });
        assert!(v.placed().count() <= 4);
        assert!(v.placed().all(|(_, p)| p.string < 4));

        // On its own it really does fold down and get drawn.
        let solo = solve_cold(&spec, &[67]);
        match solo.notes[0].outcome {
            Outcome::Placed { octave_shift, .. } => assert_eq!(octave_shift, -1),
            ref other => panic!("expected a folded placement, got {other:?}"),
        }
        assert_eq!(solo.caption().as_deref(), Some("1 an octave down"));
    }

    #[test]
    fn a_merge_is_only_called_a_merge_when_something_took_its_place() {
        let spec = std_spec();
        // C2 folds up onto the C3 that is held, and C3 IS drawn: a real merge.
        let merged = solve_cold(&spec, &[36, 48, 52, 55]);
        assert!(merged.placed().any(|(p, _)| p == 48));
        assert_eq!(
            merged.notes.iter().find(|n| n.pitch == 36).unwrap().outcome,
            Outcome::Dropped { reason: DropReason::OctaveMerged }
        );
        // On a 4-string bass the survivor is itself shed, so the merge is not
        // why the folded note is missing and it must not claim to be.
        let bass = FretboardSpec {
            tuning: Tuning::by_name("Bass (4)").unwrap(),
            ..Default::default()
        };
        let v = solve_cold(&bass, &[43, 50, 55, 59, 62, 67]);
        assert!(!v.placed().any(|(p, _)| p == 55), "the survivor was shed");
        assert_eq!(
            v.notes.iter().find(|n| n.pitch == 67).unwrap().outcome,
            Outcome::Dropped { reason: DropReason::Doubled }
        );
    }

    #[test]
    fn two_notes_folding_onto_one_pitch_keep_the_honest_one() {
        let spec = std_spec();
        // C1 and C2 both fold up; only one C3 can sound.
        let v = solve_cold(&spec, &[24, 36]);
        let placed: Vec<u8> = v.placed().map(|(p, _)| p).collect();
        assert_eq!(placed.len(), 1, "one target, one dot");
        assert_eq!(placed[0], 36, "the smaller fold wins");
        assert_eq!(
            v.notes.iter().find(|n| n.pitch == 24).unwrap().outcome,
            Outcome::Dropped { reason: DropReason::OctaveMerged }
        );
    }

    // ── capo and spec edges ─────────────────────────────────────────────────

    #[test]
    fn a_capo_is_the_new_nut_and_nothing_escapes_behind_it() {
        let spec = FretboardSpec { capo: 3, ..Default::default() };
        let (f, v) = shape(&spec, &[43, 50, 55, 59, 62, 67]);
        assert_eq!(f, [Some(3), Some(5), Some(5), Some(4), Some(3), Some(3)]);
        assert_eq!(v.shape.open_count, 3, "the capo'd strings count as open");
        assert_eq!(v.shape.fingers, 3);
        assert_eq!(v.cost, 143);

        let (f2, v2) = shape(&spec, &[43, 50, 55]);
        assert_eq!(f2, [Some(3), Some(5), Some(5), None, None, None]);
        assert_eq!(v2.shape.barre, Some(Barre { fret: 5, lo_string: 1, hi_string: 2 }));

        let mut rng = Lcg(0xc0ffee);
        for capo in [1u8, 5, 9] {
            let spec = FretboardSpec { capo, ..Default::default() };
            for _ in 0..200 {
                let v = solve_cold(&spec, &rng.set(40, 95, 7));
                assert!(v.placed().all(|(_, p)| p.fret >= capo));
                assert!(v.shape.anchor.is_none_or(|a| a > capo));
            }
        }
    }

    #[test]
    fn an_impossible_board_says_so_rather_than_going_blank() {
        // A capo past the last fret is the one case whose naive answer is an
        // empty board that looks exactly like a crash.
        let spec = FretboardSpec { frets: 22, capo: 24, ..Default::default() };
        let v = solve_cold(&spec, &[60, 64, 67]);
        assert!(v.notes.iter().all(|n| n.outcome == Outcome::Unreachable));
        assert_eq!(v.shape.playability, Playability::Silent);
        assert_eq!(v.caption().as_deref(), Some("capo 24 is past fret 22"));
        assert_eq!(v.strings.len(), 6);
    }

    #[test]
    fn hostile_specs_and_pitches_do_not_panic() {
        const EMPTY: Tuning = Tuning { name: "Nothing", open: &[] };
        const WIDE: Tuning = Tuning {
            name: "Fourteen",
            open: &[20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72],
        };
        let specs = [
            FretboardSpec { frets: 0, ..Default::default() },
            FretboardSpec { frets: 255, ..Default::default() },
            FretboardSpec { frets: 22, capo: 24, ..Default::default() },
            FretboardSpec { frets: 22, capo: 47, ..Default::default() },
            FretboardSpec { frets: 255, capo: 255, ..Default::default() },
            FretboardSpec { tuning: &EMPTY, frets: 22, capo: 0 },
            FretboardSpec { tuning: &WIDE, frets: 22, capo: 0 },
        ];
        for spec in &specs {
            for held in [
                vec![],
                vec![0u8],
                vec![127],
                vec![200],
                vec![0, 127, 200],
                vec![60, 64, 67],
                (40..=70).collect::<Vec<u8>>(),
            ] {
                let v = solve_cold(spec, &held);
                assert_eq!(v.notes.len(), held.len());
                assert_eq!(v.strings.len(), spec.tuning.strings().min(MAX_STRINGS));
                assert_eq!(v.spec_fp, spec_fingerprint(spec));
                let _ = v.caption();
            }
        }
    }

    #[test]
    fn a_pitch_class_the_board_cannot_make_is_unreachable_not_a_panic() {
        // One string, no frets: this board sounds exactly one pitch. Any other
        // pitch CLASS has nowhere to fold to, at any octave.
        const ONE: Tuning = Tuning { name: "One", open: &[60] };
        let spec = FretboardSpec { tuning: &ONE, frets: 0, capo: 0 };
        let v = solve_cold(&spec, &[61]);
        assert_eq!(v.notes[0].outcome, Outcome::Unreachable);
        assert_eq!(v.caption().as_deref(), Some("outside this instrument's range"));
        // Its own pitch class, far out of range, folds home rather than dying.
        let low = solve_cold(&spec, &[12]);
        assert!(matches!(
            low.notes[0].outcome,
            Outcome::Placed { octave_shift: 4, .. }
        ));
    }

    // ── hysteresis ──────────────────────────────────────────────────────────

    #[test]
    fn adding_a_note_does_not_move_the_notes_already_shown() {
        let spec = std_spec();
        let base = solve_cold(&spec, &[48, 52, 55, 60, 64]);
        let hist = History { prev: Some(&base), ..History::NONE };
        let grown = solve(&spec, &[45, 48, 52, 55, 60, 64], &hist, &Weights::DEFAULT);
        for (pitch, pos) in base.placed() {
            assert_eq!(
                grown.position_of(pitch),
                Some(pos),
                "pitch {pitch} moved when a note was added below it"
            );
        }
        assert!(grown.position_of(45).is_some());
    }

    #[test]
    fn inertia_yields_to_physics() {
        let spec = std_spec();
        let alone = solve_cold(&spec, &[60]);
        assert_eq!(alone.position_of(60).unwrap().string, 4);
        let hist = History { prev: Some(&alone), ..History::NONE };
        // 84 exists only at string 5 fret 20, so middle C has to move down.
        let both = solve(&spec, &[60, 84], &hist, &Weights::DEFAULT);
        assert!(both.position_of(60).unwrap().string < 5);
        assert!(both.position_of(84).is_some());
    }

    #[test]
    fn inertia_can_never_hide_a_note_and_never_outweighs_real_improvement() {
        let mut rng = Lcg(0x1111_2222);
        let spec = std_spec();
        let w = Weights::DEFAULT;
        for _ in 0..500 {
            let a = rng.set(40, 90, 6);
            let mut b = a.clone();
            b.push(40 + (rng.next() as u8 % 50));
            b.sort_unstable();
            b.dedup();
            let prev = solve_cold(&spec, &a);
            let hist = History { prev: Some(&prev), ..History::NONE };
            let warm = solve(&spec, &b, &hist, &w);
            let cold = solve_cold(&spec, &b);
            assert!(
                warm.placed().count() >= cold.placed().count(),
                "inertia lost a note: {b:?}"
            );
            // Whatever inertia bought the shape, it is worth at most
            // `hyst_clamp`. Measured with `drop_stick` switched off, because
            // that one lives in the drop tier ON PURPOSE and is outside the
            // clamp: it reorders WHICH of two similar notes is shed, which is
            // the sustain-pedal chatter case, and it is bounded by the drop
            // tier's own margins rather than by this one.
            let no_stick = Weights { drop_stick: 0, ..Weights::DEFAULT };
            let warm2 = solve(&spec, &b, &hist, &no_stick);
            let cold2 = solve(&spec, &b, &History::NONE, &no_stick);
            assert!(
                warm2.cost >= cold2.cost + w.hyst_clamp && warm2.cost <= cold2.cost,
                "inertia is outside its clamp: warm {} vs cold {} for {b:?}",
                warm2.cost,
                cold2.cost
            );
        }
    }

    #[test]
    fn a_solve_is_idempotent_against_its_own_output() {
        let mut rng = Lcg(0x9999_8888);
        let spec = std_spec();
        for _ in 0..300 {
            let held = rng.set(40, 88, 6);
            let once = solve_cold(&spec, &held);
            let hist = History { prev: Some(&once), ..History::NONE };
            let twice = solve(&spec, &held, &hist, &Weights::DEFAULT);
            assert_eq!(frets(&once), frets(&twice), "a settled shape must not drift");
        }
    }

    #[test]
    fn an_unrelated_chord_gets_no_inertia_at_all() {
        let spec = std_spec();
        let prev = solve_cold(&spec, &[45, 52, 57, 61, 64]);
        let hist = History { prev: Some(&prev), ..History::NONE };
        // Disjoint by PITCH, which is the gate: the player's hand moved too.
        let next = [50u8, 54, 59, 62, 66];
        assert!(next.iter().all(|p| !prev.notes.iter().any(|n| n.pitch == *p)));
        assert_eq!(
            solve(&spec, &next, &hist, &Weights::DEFAULT),
            solve_cold(&spec, &next)
        );
        // But sharing even one held pitch is enough to earn inertia.
        let related = [45u8, 52, 57, 61, 66];
        assert!(solve(&spec, &related, &hist, &Weights::DEFAULT).cost < solve_cold(&spec, &related).cost);
    }

    #[test]
    fn a_shape_from_a_different_board_is_ignored() {
        let spec = std_spec();
        let mut prev = solve_cold(&spec, &[48, 52, 55, 60, 64]);
        prev.spec_fp ^= 1; // as if the capo had moved since
        let hist = History { prev: Some(&prev), ..History::NONE };
        let held = [45u8, 48, 52, 55, 60, 64];
        assert_eq!(solve(&spec, &held, &hist, &Weights::DEFAULT), solve_cold(&spec, &held));
    }

    #[test]
    fn the_fingerprint_moves_when_the_board_does() {
        let base = std_spec();
        let fp = spec_fingerprint(&base);
        assert_ne!(fp, spec_fingerprint(&FretboardSpec { capo: 1, ..base.clone() }));
        assert_ne!(fp, spec_fingerprint(&FretboardSpec { frets: 24, ..base.clone() }));
        assert_ne!(
            fp,
            spec_fingerprint(&FretboardSpec {
                tuning: Tuning::by_name("Drop D").unwrap(),
                ..base.clone()
            })
        );
        assert_eq!(fp, spec_fingerprint(&FretboardSpec::default()));
    }

    // ── the session ─────────────────────────────────────────────────────────

    #[test]
    fn holding_a_chord_costs_one_solve_not_one_per_frame() {
        let mut s = VoicingSession::new(std_spec(), Weights::DEFAULT);
        let held: HashSet<u8> = [48u8, 52, 55, 60, 64].into_iter().collect();
        let first = s.update(&held, 100).clone();
        assert_eq!(s.solves(), 1);
        for _ in 0..50 {
            assert_eq!(*s.update(&held, 20), first);
        }
        assert_eq!(s.solves(), 1, "the cache is not doing its job");
    }

    #[test]
    fn letting_go_clears_the_board_and_then_forgets_the_hand() {
        let spec = std_spec();
        let mut s = VoicingSession::new(spec.clone(), Weights::DEFAULT);
        let a: HashSet<u8> = [45u8, 52, 57, 61, 64].into_iter().collect();
        s.update(&a, 100);
        let empty = HashSet::new();
        assert!(s.update(&empty, 100).notes.is_empty(), "release must clear the board");
        // Inside the decay window the hand is still remembered.
        let b: HashSet<u8> = [45u8, 52, 57, 61, 66].into_iter().collect();
        let warm = s.update(&b, 0).clone();

        let mut fresh = VoicingSession::new(spec.clone(), Weights::DEFAULT);
        fresh.update(&a, 100);
        fresh.update(&empty, 5_000); // well past idle_decay_ms
        let cold = fresh.update(&b, 0).clone();
        assert_eq!(cold, solve_cold(&spec, &[45, 52, 57, 61, 66]));
        // Same notes; the only possible difference is the inertia, which is
        // the thing the decay is supposed to have dropped.
        assert_eq!(warm.notes.len(), cold.notes.len());
    }

    #[test]
    fn changing_the_board_throws_away_the_old_hand() {
        let mut s = VoicingSession::new(std_spec(), Weights::DEFAULT);
        let held: HashSet<u8> = [43u8, 50, 55, 59, 62, 67].into_iter().collect();
        s.update(&held, 100);
        let capo = FretboardSpec { capo: 3, ..Default::default() };
        s.set_spec(capo.clone());
        let after = s.update(&held, 100).clone();
        let mut fresh = VoicingSession::new(capo, Weights::DEFAULT);
        let v: Vec<u8> = { let mut v: Vec<u8> = held.iter().copied().collect(); v.sort_unstable(); v };
        assert_eq!(after, *fresh.update_sorted(&v, 100));
    }

    #[test]
    fn a_chord_struck_all_at_once_solves_exactly_as_a_cold_solve_would() {
        // Caught on screen, not in a test: a ten-note voicing drew a different
        // shape in the app than `solve_cold` produced for the same notes,
        // because every note in the first tick was getting its own arrival
        // ordinal in pitch order. The drop policy reads those ordinals as age,
        // so the bass looked like the oldest voice and was shed first.
        let spec = std_spec();
        let mut rng = Lcg(0x4242_4242);
        for _ in 0..200 {
            let held = rng.set(36, 84, 10);
            let mut s = VoicingSession::new(spec.clone(), Weights::DEFAULT);
            let set: HashSet<u8> = held.iter().copied().collect();
            assert_eq!(
                *s.update(&set, 100),
                solve_cold(&spec, &held),
                "a fresh session disagreed with a cold solve on {held:?}"
            );
        }
        // Notes that genuinely arrive later ARE younger, and that still counts.
        let mut s = VoicingSession::new(spec.clone(), Weights::DEFAULT);
        s.update_sorted(&[60, 64], 100);
        s.update_sorted(&[60, 64, 67], 100);
        let mut flat = VoicingSession::new(spec, Weights::DEFAULT);
        flat.update_sorted(&[60, 64, 67], 100);
        assert_eq!(
            frets(flat.current()),
            frets(s.current()),
            "three notes is three notes however they got there"
        );
    }

    #[test]
    fn the_session_never_disagrees_with_a_cold_solve_about_which_notes_show() {
        let mut rng = Lcg(0x7777_3333);
        let spec = std_spec();
        let mut s = VoicingSession::new(spec.clone(), Weights::DEFAULT);
        for _ in 0..400 {
            let held = rng.set(38, 92, 7);
            let set: HashSet<u8> = held.iter().copied().collect();
            let v = s.update(&set, 100);
            assert_eq!(v.notes.len(), held.len());
            assert!(v.placed().count() >= solve_cold(&spec, &held).placed().count());
        }
    }

    // ── cost ────────────────────────────────────────────────────────────────

    #[test]
    fn the_search_stays_small_enough_for_a_frame() {
        // A leaf count, never a wall clock: a time budget goes green on an
        // M-series laptop and red on a tester's machine.
        let spec = std_spec();
        let w = Weights::DEFAULT;
        let cases: [(&[u8], u32); 4] = [
            (&[60, 64, 67], 400),
            (&[45, 52, 55, 60, 64, 69], 1_500),
            (&[36, 43, 48, 52, 55, 58, 60, 64, 67, 72], 4_000),
            (&[48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63], 40_000),
        ];
        for (held, budget) in cases {
            let (_, stats, _) = solve_debug(&spec, held, &History::NONE, &w);
            assert!(stats.leaves <= budget, "{held:?} took {} leaves", stats.leaves);
            let (_, again, _) = solve_debug(&spec, held, &History::NONE, &w);
            assert_eq!(stats, again, "the leaf count must be reproducible");
        }
    }

    #[test]
    fn a_cut_short_search_still_answers_and_says_it_was_cut() {
        let spec = std_spec();
        let w = Weights { leaf_budget: 1, ..Weights::DEFAULT };
        let held = [45u8, 52, 55, 60, 64, 69];
        let a = solve(&spec, &held, &History::NONE, &w);
        assert!(a.truncated);
        assert_eq!(a.notes.len(), 6);
        assert_eq!(a, solve(&spec, &held, &History::NONE, &w), "truncation must be deterministic");
    }

    #[test]
    fn the_probe_reports_runners_up_behind_the_winner() {
        let spec = std_spec();
        let (v, stats, runners) =
            solve_debug(&spec, &[60, 64, 67], &History::NONE, &Weights::DEFAULT);
        assert!(stats.leaves > 0);
        assert!(!runners.is_empty());
        assert!(runners.windows(2).all(|p| p[0].0 <= p[1].0), "runners-up must be best first");
        assert!(runners[0].0 >= v.cost);
        assert!(runners.len() <= 8);
    }

    // ── the taste rows, pinned so a change is visible ────────────────────────

    #[test]
    fn the_calibration_shapes_still_cost_what_they_were_calibrated_to() {
        // These are the rows the weights were tuned against. They are not
        // laws; they are a tripwire, so that turning a dial shows up as an
        // explicit diff here rather than as a shape someone notices in a demo.
        let s = std_spec();
        for (held, want) in [
            (&[40u8, 47, 52, 56, 59, 64][..], 146),
            (&[43, 47, 50, 55, 59, 67][..], 202),
            (&[41, 48, 53, 57, 60, 65][..], 507),
            (&[45, 52, 55, 60, 64, 69][..], 457),
            (&[50, 57, 62, 66][..], 276),
            (&[45, 52, 57, 61, 64][..], 176),
            (&[48, 52, 55, 60, 64][..], 225),
            (&[48, 52, 55, 59][..], 159),
            (&[53, 57, 60, 64][..], 271),
            (&[45, 55, 60, 64][..], 17),
            (&[60, 64, 67][..], 372),
            (&[57, 60, 64, 67][..], 628),
            (&[60, 64, 67, 71][..], 610),
            (&[50, 57, 60, 65][..], 268),
            (&[60][..], 110),
            (&[48, 60, 64, 67][..], 497),
            (&[36, 60, 64, 67][..], 1397),
            (&[36, 43, 64, 67, 72][..], 2221),
            (&[43, 62, 65, 69][..], 718),
            (&[62, 65, 69, 72, 76][..], 1205),
            // Rows that pay the drop tier, where the interesting arithmetic is.
            (&[40, 47, 52, 56, 59, 64, 67][..], 8305),
            (&[84, 85, 86][..], 47_040),
            (&[36, 43, 48, 52, 55, 58, 60, 64, 67, 72][..], 26_081),
            // Six open strings and no fingers: the objective's floor.
            (&[40, 45, 50, 55, 59, 64][..], -330),
        ] {
            assert_eq!(solve_cold(&s, held).cost, want, "cost moved for {held:?}");
        }
    }

    #[test]
    fn a_five_finger_chord_is_shown_as_itself_and_labelled() {
        // A rootless Dm9 genuinely needs five fingers. Showing it honestly on
        // the 12th-fret diagonal beats contorting it into something a hand
        // could hold but no one played.
        let s = std_spec();
        let (f, v) = shape(&s, &[62, 65, 69, 72, 76]);
        assert_eq!(f, [None, Some(17), Some(15), Some(14), Some(13), Some(12)]);
        assert_eq!(v.shape.fingers, 5);
        assert_eq!(v.shape.playability, Playability::TwoHands);
        assert_eq!(v.caption().as_deref(), Some("two hands"));
    }

    #[test]
    fn an_ordinary_chord_says_nothing() {
        let s = std_spec();
        assert_eq!(solve_cold(&s, &[40, 47, 52, 56, 59, 64]).caption(), None);
        assert_eq!(solve_cold(&s, &[]).caption(), None, "an empty board is not an error");
    }
}
