//! Fretboard geometry and position enumeration for the guitar view.
//!
//! Pure: no GUI, no I/O, no egui. The view layer asks "where are the frets" and
//! "where can this pitch be played", and gets numbers back in a 0.0..=1.0
//! coordinate space so the caller can scale to whatever width it has.
//!
//! The hard part of the guitar view is not drawing it, it is that ONE MIDI
//! pitch is playable in up to six places. Deciding which of them to light is
//! the solver's job (see `voicing.rs`); this module's job is to enumerate every
//! candidate honestly and let the solver choose.

/// A tuning is the open (unfretted) MIDI pitch of each string, LOW to HIGH.
/// Index 0 is the lowest-sounding string, which is drawn at the BOTTOM of a
/// standard fretboard diagram, so the view flips this for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tuning {
    pub name: &'static str,
    pub open: &'static [u8],
}

/// Standard first. Anything with a different string count works too: the
/// geometry is driven by `open.len()`, never by a hard-coded 6.
pub const TUNINGS: &[Tuning] = &[
    Tuning { name: "Standard", open: &[40, 45, 50, 55, 59, 64] }, // E A D G B E
    Tuning { name: "Drop D", open: &[38, 45, 50, 55, 59, 64] },
    Tuning { name: "DADGAD", open: &[38, 45, 50, 55, 57, 62] },
    Tuning { name: "Open G", open: &[38, 43, 50, 55, 59, 62] },
    Tuning { name: "Open D", open: &[38, 45, 50, 54, 57, 62] },
    Tuning { name: "Half Step Down", open: &[39, 44, 49, 54, 58, 63] },
    Tuning { name: "Bass (4)", open: &[28, 33, 38, 43] },
];

impl Tuning {
    pub fn standard() -> &'static Tuning {
        &TUNINGS[0]
    }

    pub fn by_name(name: &str) -> Option<&'static Tuning> {
        TUNINGS.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn strings(&self) -> usize {
        self.open.len()
    }
}

/// How the board is laid out and what counts as reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FretboardSpec {
    pub tuning: &'static Tuning,
    /// Highest fret drawn. 22 covers most electrics; 20 is typical acoustic.
    pub frets: u8,
    /// Capo position. 0 means none. A capo at n makes frets 1..n unreachable
    /// and turns fret n into the new open string.
    pub capo: u8,
}

impl Default for FretboardSpec {
    fn default() -> Self {
        Self { tuning: Tuning::standard(), frets: 22, capo: 0 }
    }
}

impl FretboardSpec {
    /// The pitch of string `s` played at `fret`, ignoring reachability.
    pub fn pitch_at(&self, string: usize, fret: u8) -> Option<u8> {
        let open = *self.tuning.open.get(string)?;
        open.checked_add(fret)
    }

    /// The lowest fret a finger may use: the capo, or the nut.
    pub fn lowest_playable_fret(&self) -> u8 {
        self.capo
    }
}

/// One place a pitch can be produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    /// Index into the tuning, 0 = lowest-sounding string.
    pub string: usize,
    /// Absolute fret number. Equal to the capo position for an "open" string
    /// when a capo is fitted, and 0 for a true open string.
    pub fret: u8,
    /// True when this position sounds the right pitch class but in a different
    /// octave than the note actually played. Only ever set when octave folding
    /// is requested, and always reported so the view can draw it differently.
    pub folded: bool,
}

impl Position {
    /// An open string, meaning nothing is fretted behind the capo.
    pub fn is_open(&self, spec: &FretboardSpec) -> bool {
        self.fret == spec.capo
    }
}

/// Fractional x of fret `n`'s WIRE, along a board of length 1.0, using the
/// historical rule of 18 (strictly 17.817…) that every real guitar is built to.
///
/// `fret_x(0)` is the nut at 0.0 and `fret_x(12)` is exactly 0.5, the octave,
/// which is the check that catches an off-by-one in the exponent.
pub fn fret_x(fret: u8) -> f32 {
    1.0 - 2f32.powf(-(fret as f32) / 12.0)
}

