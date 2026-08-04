//! Integration tests for the learned re-ranker's *user-facing* contract: what
//! `train_on_correction` reports back, and that it never trains into a silent
//! void. Unit-level perceptron tests live in `src/overrides.rs`.
#![cfg(feature = "learning")]

use ivory_core::{ChordDetector, OverrideStore, TrainOutcome};
use std::collections::HashSet;
use std::path::PathBuf;

fn notes(v: &[u8]) -> HashSet<u8> {
    v.iter().copied().collect()
}

/// Every test gets its own store path — never touch the real overrides.json.
fn detector(name: &str) -> ChordDetector {
    let mut p = std::env::temp_dir();
    p.push(format!("ivory-learn-it-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_file(&p);
    ChordDetector::new().with_overrides(OverrideStore::load_from(p))
}

fn store_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("ivory-learn-it-{}-{}", std::process::id(), name));
    p
}

/// The regression this whole API shape exists for: E-G-C *displays* as "C/E",
/// but "C/E" is never a scored candidate — the slash is added after scoring.
/// Training used to look for the displayed name among the candidates, fail to
/// find it, and return having done nothing at all.
#[test]
fn slash_renamed_winner_still_trains() {
    let mut det = detector("slash");
    let set = notes(&[64, 67, 72]);
    assert_eq!(det.detect_chord(&set).as_deref(), Some("C/E"));

    let cands: Vec<String> = det
        .trainable_candidates(&set)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(
        cands.iter().all(|c| c != "C/E"),
        "the displayed label must not appear in the trainable list: {cands:?}"
    );
    assert!(cands.contains(&"Em".to_owned()), "got {cands:?}");

    match det.train_on_correction(&set, "Em") {
        TrainOutcome::Learned { now_reads, .. } => {
            assert!(now_reads.starts_with("Em"), "now reads {now_reads:?}");
            assert!(det
                .detect_chord(&set)
                .is_some_and(|l| l.starts_with("Em")));
        }
        other => panic!("expected Learned, got {other:?}"),
    }
}

/// Picking the name that already wins is reported as such, and carries the
/// displayed label so the UI can explain a post-processing rename.
#[test]
fn already_correct_reports_the_displayed_name() {
    let mut det = detector("already");
    let set = notes(&[64, 67, 72]);
    match det.train_on_correction(&set, "C") {
        TrainOutcome::AlreadyCorrect { displays_as } => assert_eq!(displays_as, "C/E"),
        other => panic!("expected AlreadyCorrect, got {other:?}"),
    }
    // Nothing was learned, so nothing was counted.
    assert_eq!(det.overrides().unwrap().corrections(), 0);
    assert!(!det.overrides().unwrap().has_learned());
}

/// A close call flips, and says how it now reads.
#[test]
fn close_call_flips_in_one_correction() {
    let mut det = detector("close");
    let set = notes(&[60, 64, 67, 69]); // C6 (200) vs Am7 (180)
    assert_eq!(det.detect_chord(&set).as_deref(), Some("C6"));
    match det.train_on_correction(&set, "Am7") {
        TrainOutcome::Learned { steps, now_reads } => {
            assert!(steps > 0 && steps <= 25, "steps {steps}");
            assert!(now_reads.starts_with("Am7"), "now reads {now_reads:?}");
        }
        other => panic!("expected Learned, got {other:?}"),
    }
    assert_eq!(det.overrides().unwrap().corrections(), 1);
}

/// A landslide cannot be flipped by a bounded nudge — and the user is told,
/// rather than being left to wonder why nothing happened.
#[test]
fn unclosable_gap_reports_stubborn() {
    let mut det = detector("stubborn");
    let set = notes(&[55, 59, 62, 65]); // G7 at 755 vs Bm at 83
    match det.train_on_correction(&set, "Bm") {
        TrainOutcome::Stubborn { still_reads, .. } => assert_eq!(still_reads, "G7"),
        other => panic!("expected Stubborn, got {other:?}"),
    }
}

/// When a post-scoring rule (scale detection, rootless-dominant renaming) owns
/// the displayed name, the user's pick can already top the scoring and still
/// never show. That must NOT be reported as "scores too far behind" — there is
/// no gap at all.
#[test]
fn post_scoring_rule_is_reported_as_such_not_as_a_score_gap() {
    let mut det = detector("outranked");

    // A scale reading is decided AFTER the scoring loop and throws every
    // candidate away, so nothing is offered as trainable at all — otherwise the
    // picker lists readings that can never win and each attempt burns the full
    // 25-step budget before blaming a score gap that does not exist.
    let ionian = notes(&[60, 62, 64, 65, 67, 69, 71]);
    assert_eq!(det.detect_chord(&ionian).as_deref(), Some("C Ionian"));
    assert!(
        det.trainable_candidates(&ionian).is_empty(),
        "scale readings must offer nothing to re-rank"
    );
    match det.train_on_correction(&ionian, "CΔ13") {
        TrainOutcome::OutrankedByRule { displays_as, .. } => assert_eq!(displays_as, "C Ionian"),
        other => panic!("expected OutrankedByRule for a scale, got {other:?}"),
    }
    // A chord reading in the same session still offers candidates.
    assert!(det.trainable_candidates(&notes(&[60, 64, 67, 69])).len() >= 2);

    // E-Bb-D displays as C9 via the D9 rootless-dominant rename; the scored
    // candidate behind it is E7(#11).
    let rootless = notes(&[64, 70, 74]);
    assert_eq!(det.detect_chord(&rootless).as_deref(), Some("C9"));
    match det.train_on_correction(&rootless, "E7(#11)") {
        TrainOutcome::OutrankedByRule { displays_as, .. } => assert_eq!(displays_as, "C9"),
        other => panic!("expected OutrankedByRule, got {other:?}"),
    }

    // Neither attempt may leave learned state behind.
    assert!(!det.overrides().unwrap().has_learned());
    assert_eq!(det.overrides().unwrap().corrections(), 0);
}

