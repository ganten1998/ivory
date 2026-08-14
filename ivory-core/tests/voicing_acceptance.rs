//! Acceptance contract for the fretboard voicing solver.
//!
//! This file is to `voicing.rs` what `acceptance.rs` is to the chord engine:
//! the shapes are the contract, and the module's own unit tests are the
//! machinery that keeps them honest.
//!
//! Two things distinguish it from the engine's contract, and both are worth
//! saying out loud:
//!
//! * **These shapes are calibrated, not validated.** There is no corpus for
//!   fretboard shapes the way there is one for chord names. Every row was
//!   executed against the real `fretboard.rs` candidate tables, but none of it
//!   has been measured against a guitarist. When play-testing disagrees with a
//!   row, the row is what changes — after the dial that moved it is written
//!   down in `docs/DIVERGENCES.md`.
//! * **A red row here is a TASTE change, not a bug.** Turning any weight in
//!   `Weights::DEFAULT` will light some of these up. That is the point: it
//!   makes the blast radius of a dial visible as a diff instead of as a shape
//!   somebody notices in a demo three weeks later.
//!
//! Shapes are written the way a guitarist writes them, low string first, with
//! `X` for a string that does not sound. Note numbers are MIDI (middle C = 60).

use ivory_core::fretboard::{FretboardSpec, Tuning};
use ivory_core::voicing::{solve_cold, DropReason, Outcome, Playability, StringState, Voicing};

/// `None` is a muted string.
type Shape = &'static [Option<u8>];

const X: Option<u8> = None;
const fn f(n: u8) -> Option<u8> {
    Some(n)
}

