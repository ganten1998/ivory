//! Teach layer: exact voicing overrides (+ an optional learned re-ranker behind
//! the `learning` cargo feature).
//!
//! An *override* pins a user-chosen name to a voicing, keyed by the sorted
//! interval-set measured from the bass note (transposition-invariant by
//! construction — the bass is interval 0). Two entry shapes:
//!
//!   * **Literal** — a fixed name string, returned verbatim for the shape in
//!     every key (`{"intervals":[...],"name":"..."}`).
//!   * **Transposition-invariant** — a name *suffix* with the root stripped off;
//!     on lookup the root is re-rendered from the played bass under the current
//!     flat/sharp preference (`{"intervals":[...],"name_suffix":"..."}`, plus an
//!     optional `root_offset` when the taught root is not the bass).
//!
//! Persisted to `~/.config/ivory/overrides.json`, schema version 1:
//! ```json
//! { "version": 1,
//!   "overrides": [ {"intervals":[0,4,7],"name_suffix":"maj"} ],
//!   "learning_mode": false,
//!   "weights": {} }
//! ```
//! Loading is lazy and forgiving: a missing or corrupt file yields fresh
//! defaults and never panics. Saving happens on every mutation.

use crate::patterns::{NOTE_NAMES, NOTE_NAMES_FLAT};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

const SCHEMA_VERSION: u32 = 1;

// ── note-name parsing/rendering ────────────────────────────────────────────

/// Pitch class of a note *letter* A–G, or None.
fn letter_pc(c: char) -> Option<u8> {
    match c {
        'C' => Some(0),
        'D' => Some(2),
        'E' => Some(4),
        'F' => Some(5),
        'G' => Some(7),
        'A' => Some(9),
        'B' => Some(11),
        _ => None,
    }
}

/// Parse a leading note name (letter + optional single accidental) off a chord
/// name, returning its pitch class and the remaining suffix. `None` when the
/// string does not begin with a recognizable note name.
pub fn parse_root(name: &str) -> Option<(u8, &str)> {
    let mut chars = name.char_indices();
    let (_, first) = chars.next()?;
    let mut pc = letter_pc(first)?;
    // Optional accidental. A trailing 'b' right after the letter is an accidental
    // (e.g. "Bb"); '#' raises. Anything else starts the suffix.
    let mut rest_start = 1;
    if let Some((idx, acc)) = chars.next() {
        match acc {
            '#' => {
                pc = (pc + 1) % 12;
                rest_start = idx + acc.len_utf8();
            }
            'b' => {
                pc = (pc + 11) % 12;
                rest_start = idx + acc.len_utf8();
            }
            _ => {}
        }
    }
    Some((pc, &name[rest_start..]))
}

fn render_root(pc: u8, prefer_flats: bool) -> &'static str {
    let idx = (pc % 12) as usize;
    if prefer_flats { NOTE_NAMES_FLAT[idx] } else { NOTE_NAMES[idx] }
}

// ── entries ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// Fixed string, key-independent.
    Literal(String),
    /// Root-parameterized: rendered root = bass + `root_offset`, then `suffix`.
    Transposed { suffix: String, root_offset: u8 },
}

/// A user-facing view of one stored override, for the "Manage Taught Chords…"
/// list.
#[derive(Clone, Debug)]
pub struct OverrideInfo {
    /// The interval-set-from-bass key (also the map key).
    pub intervals: Vec<u8>,
    /// Human-readable voicing rendered from a C bass (e.g. "C E G").
    pub voicing: String,
    /// Human-readable name, with `(all keys)` appended for transposed entries.
    pub display_name: String,
}

// ── wire format ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct WireEntry {
    intervals: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name_suffix: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    root_offset: u8,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

#[derive(Serialize, Deserialize)]
struct WireFile {
    version: u32,
    #[serde(default)]
    overrides: Vec<WireEntry>,
    #[serde(default)]
    learning_mode: bool,
    #[serde(default)]
    weights: serde_json::Value,
}

// ── store ──────────────────────────────────────────────────────────────────

