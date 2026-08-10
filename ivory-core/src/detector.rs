use std::collections::{HashMap, HashSet};
use crate::patterns::*;
use crate::overrides::OverrideStore;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Semitone interval between two pitch-classes (0..=11), always returns 0..=11.
#[inline]
fn pc_interval(from: u8, to: u8) -> u8 {
    ((to as i32 - from as i32).rem_euclid(12)) as u8
}

/// Sorted unique pitch-classes from a set of MIDI notes.
fn pitch_classes(notes: &HashSet<u8>) -> Vec<u8> {
    let mut pcs: Vec<u8> = notes.iter().map(|&n| n % 12).collect::<HashSet<_>>().into_iter().collect();
    pcs.sort_unstable();
    pcs
}

/// Does the displayed chord label represent the candidate `correct`? True for
/// the name itself and for its slash form, which the post-scoring step appends
/// when the bass is not the root ("Am7" is shown by "Am7" and by "Am7/C").
#[cfg(feature = "learning")]
fn label_reflects(label: Option<&str>, correct: &str) -> bool {
    label.is_some_and(|l| {
        l == correct || (l.starts_with(correct) && l[correct.len()..].starts_with('/'))
    })
}

/// Is `name` a basic chord (just note + optional accidental + optional 'm')?
/// Equivalent to the Python regex `^[A-G][b#]?m?$`.
fn is_basic_chord(name: &str) -> bool {
    let s = if let Some(pos) = name.find('/') { &name[..pos] } else { name };
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) if ('A'..='G').contains(&c) => c,
        _ => return false,
    };
    let _ = first; // used implicitly
    let rest: &str = &s[1..];
    matches!(rest, "" | "b" | "#" | "m" | "bm" | "#m")
}

/// What a call to [`ChordDetector::train_on_correction`] actually achieved.
/// Every variant is reportable to the user — the re-ranker must never train
/// into a silent void.
#[cfg(feature = "learning")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrainOutcome {
    /// The corrected name now ranks first; `now_reads` is what the strip shows
    /// (post-processing may add a slash bass).
    Learned { steps: u32, now_reads: String },
    /// The correction could not overtake the winner within the step budget —
    /// the score gap is beyond what a bounded nudge can close. The attempt is
    /// rolled back, so nothing changed.
    Stubborn { steps: u32, still_reads: String },
    /// The chosen name already ranked first. `displays_as` differs from the
    /// chosen name when post-processing renames it.
    AlreadyCorrect { displays_as: String },
    /// The chosen name tops the scoring, but the displayed reading comes from
    /// a rule that runs *after* scoring and overrides it — scale detection, or
    /// rootless-dominant renaming. Re-ranking cannot reach that; only an exact
    /// override can. Distinct from `Stubborn`: there is no score gap at all.
    OutrankedByRule { wants: String, displays_as: String },
    /// The chosen name was not one of the scored candidates for this voicing.
    NotTrainable,
    /// No override store is attached (should not happen in the GUI).
    NoStore,
}

// ── ChordDetector ────────────────────────────────────────────────────────────

pub struct ChordDetector {
    pub prefer_flats: bool,
    pub min_notes_for_chord: usize,
    pub max_notes_for_chord: usize,
    debug_mode: bool,
    debug_candidates: Vec<(String, f64)>,
    /// True only while `detect_chord_simple` scores a slash-reduced note set. It
    /// lifts the D23 tritone gate on the #11-shell bonus so slash upper structures
    /// name exactly as they did pre-fix (the primary pass keeps the gate).
    simplify_pass: bool,
    /// Teach-layer store, consulted before scoring. `None` => stock behavior,
    /// byte-identical to a detector that never had a store (the 42 unit tests
    /// and the differential harness both run with no store).
    overrides: Option<OverrideStore>,
    /// Learned-re-ranker candidate capture, active only while
    /// `train_on_correction` is collecting features.
    #[cfg(feature = "learning")]
    learn_capture: Option<Vec<(String, crate::overrides::CandidateFeatures)>>,
    /// Set when the last `detect_chord` returned a scale name from the check
    /// that runs *after* the scoring loop, discarding the winner outright. The
    /// candidates were scored but are unreachable, so nothing is trainable —
    /// without this, the picker offers readings that can never be selected and
    /// every attempt burns the full step budget before blaming a score gap.
    #[cfg(feature = "learning")]
    label_from_scale: bool,
}

impl Default for ChordDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ChordDetector {
    pub fn new() -> Self {
        ChordDetector {
            prefer_flats: true,
            min_notes_for_chord: 2,
            max_notes_for_chord: 7,
            debug_mode: false,
            debug_candidates: Vec::new(),
            simplify_pass: false,
            overrides: None,
            #[cfg(feature = "learning")]
            learn_capture: None,
            #[cfg(feature = "learning")]
            label_from_scale: false,
        }
    }

    pub fn set_note_preference(&mut self, prefer_flats: bool) {
        self.prefer_flats = prefer_flats;
    }

    // ── teach layer ────────────────────────────────────────────────────────

    /// Attach an override store (builder form).
    pub fn with_overrides(mut self, store: OverrideStore) -> Self {
        self.overrides = Some(store);
        self
    }

    /// Replace (or clear) the override store. `None` restores stock behavior.
    pub fn set_overrides(&mut self, store: Option<OverrideStore>) {
        self.overrides = store;
    }

    pub fn overrides(&self) -> Option<&OverrideStore> {
        self.overrides.as_ref()
    }

    pub fn overrides_mut(&mut self) -> Option<&mut OverrideStore> {
        self.overrides.as_mut()
    }

    /// Exact teach-layer lookup, consulted before any scoring. `None` when there
    /// is no store or no exact interval-set-from-bass match.
    fn override_lookup(&self, notes: &HashSet<u8>) -> Option<String> {
        self.overrides
            .as_ref()
            .and_then(|s| s.lookup(notes, self.prefer_flats))
    }

    /// The names the re-ranker can actually be trained toward for `notes`, best
    /// score first. These are the raw scored candidates — the post-scoring
    /// steps (slash notation, rootless-dominant renaming, dim/aug re-rooting)
    /// rewrite the winner afterwards, so the displayed label is often NOT in
    /// this list. The GUI offers exactly this list so a correction can never
    /// land on an untrainable name.
    #[cfg(feature = "learning")]
    pub fn trainable_candidates(&mut self, notes: &HashSet<u8>) -> Vec<(String, f64)> {
        let (_, ranked) = self.capture_ranked(notes);
        ranked
    }

