# Chord Detector Data Tables — Lossless Extraction Spec

Source: `/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory/chord_detector.py` (2153 lines, all read).
Purpose: transcribe to Rust WITHOUT consulting the Python.

**Counts (verify after transcription):**

| Table | Entries |
|---|---|
| `CHORD_PATTERNS` | **95** |
| `ESSENTIAL_INTERVALS` | **95** (covers every pattern key) |
| `OPTIONAL_INTERVALS` | **80** explicit + **15** keys absent (absent ⇒ default `[]`) |
| `INTERVAL_NAMES` | **22** (semitones 0–21) |
| `SCALE_PATTERNS` | **28** |
| `INVERSION_NAMES` | **4** (defined but **never used** in this file) |
| `quality_map` (in `_match_chord_type`) | **38** + one special rule for `'13'` |

> **ORDER IS SEMANTIC.** `CHORD_PATTERNS` is a Python 3.7+ insertion-ordered dict and the matcher accepts a new best only on `score > best_score` (strict). Ties go to the EARLIER pattern. The Rust port MUST iterate patterns in exactly the order listed below (use an ordered structure, not a HashMap).

---

## 1. Note naming tables

```
NOTE_NAMES       (sharps) = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']
NOTE_NAMES_FLAT  (flats)  = ['C', 'Db', 'D', 'Eb', 'E', 'F', 'Gb', 'G', 'Ab', 'A', 'Bb', 'B']
PREFER_FLATS = True   # module-level default
```

**Root-name selection rule:** there is NO per-key/enharmonic context logic. A single boolean `prefer_flats` (constructor default `True`, settable via `set_note_preference`) selects the whole table: flats table if `True`, sharps table if `False`. `get_note_name(pitch_class)` indexes the chosen table. All roots, bass notes, and scale tonics use this same function.

---

## 2. CHORD_PATTERNS + ESSENTIAL + OPTIONAL + display names (95 rows, IN ORDER)

Columns:
- **Intervals**: semitones from root, VERBATIM in source order (order within the list is irrelevant to matching — sets are used — but preserved here for fidelity).
- **Essential**: `ESSENTIAL_INTERVALS[key]` verbatim.
- **Optional**: `OPTIONAL_INTERVALS[key]` verbatim; `— (default [])` marks the 15 keys absent from that dict (lookup uses `.get(key, [])`).
- **Display**: chord-name format, `R` = root note name. Slash bass is appended later as `/BASS`.

