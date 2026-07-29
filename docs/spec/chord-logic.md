# Ivory Chord Detection Engine — Definitive Algorithm Specification

Source of truth: `/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory/chord_detector.py` (2153 lines, class `ChordDetector`).
Companion docs: `Ivory Info/01_Special_Cases_and_Resolutions.md`, `Ivory Info/02_Code_and_Logic_Summary.md`.

**Goal**: behavior-identical Rust port. This spec describes what the code ACTUALLY does (verified by executing the Python on 160+ inputs), including bugs. Every divergence between code and docs is flagged. Line numbers refer to the Python source.

---

## 0. Conventions, constants, entry points

- MIDI notes are ints 0–127. A "note set" is a `Set[int]`. Pitch class (PC) = `note % 12`.
- Note names: sharp table `['C','C#','D','D#','E','F','F#','G','G#','A','A#','B']`, flat table `['C','Db','D','Eb','E','F','Gb','G','Ab','A','Bb','B']`. Instance flag `prefer_flats` (constructor default `True`) selects the table via `get_note_name(pc)`. The module-level constant `PREFER_FLATS = True` is **dead** (never read by the class).
- `INVERSION_NAMES = {0:'', 1:'/3rd', 2:'/5th', 3:'/7th'}` is **dead data** — defined, never used. Inversions are expressed only as slash chords with actual note names.
- `min_notes_for_chord = 2`, `max_notes_for_chord = 7` (instance fields).
- Public API: `detect_chord(active_notes, lowest_note=None) -> Optional[str]`. The `lowest_note` parameter is **ignored** (compat shim); the bass is always `min(active_notes)`.
- Secondary APIs used internally: `detect_interval`, `detect_scale`, `_detect_chord_simple`, `_match_chord_pattern`, `_match_chord_type`, `_chord_complexity`, `is_clustered`.
- Output uses Unicode glyphs: `Δ` (maj7 family), and note that despite the `ø7` glyph appearing in code comments/tests, the emitted half-diminished spelling is always `m7b5` (see Bug B2).

### 0.1 Interval names (2-note detection), semitone → label

`0:P1 1:m2 2:M2 3:m3 4:M3 5:P4 6:d5 7:P5 8:m6 9:M6 10:m7 11:M7 12:P8 13:m9 14:M9 15:m10 16:M10 17:P11 18:A11 19:P12 20:m13 21:M13`; any other distance → the literal string `"{n} semitones"`.

---

## 1. Chord pattern database

95 patterns, in **exact dict-insertion order** (order matters twice: (a) ties in scoring are won by the first-encountered pattern because acceptance requires strictly greater score; (b) `_match_chord_type` pattern lookup during slash simplification takes the *first* matching entry). Intervals are semitones from candidate root, mod 12. `essential` = intervals that define quality (missing one costs −40 each and a candidate with *zero* essential matches is skipped); `optional` = freely omittable (−1 each); any other missing pattern member costs −8.

| # | chord_type | pattern | essential | optional |
|---|-----------|---------|-----------|----------|
| 0 | major | [0, 4, 7] | [4] | [0, 7] |
| 1 | minor | [0, 3, 7] | [3] | [0, 7] |
| 2 | diminished | [0, 3, 6] | [3, 6] | [0] |
| 3 | augmented | [0, 4, 8] | [4, 8] | [0] |
| 4 | sus2 | [0, 2, 7] | [2] | [0, 7] |
| 5 | sus4 | [0, 5, 7] | [5] | [0, 7] |
| 6 | 7sus4 | [0, 5, 7, 10] | [5, 10] | [0, 7] |
| 7 | 7sus2 | [0, 2, 7, 10] | [2, 10] | [0, 7] |
| 8 | 9sus | [0, 2, 5, 10] | [2, 5, 10] | [] |
| 9 | 9sus_with5 | [0, 2, 5, 7, 10] | [2, 5, 10] | [] |
| 10 | 13sus | [0, 2, 5, 9, 10] | [2, 10] | [] |
| 11 | 13sus_with5 | [0, 2, 5, 7, 9, 10] | [2, 10] | [] |
| 12 | 7sus13 | [0, 2, 5, 9, 10] | [2, 10] | [0, 7, 5] |
| 13 | sus13 | [0, 2, 5, 9] | [2, 9] | [0, 7, 5] |
| 14 | half_diminished7 | [0, 3, 6, 10] | [3, 10] | [0] |
| 15 | half_diminished11 | [0, 3, 6, 10, 5] | [6, 10] | [] |
| 16 | half_diminished11_no3 | [0, 5, 6, 10] | [6, 10] | [] |
| 17 | major7 | [0, 4, 7, 11] | [4, 11] | [0, 7] |
| 18 | major7#5 | [0, 4, 8, 11] | [4, 11] | [0] |
| 19 | minor7 | [0, 3, 7, 10] | [3, 10] | [0, 7] |
| 20 | dominant7 | [0, 4, 7, 10] | [4, 10] | [0, 7] |
| 21 | diminished7 | [0, 3, 6, 9] | [3, 9] | [0] |
| 22 | diminished_major7 | [0, 3, 6, 11] | [3, 6, 11] | [0] |
| 23 | 7b13_no5 | [0, 4, 10, 8] | [4, 10] | [] |
| 24 | augmented7 | [0, 4, 8, 10] | [4, 10] | [0] |
| 25 | minor_major7 | [0, 3, 7, 11] | [3, 11] | [0, 7] |
| 26 | minor_major9 | [0, 2, 3, 7, 11] | [3, 11] | [0, 7, 2] |
| 27 | major9 | [0, 4, 7, 11, 2] | [4, 11] | [0, 7] |
| 28 | minor9 | [0, 3, 7, 10, 2] | [3, 10] | [0, 7] |
| 29 | dominant9 | [0, 4, 7, 10, 2] | [4, 10] | [0, 7] |
| 30 | major11 | [0, 4, 7, 11, 2, 5] | [4, 11] | [0, 7] |
| 31 | major9#11 | [0, 4, 7, 11, 2, 6] | [4, 11, 6] | [0, 7] |
| 32 | major7#11 | [0, 4, 7, 11, 6] | [4, 11, 6] | [0, 7, 2] |
| 33 | major7#11_no5 | [0, 4, 6, 11] | [4, 11, 6] | [0, 2] |
| 34 | major7#11_shell | [0, 6, 11] | [6, 11] | [0, 4, 7, 2] |
| 35 | minor11 | [0, 3, 7, 10, 2, 5] | [3, 10] | [0, 7] |
| 36 | minor11_no5 | [0, 3, 10, 2, 5] | [3, 10] | [0] |
| 37 | minor11_no9 | [0, 3, 5, 7, 10] | [3, 10] | [] |
| 38 | minor11_shell | [0, 3, 5, 10] | [3, 10] | [0, 2] |
| 39 | major13 | [0, 4, 7, 11, 2, 5, 9] | [4, 11] | [0, 7, 5] |
| 40 | major13#11 | [0, 4, 7, 11, 2, 6, 9] | [4, 11] | [0, 7] |
| 41 | minor13 | [0, 3, 7, 10, 2, 5, 9] | [3, 10] | [0, 7] |
| 42 | dominant11 | [0, 4, 7, 10, 2, 5] | [4, 10] | [0, 7] |
| 43 | dominant13 | [0, 4, 7, 10, 2, 5, 9] | [4, 10] | [0, 7, 5] |
| 44 | 13_shell | [0, 4, 10, 9] | [4, 10] | [0, 7] |
| 45 | 13_no5_no11 | [0, 4, 10, 2, 9] | [4, 10] | [0, 7] |
| 46 | 13_no5 | [0, 4, 10, 2, 5, 9] | [4, 10] | [0, 7] |
| 47 | 7#11_no5 | [0, 4, 10, 6] | [4, 10] | [] |
| 48 | 7#11_no3_no5 | [0, 10, 2, 6] | [10, 6] | [] |
| 49 | 13#11_no3_no5 | [0, 10, 2, 6, 9] | [10, 6] | [] |
| 50 | 13#11_no9_no5 | [0, 4, 6, 9, 10] | [4, 10] | [] |
| 51 | 13#11_no5 | [0, 4, 10, 2, 6, 9] | [4, 10] | [] |
| 52 | 7b9 | [0, 4, 7, 10, 1] | [4, 10] | [0, 7] |
| 53 | 7#9 | [0, 4, 7, 10, 3] | [4, 10] | [0, 7] |
| 54 | 7#11 | [0, 4, 7, 10, 6] | [4, 10] | [0, 7] |
| 55 | 7b13 | [0, 4, 7, 10, 8] | [4, 10] | [0, 7] |
| 56 | 7b9#11 | [0, 4, 7, 10, 1, 6] | [4, 6, 10] | [0, 7] |
| 57 | 7#9#11 | [0, 4, 7, 10, 3, 6] | [4, 3, 6, 10] | [0, 7] |
| 58 | 7b9b13 | [0, 4, 7, 10, 1, 8] | [4, 10] | [0, 7] |
| 59 | 7#9b13 | [0, 4, 7, 10, 3, 8] | [4, 10] | [0, 7] |
| 60 | 7#11b13 | [0, 4, 7, 10, 6, 8] | [4, 10] | [0, 7] |
| 61 | 7b9#11b13 | [0, 4, 7, 10, 1, 6, 8] | [4, 10] | [0, 7] |
| 62 | 7#9#11b13 | [0, 4, 7, 10, 3, 6, 8] | [4, 10] | [0, 7] |
| 63 | 7b9#9 | [0, 4, 7, 10, 1, 3] | [4, 10] | [0, 7] |
| 64 | 7b9#9#11 | [0, 4, 7, 10, 1, 3, 6] | [4, 10] | [0, 7] |
| 65 | 7b9#9b13 | [0, 4, 7, 10, 1, 3, 8] | [4, 10] | [0, 7] |
| 66 | 9b13 | [0, 4, 7, 10, 2, 8] | [4, 10] | [0, 7] |
| 67 | 9b13_no5 | [0, 4, 10, 2, 8] | [4, 10] | [0] |
| 68 | 7#11_shell | [0, 10, 2, 6, 9] | [10, 6] | [0, 2, 9] |
| 69 | 7#11_no3 | [0, 7, 10, 6] | [10, 6] | [0, 2, 9] |
| 70 | 7#9#11_shell | [0, 10, 3, 6, 9] | [10, 3, 6] | [0, 6, 9] |
| 71 | 7b9#11_shell | [0, 10, 1, 6, 9] | [10, 1, 6] | [0, 6, 9] |
| 72 | 7b9#11_no3 | [0, 7, 10, 1, 6] | [10, 1, 6] | [0, 6, 9] |
| 73 | 7b9#11_no5 | [0, 4, 10, 1, 6] | [4, 10, 1, 6] | [0] |
| 74 | 7b9#11_13_no5 | [0, 1, 4, 6, 9, 10] | [4, 10, 1, 6] | [] |
| 75 | 7b9b13_no5 | [0, 4, 10, 1, 8] | [4, 10] | [0] |
| 76 | 7#9b13_no5 | [0, 4, 10, 3, 8] | [4, 10] | [0] |
| 77 | 7b9#9_no5 | [0, 4, 10, 1, 3] | [4, 10] | [0] |
| 78 | altered | [0, 4, 7, 10, 1, 3, 6, 8] | [4, 10] | [0, 7] |
| 79 | add9 | [0, 4, 7, 2] | [4] | [0, 7] |
| 80 | minor_add9 | [0, 3, 7, 2] | [3] | [0, 7] |
| 81 | 6 | [0, 4, 7, 9] | [4, 9] | [0, 7] |
| 82 | 6_no5 | [0, 4, 9] | [4, 9] | [0] |
| 83 | 6add4 | [0, 4, 5, 7, 9] | [4, 9] | [0, 7] |
| 84 | 6add4_no5 | [0, 4, 5, 9] | [4, 9] | [0] |
| 85 | 6_9 | [0, 4, 7, 9, 2] | [4, 9] | [0, 7] |
| 86 | 6_9_no5 | [0, 4, 9, 2] | [4, 9] | [0] |
| 87 | 6_9_no3 | [0, 2, 7, 9] | [9, 2] | [0, 7] |
| 88 | major7_6_9 | [0, 4, 7, 9, 11, 2] | [4, 11, 9] | [] |
| 89 | minor6 | [0, 3, 7, 9] | [3, 9] | [0, 7] |
| 90 | minor6_no5 | [0, 3, 9] | [3, 9] | [0] |
| 91 | minor6_9 | [0, 3, 7, 9, 2] | [3, 9] | [0, 7] |
| 92 | minor6_9_no5 | [0, 2, 3, 9] | [3, 9] | [0] |
| 93 | add11 | [0, 4, 7, 5] | [4] | [0, 7] |
| 94 | 5 | [0, 7] | [7] | [0] |
### 1.2 Scale patterns

