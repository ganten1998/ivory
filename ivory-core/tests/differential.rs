//! Differential self-consistency guard.
//!
//! `tests/golden/rust-golden.json` is the frozen regression baseline: the
//! current engine's `detect_chord` output (flat + sharp) for all 13,133
//! corpus note-sets. This test re-runs the engine over those note-sets and
//! asserts the output is byte-identical to the baseline. Any change in engine
//! behavior — intended or not — turns this red, forcing the baseline to be
//! regenerated deliberately:
//!
//!   cargo run -p ivory-core --example diffcorpus --release -- \
//!       --json tests/golden/rust-golden.json
//!
//! This is NOT the Python differential (that lives in tests/golden/, diffed by
//! examples/diffcorpus.rs and classified by tests/golden/classify.py). It only
//! pins the engine to its own audited snapshot.
//!
//! `fast_subset` (first 1500 rows) runs by default; `full` (#[ignore]) covers
//! every row and is run explicitly (`cargo test -p ivory-core -- --ignored`).

use ivory_core::ChordDetector;
use std::collections::HashSet;

const GOLDEN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/golden/rust-golden.json");

fn detect(notes: &[u8], prefer_flats: bool) -> Option<String> {
    let mut d = ChordDetector::new();
    d.set_note_preference(prefer_flats);
    let set: HashSet<u8> = notes.iter().copied().collect();
    d.detect_chord(&set)
}

fn load() -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(GOLDEN)
        .expect("read tests/golden/rust-golden.json (regenerate via example diffcorpus)");
    serde_json::from_str::<serde_json::Value>(&raw)
        .expect("parse rust-golden.json")
        .as_array()
        .expect("rust-golden.json is an array")
        .clone()
}

/// Check `rows[start..end]` against the frozen baseline; return mismatch count.
fn check(rows: &[serde_json::Value], start: usize, end: usize) -> usize {
    let mut mismatches = 0usize;
    for row in &rows[start..end] {
        let notes: Vec<u8> = row["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u8)
            .collect();
        let want_flat = row["flat"].as_str();
        let want_sharp = row["sharp"].as_str();
        let got_flat = detect(&notes, true);
        let got_sharp = detect(&notes, false);
        if got_flat.as_deref() != want_flat || got_sharp.as_deref() != want_sharp {
            if mismatches < 20 {
                eprintln!(
                    "DIFFERENTIAL REGRESSION {:?}\n  flat : want {:?}  got {:?}\n  sharp: want {:?}  got {:?}",
                    notes, want_flat, got_flat, want_sharp, got_sharp
                );
            }
            mismatches += 1;
        }
    }
    mismatches
}

/// Runs in CI by default: first 1500 rows (covers edge/pattern-heavy prefix).
#[test]
fn fast_subset() {
    let rows = load();
    let end = rows.len().min(1500);
    let n = check(&rows, 0, end);
    assert_eq!(n, 0, "{n} engine outputs diverged from rust-golden.json (fast subset)");
}

/// Full 13,133-row sweep. `#[ignore]` (both prefs over the whole corpus is the
/// slow path); run with `cargo test -p ivory-core -- --ignored`.
#[test]
#[ignore]
fn full() {
    let rows = load();
    let len = rows.len();
    let n = check(&rows, 0, len);
    assert_eq!(n, 0, "{n} engine outputs diverged from rust-golden.json (full corpus)");
}