| # | Key | Intervals | Essential | Optional | Display |
|---|---|---|---|---|---|
| 1 | `major` | [0, 4, 7] | [4] | [0, 7] | `R` |
| 2 | `minor` | [0, 3, 7] | [3] | [0, 7] | `Rm` |
| 3 | `diminished` | [0, 3, 6] | [3, 6] | [0] | `Rdim` |
| 4 | `augmented` | [0, 4, 8] | [4, 8] | [0] | `Raug` |
| 5 | `sus2` | [0, 2, 7] | [2] | [0, 7] | `R2` |
| 6 | `sus4` | [0, 5, 7] | [5] | [0, 7] | `R4` |
| 7 | `7sus4` | [0, 5, 7, 10] | [5, 10] | [0, 7] | `R7sus4` |
| 8 | `7sus2` | [0, 2, 7, 10] | [2, 10] | [0, 7] | `R7sus2` |
| 9 | `9sus` | [0, 2, 5, 10] | [2, 5, 10] | — (default []) | `R9(sus)` |
| 10 | `9sus_with5` | [0, 2, 5, 7, 10] | [2, 5, 10] | — (default []) | `R9(sus)` |
| 11 | `13sus` | [0, 2, 5, 9, 10] | [2, 10] | — (default []) | `R13(sus)` |
| 12 | `13sus_with5` | [0, 2, 5, 7, 9, 10] | [2, 10] | — (default []) | `R13(sus)` |
| 13 | `7sus13` | [0, 2, 5, 9, 10] | [2, 10] | [0, 7, 5] | `R7sus13` |
| 14 | `sus13` | [0, 2, 5, 9] | [2, 9] | [0, 7, 5] | `Rsus13` |
| 15 | `half_diminished7` | [0, 3, 6, 10] | [3, 10] | [0] | `Rm7b5` |
| 16 | `half_diminished11` | [0, 3, 6, 10, 5] | [6, 10] | — (default []) | `Rm7b5(11)` |
| 17 | `half_diminished11_no3` | [0, 5, 6, 10] | [6, 10] | — (default []) | `Rm7b5(11)` |
| 18 | `major7` | [0, 4, 7, 11] | [4, 11] | [0, 7] | `RΔ7` |
| 19 | `major7#5` | [0, 4, 8, 11] | [4, 11] | [0] | `RΔ7#5` |
| 20 | `minor7` | [0, 3, 7, 10] | [3, 10] | [0, 7] | `Rm7` |
| 21 | `dominant7` | [0, 4, 7, 10] | [4, 10] | [0, 7] | `R7` |
| 22 | `diminished7` | [0, 3, 6, 9] | [3, 9] | [0] | `Rdim7` |
| 23 | `diminished_major7` | [0, 3, 6, 11] | [3, 6, 11] | [0] | `RdimΔ7` |
| 24 | `7b13_no5` | [0, 4, 10, 8] | [4, 10] | — (default []) | `R7(b13)` |
| 25 | `augmented7` | [0, 4, 8, 10] | [4, 10] | [0] | `Raug7`* |
| 26 | `minor_major7` | [0, 3, 7, 11] | [3, 11] | [0, 7] | `RmΔ7` |
| 27 | `minor_major9` | [0, 2, 3, 7, 11] | [3, 11] | [0, 7, 2] | `RmΔ7(9)` |
| 28 | `major9` | [0, 4, 7, 11, 2] | [4, 11] | [0, 7] | `RΔ9` |
| 29 | `minor9` | [0, 3, 7, 10, 2] | [3, 10] | [0, 7] | `Rm9` |
| 30 | `dominant9` | [0, 4, 7, 10, 2] | [4, 10] | [0, 7] | `R9` |
| 31 | `major11` | [0, 4, 7, 11, 2, 5] | [4, 11] | [0, 7] | `RΔ11` |
| 32 | `major9#11` | [0, 4, 7, 11, 2, 6] | [4, 11, 6] | [0, 7] | `RΔ9(#11)` |
| 33 | `major7#11` | [0, 4, 7, 11, 6] | [4, 11, 6] | [0, 7, 2] | `RΔ7(#11)` |
| 34 | `major7#11_no5` | [0, 4, 6, 11] | [4, 11, 6] | [0, 2] | `RΔ7(#11)` |
| 35 | `major7#11_shell` | [0, 6, 11] | [6, 11] | [0, 4, 7, 2] | `RΔ7(#11)` |
| 36 | `minor11` | [0, 3, 7, 10, 2, 5] | [3, 10] | [0, 7] | `Rm11` |
| 37 | `minor11_no5` | [0, 3, 10, 2, 5] | [3, 10] | [0] | `Rm11` |
| 38 | `minor11_no9` | [0, 3, 5, 7, 10] | [3, 10] | — (default []) | `Rm11` |
| 39 | `minor11_shell` | [0, 3, 5, 10] | [3, 10] | [0, 2] | `Rm11` |
| 40 | `major13` | [0, 4, 7, 11, 2, 5, 9] | [4, 11] | [0, 7, 5] | `RΔ13` |
| 41 | `major13#11` | [0, 4, 7, 11, 2, 6, 9] | [4, 11] | [0, 7] | `RΔ13#11` |
| 42 | `minor13` | [0, 3, 7, 10, 2, 5, 9] | [3, 10] | [0, 7] | `Rm13` |
| 43 | `dominant11` | [0, 4, 7, 10, 2, 5] | [4, 10] | [0, 7] | `R11` |
| 44 | `dominant13` | [0, 4, 7, 10, 2, 5, 9] | [4, 10] | [0, 7, 5] | `R13` |
| 45 | `13_shell` | [0, 4, 10, 9] | [4, 10] | [0, 7] | `R13` |
| 46 | `13_no5_no11` | [0, 4, 10, 2, 9] | [4, 10] | [0, 7] | `R13` |
| 47 | `13_no5` | [0, 4, 10, 2, 5, 9] | [4, 10] | [0, 7] | `R13` |
| 48 | `7#11_no5` | [0, 4, 10, 6] | [4, 10] | — (default []) | `R7(#11)` |
| 49 | `7#11_no3_no5` | [0, 10, 2, 6] | [10, 6] | — (default []) | `R7(#11)` |
| 50 | `13#11_no3_no5` | [0, 10, 2, 6, 9] | [10, 6] | — (default []) | `R13(#11)` |
| 51 | `13#11_no9_no5` | [0, 4, 6, 9, 10] | [4, 10] | — (default []) | `R13(#11)` |
| 52 | `13#11_no5` | [0, 4, 10, 2, 6, 9] | [4, 10] | — (default []) | `R13(#11)` |
| 53 | `7b9` | [0, 4, 7, 10, 1] | [4, 10] | [0, 7] | `R7(b9)` |
| 54 | `7#9` | [0, 4, 7, 10, 3] | [4, 10] | [0, 7] | `R7(#9)` |
| 55 | `7#11` | [0, 4, 7, 10, 6] | [4, 10] | [0, 7] | `R7(#11)` |
| 56 | `7b13` | [0, 4, 7, 10, 8] | [4, 10] | [0, 7] | `R7(b13)` |
| 57 | `7b9#11` | [0, 4, 7, 10, 1, 6] | [4, 6, 10] | [0, 7] | `R7(b9,#11)` |
| 58 | `7#9#11` | [0, 4, 7, 10, 3, 6] | [4, 3, 6, 10] | [0, 7] | `R7(#9,#11)` |
| 59 | `7b9b13` | [0, 4, 7, 10, 1, 8] | [4, 10] | [0, 7] | `R7(b9,b13)` |
| 60 | `7#9b13` | [0, 4, 7, 10, 3, 8] | [4, 10] | [0, 7] | `R7(#9,b13)` |
| 61 | `7#11b13` | [0, 4, 7, 10, 6, 8] | [4, 10] | [0, 7] | `R7(#11,b13)` |
| 62 | `7b9#11b13` | [0, 4, 7, 10, 1, 6, 8] | [4, 10] | [0, 7] | `R7(b9,#11,b13)` |
| 63 | `7#9#11b13` | [0, 4, 7, 10, 3, 6, 8] | [4, 10] | [0, 7] | `R7(#9,#11,b13)` |
| 64 | `7b9#9` | [0, 4, 7, 10, 1, 3] | [4, 10] | [0, 7] | `R7(b9,#9)` |
| 65 | `7b9#9#11` | [0, 4, 7, 10, 1, 3, 6] | [4, 10] | [0, 7] | `R7(b9,#9,#11)` |
| 66 | `7b9#9b13` | [0, 4, 7, 10, 1, 3, 8] | [4, 10] | [0, 7] | `R7(b9,#9,b13)` |
| 67 | `9b13` | [0, 4, 7, 10, 2, 8] | [4, 10] | [0, 7] | `R9(b13)` |
| 68 | `9b13_no5` | [0, 4, 10, 2, 8] | [4, 10] | [0] | `R9(b13)` |
| 69 | `7#11_shell` | [0, 10, 2, 6, 9] | [10, 6] | [0, 2, 9] | `R7(#11)` |
| 70 | `7#11_no3` | [0, 7, 10, 6] | [10, 6] | [0, 2, 9] | `R7(#11)` |
| 71 | `7#9#11_shell` | [0, 10, 3, 6, 9] | [10, 3, 6] | [0, 6, 9] | `R7(#9,#11)` |
| 72 | `7b9#11_shell` | [0, 10, 1, 6, 9] | [10, 1, 6] | [0, 6, 9] | `R7(b9,#11)` |
| 73 | `7b9#11_no3` | [0, 7, 10, 1, 6] | [10, 1, 6] | [0, 6, 9] | `R7(b9,#11)` |
| 74 | `7b9#11_no5` | [0, 4, 10, 1, 6] | [4, 10, 1, 6] | [0] | `R7(b9,#11)` |
| 75 | `7b9#11_13_no5` | [0, 1, 4, 6, 9, 10] | [4, 10, 1, 6] | — (default []) | `R7(b9,#11)` |
| 76 | `7b9b13_no5` | [0, 4, 10, 1, 8] | [4, 10] | [0] | `R7(b9,b13)` |
| 77 | `7#9b13_no5` | [0, 4, 10, 3, 8] | [4, 10] | [0] | `R7(#9,b13)` |
| 78 | `7b9#9_no5` | [0, 4, 10, 1, 3] | [4, 10] | [0] | `R7(b9,#9)` |
| 79 | `altered` | [0, 4, 7, 10, 1, 3, 6, 8] | [4, 10] | [0, 7] | `R7alt` |
| 80 | `add9` | [0, 4, 7, 2] | [4] | [0, 7] | `R(add9)` |
| 81 | `minor_add9` | [0, 3, 7, 2] | [3] | [0, 7] | `Rm(add9)` |
| 82 | `6` | [0, 4, 7, 9] | [4, 9] | [0, 7] | `R6` |
| 83 | `6_no5` | [0, 4, 9] | [4, 9] | [0] | `R6` |
| 84 | `6add4` | [0, 4, 5, 7, 9] | [4, 9] | [0, 7] | `R6add4` |
| 85 | `6add4_no5` | [0, 4, 5, 9] | [4, 9] | [0] | `R6add4` |
| 86 | `6_9` | [0, 4, 7, 9, 2] | [4, 9] | [0, 7] | `R6/9` |
| 87 | `6_9_no5` | [0, 4, 9, 2] | [4, 9] | [0] | `R6/9` |
| 88 | `6_9_no3` | [0, 2, 7, 9] | [9, 2] | [0, 7] | `R6/9` |
| 89 | `major7_6_9` | [0, 4, 7, 9, 11, 2] | [4, 11, 9] | — (default []) | `Rmaj7(6/9)` |
| 90 | `minor6` | [0, 3, 7, 9] | [3, 9] | [0, 7] | `Rm6` |
| 91 | `minor6_no5` | [0, 3, 9] | [3, 9] | [0] | `Rm6` |
| 92 | `minor6_9` | [0, 3, 7, 9, 2] | [3, 9] | [0, 7] | `Rm6/9` |
| 93 | `minor6_9_no5` | [0, 2, 3, 9] | [3, 9] | [0] | `Rm6/9` |
| 94 | `add11` | [0, 4, 7, 5] | [4] | [0, 7] | `Radd11` |
| 95 | `5` | [0, 7] | [7] | [0] | `R5` |

