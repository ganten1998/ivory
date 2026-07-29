# UNEXPLAINED — divergences no D-rule heuristic could tie to a documented fix

> **RESOLUTION NOTE (2026-07-29, after this audit ran).** The two regression
> concerns below were **fixed** in the engine (see DIVERGENCES.md **D21**): a
> note-complete reading now wins over a near-tie that drops a played tension,
> and the maj7#11 boost applies to a perfect full inversion. Corpus mismatch
> vs Python dropped 5195→5057; `AUDIT-aug-symmetry` went 15→1 and the maj7#11
> note-drop rows reclassified as fixes. The one residual note-drop is the
> reviewer's **non-blocking** slash-simplification case (e.g. Bb-C-E-F# with a
> non-root bass → `Gb7/Bb`, the slash step drops the #11) — left as a known
> limitation because touching the parity-sensitive slash logic risks the
> acceptance contract, and Python is also wrong there. The counts below
> describe the PRE-fix state that this file was written against.

The classifier (`classify.py`) maps 4,830 of the 5,195 engine↔Python
divergences to a documented D/K-rule. The remaining **365 rows** are tagged
`AUDIT-*`: rust fits *worse* than Python by raw pitch-class count and no
heuristic ties the row to a rule. This file hand-audits those 365 and ranks
the handful that look like genuine regressions.

## Method

For every mismatch, both names are reconstructed to a pitch-class set (name →
intervals table in `classify.py`) and scored against the sounding notes:

    error = (chord tones the name asserts but that are NOT sounded)
          + (sounded notes the name does NOT explain)

`error(rust) < error(python)` ⇒ rust is a tighter description (2,149 rows).
`>` ⇒ rust is looser (1,218 rows) — the pool the `AUDIT-*` tags come from.

**Caveat that governs the whole audit:** a raw-fit penalty is *not* proof of a
defect. Jazz shorthand legitimately omits unstated extensions — `Cm13` for
C-Eb-A-Bb is correct even though the metric counts G, D, F as "missing". The
live engine confirms these score highly and correctly (e.g. `F13` at 199 for
D-F-G-A-C-Eb names the D as the 13; `E11` at 192 for A-B-D-E-G# names the A as
the 11). So the audit ignores fit deltas caused by omitted extensions and
hunts only for the real failure mode: **rust leaving a sounded chord tone
unexplained when a complete tertian name exists and Python supplied it.**

Verdicts were cross-checked against the live engine with
`detect_chord_debug` (candidate scores), not just the reconstruction.

## Residual families (365 rows)

| family | rows | verdict |
|---|---|---|
| `AUDIT-ambiguous-dense` (≥6 unique PCs) | 210 | defensible — dense random piles, no unique correct name; rust and Python are two readings of the same altered/polychordal cluster |
| `AUDIT-ambiguous-voicing` (5 PC / other) | 81 | 35 are rust naming a higher extension (`X11`/`X13`, rust *better*); rest are two valid readings of an incomplete voicing |
| `AUDIT-rootless-shell-voicing` | 36 | defensible — rootless / shell inputs are ambiguous by construction |
| `AUDIT-aug-symmetry` | 15 | **CONCERN #1** — rust drops the #5 |
| `AUDIT-chromatic-cluster` (≤4 PC, ≤4 semitone span) | 23 | junk input (semitone clusters); neither engine has a real answer |
| `AUDIT-maj7#11 note-drop`* | 13 | **CONCERN #2** — rust drops the #11 (tagged `AUDIT-ambiguous-voicing`; isolated below) |

\* not a separate tag — these 13 live inside `AUDIT-ambiguous-voicing`; the
classifier comment and this file isolate them.

Of the 365, exactly **28 rows** (the two CONCERN families) show rust dropping
a genuinely-sounded chord tone. The other 337 are defensible alternates,
better shorthand, or unnameable clusters.

---

## CONCERN #1 — `major7#5` drop-2 / spread: rust drops the #5 (15 rows)

An augmented triad + major-7. The complete tertian name is `XΔ7#5`; rust
prefers a plain **major triad + slash** and leaves one note unexplained.

Live engine: Ab-C-E-B → `E/Ab` scores **152**, `CΔ7#5` **150**. The plain E
triad edges the complete chord by *2 points* and drops the C. Same shape at
all 12 roots (drop-2) plus 3 spread randoms.

