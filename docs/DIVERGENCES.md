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
D-rule ID (covering both naming preferences). **What actually enforces this
today** (corrected 2026-08-11 — there is no CI yet; see `docs/RELEASE.md` § CI):
`ivory-core/tests/differential.rs` pins the engine to
`tests/golden/rust-golden.json`, the frozen post-audit baseline, so any
behavior change turns it red and forces a deliberate regeneration.
`classified-divergences.json` is a point-in-time audit artifact, **not** an
assertion — no test reads it, and it is re-derived by hand with
`tests/golden/classify.py`. It was last generated **2026-07-29 (5,057 rows)**
and is stale: after D22–D26 the live mismatch is **5,540 rows** (`cargo run -p
ivory-core --example diffcorpus --release`). Re-run `classify.py` before
trusting any per-rule breakdown.

This policy was adversarially reviewed by a three-lens critic panel
(music-theory lens executed the Python to verify claims); all decisions below
are post-review.

## Kept as-is (identity, not bugs)

- K1 (B1): 2-note intervals render `"C (M3)"` (root-prefixed), `"{n}
  semitones"` beyond 21.
- K2 (B2): half-diminished rendered `m7b5`, never `ø7`. **SUPERSEDED by D27
  (2026-08-11)** — it now renders `ø7`. Kept here because the K-numbering is
  referenced throughout the review history; do not reuse the id.
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
- **D16** — **withdrawn.** The draft rule regressed acceptance vectors #44–#47;
  the shipped behavior it would have changed is kept as-is under K10. The ID is
  retired, never reused: the range D1–D26 therefore contains 25 rules.
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

- **D22** (owner report 2026-08-10): a scale played root-to-root includes the
  octave, so scale detection is no longer killed at span == 12. The late scale
  check now fires when the voicing is stepwise (`is_clustered`) OR within an
  octave, instead of a hard `span < 12` gate. A stepwise run that adds its top
  octave (or spans past it) keeps its scale reading — C-D-E-F-G-A-B-**C** →
  `C Ionian`, not `CΔ13`; likewise the natural minor, both pentatonics, and both
  blues scales. A spread tertian stack is NOT clustered, so a real `CΔ13` (v067)
  or `C6/9` (v133) still names as a chord. Resolves the old TODO(classifier)
  entry for §13 vector #140. (446 flat+sharp corpus rows move chord→scale.)
- **D23** (owner report 2026-08-10): the +50 #11-shell identity bonus
  (`7#11_shell`, `7#11_no3`, `7#9#11_shell`, `7b9#11_shell`, `7b9#11_no3`) is
  gated on the defining tritone (interval 6) actually sounding. Without it a
  bare {0,2,4} matched `7#11_shell` from a third above and beat the true reading
  — C-D-E → `D7(#11)` — so it now reads `C(add9)`. The gate is lifted inside the
  slash-reduction helper (`detect_chord_simple`, `simplify_pass`), so slash
  upper structures — augmented/whole-tone tetrads like `C7(#11)/Gb`,
  `B7(#11)/F` — name exactly as before (no note-dropping churn). (372 top-level
  corpus rows corrected; slash sub-structures unchanged.)
- **D24** (owner report 2026-08-10): a MAJOR add9 with its 3rd in the bass names
  as sus2/bass, rooted on the add9's OWN root (dropping the 3rd-in-bass leaves
  root-2-5). C-E-G-D over E → `C2/E`; the owner's C-Ab-Bb-Eb → `Ab2/C`. Emitted
  directly in the slash step so the sus2 keeps the chord root instead of
  re-rooting to the lowest upper voice (which read `G4/E`). A minor add9's 3rd
  is a minor third up (interval 3), so it never trips this and stays
  `Xm(add9)/bass`. Flips former "parity" rows v109/v122/v123 (same voicing shape
  in other keys). (36 corpus rows.)
- **D25** (owner report 2026-08-10): a bass-rooted maj7 shell that also carries
  the 6/13 — root + M3 + M7 + M6 in the bass, nothing foreign — is a genuine
  XΔ13, preferred over reading the 13th as the root of a rootless minor(add9).
  B-D#-G#-A# (from B: {0,4,9,11}) → `BΔ13`, not `G#m(add9)/B`. Gated on
  root-in-bass + all three characteristic tones (4, 11, 9) + extra_count == 0,
  so it cannot crown an incomplete maj13 over an unrelated chord. Flips former
  "parity" row v114 (same shape in Eb). (52 corpus rows.)