\* `augmented7` has NO explicit branch in the name formatter; it falls through to the generic `f"{root_name}{chord_type}"` → literally `Raugmented7` (e.g., `Caugmented7`). This is the only pattern hitting the fallback. Preserve this behavior for losslessness (or flag it — see open questions).

The 15 keys absent from `OPTIONAL_INTERVALS` (thus optional = `[]`): `9sus`, `9sus_with5`, `13sus`, `13sus_with5`, `half_diminished11`, `half_diminished11_no3`, `7b13_no5`, `minor11_no9`, `7#11_no5`, `7#11_no3_no5`, `13#11_no3_no5`, `13#11_no9_no5`, `13#11_no5`, `7b9#11_13_no5`, `major7_6_9`.

Note duplicate interval sets (intentional; earlier key wins ties, later key can still win via bonuses): `13sus` ≡ `7sus13` ([0,2,5,9,10]); `13sus_with5` and `7sus2`+... ; `7#11_shell` ≡ `13#11_no3_no5` ([0,10,2,6,9]); `9sus_with5` ≡ `13sus_with5` differ; `7sus2` [0,2,7,10] vs `sus2`… (only the two exact dups listed are full-set duplicates).

---

## 3. INTERVAL_NAMES (2-note detection), 22 entries

| Semitones | Name | | Semitones | Name |
|---|---|---|---|---|
| 0 | `P1` | | 11 | `M7` |
| 1 | `m2` | | 12 | `P8` |
| 2 | `M2` | | 13 | `m9` |
| 3 | `m3` | | 14 | `M9` |
| 4 | `M3` | | 15 | `m10` |
| 5 | `P4` | | 16 | `M10` |
| 6 | `d5` | | 17 | `P11` |
| 7 | `P5` | | 18 | `A11` |
| 8 | `m6` | | 19 | `P12` |
| 9 | `M6` | | 20 | `m13` |
| 10 | `m7` | | 21 | `M13` |

