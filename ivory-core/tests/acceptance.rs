//! Acceptance contract for the Ivory 2.0 chord engine.
//!
//! Sources of truth, in precedence order:
//!   1. docs/DIVERGENCES.md          — K-rules (kept identity) and D-rules (engine fixes)
//!   2. docs/spec/chord-logic.md §13 — the 146 machine-verified Python ground-truth vectors
//!
//! Every §13 vector appears below. Vectors whose shipped Python output is covered by a
//! D-rule carry the FIXED expectation and are tagged with the D-rule id; vectors covered
//! by K-rules (or by no rule) keep the shipped Python expectation and are tagged with the
//! K-rule id or "parity". Many rows are EXPECTED TO FAIL until the engine fixes land —
//! red is the point; this file is the contract the fixes are implemented against.
//!
//! Vector #140 is intentionally NOT asserted (see TODO(classifier) at the bottom).
//! Vector #142 is asserted in `not_a_chord_rows` (D17: assert non-chord, don't guess
//! which scale).
//!
//! Note numbers are MIDI (middle C = 60).

use ivory_core::ChordDetector;
use std::collections::HashSet;

/// (midi notes, expected output, tag). `None` = detector must return None.
type Row = (&'static [u8], Option<&'static str>, &'static str);

fn detect(notes: &[u8], prefer_flats: bool) -> Option<String> {
    // Fresh detector per row: no state may bleed between inputs (D2).
    let mut d = ChordDetector::new();
    d.set_note_preference(prefer_flats);
    let set: HashSet<u8> = notes.iter().copied().collect();
    d.detect_chord(&set)
}

/// Collect-then-panic: one run reports every mismatching row, not just the first.
fn run_table(label: &str, rows: &[Row], prefer_flats: bool) {
    let mut failures: Vec<String> = Vec::new();
    for (notes, expected, tag) in rows {
        let got = detect(notes, prefer_flats);
        if got.as_deref() != *expected {
            failures.push(format!(
                "  [{tag}] {notes:?} -> got {got:?}, expected {expected:?}"
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
// Flat-preference table (prefer_flats = true, the default rendering).
// Tags: "vNNN" = chord-logic.md §13 vector number; "x-*" = extra contract row
// drawn from DIVERGENCES.md examples / transpositions / edge cases.
// ─────────────────────────────────────────────────────────────────────────────
const FLAT_ROWS: &[Row] = &[
    // §13 vectors 1–10: degenerate inputs & intervals (K1 = root-prefixed interval names)
    (&[], None, "v001 parity"),
    (&[60], None, "v002 parity"),
    (&[60, 64], Some("C (M3)"), "v003 K1"),
    (&[60, 67], Some("C (P5)"), "v004 K1"),
    (&[60, 70], Some("C (m7)"), "v005 K1"),
    (&[60, 61], Some("C (m2)"), "v006 K1"),
    (&[60, 72], Some("C (P8)"), "v007 K1"),
    (&[60, 81], Some("C (M13)"), "v008 K1"),
    (&[60, 82], Some("C (22 semitones)"), "v009 K1"),
    (&[60, 72, 84], None, "v010 parity (one PC)"),
    // §13 vectors 11–22: triads & inversions
    (&[60, 64, 67], Some("C"), "v011 parity"),
    (&[60, 63, 67], Some("Cm"), "v012 parity"),
    (&[60, 63, 66], Some("C°"), "v013 parity"),
    (&[60, 64, 68], Some("C+"), "v014 parity"),
    (&[64, 68, 72], Some("E+"), "v015 parity (aug re-root to bass)"),
    (&[60, 62, 67], Some("C2"), "v016 parity"),
    (&[60, 65, 67], Some("C4"), "v017 parity"),
    (&[60, 67, 72], Some("C5"), "v018 parity"),
    (&[64, 67, 72], Some("C/E"), "v019 parity"),
    (&[55, 60, 64], Some("C/G"), "v020 parity"),
    (&[55, 60, 63], Some("Cm/G"), "v021 parity"),
    (&[63, 67, 72], Some("Eb6"), "v022 parity (not Cm/Eb)"),
    // §13 vectors 23–32: sixth-family
    (&[60, 64, 69], Some("C6"), "v023 parity"),
    (&[60, 64, 67, 69], Some("C6"), "v024 parity"),
    (&[60, 64, 69, 74], Some("C6/9"), "v025 parity"),
    (&[60, 64, 67, 69, 74], Some("C6/9"), "v026 parity"),
    (&[60, 67, 69, 74], Some("C6/9"), "v027 D11 (was Am11/C)"),
    (&[60, 64, 67, 69, 71, 74], Some("Cmaj7(6/9)"), "v028 parity"),
    (&[60, 63, 67, 69], Some("Cm6"), "v029 parity"),
    (&[60, 63, 69], Some("Cm6"), "v030 D12 (was Cdim)"),
    (&[60, 62, 63, 69], Some("Cm6/9"), "v031 parity"),
    (&[58, 60, 61, 67], Some("Bbm6/9"), "v032 parity"),
    // §13 vectors 33–42: seventh chords, m7-vs-6 (D1)
    (&[67, 71, 74, 77], Some("G7"), "v033 parity"),
    (&[62, 66, 69, 73], Some("DΔ7"), "v034 parity"),
    (&[60, 63, 67, 70], Some("Cm7"), "v035 D1 (was Eb6, closed voicing)"),
    (&[72, 75, 79, 82], Some("Cm7"), "v036 D1 (was Eb6)"),
    (&[60, 63, 67, 82], Some("Cm7"), "v037 parity (open voicing)"),
    (&[48, 63, 67, 70], Some("Cm7"), "v038 parity (open voicing)"),
    (&[57, 60, 64, 67], Some("Am7"), "v039 D1 declared flip (was C6)"),
    (&[60, 63, 67, 71], Some("CmΔ7"), "v040 parity"),
    (&[60, 63, 67, 71, 74], Some("CmΔ7(9)"), "v041 parity"),
    (&[60, 64, 68, 71], Some("CΔ7#5"), "v042 parity"),
    // §13 vectors 43–50: half-diminished (K2/K10) & diminished
    (&[71, 74, 77, 81], Some("Bø7"), "v043 D27 (K2 superseded: ø7, was m7b5)"),
    (&[55, 58, 61, 65], Some("Gø7"), "v044 K10"),
    (&[58, 61, 65, 67], Some("Bbm6"), "v045 K10 (half-dim inversion → m6)"),
    (&[53, 62, 68, 72], Some("Fm6"), "v046 K10 (Dm7b5, F bass)"),
    (&[68, 71, 74, 78], Some("Abø7"), "v047 K10"),
    (&[60, 63, 66, 69], Some("C°7"), "v048 parity"),
    (&[63, 66, 69, 72], Some("Eb°7"), "v049 parity (dim7 re-root to bass)"),
    (&[60, 63, 66, 71], Some("C°Δ7"), "v050 parity"),
    // §13 vectors 51–58: dominants & slash 7ths
    (&[60, 64, 68, 70], Some("C7(b13)"), "v051 K13 (never Caug7)"),
    (&[60, 64, 70, 80], Some("C7(b13)"), "v052 parity (7b13_no5)"),
    (&[60, 64, 67, 70], Some("C7"), "v053 parity"),
    (&[48, 60, 64, 67, 70], Some("C7"), "v054 parity (doubled root)"),
    (&[64, 67, 70, 72], Some("C7/E"), "v055 parity"),
    (&[58, 64, 67, 72], Some("C/Bb"), "v056 parity (bass not doubled → triad)"),
    (&[46, 58, 64, 67, 72], Some("C7/Bb"), "v057 parity (bass doubled → keep 7th)"),
    (&[60, 64, 67, 70, 74], Some("C9"), "v058 parity"),
    // §13 vectors 59–61: rootless dominants (D9)
    (&[64, 70, 74], Some("C9"), "v059 D9 (was E7(#11))"),
    (&[64, 70, 74, 78], Some("C7(#11)"), "v060 D9 (was E7(#11))"),
    (&[66, 70, 74], Some("Gb+"), "v061 D9-keep (no tritone, symmetric)"),
    // §13 vectors 62–73: extended chords, 13-family (D3), m11 (D20)
    (&[60, 63, 67, 70, 74], Some("Cm9"), "v062 parity"),
    (&[60, 64, 67, 71, 74], Some("CΔ9"), "v063 parity"),
    (&[60, 64, 70, 81], Some("C13"), "v064 parity (13_shell)"),
    (&[60, 64, 70, 74, 81], Some("C13"), "v065 parity"),
    (&[60, 64, 67, 70, 74, 81], Some("C13"), "v066 parity"),
    (&[60, 64, 67, 71, 74, 77, 81], Some("CΔ13"), "v067 D3 (was FΔ13#11)"),
    (&[60, 63, 67, 70, 74, 77, 81], Some("Cm13"), "v068 D3 (was EbΔ13#11)"),
    (&[60, 64, 67, 70, 74, 77], Some("C11"), "v069 parity"),
    (&[60, 64, 67, 71, 74, 77], Some("CΔ11"), "v070 D3 (was FΔ9(#11))"),
    (&[60, 63, 67, 70, 74, 77], Some("Cm11"), "v071 parity"),
    (&[60, 63, 70, 74, 77], Some("Cm11"), "v072 parity (quintal no-5th)"),
    (&[66, 69, 71, 76], Some("Gbm11"), "v073 D20-keep (root == bass keeps 8000)"),
    // §13 vectors 74–78: maj7#11 (K3), 7#9 (D4)
    (&[60, 64, 67, 71, 74, 78], Some("CΔ9(#11)"), "v074 K3 (parens kept)"),
    (&[60, 71, 78], Some("CΔ7(#11)"), "v075 K3"),
    (&[65, 69, 71, 76], Some("FΔ7(#11)"), "v076 parity"),
    (&[60, 63, 64, 67, 70], Some("C7(#9)"), "v077 D4 (was EΔ7(#11))"),
    (&[60, 64, 70, 75], Some("C7(#9)"), "v078 D4 (was EΔ7(#11))"),
    // §13 vectors 79–82: 7b9 family (D5/D6, E2 early path)
    (&[60, 61, 64, 70], Some("C7(b9)"), "v079 D5 (was Bbdim/C)"),
    (&[60, 61, 64, 67, 70], Some("C7(b9)"), "v080 D6 (was C7b9, no parens)"),
    (&[53, 57, 60, 63, 66], Some("F7(b9)"), "v081 D6 (was F7b9)"),
    (&[60, 62, 65, 68, 71], Some("D°7/C"), "v082 D5-keep (genuine E2 branch)"),
    // §13 vectors 83–97: altered dominants (D4/K3)
    (&[60, 64, 70, 78], Some("C7(#11)"), "v083 parity"),
    (&[60, 64, 67, 70, 78], Some("C7(#11)"), "v084 D4 (was Gb7(b9,#11))"),
    (&[60, 64, 67, 70, 80], Some("C7(b13)"), "v085 D4 (was Bb7(#11))"),
    (&[60, 64, 67, 70, 75, 78], Some("C7(#9,#11)"), "v086 D4 (was Gb7(b9,#11))"),
    (&[67, 70, 71, 75, 77], Some("G7(#9,b13)"), "v087 K3 (comma format kept)"),
    (&[55, 65, 68, 71, 75], Some("G7(b9,b13)"), "v088 parity"),
    (&[60, 61, 64, 66, 70], Some("C7(b9,#11)"), "v089 parity"),
    (&[60, 64, 70, 73, 78], Some("C7(b9,#11)"), "v090 parity"),
    (&[60, 64, 70, 73, 80], Some("C7(b9,b13)"), "v091 parity"),
    (&[60, 70, 74, 76, 80], Some("C9(b13)"), "v092 parity"),
    (&[60, 64, 70, 74, 80], Some("C9(b13)"), "v093 parity"),
    (&[53, 63, 67, 69, 73], Some("F9(b13)"), "v094 parity"),
    (&[62, 63, 66, 68, 71, 72], Some("D7(b9,#11)"), "v095 parity"),
    (&[60, 70, 74, 78, 81], Some("C13(#11)"), "v096 parity"),
    (&[60, 64, 67, 70, 74, 78, 81], Some("C13(#11)"), "v097 parity (7-PC 13#11)"),
    // §13 vector 98: 8-PC scale still detects (D17 keeps scale path)
    // D26: this 8-PC set is the 7-tone altered scale PLUS the natural 5th, so it is
    // NOT "C Altered" (a scale must contain every sounded tone). No exact 8-tone
    // scale matches it, so per D17 (8+ PC, no scale) it resolves to None — same
    // principle that stops a 6-tone set reading as a 5-tone pentatonic.
    (&[60, 61, 63, 64, 66, 67, 68, 70], None, "v098 D26 (8 PCs ⊋ altered scale → None)"),
    // §13 vectors 99–107: sus family (D20/D7/D8, K5/K11/K12)
    (&[60, 65, 67, 70], Some("C7sus4"), "v099 D20 (was Bbm11)"),
    (&[60, 62, 67, 70], Some("Gm/C"), "v100 K11 (kept identity)"),
    (&[60, 62, 65, 70], Some("Bb(add9)/C"), "v101 K5 (compact span rule)"),
    (&[60, 62, 65, 82], Some("C9(sus)"), "v102 K5 (spread span rule)"),
    (&[60, 70, 74, 77], Some("C9(sus)"), "v103 parity"),
    (&[60, 70, 74, 77, 79], Some("C9(sus)"), "v104 D7 (was Gm11, spread w/ 5th)"),
    (&[60, 62, 65, 69], Some("C6/9"), "v105 K12 (no b7 → 6/9 stays)"),
    (&[60, 62, 65, 69, 70], Some("C13(sus)"), "v106 D8 (was C6/9, b7 present)"),
    (&[60, 62, 65, 69, 82], Some("C13(sus)"), "v107 parity (13sus spread)"),
    // §13 vectors 108–114: add9 family (K4)
    (&[60, 62, 64, 67], Some("C(add9)"), "v108 K4"),
    // D24: a MAJOR add9 with its 3rd in the bass names as sus2/bass (owner pref
    // 2026-08-10). E-G-C-D is the {0,3,8,10}-from-bass voicing of the owner's
    // C-Ab-Bb-Eb → Ab2/C, transposed to E; "transferable to every key" makes the
    // flip from the old C(add9)/E parity mandatory. See DIVERGENCES.md D24.
    (&[64, 67, 72, 74], Some("C2/E"), "v109 D24 (add9 3rd-in-bass → sus2/bass)"),
    (&[62, 64, 67, 72], Some("C(add9)/D"), "v110 parity"),
    (&[62, 64, 67, 84], Some("D9(sus)"), "v111 parity (span ≥ 12 → 9sus)"),
    (&[60, 62, 63, 67], Some("Cm(add9)"), "v112 parity"),
    (&[55, 60, 62, 63], Some("Cm(add9)/G"), "v113 parity"),
    // D25: a bass-rooted maj7 shell carrying the 6/13 (root+M3+M7+M6 from the bass)
    // is a genuine XΔ13, not the 13th read as a rootless minor-add9 (owner pref
    // 2026-08-10). Eb-G-C-D is the {0,4,9,11}-from-bass voicing of the owner's
    // B-Ab-Bb-Eb → BΔ13, transposed to Eb. See DIVERGENCES.md D25.
    (&[63, 67, 72, 74], Some("EbΔ13"), "v114 D25 (bass-rooted maj13 shell)"),
    // §13 vectors 115–125: early exits & special cases (K11, D5)
    (&[60, 70, 73, 79], Some("Bbm6/C"), "v115 K11 (E1)"),
    (&[60, 70, 73, 77, 79], Some("Bbm6/C"), "v116 K11 (E1b)"),
    (&[60, 70, 74, 79], Some("Bb6/C"), "v117 K11 (S2k, Bb second)"),
    (&[60, 67, 70, 74], Some("Gm/C"), "v118 K11 (S2k, G second)"),
    (&[60, 62, 65, 67, 70], Some("G Minor Pentatonic"), "v119 K6 (compact → scale)"),
    (&[64, 69, 70, 74], Some("Eø7(11)"), "v120 parity"),
    (&[60, 65, 66, 70], Some("Cø7(11)"), "v121 parity"),
    (&[55, 58, 63, 65], Some("Eb2/G"), "v122 D24 (add9 3rd-in-bass → sus2/bass)"),
    (&[55, 58, 63, 65, 67], Some("Eb2/G"), "v123 D24 (add9 3rd-in-bass → sus2/bass)"),
    (&[53, 63, 67, 70], Some("F9(sus)"), "v124 parity (Eb/F)"),
    (&[55, 71, 77, 80], Some("G7(b9)"), "v125 D5 (was Fdim/G)"),
    // §13 vectors 126–139: scales (K6/K7/K8, D17-keep)
    (&[65, 67, 69, 70, 72, 74, 76], Some("F Ionian"), "v126 parity"),
    (&[60, 62, 63, 65, 67, 68, 70], Some("C Aeolian"), "v127 parity"),
    (&[62, 64, 65, 67, 69, 71, 72], Some("D Dorian"), "v128 parity"),
    (&[60, 62, 64, 65, 67, 69, 71], Some("C Ionian"), "v129 parity"),
    (&[60, 62, 64, 66, 67, 69, 71], Some("C Lydian"), "v130 parity"),
    (&[60, 62, 64, 67, 69], Some("C Major Pentatonic"), "v131 parity"),
    (&[62, 64, 67, 69, 72], Some("C Major Pentatonic"), "v132 K8 (doc's D-min-pent wrong)"),
    (&[60, 62, 76, 79, 81], Some("C6/9"), "v133 K6 (spread → chord)"),
    (&[60, 63, 65, 67, 70], Some("C Minor Pentatonic"), "v134 parity"),
    (&[60, 62, 64, 66, 68, 70], Some("C Whole Tone"), "v135 parity"),
    (&[60, 62, 64, 66, 68], Some("D7(#11)"), "v136 K7 (Whole Tone needs 6 PCs)"),
    (&[60, 63, 65, 66, 67, 70], Some("C Minor Blues"), "v137 parity"),
    (&[60, 62, 63, 64, 67, 69], Some("C Major Blues"), "v138 parity"),
    (
        &[60, 61, 63, 64, 66, 67, 69, 70],
        Some("C Half-Whole Diminished"),
        "v139 D17-keep (8-PC scale)",
    ),
    // D22 (owner pref 2026-08-10): a scale played root-to-root includes the octave,
    // so a stepwise (`is_clustered`) voicing keeps its scale reading at span >= 12
    // instead of flipping to maj13 the instant the octave is added. Vector #140 —
    // the compact C-major scale + octave — resolves to C Ionian (was left open in
    // TODO(classifier); the owner pinned it). A spread tertian stack (v067's CΔ13,
    // v133's C6/9) is NOT clustered, so it still names as a chord. See DIVERGENCES D22.
    (&[60, 62, 64, 65, 67, 69, 71, 72], Some("C Ionian"), "v140 D22 (scale + octave)"),
    (&[60, 62, 63, 65, 67, 68, 70, 72], Some("C Aeolian"), "x-D22 minor scale + octave"),
    (&[60, 62, 64, 67, 69, 72], Some("C Major Pentatonic"), "x-D22 maj pent + octave"),
    (&[60, 63, 65, 67, 70, 72], Some("C Minor Pentatonic"), "x-D22 min pent + octave"),
    (&[60, 63, 65, 66, 67, 70, 72], Some("C Minor Blues"), "x-D22 min blues + octave"),
    (&[60, 62, 63, 64, 67, 69, 72], Some("C Major Blues"), "x-D22 maj blues + octave"),
    // D26 (owner report 2026-08-10): a scale must account for every sounded tone, so
    // a 6+ unique-PC set is NEVER a 5-tone pentatonic (nor any smaller scale). These
    // fall through to chord detection; the genuine 5-tone pentatonics (v131/v134) and
    // exact 6-tone scales (v135 whole-tone, v137 blues) still name as scales.
    (&[60, 62, 64, 65, 67, 69], Some("Dm11"), "x-D26 CDEFGA not C maj pent"),
    (&[55, 57, 59, 60, 62, 64], Some("Am11"), "x-D26 GABCDE transposition"),
    (&[60, 62, 63, 65, 67, 70], Some("Cm11"), "x-D26 6-tone not C min pent"),
    (&[60, 62, 64, 65, 67, 71], Some("CΔ11"), "x-D26 CDEFGB (already a chord)"),
    (&[60, 61, 62, 63, 64], Some("DbmΔ7/C"), "v141 parity (chromatic cluster)"),
    // v142 asserted in not_a_chord_rows (D17).
    (&[60, 64, 67, 72, 76, 79], Some("C"), "v143 parity (doubled octaves)"),
    (&[52, 55, 60, 64], Some("C/E"), "v144 parity (doubled 3rd)"),
    (&[60, 64, 65, 67], Some("Cadd11"), "v145 parity"),
    (&[60, 64, 65, 67, 69], Some("C6add4"), "v146 parity"),
    // ── Owner report 2026-08-10 (D23/D24/D25), as intervals for every key ───
    // D23: a bare {0,2,4} is C(add9)-no-5, NOT a #11 shell from a third above —
    // the #11 identity bonus now requires the tritone to actually sound.
    (&[60, 62, 64], Some("C(add9)"), "x-D23 CDE root pos"),
    (&[67, 69, 71], Some("G(add9)"), "x-D23 GAB transposition"),
    (&[62, 64, 66], Some("D(add9)"), "x-D23 DEF# transposition"),
    // D24: MAJOR add9 with the 3rd in the bass → sus2/bass, on the add9's own root.
    (&[60, 68, 70, 75], Some("Ab2/C"), "x-D24 owner C-Ab-Bb-Eb"),
    (&[53, 61, 63, 68], Some("Db2/F"), "x-D24 F-Db-Eb-Ab transposition"),
    // D25: bass-rooted maj7 shell with the 6/13 → XΔ13, not rootless minor(add9).
    (&[59, 68, 70, 75], Some("BΔ13"), "x-D25 owner B-Ab-Bb-Eb"),
    (&[64, 68, 73, 75], Some("EΔ13"), "x-D25 E-G#-C#-D# transposition"),
    // ── Extra rows from DIVERGENCES.md examples ─────────────────────────────
    // D1: bass = shared 3rd/5th of {A,C,E,G} keeps Python's shipped answers.
    (&[64, 67, 69, 72], Some("C6/E"), "x-D1-keep E bass"),
    (&[55, 57, 60, 64], Some("G6/9"), "x-D1-keep G-A-C-E"),
    // D1 transpositions (rules are pitch-class-relational, DIVERGENCES note).
    (&[62, 65, 69, 72], Some("Dm7"), "x-D1 closed Dm7"),
    (&[64, 67, 71, 74], Some("Em7"), "x-D1 closed Em7"),
    (&[61, 64, 68, 71], Some("Dbm7"), "x-D1 closed Dbm7"),
    (&[62, 66, 69, 71], Some("D6"), "x-D1 bass=6-root D-F#-A-B"),
    // D3 transpositions: 7-PC tertian stacks name from a coherent bass root.
    (&[53, 57, 60, 64, 67, 70, 74], Some("FΔ13"), "x-D3 FΔ13 stack"),
    (&[55, 58, 62, 65, 69, 72, 76], Some("Gm13"), "x-D3 Gm13 stack"),
    // D4: named voicing C-E-G-Bb-Eb (Eb on top) and a transposition with the 5th.
    (&[60, 64, 67, 70, 75], Some("C7(#9)"), "x-D4 Eb on top"),
    (&[53, 57, 60, 63, 68], Some("F7(#9)"), "x-D4 F transposition"),
    // D9 transposition: rootless F9 shell (A-Eb-G contains the F7 tritone A-Eb).
    (&[57, 63, 67], Some("F9"), "x-D9 rootless F9"),
    // D10: E-G-Bb-D-C, E bass, C on top → C9/E (doc §2, was E7(#11)).
    (&[52, 55, 58, 62, 72], Some("C9/E"), "x-D10 low voicing"),
    (&[64, 67, 70, 74, 84], Some("C9/E"), "x-D10 high voicing"),
    // D12 transposition: bass-coherent minor6_no5.
    (&[62, 65, 71], Some("Dm6"), "x-D12 D-F-B"),
    // D13: doublings never change the PC set; 8+ sounding notes with ≤7 PCs
    // are still chords (no reduction lottery).
    (&[48, 52, 55, 60, 64, 67, 72, 76, 79], Some("C"), "x-D13 9 notes 3 PCs"),
    (&[48, 60, 64, 67, 70, 72, 76, 79, 82], Some("C7"), "x-D13 9 notes 4 PCs"),
    // D26 (owner report 2026-08-10): all twelve PCs is the Chromatic Scale, not an
    // 8-tone diminished scale it happens to contain. The old `C Whole-Half
    // Diminished` here was the subset-match bug (a 12-tone input can't be an 8-tone
    // scale) — detect_scale now requires an exact PC match, so this falls through to
    // the 12-PC → "Chromatic Scale" rule.
    (
        &[60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71],
        Some("Chromatic Scale"),
        "x-D26 compact chromatic (was subset-match Whole-Half Dim)",
    ),
    (
        &[48, 50, 52, 54, 56, 58, 61, 63, 65, 67, 69, 71],
        Some("Chromatic Scale"),
        "x-D17 spread chromatic",
    ),
    // D20 transpositions delivered by the constrained m11 bonus.
    (&[62, 67, 69, 72], Some("D7sus4"), "x-D20 D-G-A-C"),
    (&[53, 60, 62, 67], Some("F6/9"), "x-D11 F-C-D-G"),
    (&[62, 72, 76, 79, 81], Some("D9(sus)"), "x-D7 spread D-C-E-G-A"),
    (&[55, 57, 60, 64, 65], Some("G13(sus)"), "x-D8 compact G-A-C-E-F"),
    // Flat-rendering sanity (counterparts to the sharp-preference table).
    (&[61, 65, 68], Some("Db"), "x-flat Db triad"),
    (&[66, 70, 73], Some("Gb"), "x-flat Gb triad"),
    (&[68, 72, 75], Some("Ab"), "x-flat Ab triad"),
    (&[63, 66, 70], Some("Ebm"), "x-flat Ebm triad"),
    (&[58, 62, 65, 68], Some("Bb7"), "x-flat Bb7"),
    (&[61, 65], Some("Db (M3)"), "x-flat K1 interval"),
    // K1 interval sanity beyond the §13 rows.
    (&[60, 62], Some("C (M2)"), "x-K1 M2"),
    (&[60, 63], Some("C (m3)"), "x-K1 m3"),
    (&[60, 65], Some("C (P4)"), "x-K1 P4"),
    (&[60, 66], Some("C (d5)"), "x-K1 d5"),
    (&[60, 68], Some("C (m6)"), "x-K1 m6"),
    (&[60, 69], Some("C (M6)"), "x-K1 M6"),
    (&[60, 71], Some("C (M7)"), "x-K1 M7"),
    (&[60, 83], Some("C (23 semitones)"), "x-K1 >21 semitones"),
    // All-octaves-same-PC → None (unique PCs < 2), any count of sounding notes.
    (&[48, 60, 72, 84], None, "x-edge C octaves x4"),
    (&[40, 52, 64, 76, 88], None, "x-edge E octaves x5"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Sharp-preference spot checks (prefer_flats = false). Every rule is
// pitch-class-relational and applies identically under sharps (DIVERGENCES,
// naming preference note).
// ─────────────────────────────────────────────────────────────────────────────
const SHARP_ROWS: &[Row] = &[
    (&[61, 65, 68], Some("C#"), "s-C# triad"),
    (&[66, 70, 73], Some("F#"), "s-F# triad"),
    (&[68, 72, 75], Some("G#"), "s-G# triad"),
    (&[63, 66, 70], Some("D#m"), "s-D#m triad"),
    (&[63, 67, 72], Some("D#6"), "s-v022 sharp"),
    (&[58, 62, 65, 68], Some("A#7"), "s-A#7"),
    (&[61, 65], Some("C# (M3)"), "s-K1 interval"),
    (&[57, 60, 64, 67], Some("Am7"), "s-D1 v039 sharp"),
    (&[60, 63, 67, 70], Some("Cm7"), "s-D1 v035 sharp"),
    (&[61, 64, 68, 71], Some("C#m7"), "s-D1 closed C#m7"),
    (&[66, 69, 71, 76], Some("F#m11"), "s-D20-keep v073 sharp"),
    (&[55, 71, 77, 80], Some("G7(b9)"), "s-D5 v125 sharp"),
    (&[60, 61, 64, 70], Some("C7(b9)"), "s-D5 v079 sharp"),
    (&[60, 63, 64, 67, 70], Some("C7(#9)"), "s-D4 v077 sharp"),
    (&[53, 63, 67, 70], Some("F9(sus)"), "s-v124 sharp (no accidental)"),
];

#[test]
fn flat_preference_rows() {
    run_table("flat-preference", FLAT_ROWS, true);
}

#[test]
fn sharp_preference_rows() {
    run_table("sharp-preference", SHARP_ROWS, false);
}

// ─────────────────────────────────────────────────────────────────────────────
// D17: ≥8 unique pitch classes NEVER name a chord. The outcome is a scale name
// or None; where DIVERGENCES does not pin which scale, we assert only
// "not a chord". Discriminator: for ≥3-note inputs, chord names never contain
// a space while every scale name does (interval strings need exactly 2 notes).
// Includes §13 vector #142 (declared flip: was `G7(b9,#11)/C` via the removed
// reduction lottery).
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn not_a_chord_rows() {
    let mut cases: Vec<(Vec<u8>, &'static str)> = vec![
        (
            vec![60, 61, 62, 63, 64, 65, 66, 67, 68],
            "v142 D17 (9-PC spread chromatic; was G7(b9,#11)/C)",
        ),
        (
            vec![60, 61, 62, 64, 66, 67, 69, 70],
            "x-D17 8 PCs, non-scale set",
        ),
        (
            vec![60, 61, 62, 63, 64, 65, 66, 67, 68, 69],
            "x-D17 10 PCs",
        ),
    ];
    cases.push(((21u8..=108).collect(), "x-D17 all 88 keys"));

    let mut failures: Vec<String> = Vec::new();
    for (notes, tag) in &cases {
        let got = detect(notes, true);
        let ok = match &got {
            None => true,
            // Scale names ("C Altered", "Chromatic Scale", …) contain a space;
            // chord names never do.
            Some(s) => s.contains(' '),
        };
        if !ok {
            failures.push(format!(
                "  [{tag}] {} notes -> got chord {got:?}, expected a scale name or None",
                notes.len()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "not-a-chord (D17): {} of {} rows failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// TODO(classifier): outcomes DIVERGENCES.md leaves genuinely under-determined.
// Do NOT assert these until the differential classification pass pins them;
// the engine surgeon should not treat any particular answer here as the
// contract.
//
// - [RESOLVED 2026-08-10 → D22] §13 vector #140 {60,62,64,65,67,69,71,72}: the
//   octave-doubled C-major scale now reads `C Ionian` (asserted above as v140).
//   A stepwise voicing keeps its scale reading past span == 12; only a spread
//   (non-clustered) tertian stack of the same PC set names as a chord.
// - D5 voicings other than its named examples, e.g. {60,64,70,73}
//   (C-E-Bb-Db with Db on top): the constrained +380 pins only the two named
//   flips ({60,61,64,70} → C7(b9), {55,71,77,80} → G7(b9)); what the bonus
//   yields for other orderings of the {0,1,4,10} set is corpus-audit territory.
// - D5: replacement outputs for non-root-position dim-slash readings shadowed
//   by the old blanket bonus (beyond the two named vectors).
// - D15 (7-PC early scale fallback, PC match instead of `startswith`): no
//   concrete vector is named in DIVERGENCES; an A-vs-Ab/A# collision input
//   needs to be minted during classification.
// - D17: which scale name (if any) the 88-key and other clustered ≥8-PC
//   inputs resolve to — asserted above only as "not a chord".
// ─────────────────────────────────────────────────────────────────────────────