| src | notes | Python (complete) | rust (drops a note) |
|---|---|---|---|
| `major7#5:drop2` | Ab-C-E-B | `CΔ7#5/Ab` | `E/Ab` |
| `major7#5:drop2` | A-Db-F-C | `DbΔ7#5/A` | `F/A` |
| `major7#5:drop2` | Bb-D-Gb-Db | `DΔ7#5/Bb` | `Gb/Bb` |
| `major7#5:drop2` | B-Eb-G-D | `EbΔ7#5/B` | `G/B` |
| `major7#5:drop2` | C-E-Ab-Eb | `EΔ7#5/C` | `Ab/C` |
| `major7#5:drop2` | Db-F-A-E | `FΔ7#5/Db` | `A/Db` |
| `major7#5:drop2` | D-Gb-Bb-F | `GbΔ7#5/D` | `Bb/D` |
| `major7#5:drop2` | Eb-G-B-Gb | `GΔ7#5/Eb` | `B/Eb` |
| `major7#5:drop2` | E-Ab-C-G | `AbΔ7#5/E` | `C/E` |
| `major7#5:drop2` | F-A-Db-Ab | `AΔ7#5/F` | `Db/F` |
| `major7#5:drop2` | Gb-Bb-D-A | `BbΔ7#5/Gb` | `D/Gb` |
| `major7#5:drop2` | G-B-Eb-Bb | `BΔ7#5/G` | `Eb/G` |
| `random` | Eb-Gb-B-G | `GΔ7#5/Eb` | `B/Eb` |
| `random` | D-Gb-Bb-F | `GbΔ7#5/D` | `Bb/D` |
| `random` | A-D-Gb-Bb-F | `GbΔ7#5/A` | `Bb/A` |