Two-note format: `"{lower_note_name} ({interval_name})"`, e.g. `C (P5)`. Interval computed from actual MIDI numbers (`upper - lower`, NOT mod 12). If semitones > 21: fallback string `"{n} semitones"`.

---

## 4. SCALE_PATTERNS (28 entries)

| Scale name (display string, verbatim) | Intervals |
|---|---|
| `Ionian` | [0, 2, 4, 5, 7, 9, 11] |
| `Dorian` | [0, 2, 3, 5, 7, 9, 10] |
| `Phrygian` | [0, 1, 3, 5, 7, 8, 10] |
| `Lydian` | [0, 2, 4, 6, 7, 9, 11] |
| `Mixolydian` | [0, 2, 4, 5, 7, 9, 10] |
| `Aeolian` | [0, 2, 3, 5, 7, 8, 10] |
| `Locrian` | [0, 1, 3, 5, 6, 8, 10] |
| `Melodic Minor` | [0, 2, 3, 5, 7, 9, 11] |
| `Dorian b2` | [0, 1, 3, 5, 7, 9, 10] |
| `Lydian Augmented` | [0, 2, 4, 6, 8, 9, 11] |
| `Lydian Dominant` | [0, 2, 4, 6, 7, 9, 10] |
| `Mixolydian b6` | [0, 2, 4, 5, 7, 8, 10] |
| `Locrian #2` | [0, 2, 3, 5, 6, 8, 10] |
| `Altered` | [0, 1, 3, 4, 6, 8, 10] |
| `Harmonic Minor` | [0, 2, 3, 5, 7, 8, 11] |
| `Locrian #6` | [0, 1, 3, 5, 6, 9, 10] |
| `Ionian #5` | [0, 2, 4, 5, 8, 9, 11] |
| `Dorian #4` | [0, 2, 3, 6, 7, 9, 10] |
| `Phrygian Dominant` | [0, 1, 4, 5, 7, 8, 10] |
| `Lydian #2` | [0, 3, 4, 6, 7, 9, 11] |
| `Altered Diminished` | [0, 1, 3, 4, 6, 8, 9] |
| `Major Pentatonic` | [0, 2, 4, 7, 9] |
| `Minor Pentatonic` | [0, 3, 5, 7, 10] |
| `Major Blues` | [0, 2, 3, 4, 7, 9] |
| `Minor Blues` | [0, 3, 5, 6, 7, 10] |
| `Whole Tone` | [0, 2, 4, 6, 8, 10] |
| `Whole-Half Diminished` | [0, 2, 3, 5, 6, 8, 9, 11] |
| `Half-Whole Diminished` | [0, 1, 3, 4, 6, 7, 9, 10] |

Scale display format: `"{root_name} {scale_name}"` e.g. `C Ionian`.

Scale-category sets (used for bonuses in `detect_scale`):
```
major_modes          = {Ionian, Dorian, Phrygian, Lydian, Mixolydian, Aeolian, Locrian}
melodic_minor_modes  = {Melodic Minor, Dorian b2, Lydian Augmented, Lydian Dominant, Mixolydian b6, Locrian #2, Altered}
harmonic_minor_modes = {Harmonic Minor, Locrian #6, Ionian #5, Dorian #4, Phrygian Dominant, Lydian #2, Altered Diminished}
clustered_only_scales = {Major Pentatonic, Minor Pentatonic, Major Blues, Minor Blues, Whole Tone}
```

---

## 5. INVERSION_NAMES (defined, UNUSED anywhere in file)

```
0: ''        # Root position
1: '/3rd'    # First inversion
2: '/5th'    # Second inversion
3: '/7th'    # Third inversion
```

---

## 6. Detector constants & entry thresholds

| Constant | Value |
|---|---|
| `min_notes_for_chord` | 2 |
| `max_notes_for_chord` | 7 |
| exactly 2 notes | → interval detection (section 3), not chord |
| > 7 notes | keep only notes whose pitch class is among the 7 most common pitch classes (`Counter.most_common(7)`; Python Counter tie order = first-encountered in iteration of `[note % 12 for note in active_notes]` — set iteration order, effectively unspecified) |
| after PC conversion, < 2 unique PCs | → return None |

**`is_clustered(notes)`**: `False` if fewer than 5 notes. Sort MIDI notes; for each adjacent pair, gap `<= 2` semitones counts as "adjacent". Returns `adjacent_count / (len-1) >= 0.6`.

**Scale-check trigger (`should_check_scale_later`)**: ≥ 5 unique pitch classes AND (total MIDI span `< 12` OR `is_clustered`). Chord detection runs first; at the end, if triggered: if span of ORIGINAL notes `>= 12` return the chord; else run `detect_scale` on original notes and prefer scale result if any, else chord.

**7-unique-PC pre-check**: if exactly 7 unique PCs — compute intervals from the LOWEST note's PC; `has_third` = (3 or 4 present), `has_seventh` = (10 or 11 present). If NOT (third AND seventh): run `detect_scale`; if a scale is found AND its name string starts with the lowest note's name, return the scale immediately. Otherwise continue chord detection.

---

## 7. Early special-case returns (run BEFORE pattern scoring, in this order)