| scale | pattern |
|-------|---------|
| Ionian | [0, 2, 4, 5, 7, 9, 11] |
| Dorian | [0, 2, 3, 5, 7, 9, 10] |
| Phrygian | [0, 1, 3, 5, 7, 8, 10] |
| Lydian | [0, 2, 4, 6, 7, 9, 11] |
| Mixolydian | [0, 2, 4, 5, 7, 9, 10] |
| Aeolian | [0, 2, 3, 5, 7, 8, 10] |
| Locrian | [0, 1, 3, 5, 6, 8, 10] |
| Melodic Minor | [0, 2, 3, 5, 7, 9, 11] |
| Dorian b2 | [0, 1, 3, 5, 7, 9, 10] |
| Lydian Augmented | [0, 2, 4, 6, 8, 9, 11] |
| Lydian Dominant | [0, 2, 4, 6, 7, 9, 10] |
| Mixolydian b6 | [0, 2, 4, 5, 7, 8, 10] |
| Locrian #2 | [0, 2, 3, 5, 6, 8, 10] |
| Altered | [0, 1, 3, 4, 6, 8, 10] |
| Harmonic Minor | [0, 2, 3, 5, 7, 8, 11] |
| Locrian #6 | [0, 1, 3, 5, 6, 9, 10] |
| Ionian #5 | [0, 2, 4, 5, 8, 9, 11] |
| Dorian #4 | [0, 2, 3, 6, 7, 9, 10] |
| Phrygian Dominant | [0, 1, 4, 5, 7, 8, 10] |
| Lydian #2 | [0, 3, 4, 6, 7, 9, 11] |
| Altered Diminished | [0, 1, 3, 4, 6, 8, 9] |
| Major Pentatonic | [0, 2, 4, 7, 9] |
| Minor Pentatonic | [0, 3, 5, 7, 10] |
| Major Blues | [0, 2, 3, 4, 7, 9] |
| Minor Blues | [0, 3, 5, 6, 7, 10] |
| Whole Tone | [0, 2, 4, 6, 8, 10] |
| Whole-Half Diminished | [0, 2, 3, 5, 6, 8, 9, 11] |
| Half-Whole Diminished | [0, 1, 3, 4, 6, 7, 9, 10] |

### 1.1 Display-name mapping (chord_type → rendered suffix)

Formatting is a literal `elif` chain (lines 1790–1972). All shell/no5/no3 variants collapse to the parent display name. Verbatim mapping (root name `R` prepended):

| chord_type(s) | rendered |
|---|---|
| major | `R` |
| minor | `Rm` |
| diminished | `Rdim` |
| augmented | `Raug` |
| sus2 | `R2` |
| sus4 | `R4` |
| 7sus4 / 7sus2 | `R7sus4` / `R7sus2` |
| 9sus, 9sus_with5 | `R9(sus)` |
| 13sus, 13sus_with5 | `R13(sus)` |
| 7sus13 | `R7sus13` |
| sus13 | `Rsus13` |
| major7 | `RΔ7` |
| major7#5 | `RΔ7#5` |
| minor7 | `Rm7` |
| minor_major7 | `RmΔ7` |
| minor_major9 | `RmΔ7(9)` |
| dominant7 | `R7` |
| diminished7 | `Rdim7` |
| diminished_major7 | `RdimΔ7` |
| half_diminished7 | `Rm7b5` |
| half_diminished11, half_diminished11_no3 | `Rm7b5(11)` |
| dominant9 / dominant11 / dominant13 | `R9` / `R11` / `R13` |
| 13_shell, 13_no5_no11, 13_no5 | `R13` |
| 7#11_no5, 7#11_no3_no5, 7#11, 7#11_shell, 7#11_no3 | `R7(#11)` |
| 13#11_no3_no5, 13#11_no9_no5, 13#11_no5 | `R13(#11)` |
| major9 | `RΔ9` |
| minor9 | `Rm9` |
| major11 | `RΔ11` |
| major7#11, major7#11_no5, major7#11_shell | `RΔ7(#11)` |
| major9#11 | `RΔ9(#11)` |
| minor11, minor11_no5, minor11_no9, minor11_shell | `Rm11` |
| major13 | `RΔ13` |
| major13#11 | `RΔ13#11` |
| minor13 | `Rm13` |
| altered | `R7alt` |
| 7b9 | `R7(b9)` |
| 7#9 | `R7(#9)` |
| 7b13, 7b13_no5 | `R7(b13)` |
| 9b13, 9b13_no5 | `R9(b13)` |
| 7b9#11, 7b9#11_shell, 7b9#11_no3, 7b9#11_no5, 7b9#11_13_no5 | `R7(b9,#11)` |
| 7#9#11, 7#9#11_shell | `R7(#9,#11)` |
| 7b9b13, 7b9b13_no5 | `R7(b9,b13)` |
| 7#9b13, 7#9b13_no5 | `R7(#9,b13)` |
| 7b9#11b13 | `R7(b9,#11,b13)` |
| 7#9#11b13 | `R7(#9,#11,b13)` |
| 7b9#9 , 7b9#9_no5 | `R7(b9,#9)` |
| 7b9#9#11 | `R7(b9,#9,#11)` |
| 7b9#9b13 | `R7(b9,#9,b13)` |
| 5 | `R5` |
| 6, 6_no5 | `R6` |
| 6add4, 6add4_no5 | `R6add4` |
| 6_9, 6_9_no5, 6_9_no3 | `R6/9` |
| major7_6_9 | `Rmaj7(6/9)` |
| minor6, minor6_no5 | `Rm6` |
| minor6_9, minor6_9_no5 | `Rm6/9` |
| add9 | `R(add9)` |
| minor_add9 | `Rm(add9)` |
| add11 | `Radd11` |
| **fallback** (unmapped type) | `R{chord_type}` — reachable only for `altered` alias path; in practice `half_diminished11*` etc. are all mapped. NOTE: the `altered` scale-name fallback appears when scale detection returns `"C Altered"` (a SCALE name, from `detect_scale`), not from this chain. |

