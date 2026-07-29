# DIVERGENCES — where Ivory 2.0 intentionally differs from the Python engine

Policy: the rewrite reproduces the Python engine's *shipped* behavior exactly,
except where that behavior is a documented bug (the B-catalog in
`docs/spec/chord-logic.md`, cross-checked against
`01_Special_Cases_and_Resolutions.md`) or an indefensible musical error.

Divergence accounting works with **classified data, not matchers**: after the
engine lands, the differential harness diffs the Rust engine against
`tests/golden/corpus.json` (13,133 note sets, flat + sharp renderings). Each
mismatch is classified once — scripted heuristics plus hand audit — and
committed as `tests/golden/classified-divergences.json` mapping note-set →
D-rule ID (covering both naming preferences). CI asserts the live mismatch
set equals the classified set exactly; any new, vanished, or reclassified
mismatch fails the build. After the audit, outputs freeze as
`tests/golden/rust-golden.json` — the permanent regression baseline.

This policy was adversarially reviewed by a three-lens critic panel
(music-theory lens executed the Python to verify claims); all decisions below
are post-review.

## Kept as-is (identity, not bugs)

- K1 (B1): 2-note intervals render `"C (M3)"` (root-prefixed), `"{n}
  semitones"` beyond 21.
- K2 (B2): half-diminished renders `m7b5`, never `ø7`.
- K3 (B6/B10): parenthesized, comma-joined tensions: `CΔ7(#11)`,
  `G7(#9,b13)`.
- K4: `Δ` glyph for major-7 family, `C(add9)`, `C6/9`, `/Bass` slash format.
- K5 (resolves the doc §1/§3 self-contradiction): the span rule for the
  `[0,2,5,10]`-from-bass shape is kept intact — compact → `(bass+10)(add9)/
  bass` (e.g. C-D-F-Bb → `Bb(add9)/C`), spread → `bass 9(sus)`. The two doc
  claims cannot both hold in a transposition-invariant engine; the span
  mechanism is the shipped, coherent one. (B13 reclassified: not a bug.)
- K6: scale-vs-chord span rule (within an octave prefer scale on ≥5 PCs).
- K7 (B20): Whole Tone requires ≥6 PCs.
- K8 (B16): "D E G A C → C Major Pentatonic" — the doc's claim of `D Minor
  Pentatonic` is musically wrong; code was right.
- K9: auto-connect priority chain incl. "Scarlett"; CC64 sustain semantics;
  channels merged. (UI, listed for completeness.)
- K10 (B24): the E3 half-diminished early return stays exactly as shipped —
  its outcomes (`m7b5` when root==bass, m6-from-root+3 otherwise) are all
  verified-correct vectors; the "dead" ø7/700-bonus code paths it shadows
  are simply not ported. (Supersedes the earlier D16 idea, which was found
  to regress vectors #44–#47.)
- K11: E1/E1b (`Xm6/bass` early exits), both E2 branches (`Xdim7/bass` and
  the 7b9 upper-structure), and S2k's Bb6/C-vs-Gm/C second-note voicing
  discrimination are kept-identity special cases. C-D-G-Bb stays `Gm/C`
  (idiomatic 9sus shorthand — deliberate, not a B4 artifact; verified
  unchanged under mutation removal).
- K12 (B18-partial): C-D-F-A (no 7th) → `C6/9` stays, even though sus13
  `[0,2,5,9]` is an exact table match — shipped identity. The b7 cases are
  fixed by D8.
- K13 (B22): C-E-G#-Bb → `C7(b13)` (the 7b13_no5 reading) stays; see D14
  for the table hygiene.

## Engine fixes

- **D1** (B3): closed-voicing complete m7 is NOT reinterpreted as relative
  major 6. Scope is limited to basses that are one of the two candidate
  roots of the {m7-root, 6-root} pair: bass = 6-root → 6 chord (C-E-G-A →
  `C6`), bass = m7-root → m7 (A-C-E-G → `Am7`, C-Eb-G-Bb any voicing →
  `Cm7`). When the bass is the shared 3rd/5th (E or G in the {A,C,E,G}
  set), Python's shipped answers are kept (`C6/E` for E bass, `G6/9` for
  G-A-C-E) — identity, not covered by this rule. Explicitly declared flip:
  acceptance vector #39 (A-C-E-G → was `C6`, doc-"intended") becomes `Am7`;
  root-position m7 over its own root is the only defensible reading.
- **D2** (B4): the `root_pc` mutation corruption is removed (scoring is
  pure; no state leaks between pattern evaluations). Honest expected
  effects: closed Cm7-family sets stop misnaming via D1; C-F-G-Bb →
  `C7sus4` arrives via D20 (not via mutation removal alone — verified that
  mutation removal by itself leaves `Gm11`).
- **D3** (B5): 7-PC tertian stacks name from the bass root when the bass
  holds a coherent 13-family reading: CΔ13 set → `CΔ13`, Cm13 set →
  `Cm13`, CΔ11 set → `CΔ11`.
- **D4** (B7, extended per review): altered dominants name from the bass
  root whenever the bass is a root-in-bass dominant (M3 + m7 above the bass
  present), with or without the natural 5th: C-E-G-Bb-Eb → `C7(#9)`,
  C-E-Bb-Eb → `C7(#9)`, C-E-G-Bb-F# → `C7(#11)`, C-E-G-Bb-Ab → `C7(b13)`,
  C-E-G-Bb-Eb-F# → `C7(#9,#11)`.
- **D5** (B8, restated per review — the bug lives in mid-loop scoring, not
  the E2 early path): the S2c2b else-branch blanket +380 (the spec's own
  "blanket distorter", §9.13) is constrained so it can no longer crown
  spurious mΔ7 readings over root-position dominants; root-position
  7(b9)-no5 voicings win: C-E-Bb-Db → `C7(b9)`, G-B-F-Ab → `G7(b9)` (was
  `Bbdim/C`, `Fdim/G`). E2's genuine branches are untouched (Ddim7/C,
  F7b9-style outputs preserved; verified).