1. **m6 slash, 4 PCs**: intervals from bass == `[0, 1, 7, 10]` → return `"{name(bass+10)}m6/{name(bass)}"`.
2. **m6 slash, 5 PCs**: intervals from bass == `[0, 1, 5, 7, 10]` → return `"{name(bass+10)}m6/{name(bass)}"`.
3. **dim7 upper structure, 5 PCs**: remove bass PC; if remaining 4 PCs form `[0, 3, 6, 9]` from any of those 4 as root (checked in PC order, first hit wins):
   - If upper structure contains intervals {4, 7, 10, 1} from bass → return `"{name(bass)}7b9"` (NOTE: literal `7b9`, no parentheses — differs from pattern-table display `7(b9)`).
   - Else → return `"{name(dim7_root)}dim7/{name(bass)}"`.
4. **half-dim7 vs m6, 4 PCs**: for each PC as candidate root (in sorted-PC order), if intervals from it == `[0, 3, 6, 10]`:
   - candidate == bass PC → return `"{name}m7b5"`.
   - else m6_root = candidate + 3 (mod 12): if m6_root == bass → return `"{name(m6_root)}m6"`; else return `"{name(m6_root)}m6/{name(bass)}"`.

**Global dominant quality flag** (computed next, passed into scoring): true iff ANY pitch class p in the chord has both `(p+4)%12` and `(p+10)%12` also present.

---

## 8. Pattern-matching score model (`_match_chord_pattern`)

For each candidate root (every unique PC), intervals = sorted `(pc - root) % 12`. For each pattern (in table order):

**Hard skips (in order):**
1. `chord_type ∈ {7b9#11, 7#9#11, 7#9#11_shell, 7b9#11_shell, 7b9#11_no3}` AND any essential interval missing → skip.
2. essential list non-empty AND zero essential intervals matched → skip.
3. `matched_count < 2` → skip.

**Score components (sum):**

| # | Component | Value |
|---|---|---|
| 1 | `essential_score` | `(matched_essential / len(essential)) * 60.0`; if pattern has NO essential entry (never happens — all 95 have one, but code path exists): `30.0` |
| 2 | `percentage_match` | `(matched_count / unique_input_PC_count) * 40.0` |
| 3 | `highest_note_bonus` | `10.0` if `(highest_pc - root) % 12 ∈ pattern` else 0 |
| 4 | `completeness_bonus` | perfect match (missing=0 AND extra=0): `30.0`; overridden to `60.0` if chord_type ∈ {`7b13_no5`,`7b9b13_no5`,`7#9b13_no5`,`7b9#11_no5`,`7b9`,`7#9`,`7b13`,`7b9b13`,`7#9b13`,`7#11b13`,`7b9#11`,`7#9#11`}; `500.0` if `diminished_major7`; `700.0` if `half_diminished7`; `200.0` if `major7_6_9`. Non-perfect but missing=0 (extras present): `10.0`. Else 0. |
| 5 | `extra_penalty` | `extra_count * 3.0` (subtracted) |
| 6 | `missing_penalty` | `len(essential_missing) * 40.0 + len(optional_missing) * 1.0 + len(required_missing) * 8.0` (subtracted). `required_missing` = missing − optional − essential. |
| 7 | `rootless_bonus` | `15.0` if `0 ∈ missing` AND all essential matched AND `len(essential) >= 2` |
| 8 | `root_in_bass_bonus` | `15.0` if `root == lowest_pc` AND `0 ∈ matched` |
| 9 | `characteristic_bonus` | `10.0` if `6 ∈ matched` OR `8 ∈ matched`; PLUS `50.0` if chord_type ∈ {`7#11_shell`,`7#11_no3`,`7#9#11_shell`,`7b9#11_shell`,`7b9#11_no3`} |
| 10 | `dominant_quality_adjustment` | if (global flag OR (4 ∈ intervals AND 10 ∈ intervals)): `-500.0` if chord_type startswith `6` or startswith `minor6` or == `diminished7` or == `diminished`; `+600.0` if chord_type == `dominant7` AND perfect match; `+50.0` if chord_type startswith `13` or startswith `dominant` (incl. dominant7 non-perfect); else 0 |
| 11 | `special_pattern_bonus` | see section 9 (single variable, later assignments OVERWRITE earlier ones) |
| 12 | `inversion_bonus` | see below |

**Inversion bonus** (`bass_interval = (lowest_pc - root) % 12`):
- `is_triad` = chord_type ∈ {major, minor, diminished, augmented}; if bass_interval ∈ {3, 4, 7} → `35.0`.
- `is_seventh` = chord_type ∈ {major7, minor7, dominant7, diminished7, diminished_major7, half_diminished7, augmented7, minor_major7} OR (chord_type starts with `7` AND (contains `b9` or `#9` or `#11` or `b13` or == `altered` — note `== 'altered'` can never start with '7', dead clause)); elif bass_interval ∈ pattern AND ≠ 0 → `40.0`.
- `is_sixth_chord` = chord_type ∈ {`6`,`6_no5`,`minor6`,`minor6_no5`,`6_9`,`6_9_no5`,`6_9_no3`,`minor6_9`,`6add4`,`6add4_no5`}; if sixth chord AND bass_interval == 0: compute potential minor-triad root = `(lowest_pc - 3) % 12`; if `{0,3,7} ⊆` intervals from that root: sixth_pc = `(root + 9) % 12`; if `highest_pc == sixth_pc` AND `len(active_notes) >= 4` → inversion_bonus = `45.0`, else `-40.0` (overwrites any earlier inversion bonus).