/// Where a fingertip actually goes: centred between this fret's wire and the
/// previous one. Drawing dots on the wires themselves reads as wrong to anyone
/// who plays, which is the whole audience for this view.
pub fn press_x(fret: u8) -> f32 {
    if fret == 0 {
        // Behind the nut, where an open-string marker belongs.
        return -0.012;
    }
    (fret_x(fret) + fret_x(fret - 1)) * 0.5
}

/// Fret numbers that carry an inlay on a standard board. 12 and 24 are doubles.
pub const INLAY_FRETS: &[u8] = &[3, 5, 7, 9, 12, 15, 17, 19, 21, 24];

pub fn is_double_inlay(fret: u8) -> bool {
    fret == 12 || fret == 24
}

/// Every place `pitch` can be played on this board.
///
/// Ordered lowest string first, then lowest fret, so the output is stable and
/// the solver's tie-breaking is reproducible run to run. That determinism is
/// load-bearing: a fretboard that shows a different shape each time you play
/// the same chord is worse than useless.
pub fn candidates(spec: &FretboardSpec, pitch: u8) -> Vec<Position> {
    let mut out = Vec::new();
    let lo = spec.lowest_playable_fret();
    for string in 0..spec.tuning.strings() {
        for fret in lo..=spec.frets {
            if spec.pitch_at(string, fret) == Some(pitch) {
                out.push(Position { string, fret, folded: false });
            }
        }
    }
    out
}

/// Every place any octave of `pitch`'s pitch class can be played.
///
/// A guitar's range stops at E2 and a piano's does not, so a bass note played
/// two octaves below the guitar has nowhere to go. Rather than silently drop
/// it, fold it: show the pitch class where it IS reachable and mark it
/// `folded` so the view can render it as a ghost rather than a lie.
pub fn candidates_folded(spec: &FretboardSpec, pitch: u8) -> Vec<Position> {
    let exact = candidates(spec, pitch);
    if !exact.is_empty() {
        return exact;
    }
    let pc = pitch % 12;
    let mut out = Vec::new();
    let lo = spec.lowest_playable_fret();
    for string in 0..spec.tuning.strings() {
        for fret in lo..=spec.frets {
            if spec.pitch_at(string, fret).is_some_and(|p| p % 12 == pc) {
                out.push(Position { string, fret, folded: true });
            }
        }
    }
    out
}