- **D6** (B9): the early-path bare `7b9` gains parens: always `C7(b9)`.
- **D7** (B11, spread voicings only — compact orderings of this PC set are
  correctly eaten by the K6 scale check): spread C-Bb-D-F-G → `C9(sus)`
  (9sus_with5 from the bass), not `Gm11`. Delivered by D20.
- **D8** (B18): with the b7 present, compact root-position 13sus wins over
  the 6/9-no3 reading: C-D-F-A-Bb → `C13(sus)`.
- **D9** (B14 + doc §7, trimmed per review): rootless dominant voicings
  that genuinely contain the defining tritone resolve to the implied root:
  E-Bb-D → `C9`, E-Bb-D-F# → `C7(#11)`. The bare augmented triad F#-Bb-D
  keeps `Gbaug` (no tritone; symmetric — nothing selects a root; the doc's
  claim was wrong).
- **D10** (B15 + doc §2): E-G-Bb-D-C (E bass, C on top) → `C9/E`.
- **D11** (B17): C-G-A-D (C bass) → `C6/9`, not `Am11/C`. Delivered by D20.
- **D12** (B19, decided per review — supersedes the Adim/C draft): C-Eb-A →
  `Cm6` (bass-coherent perfect minor6_no5 table match; consistent with
  C-Eb-G-A → `Cm6` and with D1's bass-coherence principle; matches the
  module's own acceptance test).
- **D13** (B26, simplified per review): there is **no PC-reduction step**.
  ≤7 unique PCs → all are used, regardless of how many notes sound
  (doublings never change the PC set). ≥8 unique PCs → never a chord; see
  D17. Fully deterministic; the Counter.most_common(7) iteration-order
  lottery is gone.
- **D14** (B22/B23, hygiene only — zero behavior change): the shadowed
  duplicate patterns `7sus13` (== `13sus`) and `augmented7` (== `7b13_no5`,
  and #5-vs-b13 is not recoverable from pitch classes) are deleted from the
  table. Their outputs were unreachable; K13 pins the surviving behavior.
  The never-reachable generic-formatter string `Caugmented7` can no longer
  exist even in principle.
- **D15** (B21): the 7-PC early scale fallback matches the scale root by
  pitch class, not string `startswith` (which confused A with Ab/A#).
- **D17** (edge, corrected per review): ≥8 unique pitch classes never name
  a chord — the scale check runs against the original notes (8-PC
  diminished scales still detect; compact 12-PC already yields
  `C Whole-Half Diminished`); an all-12-PC set with no scale hit (spread
  chromatic) renders `Chromatic Scale` (rootless label, no note name);
  otherwise `None`. Declared flip: vector #142 (9-note spread chromatic →
  was `G7(b9,#11)/C` via reduction lottery). Kept identity: everything
  with ≤7 unique PCs.
- **D18**: the `altered` pattern keeps Python's interval set (natural 5
  as optional) — resolved against the old Rust core's silent P5 drop.
- **D19** (B27): dead code not ported: `INVERSION_NAMES`, `PREFER_FLATS`
  global, ignored `lowest_note` param.
- **D20** (new, per review — the load-bearing mechanism behind D2/D7/D11):
  the m11-family "perfect match" +8000 bonus is suppressed only when the bass
  roots the competing resolved chord — bass interval ∈ {3 (relative-major
  6/9), 5 (the 9sus root)}. Root-position m11 voicings keep it (F#-A-B-E →
  `Gbm11`, #73), and inversions where the bass is another chord tone (a Gm11
  drop-2 with the 9th in the bass) stay `Gm11`; only the two competing-root
  intervals demote it, letting bass-coherent sus/6-9/7sus readings win
  (C-F-G-Bb → `C7sus4`; spread C-Bb-D-F-G → `C9(sus)`; C-G-A-D → `C6/9`).
  (The initial blunt root==bass gate flipped 44 m11 inversions to the relative
  major; the interval-{3,5} refinement fixed them. Corpus-audited.)
- **D21** (new, from the differential classifier's regression hunt): two
  linked completeness fixes so a reading never hides a played tension.
  (a) When the top-scored reading drops a sounded note (extra > 0) and a
  note-complete reading (extra == 0) scored within 12 points, the complete
  one wins — fixes drop-2 voicings where a bare triad/Δ7 edged the full
  tertian by ~2 points via the inversion bonus (Ab-C-E-B → `CΔ7#5/Ab`, not
  `E/Ab`; a maj7#5 drop-2, 15 rows). (b) The major7#11 boost also applies to a
  perfect full voicing (5th present, nothing missing/extra) from a non-bass
  root — a genuine inversion (B-C-E-F#-G → `CΔ7(#11)/B`, 13 rows) — while a
  no-5th shell from a non-bass root stays gated so v120 (E-A-Bb-D →
  `Em7b5(11)`) is preserved. KNOWN RESIDUAL: a slash-simplification step can
  still drop a #11 on a non-root-bass tritone voicing (Bb-C-E-F# → `Gb7/Bb`);
  left as a documented limitation (reviewer-rated non-blocking; Python is also
  wrong; the slash logic is parity-critical).

Naming preference note: all examples above are written in flats
(prefer_flats=true, the default); every rule is pitch-class-relational and
applies identically under sharps. The classification file covers both
renderings of every affected row.

## UI parity exceptions

- **D-UI-1**: `detached_chord_height` persisted value is honored (Python
  overwrote it with 50 on init); values ≤ 0 fall back to 50.
- **D-UI-2**: single-instance via lock file — same dialog + exit UX; a
  crashed instance never blocks relaunch.
- **D-UI-3**: no busy-loop rendering; repaint scheduled on MIDI/timer
  events at Python cadences (50ms GUI / 100ms detection). Keytoggle clicks
  trigger an immediate off-cadence detection + repaint (as Python did).
- **D-UI-4**: MIDI picker shows a dead current connection as
  "(disconnected)"; no auto-reconnect (parity).
- **D-UI-5**: teach-layer additions, precisely scoped: two context-menu
  items inside the chord-detection block, immediately after the
  Detach/Attach entry, preceded by their own separator — `Teach Chord
  Name...` (greyed when no notes held) and `Manage Taught Chords...`. All
  other menu items keep byte-identical order. Additive settings key
  `custom_font_path` in settings.json; `learning_mode` lives in
  overrides.json (Python rewrites settings.json with a fixed key set, so
  additive settings keys are lost on a Python downgrade — tolerable for
  the font path, not for learning state).
- **D-UI-6**: About dialog shows "Version 2.0.0" and adds one 8pt
  left-aligned credit line under the version: "Courier Prime © The Courier
  Prime Project Authors, SIL OFL 1.1". Every other About string, size,
  color, and layout is parity.
- **D-UI-7**: color selection uses an egui modal (egui color picker) themed
  like the About dialog, with the spec's exact titles, seeded with the
  current color ("Set Active Key Color..." seeds from
  white_key_active_color and writes both active keys), OK applies + saves,
  Cancel/close is a strict no-op. (Qt used the native color panel; pure
  egui cannot; this is the sanctioned replacement.)
- **D-UI-8**: "Reset Settings to Default" resets the 13 parity keys and
  `custom_font_path`, replicating Python's side-effect chain (borderless
  off, 100% size, flats, reattach, keytoggle cleared). It never touches
  overrides.json — taught chords are deleted only via "Manage Taught
  Chords...".