**Judgment:** minor regression. Python's `XΔ7#5` names all four notes; rust
leaves one (the note that completes the maj-7#5 stack) unexplained. Mitigating:
the augmented triad is symmetric so the "correct" root is itself somewhat
arbitrary, the margin is ~2 points, and it only surfaces in drop-2/spread
voicings (closed `major7#5` is named correctly — not in the mismatch set). No
D-rule authorises dropping the note. Worth an orchestrator look at the
triad-vs-tertian scoring tie-break for augmented-based chords.

## CONCERN #2 — `major7#11` drop-2: rust drops the #11 (13 rows)

A *complete* major-7♯11 (5th present). The #11 is a sounded chord tone; rust
names plain `XΔ7/bass` and drops it.

Live engine: B-C-E-F#-G → `CΔ7` scores **159**, and `CΔ7#11` is not even in
the top 5 (`Gb7(b9,#11)` is 2nd at 158). The Δ7♯11 demotion (the documented
purpose of the rewrite — `Δ7(#11)` appears 5,570× in the Python corpus)
overshoots here into a bare `CΔ7` that ignores the F#.

| src | notes | Python (complete) | rust (drops the #11) |
|---|---|---|---|
| `major7#11:drop2` | B-C-E-Gb-G | `CΔ7(#11)/B` | `CΔ7/B` |
| `major7#11:drop2` | C-Db-F-G-Ab | `DbΔ7(#11)/C` | `DbΔ7/C` |
| `major7#11:drop2` | Db-D-Gb-Ab-A | `DΔ7(#11)/Db` | `DΔ7/Db` |
| `major7#11:drop2` | D-Eb-G-A-Bb | `EbΔ7(#11)/D` | `EbΔ7/D` |
| `major7#11:drop2` | Eb-E-Ab-Bb-B | `EΔ7(#11)/Eb` | `EΔ7/Eb` |
| `major7#11:drop2` | E-F-A-B-C | `FΔ7(#11)/E` | `FΔ7/E` |
| `major7#11:drop2` | F-Gb-Bb-C-Db | `GbΔ7(#11)/F` | `GbΔ7/F` |
| `major7#11:drop2` | Gb-G-B-Db-D | `GΔ7(#11)/Gb` | `GΔ7/Gb` |
| `major7#11:drop2` | G-Ab-C-D-Eb | `AbΔ7(#11)/G` | `AbΔ7/G` |
| `major7#11:drop2` | Ab-A-Db-Eb-E | `AΔ7(#11)/Ab` | `AΔ7/Ab` |
| `major7#11:drop2` | A-Bb-D-E-F | `BbΔ7(#11)/A` | `BbΔ7/A` |
| `major7#11:drop2` | Bb-B-Eb-F-Gb | `BΔ7(#11)/Bb` | `BΔ7/Bb` |
| `random` | G-C-D-Eb-Ab | `AbΔ7(#11)/G` | `AbΔ7/G` |

**Judgment:** minor regression, same shape as #1 (rust drops a held chord tone
for a simpler name). Strongly mitigated: it is a boundary effect of the
*intended* `Δ7(#11)` demotion, and only in drop-2 voicings — closed
`major7#11` is still named `CΔ7(#11)` correctly. The right fix, if any, is a
"don't drop a sounded tension" guard, not reverting the demotion.

---

## Defensible families — spot audit (representative rows)

### `AUDIT-ambiguous-dense` (210) — dense altered/polychordal piles, ≥6 PC
No unique correct name exists; rust and Python pick different roots of the same
tritone/altered ambiguity. Worst-fit example is the top of the whole worse-list:

- `F7(b9,#11)/E` vs rust `Gb7(#9,#11)` — E-F#-A-B-C-Eb-Fb-F (7 PC). Both are
  "an altered dominant"; Python's slash reading happens to be exact, rust's
  tritone reading claims Bb/Db. Defensible — a 7-note random cluster with a
  full tritone has no canonical name.
- `B7(b9)/Ab` vs rust `B13`, `F9/D` vs rust `F13` — rust names the higher
  extension (13) that Python slashed off. Rust **better**, penalised only by
  the omitted-extension artifact.

### `AUDIT-ambiguous-voicing` (81, minus the 13 note-drops) — 5-PC / incomplete
- `E7/A` → `E11`, `F7/Bb` → `F11`, `Bb7/Eb` → `Bb11` (35 rows): rust names the
  perfect-4th bass as the 11th instead of slashing it. **Better**, not a defect.
- `D2/Ab` → `E11/Ab`: Ab-D-E-A. Dsus2-over-tritone vs E7add11 (Ab=E's 3rd,
  A=11th). Two valid readings; rust's is bass-coherent (Ab = the 3rd of E11).

### `AUDIT-rootless-shell-voicing` (36)
Inputs generated from `*_shell` / `rootless` patterns — deliberately missing
their root. `Gb/Db` vs `BbmΔ7/Db` for Db-Gb-A-Bb: both are 4-note guesses at a
rootless cluster; ambiguous by construction, no correct answer.

### `AUDIT-chromatic-cluster` (23)
`Ab-A-Bb` → `Bb7(#11)`, semitone triads, `C-Db-D` clusters. Junk MIDI input;
Python's `Ab2` and rust's `Bb7(#11)` are equally arbitrary. Not chords.

---

## Cross-rule confirmation audit — rust-correct / Python-buggy (sample)

Beyond the 60-row target, spanning every fix rule; each verified by
reconstruction + (for the tricky ones) live `detect_chord_debug`. These are
the *opposite* of regressions: Python is the buggy engine.

| rule | notes | Python (buggy) | rust (correct) | why rust is right |
|---|---|---|---|---|
| D1 | C-Eb-G-Bb | `Eb6` | `Cm7` | closed root-position m7 named from its own root, not the relative major 6 |
| D1 | A-C-E-G | `C6` | `Am7` | declared flip (acceptance #39): root-in-bass m7 |
| D3 | A-E-Bb-Eb-Ab | `EΔ7(#11)` | `AΔ7(#11)` | 7-PC tertian stack named from the bass A, not a 3rd above |
| D4 | G-B-F-Bb | `BΔ7(#11)` | `G7(#9)` | root-in-bass dominant (M3+m7 over G) → altered dom from bass |
| D5 | Gb-G-E-Bb | `Edim/Gb` | `Gb7(b9)` | root-position 7(b9)-no5, not a dim slash (the +380 gate) |
| D6 | C-Db-E-G-Bb | `C7b9` | `C7(b9)` | tension parenthesised per K3 |
| D8 | C-D-F-A-Bb | `C6/9` | `C13(sus)` | b7 present ⇒ 13sus wins over 6/9-no3 |
| D9 | Ab-E-Bb | `EΔ7(#11)/Ab` | `Gb9` | Bb-E tritone = rootless Gb dominant (3rd,b7,9) |
| D10 | E-G-Bb-D-C | `E7(#11)` | `C9/E` | complete C9, E in bass — the documented D10 vector |
| D12 | Db-C-Eb-A | `Adim/Db` | `Cm6/Db` | {0,3,9} spelled as the bass-coherent m6, not a rootless dim |
| D17 | 8-PC spread altered | `BbΔ7(#11)` | `None` | ≥8 unique PCs never name a chord (reduction lottery removed) |
| D17 | 8-PC diminished-scale | `A7(b9,#11)` | `Gb Locrian #6` | scale check on original notes |
| D19 | Ab-E-Gb-Eb | `EΔ7/Ab` | `EΔ9/Ab` | keeps the 9th instead of the dead inversion name |
| D20 | C-F-G-Bb | `Gm11` | `C7sus4` | root==bass m11 bonus gated; bass-coherent 7sus4 wins |
| D20 | C-D-G-A | `Am11/C` | `C6/9` | idiomatic 6/9 from the bass, not a rootless m11 |

## Bottom line

Of 5,195 divergences: 2,149 rust strictly tighter, 952 equal, 876
non-comparable (scale/None edges), 1,218 looser — and of those 1,218, all but
**28 rows** (CONCERN #1 + #2) are legitimate shorthand, defensible altered/
tritone re-readings, or unnameable clusters. The 28 are two variants of one
narrow behavior (drop-2 voicings of `Δ7#5` / `Δ7#11` where a simpler name edges
out the complete one and drops a sounded tension), both downstream of the
documented and intended `Δ7(#11)` demotion. No broad-scoring change (D4 shell
penalty, the 13-needs-its-13th skip, dominant9 inversion, D5 380-gate, D20 m11
bonus) was found to mislabel a clean, complete, common chord that Python got
right.