    /// Run detection with candidate capture on, returning the final label and
    /// the captured candidates ranked by score (best first, deduped by name).
    #[cfg(feature = "learning")]
    fn capture_ranked(
        &mut self,
        notes: &HashSet<u8>,
    ) -> (Option<String>, Vec<(String, f64)>) {
        self.learn_capture = Some(Vec::new());
        let prev_debug = self.debug_mode;
        // debug_candidates carries the scores; learn_capture carries features.
        self.debug_mode = true;
        self.debug_candidates.clear();
        let label = self.detect_chord(notes);
        self.debug_mode = prev_debug;
        let captured = self.learn_capture.take().unwrap_or_default();
        let scores = std::mem::take(&mut self.debug_candidates);

        let mut best: HashMap<String, f64> = HashMap::new();
        for (name, score) in scores {
            let e = best.entry(name).or_insert(f64::NEG_INFINITY);
            if score > *e {
                *e = score;
            }
        }
        // Only names that were genuinely captured with features are trainable.
        let mut ranked: Vec<(String, f64)> = captured
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|name| best.get(&name).map(|&s| (name, s)))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        if self.label_from_scale {
            // The late scale check threw the scoring result away. These
            // candidates were computed but are unreachable — reporting them as
            // trainable would offer the user choices that cannot ever apply.
            return (label, Vec::new());
        }
        (label, ranked)
    }

    /// Learned re-ranker: nudge the store's weights until this voicing ranks
    /// `correct_label` top, bounded by `MAX_TRAIN_STEPS`.
    ///
    /// The re-ranker only reorders the *scored* candidates, so `correct_label`
    /// must be one of `trainable_candidates()`. The candidate it trains
    /// against is the one behind the currently displayed label — its base name
    /// with any `/bass` stripped, when that is genuinely a captured candidate
    /// and not the target itself — falling back to the top-scored candidate
    /// when post-processing renamed the winner past recognition (a rootless
    /// dominant, say, where the label shares no text with any candidate).
    #[cfg(feature = "learning")]
    pub fn train_on_correction(
        &mut self,
        notes: &HashSet<u8>,
        correct_label: &str,
    ) -> TrainOutcome {
        /// Weight updates are clamped, so a large score gap can be unclosable;
        /// stop rather than saturate the weights against an impossible target.
        const MAX_TRAIN_STEPS: u32 = 25;

        if self.overrides.is_none() {
            return TrainOutcome::NoStore;
        }
        self.learn_capture = Some(Vec::new());
        let _ = self.detect_chord(notes);
        let captured = self.learn_capture.take().unwrap_or_default();

        let Some(correct_f) = captured
            .iter()
            .find(|(name, _)| name == correct_label)
            .map(|(_, f)| *f)
        else {
            return TrainOutcome::NotTrainable;
        };

        let snapshot = self
            .overrides
            .as_ref()
            .map(|s| s.learning_snapshot())
            .expect("store presence checked above");

        let mut steps = 0;
        loop {
            let (label, ranked) = self.capture_ranked(notes);
            // Success is judged on the DISPLAYED label, not on the raw ranking:
            // winning the scoring loop is necessary but not sufficient (the D21
            // completeness preference can still swap in a note-complete reading
            // that scores within 12 points, and the slash step then appends the
            // bass). "Am7" is satisfied by "Am7" and by "Am7/C".
            if label_reflects(label.as_deref(), correct_label) {
                if steps > 0 {
                    if let Some(store) = self.overrides.as_mut() {
                        store.commit_correction();
                    }
                    return TrainOutcome::Learned {
                        steps,
                        now_reads: label.unwrap_or_else(|| correct_label.to_owned()),
                    };
                }
                return TrainOutcome::AlreadyCorrect {
                    displays_as: label.unwrap_or_else(|| correct_label.to_owned()),
                };
            }
            if steps >= MAX_TRAIN_STEPS {
                // The gap is beyond a bounded nudge. Roll back: a correction
                // that did not land must not silently reshape every other
                // chord's ranking as a consolation prize.
                if let Some(store) = self.overrides.as_mut() {
                    store.restore_learning(snapshot);
                }
                // Report the reading as it stands *after* the rollback.
                let still_reads = self
                    .detect_chord(notes)
                    .unwrap_or_else(|| "(none)".to_owned());
                return TrainOutcome::Stubborn {
                    steps,
                    still_reads,
                };
            }
            // Push down whatever is beating the user's choice right now: the
            // candidate the displayed label came from (its base name, before
            // any slash bass), falling back to the top-scored candidate when
            // post-processing renamed it past recognition.
            let base = label.as_deref().and_then(|l| l.split('/').next());
            let wrong_name = base
                .filter(|b| captured.iter().any(|(n, _)| n == b) && *b != correct_label)
                .map(|b| b.to_owned())
                .or_else(|| ranked.first().map(|(n, _)| n.clone()));
            let Some(wrong_f) = wrong_name
                .filter(|n| n != correct_label)
                .and_then(|name| captured.iter().find(|(n, _)| *n == name))
                .map(|(_, f)| *f)
            else {
                // Nothing left to push against: the pick already tops the
                // ranking, yet the display still disagrees. That is not a score
                // gap — a post-scoring rule (scale detection, rootless-dominant
                // renaming) is overriding the winner, and no amount of
                // re-ranking reaches it. Saying "scores too far behind" here
                // would be a lie.
                if let Some(store) = self.overrides.as_mut() {
                    store.restore_learning(snapshot);
                }
                let displays_as = self
                    .detect_chord(notes)
                    .unwrap_or_else(|| "(none)".to_owned());
                return TrainOutcome::OutrankedByRule {
                    wants: correct_label.to_owned(),
                    displays_as,
                };
            };
            if let Some(store) = self.overrides.as_mut() {
                store.train_step_unsaved(&correct_f, &wrong_f);
            }
            steps += 1;
        }
    }

    /// Forget all learned weights.
    #[cfg(feature = "learning")]
    pub fn reset_learning(&mut self) {
        if let Some(store) = self.overrides.as_mut() {
            store.reset_learning();
        }
    }

    /// Is the learned re-ranker currently allowed to influence detection?
    pub fn learning_mode(&self) -> bool {
        self.overrides.as_ref().is_some_and(|s| s.learning_mode())
    }

    /// Turn the learned re-ranker's influence on or off (weights are kept).
    pub fn set_learning_mode(&mut self, on: bool) {
        if let Some(store) = self.overrides.as_mut() {
            store.set_learning_mode(on);
        }
    }

    pub fn get_note_name(&self, pitch_class: u8) -> &'static str {
        let idx = (pitch_class % 12) as usize;
        if self.prefer_flats { NOTE_NAMES_FLAT[idx] } else { NOTE_NAMES[idx] }
    }

    // ── public API ───────────────────────────────────────────────────────────

    pub fn detect_chord(&mut self, active_notes_in: &HashSet<u8>) -> Option<String> {
        #[cfg(feature = "learning")]
        {
            self.label_from_scale = false;
        }
        if active_notes_in.is_empty() {
            return None;
        }

        // Teach layer: an exact interval-set-from-bass override wins outright,
        // before any scoring. No store => this is a no-op and behavior is
        // identical to the stock engine.
        if let Some(name) = self.override_lookup(active_notes_in) {
            return Some(name);
        }

        let n = active_notes_in.len();
        if n < self.min_notes_for_chord {
            return None;
        }
        if n == 2 {
            return self.detect_interval(active_notes_in);
        }

        let original = active_notes_in.clone();

        // D13: no pitch-class reduction. Doublings never change the PC set, so
        // ≤7 unique PCs use every PC; the old Counter.most_common(7) lottery is gone.
        let active_notes: HashSet<u8> = active_notes_in.clone();

        let pcs_all = pitch_classes(&active_notes);

        // D17: eight or more unique pitch classes never name a chord. Within an
        // octave, defer to a scale reading (8-PC diminished/altered scales still
        // resolve). All twelve tones with no octave-local organization → chromatic.
        // 8–11 PCs spread with no scale → nothing.
        if pcs_all.len() >= 8 {
            let (omin, omax) = (
                original.iter().min().copied().unwrap_or(0) as i32,
                original.iter().max().copied().unwrap_or(0) as i32,
            );
            if omax - omin < 12 {
                if let Some(scale) = self.detect_scale(&original) {
                    return Some(scale);
                }
            }
            if pcs_all.len() == 12 {
                return Some("Chromatic Scale".to_string());
            }
            return self.detect_scale(&original);
        }

        // Pre-check: should we attempt scale detection for clustered notes?
        let span_early = active_notes.iter().max().copied().unwrap_or(0) as i32
            - active_notes.iter().min().copied().unwrap_or(0) as i32;
        let should_check_scale_later = pcs_all.len() >= 5
            && (span_early < 12 || self.is_clustered(&active_notes));

        // For 7 unique pitch classes, quick scale fallback if no dominant quality.
        if pcs_all.len() == 7 {
            let lowest_note = active_notes.iter().min().copied().unwrap_or(0);
            let lowest_pc = lowest_note % 12;
            let intervals_from_lowest: HashSet<u8> =
                pcs_all.iter().map(|&pc| pc_interval(lowest_pc, pc)).collect();
            let has_third = intervals_from_lowest.contains(&3) || intervals_from_lowest.contains(&4);
            let has_seventh = intervals_from_lowest.contains(&10) || intervals_from_lowest.contains(&11);
            if !(has_third && has_seventh) {
                if let Some(scale) = self.detect_scale(&original) {
                    let root_name = self.get_note_name(lowest_pc);
                    if scale.starts_with(root_name) {
                        return Some(scale);
                    }
                }
            }
        }

        // ── EARLY SPECIAL CASES ──────────────────────────────────────────────

        // Case 1: 4-note m6 slash pattern [0,1,7,10] from bass → Xm6/bass
        if pcs_all.len() == 4 {
            let lowest = active_notes.iter().min().copied().unwrap_or(0);
            let lpc = lowest % 12;
            let ivs: Vec<u8> = pcs_all.iter().map(|&pc| pc_interval(lpc, pc)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            if ivs == [0, 1, 7, 10] {
                let root_pc = (lpc + 10) % 12;
                return Some(format!("{}m6/{}", self.get_note_name(root_pc), self.get_note_name(lpc)));
            }
        }

        // Case 1b: 5-note m6 slash [0,1,5,7,10] → Xm6/bass
        if pcs_all.len() == 5 {
            let lowest = active_notes.iter().min().copied().unwrap_or(0);
            let lpc = lowest % 12;
            let ivs: Vec<u8> = pcs_all.iter().map(|&pc| pc_interval(lpc, pc)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            if ivs == [0, 1, 5, 7, 10] {
                let root_pc = (lpc + 10) % 12;
                return Some(format!("{}m6/{}", self.get_note_name(root_pc), self.get_note_name(lpc)));
            }
        }

        // Case 2: 5-note dim7 upper structure → 7b9 or dim7 slash
        if pcs_all.len() == 5 {
            let lowest = active_notes.iter().min().copied().unwrap_or(0);
            let lpc = lowest % 12;
            let remaining: Vec<u8> = pcs_all.iter().copied().filter(|&pc| pc != lpc).collect();
            if remaining.len() == 4 {
                for &dim_root in &remaining {
                    let dim_ivs: Vec<u8> = remaining.iter()
                        .map(|&pc| pc_interval(dim_root, pc))
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter().collect();
                    if dim_ivs == [0, 3, 6, 9] {
                        let ivs_from_bass: HashSet<u8> = remaining.iter()
                            .map(|&pc| pc_interval(lpc, pc)).collect();
                        if ivs_from_bass.contains(&4) && ivs_from_bass.contains(&7)
                            && ivs_from_bass.contains(&10) && ivs_from_bass.contains(&1) {
                            return Some(format!("{}7(b9)", self.get_note_name(lpc)));
                        } else {
                            return Some(format!("{}dim7/{}", self.get_note_name(dim_root), self.get_note_name(lpc)));
                        }
                    }
                }
            }
        }

        // D1: a pure 4-note [0,4,7,9] set is the {R6, relative-(R+9)m7} ambiguity.
        // When the bass is the MAJOR THIRD of the 6-root, Python names it R6/bass
        // (not the relative-minor inversion, e.g. E-G-A-C → C6/E, not Am7/E). Every
        // other bass — the 6-root (→ R6), the 6th/m7-root (→ (R+9)m7), or the 5th
        // (→ the shipped bass-rooted 6/9) — already resolves correctly in scoring,
        // so only the third-in-bass case is special-cased here.
        if pcs_all.len() == 4 {
            let lowest = active_notes.iter().min().copied().unwrap_or(0);
            let lpc = lowest % 12;
            for &r in &pcs_all {
                let ivs: Vec<u8> = pcs_all.iter().map(|&pc| pc_interval(r, pc))
                    .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                if ivs == [0u8, 4, 7, 9] {
                    if lpc == (r + 4) % 12 {
                        return Some(format!("{}6/{}", self.get_note_name(r),
                            self.get_note_name(lpc)));
                    }
                    break;
                }
            }
        }

        // Case 3: Half-dim7 vs minor6
        if pcs_all.len() == 4 {
            let lowest = active_notes.iter().min().copied().unwrap_or(0);
            let lpc = lowest % 12;
            for &potential_root in &pcs_all {
                let ivs: Vec<u8> = pcs_all.iter()
                    .map(|&pc| pc_interval(potential_root, pc))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter().collect();
                if ivs == [0, 3, 6, 10] {
                    if potential_root == lpc {
                        return Some(format!("{}m7b5", self.get_note_name(potential_root)));
                    } else {
                        let m6_root = (potential_root + 3) % 12;
                        let m6_name = self.get_note_name(m6_root);
                        if m6_root == lpc {
                            return Some(format!("{}m6", m6_name));
                        } else {
                            return Some(format!("{}m6/{}", m6_name, self.get_note_name(lpc)));
                        }
                    }
                }
            }
        }

        // ── MAIN SCORING LOOP ────────────────────────────────────────────────
        let highest_note = active_notes.iter().max().copied().unwrap_or(0);
        let highest_pc = highest_note % 12;
        let lowest_note = active_notes.iter().min().copied().unwrap_or(0);
        let lowest_pc = lowest_note % 12;
        let pcs_set: HashSet<u8> = pcs_all.iter().copied().collect();

        let has_global_dominant_quality = pcs_all.iter().any(|&root| {
            pcs_set.contains(&((root + 4) % 12)) && pcs_set.contains(&((root + 10) % 12))
        });

        let mut best_match: Option<String> = None;
        // NEG_INFINITY, not 0.0: admission is decided by the UNADJUSTED score
        // (`raw_score > 0.0`) so the learned re-ranker can only reorder
        // candidates, never eliminate them. With a 0.0 floor here, a candidate
        // whose learned nudge (bounded at -100) dragged it to <= 0 was rejected
        // outright, and a voicing whose every candidate got floored produced
        // best_match = None — i.e. the chord strip went blank on a chord the
        // stock engine names fine. With zero weights this is bit-identical to
        // the old code, since the first admitted candidate always beat 0.0.
        let mut best_score: f64 = f64::NEG_INFINITY;
        let mut best_root_pc: u8 = 0;
        // Best reading that leaves NO sounded note unexplained, plus whether the
        // overall best dropped a note. Used to prefer a complete reading over a
        // near-tie that hides a played tension (maj7#5/maj7#11 drop-2 voicings).
        let mut best_complete: Option<(String, f64, u8)> = None;
        let mut best_is_complete = true;

        #[cfg(feature = "learning")]
        let learn_span = (active_notes.iter().max().copied().unwrap_or(0)
            - active_notes.iter().min().copied().unwrap_or(0)) as u8;
        #[cfg(feature = "learning")]
        let learn_clustered = self.is_clustered(&active_notes);
        #[cfg(feature = "learning")]
        let learn_note_count = active_notes.len() as u8;

        for &root_pc in &pcs_all {
            let intervals: Vec<u8> = pcs_all.iter()
                .map(|&pc| pc_interval(root_pc, pc))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter().collect();

            if let Some((chord_name, score, complete)) = self.match_chord_pattern(
                &intervals, root_pc, &active_notes,
                highest_note, highest_pc, lowest_pc, has_global_dominant_quality,
            ) {
                // Feature-gated learned re-ranker. With no store, learning off,
                // or zero (untrained) weights this adds exactly 0.0, so the
                // default build and the differential harness are unaffected.
                #[allow(unused_mut)]
                let mut score = score;
                // Admission is judged on this, ranking on the adjusted `score`.
                let raw_score = score;
                #[cfg(feature = "learning")]
                {
                    let feats = crate::overrides::CandidateFeatures {
                        root_bass_interval: pc_interval(root_pc, lowest_pc),
                        span: learn_span,
                        note_count: learn_note_count,
                        clustered: learn_clustered,
                        root_is_bass: root_pc == lowest_pc,
                        pattern_hash: crate::overrides::pattern_class_hash(&chord_name),
                    };
                    if let Some(store) = self.overrides.as_ref() {
                        score += store.learning_adjustment(&feats);
                    }
                    if let Some(buf) = self.learn_capture.as_mut() {
                        buf.push((chord_name.clone(), feats));
                    }
                }
                if self.debug_mode {
                    self.debug_candidates.push((chord_name.clone(), score));
                }
                if complete
                    && raw_score > 0.0
                    && score > best_complete.as_ref().map_or(f64::NEG_INFINITY, |c| c.1)
                {
                    best_complete = Some((chord_name.clone(), score, root_pc));
                }
                if raw_score > 0.0 && score > best_score {
                    best_score = score;
                    best_match = Some(chord_name);
                    best_root_pc = root_pc;
                    best_is_complete = complete;
                }
            }
        }

        // If the winning reading dropped a sounded note (extra_count > 0) but a
        // note-complete reading scored within a small margin, prefer the complete
        // one — it names every played tension (fixes maj7#5/maj7#11 drop-2 voicings
        // and tritone-subs where a bare triad/Δ7 edged the full tertian by ~2 pts).
        if !best_is_complete {
            if let Some((cname, cscore, croot)) = best_complete.clone() {
                if cscore >= best_score - 12.0 {
                    best_match = Some(cname);
                    best_score = cscore;
                    best_root_pc = croot;
                    best_is_complete = true;
                }
            }
        }
        let _ = best_is_complete;

        // Dim7 + M3 below = 7(b9) override
        if pcs_all.len() == 4 || pcs_all.len() == 5 {
            'b9_search: for &potential_root in &pcs_all {
                let m3 = (potential_root + 4) % 12;
                if !pcs_set.contains(&m3) { continue; }
                let remaining: Vec<u8> = if pcs_all.len() == 5 {
                    pcs_all.iter().copied().filter(|&pc| pc != potential_root).collect()
                } else {
                    pcs_all.clone()
                };
                let dim_ivs: Vec<u8> = remaining.iter()
                    .map(|&pc| pc_interval(m3, pc))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter().collect();
                if remaining.len() == 4 && dim_ivs == [0, 3, 6, 9] {
                    let b7 = (potential_root + 10) % 12;
                    if remaining.contains(&b7) {
                        let intervals_from_root: Vec<u8> = pcs_all.iter()
                            .map(|&pc| pc_interval(potential_root, pc))
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter().collect();
                        if let Some((cname, cscore, _)) = self.match_chord_pattern(
                            &intervals_from_root, potential_root, &active_notes,
                            highest_note, highest_pc, lowest_pc, has_global_dominant_quality,
                        ) {
                            if cname.contains("7(b9)") || (cname.contains('7') && cscore > best_score) {
                                best_match = Some(cname);
                                best_root_pc = potential_root;
                                best_score = cscore;
                                break 'b9_search;
                            }
                        }
                    }
                }
            }
        }

        // Dim / augmented symmetry: always use bass as root
        if let Some(ref bm) = best_match {
            let is_triadic_dim = self.match_chord_type(bm, "diminished");
            let is_dim7 = self.match_chord_type(bm, "diminished7");
            // No chord NAME maps to the "augmented7" type (match_chord_type has no
            // arm for it, and the pattern was deleted), so testing it is always false.
            let is_aug = self.match_chord_type(bm, "augmented");

            // D12: only re-root a diminished TRIAD to the bass when the bass
            // actually forms a diminished triad ([0,3,6] from it). Otherwise the
            // bass is merely a chord tone (e.g. C-Eb-A: bass C is the b3 of A°, not
            // a °-root) and the minor6_no5 reading from the bass is correct.
            let bass_is_dim_root = {
                let ivs: HashSet<u8> = pcs_all.iter().map(|&pc| pc_interval(lowest_pc, pc)).collect();
                ivs.is_superset(&[0u8, 3, 6].iter().copied().collect())
            };
            if (is_triadic_dim || is_dim7 || is_aug) && best_root_pc != lowest_pc {
                if is_triadic_dim {
                    if bass_is_dim_root {
                        best_match = Some(format!("{}dim", self.get_note_name(lowest_pc)));
                        best_root_pc = lowest_pc;
                    }
                } else {
                    // re-detect from lowest
                    let ivs: Vec<u8> = pcs_all.iter()
                        .map(|&pc| pc_interval(lowest_pc, pc))
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter().collect();
                    if let Some((cname, _, _)) = self.match_chord_pattern(
                        &ivs, lowest_pc, &active_notes,
                        highest_note, highest_pc, lowest_pc, has_global_dominant_quality,
                    ) {
                        let ok = if is_dim7 { self.match_chord_type(&cname, "diminished7") }
                                 else { self.match_chord_type(&cname, "augmented") };
                        if ok {
                            best_match = Some(cname);
                            best_root_pc = lowest_pc;
                        }
                    }
                }
            }
        }

        // ── ROOTLESS DOMINANT RESOLUTION (D9) ────────────────────────────────
        // A voicing like E-Bb-D reads on its bass as an E7(#11) shell, but E has no
        // major third of its own — it is really the M3 of an absent C dominant
        // (C-E-…-Bb-D). When the current best reading is such a false-dominant shell
        // (bass has m7 + tritone but NO M3 and NO 11), and the note a major third
        // below is absent yet forms a genuine dominant (its own M3 and m7 present),
        // re-root there and name from the tensions: E-Bb-D → C9, E-Bb-D-F# → C7(#11).
        //
        // A genuinely rooted dominant (root with real M3 + m7, e.g. whole-tone
        // D7#11) is left untouched, and an m7b5(11) shell (11 present, e.g.
        // E-A-Bb-D → Em7b5(11)) is excluded by the "no 11" test.
        if best_match.is_some() {
            let r = best_root_pc;
            let has_m3 = pcs_set.contains(&((r + 4) % 12));
            let has_m7 = pcs_set.contains(&((r + 10) % 12));
            let has_tritone = pcs_set.contains(&((r + 6) % 12));
            let has_eleventh = pcs_set.contains(&((r + 5) % 12));
            let has_thirteenth = pcs_set.contains(&((r + 9) % 12));
            // A bare #11 shell (m7 + tritone, no 3rd). The 11 or 13 being present
            // marks a full m7b5(11) or 13(#11) rooted here — keep those.
            let is_false_dom_shell =
                has_m7 && has_tritone && !has_m3 && !has_eleventh && !has_thirteenth;
            if is_false_dom_shell {
                let implied = (r + 12 - 4) % 12;
                if !pcs_set.contains(&implied)
                    && pcs_set.contains(&((implied + 4) % 12))
                    && pcs_set.contains(&((implied + 10) % 12))
                {
                    let iv: HashSet<u8> =
                        pcs_all.iter().map(|&pc| pc_interval(implied, pc)).collect();
                    let name = self.name_rootless_dominant(implied, &iv);
                    if self.debug_mode { self.debug_candidates.push((name.clone(), best_score)); }
                    best_match = Some(name);
                    best_root_pc = implied;
                }
            }
        }

        // ── SLASH CHORD HANDLING ─────────────────────────────────────────────
        if let Some(ref bm) = best_match.clone() {
            if best_root_pc != lowest_pc {
                // Rootless voicing: root not in notes → no slash notation
                let root_in_notes = active_notes.iter().any(|&n| n % 12 == best_root_pc);
                if !root_in_notes { /* skip slash for rootless */ } else {
                let bass_interval = pc_interval(best_root_pc, lowest_pc);
                // A MAJOR add9 with its 3rd in the bass names as sus2/bass, rooted on
                // the add9's OWN root: dropping the 3rd-in-bass leaves root-2-5, a clean
                // sus2. Transposition-invariant (C-E-G-D / E bass → C2/E) — emitted here
                // rather than via slash-simplify, which would re-root to the lowest upper
                // voice and misread G4/E. A minor add9's 3rd is a minor third up (interval
                // 3), so it never trips this and stays Xm(add9)/bass.
                let is_major_add9 = bm.contains("(add9)") && !bm.contains("m(add9)");
                if is_major_add9 && bass_interval == 4
                    && pcs_set.contains(&((best_root_pc + 7) % 12))
                {
                    best_match = Some(format!(
                        "{}2/{}",
                        self.get_note_name(best_root_pc),
                        self.get_note_name(lowest_pc)
                    ));
                } else {
                let is_extended = (bm.contains('9') || bm.contains("11") || bm.contains("13") || bm.contains("6/9"))
                    && !bm.contains("add9");
                let is_altered = bm.contains("b9") || bm.contains("#9") || bm.contains("b13") || bm.contains("#11");
                let is_six_nine = bm.contains("6/9");
                let skip_due_to_ext = if is_six_nine && bass_interval == 2 {
                    false
                } else {
                    (is_extended && [2u8, 5, 7, 9, 10].contains(&bass_interval))
                        || (is_altered && [1u8, 3, 6, 8].contains(&bass_interval))
                };
                let is_dim7 = self.match_chord_type(bm, "diminished7");
                let is_aug_chord = self.match_chord_type(bm, "augmented");
                let skip_slash = skip_due_to_ext || is_dim7 || is_aug_chord;

                if !skip_slash {
                    // Possibly simplify the chord above the bass
                    let should_simplify = self.should_simplify_slash(
                        bm, &active_notes, &pcs_all, best_root_pc, lowest_pc, bass_interval,
                        highest_pc,
                    );
                    let final_chord = if should_simplify {
                        let notes_no_bass: HashSet<u8> = active_notes.iter()
                            .copied().filter(|n| n % 12 != lowest_pc).collect();
                        // Don't simplify if only 2 notes remain from a 3-note triad —
                        // avoids "C/E" → "G4/E" (Python guards: len < 3 and pitch_classes == 3)
                        // Don't simplify 3-note triads where removing the bass leaves 2 notes
                        let too_few = notes_no_bass.len() < 3 && pcs_all.len() == 3;
                        if notes_no_bass.len() >= 2 && !too_few {
                            let alt = self.detect_chord_simple(&notes_no_bass);
                            if let Some(alt_chord) = alt {
                                let current_complexity = self.chord_complexity(bm);
                                let alt_complexity = self.chord_complexity(&alt_chord);
                                let alt_is_sus = alt_chord.ends_with('2') || alt_chord.ends_with('4')
                                    || alt_chord.contains("sus");
                                let current_is_add9 = bm.contains("add9");
                                // Prefer sus2 over add9 for slash simplification
                                if alt_is_sus && alt_chord.ends_with('2') && current_is_add9 {
                                    alt_chord
                                } else if alt_is_sus && is_basic_chord(bm) {
                                    alt_chord
                                } else if alt_complexity <= current_complexity {
                                    alt_chord
                                } else {
                                    bm.clone()
                                }
                            } else {
                                bm.clone()
                            }
                        } else {
                            bm.clone()
                        }
                    } else {
                        bm.clone()
                    };
                    let bass_name = self.get_note_name(lowest_pc);
                    best_match = Some(format!("{}/{}", final_chord, bass_name));
                }
                } // end major-add9-3rd-in-bass else
                } // end root_in_notes else
            }
        }

        // ── SCALE CHECK for clustered notes ──────────────────────────────────
        if should_check_scale_later {
            let span = original.iter().max().copied().unwrap_or(0) as i32
                - original.iter().min().copied().unwrap_or(0) as i32;
            // A scale run played root-to-root spans exactly an octave (span == 12,
            // the root doubled on top) or wider, so a bare `span < 12` gate dropped
            // the scale reading the instant the octave was added — the same PC set
            // then scored as maj13 (C-D-E-F-G-A-B-C → CΔ13 instead of C Ionian).
            // A stepwise voicing is `is_clustered`; a spread tertian chord (v067's
            // CΔ13 stack, thirds apart) is not, so it still names as a chord.
            if span < 12 || self.is_clustered(&original) {
                if let Some(scale) = self.detect_scale(&original) {
                    // This discards every scored candidate. Mark it so the teach
                    // layer does not offer readings that can never win.
                    #[cfg(feature = "learning")]
                    {
                        self.label_from_scale = true;
                    }
                    return Some(scale);
                }
            }
        }

        best_match
    }

    pub fn detect_chord_debug(
        &mut self,
        active_notes: &HashSet<u8>,
        top_n: usize,
    ) -> (Option<String>, Vec<(String, f64)>) {
        self.debug_mode = true;
        self.debug_candidates.clear();
        let result = self.detect_chord(active_notes);
        let mut seen: HashMap<String, f64> = HashMap::new();
        for (name, score) in self.debug_candidates.drain(..) {
            let e = seen.entry(name).or_insert(f64::NEG_INFINITY);
            if score > *e { *e = score; }
        }
        let mut candidates: Vec<(String, f64)> = seen.into_iter().collect();
        candidates.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(top_n);
        self.debug_mode = false;
        if candidates.is_empty() {
            if let Some(ref r) = result {
                candidates.push((r.clone(), 9999.0));
            }
        }
        (result, candidates)
    }

    pub fn detect_interval(&self, active_notes: &HashSet<u8>) -> Option<String> {
        if active_notes.len() != 2 { return None; }
        let mut notes: Vec<u8> = active_notes.iter().copied().collect();
        notes.sort_unstable();
        let lower = notes[0];
        let upper = notes[1];
        let semitones = (upper - lower) as usize;
        let lower_name = self.get_note_name(lower % 12);
        // K1: named intervals up to 21 semitones; beyond that, "{n} semitones".
        if semitones < INTERVAL_NAMES.len() {
            Some(format!("{} ({})", lower_name, INTERVAL_NAMES[semitones]))
        } else {
            Some(format!("{} ({} semitones)", lower_name, semitones))
        }
    }

    /// Name an absent-root dominant from the tensions present above `root`.
    /// Any altered tension (b9/#9/#11/b13) drives a `7(...)` name; otherwise a
    /// bare 9 or 13 names the chord; failing those, plain `7`.
    fn name_rootless_dominant(&self, root: u8, iv: &HashSet<u8>) -> String {
        let r = self.get_note_name(root);
        let mut alts: Vec<&str> = Vec::new();
        if iv.contains(&1) { alts.push("b9"); }
        if iv.contains(&3) { alts.push("#9"); }
        if iv.contains(&6) { alts.push("#11"); }
        if iv.contains(&8) { alts.push("b13"); }
        if !alts.is_empty() {
            format!("{}7({})", r, alts.join(","))
        } else if iv.contains(&2) {
            format!("{}9", r)
        } else if iv.contains(&9) {
            format!("{}13", r)
        } else {
            format!("{}7", r)
        }
    }

    pub fn is_clustered(&self, active_notes: &HashSet<u8>) -> bool {
        if active_notes.len() < 5 { return false; }
        let mut sorted: Vec<u8> = active_notes.iter().copied().collect();
        sorted.sort_unstable();
        let adjacent = sorted.windows(2).filter(|w| w[1] - w[0] <= 2).count();
        let total = sorted.len() - 1;
        if total == 0 { return false; }
        (adjacent as f64 / total as f64) >= 0.6
    }

    pub fn detect_scale(&self, active_notes: &HashSet<u8>) -> Option<String> {
        if active_notes.len() < 5 { return None; }
        let pcs = pitch_classes(active_notes);
        if pcs.len() < 5 { return None; }
        let lowest_pc = active_notes.iter().min().copied().unwrap_or(0) % 12;
        let is_cl = self.is_clustered(active_notes);
        let span = active_notes.iter().max().copied().unwrap_or(0) as i32
            - active_notes.iter().min().copied().unwrap_or(0) as i32;
        let within_octave = span < 12;
        let pcs_set: HashSet<u8> = pcs.iter().copied().collect();

        let mut best_match: Option<String> = None;
        let mut best_score: i64 = 0;

        for &root_pc in &pcs {
            let intervals: HashSet<u8> = pcs.iter().map(|&pc| pc_interval(root_pc, pc)).collect();
            for &(scale_name, pattern) in SCALE_PATTERNS {
                if CLUSTERED_ONLY_SCALES.contains(&scale_name) && !(is_cl || within_octave) {
                    continue;
                }
                if scale_name == "Whole Tone" && pcs.len() < 6 { continue; }
                let pat_set: HashSet<u8> = pattern.iter().copied().collect();
                if !pat_set.is_subset(&intervals) { continue; }
                // D26 (owner report 2026-08-10): a scale must account for EVERY
                // sounded pitch class. An input with more unique PCs than the
                // pattern has tones is a larger scale or a chord, never this one —
                // a 6-tone C-D-E-F-G-A is NOT the 5-tone C Major Pentatonic. So a
                // pattern only matches its EXACT pitch-class set (was: any subset,
                // which let every superset inherit the smaller scale's name).
                if intervals.len() != pat_set.len() { continue; }
                let base = 5000 + pat_set.len() as i64;
                let mode_bonus: i64 = if MAJOR_MODES.contains(&scale_name)
                    || MELODIC_MINOR_MODES.contains(&scale_name)
                    || HARMONIC_MINOR_MODES.contains(&scale_name) { 1000 } else { 0 };
                let root_bonus: i64 = if root_pc == lowest_pc { 500 } else { 0 };
                let total = base + mode_bonus + root_bonus;
                if total > best_score {
                    best_score = total;
                    best_match = Some(format!("{} {}", self.get_note_name(root_pc), scale_name));
                }
            }
        }
        best_match
    }

    // ── private helpers ──────────────────────────────────────────────────────

    fn match_chord_type(&self, chord_name: &str, chord_type: &str) -> bool {
        if chord_name.is_empty() { return false; }
        let name = if let Some(p) = chord_name.find('/') { &chord_name[..p] } else { chord_name };
        let quality = if name.len() >= 2 {
            let two: &str = &name[..name.char_indices().nth(2).map_or(name.len(), |(i,_)| i)];
            if NOTE_NAMES_FLAT.contains(&two) || NOTE_NAMES.contains(&two) {
                &name[two.len()..]
            } else {
                let one: &str = &name[..name.char_indices().nth(1).map_or(name.len(), |(i,_)| i)];
                if NOTE_NAMES.contains(&one) || NOTE_NAMES_FLAT.contains(&one) {
                    &name[one.len()..]
                } else { return false; }
            }
        } else if name.len() == 1 {
            let one: &str = &name[..name.char_indices().nth(1).map_or(name.len(), |(i,_)| i)];
            if NOTE_NAMES.contains(&one) { &name[one.len()..] } else { return false; }
        } else { return false; };

        // Special: any 13 variant
        if quality == "13" {
            return matches!(chord_type, "dominant13"|"13_shell"|"13_no5_no11"|"13_no5");
        }

        let mapped = match quality {
            ""          => "major",
            "m"         => "minor",
            "dim"       => "diminished",
            "aug"       => "augmented",
            "2"         => "sus2",
            "4"         => "sus4",
            "7sus4"     => "7sus4",
            "7sus2"     => "7sus2",
            "sus13"     => "sus13",
            "Δ7"        => "major7",
            "Δ7#5"      => "major7#5",
            "m7"        => "minor7",
            "mΔ7"       => "minor_major7",
            "mΔ7(9)"    => "minor_major9",
            "7"         => "dominant7",
            "dim7"      => "diminished7",
            "dimΔ7"     => "diminished_major7",
            "m7b5"      => "half_diminished7",  // ø7 also
            "ø7"        => "half_diminished7",
            "9"         => "dominant9",
            "11"        => "dominant11",
            "Δ9"        => "major9",
            "m9"        => "minor9",
            "Δ11"       => "major11",
            "Δ7(#11)"   => "major7#11",
            "m11"       => "minor11",
            "Δ13"       => "major13",
            "Δ13#11"    => "major13#11",
            "m13"       => "minor13",
            "7alt"      => "altered",
            "5"         => "5",
            "6"         => "6",
            "6/9"       => "6_9",
            "m6"        => "minor6",
            "m6/9"      => "minor6_9",
            "(add9)"    => "add9",
            "add9"      => "add9",
            "add11"     => "add11",
            _           => "",
        };
        mapped == chord_type
    }

    fn chord_complexity(&self, chord_name: &str) -> i32 {
        let name = if let Some(p) = chord_name.find('/') { &chord_name[..p] } else { chord_name };
        if name.contains("13") { 5 }
        else if name.contains("11") { 4 }
        else if name.contains('9') || name.contains("6/9") { 3 }
        else if name.contains("add") || name.contains('6') { 3 }
        else if name.contains('7') || name.contains('Δ') || name.contains("ø") { 2 }
        else { 1 }
    }

    fn detect_chord_simple(&mut self, active_notes: &HashSet<u8>) -> Option<String> {
        if active_notes.len() < 2 { return None; }
        let pcs = pitch_classes(active_notes);
        if pcs.len() < 2 { return None; }
        let pcs_set: HashSet<u8> = pcs.iter().copied().collect();
        let highest = active_notes.iter().max().copied().unwrap_or(0);
        let highest_pc = highest % 12;
        let lowest_pc = active_notes.iter().min().copied().unwrap_or(0) % 12;
        let has_gdq = pcs.iter().any(|&r| pcs_set.contains(&((r + 4) % 12)) && pcs_set.contains(&((r + 10) % 12)));
        let mut best: Option<String> = None;
        let mut best_score = 0.0_f64;
        // Score this reduced set with the pre-fix #11-shell bonus (D23 gate lifted),
        // so slash upper structures keep naming as before.
        let prev_simplify = self.simplify_pass;
        self.simplify_pass = true;
        for &root_pc in &pcs {
            let intervals: Vec<u8> = pcs.iter()
                .map(|&pc| pc_interval(root_pc, pc))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter().collect();
            if let Some((name, score, _)) = self.match_chord_pattern(
                &intervals, root_pc, active_notes, highest, highest_pc, lowest_pc, has_gdq,
            ) {
                if score > best_score { best_score = score; best = Some(name); }
            }
        }
        self.simplify_pass = prev_simplify;
        best
    }

    fn should_simplify_slash(
        &self,
        best_match: &str,
        active_notes: &HashSet<u8>,
        pcs_all: &[u8],
        best_root_pc: u8,
        lowest_pc: u8,
        bass_interval: u8,
        highest_pc: u8,
    ) -> bool {
        // Special voicing check: [0,2,5,7,10] or [0,2,7,10] — Bb6/C vs Gm7/C decision
        let ivs_from_lowest: Vec<u8> = pcs_all.iter()
            .map(|&pc| pc_interval(lowest_pc, pc))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter().collect();
        if ivs_from_lowest == [0, 2, 5, 7, 10] || ivs_from_lowest == [0, 2, 7, 10] {
            return false; // handled specially in match_chord_pattern
        }

        let is_extended_chord = best_match.contains('9') || best_match.contains("11")
            || best_match.contains("13") || best_match.contains("6/9");

        // Find pattern for current chord
        let pattern_has_bass = CHORD_PATTERNS.iter().find_map(|&(ct, pat)| {
            if self.match_chord_type(best_match, ct) { Some(pat) } else { None }
        }).map_or(false, |pat| pat.contains(&bass_interval));

        if is_extended_chord && pattern_has_bass { return false; }

        let mut essential_intervals: HashSet<u8> = [0u8, 3, 4, 6, 7, 8].iter().copied().collect();

        if self.match_chord_type(best_match, "diminished_major7") {
            essential_intervals.extend([6u8, 11]);
        }
        if self.match_chord_type(best_match, "half_diminished7") {
            essential_intervals.extend([3u8, 6, 10]);
        }
        let is_dominant = (best_match.ends_with('7') || best_match.contains("7(") || best_match.ends_with("13"))
            && !best_match.contains('Δ') && !best_match.contains("dim7")
            && !best_match.contains("m7");
        if is_dominant { essential_intervals.insert(10); }

        let is_sus = best_match.ends_with('2') || best_match.ends_with('4') || best_match.contains("sus");
        let is_add9 = best_match.contains("add9");

        if is_sus || is_add9 { return false; }

        // 7th chord with un-doubled bass → simplify
        if best_match.contains('7') && !best_match.contains('Δ')
            && !best_match.contains("m7") && !best_match.contains("dim7") {
            let bass_count = active_notes.iter().filter(|&&n| n % 12 == lowest_pc).count();
            if bass_count == 1 { return true; } else { return false; }
        }

        if essential_intervals.contains(&bass_interval) {
            // Bass is essential — only simplify for basic triads / add9
            let basic_or_add9 = best_match.ends_with('m') || best_match.contains("add9")
                || (best_match.len() <= 3 && !best_match.contains('7') && !best_match.contains('6'));
            basic_or_add9
        } else {
            true
        }
    }

    /// The core scoring engine — port of Python's `_match_chord_pattern`.
    fn match_chord_pattern(
        &mut self,
        intervals: &[u8],
        root_pc: u8,
        active_notes: &HashSet<u8>,
        highest_note: u8,
        highest_pc: u8,
        lowest_pc: u8,
        has_global_dominant_quality: bool,
    ) -> Option<(String, f64, bool)> {
        let mut best_match: Option<(String, f64, bool)> = None;
        let mut best_score = 0.0_f64;

        let input_pc_count = active_notes.iter().map(|&n| n % 12).collect::<HashSet<_>>().len();
        let intervals_set: HashSet<u8> = intervals.iter().copied().collect();
        let pcs_all: Vec<u8> = active_notes.iter().map(|&n| n % 12)
            .collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let pcs_set: HashSet<u8> = pcs_all.iter().copied().collect();

        let intervals_from_lowest: Vec<u8> = pcs_all.iter()
            .map(|&pc| pc_interval(lowest_pc, pc))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter().collect();

        for &(chord_type, pattern) in CHORD_PATTERNS {
            let pattern_set: HashSet<u8> = pattern.iter().copied().collect();
            let matched: HashSet<u8> = pattern_set.intersection(&intervals_set).copied().collect();
            let matched_count = matched.len();
            let extra: HashSet<u8> = intervals_set.difference(&pattern_set).copied().collect();
            let extra_count = extra.len();
            let missing: HashSet<u8> = pattern_set.difference(&intervals_set).copied().collect();
            let missing_count = missing.len();

            let essential: HashSet<u8> = essential_for(chord_type).iter().copied().collect();
            let optional: HashSet<u8> = optional_for(chord_type).iter().copied().collect();
            let essential_matched: HashSet<u8> = essential.intersection(&matched).copied().collect();
            let essential_missing: HashSet<u8> = essential.difference(&matched).copied().collect();

            // Skip certain chord types when essential intervals are all required
            if ["7b9#11","7#9#11","7#9#11_shell","7b9#11_shell","7b9#11_no3"]
                .contains(&chord_type) && !essential_missing.is_empty() { continue; }

            // A "13" chord must contain its 13th (interval 9). Without it, a 13-named
            // pattern would match a subset (e.g. a whole-tone [0,2,4,6,10] as 13#11
            // missing the 13th) and mislabel it — the correct name is 7(#11)/9(#11).
            if chord_type.starts_with("13") && !intervals_set.contains(&9) { continue; }

            if !essential.is_empty() && essential_matched.is_empty() { continue; }
            if matched_count < 2 { continue; }

            // 1. Essential score (up to 60)
            let essential_score = if !essential.is_empty() {
                (essential_matched.len() as f64 / essential.len() as f64) * 60.0
            } else { 30.0 };

            // 2. Percentage match (up to 40)
            let pct_match = (matched_count as f64 / input_pc_count as f64) * 40.0;

            // 3. Highest note bonus
            let highest_interval = pc_interval(root_pc, highest_pc);
            let highest_bonus = if pattern_set.contains(&highest_interval) { 10.0 } else { 0.0 };

            // 4. Completeness bonus
            let completeness_bonus = if missing_count == 0 && extra_count == 0 {
                let base = match chord_type {
                    "diminished_major7" => 500.0,
                    "half_diminished7"  => 700.0,
                    "major7_6_9"        => 200.0,
                    ct if ["7b13_no5","7b9b13_no5","7#9b13_no5","7b9#11_no5",
                            "7b9","7#9","7#11","7b13","7b9b13","7#9b13","7#11b13",
                            "7b9#11","7#9#11"].contains(&ct) => 60.0,
                    _ => 30.0,
                };
                base
            } else if missing_count == 0 { 10.0 } else { 0.0 };

            // 5. Penalties
            let extra_penalty = extra_count as f64 * 3.0;
            let optional_missing: HashSet<u8> = optional.intersection(&missing).copied().collect();
            let required_missing: HashSet<u8> = missing.iter()
                .copied()
                .filter(|x| !optional.contains(x) && !essential.contains(x))
                .collect();
            let missing_penalty =
                essential_missing.len() as f64 * 40.0
                + optional_missing.len() as f64 * 1.0
                + required_missing.len() as f64 * 8.0;

            // 6. Rootless voicing bonus
            let rootless_bonus = if missing.contains(&0)
                && essential_matched.len() == essential.len() && essential.len() >= 2
            { 15.0 } else { 0.0 };

            // 7. Root in bass bonus
            let root_in_bass_bonus = if root_pc == lowest_pc && intervals_set.contains(&0) { 15.0 } else { 0.0 };

            // 8. Characteristic bonus. The +50 shell identity bonus is gated on the
            // tritone (interval 6) actually sounding in the PRIMARY pass: every one of
            // these shells is a #11 chord defined by that tritone, so awarding it to a
            // subset that lacks the #11 let a bare {0,2,4} match "7#11_shell" from a
            // third above and outrank the true reading (C-D-E → D7(#11) not C(add9)).
            // The gate is lifted during `simplify_pass` (the slash-reduction helper),
            // where the pre-fix behavior is kept so augmented/whole-tone slash upper
            // structures name exactly as before — no note-dropping churn there (D23).
            let char_bonus = if intervals_set.contains(&6) || intervals_set.contains(&8) { 10.0 } else { 0.0 }
                + if ["7#11_shell","7#11_no3","7#9#11_shell","7b9#11_shell","7b9#11_no3"]
                    .contains(&chord_type) && (intervals_set.contains(&6) || self.simplify_pass)
                { 50.0 } else { 0.0 };

            // 9. Dominant quality adjustment
            let has_major_third = intervals_set.contains(&4);
            let has_minor_seventh = intervals_set.contains(&10);
            let has_dom = has_global_dominant_quality || (has_major_third && has_minor_seventh);
            let dominant_adj = if has_dom {
                if chord_type.starts_with("6") || chord_type.starts_with("minor6")
                    || chord_type == "diminished7" || chord_type == "diminished"
                { -500.0 }
                else if chord_type == "dominant7" && missing_count == 0 && extra_count == 0
                { 600.0 }
                else if chord_type.starts_with("13") || chord_type.starts_with("dominant")
                { 50.0 }
                else { 0.0 }
            } else { 0.0 };

            // 10. Special pattern bonuses
            let special = self.special_bonus(
                chord_type, intervals, &intervals_set, &intervals_from_lowest,
                root_pc, lowest_pc, active_notes, &pcs_set,
                matched_count, missing_count, extra_count,
                has_global_dominant_quality,
            );

            // 11. Inversion bonus
            let bass_iv = pc_interval(root_pc, lowest_pc);
            let is_triad = matches!(chord_type, "major"|"minor"|"diminished"|"augmented");
            let is_seventh = chord_type == "major7" || chord_type == "minor7"
                || chord_type == "dominant7" || chord_type == "diminished7"
                || chord_type == "diminished_major7" || chord_type == "half_diminished7"
                || chord_type == "minor_major7"
                // Extended tertian chords invert too (C9 over its 3rd → C9/E).
                || matches!(chord_type, "dominant9"|"dominant11"|"dominant13"|"13#11"
                    |"major9"|"minor9"|"major11"|"minor11"|"major13"|"minor13")
                // "altered" is intentionally NOT here: it does not start with '7', so
                // the old `|| chord_type == "altered"` inside this group was dead.
                || (chord_type.starts_with('7') && (chord_type.contains("b9") || chord_type.contains("#9")
                    || chord_type.contains("#11") || chord_type.contains("b13")));
            let is_sixth_chord = matches!(chord_type, "6"|"6_no5"|"minor6"|"minor6_no5"
                |"6_9"|"6_9_no5"|"6_9_no3"|"minor6_9"|"6add4"|"6add4_no5");

            let inversion_bonus = if is_triad && [3u8, 4, 7].contains(&bass_iv) {
                35.0
            } else if is_seventh && pattern_set.contains(&bass_iv) && bass_iv != 0 {
                40.0
            } else if is_sixth_chord && bass_iv == 0 {
                // Check if could be minor triad inversion
                let potential_root = (lowest_pc + 12 - 3) % 12;
                let potential_ivs: HashSet<u8> = pcs_all.iter().map(|&pc| pc_interval(potential_root, pc)).collect();
                if potential_ivs.is_superset(&[0u8, 3, 7].iter().copied().collect()) {
                    let sixth_pc = (root_pc + 9) % 12;
                    if highest_pc == sixth_pc && active_notes.len() >= 4 { 45.0 } else { -40.0 }
                } else { 0.0 }
            } else { 0.0 };

            let score = essential_score + pct_match + highest_bonus + completeness_bonus
                + rootless_bonus + root_in_bass_bonus + char_bonus + dominant_adj + special
                + inversion_bonus - extra_penalty - missing_penalty;

            // matched_count >= 2 already guaranteed by the `matched_count < 2` continue above.
            if score > best_score && score > 10.0 {
                best_score = score;

                let final_type = chord_type;
                let final_root = root_pc;

                let root_name = self.get_note_name(final_root);
                let chord_name = format_chord_name(root_name, final_type);
                // `complete` = this pattern leaves no sounded pitch class
                // unexplained (extra_count == 0). Used to prefer a note-complete
                // reading over one that drops a tension (see detect_chord).
                best_match = Some((chord_name, score, extra_count == 0));
            }
        }
        best_match
    }

    /// All special-case bonuses extracted from match_chord_pattern for readability.
    #[allow(clippy::too_many_arguments)]
    fn special_bonus(
        &self,
        chord_type: &str,
        intervals: &[u8],
        intervals_set: &HashSet<u8>,
        intervals_from_lowest: &[u8],
        root_pc: u8,
        lowest_pc: u8,
        active_notes: &HashSet<u8>,
        pcs_set: &HashSet<u8>,
        matched_count: usize,
        missing_count: usize,
        extra_count: usize,
        has_global_dominant_quality: bool,
    ) -> f64 {
        let unique_pcs = active_notes.iter().map(|&n| n % 12).collect::<HashSet<_>>().len();
        let mut bonus = 0.0_f64;

        // Exact pattern boosts for altered dominants
        if chord_type == "7b13_no5" && intervals == [0, 4, 8, 10] { bonus += 100.0; }
        if chord_type == "7b9b13_no5" && intervals == [0, 1, 4, 8, 10] { bonus += 150.0; }
        if chord_type == "7#9b13_no5" && intervals == [0, 3, 4, 8, 10] { bonus += 150.0; }
        if chord_type == "7b9#11_no5" && intervals == [0, 1, 4, 6, 10] { bonus += 400.0; }

        // m6 slash exact pattern
        if intervals_from_lowest == [0, 1, 7, 10] && !has_global_dominant_quality {
            if matches!(chord_type, "minor6"|"minor6_no5"|"minor6_9_no5") && root_pc != lowest_pc {
                bonus += 1500.0;
            }
        }

        // Penalise dim triad with 4+ notes
        if chord_type == "diminished" && unique_pcs >= 4 { bonus -= 1000.0; }

        // C E A → C6 (not Am/C)
        if matches!(chord_type, "6_no5"|"6") && root_pc == lowest_pc && intervals == [0, 4, 9] {
            bonus += 100.0;
        }

        // D12: C Eb A → Cm6 (not the A° reading re-rooted to the bass). The
        // minor6_no5 [0,3,9] from the bass is the coherent, test-intended name.
        if chord_type == "minor6_no5" && root_pc == lowest_pc && intervals == [0, 3, 9] {
            bonus += 100.0;
        }

        // add9 slash vs 9sus span heuristic
        if chord_type == "add9" && missing_count == 0 && extra_count == 0 && root_pc != lowest_pc {
            let ivs_bass: Vec<u8> = active_notes.iter()
                .map(|&n| n % 12)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|pc| ((pc as i32 - lowest_pc as i32 + 12) % 12) as u8)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter().collect::<Vec<_>>();
            if ivs_bass == [0, 2, 5, 10] {
                let has_m3 = intervals.contains(&3); let has_M3 = intervals.contains(&4);
                let has_p5 = intervals.contains(&7);
                let triad_complete = intervals.contains(&0) && (has_m3 || has_M3) && has_p5;
                if triad_complete {
                    let bass_iv_from_root = ((lowest_pc as i32 - root_pc as i32 + 12) % 12) as u8;
                    let bass_is_triad = [0u8, 3, 4, 7].contains(&bass_iv_from_root);
                    if !bass_is_triad {
                        let span = active_notes.iter().max().copied().unwrap_or(0) as i32
                            - active_notes.iter().min().copied().unwrap_or(0) as i32;
                        bonus += if span < 12 { 6200.0 } else { 150.0 };
                    } else { bonus += 4200.0; }
                } else { bonus += 150.0; }
            } else { bonus += 150.0; }
        }

        // minor_add9 perfect match beats minor triad inversions
        if chord_type == "minor_add9" && missing_count == 0 && extra_count == 0 { bonus += 50.0; }

        // m6 slash chords beat dim
        if matches!(chord_type, "minor6"|"minor6_no5"|"minor6_9"|"minor6_9_no5")
            && root_pc != lowest_pc && !has_global_dominant_quality
        {
            if intervals_set.contains(&3) && intervals_set.contains(&9) && unique_pcs == 4 {
                bonus += if intervals == [0, 2, 3, 9] { 600.0 } else { 400.0 };
            }
        }

        // half-dim7 perfect match beats 7#11
        if chord_type == "half_diminished7" && intervals == [0, 3, 6, 10]
            && missing_count == 0 && extra_count == 0 { bonus += 180.0; }

        // sus2/sus4 root position boost
        if matches!(chord_type, "sus2"|"sus4") && root_pc == lowest_pc {
            let essential: HashSet<u8> = essential_for(chord_type).iter().copied().collect();
            let essential_missing: HashSet<u8> = essential.difference(intervals_set).copied().collect();
            if essential_missing.is_empty() && missing_count <= 1 && extra_count == 0 && bonus == 0.0 {
                bonus += 80.0;
            }
        }

        // Major extended with #11. Boost when the major-7 root is in the bass, OR
        // when it is a PERFECT full voicing (5th present, nothing missing/extra) —
        // that is a genuine inversion like CΔ7(#11)/B and should win. A no-5th shell
        // from a non-bass root stays gated, so E-A-Bb-D is Em7b5(11), not BbΔ7(#11).
        if matches!(chord_type, "major7#11"|"major7#11_no5"|"major9#11"|"major13#11")
            && intervals_set.contains(&6)
            && (root_pc == lowest_pc
                || (missing_count == 0 && extra_count == 0 && intervals_set.contains(&7)))
        {
            bonus += if missing_count == 0 && extra_count == 0 { 250.0 } else if missing_count <= 1 { 150.0 } else { 0.0 };
        }
        if chord_type == "major7#11_no5" && intervals == [0, 4, 6, 11] && root_pc == lowest_pc {
            bonus += 300.0;
        }

        // 6/9 chords
        if matches!(chord_type, "6_9"|"6_9_no5") {
            if intervals_set.contains(&9) && intervals_set.contains(&2) && root_pc == lowest_pc {
                bonus += if missing_count == 0 && extra_count == 0 { 9000.0 } else if missing_count <= 1 { 220.0 } else { 0.0 };
            } else if !intervals_set.contains(&9) { bonus -= 300.0; }
        }
        if chord_type == "6_9_no3" {
            if intervals_set.contains(&9) && intervals_set.contains(&2) && root_pc == lowest_pc {
                bonus += if missing_count == 0 && extra_count == 0 { 290.0 } else if missing_count <= 1 { 220.0 } else { 0.0 };
            }
        }

        // minor6/9
        if matches!(chord_type, "minor6_9"|"minor6_9_no5") {
            if intervals_set.contains(&9) && intervals_set.contains(&2) && intervals_set.contains(&3) && root_pc == lowest_pc {
                if missing_count == 0 && extra_count == 0 { bonus += 9500.0; }
            }
        }

        // major7(6/9)
        if chord_type == "major7_6_9" {
            if missing_count == 0 && extra_count == 0 && root_pc == lowest_pc { bonus += 10000.0; }
            else if !intervals_set.contains(&9) { bonus -= 300.0; }
        }

        // m3 + M6 present with 4 unique pcs → boost m6 interpretations
        // D5: the blanket +380 (spec §9.13's "distorter") must not crown a spurious
        // mΔ7/6-family reading over a root-position dominant. Suppress it when the
        // BASS itself is a dominant root (M3 + m7 above the bass present) — that set
        // is a 7(b9)/7(#9) voicing, e.g. C-Db-E-Bb → C7(b9), not Bbdim/C.
        let bass_has_dominant =
            pcs_set.contains(&((lowest_pc + 4) % 12)) && pcs_set.contains(&((lowest_pc + 10) % 12));
        if intervals_set.contains(&3) && intervals_set.contains(&9) && unique_pcs == 4 {
            if matches!(chord_type, "minor6"|"minor6_no5"|"minor6_9"|"minor6_9_no5") {
                if bonus == 0.0 {
                    bonus += if missing_count == 0 && extra_count == 0 { 450.0 }
                             else if missing_count <= 1 && extra_count <= 2 { 410.0 }
                             else { 0.0 };
                }
            } else if bonus == 0.0 && !bass_has_dominant { bonus += 380.0; }
        }

        // 13th shells beat major7#11 from other roots
        if matches!(chord_type, "13_shell"|"13_no5_no11"|"13_no5") && root_pc == lowest_pc {
            if intervals_set.contains(&4) && intervals_set.contains(&10) && intervals_set.contains(&9) {
                bonus += if missing_count == 0 && extra_count == 0 { 250.0 } else if missing_count <= 1 { 180.0 } else { 0.0 };
            }
        }

        // half-dim11 no3 specific voicing
        if chord_type == "half_diminished11_no3" && intervals == [0, 5, 6, 10] {
            let mut sorted: Vec<u8> = active_notes.iter().copied().collect();
            sorted.sort_unstable();
            if sorted.len() >= 2 && root_pc == lowest_pc {
                let second_iv = pc_interval(lowest_pc, sorted[1] % 12);
                if second_iv == 5 { bonus += 300.0; }
            }
        }

        // Dom 7#11 / 13#11 voicings. A "13#11" name requires the actual 13th
        // (interval 9) — but match_chord_pattern already `continue`d any 13-prefixed
        // type lacking interval 9, so every 13* type reaching here has it. The old
        // `(!claims_13 || contains(&9))` guard was therefore always true.
        if matches!(chord_type, "7#11_no5"|"7#11_no3_no5"|"9#11_no5"
                                 |"13#11_no3_no5"|"13#11_no9_no5"|"13#11_no5")
            && root_pc == lowest_pc && intervals_set.contains(&10) && intervals_set.contains(&6)
        {
            bonus += if missing_count == 0 && extra_count == 0 { 250.0 }
                     else if missing_count <= 1 && extra_count == 0 { 180.0 }
                     else { 0.0 };
        }

        // minor11 chords beat scale interpretations (only when root is in bass)
        // D20: the m11 "beat the scale" bonus is suppressed only when the bass roots
        // a competing resolved chord — the relative-major 6/9 (bass = root+3) or the
        // 9sus whose root is the m11's 11th (bass = root+5). Other basses (root, the
        // 9th at root+2, 5th, b7) keep the m11 reading, so inversions like a Gm11
        // drop-2 with the 9th (A) in the bass stay Gm11 rather than flipping to the
        // relative BbΔ(6/9).
        if matches!(chord_type, "minor11"|"minor11_no5"|"minor11_no9"|"minor11_shell")
            && missing_count == 0 && extra_count == 0
        {
            let bass_iv = pc_interval(root_pc, lowest_pc);
            if bass_iv != 3 && bass_iv != 5 { bonus += 8000.0; }
        }

        // D3: 6–7 note tertian stacks name from a coherent bass root. A perfect
        // major/minor 11/13-family match with root in bass beats the same PC set
        // read as a #11 extension from a third above (CΔ13 set → CΔ13, not FΔ13#11).
        if matches!(chord_type,
            "major13"|"major11"|"minor13"|"major9"|"13#11"|"dominant13"|"dominant11")
            && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc { bonus += 8000.0; }

        // A bass-rooted maj7 shell that also carries the 6/13 — root + M3 + M7 + M6
        // in the bass, nothing foreign — is a genuine XΔ13, even with the 5th/9th/11th
        // absent. Prefer it over reading the 13th as the root of a minor(add9):
        // B-D#-G#-A# (from B: {0,4,9,11}) → BΔ13, not the complete-but-rootless
        // G#m(add9)/B. Gated on root-in-bass + all three characteristic tones so it
        // cannot crown an incomplete maj13 over an unrelated chord.
        if matches!(chord_type, "major13"|"major13#11")
            && root_pc == lowest_pc && extra_count == 0
            && intervals_set.contains(&4) && intervals_set.contains(&11) && intervals_set.contains(&9)
        {
            bonus += 120.0;
        }

        // 13sus: large bonus when root is in bass and perfect match. The 13th (9)
        // rules out the add9-slash reading, so no span gate (D8).
        if matches!(chord_type, "13sus"|"13sus_with5")
            && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc
        {
            bonus += 6400.0;
        }

        // 9sus: K5 span rule. Compact [0,2,5,10]-from-bass reads as (bass+10)add9/
        // bass (that bonus is +6200); only a spread voicing (≥12 span) names 9sus.
        if matches!(chord_type, "9sus"|"9sus_with5")
            && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc
        {
            let span = active_notes.iter().max().copied().unwrap_or(0) as i32
                - active_notes.iter().min().copied().unwrap_or(0) as i32;
            if span >= 12 { bonus += 6400.0; }
        }

        // 7sus4: large bonus when root is in bass and perfect match (D2/D20). Note:
        // 7sus2 [0,2,7,10] is intentionally NOT boosted — that voicing is the
        // Bb6/C-vs-Gm/C shape (K11), resolved by the S2k second-note logic below.
        if chord_type == "7sus4"
            && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc
        {
            bonus += 8000.0;
        }

        // Altered dominant, root in bass, no foreign notes: beat plain dominant7 and
        // partial-match shells. D4/D5: allow a missing (optional) 5th so no-5th
        // voicings still name their alteration — C-Db-E-Bb → C7(b9), C-E-Bb-Eb →
        // C7(#9) — rather than collapsing to a bare C7 that drops the b9/#9.
        // "missing ⊆ {5th}": every pattern tone present except perhaps the perfect
        // 5th. (All these patterns contain interval 7, so a single missing tone with
        // 7 absent from the input must be that 5th.)
        let only_fifth_missing =
            missing_count == 0 || (missing_count == 1 && !intervals_set.contains(&7));
        if matches!(chord_type, "7b9"|"7#9"|"7#11"|"7b13"|"7b9#11"|"7#9#11"|
                                 "7b9b13"|"7#9b13"|"7#11b13"|"9b13")
            && root_pc == lowest_pc && extra_count == 0 && only_fifth_missing
        {
            bonus += 120.0;
        }

        // D4: penalize no5/shell readings from a NON-bass root when the bass is a
        // dominant root (M3 + m7 above the bass present), with or without the 5th.
        // Prevents EΔ7(#11)/Gb7(b9,#11)/Bb7(#11) winning over C7(#9)/C7(#11)/C7(b13)
        // when C is the bass carrying the M3 and m7 — covers both the full voicing
        // (C-E-G-Bb-Eb → C7(#9)) and the no-5th shell (C-E-Bb-Eb → C7(#9)).
        if root_pc != lowest_pc {
            let b_m3 = (lowest_pc + 4) % 12;
            let b_m7 = (lowest_pc + 10) % 12;
            if pcs_set.contains(&b_m3) && pcs_set.contains(&b_m7) {
                let is_shell_or_no5 = chord_type.contains("no5") || chord_type.contains("shell");
                if is_shell_or_no5 {
                    bonus -= 600.0;
                }
            }
        }

        // Rootless dominant9 shell: {M3, m7, 9} with root and 5th absent → C9
        if chord_type == "dominant9" && missing_count == 2
            && !intervals_set.contains(&0) && !intervals_set.contains(&7) && extra_count == 0
        {
            bonus += 250.0;
        }

        // 7b9#11 with 13
        if chord_type == "7b9#11_13_no5" && missing_count == 0 && extra_count == 0 { bonus += 260.0; }

        // 9b13
        if matches!(chord_type, "9b13"|"9b13_no5") && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc {
            bonus += 250.0;
        }

        // dominant9 root in bass — but only a real dominant (m7 present); without the
        // b7 the voicing is an add9 (C-D-E-G → C(add9), not C9).
        if chord_type == "dominant9" && root_pc == lowest_pc && missing_count <= 1
            && extra_count == 0 && intervals_set.contains(&10)
        {
            bonus += 200.0;
        }

        // Bb6/C voicing detection
        let is_bb6_voicing = (intervals_from_lowest == [0, 2, 5, 7, 10] || intervals_from_lowest == [0, 2, 7, 10]) && {
            let mut sorted: Vec<u8> = active_notes.iter().copied().collect();
            sorted.sort_unstable();
            sorted.len() >= 2 && pc_interval(lowest_pc, sorted[1] % 12) == 10
        };
        if is_bb6_voicing {
            if chord_type == "6" && ((root_pc as i32 - lowest_pc as i32 + 12) % 12) as u8 == 10 {
                bonus += 250.0;
            } else if matches!(chord_type, "6_9"|"6_9_no5")
                && ((root_pc as i32 - lowest_pc as i32 + 12) % 12) as u8 == 10
            {
                bonus -= 100.0;
            } else if matches!(chord_type, "minor7"|"minor") {
                bonus -= 200.0;
            }
        } else if intervals_from_lowest == [0, 2, 5, 7, 10] || intervals_from_lowest == [0, 2, 7, 10] {
            let root_iv_from_bass = pc_interval(lowest_pc, root_pc);
            if matches!(chord_type, "minor7"|"minor") && root_iv_from_bass == 7 { bonus += 200.0; }
            else if chord_type == "6" && root_iv_from_bass == 10 { bonus -= 200.0; }
        }

        // Bb6 exact pattern from Bb root
        if intervals == [0, 2, 4, 7, 9] && chord_type == "6" { bonus += 200.0; }
        // (dominant9-root-in-bass was already scored above; the old trailing
        // `bonus += 0.0` block here did nothing and is removed.)

        bonus
    }
}

