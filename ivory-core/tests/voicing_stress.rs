//! `solve` is documented as TOTAL: no input, and no hand-written `Weights` or
//! `FretboardSpec` the type system permits, may panic or break the output
//! contract. This is the sweep that says so.
//!
//! It matters because `Weights` is public and its whole purpose is to be
//! TURNED — the module invites the owner to change `position_per_fret` and
//! `finger_cum[5]` while play-testing. A dial set to something silly has to
//! produce a silly shape, not a crash. Debug builds panic on integer overflow,
//! which is exactly the failure this found: `note_slack: usize::MAX` overflowed
//! the pre-cap, and several cost terms could overflow `i32` on a large dial.
//! Release builds would have wrapped instead, which is worse — a cost that
//! wrapped negative makes the solver prefer the shape the dial was meant to
//! discourage.
//!
//! `fast` runs with the suite. `full` is the whole cross product and is
//! `#[ignore]`d because it takes about a minute in debug:
//!
//!     cargo test -p ivory-core --test voicing_stress -- --ignored

use ivory_core::fretboard::{FretboardSpec, Tuning, TUNINGS};
use ivory_core::voicing::{
    solve, solve_cold, History, Outcome, StringState, VoicingSession, Weights, MAX_STRINGS,
};
use std::collections::HashSet;

// Boards nothing in TUNINGS looks like, all reachable through a leaked
// `&'static Tuning`, which is what `FretboardSpec` takes.
static EMPTY: Tuning = Tuning { name: "empty", open: &[] };
static ONE: Tuning = Tuning { name: "one", open: &[60] };
static DUP: Tuning = Tuning { name: "dup", open: &[40, 40, 45, 45] };
static DESC: Tuning = Tuning { name: "desc", open: &[64, 59, 55, 50, 45, 40] };
static WIDE: Tuning = Tuning {
    name: "wide",
    open: &[10, 14, 18, 22, 26, 30, 34, 38, 42, 46, 50, 54, 58, 62, 66, 70, 74, 78, 82, 86],
};
static HIGH: Tuning = Tuning { name: "high", open: &[250, 251, 252] };

fn hostile_tunings() -> Vec<&'static Tuning> {
    let mut v: Vec<&'static Tuning> = TUNINGS.iter().collect();
    v.extend([&EMPTY, &ONE, &DUP, &DESC, &WIDE, &HIGH]);
    v
}

fn specs(full: bool) -> Vec<FretboardSpec> {
    let frets: &[u8] = if full { &[0, 1, 12, 22, 24, 254, 255] } else { &[0, 22, 255] };
    let capos: &[u8] = if full { &[0, 1, 12, 22, 23, 47, 254, 255] } else { &[0, 5, 23, 255] };
    let mut v = Vec::new();
    for t in hostile_tunings() {
        for &f in frets {
            for &c in capos {
                v.push(FretboardSpec { tuning: t, frets: f, capo: c });
            }
        }
    }
    v
}

fn hostile_weights(full: bool) -> Vec<Weights> {
    let d = Weights::DEFAULT;
    let mut v = vec![
        d.clone(),
        Weights::BASS,
        // The one that actually caught a panic.
        Weights { note_slack: usize::MAX, ..d.clone() },
        Weights { leaf_budget: 0, ..d.clone() },
        // Two independent dials the owner can invert, which would make a
        // `clamp()` panic.
        Weights { open_min: 500, open_max: -500, ..d.clone() },
        Weights { position_per_fret: i32::MAX, ..d.clone() },
    ];
    // PAIRS, not just singles. The sweep turned every dial and still missed a
    // real overflow, because the one that bit needed `stretch` and
    // `stretch_scale` hostile in the SAME `Weights` and this list only ever
    // held one apiece. Each alone was green.
    if full {
        v.push(Weights {
            stretch: [i32::MAX; 9],
            stretch_scale: i32::MAX,
            finger_cum: [i32::MAX; 9],
            position_per_fret: i32::MAX,
            position_cap: i32::MAX,
            open_min: i32::MIN,
            open_max: i32::MAX,
            open_slope: i32::MAX,
            fold_place: i32::MAX,
            fold_per_extra_octave: i32::MAX,
            skip_interior: i32::MAX,
            drop_base: i32::MAX,
            recency_per: i32::MAX,
            ..d.clone()
        });
        v.push(Weights {
            stretch: [i32::MIN; 9],
            stretch_scale: i32::MIN,
            finger_cum: [i32::MIN; 9],
            position_per_fret: i32::MIN,
            position_cap: i32::MIN,
            drop_base: i32::MIN,
            hyst_clamp: i32::MIN,
            drop_history_clamp: i32::MAX,
            ..d.clone()
        });
        v.extend([
            Weights { leaf_budget: 1, ..d.clone() },
            Weights { note_slack: 0, ..d.clone() },
            Weights { position_cap: -1, ..d.clone() },
            Weights { position_cap: i32::MAX, ..d.clone() },
            Weights { stretch_scale: -1000, ..d.clone() },
            Weights { stretch_scale: i32::MAX, ..d.clone() },
            Weights { position_high_per_fret: i32::MAX, position_high_fret: 0, ..d.clone() },
            Weights {
                hyst_clamp: 5000,
                sticky_anchor: 9999,
                retain_note: 9999,
                retain_cap: 9999,
                ..d.clone()
            },
            Weights { finger_cum: [-9999; 9], ..d.clone() },
            Weights { stretch: [i32::MAX; 9], ..d.clone() },
            Weights { drop_base: i32::MIN, drop_lowest: i32::MIN, ..d.clone() },
            Weights { fold_place: i32::MAX, fold_per_extra_octave: i32::MAX, ..d.clone() },
            Weights { skip_interior: i32::MAX, ..d.clone() },
            Weights { max_fingers: 0, ..d.clone() },
            Weights { idle_decay_ms: 0, ..d },
        ]);
    }
    v
}