/// A name that was never a candidate is rejected up front.
#[test]
fn unknown_label_is_not_trainable() {
    let mut det = detector("unknown");
    let set = notes(&[60, 64, 67]);
    assert_eq!(
        det.train_on_correction(&set, "Xyzzy9"),
        TrainOutcome::NotTrainable
    );
}

/// An exact override short-circuits before scoring, so there is nothing to
/// re-rank — and the override keeps winning.
#[test]
fn override_makes_the_voicing_untrainable() {
    let mut det = detector("pinned");
    let set = notes(&[60, 64, 67, 69]);
    det.overrides_mut().unwrap().teach(&set, "PINNED", false);
    assert_eq!(
        det.train_on_correction(&set, "Am7"),
        TrainOutcome::NotTrainable
    );
    assert_eq!(det.detect_chord(&set).as_deref(), Some("PINNED"));
}

/// Learning state survives a restart, and Forget clears both weights and count.
#[test]
fn state_persists_and_resets() {
    let path = store_path("persist");
    let _ = std::fs::remove_file(&path);
    {
        let mut det = ChordDetector::new().with_overrides(OverrideStore::load_from(&path));
        let set = notes(&[60, 64, 67, 69]);
        assert!(matches!(
            det.train_on_correction(&set, "Am7"),
            TrainOutcome::Learned { .. }
        ));
    }
    let mut det = ChordDetector::new().with_overrides(OverrideStore::load_from(&path));
    assert!(det.learning_mode(), "learning mode should persist");
    assert_eq!(det.overrides().unwrap().corrections(), 1);
    assert!(det.overrides().unwrap().has_learned());
    assert!(det
        .detect_chord(&notes(&[60, 64, 67, 69]))
        .is_some_and(|l| l.starts_with("Am7")));

    det.reset_learning();
    assert_eq!(det.overrides().unwrap().corrections(), 0);
    assert!(!det.overrides().unwrap().has_learned());
    assert_eq!(
        det.detect_chord(&notes(&[60, 64, 67, 69])).as_deref(),
        Some("C6"),
        "forgetting restores the stock reading"
    );
    let _ = std::fs::remove_file(&path);
}

/// The re-ranker may REORDER candidates; it must never ELIMINATE them.
///
/// Regression: candidate admission used a `score > 0.0` floor applied to the
/// score *after* the learned adjustment (bounded at -100). A few ordinary
/// corrections could therefore drag every candidate for some voicing to <= 0,
/// leaving `best_match = None` — the chord strip went blank on chords the stock
/// engine names perfectly well. Admission now uses the unadjusted score.
#[test]
fn training_never_makes_a_chord_nameless() {
    let mut det = detector("nameless");

    // A grid of ordinary voicings, plus the exact one that used to vanish.
    let mut probes: Vec<Vec<u8>> = vec![
        vec![59, 71, 82], // B + B + A# — reads BΔ7(#11) stock, used to go blank
        vec![41, 65, 76],
        vec![36, 59, 60],
        vec![39, 51, 63, 74],
    ];
    for root in 48u8..60 {
        probes.push(vec![root, root + 4, root + 7]);
        probes.push(vec![root, root + 3, root + 7, root + 10]);
        probes.push(vec![root, root + 12, root + 23]);
        probes.push(vec![root, root + 4, root + 7, root + 11]);
    }

    let stock: Vec<Option<String>> = probes
        .iter()
        .map(|p| det.detect_chord(&notes(p)))
        .collect();

    // Three legitimate corrections, each picked from the real candidate list.
    for (set, pick) in [
        (vec![55u8, 72, 83], "C5"),
        (vec![73, 75, 76], "Dbm(add9)"),
        (vec![53, 65, 72, 82], "Bb2"),
    ] {
        let s = notes(&set);
        if det.trainable_candidates(&s).iter().any(|(n, _)| n == pick) {
            let _ = det.train_on_correction(&s, pick);
        }
    }
    assert!(det.overrides().unwrap().has_learned(), "nothing was learned");

    for (i, p) in probes.iter().enumerate() {
        let after = det.detect_chord(&notes(p));
        if stock[i].is_some() {
            assert!(
                after.is_some(),
                "voicing {p:?} read {:?} stock but is nameless after training",
                stock[i]
            );
        }
    }
}

/// The master switch silences the re-ranker without discarding what it learned.
#[test]
fn learning_mode_off_restores_stock_reading() {
    let mut det = detector("switch");
    let set = notes(&[60, 64, 67, 69]);
    assert!(matches!(
        det.train_on_correction(&set, "Am7"),
        TrainOutcome::Learned { .. }
    ));
    det.set_learning_mode(false);
    assert_eq!(det.detect_chord(&set).as_deref(), Some("C6"));
    assert!(det.overrides().unwrap().has_learned(), "weights are kept");
    det.set_learning_mode(true);
    assert!(det.detect_chord(&set).is_some_and(|l| l.starts_with("Am7")));
}
