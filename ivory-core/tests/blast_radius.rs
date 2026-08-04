//! Measures how far ONE correction reaches: after training a single voicing,
//! how many of the 13,133 golden-corpus voicings read differently?
//!
//! This is the number that decides whether the learned re-ranker is worth
//! keeping. It is `#[ignore]`d because it is a measurement, not an assertion —
//! run it deliberately:
//!
//!   cargo test -p ivory-core --features learning --test blast_radius --release -- --ignored --nocapture
#![cfg(feature = "learning")]

use ivory_core::{ChordDetector, OverrideStore, TrainOutcome};
use std::collections::HashSet;

fn corpus() -> Vec<Vec<u8>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/golden/corpus.json");
    let text = std::fs::read_to_string(path).expect("corpus.json");
    let rows: serde_json::Value = serde_json::from_str(&text).expect("corpus json");
    rows.as_array()
        .expect("array")
        .iter()
        .filter_map(|r| {
            let notes = r.get("notes")?.as_array()?;
            Some(notes.iter().filter_map(|n| n.as_u64().map(|n| n as u8)).collect())
        })
        .collect()
}

fn read_all(det: &mut ChordDetector, sets: &[Vec<u8>]) -> Vec<Option<String>> {
    sets.iter()
        .map(|s| {
            let set: HashSet<u8> = s.iter().copied().collect();
            det.detect_chord(&set)
        })
        .collect()
}

#[test]
#[ignore = "measurement, not an assertion — see the module doc"]
fn one_correction_blast_radius() {
    let sets = corpus();
    println!("corpus: {} voicings", sets.len());

    let mut path = std::env::temp_dir();
    path.push(format!("ivory-blast-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let mut det = ChordDetector::new().with_overrides(OverrideStore::load_from(&path));
    let before = read_all(&mut det, &sets);

    // The canonical ambiguous case: C-E-G-A reads C6; many players say Am7.
    let target: HashSet<u8> = [60u8, 64, 67, 69].into_iter().collect();
    let outcome = det.train_on_correction(&target, "Am7");
    println!("training C-E-G-A -> Am7: {outcome:?}");
    assert!(matches!(outcome, TrainOutcome::Learned { .. }));

    let after = read_all(&mut det, &sets);

    let changed: Vec<usize> = (0..sets.len()).filter(|&i| before[i] != after[i]).collect();
    println!(
        "\nAFTER ONE CORRECTION: {} of {} voicings read differently ({:.1}%)",
        changed.len(),
        sets.len(),
        100.0 * changed.len() as f64 / sets.len() as f64
    );
    for &i in changed.iter().take(25) {
        println!(
            "   {:?}: {:?} -> {:?}",
            sets[i],
            before[i].as_deref().unwrap_or("-"),
            after[i].as_deref().unwrap_or("-")
        );
    }

    // Forgetting must restore every single reading.
    det.reset_learning();
    let restored = read_all(&mut det, &sets);
    let still_off = (0..sets.len()).filter(|&i| before[i] != restored[i]).count();
    println!("\nafter Forget Learning: {still_off} voicings still differ (want 0)");
    assert_eq!(still_off, 0, "Forget Learning must restore stock readings exactly");

    let _ = std::fs::remove_file(&path);
}