**Final**: `score = 1+2+3+4+7+8+9+10+11+12 − 5 − 6`. Accept as new best iff `score > best_score` AND `matched_count >= 2` AND `score > 10.0`.

**Post-accept reinterpretation (inside the accept branch, BEFORE naming):** if chord_type == `minor7` AND MIDI span (max−min of active notes) `< 12`: chord becomes `6` with root = `(root + 3) % 12` (e.g., Am7 closed → C6). Applies regardless of whether the m7 was complete.

---

## 9. `special_pattern_bonus` cases — sequential assignments (LATER OVERWRITES EARLIER; transcribe in this exact order)

`intervals` = intervals from candidate root (sorted list). `ifl` = sorted intervals of unique PCs from the LOWEST note. `unique` = count of unique PCs. "perfect" = missing_count==0 AND extra_count==0.

| # | Condition | Bonus |
|---|---|---|
| 1 | `7b13_no5` AND intervals == [0,4,8,10] | 100.0 |
| 2 | `7b9b13_no5` AND intervals == [0,1,4,8,10] | 150.0 |
| 3 | `7#9b13_no5` AND intervals == [0,3,4,8,10] | 150.0 |
| 4 | `7b9#11_no5` AND intervals == [0,1,4,6,10] | 400.0 |
| 5 | ifl == [0,1,7,10] AND NOT global-dominant AND chord_type ∈ {minor6, minor6_no5, minor6_9_no5} AND root ≠ bass | 1500.0 |
| 6 | `diminished` AND unique ≥ 4 | −1000.0 |
| 7 | chord_type ∈ {6_no5, 6} AND root == bass AND intervals == [0,4,9] | 100.0 |
| 8 | `add9` AND perfect: (a) root ≠ bass AND ifl == [0,2,5,10]: if (3∈intervals or 4∈intervals) and 7∈intervals: if triad complete (0 AND (3 or 4) AND 7 all ∈ intervals): if bass_interval_from_root ∉ {0,3,4,7}: MIDI span < 12 → **6200.0**, span ≥ 12 → **150.0**; if bass IS triad tone → **4200.0**; triad incomplete → **150.0**; no 3rd+5th → **150.0**. (b) root ≠ bass but ifl ≠ [0,2,5,10] → **150.0**. (c) root == bass → **150.0** | see left |
| 9 | `minor_add9` AND perfect | 50.0 |
| 10 | chord_type ∈ {minor6, minor6_no5, minor6_9, minor6_9_no5} AND root ≠ bass AND NOT global-dominant AND 3∈intervals AND 9∈intervals AND unique == 4: intervals == [0,2,3,9] → **600.0**, else → **400.0** | see left |
| 11 | `half_diminished7` AND intervals == [0,3,6,10] AND perfect | 180.0 |
| 12 | chord_type ∈ {sus2, sus4} AND root == bass AND no essential missing AND missing ≤ 1 AND extra == 0 AND bonus still 0.0 | 80.0 |
| 13 | chord_type ∈ {major7#11, major7#11_no5, major9#11, major13#11} AND 6∈intervals: perfect → **250.0**; elif missing ≤ 1 → **150.0** | see left |
| 14 | `major7#11_no5` AND intervals == [0,4,6,11] | 300.0 |
| 15 | chord_type ∈ {6_9, 6_9_no5}: if 9∈intervals AND 2∈intervals AND root == bass: perfect → **9000.0**, elif missing ≤ 1 → **220.0**; elif 9∉intervals → **−300.0** | see left |
| 16 | `6_9_no3`: 9∈ AND 2∈ AND root == bass: perfect → **290.0**, elif missing ≤ 1 → **220.0** | see left |
| 17 | chord_type ∈ {minor6_9, minor6_9_no5} AND 9∈ AND 2∈ AND 3∈ AND root == bass AND perfect | 9500.0 |
| 18 | `major7_6_9`: perfect AND root == bass → **10000.0**; elif 9∉intervals → **−300.0** | see left |
| 19 | 3∈intervals AND 9∈intervals AND unique == 4: chord_type ∈ {minor6, minor6_no5, minor6_9, minor6_9_no5}: perfect → **450.0**, elif missing ≤ 1 AND extra ≤ 2 → **410.0**; OTHER chord types → **380.0** (unconditional overwrite!) | see left |
| 20 | chord_type ∈ {13_shell, 13_no5_no11, 13_no5} AND root == bass AND 4∈ AND 10∈ AND 9∈: perfect → **250.0**, elif missing ≤ 1 → **180.0** | see left |
| 21 | `half_diminished11_no3` AND intervals == [0,5,6,10] AND root == bass AND the 2nd-lowest MIDI note's interval from bass == 5 | 300.0 |
| 22 | chord_type ∈ {7#11_no5, 7#11_no3_no5, 13#11_no3_no5, 13#11_no9_no5, 13#11_no5} AND root == bass AND 10∈ AND 6∈: perfect → **250.0**, elif missing ≤ 1 → **180.0** | see left |
| 23 | chord_type ∈ {minor11, minor11_no5, minor11_no9, minor11_shell} AND perfect | 8000.0 |
| 24 | chord_type ∈ {9sus, 9sus_with5, 13sus, 13sus_with5} AND perfect AND root == bass: MIDI span ≥ 12 → **6400.0**, else → **150.0** | see left |
| 25 | `7b9#11_13_no5` AND perfect | 260.0 |
| 26 | chord_type ∈ {9b13, 9b13_no5} AND perfect AND root == bass | 250.0 |
| 27 | `dominant9` AND root == bass AND missing ≤ 1 AND extra == 0 | 200.0 |
| 28 | **Bb6/C voicing rule.** `is_bb6_voicing` = ifl ∈ {[0,2,5,7,10], [0,2,7,10]} AND 2nd-lowest MIDI note's interval from bass == 10. If is_bb6_voicing: chord `6` with `(root−bass)%12==10` → **250.0**; chord ∈ {6_9, 6_9_no5} with `(root−bass)%12==10` → **−100.0**; chord ∈ {minor7, minor} → **−200.0**. Else if ifl ∈ same two lists (other voicing): chord ∈ {minor7, minor} with `(root−bass)%12==7` → **200.0**; chord `6` with `(root−bass)%12==10` → **−200.0** | see left |
| 29 | intervals == [0,2,4,7,9] AND chord_type == `6` | 200.0 |

---

## 10. Post-scoring root/name adjustments (in `detect_chord`, after best match chosen)

1. **dim7-as-7(b9)** (4 or 5 PCs): for each PC `r` (sorted order): if `(r+4)%12` present, and the other PCs (all 4 if 4 PCs; the 4 ≠ r if 5 PCs) form `[0,3,6,9]` from `(r+4)%12`, and `(r+10)%12` is among them: re-run matcher with root `r`; if resulting name contains `7(b9)` or `7` → adopt it (name, root, score) and stop.
2. **Diminished root normalization** (skipped if best name contains `7(b9)`): if best is a `diminished` triad and root ≠ bass → rename to `{bass_name}dim`, root = bass. If best is `diminished7` and root ≠ bass → re-match from bass; adopt only if still diminished7.
3. **Augmented normalization**: if best is `augmented` or `augmented7` and root ≠ bass → re-match from bass; adopt only if still augmented/augmented7.

---

## 11. Slash-chord / simplification rules (root ≠ bass)

`bass_int = (lowest_pc − best_root_pc) % 12`.

**Skip slash entirely (return name without `/bass`) when:**
- `is_extended` = name contains `9`/`11`/`13` but NOT `add9`, AND bass_int ∈ {2, 5, 7, 9, 10}; OR
- `is_altered` = name contains `b9`/`#9`/`b13`/`#11`, AND bass_int ∈ {1, 3, 6, 8};
- EXCEPTION: `is_six_nine` (name contains `6/9` or `(6/9)`) AND bass_int == 2 → do NOT skip (show slash);
- ALWAYS skip slash for `diminished7`, `augmented`, `augmented7` matches (symmetrical).

**Otherwise attempt simplification before appending `/bass`:**
- Base essential-set for bass check: `{0, 3, 4, 6, 7, 8}`; add `{6, 11}` if match is `diminished_major7`; add `{3, 6, 10}` if `half_diminished7`; add `{10}` if "dominant-ish" (name ends with `7` or contains `7(` or ends with `13`, and contains none of `Δ7`, `dim7`, `ø7`, `m7`).
- Voicing special case: if intervals-from-bass ∈ {[0,2,5,7,10], [0,2,7,10]} → never simplify (`special_case_no_simplify`), regardless of 2nd-note voicing branch (both branches set it).
- If `is_extended_chord` (name contains `9`/`11`/`13`/`6/9`) AND bass_int ∈ best pattern → don't simplify.
- If bass_int ∈ essential-set: don't simplify, EXCEPT allow when name ends with `m`, or contains `add9`, or (len(name) ≤ 2 and not ending `7`/`6`) — i.e., basic triads/add9 may still simplify (to find sus chords).
- If bass_int ∉ essential-set → simplify.
- Never simplify if name ends `2`/`4` or contains `sus2`/`sus4`/`sus13` (sus chords), or contains `add9`.
- 7th-chord bass-doubling rule: if not special-cased, not sus, name contains `7` but not `Δ7`/`m7`/`dim7`: count MIDI notes with bass PC; count == 1 → simplify to triad; count ≥ 2 → keep 7th.
- Simplification = re-detect on notes minus all bass-PC notes (`_detect_chord_simple`, same matcher, no early cases). Abort if < 3 notes remain and original had exactly 3 PCs, or fewer than 2 notes remain.
- Choosing the alternative: `current_is_basic` = regex `^[A-G][b#]?m?$`. If current contains `add9` and alt ends `4` (sus4): try re-detecting the upper structure as sus2 from the current root — if upper intervals from that root == [0,2,7] exactly, alt becomes `{root}2`. Then: alt is sus ending `2` and current is add9 → take alt; alt is sus and current is basic triad → take alt; else take alt iff `complexity(alt) <= complexity(current)`.
- Finally append `/{bass_name}`.

**`_chord_complexity`** (checked in this order on the name, bass stripped):

| Contains | Complexity |
|---|---|
| (empty/None) | 999 |
| `13` | 5 |
| `11` | 4 |
| `9` or `6/9` | 3 |
| `add` or `6` | 3 |
| `7` or `Δ7` or `ø7` | 2 |
| otherwise (triads/sus) | 1 |

---

## 12. `quality_map` — display quality string → pattern key (`_match_chord_type`)

Root is stripped by checking 2-char note names first (`Bb`, `Db`, …) against `NOTE_NAMES_FLAT + NOTE_NAMES`, then 1-char. Bass (`/...`) stripped first.

| Quality | Key | | Quality | Key |
|---|---|---|---|---|
| `` (empty) | major | | `9` | dominant9 |
| `m` | minor | | `11` | dominant11 |
| `dim` | diminished | | `13` | dominant13 * |
| `aug` | augmented | | `Δ9` | major9 |
| `2` | sus2 | | `m9` | minor9 |
| `4` | sus4 | | `Δ11` | major11 |
| `7sus4` | 7sus4 | | `Δ7#11` | major7#11 |
| `7sus2` | 7sus2 | | `m11` | minor11 |
| `7sus13` | 7sus13 | | `Δ13` | major13 |
| `sus13` | sus13 | | `Δ13#11` | major13#11 |
| `Δ7` | major7 | | `m13` | minor13 |
| `Δ7#5` | major7#5 | | `7alt` | altered |
| `m7` | minor7 | | `5` | 5 |
| `mΔ7` | minor_major7 | | `6` | 6 |
| `mΔ7(9)` | minor_major9 | | `6/9` | 6_9 |
| `7` | dominant7 | | `m6` | minor6 |
| `dim7` | diminished7 | | `m6/9` | minor6_9 |
| `dimΔ7` | diminished_major7 | | `add9` | add9 |
| `ø7` | half_diminished7 | | `add11` | add11 |

\* Special rule: quality `13` matches chord_type ∈ {`dominant13`, `13_shell`, `13_no5_no11`, `13_no5`} (returns true for any of the four). All other qualities: exact map lookup. Unmapped qualities (e.g. `m7b5`, `7(b9)`, `(add9)`) → no match. Note: displayed names `m7b5`, `R(add9)`, parenthesized altered names are NOT in this map, so `_match_chord_type` returns False for them — this matters for the slash-chord pattern lookup (pattern search silently finds nothing and `best_pattern` stays None).

---

## 13. Scale detection scoring (`detect_scale`)

| Rule | Value |
|---|---|
| Minimum active notes | 5 |
| Minimum unique pitch classes | 5 |
| `clustered_only_scales` (see §4) skipped unless `is_clustered` OR span < 12 | — |
| `Whole Tone` additionally requires ≥ 6 unique PCs | — |
| Pattern must be a SUBSET of played intervals (all scale tones present; extras allowed) | — |
| Perfect (no extra notes) | `5000 + matched_note_count` |
| Perfect AND scale ∈ major_modes / melodic_minor_modes / harmonic_minor_modes | additional `+1000` |
| Non-perfect (extras) | `matched * 10 − extra * 5` |
| Root == lowest note's PC | `+500` |
| Best score wins (strict `>`); all PCs tried as roots, patterns in dict order | — |

---

## 14. Symbol / formatting summary

- Major 7 symbol: **`Δ7`** (Greek capital delta, U+0394) — e.g. `CΔ7`, `CΔ9`, `CΔ13#11`. Exception: `major7_6_9` renders as `maj7(6/9)` (ASCII `maj7`).
- Half-diminished renders `m7b5` (NOT `ø7`), though `ø7` is accepted as input in `quality_map` and checked in "is_dominant" string tests.
- Minor: lowercase `m` (`Cm`, `Cm7`, `Cm11`, `Cm6/9`, `CmΔ7`, `CmΔ7(9)`).
- Sus triads: bare digits `C2`, `C4` (not `Csus2`/`Csus4`). Sus sevenths keep `sus`: `C7sus4`, `C7sus2`, `C7sus13`, `Csus13`; 9/13 sus use parens: `C9(sus)`, `C13(sus)`.
- Altered tensions: parenthesized, comma-separated, no spaces: `C7(b9)`, `C7(#9,b13)`, `C7(b9,#11,b13)`. Exception: early-return dim7-upper-structure case emits bare `C7b9`. `Δ13#11` and `Δ7#5` have no parens; `Δ7(#11)` and `Δ9(#11)` do.
- add9: `C(add9)` / `Cm(add9)` (parens); add11: `Cadd11` (no parens).
- 6/9: `C6/9`, `Cm6/9` (slash inside name — beware when parsing bass: `_match_chord_type`/complexity split on first `/`).
- Slash chord: `{name}/{bass_note_name}` e.g. `Bbm6/C`.
- Interval (2 notes): `C (P5)`.
- Scale: `C Ionian`.
- No superscripts/Unicode besides `Δ` and `ø` (the latter input-only).

---

## 15. Open questions / port hazards (also listed in summary)

1. `augmented7` display falls through to `f"{root}{chord_type}"` → `Caugmented7`. Likely a latent bug; port verbatim or fix deliberately.
2. Early dim7-upper-structure case returns `7b9` (no parens) while the pattern path returns `7(b9)`; downstream checks look for `'7(b9)' in name` — the bare form bypasses them by design of the code as written.
3. `7sus13` ≡ `13sus` and `7#11_shell` ≡ `13#11_no3_no5` have identical interval sets but different essentials/optionals/bonuses; dict order breaks ties.
4. Scoring depends on dict iteration order in three places: CHORD_PATTERNS loop, SCALE_PATTERNS loop, PC-sorted root loops. Use ordered iteration in Rust.
5. `Counter.most_common(7)` tie order for >7-note inputs is unspecified (set iteration); pick a deterministic rule in Rust and document divergence.
6. `INVERSION_NAMES` is dead data; several test expectations in `test_chord_detector()` do not match the formatter (e.g. expects `Bø7`, formatter emits `Bm7b5`; expects `CΔ7#11` vs `CΔ7(#11)`, `DΔ7` naming) — tests were not kept in sync.
