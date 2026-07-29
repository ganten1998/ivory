use std::collections::{HashMap, HashSet};
use crate::patterns::*;

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

// ── ChordDetector ────────────────────────────────────────────────────────────

pub struct ChordDetector {
    pub prefer_flats: bool,
    pub min_notes_for_chord: usize,
    pub max_notes_for_chord: usize,
    debug_mode: bool,
    debug_candidates: Vec<(String, f64)>,
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
        }
    }

    pub fn set_note_preference(&mut self, prefer_flats: bool) {
        self.prefer_flats = prefer_flats;
    }

    pub fn get_note_name(&self, pitch_class: u8) -> &'static str {
        let idx = (pitch_class % 12) as usize;
        if self.prefer_flats { NOTE_NAMES_FLAT[idx] } else { NOTE_NAMES[idx] }
    }

    // ── public API ───────────────────────────────────────────────────────────

    pub fn detect_chord(&mut self, active_notes_in: &HashSet<u8>) -> Option<String> {
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
                'dim7_search: for &dim_root in &remaining {
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
                        #[allow(unreachable_code)]
                        break 'dim7_search;
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
            'halfdim: for &potential_root in &pcs_all {
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
                    #[allow(unreachable_code)]
                    break 'halfdim;
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
        let mut best_score: f64 = 0.0;
        let mut best_root_pc: u8 = 0;

        for &root_pc in &pcs_all {
            let intervals: Vec<u8> = pcs_all.iter()
                .map(|&pc| pc_interval(root_pc, pc))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter().collect();

            if let Some((chord_name, score)) = self.match_chord_pattern(
                &intervals, root_pc, &active_notes,
                highest_note, highest_pc, lowest_pc, has_global_dominant_quality,
            ) {
                if self.debug_mode {
                    self.debug_candidates.push((chord_name.clone(), score));
                }
                if score > best_score {
                    best_score = score;
                    best_match = Some(chord_name);
                    best_root_pc = root_pc;
                }
            }
        }

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
                        if let Some((cname, cscore)) = self.match_chord_pattern(
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
            let is_aug = self.match_chord_type(bm, "augmented")
                || self.match_chord_type(bm, "augmented7");

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
                    if let Some((cname, _)) = self.match_chord_pattern(
                        &ivs, lowest_pc, &active_notes,
                        highest_note, highest_pc, lowest_pc, has_global_dominant_quality,
                    ) {
                        let ok = if is_dim7 { self.match_chord_type(&cname, "diminished7") }
                                 else { self.match_chord_type(&cname, "augmented") || self.match_chord_type(&cname, "augmented7") };
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
                let is_aug_chord = self.match_chord_type(bm, "augmented")
                    || self.match_chord_type(bm, "augmented7");
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
                } // end root_in_notes else
            }
        }

        // ── SCALE CHECK for clustered notes ──────────────────────────────────
        if should_check_scale_later {
            let span = original.iter().max().copied().unwrap_or(0) as i32
                - original.iter().min().copied().unwrap_or(0) as i32;
            if span < 12 {
                if let Some(scale) = self.detect_scale(&original) {
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
                let extra = intervals.len() - pat_set.intersection(&intervals).count();
                let score: i64 = if extra == 0 {
                    let base = 5000 + pat_set.len() as i64;
                    let mode_bonus: i64 = if MAJOR_MODES.contains(&scale_name)
                        || MELODIC_MINOR_MODES.contains(&scale_name)
                        || HARMONIC_MINOR_MODES.contains(&scale_name) { 1000 } else { 0 };
                    base + mode_bonus
                } else {
                    (pat_set.len() as i64) * 10 - (extra as i64) * 5
                };
                let root_bonus: i64 = if root_pc == lowest_pc { 500 } else { 0 };
                let total = score + root_bonus;
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
        for &root_pc in &pcs {
            let intervals: Vec<u8> = pcs.iter()
                .map(|&pc| pc_interval(root_pc, pc))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter().collect();
            if let Some((name, score)) = self.match_chord_pattern(
                &intervals, root_pc, active_notes, highest, highest_pc, lowest_pc, has_gdq,
            ) {
                if score > best_score { best_score = score; best = Some(name); }
            }
        }
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
    ) -> Option<(String, f64)> {
        let mut best_match: Option<(String, f64)> = None;
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

            // 8. Characteristic bonus
            let char_bonus = if intervals_set.contains(&6) || intervals_set.contains(&8) { 10.0 } else { 0.0 }
                + if ["7#11_shell","7#11_no3","7#9#11_shell","7b9#11_shell","7b9#11_no3"]
                    .contains(&chord_type) { 50.0 } else { 0.0 };

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
                || chord_type == "augmented7" || chord_type == "minor_major7"
                // Extended tertian chords invert too (C9 over its 3rd → C9/E).
                || matches!(chord_type, "dominant9"|"dominant11"|"dominant13"|"13#11"
                    |"major9"|"minor9"|"major11"|"minor11"|"major13"|"minor13")
                || (chord_type.starts_with('7') && (chord_type.contains("b9") || chord_type.contains("#9")
                    || chord_type.contains("#11") || chord_type.contains("b13") || chord_type == "altered"));
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

            if score > best_score && matched_count >= 2 && score > 10.0 {
                best_score = score;

                let final_type = chord_type;
                let final_root = root_pc;

                let root_name = self.get_note_name(final_root);
                let chord_name = format_chord_name(root_name, final_type);
                best_match = Some((chord_name, score));
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

        // Major extended with #11 — only when the major-7 root is itself in the bass.
        // From a non-bass root this shell must not out-bonus a bass-rooted reading
        // (E-A-Bb-D is Em7b5(11), not BbΔ7(#11)).
        if matches!(chord_type, "major7#11"|"major7#11_no5"|"major9#11"|"major13#11")
            && intervals_set.contains(&6) && root_pc == lowest_pc
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
        // (interval 9) present — otherwise [0,2,4,6,10] is a 7(#11), not a 13(#11).
        let claims_13 = chord_type.starts_with("13");
        if matches!(chord_type, "7#11_no5"|"7#11_no3_no5"|"9#11_no5"
                                 |"13#11_no3_no5"|"13#11_no9_no5"|"13#11_no5")
            && root_pc == lowest_pc && intervals_set.contains(&10) && intervals_set.contains(&6)
            && (!claims_13 || intervals_set.contains(&9))
        {
            bonus += if missing_count == 0 && extra_count == 0 { 250.0 }
                     else if missing_count <= 1 && extra_count == 0 { 180.0 }
                     else { 0.0 };
        }

        // minor11 chords beat scale interpretations (only when root is in bass)
        if matches!(chord_type, "minor11"|"minor11_no5"|"minor11_no9"|"minor11_shell")
            && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc { bonus += 8000.0; }

        // D3: 6–7 note tertian stacks name from a coherent bass root. A perfect
        // major/minor 11/13-family match with root in bass beats the same PC set
        // read as a #11 extension from a third above (CΔ13 set → CΔ13, not FΔ13#11).
        if matches!(chord_type,
            "major13"|"major11"|"minor13"|"major9"|"13#11"|"dominant13"|"dominant11")
            && missing_count == 0 && extra_count == 0 && root_pc == lowest_pc { bonus += 8000.0; }

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

        // dominant9 with root in bass beats BbΔ7#11
        if chord_type == "dominant9" && root_pc == lowest_pc && missing_count <= 1 && extra_count == 0 {
            bonus += 0.0; // already handled above
        }

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
        "7sus13"             => format!("{}7sus13", root),
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