**Format inconsistency to preserve**: the *early* 7b9 return path (§2.4 case E2) emits `"{R}7b9"` (no parentheses), while the pattern-scored path emits `"{R}7(b9)"`.

---

## 2. Top-level pipeline — `detect_chord(active_notes)`

Ordered steps; each early `return` short-circuits everything below it.

```
S1  if |active_notes| < 2: return None
S2  if |active_notes| == 2: return detect_interval(active_notes)          # §3
S3  original_active_notes = copy(active_notes)
    pitch_classes_all = sorted unique PCs
S4  # scale flag (computed BEFORE any note reduction, on the FULL set)
    should_check_scale_later =
        |pitch_classes_all| >= 5 AND
        ( (max(active_notes)-min(active_notes) < 12) OR is_clustered(active_notes) )   # §7
S5  # 7-unique-PC early scale fallback
    if |pitch_classes_all| == 7:
        lowest_pc = min(active_notes) % 12
        iv = {(pc - lowest_pc) % 12}
        has_third   = 3 in iv or 4 in iv
        has_seventh = 10 in iv or 11 in iv
        if NOT (has_third AND has_seventh):
            scale = detect_scale(active_notes)                              # §6
            if scale and scale.startswith(get_note_name(lowest_pc)): return scale
        # else fall through to chord detection
S6  # too many notes
    if |active_notes| > 7:
        keep the 7 most common PCs (Counter over [n%12 for n in active_notes],
        most_common(7)); active_notes = {n : n%12 in kept}
        # TIE-BREAK HAZARD: with all counts equal, Counter preserves first-seen
        # order of iteration over the *set* — i.e. CPython small-int set iteration
        # order. Not semantically specified; a port must pick/replicate an order.
S7  pitch_classes = sorted unique PCs (post-reduction); if < 2: return None
S8  EARLY SPECIAL CASES (E1..E4, §2.4) — may return immediately
S9  highest_note/highest_pc, lowest_note/lowest_pc computed
S10 has_global_dominant_quality = ∃ pc ∈ pitch_classes with
        (pc+4)%12 ∈ pitch_classes AND (pc+10)%12 ∈ pitch_classes
S11 ROOT LOOP: for root_pc in pitch_classes (ascending):
        intervals = sorted((pc - root_pc) % 12)
        r = _match_chord_pattern(intervals, root_pc, active_notes,
                                 highest_note, highest_pc, lowest_pc,
                                 has_global_dominant_quality)                # §4
        if r and r.score > best_score: (best_match, best_score, best_root_pc) = r + root
        # strict '>' ⇒ earliest (lowest) root wins ties
S12 POST-LOOP dim7→7(b9) forcing (§2.5)
S13 diminished / augmented symmetry re-rooting (§2.6)
S14 slash-chord & simplification stage (§2.7) — appends "/{bass}" or rewrites name
S15 if should_check_scale_later:
        span = max(original) - min(original)
        if span >= 12: return best_match            # open voicing: chord wins
        scale = detect_scale(original_active_notes)
        return scale if scale else best_match
S16 return best_match           # may be None if nothing scored > 10
```

### 2.4 Early special cases (run in this order, each may `return`)

**E1 — 4-PC m6 slash `[0,1,7,10]`** (lines 604–618). If exactly 4 unique PCs and sorted intervals-from-lowest == `[0,1,7,10]`: root is PC at bass+10; return `"{root}m6/{bass}"`. Example: C Bb Db G → `Bbm6/C`.

**E1b — 5-PC m6 slash `[0,1,5,7,10]`** (620–634). Same rule with 5 PCs and pattern `[0,1,5,7,10]`: return `"{bass+10}m6/{bass}"`. Example: C Bb Db F G → `Bbm6/C`.

**E2 — 5-PC dim7 upper structure** (636–669). If exactly 5 unique PCs: let `rest` = the 4 PCs ≠ bass PC. For each candidate root in `rest` (iteration order = sorted-PC order with bass removed): if intervals of `rest` from that candidate == `[0,3,6,9]` (a dim7):
- if `rest`'s intervals **from the bass** contain all of {4,7,10,1} → return `"{bass}7b9"` (**no parentheses** — format anomaly);
- else → return `"{dim7Root}dim7/{bass}"`. Example: C D F Ab Cb → `Ddim7/C`; C E G Bb Db → `C7b9`.
The first dim7 found returns; no scoring.

**E3 — 4-PC half-diminished vs m6** (671–703). If exactly 4 unique PCs: for each PC p (ascending): if intervals of the 4 PCs from p == `[0,3,6,10]`:
- p == bass PC → return `"{p}m7b5"`  (this is why ø7 never appears: every pure 4-note m7b5 exits here);
- else, m6root = (p+3)%12; if m6root == bass → return `"{m6root}m6"`; else return `"{m6root}m6/{bass}"`.
Examples: G Bb Db F → `Gm7b5`; Bb Db F G → `Bbm6`; D F Ab C (F bass) → `Fm6`.

**Consequence**: `half_diminished7`'s huge 700 completeness bonus and its `Rm7b5` formatter inside `_match_chord_pattern` are unreachable from `detect_chord` for exact 4-PC sets (E3 short-circuits); they remain reachable via `_detect_chord_simple` (slash simplification, which has no early cases).

### 2.5 Post-loop dim7 + M3-below ⇒ 7(b9) forcing (lines 750–789)

