# DIVERGENCES — where Ivory 2.0 intentionally differs from the Python engine

Policy: the rewrite reproduces the Python engine's *shipped* behavior
exactly, except where that behavior is a documented bug (the B-catalog from
the algorithm spec, cross-checked against `01_Special_Cases_and_Resolutions.md`)
or an indefensible musical error. Every divergence below carries:
an ID, the fix, the rationale, and a **matcher** used by the differential
harness to classify golden-corpus mismatches. A corpus mismatch that no
matcher claims is a build failure.

Formatting/style items that tests or docs disagree with but that the shipped
app displayed are **kept** (they are the app's identity, not bugs).

## Kept as-is (identity, not bugs)

- K1 (B1): 2-note intervals render `"C (M3)"` (root-prefixed), `"{n}
  semitones"` beyond 21.
- K2 (B2): half-diminished renders `m7b5`, never `ø7`.
- K3 (B6/B10): parenthesized, comma-joined tensions: `CΔ7(#11)`,
  `G7(#9,b13)`. The stale test expectations without parens/commas lose.
- K4: `Δ` glyph for major-7 family, `C(add9)`, `C6/9`, `/Bass` slash format.
- K5: sus-vs-add9 span rule (compact → add9-reading, spread → sus-reading)
  as a *mechanism* — specific wrong outcomes fixed below (D7, D8).
- K6: scale-vs-chord span rule (within an octave prefer scale on ≥5 PCs).
- K7 (B20): Whole Tone requires ≥6 PCs.
- K8 (B16): "D E G A C → C Major Pentatonic" — the doc's `D Minor
  Pentatonic` claim is musically wrong; code was right.
- K9: auto-connect priority chain incl. the "Scarlett" preference; sustain
  semantics; channels ignored. (UI parity, not engine.)

## Engine fixes

- **D1** (B3): closed-voicing complete m7 is NOT reinterpreted as relative
  major 6. `C-Eb-G-Bb` (any voicing) → `Cm7`. 6-vs-m7 ambiguity resolves by
  bass: bass=6-root → 6 chord (`C-E-G-A` → `C6`), bass=m7-root → m7
  (`A-C-E-G` → `Am7`); other basses → inversion of the bass-coherent
  reading with slash. Matcher: Python said `X6`/`X6/y` where fixed says
  `Ym7`-family with Y = relative minor of X (or vice versa).
- **D2** (B4/B25): the root-mutation corruption is gone (pure scoring, no
  shared mutable root). Fixes `C-F-G-Bb` → `C7sus4` (was `Bbm11`),
  `C-D-G-Bb` → `C7sus2` (was `Gm/C`).
- **D3** (B5): 7-PC tertian stacks name from the bass root when the bass
  holds a coherent 13-family reading: CΔ13 set → `CΔ13`, Cm13 set → `Cm13`,
  CΔ11 set → `CΔ11`. Matcher: Python renamed to a Δ13#11/Δ9(#11) from
  another root.
- **D4** (B7 + doc §5): full altered dominants **with natural 5th present**
  name from the dominant root: `C-E-G-Bb-Eb` → `C7(#9)`, `C-E-G-Bb-F#` →
  `C7(#11)`, `C-E-G-Bb-Ab` → `C7(b13)`, `C-E-G-Bb-Eb-F#` → `C7(#9,#11)`.
- **D5** (B8): `C-E-Bb-Db` (4 notes) → `C7(b9)` per doc §5, not `Bbdim/C`.
  The early dim7-upper-structure path only fires when it does not displace a
  root-position dominant reading.
- **D6** (B9): the early-path bare `7b9` gains parens: always `C7(b9)`.
  Matcher: exact-string `X7b9` → `X7(b9)`.
- **D7** (B13 + doc §3, resolving doc §1's self-contradiction in §3's
  favor): compact root-position 9sus wins over the b7-add9 slash reading
  when the bass is the sus root: `C-D-F-Bb` → `C9(sus)`, and (B11)
  `C-Bb-D-F-G` → `C9(sus)` (9sus_with5), not `Gm11`/`Bb(add9)/C`.
- **D8** (B18): compact 13sus wins over the 6/9-no3 reading when the b7 is
  present and bass is the sus root: `C-D-F-A-Bb` → `C13(sus)`.
  `C-D-F-A` (no 7th) stays `C6/9` (K5 span logic; sus13-no-7 is the rarer
  reading).
- **D9** (B14 + doc §7): rootless dominant voicings resolve to the implied
  root via the tritone: `E-Bb-D` → `C9`, `E-Bb-D-F#` → `C7(#11)`,
  `F#-Bb-D` → `C7(#11)` (not `Gbaug`). (The old Rust core already
  implemented this; kept and extended.)
- **D10** (B15 + doc §2): `E-G-Bb-D-C` (E bass, C on top) → `C9/E`.
- **D11** (B17): `C-G-A-D` (C bass) → `C6/9`, not `Am11/C`.
- **D12** (B19): `C-Eb-A` → `Adim/C` (a true diminished triad over its
  third), never `Cdim` (C-Eb-A is not a C diminished triad).
- **D13** (B26): >7-note reduction is deterministic: rank PCs by sounding
  count desc, then bass PC first, then ascending PC; keep 7. Documented,
  test-pinned. Matcher: any >7-note corpus row (Python's own answer was
  iteration-order lottery).
- **D14** (B22/B23 + tables audit): duplicate/shadowed patterns removed or
  made reachable: `7sus13` (exact duplicate of `13sus`) deleted;
  `augmented7` gets a real display branch `C7(#5)` and fires only when it
  genuinely beats `7b13_no5` (bass-root aug voicings); the never-reachable
  generic-formatter output `Caugmented7` can no longer occur.
- **D15** (B21): the 7-PC early scale fallback matches the scale *root
  exactly* (pitch-class compare), not `startswith` (which confused A with
  Ab/A#).
- **D16** (B24/E3): half-diminished scoring path is live again (E3
  short-circuit removed in favor of scoring with the ø-vs-m6 disambiguation
  preserved as a bonus rule). Rendered name remains `m7b5` (K2).
- **D17** (edge): all-12-PC input (e.g. every key held) → `Chromatic Scale`
  (new 12-tone entry in the scale table) instead of a nonsense chord like
  `Eb7(#11)`. ≥8 unique PCs never name a chord; scale table or `None`.
- **D18** (tables audit): the `altered` pattern keeps Python's interval set
  (including the natural 5 as *optional*) so a played 5th isn't punished as
  an extra — but D4 governs which root wins. Resolved against the old Rust
  core's silent P5 drop.
- **D19** (B27 hygiene): dead code not ported: `INVERSION_NAMES`,
  `PREFER_FLATS` global, ignored `lowest_note` param. Public API takes the
  full note set; bass derived internally.

## UI fixes (parity exceptions)

- **D-UI-1**: `detached_chord_height` persisted value is honored (Python
  overwrote it with 50 on init).
- **D-UI-2**: single-instance via lock file — same dialog + exit UX, but a
  crashed instance never blocks relaunch (QSharedMemory stale-segment bug).
- **D-UI-3**: no busy-loop rendering; repaint scheduled on MIDI/timer
  events at the Python cadences.
- **D-UI-4**: MIDI picker shows a dead current connection as
  "(disconnected)"; no auto-reconnect (parity).
- **D-UI-5**: additive menu items for the teach layer (`Teach Chord
  Name...`, `Manage Taught Chords...`); additive settings keys
  (`custom_font_path`, `learning_mode`). Everything else pixel/behavior
  parity.

## Open items to resolve during implementation (with tests either way)

- O1: exact tie-break constants for D1's bass-coherent 6-vs-m7 rule so no
  *other* corpus rows flip unintentionally.
- O2: D7/D8 must not regress the spread-voicing behaviors that already
  worked (`C9(sus)` spread, `C13(sus)` spread, `C(add9)/D` compact).
- O3: whether `13sus`-family display uses `C13(sus)` (shipped) — confirm
  against the reference app's rendering, keep shipped form.
- O4: D12 alternative reading `Cm6(no5)` — rejected for now (slash of a real
  triad beats a fabricated no-5 m6), revisit if the critic panel objects.