/// The playable pitch range of a board, inclusive.
pub fn range(spec: &FretboardSpec) -> (u8, u8) {
    let lo = spec.lowest_playable_fret();
    let min = spec
        .tuning
        .open
        .iter()
        .map(|&o| o.saturating_add(lo))
        .min()
        .unwrap_or(0);
    let max = spec
        .tuning
        .open
        .iter()
        .map(|&o| o.saturating_add(spec.frets))
        .max()
        .unwrap_or(0);
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelfth_fret_is_the_midpoint() {
        // The rule of 18. If this drifts, every fret on screen is wrong.
        assert!((fret_x(0) - 0.0).abs() < 1e-6);
        assert!((fret_x(12) - 0.5).abs() < 1e-5, "octave must halve the string");
        assert!((fret_x(24) - 0.75).abs() < 1e-5);
        // Monotonic, and each gap narrower than the one below it. Start at 2:
        // a gap needs two predecessors to compare against.
        for f in 1..=24u8 {
            assert!(fret_x(f) > fret_x(f - 1), "fret {f} must sit above {}", f - 1);
        }
        for f in 2..=24u8 {
            let this_gap = fret_x(f) - fret_x(f - 1);
            let prev_gap = fret_x(f - 1) - fret_x(f - 2);
            assert!(this_gap < prev_gap, "fret {f}'s gap must be narrower than fret {}'s", f - 1);
        }
    }

    #[test]
    fn press_points_sit_between_wires_not_on_them() {
        for f in 1..=22u8 {
            let p = press_x(f);
            assert!(p > fret_x(f - 1) && p < fret_x(f), "fret {f} press point escaped its space");
        }
        assert!(press_x(0) < 0.0, "an open marker belongs behind the nut");
    }

    #[test]
    fn middle_c_has_five_places_on_a_standard_board() {
        let spec = FretboardSpec::default();
        let c4 = candidates(&spec, 60);
        let got: Vec<(usize, u8)> = c4.iter().map(|p| (p.string, p.fret)).collect();
        // Low E 20, A 15, D 10, G 5, B 1. The high E string is tuned to 64 and
        // cannot reach 60 at all, which is exactly why one pitch does not mean
        // one place and the solver has to choose.
        assert_eq!(got, vec![(0, 20), (1, 15), (2, 10), (3, 5), (4, 1)]);
        assert!(c4.iter().all(|p| !p.folded));
        assert!(c4.iter().all(|p| spec.pitch_at(p.string, p.fret) == Some(60)));
    }

    #[test]
    fn open_e_low_is_the_only_place_for_e2() {
        let spec = FretboardSpec::default();
        let e2 = candidates(&spec, 40);
        assert_eq!(e2.len(), 1);
        assert_eq!((e2[0].string, e2[0].fret), (0, 0));
        assert!(e2[0].is_open(&spec));
    }

    #[test]
    fn a_capo_removes_the_frets_behind_it() {
        let spec = FretboardSpec { capo: 3, ..Default::default() };
        // Nothing below the capo survives.
        assert!(candidates(&spec, 40).is_empty(), "open low E is under a 3rd-fret capo");
        assert!(candidates(&spec, 41).is_empty());
        // The capo becomes the new open string.
        let g2 = candidates(&spec, 43);
        assert_eq!((g2[0].string, g2[0].fret), (0, 3));
        assert!(g2[0].is_open(&spec), "the capo fret is the new nut");
    }

    #[test]
    fn out_of_range_pitches_fold_and_say_so() {
        let spec = FretboardSpec::default();
        // C2 is below a guitar entirely.
        assert!(candidates(&spec, 36).is_empty());
        let folded = candidates_folded(&spec, 36);
        assert!(!folded.is_empty(), "a low C should still show somewhere");
        assert!(folded.iter().all(|p| p.folded), "folded positions must be flagged");
        assert!(
            folded.iter().all(|p| spec.pitch_at(p.string, p.fret).unwrap() % 12 == 0),
            "every folded position must still be a C"
        );
        // In-range pitches are never folded.
        assert!(candidates_folded(&spec, 60).iter().all(|p| !p.folded));
    }

    #[test]
    fn enumeration_is_ordered_and_deterministic() {
        let spec = FretboardSpec::default();
        for pitch in 40..=88u8 {
            let a = candidates(&spec, pitch);
            let b = candidates(&spec, pitch);
            assert_eq!(a, b, "enumeration must not vary run to run");
            let mut sorted = a.clone();
            sorted.sort();
            assert_eq!(a, sorted, "output must already be in (string, fret) order");
        }
    }

    #[test]
    fn every_tuning_is_well_formed_and_ascending() {
        for t in TUNINGS {
            assert!(t.strings() >= 4, "{}: too few strings", t.name);
            // Not strictly ascending in every tuning, but never descending:
            // Drop D and DADGAD both keep low-to-high order.
            for w in t.open.windows(2) {
                assert!(w[1] > w[0], "{}: strings must run low to high", t.name);
            }
            let spec = FretboardSpec { tuning: t, ..Default::default() };
            let (lo, hi) = range(&spec);
            assert!(hi > lo);
            // Every pitch in range must be playable somewhere, which is the
            // property the view relies on to never silently drop a note.
            for p in lo..=hi {
                assert!(
                    !candidates(&spec, p).is_empty(),
                    "{}: pitch {p} is inside the range but unplayable",
                    t.name
                );
            }
        }
        assert!(Tuning::by_name("standard").is_some(), "lookup is case-insensitive");
        assert!(Tuning::by_name("nonesuch").is_none());
    }
}