/// (held notes, tuning name, capo, expected shape, expected cost, tag)
type Row = (&'static [u8], &'static str, u8, Shape, i32, &'static str);

fn shape_of(v: &Voicing) -> Vec<Option<u8>> {
    v.strings
        .iter()
        .map(|s| match s {
            StringState::Sounding { fret, .. } => Some(*fret),
            _ => None,
        })
        .collect()
}

fn spec_for(tuning: &str, capo: u8) -> FretboardSpec {
    FretboardSpec {
        tuning: Tuning::by_name(tuning).unwrap_or_else(|| panic!("no tuning named {tuning}")),
        frets: 22,
        capo,
    }
}

/// Collect-then-panic, like the engine's table: one run reports every failing
/// row rather than stopping at the first.
fn run_table(label: &str, rows: &[Row]) {
    let mut failures: Vec<String> = Vec::new();
    for (held, tuning, capo, want_shape, want_cost, tag) in rows {
        let spec = spec_for(tuning, *capo);
        let v = solve_cold(&spec, held);
        let got = shape_of(&v);
        if got != *want_shape {
            failures.push(format!(
                "  [{tag}] {held:?} -> shape {got:?}, expected {want_shape:?}"
            ));
        } else if v.cost != *want_cost {
            failures.push(format!(
                "  [{tag}] {held:?} -> right shape, cost {} not {want_cost}",
                v.cost
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{label}: {} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The shapes a guitarist would recognise without being told what they are.
// ─────────────────────────────────────────────────────────────────────────────
const OPEN_SHAPES: &[Row] = &[
    (&[40, 47, 52, 56, 59, 64], "Standard", 0, &[f(0), f(2), f(2), f(1), f(0), f(0)], 146, "open E"),
    (&[43, 47, 50, 55, 59, 67], "Standard", 0, &[f(3), f(2), f(0), f(0), f(0), f(3)], 202, "open G"),
    (&[45, 52, 57, 61, 64], "Standard", 0, &[X, f(0), f(2), f(2), f(2), f(0)], 176, "open A"),
    (&[50, 57, 62, 66], "Standard", 0, &[X, X, f(0), f(2), f(3), f(2)], 276, "open D"),
    (&[48, 52, 55, 60, 64], "Standard", 0, &[X, f(3), f(2), f(0), f(1), f(0)], 225, "open C"),
    (&[48, 52, 55, 59], "Standard", 0, &[X, f(3), f(2), f(0), f(0), X], 159, "open Cmaj7"),
    (&[53, 57, 60, 64], "Standard", 0, &[X, X, f(3), f(2), f(1), f(0)], 271, "Fmaj7"),
    (&[45, 55, 60, 64], "Standard", 0, &[X, f(0), X, f(0), f(1), f(0)], 17, "Am7 with three open strings"),
];

const BARRES: &[Row] = &[
    (&[41, 48, 53, 57, 60, 65], "Standard", 0, &[f(1), f(3), f(3), f(2), f(1), f(1)], 507, "F barre at 1"),
    (&[45, 52, 55, 60, 64, 69], "Standard", 0, &[f(5), f(7), f(5), f(5), f(5), f(5)], 457, "Am7 barre at 5"),
    (&[50, 57, 60, 65], "Standard", 0, &[X, X, f(0), f(2), f(1), f(1)], 268, "Dm7 mini-barre"),
];

const CLOSE_VOICINGS: &[Row] = &[
    (&[60], "Standard", 0, &[X, X, X, X, f(1), X], 110, "middle C, five places, take the low one"),
    (&[60, 64, 67], "Standard", 0, &[X, X, X, f(5), f(5), f(3)], 372, "C triad, piano register"),
    (&[57, 60, 64, 67], "Standard", 0, &[X, X, f(7), f(5), f(5), f(3)], 628, "Am7 close"),
    (&[60, 64, 67, 71], "Standard", 0, &[X, X, f(10), f(9), f(8), f(7)], 610, "Cmaj7 close"),
    (&[48, 60, 64, 67], "Standard", 0, &[X, f(3), X, f(5), f(5), f(3)], 497, "C3 under a C triad"),
    (&[43, 62, 65, 69], "Standard", 0, &[f(3), X, X, f(7), f(6), f(5)], 718, "fretted bass under a triad, all four shown"),
    (&[64, 65], "Standard", 0, &[X, X, X, f(9), f(6), X], 424, "a semitone apart, 11 points from the alternative"),
];

const CAPO: &[Row] = &[
    (&[43, 50, 55, 59, 62, 67], "Standard", 3, &[f(3), f(5), f(5), f(4), f(3), f(3)], 143, "capo 3 does the barring"),
    (&[43, 50, 55], "Standard", 3, &[f(3), f(5), f(5), X, X, X], 213, "capo 3, mini-barre above it"),
];

const OTHER_INSTRUMENTS: &[Row] = &[
    (&[57, 58], "DADGAD", 0, &[X, X, X, f(2), f(1), X], 204, "DADGAD semitone, monotone answer exists"),
    // Six open strings and not a finger anywhere: the cheapest shape the
    // objective can express, and the floor `Weights::min_shape_cost` promises.
    (&[38, 43, 50, 55, 59, 62], "Open G", 0, &[f(0), f(0), f(0), f(0), f(0), f(0)], -330, "Open G played open"),
];

#[test]
fn open_shapes() {
    run_table("open shapes", OPEN_SHAPES);
}

#[test]
fn barres() {
    run_table("barres", BARRES);
}

#[test]
fn close_voicings() {
    run_table("close voicings", CLOSE_VOICINGS);
}

#[test]
fn capo() {
    run_table("capo", CAPO);
}

#[test]
fn other_instruments() {
    run_table("other instruments", OTHER_INSTRUMENTS);
}

// ─────────────────────────────────────────────────────────────────────────────
// Where the picture stops being a plain chord diagram. These rows are the ones
// the VIEW has to render specially, so each asserts the reason as well as the
// shape: a hollow ring, a ghost, and a struck-through chip are three different
// pictures and the solver is what tells them apart.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_bass_below_the_guitar_becomes_a_ghost_rather_than_disappearing() {
    let spec = spec_for("Standard", 0);
    let v = solve_cold(&spec, &[36, 60, 64, 67]);
    assert_eq!(shape_of(&v), [X, f(3), X, f(5), f(5), f(3)]);
    assert_eq!(v.cost, 1397);
    let c2 = v.notes.iter().find(|n| n.pitch == 36).unwrap();
    assert!(
        matches!(c2.outcome, Outcome::Placed { octave_shift: 1, sounding: 48, .. }),
        "C2 should fold UP one octave, got {:?}",
        c2.outcome
    );
    assert_eq!(v.caption().as_deref(), Some("1 an octave up"));
}

#[test]
fn a_two_handed_voicing_shows_every_note_and_admits_it_needs_two_hands() {
    let spec = spec_for("Standard", 0);
    let v = solve_cold(&spec, &[36, 43, 64, 67, 72]);
    assert_eq!(shape_of(&v), [f(3), f(3), X, f(9), f(8), f(8)]);
    assert_eq!(v.cost, 2221);
    assert_eq!(v.placed().count(), 5, "nothing is dropped: there are strings for all of it");
    assert_eq!(v.shape.playability, Playability::Stretch);
}

#[test]
fn a_five_finger_chord_is_drawn_honestly_rather_than_contorted() {
    // Every design in the field put this at frets 17 to 22. It belongs on the
    // 12th-fret diagonal, and it really does need five fingers.
    let spec = spec_for("Standard", 0);
    let v = solve_cold(&spec, &[62, 65, 69, 72, 76]);
    assert_eq!(shape_of(&v), [X, f(17), f(15), f(14), f(13), f(12)]);
    assert_eq!(v.cost, 1205);
    assert_eq!(v.shape.fingers, 5);
    assert_eq!(v.shape.playability, Playability::TwoHands);
}

#[test]
fn notes_that_cannot_sound_together_are_rings_not_absences() {
    // 84, 85 and 86 each exist in exactly one place, all three on the high E.
    let spec = spec_for("Standard", 0);
    let v = solve_cold(&spec, &[84, 85, 86]);
    assert_eq!(shape_of(&v), [X, X, X, X, X, f(20)]);
    assert_eq!(v.cost, 47040);
    assert_eq!(v.placed().count(), 1);
    assert_eq!(v.shape.conflict_count, 2);
    for n in &v.notes {
        if let Outcome::Conflict { wanted } = &n.outcome {
            assert_eq!(wanted.len(), 1, "each of these has exactly one home");
            assert_eq!(wanted[0].string, 5);
        }
    }
}

#[test]
fn a_ten_note_piano_voicing_keeps_the_bass_and_the_melody_and_says_what_it_lost() {
    let spec = spec_for("Standard", 0);
    let v = solve_cold(&spec, &[36, 43, 48, 52, 55, 58, 60, 64, 67, 72]);
    assert_eq!(shape_of(&v), [f(3), f(3), f(2), f(3), f(5), f(8)]);
    // 2081 of shape and three 8000-point octave doubles: the drop tier is two
    // orders of magnitude above the shape terms, which is what stops a nicer
    // hand position from ever being worth a note.
    //
    // This read 18081 while `note_slack` was 2, because the pre-cap removed a
    // note before the search and its drop was never charged to anything. The
    // SHAPE is identical either way; raising the slack simply made the
    // objective account for every note the player pressed. A cost is only
    // comparable within one `Weights`, which is exactly why it is not shown to
    // anyone.
    assert_eq!(v.cost, 26081);
    assert_eq!(v.notes.len(), 10, "every key pressed is still accounted for");
    assert_eq!(v.placed().count(), 6);
    assert_eq!(v.caption().as_deref(), Some("6 of 10 notes  \u{b7}  two hands"));
    // The bass note folded onto a C that was already held, so it merges rather
    // than being drawn twice.
    assert_eq!(
        v.notes[0].outcome,
        Outcome::Dropped { reason: DropReason::OctaveMerged }
    );
    assert!(v.placed().any(|(p, _)| p == 43), "the lowest real note survives");
    assert!(v.placed().any(|(p, _)| p == 72), "so does the melody");
    // Everything shed is an octave of something still sounding.
    for n in &v.notes {
        if let Outcome::Dropped { reason } = n.outcome {
            assert!(
                matches!(reason, DropReason::Doubled | DropReason::OctaveMerged),
                "{} was shed for {reason:?}, which is not an octave double",
                n.pitch
            );
        }
    }
}

#[test]
fn a_piano_chord_on_a_four_string_bass_is_honest_about_being_a_bass() {
    let spec = FretboardSpec {
        tuning: Tuning::by_name("Bass (4)").unwrap(),
        frets: 22,
        capo: 0,
    };
    let v = solve_cold(&spec, &[43, 50, 55, 59, 62, 67]);
    assert_eq!(v.notes.len(), 6);
    assert!(v.placed().count() <= 4, "four strings, at most four notes");
    assert!(v.placed().all(|(_, p)| p.string < 4));
    // G4 is above this instrument entirely and folds down onto the G3 already
    // held rather than being invented somewhere. That G3 is then shed too for
    // want of a fourth string, so the reason given is the one that is still
    // true: G is sounding, as G2.
    assert_eq!(
        v.notes.iter().find(|n| n.pitch == 67).unwrap().outcome,
        Outcome::Dropped { reason: DropReason::Doubled }
    );
    assert!(v.caption().is_some_and(|c| c.starts_with("4 of 6 notes")));
}

#[test]
fn a_capo_past_the_last_fret_says_so_instead_of_going_blank() {
    // The one impossible board whose naive answer is indistinguishable from a
    // crash. It has to name its own cause.
    let spec = FretboardSpec {
        tuning: Tuning::standard(),
        frets: 22,
        capo: 24,
    };
    let v = solve_cold(&spec, &[60, 64, 67]);
    assert_eq!(v.notes.len(), 3);
    assert!(v.notes.iter().all(|n| n.outcome == Outcome::Unreachable));
    assert_eq!(v.strings.len(), 6, "the board is still drawn, just slashed");
    assert_eq!(v.caption().as_deref(), Some("capo 24 is past fret 22"));
}