// ── chord name formatter ──────────────────────────────────────────────────────

fn format_chord_name(root: &str, chord_type: &str) -> String {
    match chord_type {
        "major"              => root.to_string(),
        "minor"              => format!("{}m", root),
        "diminished"         => format!("{}dim", root),
        "augmented"          => format!("{}aug", root),
        "sus2"               => format!("{}2", root),
        "sus4"               => format!("{}4", root),
        "7sus4"              => format!("{}7sus4", root),
        "7sus2"              => format!("{}7sus2", root),
        "9sus" | "9sus_with5"=> format!("{}9(sus)", root),
        "13sus"|"13sus_with5"=> format!("{}13(sus)", root),
        // "7sus13" pattern was deleted (dup of 13sus); the `other` catch-all below
        // would render it identically anyway, so no explicit arm is needed.
        "sus13"              => format!("{}sus13", root),
        "major7"             => format!("{}Δ7", root),
        "major7#5"           => format!("{}Δ7#5", root),
        "minor7"             => format!("{}m7", root),
        "minor_major7"       => format!("{}mΔ7", root),
        "minor_major9"       => format!("{}mΔ7(9)", root),
        "dominant7"          => format!("{}7", root),
        "diminished7"        => format!("{}dim7", root),
        "diminished_major7"  => format!("{}dimΔ7", root),
        "half_diminished7"   => format!("{}m7b5", root),
        "half_diminished11" | "half_diminished11_no3" => format!("{}m7b5(11)", root),
        "dominant9"          => format!("{}9", root),
        "dominant11"         => format!("{}11", root),
        "dominant13"         => format!("{}13", root),
        "13#11"              => format!("{}13(#11)", root),
        "13_shell"|"13_no5_no11"|"13_no5" => format!("{}13", root),
        "7#11_no5"|"7#11_no3_no5"|"9#11_no5" => format!("{}7(#11)", root),
        "13#11_no3_no5"|"13#11_no9_no5"|"13#11_no5" => format!("{}13(#11)", root),
        "major9"             => format!("{}Δ9", root),
        "minor9"             => format!("{}m9", root),
        "major11"            => format!("{}Δ11", root),
        "major7#11"|"major7#11_no5"|"major7#11_shell" => format!("{}Δ7(#11)", root),
        "major9#11"          => format!("{}Δ9(#11)", root),
        "minor11"|"minor11_no5"|"minor11_no9"|"minor11_shell" => format!("{}m11", root),
        "major13"            => format!("{}Δ13", root),
        "major13#11"         => format!("{}Δ13#11", root),
        "minor13"            => format!("{}m13", root),
        "altered"            => format!("{}7alt", root),
        "7b9"                => format!("{}7(b9)", root),
        "7#9"                => format!("{}7(#9)", root),
        "7#11"               => format!("{}7(#11)", root),
        "7#11_shell"|"7#11_no3" => format!("{}7(#11)", root),
        "7b13"               => format!("{}7(b13)", root),
        "9b13"|"9b13_no5"    => format!("{}9(b13)", root),
        "7b9#11"|"7b9#11_shell"|"7b9#11_no3"|"7b9#11_no5"|"7b9#11_13_no5" => format!("{}7(b9,#11)", root),
        "7#9#11"|"7#9#11_shell"  => format!("{}7(#9,#11)", root),
        "7b9b13"|"7b9b13_no5"    => format!("{}7(b9,b13)", root),
        "7#9b13"|"7#9b13_no5"    => format!("{}7(#9,b13)", root),
        "7#11b13"            => format!("{}7(#11,b13)", root),
        "7b9#11b13"          => format!("{}7(b9,#11,b13)", root),
        "7#9#11b13"          => format!("{}7(#9,#11,b13)", root),
        "7b9#9"|"7b9#9_no5"  => format!("{}7(b9,#9)", root),
        "7b9#9#11"           => format!("{}7(b9,#9,#11)", root),
        "7b9#9b13"           => format!("{}7(b9,#9,b13)", root),
        "7b13_no5"           => format!("{}7(b13)", root),
        "5"                  => format!("{}5", root),
        "6"|"6_no5"          => format!("{}6", root),
        "6add4"|"6add4_no5"  => format!("{}6add4", root),
        "6_9"|"6_9_no5"|"6_9_no3" => format!("{}6/9", root),
        "major7_6_9"         => format!("{}maj7(6/9)", root),
        "minor6"|"minor6_no5"=> format!("{}m6", root),
        "minor6_9"|"minor6_9_no5" => format!("{}m6/9", root),
        "add9"               => format!("{}(add9)", root),
        "minor_add9"         => format!("{}m(add9)", root),
        "add11"              => format!("{}add11", root),
        other                => format!("{}{}", root, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(v: &[u8]) -> HashSet<u8> { v.iter().copied().collect() }

    // ── Intervals ────────────────────────────────────────────────────────────
    #[test] fn t_m2()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,61])), Some("C (m2)".into())); }
    #[test] fn t_M2()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,62])), Some("C (M2)".into())); }
    #[test] fn t_m3()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63])), Some("C (m3)".into())); }
    #[test] fn t_M3()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64])), Some("C (M3)".into())); }
    #[test] fn t_P4()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,65])), Some("C (P4)".into())); }
    #[test] fn t_d5()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,66])), Some("C (d5)".into())); }
    #[test] fn t_P5()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,67])), Some("C (P5)".into())); }
    #[test] fn t_m6()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,68])), Some("C (m6)".into())); }
    #[test] fn t_M6()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,69])), Some("C (M6)".into())); }
    #[test] fn t_m7()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,70])), Some("C (m7)".into())); }
    #[test] fn t_M7()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,71])), Some("C (M7)".into())); }

    // ── Triads ───────────────────────────────────────────────────────────────
    #[test] fn t_major()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67])), Some("C".into())); }
    #[test] fn t_minor()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63,67])), Some("Cm".into())); }
    #[test] fn t_dim()    { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63,66])), Some("Cdim".into())); }
    #[test] fn t_aug()    { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,68])), Some("Caug".into())); }
    #[test] fn t_sus2()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,62,67])), Some("C2".into())); }
    #[test] fn t_sus4()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,65,67])), Some("C4".into())); }

    // ── 7th chords ───────────────────────────────────────────────────────────
    #[test] fn t_dom7()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[67,71,74,77])), Some("G7".into())); }
    #[test] fn t_maj7()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[62,66,69,73])), Some("DΔ7".into())); }
    #[test] fn t_min7()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63,67,70])), Some("Cm7".into())); }
    #[test] fn t_dim7()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63,66,69])), Some("Cdim7".into())); }
    #[test] fn t_halfdim(){ assert_eq!(ChordDetector::new().detect_chord(&notes(&[55,58,61,65])), Some("Gm7b5".into())); }

    // ── Extended ─────────────────────────────────────────────────────────────
    #[test] fn t_dom9()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67,70,74])), Some("C9".into())); }
    #[test] fn t_maj9()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67,71,74])), Some("CΔ9".into())); }
    #[test] fn t_min9()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63,67,70,74])), Some("Cm9".into())); }

    // ── Sus chords ───────────────────────────────────────────────────────────
    #[test] fn t_7sus4()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,65,67,70])), Some("C7sus4".into())); }
    // K5: compact [0,2,5,10] reads as the b7-add9 slash; only a spread voicing names
    // 9sus (see acceptance v101/v102). Updated notes to a spread voicing.
    #[test] fn t_9sus()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,62,65,82])), Some("C9(sus)".into())); }

    // ── Altered dominants ────────────────────────────────────────────────────
    #[test] fn t_7b9()    { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67,70,61])), Some("C7(b9)".into())); }
    #[test] fn t_7s9()    { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67,70,63])), Some("C7(#9)".into())); }
    #[test] fn t_7s11()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67,70,66])), Some("C7(#11)".into())); }
    #[test] fn t_7b13()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,64,67,70,68])), Some("C7(b13)".into())); }

    // ── Inversions ───────────────────────────────────────────────────────────
    #[test] fn t_inv1()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[52,55,60])), Some("C/E".into())); }
    #[test] fn t_inv2()   { assert_eq!(ChordDetector::new().detect_chord(&notes(&[55,60,64])), Some("C/G".into())); }
    // Parity (acceptance v022): the shipped Python reading of Eb-G-C is Eb6, not the
    // old Rust core's Cm/Eb (Case-3b divergence, removed). Bass Eb, [0,4,9] from Eb.
    #[test] fn t_eb6_not_cm_inv(){ assert_eq!(ChordDetector::new().detect_chord(&notes(&[63,67,72])), Some("Eb6".into())); }

    // ── Rootless voicings ────────────────────────────────────────────────────
    #[test] fn t_rootless_dom9()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[64,70,74])), Some("C9".into())); }

    // ── Scales ───────────────────────────────────────────────────────────────
    #[test] fn t_ionian()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[65,67,69,70,72,74,76])), Some("F Ionian".into())); }
    #[test] fn t_aeolian() { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,62,63,65,67,68,70])), Some("C Aeolian".into())); }
    #[test] fn t_dorian()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[62,64,65,67,69,71,72])), Some("D Dorian".into())); }

    // ── Half-dim vs minor6 ───────────────────────────────────────────────────
    #[test] fn t_halfdim_root_bass() { assert_eq!(ChordDetector::new().detect_chord(&notes(&[55,58,61,65])), Some("Gm7b5".into())); }
    #[test] fn t_minor6_root_bass()  { assert_eq!(ChordDetector::new().detect_chord(&notes(&[58,61,65,67])), Some("Bbm6".into())); }
    #[test] fn t_minor6_slash()      { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,61,67,70])), Some("Bbm6/C".into())); }

    // ── Cm7 not Am6 ─────────────────────────────────────────────────────────
    #[test] fn t_cm7_not_am6() { assert_eq!(ChordDetector::new().detect_chord(&notes(&[60,63,67,70])), Some("Cm7".into())); }
}