Runs only when 4 or 5 unique PCs (note: 5-PC inputs matching E2 already returned; this catches e.g. 4-note F A C Eb-type sets and 5-PC sets whose dim7 isn't the 4 non-bass notes). For each `potential_root` in pitch_classes:
- `m3_above = (potential_root+4)%12` must be a present PC.
- `remaining` = pitch_classes minus potential_root (5-PC case) or all pitch_classes (4-PC case).
- If sorted intervals of `remaining` from `m3_above` == `[0,3,6,9]` **and** `(potential_root+10)%12 ∈ remaining`:
  re-run `_match_chord_pattern` with intervals from `potential_root`; if it returns a name containing `'7(b9)'` **or containing `'7'` at all** (extremely loose test), overwrite best_match/root/score with it and break.
Outer break: `if best_match and '7(b9)' in best_match`.

### 2.6 Symmetric-chord re-rooting (lines 791–835)

Skipped when best name contains `'7(b9)'`.
- **Triadic diminished**: if best is `Xdim` and best_root ≠ bass PC → rename to `"{bassName}dim"`, set root=bass **without re-scoring** (the bass need not even be in the dim triad — see Bug B19: C Eb A → `Cdim`).
- **dim7**: re-run `_match_chord_pattern` from the bass; adopt only if the result is also diminished7. (Cdim7 over Eb bass → `Ebdim7`.)
- **augmented / augmented7**: same re-root-from-bass procedure (E G# C → `Eaug`).
Type checks use `_match_chord_type` (§8).

### 2.7 Slash chord & simplification stage (lines 837–1044)

Runs only when `best_match` exists and `lowest_pc != best_root_pc`. Let `bi = (lowest_pc − best_root_pc) % 12`.

```
is_extended = ('9' in name or '11' in name or '13' in name) and 'add9' not in name
is_altered  = any of 'b9','#9','b13','#11' in name
is_six_nine = '6/9' in name or '(6/9)' in name
if is_six_nine and bi == 2:            skip_slash = False
else: skip_slash = (is_extended and bi in {2,5,7,9,10})
                 or (is_altered and bi in {1,3,6,8})
if best is diminished7 or augmented/augmented7 (via _match_chord_type): skip_slash = True
if skip_slash: return name unchanged (bass silently treated as chord member)
```

Otherwise the simplification decision tree runs, then `"/{bassName}"` is appended:

1. `best_pattern` = pattern of the **first** CHORD_PATTERNS entry whose type `_match_chord_type(best_match, type)` accepts. NOTE: `add9` best-matches render as `"R(add9)"`, whose quality string `"(add9)"` is NOT in the quality map ⇒ `best_pattern = None` for add9 (harmless because add9 never simplifies, rule 7).
2. Defaults: `should_simplify = True`, `special_case_no_simplify = False`, `essential_intervals = {0,3,4,6,7,8}`.
3. **Bb6/C voicing gate**: if sorted intervals-from-lowest ∈ {`[0,2,5,7,10]`, `[0,2,7,10]`}: look at the *second lowest sounding note*; whatever its interval (10 or not), set `should_simplify=False, special_case_no_simplify=True` (both branches do the same). This preserves `Bb6/C` or `Gm/C`-style names chosen during scoring.
4. If `is_extended_chord` (`'9'|'11'|'13'|'6/9' in name`) and `bi ∈ best_pattern`: `should_simplify=False`.
5. Essential-set augmentation: dimΔ7 adds {6,11}; m7b5 adds {3,6,10}; "dominant" names (endswith `'7'` or contains `'7('` or endswith `'13'`, and none of `Δ7|dim7|ø7|m7` in name) add {10}.
6. If not special-cased: `bi ∈ essential_intervals` ⇒ `should_simplify=False` **unless** best name endswith `'m'`, or contains `'add9'`, or (len ≤ 2 and doesn't end `'7'`/`'6'`) — i.e. plain triads get a chance to re-simplify into sus chords. `bi ∉ essential` ⇒ `should_simplify=True`.
7. **Never simplify** sus (name ends `'2'`/`'4'` or contains `sus2/sus4/sus13`) or anything containing `'add9'`.
8. **7th-chord bass-doubling rule**: if not special-cased, not sus, `'7' in name` and none of `Δ7|m7|dim7`: count sounding notes whose PC == bass PC; count==1 ⇒ `should_simplify=True` (drop to triad), count≥2 ⇒ `False`. (Bb E G C → `C/Bb`; with two Bbs → `C7/Bb`.)
9. If simplifying: `notes_without_bass = notes with PC != bass PC`. Guard: if `|notes_without_bass| < 3` and total unique PCs == 3 ⇒ don't simplify (protects `C/E`). If ≥2 notes remain: `alt = _detect_chord_simple(notes_without_bass)` (full scoring, no early cases, no slash stage). Then:
   - add9→sus2 rewrite: if current contains `add9` and alt endswith `'4'` and the upper-structure PCs contain the current root forming exactly `[0,2,7]` from it ⇒ `alt = currentRoot + '2'`.
   - if alt is sus and endswith `'2'` and current is add9 ⇒ take alt;
   - elif alt is sus and current matches `^[A-G][b#]?m?$` ⇒ take alt;
   - elif `_chord_complexity(alt) <= _chord_complexity(current)` ⇒ take alt.  (§8 complexity: 13→5, 11→4, 9 or 6/9→3, add/6→3, 7→2, else 1; substring tests in that order.)
10. Append `"/{bassName}"` (always, in the non-skip branch, whether or not simplified).

---

## 3. Two-note interval detection — `detect_interval`

Exactly 2 MIDI notes. `semis = upper − lower` (actual distance, NOT mod 12). Output `"{lowerNoteName} ({intervalLabel})"` using §0.1; distances > 21 render `"{n} semitones"`. Examples: {60,64} → `C (M3)`; {60,72} → `C (P8)`; {60,82} → `C (22 semitones)`.
**Bug B1**: the module's own test table expects bare labels (`"M3"`); real output includes the root prefix.

---

## 4. Pattern scoring — `_match_chord_pattern(intervals, root_pc, active_notes, highest_note, highest_pc, lowest_pc, has_global_dominant_quality)`

Evaluates ALL 95 patterns for one candidate root and returns the best `(name, score)` or `None`. Per pattern:

```
pattern_set, intervals_set  (sets; intervals are unique PC-intervals from root)
matched  = pattern ∩ intervals ; extra = intervals − pattern ; missing = pattern − intervals
essential = ESSENTIAL[type] (default []); optional = OPTIONAL[type] (default [])
essential_matched = essential ∩ matched ; essential_missing = essential − matched
input_pc_count = |unique PCs of active_notes|
```

**Hard skips** (in order):
1. type ∈ {7b9#11, 7#9#11, 7#9#11_shell, 7b9#11_shell, 7b9#11_no3} and essential_missing ≠ ∅ → skip.
2. essential ≠ ∅ and essential_matched == ∅ → skip.
3. |matched| < 2 → skip.

**Score components** (exact values):

| # | component | formula |
|---|-----------|---------|
| 1 | essential_score | essential ≠ ∅: `(|essential_matched|/|essential|) * 60.0`; else flat `30.0` |
| 2 | percentage_match | `(|matched| / input_pc_count) * 40.0` |
| 3 | highest_note_bonus | `10.0` if `(highest_pc − root_pc) % 12 ∈ pattern` |
| 4 | completeness_bonus | perfect (missing=∅ and extra=∅): `30.0`; overridden for perfect matches of: altered-dominant set {7b13_no5,7b9b13_no5,7#9b13_no5,7b9#11_no5,7b9,7#9,7b13,7b9b13,7#9b13,7#11b13,7b9#11,7#9#11} → `60.0`; diminished_major7 → `500.0`; half_diminished7 → `700.0`; major7_6_9 → `200.0`. Non-perfect but missing=∅ (extras present): `10.0` |
| 5 | extra_penalty | `extra_count * 3.0` |
| 6 | missing_penalty | `40.0 * |essential_missing| + 1.0 * |optional ∩ missing| + 8.0 * |missing − optional − essential|` |
| 7 | rootless_bonus | `15.0` if `0 ∈ missing` and all essentials matched and `|essential| ≥ 2` |
| 8 | root_in_bass_bonus | `15.0` if `root_pc == lowest_pc` and `0 ∈ matched` |
| 9 | characteristic_bonus | `10.0` if `6 ∈ matched or 8 ∈ matched`; **plus** `50.0` if type ∈ {7#11_shell, 7#11_no3, 7#9#11_shell, 7b9#11_shell, 7b9#11_no3} |
| 10 | dominant_quality_adjustment | let `hdq = has_global_dominant_quality OR (4 ∈ intervals AND 10 ∈ intervals)`. If hdq: type starts with `'6'` or `'minor6'` or is `diminished7`/`diminished` → `−500.0`; type starts with `'13'` or `'dominant'` → (`+600.0` if type==dominant7 and perfect else `+50.0`); otherwise `0` (NB: types starting `'7…'` get no boost) |
| 11 | special_pattern_bonus | single float, default 0; assigned by the ordered rule list §4.1 — a LATER matching rule OVERWRITES an earlier one |
| 12 | inversion_bonus | §4.2 |

`score = 1+2+3+4+7+8+9+10+11+12 − 5 − 6`.
Acceptance: `score > best_score_so_far AND |matched| >= 2 AND score > 10.0` (strict > ⇒ earlier pattern in dict order wins ties).

### 4.0 minor7 → relative-major-6 reinterpretation (CRITICAL QUIRK, lines 1774–1784)

Inside the acceptance block, if the accepted type is `minor7` and `max(active_notes) − min(active_notes) < 12` (closed voicing, measured on the whole sounding set), then:
`root_pc = (root_pc + 3) % 12; chord_type = '6'` — the chord is renamed as the major 6 of the m3 (Am7 closed → `C6`). No completeness check despite the comment claiming one.
**Side effects to replicate exactly**:
- `detect_chord` still records `best_root_pc` = the ORIGINAL loop root, so slash comparison uses the m7 root (that's why closed Am7 with A bass shows no slash).
- The local `root_pc` variable stays mutated for ALL REMAINING pattern iterations of this call: subsequent patterns compute names, `root_in_bass_bonus`, `bass_interval`, etc. against the shifted root. Observable: {60,65,67,70} (C7sus4 voicing) → minor7-from-G is accepted mid-loop, root mutates G→Bb, then minor11_shell matches perfectly (+8000) and is misnamed `Bbm11` (final output `Bbm11`, musically wrong root).
- Consequence: any complete m7 within one octave is reported as the relative 6th chord: closed Cm7 (60,63,67,70) → `Eb6` (contradicts doc §1; see Bug B3). Open voicing (span ≥ 12) keeps `Cm7`.

### 4.1 special_pattern_bonus rules — exact, in source order (later assignment wins)

Pre-computed for several rules: `ifl` = sorted intervals-from-lowest over unique PCs; `upc` = unique PC count; `sorted_notes` = ascending MIDI notes; `second_interval` = (PC of 2nd lowest sounding note − lowest_pc) % 12.

| id | condition | bonus |
|----|-----------|-------|
| S1 | type==`7b13_no5` and intervals==`[0,4,8,10]` | 100 |
| S1b | type==`7b9b13_no5` and intervals==`[0,1,4,8,10]` | 150 |
| S1c | type==`7#9b13_no5` and intervals==`[0,3,4,8,10]` | 150 |
| S1d | type==`7b9#11_no5` and intervals==`[0,1,4,6,10]` | 400 |
| S-m6a | `ifl==[0,1,7,10]` and not global-dominant and type ∈ {minor6, minor6_no5, minor6_9_no5} and root≠bass | 1500 (in practice unreachable from detect_chord: E1 already returned; live via `_detect_chord_simple`) |
| S-dim | type==`diminished` and upc ≥ 4 | −1000 |
| S1e | type ∈ {6_no5, 6} and root==bass and intervals==`[0,4,9]` | 100 |
| S1f | type==`add9`, perfect match: root≠bass and `ifl==[0,2,5,10]`: if add9 triad complete (0,3/4,7 all in intervals): bass-interval ∉ {0,3,4,7} → span(max−min of notes) < 12 ? **6200** : 150; bass ∈ triad → 4200. Triad incomplete or no P5/3rd → 150. `ifl` ≠ 9sus-pattern → 150. root==bass → 150 | see left |
| S1f0 | type==`minor_add9`, perfect | 50 |
| S1f1 | type ∈ {minor6, minor6_no5, minor6_9, minor6_9_no5}, root≠bass, not global-dominant, 3∈iv and 9∈iv and upc==4 | intervals==`[0,2,3,9]` → 600 else 400 |
| S1f2 | type==`half_diminished7` and intervals==`[0,3,6,10]` and perfect | 180 (dead from detect_chord — E3; live via _detect_chord_simple) |
| S1g | type ∈ {sus2, sus4}, root==bass, essential_missing=∅, missing ≤ 1, extra=0, and bonus still 0.0 | 80 |
| S2 | type ∈ {major7#11, major7#11_no5, major9#11, major13#11} and 6 ∈ iv | perfect → 250; missing ≤ 1 → 150 |
| S2b | type==`major7#11_no5` and intervals==`[0,4,6,11]` | 300 |
| S2c | type ∈ {6_9, 6_9_no5}: (9∈iv and 2∈iv and root==bass) → perfect 9000 / missing≤1 220; elif 9∉iv → −300. type==`6_9_no3`: 9,2∈iv and root==bass → perfect 290 / missing≤1 220 | see left |
| S2c2a | type ∈ {minor6_9, minor6_9_no5} and 9,2,3 ∈ iv and root==bass and perfect | 9500 |
| S2d1 | type==`major7_6_9`: perfect and root==bass → 10000; elif 9∉iv → −300 | see left |
| S2c2b | 3∈iv and 9∈iv and upc==4: if type ∈ {minor6, minor6_no5, minor6_9, minor6_9_no5}: perfect → 450, elif missing≤1 and extra≤2 → 410; **else (ANY other type!) → 380** | see left. The 380 branch is a blanket distorter: e.g. G B F Ab from root Ab gives `AbmΔ7` +380 |
| S2d2 | type ∈ {13_shell, 13_no5_no11, 13_no5}, root==bass, 4,10,9 ∈ iv | perfect 250 / missing≤1 180 |
| S2e | type==`half_diminished11_no3`, intervals==`[0,5,6,10]`, root==bass, second_interval==5 | 300 |
| S2f | type ∈ {7#11_no5, 7#11_no3_no5, 13#11_no3_no5, 13#11_no9_no5, 13#11_no5}, root==bass, 10,6 ∈ iv | perfect 250 / missing≤1 180 |
| S2g | type ∈ {minor11, minor11_no5, minor11_no9, minor11_shell}, perfect | 8000 |
| S2h | type ∈ {9sus, 9sus_with5, 13sus, 13sus_with5}, perfect, root==bass | span(max−min) ≥ 12 → 6400; else 150 |
| S2i | type==`7b9#11_13_no5`, perfect | 260 |
| S2i2 | type ∈ {9b13, 9b13_no5}, perfect, root==bass | 250 |
| S2j | type==`dominant9`, root==bass, missing ≤ 1, extra == 0 | 200 |
| S2k | `ifl` ∈ {`[0,2,5,7,10]`,`[0,2,7,10]`}: if second_interval==10 ("Bb6/C voicing"): type==`6` and (root−bass)%12==10 → 250; type ∈ {6_9,6_9_no5} and (root−bass)%12==10 → −100; type ∈ {minor7, minor} → −200. Else: type ∈ {minor7, minor} and (root−bass)%12==7 → 200; type==`6` and (root−bass)%12==10 → −200 | see left |
| S2l | intervals==`[0,2,4,7,9]` and type==`6` | 200 (OVERWRITES S2k's 250 when both hit) |

### 4.2 inversion_bonus (lines 1708–1755)

`bass_interval = (lowest_pc − root_pc) % 12`.
- `is_triad` = type ∈ {major, minor, diminished, augmented}; bass_interval ∈ {3,4,7} → **+35**.
- `is_seventh` = type ∈ {major7, minor7, dominant7, diminished7, diminished_major7, half_diminished7, augmented7, minor_major7} OR (type starts with `'7'` and contains one of b9/#9/#11/b13, or type=='altered' — note `'altered'` doesn't start with '7' so the altered arm is actually false); if bass_interval ∈ pattern and ≠ 0 → **+40**.
- Sixth-chord anti-bonus: if type ∈ {6, 6_no5, minor6, minor6_no5, 6_9, 6_9_no5, 6_9_no3, minor6_9, 6add4, 6add4_no5} and bass_interval == 0: test whether PCs ⊇ minor triad from (bass−3): if yes, then if the M6 PC `(root+9)%12` equals highest_pc AND |active_notes| ≥ 4 → **+45**, else **−40** (prefer the minor-triad inversion reading; e.g. Eb G C stays candidate `Cm/Eb` unless… in practice S1e's +100 rescues 3-note `Eb6`).

---

## 5. `_detect_chord_simple(notes)` (slash simplification helper)

Same root loop + `_match_chord_pattern` as S10–S11, but: no early cases, no dim/aug re-rooting, no 7b9 forcing, no slash stage, no scale check. Recomputes its own `has_global_dominant_quality` and highest/lowest from the reduced note set. Returns bare name or None.

## 6. Scale detection — `detect_scale(active_notes)`

```
if |notes| < 5 or |unique PCs| < 5: return None
is_clustered (§7); is_within_octave = (max−min) < 12
clustered_only = {Major Pentatonic, Minor Pentatonic, Major Blues, Minor Blues, Whole Tone}
for root_pc in sorted unique PCs:
  intervals = set of (pc−root)%12
  for (scale_name, pattern) in SCALE_PATTERNS (dict order):
     if scale_name ∈ clustered_only and not (is_clustered or is_within_octave): continue
     if scale_name == 'Whole Tone' and |PCs| < 6: continue
     if pattern ⊄ intervals: continue          # every scale tone must be present
     extra = |intervals − pattern|
     if extra == 0:
         score = 5000 + |pattern|
         + 1000 if scale_name in the 21 seven-note modes (major/melodic-minor/harmonic-minor families)
     else:
         score = |pattern|*10 − extra*5        # weak partial path
     if root_pc == lowest_pc: score += 500
     keep max (strict >)
return "{rootName} {scale_name}" or None
```

Scale families: Ionian…Locrian; Melodic Minor, Dorian b2, Lydian Augmented, Lydian Dominant, Mixolydian b6, Locrian #2, Altered; Harmonic Minor, Locrian #6, Ionian #5, Dorian #4, Phrygian Dominant, Lydian #2, Altered Diminished; Major/Minor Pentatonic; Major/Minor Blues; Whole Tone; Whole-Half and Half-Whole Diminished (8-note). Patterns as in the table above (§1.2).

Scale detection is invoked from exactly two places: S5 (7-PC, lowest note lacks 3rd+7th, with the `startswith` root filter) and S15 (post-chord clustered check, unfiltered).

## 7. `is_clustered(notes)`

`False` if |notes| < 5. Over sorted MIDI notes, count adjacent pairs with gap ≤ 2 vs gaps ≥ 3; clustered iff `adjacent/(n−1) ≥ 0.6`.

## 8. `_match_chord_type(name, type)` and `_chord_complexity`

`_match_chord_type`: strip `/bass`; parse root (2-char flat/sharp names first, then 1-char); map the remaining quality string through a fixed table (`'' → major`, `m → minor`, `dim`, `aug`, `2/4 → sus`, `Δ7`, `m7`, `mΔ7`, `mΔ7(9)`, `7`, `dim7`, `dimΔ7`, `ø7 → half_diminished7` (never produced!), `9/11/13`, `Δ9/m9/Δ11/Δ7#11/m11/Δ13/Δ13#11/m13`, `7alt`, `5`, `6`, `6/9`, `m6`, `m6/9`, `add9`, `add11`, `7sus4/7sus2/7sus13/sus13`). Special: quality `'13'` matches any of {dominant13, 13_shell, 13_no5_no11, 13_no5}. Qualities like `(add9)`, `m7b5`, `9(sus)`, `7(b9)` are NOT in the map → no pattern found (affects §2.7 step 1).

`_chord_complexity`: 999 if empty; strip bass; `'13'`→5, `'11'`→4, `'9' or '6/9'`→3, `'add' or '6'`→3, `'7'|'Δ7'|'ø7'`→2, else 1 (substring checks in that order).

---

## 9. Complete inventory of special cases & heuristics

1. **2 notes = interval, never a chord** (even a P5 "power chord" needs 3 sounding notes: C G C → `C5`).
2. **m6-slash early exits** E1/E1b (`[0,1,7,10]`, `[0,1,5,7,10]` from bass → `{bass+10}m6/{bass}`).
3. **dim7-over-bass early exit** E2, with the 7b9 promotion (upper dim7 containing 3-5-b7-b9 of bass → `{bass}7b9`).
4. **half-dim vs m6** E3: m7b5 root in bass → `m7b5`; otherwise reinterpret as m6 (root = m7b5 root + 3), slash if that root isn't the bass. (Doc §"CRITICAL EARLY SPECIAL CASE 3" comments match code.)
5. **Global dominant-quality veto**: if ANY present PC has M3+m7 above it, 6th/m6/dim/dim7 readings take −500 and dominant readings +50/+600 — from every candidate root, not just the dominant one.
6. **minor7 closed-voicing → relative 6** (§4.0) with root-mutation side effect.
7. **add9 vs 9sus span rule** (S1f + S2h): bass-relative pattern `[0,2,5,10]`; total span < 12 → `{root}(add9)/{bass}` wins (6200), span ≥ 12 → `{bass}9(sus)` wins (6400). Root-position 9sus with compact span only gets 150 and can LOSE to the add9-from-b7 reading: C D F Bb compact → `Bb(add9)/C` (contradicts doc; see B13).
8. **minor_add9 beats minor-triad inversion** (+50 perfect): G C D Eb → `Cm(add9)/G`.
9. **sus chords in root position** +80 (sus2/sus4, near-perfect only).
10. **sus2 preferred over sus4 upper structure for add9 slash rewrites** (§2.7 step 9): Ebadd9/G upper Bb-Eb-F → `Eb2/G` path exists, though live inputs tested produce `Eb(add9)/G` because bass G ∈ triad blocks simplification via rule 6/7.
11. **maj7#11 family boost** (S2/S2b): FABE → `FΔ7(#11)`; C F# B shell → `CΔ7(#11)`.
12. **6/9 tower**: m6/9 (9500) > 6/9 (9000) > … and maj7(6/9) 10000 > 6/9; all root-in-bass only.
13. **m3+M6 blanket bonus** (S2c2b): with exactly 4 unique PCs, ANY pattern whose interval-set from its root contains {3,9} gets ≥380 — deliberately kills dim readings but also warps unrelated ones (`AbmΔ7` in G B F Ab).
14. **13-shell boost** (S2d2) and **7#11/13#11 boost** (S2f), root in bass.
15. **m11 family perfect = 8000** (S2g): quintal voicings (C Eb Bb D F → `Cm11`) beat scales (≈6500) but lose to maj7(6/9) (10000). Side effect: many sus/add sets get eaten by m11 from another root (C6/9-no3 test → `Am11/C`, C9sus-with5 spread → `Gm11`).
16. **9b13** (S2i2 +250): C Bb D E Ab → `C9(b13)` (doc §8 works).
17. **dominant9 root-in-bass** +200 (S2j).
18. **Bb6/C voicing discrimination** (S2k/S2l + §2.7 step 3): `[0,2,(5,)7,10]` from bass with the b7 as second-lowest note → `{bass+10}6/{bass}`; otherwise prefer minor/minor7 from the 5th (`Gm/C`). 5-note version with 4th present is usually overridden by pentatonic scale detection or m11 (see vectors).
19. **Em7b5(11) voicing** (S2e): E A Bb D with A as 2nd note → `Em7b5(11)`.
20. **7b9 forced from dim7+M3-below** (§2.5): F A C Eb Gb type sets → `F7b9`.
21. **Triadic dim renamed to bass**; dim7/aug re-rooted to bass (symmetry).
22. **Slash-skip for extensions in bass**: extended chords never show a slash when the bass is the 9/11/5/13/b7 (bi ∈ {2,5,7,9,10}); altered chords never when bi ∈ {1,3,6,8}; exception 6/9 chords with 9 in bass DO slash (`Bb6/9`-family /C).
23. **7th-chord bass-doubling** simplification (Bb E G C → `C/Bb`, doubled → `C7/Bb`).
24. **add9 and sus never simplify in slash stage** (Cadd9/E preserved — doc §2 works).
25. **Scale-vs-chord**: 5+ unique PCs and (span < 12 or clustered) → after chord detection, if span of ORIGINAL notes < 12 run scale detection and prefer any hit; span ≥ 12 keep chord. Clustered-but-wide sets (e.g. full scale spanning exactly 12) therefore stay chords — C major scale + top octave C (span 12) → `FΔ13#11`.
26. **7-PC no-tertian-bass scale fallback** (S5) with the `startswith(lowestName)` root filter (quirk: prefix collisions like "A" vs "Ab"/"A#" make the filter unreliable; with flats, lowest A accepts an Ab-rooted scale).
27. **>7 notes**: keep 7 most common PCs (octave-doubled PCs win; ties = set-iteration order), detect on the survivors, but scale check still sees the original set.
28. **Whole Tone needs ≥ 6 PCs**; pentatonic/blues/whole-tone need clustered-or-within-octave.
29. **Duplicate PCs across octaves** are collapsed for matching (interval sets), but raw notes still drive: span checks (S1f, S2h, §4.0, S15), bass-doubling counts (§2.7.8), second-lowest-note voicing checks (S2e/S2k, §2.7.3), highest-note bonus, and is_clustered.

---

## 10. Cross-reference: 01_Special_Cases_and_Resolutions.md vs code

| Doc § | Claim / test | Implemented? | Current behavior (verified) |
|---|---|---|---|
| 1. m6 vs M6 | mechanism (350/200 voicing bonus) | Partially — code has S2k (250/200/−100/−200) and §2.7.3, not the doc's 350 | mechanism present with different constants |
| 1 | "C Eb G Bb → Cm7 (not Am6/C)" | **BROKEN** | closed voicing → `Eb6` (§4.0); open voicing → `Cm7` |
| 1 | "C Bb D F G → Bb6/9/C" | **BROKEN + doc self-contradiction** (doc §3 says same notes → C9sus) | → `Gm11` (minor11_no9 perfect, 8000) |
| 1 | "G C D Bb → Gm7/C" | Partially | C-bass voicing → `Gm/C` (no F present, so "Gm7" in the doc is a typo); G-bass → `Gm` |
| 2. Slash / add9 preserve | never simplify add9 | ✅ | E G D C → `C(add9)/E` (doc writes "Cadd9/E"; rendered form differs) |
| 2 | "G C D Eb → Cm(add9)/G" | ✅ | `Cm(add9)/G` |
| 2 | "E G Bb D C → C9/E" | **BROKEN** | → `E7(#11)` (7#11_no3 +50 shell bonus from root E wins) |
| 3. 9sus vs add9 span rule | span<12 → add9/bass, ≥12 → 9sus | ✅ mechanism (6200/150 vs 6400/150) | D E G C → `C(add9)/D`; D E G C spread → `D9(sus)` |
| 3 | "C D F Bb (compact) → C9sus" | **BROKEN** (contradicts the doc's own decision rule) | → `Bb(add9)/C` |
| 3 | "C Bb D F G spread → C9sus" | **BROKEN** | with 5th present → `Gm11`; without the G (C Bb D F spread) → `C9(sus)` ✅ |
| 4. Scale vs chord | span rule + code | ✅ | C D E G A closed → `C Major Pentatonic`; spread → `C6/9` |
| 4 | "D E G A C → D Minor Pentatonic" | **Doc wrong** (those PCs aren't D minor pentatonic) | → `C Major Pentatonic` |
| 5. Altered dominants | +60 perfect bonus | ✅ constant exists | |
| 5 | "C E Bb Db → C7(b9)" | **BROKEN** | 4-note → `Bbdim/C`; add G (5 notes) → `C7b9` (note: no parens) |
| 5 | "C E Bb F# → C7(#11)" | ✅ | `C7(#11)` |
| 5 | "C E Bb Db F# → C7(b9,#11)" | ✅ | `C7(b9,#11)` |
| 5 | "C E Bb Db Ab → C7(b9,b13)" | ✅ | `C7(b9,b13)` |
| 5 | full-pattern alterations WITH 5th (7#9, 7#11, 7b13, 7#9#11 …) | **Mostly BROKEN** | C7#9 → `EΔ7(#11)`; C7#11+5 → `Gb7(b9,#11)`; C7b13+5 → `Bb7(#11)`; C7#9#11 → `Gb7(b9,#11)`. Only some no-5th shells behave (7b9b13_no5, 7#9b13_no5, 7b9#11_no5, 9b13…) |
| 6. Inversions | 35/40 bonuses | ✅ | E G C → `C/E`; G C E → `C/G`; Eb G C → `Eb6` (NOT `Cm/Eb` — S1e wins; doc's own §6 example list says Cm/Eb for Eb-G-C ordering, but the earlier test file expects Eb6; code gives Eb6); E G Bb C → `C7/E` ✅ |
| 7. Rootless voicings | +15 rootless bonus | ✅ constant exists | |
| 7 | "E Bb D F# → C7#11", "E Bb D → C9", "F# Bb D → C7#11" | **ALL BROKEN** | → `E7(#11)`, `E7(#11)`, `Gbaug` (root-position readings beat rootless C readings) |
| 8. C9b13 | pattern + 250 bonus + naming | ✅ | C Bb D E Ab → `C9(b13)`; no-5th → `C9(b13)`; F Eb G A Db → `F9(b13)` |
| 9. minor add9 slash | +50 perfect bonus | ✅ | G C D Eb → `Cm(add9)/G`; C Eb G D → `Cm(add9)`; Eb G D C → `Cm(add9)/Eb` (doc says "other inversions preserved" — slash IS shown) |

Doc's "Detection Accuracy: Triads 100% / 7ths 100% / Slash 100%" is aspirational: the module's own 30-case test suite passes 18/30.

02_Code_and_Logic_Summary.md is accurate on architecture and on the scoring skeleton but abridges constants (e.g. quotes patterns `'9'`, `'13'` that don't exist under those names — real keys are `dominant9`, `dominant13`) and omits every special_pattern_bonus above 250 except examples. Trust the source, not that doc, for numbers.

---

## 11. Known bugs / discrepancies (code ≠ docs/tests) — replicate all for behavior-identical port

- **B1** 2-note output includes root name (`"C (M3)"`); built-in tests expect bare `"M3"`.
- **B2** Half-diminished renders `m7b5`, never `ø7` (tests expect `Bø7`). The ø7 glyph exists only in the quality-parse map and comments.
- **B3** Closed-voicing complete m7 → relative major 6 (`Cm7` closed → `Eb6`), contradicting doc §1. Open voicing unaffected.
- **B4** `root_pc` mutation after the m7→6 reinterpretation corrupts later patterns in the same root call (C F G Bb → `Bbm11`, root misnamed; should be G-rooted).
- **B5** 7-PC tertian sets are renamed from other roots: C major13 set → `FΔ13#11`; C minor13 set → `EbΔ13#11`; C dominant11 set stays `C11` but Cmaj11 set → `FΔ9(#11)`.
- **B6** Test expects `CΔ7#11`/`CΔ13` naming without parens; code renders `CΔ7(#11)`, `CΔ9(#11)`.
- **B7** `C7(#9)` unreachable for the full pattern: {C,E,G,Bb,Eb} → `EΔ7(#11)`.
- **B8** 4-note C E Bb Db → `Bbdim/C` (doc expects C7(b9)).
- **B9** Early 7b9 path formats without parens: `C7b9`, `F7b9`.
- **B10** Comma formatting `G7(#9,b13)` vs test's `G7(#9b13)`.
- **B11/B12** Doc §1 Bb6/9/C and Gm7/C examples wrong (→ `Gm11`, `Gm/C`).
- **B13** Compact root-position 9sus loses to b7-add9 slash reading: C D F Bb → `Bb(add9)/C`.
- **B14** Doc §7 rootless C-chord claims all fail (root-position E/Gb readings win).
- **B15** Doc §2 "E G Bb D C → C9/E" → `E7(#11)`.
- **B16** Doc §4 "D E G A C → D Minor Pentatonic" → `C Major Pentatonic` (doc musically wrong).
- **B17** Test `C6/9` for C G A D → `Am11/C` (m11 8000 bonus).
- **B18** `13sus` compact loses to 6/9-no3 reading: C D F A Bb → `C6/9`; C D F A → `C6/9`. Spread ≥ 12 works: → `C13(sus)`.
- **B19** C Eb A (m6-no5) → `Cdim` (dim triad from A renamed to bass C, which isn't even in an actual C dim triad).
- **B20** 5-note whole-tone subset → `D7(#11)` (Whole Tone requires 6 PCs — by design, but surprising).
- **B21** S5 `startswith` root filter has prefix collisions ("A" accepts "Ab …"/"A# …" scale names).
- **B22** `augmented7` pattern is effectively shadowed by `7b13_no5` (same PCs, earlier + S1 bonus): C E G# Bb → `C7(b13)`. Intentional per comments, but means `Raug7`… never appears; is_seventh/`augmented7` handling in §2.6 is near-dead.
- **B23** `7sus13` duplicates `13sus`'s pattern `[0,2,5,9,10]`; `13sus` is earlier in dict order and always wins ties, so `R7sus13` is unreachable from equal matches.
- **B24** `half_diminished7` 700-bonus and S1f2 are dead from `detect_chord` (E3 short-circuit) — live only via `_detect_chord_simple`.
- **B25** `C7sus4` (C F G Bb) → `Bbm11` (B4 manifestation); `C7sus2` (C D G Bb) → `Gm/C`. The plain 7sus patterns rarely win.
- **B26** >7-note PC-reduction tie-break is CPython-set-iteration-order dependent — semantically unspecified.
- **B27** `PREFER_FLATS` module global and `INVERSION_NAMES` are dead; `detect_chord(…, lowest_note=…)` param ignored.

---

## 12. Edge-case behavior summary

| input | result |
|---|---|
| ∅ or 1 note | `None` |
| 2 notes (any, incl. octave/unison PCs) | interval string `"{lower} ({label})"`; > 21 semis → `"{n} semitones"` |
| ≥3 notes, all one PC (octaves) | `None` (unique PCs < 2) |
| duplicate PCs across octaves | collapsed for matching; still affect span/bass-doubling/second-note/highest-note/cluster logic (§9.29) |
| 3 notes | chords possible incl. `R5` power chord (needs 3 sounding notes) |
| >7 sounding notes | PC-frequency reduction to 7 PCs (B26), then normal flow; original set still used for the S15 scale check (8-PC diminished scales ARE detectable this way) |
| 5+ PCs within an octave | scale detection can override the chord answer (perfect subset matches only, in practice) |
| 7 PCs, bass lacks 3rd or 7th | early scale fallback S5 (root must "startswith"-match bass name) |
| no pattern scores > 10 | `None` (rare; needs ≥2 matched intervals everywhere to fail) |

---

## 13. Test vectors (current-code ground truth; ✱ = doc/test disagrees → "known bug")

All verified by executing the Python (prefer_flats=True). MIDI sets → exact expected output string for the port.

| # | MIDI notes | output | notes |
|---|---|---|---|
| 1 | {} | None | |
| 2 | {60} | None | |
| 3 | {60,64} | `C (M3)` | ✱ B1: test wants `M3` |
| 4 | {60,67} | `C (P5)` | ✱ B1 |
| 5 | {60,70} | `C (m7)` | |
| 6 | {60,61} | `C (m2)` | |
| 7 | {60,72} | `C (P8)` | |
| 8 | {60,81} | `C (M13)` | |
| 9 | {60,82} | `C (22 semitones)` | |
| 10 | {60,72,84} | None | one PC |
| 11 | {60,64,67} | `C` | |
| 12 | {60,63,67} | `Cm` | |
| 13 | {60,63,66} | `Cdim` | |
| 14 | {60,64,68} | `Caug` | |
| 15 | {64,68,72} | `Eaug` | aug re-root to bass |
| 16 | {60,62,67} | `C2` | sus2 |
| 17 | {60,65,67} | `C4` | sus4 |
| 18 | {60,67,72} | `C5` | power chord |
| 19 | {64,67,72} | `C/E` | |
| 20 | {55,60,64} | `C/G` | |
| 21 | {55,60,63} | `Cm/G` | |
| 22 | {63,67,72} | `Eb6` | not Cm/Eb |
| 23 | {60,64,69} | `C6` | |
| 24 | {60,64,67,69} | `C6` | |
| 25 | {60,64,69,74} | `C6/9` | |
| 26 | {60,64,67,69,74} | `C6/9` | |
| 27 | {60,67,69,74} | `Am11/C` | ✱ B17: test wants `C6/9` |
| 28 | {60,64,67,69,71,74} | `Cmaj7(6/9)` | |
| 29 | {60,63,67,69} | `Cm6` | |
| 30 | {60,63,69} | `Cdim` | ✱ B19: should be `Cm6` |
| 31 | {60,62,63,69} | `Cm6/9` | |
| 32 | {58,60,61,67} | `Bbm6/9` | Bb C Db G |
| 33 | {67,71,74,77} | `G7` | |
| 34 | {62,66,69,73} | `DΔ7` | |
| 35 | {60,63,67,70} | `Eb6` | ✱ B3: doc §1 says `Cm7` (closed voicing) |
| 36 | {72,75,79,82} | `Eb6` | same, other octave |
| 37 | {60,63,67,82} | `Cm7` | open voicing |
| 38 | {48,63,67,70} | `Cm7` | open voicing |
| 39 | {57,60,64,67} | `C6` | closed Am7 = C6 (intended) |
| 40 | {60,63,67,71} | `CmΔ7` | |
| 41 | {60,63,67,71,74} | `CmΔ7(9)` | |
| 42 | {60,64,68,71} | `CΔ7#5` | |
| 43 | {71,74,77,81} | `Bm7b5` | ✱ B2: test wants `Bø7` |
| 44 | {55,58,61,65} | `Gm7b5` | root in bass |
| 45 | {58,61,65,67} | `Bbm6` | half-dim inversion → m6 |
| 46 | {53,62,68,72} | `Fm6` | Dm7b5 with F bass |
| 47 | {68,71,74,78} | `Abm7b5` | Ab Cb D Gb |
| 48 | {60,63,66,69} | `Cdim7` | |
| 49 | {63,66,69,72} | `Ebdim7` | dim7 re-root to bass |
| 50 | {60,63,66,71} | `CdimΔ7` | |
| 51 | {60,64,68,70} | `C7(b13)` | ✱ B22: never `Caug7` |
| 52 | {60,64,70,80} | `C7(b13)` | 7b13_no5 |
| 53 | {60,64,67,70} | `C7` | |
| 54 | {48,60,64,67,70} | `C7` | doubled root |
| 55 | {64,67,70,72} | `C7/E` | doc §6 ✓ |
| 56 | {58,64,67,72} | `C/Bb` | bass not doubled → triad |
| 57 | {46,58,64,67,72} | `C7/Bb` | bass doubled → keep 7th |
| 58 | {60,64,67,70,74} | `C9` | |
| 59 | {64,70,74} | `E7(#11)` | ✱ B14: doc §7 wants `C9` |
| 60 | {64,70,74,78} | `E7(#11)` | ✱ B14: doc §7 wants `C7#11` |
| 61 | {66,70,74} | `Gbaug` | ✱ B14: doc §7 wants `C7#11` |
| 62 | {60,63,67,70,74} | `Cm9` | |
| 63 | {60,64,67,71,74} | `CΔ9` | |
| 64 | {60,64,70,81} | `C13` | 13_shell |
| 65 | {60,64,70,74,81} | `C13` | 13_no5_no11 |
| 66 | {60,64,67,70,74,81} | `C13` | |
| 67 | {60,64,67,71,74,77,81} | `FΔ13#11` | ✱ B5: test wants `CΔ13` |
| 68 | {60,63,67,70,74,77,81} | `EbΔ13#11` | ✱ B5: test wants `Cm13` |
| 69 | {60,64,67,70,74,77} | `C11` | |
| 70 | {60,64,67,71,74,77} | `FΔ9(#11)` | ✱ B5: "CΔ11" set |
| 71 | {60,63,67,70,74,77} | `Cm11` | |
| 72 | {60,63,70,74,77} | `Cm11` | quintal no-5th |
| 73 | {66,69,71,76} | `Gbm11` | m11 shell F# A B E |
| 74 | {60,64,67,71,74,78} | `CΔ9(#11)` | ✱ B6: test writes `CΔ7#11` |
| 75 | {60,71,78} | `CΔ7(#11)` | shell; ✱ B6 parens |
| 76 | {65,69,71,76} | `FΔ7(#11)` | FABE |
| 77 | {60,63,64,67,70} | `EΔ7(#11)` | ✱ B7: test wants `C7(#9)` |
| 78 | {60,64,70,75} | `EΔ7(#11)` | 7#9-no5 also lost |
| 79 | {60,61,64,70} | `Bbdim/C` | ✱ B8: doc §5 wants `C7(b9)` |
| 80 | {60,61,64,67,70} | `C7b9` | ✱ B9: no parens (early E2) |
| 81 | {53,57,60,63,66} | `F7b9` | F+Adim7, early E2 |
| 82 | {60,62,65,68,71} | `Ddim7/C` | early E2 |
| 83 | {60,64,70,78} | `C7(#11)` | doc §5 ✓ |
| 84 | {60,64,67,70,78} | `Gb7(b9,#11)` | ✱ 7#11 WITH 5th broken |
| 85 | {60,64,67,70,80} | `Bb7(#11)` | ✱ 7b13 WITH 5th broken |
| 86 | {60,64,67,70,75,78} | `Gb7(b9,#11)` | ✱ 7#9#11 broken |
| 87 | {67,70,71,75,77} | `G7(#9,b13)` | ✱ B10: test writes `G7(#9b13)` |
| 88 | {55,65,68,71,75} | `G7(b9,b13)` | |
| 89 | {60,61,64,66,70} | `C7(b9,#11)` | 7b9#11_no5, S1d |
| 90 | {60,64,70,73,78} | `C7(b9,#11)` | doc §5 ✓ |
| 91 | {60,64,70,73,80} | `C7(b9,b13)` | doc §5 ✓ |
| 92 | {60,70,74,76,80} | `C9(b13)` | doc §8 ✓ |
| 93 | {60,64,70,74,80} | `C9(b13)` | doc §8 ✓ |
| 94 | {53,63,67,69,73} | `F9(b13)` | doc §8 ✓ |
| 95 | {62,63,66,68,71,72} | `D7(b9,#11)` | 7b9#11_13_no5 set |
| 96 | {60,70,74,78,81} | `C13(#11)` | 13#11_no3_no5 |
| 97 | {60,64,67,70,74,78,81} | `C13(#11)` | 7-PC 13#11 |
| 98 | {60,61,63,64,66,67,68,70} | `C Altered` | 8 PCs → scale via S15 |
| 99 | {60,65,67,70} | `Bbm11` | ✱ B4/B25: "C7sus4" |
| 100 | {60,62,67,70} | `Gm/C` | ✱ B25: "C7sus2" |
| 101 | {60,62,65,70} | `Bb(add9)/C` | ✱ B13: doc §3 wants `C9sus` |
| 102 | {60,62,65,82} | `C9(sus)` | 9sus spread, no 5th |
| 103 | {60,70,74,77} | `C9(sus)` | doc §3 spread ✓ |
| 104 | {60,70,74,77,79} | `Gm11` | ✱ B11: doc wants Bb6/9/C or C9sus |
| 105 | {60,62,65,69} | `C6/9` | ✱ B18: sus13 set |
| 106 | {60,62,65,69,70} | `C6/9` | ✱ B18: 13sus compact |
| 107 | {60,62,65,69,82} | `C13(sus)` | 13sus spread ✓ |
| 108 | {60,62,64,67} | `C(add9)` | |
| 109 | {64,67,72,74} | `C(add9)/E` | doc §2 ✓ (rendered w/ parens) |
| 110 | {62,64,67,72} | `C(add9)/D` | doc §3 ✓ |
| 111 | {62,64,67,84} | `D9(sus)` | span ≥ 12 → 9sus ✓ |
| 112 | {60,62,63,67} | `Cm(add9)` | |
| 113 | {55,60,62,63} | `Cm(add9)/G` | doc §9 ✓ |
| 114 | {63,67,72,74} | `Cm(add9)/Eb` | doc §9 "inversions preserved" |
| 115 | {60,70,73,79} | `Bbm6/C` | early E1 |
| 116 | {60,70,73,77,79} | `Bbm6/C` | early E1b |
| 117 | {60,70,74,79} | `Bb6/C` | S2k voicing (Bb 2nd) |
| 118 | {60,67,70,74} | `Gm/C` | S2k voicing (G 2nd) |
| 119 | {60,62,65,67,70} | `G Minor Pentatonic` | closed C-D-F-G-Bb |
| 120 | {64,69,70,74} | `Em7b5(11)` | S2e ✓ |
| 121 | {60,65,66,70} | `Cm7b5(11)` | half_diminished11_no3 |
| 122 | {55,58,63,65} | `Eb(add9)/G` | |
| 123 | {55,58,63,65,67} | `Eb(add9)/G` | |
| 124 | {53,63,67,70} | `F9(sus)` | Eb/F |
| 125 | {55,71,77,80} | `Fdim/G` | via slash re-simplification |
| 126 | {65,67,69,70,72,74,76} | `F Ionian` | |
| 127 | {60,62,63,65,67,68,70} | `C Aeolian` | |
| 128 | {62,64,65,67,69,71,72} | `D Dorian` | |
| 129 | {60,62,64,65,67,69,71} | `C Ionian` | |
| 130 | {60,62,64,66,67,69,71} | `C Lydian` | |
| 131 | {60,62,64,67,69} | `C Major Pentatonic` | doc §4 ✓ |
| 132 | {62,64,67,69,72} | `C Major Pentatonic` | ✱ B16: doc says D Minor Pent |
| 133 | {60,62,76,79,81} | `C6/9` | spread → chord, doc §4 ✓ |
| 134 | {60,63,65,67,70} | `C Minor Pentatonic` | |
| 135 | {60,62,64,66,68,70} | `C Whole Tone` | |
| 136 | {60,62,64,66,68} | `D7(#11)` | ✱ B20: 5-note whole-tone |
| 137 | {60,63,65,66,67,70} | `C Minor Blues` | |
| 138 | {60,62,63,64,67,69} | `C Major Blues` | |
| 139 | {60,61,63,64,66,67,69,70} | `C Half-Whole Diminished` | 8 notes |
| 140 | {60,62,64,65,67,69,71,72} | `FΔ13#11` | span==12 kills scale check |
| 141 | {60,61,62,63,64} | `DbmΔ7/C` | chromatic cluster |
| 142 | {60,61,62,63,64,65,66,67,68} | `G7(b9,#11)/C` | 9 notes → reduction |
| 143 | {60,64,67,72,76,79} | `C` | doubled octaves |
| 144 | {52,55,60,64} | `C/E` | doubled 3rd |
| 145 | {60,64,65,67} | `Cadd11` | |
| 146 | {60,64,65,67,69} | `C6add4` | |

(Additional interval sanity rows 3–9 count toward the 60+ requirement; total = 146 vectors.)

---

## 14. Port guidance

- Score with f64 exactly as specified; comparisons are strict `>`. Iteration order of `CHORD_PATTERNS` and `SCALE_PATTERNS` must match the Python dict-definition order (use an ordered structure / Vec of tuples).
- Reproduce the `special_pattern_bonus` overwrite semantics (single mutable slot, rules applied in source order) — do NOT sum them.
- Reproduce §4.0's root mutation exactly (including its bleed into later patterns of the same call) or gate the port behind golden tests from §13.
- Decide up front whether B26 (>7-note tie-break) needs a deterministic rule; any fixed rule will diverge from CPython on some inputs.
- The §13 table is the acceptance suite: all 146 rows must match byte-for-byte (including Unicode `Δ`, parens, commas, and the no-paren `7b9` anomaly).