- **D26** (owner report 2026-08-10): a scale must account for EVERY sounded
  pitch class. `detect_scale` previously matched a pattern as a `is_subset` of
  the played notes, so every superset inherited the smaller scale's name — a
  6-tone `C-D-E-F-G-A` read as the 5-tone `C Major Pentatonic`. It now requires
  an EXACT pitch-class match (`intervals.len() == pat_set.len()`), so a set with
  more unique PCs than the pattern has tones is never that scale. `C-D-E-F-G-A`
  → `Dm11`, `C-D-Eb-F-G-Bb` → `Cm11`; genuine 5-tone pentatonics and exact
  6-tone scales (whole-tone, blues) are unchanged. Consequences: the 12-PC
  chromatic set is `Chromatic Scale` (not the 8-tone `C Whole-Half Diminished`
  it contained — old v-row flipped), and an 8-PC "altered scale + natural 5th"
  is `None` (not `C Altered`; old v098 flipped) per D17. 905 corpus note-sets
  move subset-scale → chord/None/Chromatic; a classifier confirmed 0 touched a
  chord reading and 0 exact-match scale was lost.

- **D27** (owner preference 2026-08-11, presentation only): chord symbols use
  standard jazz lead-sheet glyphs — `°` diminished, `ø` half-diminished, `+`
  augmented. `Cdim`→`C°`, `Cdim7`→`C°7`, `CdimΔ7`→`C°Δ7`, `Cm7b5`→`Cø7`,
  `Cm7b5(11)`→`Cø7(11)`, `Caug`→`C+`. **Supersedes K2.** Detection is
  untouched: a classifier over the whole golden corpus confirmed 1,014 rows
  changed symbol and **0 changed identity**. NOTE the engine makes decisions by
  re-parsing rendered names, so the change had to land in lockstep in three
  places — `format_chord_name`, `match_chord_type`'s quality map, and the
  name-based `is_dominant`/slash-simplify guards, which excluded `"dim7"` and
  `"m7"`; `°7`/`ø7` contain neither and would otherwise have been scored as
  dominants. Pre-D27 spellings are still accepted by `match_chord_type` so a
  chord taught under an older build still resolves.

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
- **D-UI-4** (corrected 2026-08-11 — the previous wording described a feature
  that was never built): there is **no liveness indicator and no
  auto-reconnect**. `Select MIDI Input...` calls `midi::list_port_names()` at
  open time (`app.rs`), so the list is always a fresh enumeration of the ports
  that exist right now; the dialog prints `Current: <port name>` taken from the
  live `MidiConnection` — the name it was opened with, never a status — and
  pre-selects nothing. If the port dies, events simply stop arriving
  (`ivory/src/midi.rs`: "No reconnect logic (parity): if the port dies, events
  just stop") and the app keeps drawing its last state until the user re-picks.
  Picking a port drops the old connection before opening the new one, and a
  failed open raises the `MIDI Error` dialog.
- **D-UI-5**: teach-layer additions, precisely scoped: two context-menu
  items inside the chord-detection block, immediately after the
  Detach/Attach entry, preceded by their own separator — `Teach Chord
  Name...` (greyed when no notes held) and `Manage Taught Chords...`. All
  other menu items keep byte-identical order. Additive settings key
  `custom_font_path` in settings.json; `learning_mode` lives in
  overrides.json (Python rewrites settings.json with a fixed key set, so
  additive settings keys are lost on a Python downgrade — tolerable for
  the font path, not for learning state).
- **D-UI-9** (2.1.0): the learned re-ranker becomes a user-facing option.
  Two further context-menu items directly after the D-UI-5 pair, preceded
  by their own separator: `Correct Chord Name...` (greyed when no notes
  held, like the teach item) and `Enable/Disable Chord Learning` (a
  self-renaming toggle — Qt parity, no checkmarks). Forgetting lives in
  `Manage Taught Chords...`, which grows a footer showing learning state,
  correction count, the non-zero learned weights, and a `Forget Learning`
  button. The correction dialog offers **only the scored candidates**
  (`ChordDetector::trainable_candidates`), because the re-ranker can only
  reorder those — the displayed label is frequently a post-scoring rename
  (slash bass, rootless dominant) that was never a candidate and can never
  be trained toward. Every attempt reports its outcome
  (`TrainOutcome::{Learned, Stubborn, AlreadyCorrect, NotTrainable}`); a
  correction that cannot be made is rolled back rather than left as a
  partial nudge. Learning state stays in overrides.json (never
  settings.json). The `learning` cargo feature is now always enabled by the
  `ivory` GUI crate, so every packaged binary ships the option; the engine
  crate keeps the feature gate so `cargo test -p ivory-core` still
  exercises the stock path.
- **D-UI-6**: About dialog shows "Version &lt;crate version&gt;" (read from
  `CARGO_PKG_VERSION` since 2.1.0 — it was hardcoded "2.0.0") and adds one 8pt
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

### 2.3.0 — tester report, 2026-08-12 (Windows)

Five deliberate breaks with the Python-era UI spec. All five come from one
tester session on 2.2.0; the report and its screenshots are the rationale.

- **D-UI-10**: the detached chord window is **no longer slaved to the piano's
  width**. `ui-spec.md` §5.7 (§168, §171) specified an initial width equal to
  the main window's plus a 100 ms debounced re-sync on every size change, and
  `chord_strip::sync_width` implemented exactly that. Both are gone. A chord
  readout has no reason to inherit the keyboard's 8.6667:1 proportions, and the
  re-assert made D-UI-11 impossible: any restored width was overwritten within
  100 ms of the next size change. New windows open at
  `settings::DETACHED_DEFAULT` (460x150).
- **D-UI-11**: window geometry is remembered. New optional settings keys
  `window_x`/`window_y`, `detached_chord_x`/`detached_chord_y` and
  `detached_chord_width`, written back on a 700 ms debounce so a drag is one
  file write rather than one per frame, and absent from the file until
  something is actually placed. `detached_chord_width` doubles as the marker
  for the new geometry model: while it is absent the stored
  `detached_chord_height` is **ignored**, because pre-2.3 builds overwrote that
  key with the attached strip's height on every detach, so an inherited 50 is
  not a size any user chose. A restored position is clamped to the monitor, and
  a window restored entirely off-screen is recentred on the first frame that
  knows the monitor size.
  **Tiling window managers are detected and excluded.** Under AeroSpace, yabai
  or i3 the size we ask for is simply overruled, and recording the result as a
  user preference is worse than recording nothing, because the tiled geometry
  then follows the user into sessions where nothing is tiling. That is exactly
  how the 1377 above got written in the first place. Size alone cannot tell a
  WM apart from a person dragging an edge, so timing does it: a mismatch within
  `WM_GRACE` (600 ms) of the window appearing is the WM's, and that detachment
  is not recorded; later mismatches are real resizes. The main window is not
  user-resizable at all, so any settled disagreement with its target size means
  something else is placing it and its position is left alone. Verified against
  a real AeroSpace session, which tiled the detached window to 853x1377 and
  wrote nothing.
- **D-UI-12**: child windows open centred on the main window instead of
  wherever the OS puts them, which on Windows is the top-left of the screen no
  matter where the user has dragged the piano. `dialogs::Placement` carries the
  main window's rect in, because a viewport can only see its own geometry.
- **D-UI-13**: the chord label may use **two lines**. Spec §5 specified one
  line at `max(12, int(0.6*h))` with a single-pass shrink to 95% width, so a
  long name like `Eb Minor Pentatonic` rendered a fraction of the size of
  `EbM4` in the same window. A two-line layout now competes with the shrunken
  single line and the larger glyphs win. The search starts at the shrunken
  size, so the result is **never smaller** than the old behaviour; that is a
  property test, not an observation. Labels with no whitespace never wrap,
  because egui would break them mid-token.
- **D-UI-14**: "Apply in all keys" in Teach Chord Name starts checked, and the
  last choice is remembered in `teach_apply_all_keys`. Naming a voicing
  usually means naming the shape rather than that one key.

Also from that report and **not** done: keyboard shortcuts, which the report
asks for without saying for what.

### D-UI-15 — the guitar view

A third band under the piano showing the same notes on a fretboard. The Python
app has nothing like it; this is an addition, not a divergence from parity, and
it is scoped so that everything above it is untouched.

- **Off by default.** `show_fretboard` starts `false`. Turning it on makes the
  window taller, and a window that grows on its own after an update is exactly
  the geometry surprise D-UI-10/11 came out of. It is one line in
  `Settings::default()` to change that mind once the view has been played with.
- **Menu**: `Show Fretboard` / `Hide Fretboard` in its own block before About,
  renaming itself like every other toggle here. `Tuning` and `Capo` submenus
  appear only while it is on — a Tuning row for a hidden fretboard is a control
  for something the user cannot see. Both mark the current choice with a bullet
  rather than hiding it, because a submenu that never says what is selected
  makes you open it again to find out.
- **Submenus are now plural.** `Size` used to be the only one and was hard-coded
  from the entry list down to the viewport. `Entry::Submenu { label, items }`
  replaces `Entry::SizeParent`, `MenuState` carries a `SubGeom` per submenu, and
  one viewport id is shared because only one can be open at a time. Size comes
  out of the generalised path byte-for-byte the same, which
  `size_is_still_the_first_submenu_and_still_lists_the_same_percents` asserts.
- **Settings keys** (additive): `show_fretboard`, `fretboard_tuning`,
  `fretboard_capo`. All three are sanitised at USE, not at load: an unknown
  tuning name draws as Standard but is written back untouched, and a capo of 40
  is clamped to something playable. A settings file can therefore travel
  between builds without either one eating what it did not understand.
- **Layout**: the band is `132 * width / 1300`, truncated, stacked below the
  piano. `initial_window_size` and `layout_sizes` were two copies of the same
  arithmetic and are now one function, `band_sizes` — drift between them is a
  window that visibly jumps on the first frame.
- **The view is dumb.** Every choice (which of a pitch's five positions, what is
  a barre, what folded an octave, what could not fit) belongs to
  `ivory_core::voicing`; `fretboard_panel.rs` only draws the answer. The app
  owns exactly one `VoicingSession`, so no two surfaces can disagree about the
  shape, and it is re-solved on the same 100ms gate as chord detection rather
  than per frame.
- **Independent of chord detection.** The guitar view is a second instrument,
  not a decoration on the chord strip: it works with detection off, and hiding
  one band does not resize the other.
- **Four pictures that are not a dot**, because the failure that makes a panel
  like this untrustworthy is silently drawing five of the six notes someone
  played: a hollow ring behind the nut is an open string, a hollow dot with an
  arrow is a note outside the instrument's range shown an octave away, a faint
  ring on the board is a note the guitar can make but not at the same time as
  the others, and an `×` behind the nut is a string to damp. Anything genuinely
  not shown is counted in the caption instead. Mute marks are suppressed
  entirely while nothing sounds — six crosses telling nobody to mute nothing is
  noise on the view the app sits at all day.
- **Colour**: the neck is a dark fingerboard with light strings, and held notes
  use `white_key_active_color`, the colour the user already chose for a held
  key on the piano. The first attempt drew dark strings on the piano's own
  light background and came out looking like a spreadsheet.

Not done, and deliberately: the fretboard is read-only (keytoggle stays a piano
gesture), there are no fret-position numbers (the inlays carry it), and the
detached chord window does not gain a fretboard.

### D-UI-16 — three woods, and the neck in its own window

- **Three fingerboard woods**, in a `Wood` submenu that appears with the rest of
  the guitar block: **Rosewood** (default), **Maple**, **Ebony**. Stored as
  `fretboard_wood`, sanitised at use like `font_choice`.
- **Each wood carries its whole palette, not a fill colour.** Maple is pale, so
  on it the strings, fret wires, inlay dots and nut are all DARK, and note dots
  get a dark edge ring — the light strings that read well on rosewood vanish on
  blonde wood, and an accent-coloured dot on it becomes a smudge. Rosewood and
  ebony keep light strings on a dark board.
- **The wood does not follow dark mode.** A neck is made of what it is made of;
  swapping maple for ebony when the lights go down would be a different
  instrument. Only the band around it and the gutter marks follow the theme.
- **The neck pops out** into its own window: `Detach Fretboard` /
  `Attach Fretboard`, mirroring the chord window's pair exactly — close to
  reattach, right-click anywhere for the menu, drag anywhere when borderless —
  so there is one set of habits rather than two. While it is out, the attached
  band disappears and the main window shrinks by exactly its height.
- Geometry is remembered in `fretboard_win_w/h/x/y`, absent until the window
  has been placed, and it carries the same tiling-WM guard as D-UI-11: a size
  the window manager imposed inside `WM_GRACE` is not recorded as a preference.
  Default size is 880x190, which is a neck's proportions rather than the
  attached band's, for the same reason the detached chord window is not the
  piano's.
- `Hide Fretboard` closes the popout too, rather than leaving a window on
  screen whose menu entry has gone. The detached state is remembered, so
  showing it again puts it back where it was.

### D-UI-16a — play-test corrections

From the first hands-on session with the guitar view:

- **The caption no longer moves the neck.** It used to take 19% of the band, so
  the board shrank whenever there was something to say and grew back when there
  was not. A note going out of range mid-phrase resized the fretboard under the
  player's hands. The caption is drawn OVER the board now, in the bottom-right
  corner past the last inlay, and the geometry is constant.
- **Playability is out of the caption.** "two hands" and "stretch" told the
  player something the shape already shows, and being the most frequent line
  they were also the main cause of the resizing. `Shape::playability` stays on
  the struct for a view that wants to desaturate an unplayable shape.
- **Dots are smaller** (0.30 of the string spacing, was 0.38) and now have a
  ceiling as well as a floor. The flat 2pt minimum made small windows worse
  rather than better: at 3pt of string spacing it produced a 4pt dot that
  overlapped its neighbours, which the popped-out window can reach at its
  minimum height.

### D-UI-18 — the fretboard is an input, not just a readout

Keytoggle now works on the neck as well as the keyboard. Click a fret and that
note toggles, so a guitar voicing can be entered by shape and read off the
piano with its name, instead of only the other way round. Both instruments
toggle the same set, so it is symmetric.

Two things this needed that were not obvious:

- **`fretboard_panel::hit_test` is the exact inverse of `draw`**, the same rule
  `piano::hit_test` follows (spec §4.5), so a dot can never light somewhere it
  cannot be clicked. With no headstock there is nowhere left of the nut to
  click, so the first `2 * dot_r` of the board is the open-string zone, which is
  where the open rings are drawn. A click also has to land NEAR a string:
  halfway between two is a miss, not a guess, or every near-miss silently adds
  a note.
- **A clicked position is PINNED** (`voicing::History::pins`). Left to choose,
  the solver redraws a hand-entered shape somewhere else about three times in
  four, measured: 25% to 58% of shapes survived depending on size. Correct when
  it is choosing, useless when the choosing is done.

  Narrowing the candidate list was not enough. When every held note is pinned
  the search is **bypassed entirely**, because the monotone constraint is not
  optional inside it and ordinary guitar shapes violate it: low E at fret 6
  sounds Bb2 (46) against an open A (45), a higher pitch on a lower string,
  entirely playable and structurally undrawable by the search, which simply
  dropped one of the two. Pins for a partial set still go through the search.

  A pin that does not name a real position for its pitch is ignored rather than
  obeyed, and pins are dropped on any tuning or capo change, so a stale one can
  never move a note somewhere it cannot sound.

### D-UI-19 — keyboard shortcuts, and one finger per string

The 2.2.0 tester report asked for keyboard shortcuts without saying for what,
which is why it was not done then. Now there is something to bind them to.

`F1` shows a card listing every shortcut; `K` keytoggle, `R` clears the notes
you placed, `G` guitar view, `D` dark mode, `C` chord detection, `Esc` closes
the card.

- **One table, in `keys.rs`, and the card is rendered FROM it.** A shortcut that
  works but is not listed, or is listed and does not work, is worse than no
  shortcut; one source is the only way to guarantee neither. Every action routes
  through the same `apply_menu_action` the menu rows use, so a key and a menu
  item cannot drift apart in behaviour.
- **A modifier suppresses every shortcut.** Cmd-R and Ctrl-R belong to the OS
  and to muscle memory; swallowing them would be rude. Asserted by test.
- **Shortcuts are dead while a dialog or the context menu is open**, both being
  modal. A stray `K` changing the app behind a modal reads as a haunting.
- `R` clears what the USER placed, not everything. Notes arriving from a MIDI
  keyboard are not ours to drop, and would return on the next frame anyway.
- **The card is drawn in the CANVAS**, not in a child window: painted directly
  rather than via `egui::Window`, so it cannot be dragged off or resized into
  nothing. It is also the first surface in this app that already works in a
  VST3 editor, where no child viewport can exist, and it is the shape the rest
  will move to (docs/PLUGIN-PLAN.md).

**One finger per string.** Clicking a fret on a string that already holds a
placed note MOVES that note rather than adding a second, because a string can
only sound once. This is not only physical honesty: pinning is all-or-nothing,
so a single impossible note sent every other note back to the solver to be
rearranged, and the board started reporting "4 of 5 notes" with conflict rings
after what looked like an ordinary click.

### D-UI-17 — the white keys tile

The piano drew each white key `trunc(width / 52)` wide while stepping by the
untruncated pitch, so wherever the fractional part rolled over the key was a
pixel short and the background showed through: **twelve grey slivers** across a
1625pt keyboard, irregularly spaced. Spec §4 calls the sub-pixel slivers part of
the Qt look and they were preserved deliberately, but they are only invisible in
dark mode, where the background happens to equal the white-key colour. In light
mode they are twelve grey gaps.

Each key now runs to where the next one starts. Both forms are integer-aligned
and the separator lines land on exactly the same pixels, so nothing else about
the keyboard changes. `white_keys_tile_with_no_gaps_at_any_window_size` checks
every size preset.