pub struct OverrideStore {
    path: PathBuf,
    /// BTreeMap keeps a deterministic on-disk and list order.
    map: BTreeMap<Vec<u8>, Entry>,
    learning_mode: bool,
    /// Raw persisted weights, round-tripped verbatim in the default build;
    /// under the `learning` feature the perceptron owns them instead.
    #[cfg_attr(feature = "learning", allow(dead_code))]
    weights_raw: serde_json::Value,
    #[cfg(feature = "learning")]
    perceptron: learning::Perceptron,
}

impl OverrideStore {
    /// `~/.config/ivory/overrides.json` on every platform (matches settings.rs).
    pub fn default_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("ivory").join("overrides.json")
    }

    /// Load from the default path, lazily; corrupt/missing → fresh defaults.
    pub fn load() -> Self {
        Self::load_from(Self::default_path())
    }

    /// Load from an explicit path (used by tests). Never panics.
    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let fresh = |path: PathBuf| Self {
            path,
            map: BTreeMap::new(),
            learning_mode: false,
            weights_raw: serde_json::Value::Object(Default::default()),
            #[cfg(feature = "learning")]
            perceptron: learning::Perceptron::default(),
        };

        let Ok(text) = std::fs::read_to_string(&path) else {
            return fresh(path);
        };
        let Ok(wire) = serde_json::from_str::<WireFile>(&text) else {
            return fresh(path);
        };
        if wire.version != SCHEMA_VERSION {
            return fresh(path);
        }

        let mut map = BTreeMap::new();
        for e in wire.overrides {
            let mut intervals = e.intervals.clone();
            intervals.sort_unstable();
            intervals.dedup();
            let entry = if let Some(sfx) = e.name_suffix {
                Entry::Transposed {
                    suffix: sfx,
                    root_offset: e.root_offset % 12,
                }
            } else if let Some(name) = e.name {
                Entry::Literal(name)
            } else {
                continue; // malformed row — skip, don't panic
            };
            map.insert(intervals, entry);
        }

        #[cfg(feature = "learning")]
        let perceptron = learning::Perceptron::from_value(&wire.weights);

        OverrideStore {
            path,
            map,
            learning_mode: wire.learning_mode,
            weights_raw: wire.weights,
            #[cfg(feature = "learning")]
            perceptron,
        }
    }

    /// Persist to disk. Errors are silently ignored (parity with settings.rs).
    pub fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut overrides: Vec<WireEntry> = Vec::with_capacity(self.map.len());
        for (intervals, entry) in &self.map {
            match entry {
                Entry::Literal(name) => overrides.push(WireEntry {
                    intervals: intervals.clone(),
                    name: Some(name.clone()),
                    name_suffix: None,
                    root_offset: 0,
                }),
                Entry::Transposed { suffix, root_offset } => overrides.push(WireEntry {
                    intervals: intervals.clone(),
                    name: None,
                    name_suffix: Some(suffix.clone()),
                    root_offset: *root_offset,
                }),
            }
        }

        #[cfg(feature = "learning")]
        let weights = self.perceptron.to_value();
        #[cfg(not(feature = "learning"))]
        let weights = self.weights_raw.clone();

        let wire = WireFile {
            version: SCHEMA_VERSION,
            overrides,
            learning_mode: self.learning_mode,
            weights,
        };
        if let Ok(text) = serde_json::to_string_pretty(&wire) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    // ── lookup / mutation ──────────────────────────────────────────────────

    /// Sorted, unique interval-set measured from the bass (lowest MIDI note).
    /// Always begins with 0. Empty input yields an empty vec.
    pub fn interval_set_from_bass(notes: &HashSet<u8>) -> (u8, Vec<u8>) {
        let Some(&bass) = notes.iter().min() else {
            return (0, Vec::new());
        };
        let bass_pc = bass % 12;
        let mut ivs: Vec<u8> = notes
            .iter()
            .map(|&n| ((n % 12) as i32 - bass_pc as i32).rem_euclid(12) as u8)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        ivs.sort_unstable();
        (bass_pc, ivs)
    }

    /// Look up an override for a held note set. `None` when no exact match.
    pub fn lookup(&self, notes: &HashSet<u8>, prefer_flats: bool) -> Option<String> {
        if self.map.is_empty() {
            return None;
        }
        let (bass_pc, ivs) = Self::interval_set_from_bass(notes);
        let entry = self.map.get(&ivs)?;
        Some(self.render(entry, bass_pc, prefer_flats))
    }

    fn render(&self, entry: &Entry, bass_pc: u8, prefer_flats: bool) -> String {
        match entry {
            Entry::Literal(name) => name.clone(),
            Entry::Transposed { suffix, root_offset } => {
                let root_pc = (bass_pc + root_offset) % 12;
                format!("{}{}", render_root(root_pc, prefer_flats), suffix)
            }
        }
    }

    /// Teach a name for the given held voicing. When `apply_all_keys` is set and
    /// `taught_name` begins with a note name, the name is stored
    /// transposition-invariantly (root stripped, re-rendered on lookup);
    /// otherwise it is stored literally. Replaces any existing entry for the
    /// same shape. Saves immediately.
    pub fn teach(&mut self, notes: &HashSet<u8>, taught_name: &str, apply_all_keys: bool) {
        let (bass_pc, ivs) = Self::interval_set_from_bass(notes);
        if ivs.is_empty() {
            return;
        }
        let taught = taught_name.trim();
        let entry = match (apply_all_keys, parse_root(taught)) {
            (true, Some((root_pc, suffix))) => Entry::Transposed {
                suffix: suffix.to_owned(),
                root_offset: (root_pc + 12 - bass_pc) % 12,
            },
            _ => Entry::Literal(taught.to_owned()),
        };
        self.map.insert(ivs, entry);
        self.save();
    }

    /// Delete the override for an exact interval-set key. Saves if it existed.
    pub fn delete(&mut self, intervals: &[u8]) -> bool {
        if self.map.remove(intervals).is_some() {
            self.save();
            true
        } else {
            false
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Human-readable listing for the "Manage Taught Chords…" dialog.
    pub fn list(&self, prefer_flats: bool) -> Vec<OverrideInfo> {
        self.map
            .iter()
            .map(|(intervals, entry)| {
                let voicing = intervals
                    .iter()
                    .map(|&iv| render_root(iv % 12, prefer_flats))
                    .collect::<Vec<_>>()
                    .join(" ");
                let display_name = match entry {
                    Entry::Literal(name) => name.clone(),
                    Entry::Transposed { suffix, root_offset } => {
                        // Show it rendered from a C-based bass, tagged "(all keys)".
                        let root_pc = *root_offset % 12;
                        format!("{}{}  (all keys)", render_root(root_pc, prefer_flats), suffix)
                    }
                };
                OverrideInfo {
                    intervals: intervals.clone(),
                    voicing,
                    display_name,
                }
            })
            .collect()
    }

    pub fn learning_mode(&self) -> bool {
        self.learning_mode
    }

    pub fn set_learning_mode(&mut self, on: bool) {
        self.learning_mode = on;
        self.save();
    }
}

// ── learned re-ranker (feature-gated) ──────────────────────────────────────

#[cfg(feature = "learning")]
pub use learning::{pattern_class_hash, CandidateFeatures};

#[cfg(feature = "learning")]
mod learning {
    use super::OverrideStore;
    use serde_json::json;

    /// Number of features (see `CandidateFeatures::vector`).
    pub const N_FEATURES: usize = 7;
    /// Maximum absolute score nudge the re-ranker may apply to any candidate.
    /// Exact overrides bypass scoring entirely, so this can never overturn one.
    pub const ADJ_BOUND: f64 = 100.0;
    const LEARNING_RATE: f64 = 4.0;
    const WEIGHT_BOUND: f64 = 500.0;

    /// FNV-1a hash of a chord's transposition-invariant quality (its name with
    /// the leading root — and any slash bass — removed).
    pub fn pattern_class_hash(chord_name: &str) -> u64 {
        let quality = match super::parse_root(chord_name) {
            Some((_, rest)) => rest,
            None => chord_name,
        };
        let quality = quality.split('/').next().unwrap_or(quality);
        let mut h: u64 = 0xcbf29ce484222325;
        for b in quality.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// The feature vector for one scored candidate. All components are
    /// transposition-invariant and roughly normalized to [0, 1].
    #[derive(Clone, Copy, Debug)]
    pub struct CandidateFeatures {
        pub root_bass_interval: u8,
        pub span: u8,
        pub note_count: u8,
        pub clustered: bool,
        pub root_is_bass: bool,
        pub pattern_hash: u64,
    }

    impl CandidateFeatures {
        pub fn vector(&self) -> [f64; N_FEATURES] {
            [
                1.0,                                            // bias
                (self.root_bass_interval % 12) as f64 / 11.0,   // inversion
                (self.span.min(24)) as f64 / 24.0,              // voicing span
                (self.note_count.min(8)) as f64 / 8.0,          // note count
                if self.clustered { 1.0 } else { 0.0 },         // cluster flag
                if self.root_is_bass { 1.0 } else { 0.0 },      // root == bass
                (self.pattern_hash % 97) as f64 / 97.0,         // pattern class
            ]
        }
    }

    #[derive(Clone, Debug)]
    pub struct Perceptron {
        weights: [f64; N_FEATURES],
    }

    impl Default for Perceptron {
        fn default() -> Self {
            Perceptron {
                weights: [0.0; N_FEATURES],
            }
        }
    }

    impl Perceptron {
        pub fn from_value(v: &serde_json::Value) -> Self {
            let mut w = [0.0; N_FEATURES];
            if let Some(arr) = v.get("w").and_then(|x| x.as_array()) {
                for (i, slot) in w.iter_mut().enumerate() {
                    if let Some(x) = arr.get(i).and_then(|x| x.as_f64()) {
                        *slot = x;
                    }
                }
            }
            Perceptron { weights: w }
        }

        pub fn to_value(&self) -> serde_json::Value {
            json!({ "w": self.weights.to_vec() })
        }

        /// Bounded score adjustment for a candidate. Zero weights → exactly 0.
        pub fn adjustment(&self, f: &CandidateFeatures) -> f64 {
            let v = f.vector();
            let dot: f64 = self.weights.iter().zip(v.iter()).map(|(w, x)| w * x).sum();
            dot.clamp(-ADJ_BOUND, ADJ_BOUND)
        }

        /// One perceptron step nudging `correct` above `wrong`.
        pub fn update(&mut self, correct: &CandidateFeatures, wrong: &CandidateFeatures) {
            let c = correct.vector();
            let w = wrong.vector();
            for i in 0..N_FEATURES {
                let grad = c[i] - w[i];
                self.weights[i] =
                    (self.weights[i] + LEARNING_RATE * grad).clamp(-WEIGHT_BOUND, WEIGHT_BOUND);
            }
        }

        pub fn reset(&mut self) {
            self.weights = [0.0; N_FEATURES];
        }

        pub fn is_zero(&self) -> bool {
            self.weights.iter().all(|&w| w == 0.0)
        }
    }

    impl OverrideStore {
        /// Bounded re-rank nudge for a candidate; 0 unless learning is enabled
        /// with non-zero trained weights.
        pub fn learning_adjustment(&self, f: &CandidateFeatures) -> f64 {
            if !self.learning_mode || self.perceptron.is_zero() {
                return 0.0;
            }
            self.perceptron.adjustment(f)
        }

        /// Train the re-ranker to prefer `correct` over `wrong` for a voicing.
        /// Enables learning mode and persists.
        pub fn train(&mut self, correct: &CandidateFeatures, wrong: &CandidateFeatures) {
            self.perceptron.update(correct, wrong);
            self.learning_mode = true;
            self.save();
        }

        /// Forget all learned weights (learning mode is left untouched). Persists.
        pub fn reset_learning(&mut self) {
            self.perceptron.reset();
            self.save();
        }

        #[cfg(test)]
        pub(crate) fn perceptron_is_zero(&self) -> bool {
            self.perceptron.is_zero()
        }
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(v: &[u8]) -> HashSet<u8> {
        v.iter().copied().collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ivory-ovr-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn parse_root_basic() {
        assert_eq!(parse_root("Cmaj7"), Some((0, "maj7")));
        assert_eq!(parse_root("Bbm"), Some((10, "m")));
        assert_eq!(parse_root("F#7(#9)"), Some((6, "7(#9)")));
        assert_eq!(parse_root("B"), Some((11, "")));
        assert_eq!(parse_root("xyz"), None);
        // A plain 'b' after a letter is an accidental, not a suffix start.
        assert_eq!(parse_root("Ab(add9)"), Some((8, "(add9)")));
    }

    #[test]
    fn missing_file_is_fresh() {
        let store = OverrideStore::load_from(tmp("missing"));
        assert!(store.is_empty());
        assert!(store.lookup(&notes(&[60, 64, 67]), true).is_none());
    }

    #[test]
    fn corrupt_file_is_fresh_no_panic() {
        let p = tmp("corrupt");
        std::fs::write(&p, "{ not json at all ][").unwrap();
        let store = OverrideStore::load_from(&p);
        assert!(store.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn literal_round_trip() {
        let p = tmp("literal");
        {
            let mut s = OverrideStore::load_from(&p);
            s.teach(&notes(&[60, 64, 67]), "Power", false);
        }
        let s = OverrideStore::load_from(&p);
        assert_eq!(s.lookup(&notes(&[60, 64, 67]), true), Some("Power".into()));
        // Literal names apply to the same shape in any key (voicing-bound).
        assert_eq!(s.lookup(&notes(&[62, 66, 69]), true), Some("Power".into()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn transposition_render_both_prefs() {
        let p = tmp("transpose");
        {
            let mut s = OverrideStore::load_from(&p);
            // Taught at C, root == bass → offset 0, suffix "maj".
            s.teach(&notes(&[60, 64, 67]), "Cmaj", true);
        }
        let s = OverrideStore::load_from(&p);
        // Same shape rooted on Gb/F# renders per preference.
        let gb = notes(&[66, 70, 73]); // F#-A#-C#  == Gb major triad
        assert_eq!(s.lookup(&gb, true), Some("Gbmaj".into()));
        assert_eq!(s.lookup(&gb, false), Some("F#maj".into()));
        // And at D.
        assert_eq!(s.lookup(&notes(&[62, 66, 69]), true), Some("Dmaj".into()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn transposition_offset_when_root_not_bass() {
        let p = tmp("offset");
        {
            let mut s = OverrideStore::load_from(&p);
            // Rootless C9 shell E-Bb-D, bass E; taught "C9" with apply-all-keys.
            // parse root C(0), bass E(4) → offset 8.
            s.teach(&notes(&[64, 70, 74]), "C9", true);
        }
        let s = OverrideStore::load_from(&p);
        // Same shape a whole tone up (bass F#): root = F# + 8 = D → "D9".
        assert_eq!(s.lookup(&notes(&[66, 72, 76]), true), Some("D9".into()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn teach_replaces_same_shape() {
        let p = tmp("replace");
        let mut s = OverrideStore::load_from(&p);
        s.teach(&notes(&[60, 64, 67]), "First", false);
        s.teach(&notes(&[60, 64, 67]), "Second", false);
        assert_eq!(s.len(), 1);
        assert_eq!(s.lookup(&notes(&[60, 64, 67]), true), Some("Second".into()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn delete_removes_entry() {
        let p = tmp("delete");
        let mut s = OverrideStore::load_from(&p);
        s.teach(&notes(&[60, 64, 67]), "Power", false);
        let (_, ivs) = OverrideStore::interval_set_from_bass(&notes(&[60, 64, 67]));
        assert!(s.delete(&ivs));
        assert!(!s.delete(&ivs));
        assert!(s.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn interval_set_is_transposition_invariant() {
        let (_, a) = OverrideStore::interval_set_from_bass(&notes(&[60, 64, 67]));
        let (_, b) = OverrideStore::interval_set_from_bass(&notes(&[62, 66, 69]));
        assert_eq!(a, vec![0, 4, 7]);
        assert_eq!(a, b);
    }

    #[test]
    fn override_takes_precedence_over_detection() {
        use crate::ChordDetector;
        let p = tmp("precedence");
        let mut store = OverrideStore::load_from(&p);
        // A plain C major triad is normally "C"; teach it "MyChord" literally.
        store.teach(&notes(&[60, 64, 67]), "MyChord", false);

        let mut det = ChordDetector::new().with_overrides(store);
        assert_eq!(det.detect_chord(&notes(&[60, 64, 67])), Some("MyChord".into()));
        // A voicing with no override falls through to normal scoring.
        assert_eq!(det.detect_chord(&notes(&[60, 63, 67])), Some("Cm".into()));

        // Detaching the store restores identical stock behavior.
        det.set_overrides(None);
        assert_eq!(det.detect_chord(&notes(&[60, 64, 67])), Some("C".into()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn no_store_is_identical_to_stock() {
        use crate::ChordDetector;
        let mut a = ChordDetector::new();
        let mut b = ChordDetector::new().with_overrides(OverrideStore::load_from(tmp("empty")));
        for set in [
            vec![60u8, 64, 67],
            vec![60, 63, 67, 70],
            vec![67, 71, 74, 77],
            vec![60, 62, 65, 82],
        ] {
            assert_eq!(a.detect_chord(&notes(&set)), b.detect_chord(&notes(&set)));
        }
    }

    #[test]
    fn list_reports_voicing_and_name() {
        let p = tmp("list");
        let mut s = OverrideStore::load_from(&p);
        s.teach(&notes(&[60, 64, 67]), "Power", false);
        let rows = s.list(true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].voicing, "C E G");
        assert_eq!(rows[0].display_name, "Power");
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(all(test, feature = "learning"))]
mod learning_tests {
    use super::*;
    use crate::ChordDetector;

    fn notes(v: &[u8]) -> HashSet<u8> {
        v.iter().copied().collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ivory-learn-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn zero_weights_are_a_no_op() {
        // A store with learning enabled but untrained weights must not change
        // any output vs the stock detector.
        let mut store = OverrideStore::load_from(tmp("zero"));
        store.set_learning_mode(true);
        assert!(store.perceptron_is_zero());

        let mut learned = ChordDetector::new().with_overrides(store);
        let mut stock = ChordDetector::new();
        for set in [
            vec![60u8, 64, 67],
            vec![60, 64, 67, 69],
            vec![60, 63, 67, 70],
            vec![67, 71, 74, 77],
        ] {
            assert_eq!(
                learned.detect_chord(&notes(&set)),
                stock.detect_chord(&notes(&set))
            );
        }
    }

    #[test]
    fn weights_round_trip_through_json() {
        let p = tmp("weights");
        {
            let mut det = ChordDetector::new().with_overrides(OverrideStore::load_from(&p));
            // Train the C6-vs-Am7 voicing toward Am7 a few times.
            for _ in 0..20 {
                det.train_on_correction(&notes(&[60, 64, 67, 69]), "Am7");
            }
        }
        // Reload from disk: learning mode + weights persisted.
        let store = OverrideStore::load_from(&p);
        assert!(store.learning_mode());
        assert!(!store.perceptron_is_zero());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn train_on_correction_flips_a_close_call() {
        let p = tmp("flip");
        let mut det = ChordDetector::new().with_overrides(OverrideStore::load_from(&p));
        // Stock reading is C6 (Am7 is the close runner-up).
        assert_eq!(det.detect_chord(&notes(&[60, 64, 67, 69])), Some("C6".into()));

        for _ in 0..20 {
            det.train_on_correction(&notes(&[60, 64, 67, 69]), "Am7");
        }
        let after = det.detect_chord(&notes(&[60, 64, 67, 69])).unwrap();
        assert!(
            after.starts_with("Am7"),
            "expected an Am7 reading after training, got {after:?}"
        );

        // Resetting the weights restores the stock reading.
        det.reset_learning();
        assert_eq!(det.detect_chord(&notes(&[60, 64, 67, 69])), Some("C6".into()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn exact_override_beats_the_reranker() {
        // Even a trained re-ranker cannot overturn an exact override, which
        // short-circuits before scoring.
        let p = tmp("override-wins");
        let mut det = ChordDetector::new().with_overrides(OverrideStore::load_from(&p));
        det.overrides_mut()
            .unwrap()
            .teach(&notes(&[60, 64, 67, 69]), "PINNED", false);
        for _ in 0..20 {
            det.train_on_correction(&notes(&[60, 64, 67, 69]), "Am7");
        }
        assert_eq!(det.detect_chord(&notes(&[60, 64, 67, 69])), Some("PINNED".into()));
        let _ = std::fs::remove_file(&p);
    }
}