fn note_sets(full: bool) -> Vec<Vec<u8>> {
    let mut v = vec![
        vec![],
        vec![0],
        vec![255],
        vec![0, 127, 255],
        vec![60, 64, 67],
        vec![36, 43, 48, 52, 55, 58, 60, 64, 67, 72],
    ];
    if full {
        // Forty-one held notes is not music, but it is what the pre-cap and
        // the saturating counters exist for. Slow enough to keep out of the
        // default run.
        v.push((0..=20u8).map(|i| i * 12).collect());
        v.push((40..=80).collect());
    }
    v
}

/// Fixed-seed LCG so a failure is reproducible rather than "it went red once".
struct Lcg(u64);
impl Lcg {
    fn n(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 11
    }
}

/// Nothing the player pressed vanishes, nothing lands where it cannot sound,
/// and `strings` and `notes` agree about what is on the board.
fn assert_contract(spec: &FretboardSpec, held: &[u8], v: &ivory_core::Voicing) {
    let mut uniq = held.to_vec();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(v.notes.len(), uniq.len(), "note count for {held:?}");
    assert_eq!(v.strings.len(), spec.tuning.strings().min(MAX_STRINGS));
    let placed = v.placed().count();
    assert_eq!(
        placed
            + v.shape.dropped_count as usize
            + v.shape.conflict_count as usize
            + v.shape.unreachable_count as usize,
        v.notes.len(),
        "every note needs exactly one outcome: {held:?}"
    );
    let mut sounding = 0usize;
    let mut last: Option<u8> = None;
    for st in &v.strings {
        if let StringState::Sounding { pitch, fret, .. } = st {
            sounding += 1;
            assert!(*fret >= spec.capo && *fret <= spec.frets, "fret escaped the board");
            if let Some(p) = last {
                assert!(*pitch > p, "voice crossing in {held:?}");
            }
            last = Some(*pitch);
        }
    }
    assert_eq!(sounding, placed, "strings disagree with notes for {held:?}");
    for n in &v.notes {
        if let Outcome::Placed { pos, sounding, octave_shift } = n.outcome {
            assert_eq!(spec.pitch_at(pos.string, pos.fret), Some(sounding));
            assert_eq!(sounding as i32, n.pitch as i32 + 12 * octave_shift as i32);
            assert_eq!(pos.folded, octave_shift != 0);
        }
    }
    let _ = v.caption();
}

fn sweep(full: bool) {
    let sp = specs(full);
    let ws = hostile_weights(full);
    let mut rng = Lcg(9);

    // Stateless, every combination.
    for spec in &sp {
        for w in &ws {
            for held in note_sets(full) {
                let v = solve(spec, &held, &History::NONE, w);
                assert_contract(spec, &held, &v);
                assert_eq!(v, solve(spec, &held, &History::NONE, w), "not deterministic");
                // And warm, against its own output.
                let h = History { prev: Some(&v), ..History::NONE };
                assert_contract(spec, &held, &solve(spec, &held, &h, w));
            }
        }
    }

    // Sessions, with the board and the dials changing mid-flight.
    for _ in 0..(if full { 400 } else { 60 }) {
        let spec = sp[rng.n() as usize % sp.len()].clone();
        let w = ws[rng.n() as usize % ws.len()].clone();
        let mut s = VoicingSession::new(spec.clone(), w);
        let mut live = spec;
        for _ in 0..12 {
            let len = rng.n() as usize % 14;
            let held: HashSet<u8> = (0..len).map(|_| rng.n() as u8).collect();
            let sorted: Vec<u8> = {
                let mut v: Vec<u8> = held.iter().copied().collect();
                v.sort_unstable();
                v
            };
            assert_contract(&live, &sorted, s.update(&held, (rng.n() % 5000) as u32));
            match rng.n() % 7 {
                0 => {
                    live = sp[rng.n() as usize % sp.len()].clone();
                    s.set_spec(live.clone());
                }
                1 => s.set_weights(ws[rng.n() as usize % ws.len()].clone()),
                2 => s.reset(),
                _ => {}
            }
        }
    }

    // Random input on the shipped boards, which is what users will actually hit.
    for _ in 0..(if full { 3000 } else { 600 }) {
        let spec = FretboardSpec {
            tuning: &TUNINGS[rng.n() as usize % TUNINGS.len()],
            frets: 22,
            capo: (rng.n() % 8) as u8,
        };
        let mut held: Vec<u8> = (0..(rng.n() as usize % 12 + 1))
            .map(|_| (rng.n() % 128) as u8)
            .collect();
        held.sort_unstable();
        held.dedup();
        assert_contract(&spec, &held, &solve_cold(&spec, &held));
    }
}

#[test]
fn fast() {
    sweep(false);
}

#[test]
#[ignore = "the whole cross product; about a minute in debug"]
fn full() {
    sweep(true);
}
