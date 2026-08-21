# Ivory 2.0 — Handoff / Resume Document

**Last updated:** 2026-08-20. **The app is now called TANGENT.** Newest work is
§2u: the DESK — the effects as a bus, the limiter on the master, and a mixer
view behind Tab — plus the camera preview, two DX7 reports and the plugin rack
that was listing effects it could not load. Before it,
§2t: the submenu that opened the wrong row, two Windows-only white flashes, the
channel chooser as tick boxes, and one tick that was wired to itself. Before it,
§2s: the potato pass. Before it, §2r: the takes with no instrument in them.
Before it, §2o: the fullscreen freeze, diagnosed over ssh on the Linux box. Before it,
§2n: the clip lamp and the take report. Before it,
§2m: Linux hardening. Before it, §2l: the backing-track player. Before it, §2k: the limiter's makeup gain, the
DX7 fader bug, and a dismissable CLIPPED.
Before it, §2j: one gesture set across all eight knobs.
Before it, §2i (the master column) and §2h (six effect knobs, the true-peak
limiter, video without a camera).
Before it, §2d: the fretboard voicing solver and the guitar view. §2c before it has the
rename, the 2.2.0 tester-report UI fixes, the egui 0.33 downgrade and the
MIT/GPLv3 split. Read §2c then §2d FIRST; everything above them still says
"Ivory" and that is now the internal codename, not the product.
This file is the single source of truth for
picking the project up cold. Read it top-to-bottom before touching anything.
Pair it with `docs/DESIGN.md` (architecture), `docs/DIVERGENCES.md` (the chord
policy) and `docs/spec/` (the machine-verified extraction of the Python app).

---

## 1. What this is

A ground-up Rust rewrite of **Ivory** — a MIDI keyboard monitor that draws an
88-key piano and names the chord you're playing in real time. It replaces a
Python/PySide6 app (v1.1.0) that lives at `~/Dropbox/Archive/Ivory`. Goals, in
priority order (from the owner's brief):

1. **UI/UX parity** with the last working Python mac build — same look/feel.
2. **Solidify & perfect the chord logic and all edge cases** — reproduce the
   Python engine's *intended* behavior with every documented bug fixed.
3. **Teachable chord naming** — users can override names for voicings, plus an
   optional experiment where the app learns preferred names.
4. **Sellable** under "pay what you can (incl. $0)" — clean licensing,
   packaged for macOS + Linux + Windows.
5. **Clean and lean** — two crates, minimal deps. Retire the old `ivory-rust`.

**Repo:** `~/Dropbox/Projects/Apps/ivory` (git, branch `main`; `origin` =
`git@codeberg.org:ganten1998/ivory.git`, pushed 2026-07-29 — see §7). The old
abandoned attempt `~/Dropbox/Projects/Apps/ivory-rust` was mined for its engine
and has since been **trashed** (§7; do NOT push it anywhere).

---

## 2. Status at 2026-07-29 (historical — see §2a and §2b for what came after)

### Done and committed
| Commit | What |
|---|---|
| `cf71ba6` | Scaffold: workspace, engine base copied from ivory-rust, specs, fonts, golden corpus |
| `c9e15a0` | Design + divergence policy (after a 3-lens adversarial critique) |
| `c8be947` | GUI parity port (9 modules), packaging scripts, engine acceptance contract |
| `5dbe50f` | **Engine surgery: all 20 D-rules then defined land; acceptance + 42 unit tests green** |
| `290a73d` | Refine D20 (fixed 44 m11-inversion regressions) |
| `0d0926e` | Fix invisible white-key separators (bug the owner caught on-device) |
| `da487fa` | This HANDOFF doc |
| `60bc651` | **Verify phase: teach layer + differential classification + D21 note-drop fixes** |

**Engine + teach + verify are all DONE and green.** At this commit the counts
were GUI 11 + engine 54 unit + 3 acceptance + differential(fast), and
`--features learning` → 58 + 3. **As of 2026-08-11 they are:** `cargo test
--workspace` → GUI 14 + engine 59 unit + 3 acceptance + 10 learning +
differential(fast); `cargo test -p ivory-core` (stock, no `learning`) → 55 + 3
+ differential(fast). The verify workflow (classifier + adversarial reviewer +
teach agent) completed:
- **Teach layer done**: `overrides.rs` (exact overrides + feature-gated
  `learning` perceptron, off by default, zero-weights no-op), wired into the
  detector (consulted before scoring) and the two GUI menu items. 0 engine
  regressions vs the frozen baseline.
- **Classification done** (`tests/golden/`): `rust-golden.json` (frozen
  baseline), `classified-divergences.json` (every corpus divergence → D-rule),
  `classify.py`, `UNEXPLAINED.md`, `README.md`, and
  `ivory-core/tests/differential.rs` (self-consistency guard — regenerate the
  baseline + re-run `classify.py` after ANY engine change; procedure in
  `tests/golden/README.md`). Corpus mismatch vs raw Python was 5057 at this
  commit; after D22–D26 it is **5540** (verified 2026-08-11 via `diffcorpus`),
  and `classified-divergences.json` has not been regenerated since.
- **Adversarial review done**: no misfire found in the 7 broad scoring changes.
- **D21 note-drop fixes**: the classifier caught 28 drop-2 voicings dropping a
  #5/#11; fixed via a completeness preference + a maj7#11 perfect-inversion
  boost. One residual non-blocking slash-simplify note-drop is documented in
  DIVERGENCES.md D21 and `tests/golden/UNEXPLAINED.md` (top note).

## 2a. v2.1.0 — Chord Learning is now a real GUI option (2026-08-04)

Built for a first hands-on test by a non-owner on macOS **and** Windows.

- **Menu** (D-UI-9): `Correct Chord Name...` (greyed with no notes held) and
  `Enable/Disable Chord Learning`, right after the D-UI-5 teach pair.
  `Manage Taught Chords...` grew a footer: learning on/off, correction count,
  the non-zero learned weights, and `Forget Learning`.
- **Correction dialog** lists only `trainable_candidates()` with their scores
  and marks the current winner, so a correction can never land on a name the
  re-ranker is unable to reach. Every attempt reports its outcome.
- **Measured blast radius** (`ivory-core/tests/blast_radius.rs`, `#[ignore]`d —
  run it with `--release -- --ignored --nocapture`): ONE correction
  (`C-E-G-A → Am7`, 5 steps) changed **1,182 of the 13,133 corpus voicings
  (9.0%)** when measured at 2.1.0 — **re-measured 2026-08-13 after D22–D26 it
  is 1,278 (9.7%)** — many in unrelated keys, because chord identity enters the
  feature vector only as `hash % 97`. `Forget Learning` restored all 13,133
  exactly, both times.
  That number is the honest answer to "is the re-ranker worth keeping" — it is
  a taste dial with a very wide blast radius, not a per-chord memory. It is
  quoted in the correction dialog and in the tester README rather than hidden.
- **Six real bugs fixed on the way** (all silent, four found by an adversarial
  review panel, two by a design panel):
  1. `train_on_correction` matched the winning candidate by the *final*
     label. Post-scoring renames (slash bass, rootless dominant) mean that
     label usually is not a candidate — e.g. E-G-C displays `C/E` while the
     candidates are `C`, `Em`, `G4` — so training silently did nothing on
     exactly the ambiguous voicings worth correcting. It now trains against
     the candidate behind the displayed label, judges success on the
     displayed label (the D21 completeness rule needs a >12-point margin,
     not merely first place), and rolls back if it cannot get there.
  2. **The re-ranker could make a chord nameless.** Candidate admission used
     `score > 0.0` applied *after* the learned adjustment (bounded at −100), so
     a few ordinary corrections could drag every candidate for a voicing to
     ≤ 0, leaving `best_match = None` — the chord strip went blank on chords
     the stock engine names fine (e.g. B-B-A♯ = `BΔ7(#11)`). Admission now uses
     the **unadjusted** score and only ranking uses the adjusted one, so the
     re-ranker can reorder but never eliminate. Regression-tested
     (`training_never_makes_a_chord_nameless`, verified to fail before the fix).
  3. **Scale readings offered 7 impossible choices.** The late scale check runs
     *after* the scoring loop and discards the winner outright, so for e.g.
     C Ionian the picker listed 7 candidates of which six burned the full
     25-step budget and then blamed a score gap that did not exist. A
     `label_from_scale` flag now makes `trainable_candidates()` return empty,
     so the GUI says "named by a fixed rule" instead.
  4. `scripts/build-cross.sh` shipped **binary-less Linux tarballs**: the
     function is called as `package_linux ... || handler`, which suppresses
     `set -e` for its whole body, so a failed ALSA build fell through to
     `tar` and produced an 85 KB archive of fonts and licences. Every step is
     now checked by hand; failure leaves no artifact.
  5. `scripts/build-macos.sh` packaged `"$APP"` alone with ditto/hdiutil, so
     the tester README never left the build machine while the Windows zip got
     it. Both now package a staged folder (app + `READ-ME-FIRST.md`).
  6. `OverrideStore::save()` was a plain `fs::write` on a file that now holds
     taught chords AND learned weights and is rewritten on every correction —
     now write-then-rename, so a torn write cannot destroy taught names.
  Also corrected: several outcome messages were confidently wrong (a phantom
  slash-bass explanation on root-position chords, "scores too far behind" when
  the pick had actually won, "intervals and scales" blamed for chords resolved
  by special-case branches, and a correction silently re-arming every earlier
  correction without saying so). `Correct Chord Name...` is now also gated on
  `detection_enabled`, and is no longer a dead click if the notes were released
  while the menu was open.
- **Feature flag**: `ivory` always enables `ivory-core/learning`, so both
  packaging scripts ship it with no changes. `ivory-core` keeps the gate, so
  `cargo test -p ivory-core` still exercises the stock engine.
- **Safety invariants** (all test-asserted): exact overrides still short-circuit
  before scoring and beat the re-ranker; learning off or zero weights ⇒
  adjustment exactly 0.0 ⇒ bit-identical to stock; `Forget Learning` restores
  stock readings; learning state lives only in overrides.json.
- **Tests**: `ivory-core/tests/learning.rs` (10, the user-facing contract),
  `blast_radius.rs` (ignored measurement), plus 3 menu tests and an
  atomic-save test. Whole workspace green: GUI 14 + engine 59 + acceptance 3 +
  differential + learning 10. `cargo test -p ivory-core` (stock, no feature)
  is also still green at 55 + 3 + differential — the differential guard passing
  is the proof that none of this moved stock detection.
- **Not yet verified by a human**: the new dialogs have not been clicked
  through on-device (the owner was mid-call; synthetic clicks do not drive
  egui reliably on macOS anyway), and no build has ever been run on real
  Windows. That is exactly what the friend test covers.

## 2b. 2026-08-10/11 — engine bug-fix round, refactor pass, doc reconciliation

Commits after §2a, none of which were reflected above:

| Commit | What |
|---|---|
| `849a71e` | GUI: Chord Learning becomes a real menu option (D-UI-9) |
| `ecfe669` | Engine: re-ranker made trainable, and unable to erase a chord |
| `8257a5a` | Packaging + docs for 2.1.0; two artifacts were quietly broken |
| `bbf7866` | **D22–D25** — chord-logic fixes from an owner bug report |
| `6d607a7` | **D26** — a scale must account for every sounded pitch class |
| `4310295` `b526a18` `1c98f66` `656edb8` | Tier A refactors: dead code from D-rule deletions, hot-path dedup/hoist, inert `optional_for` |
| `7aa34ab` | Tier B perf: u16 bitmask set-algebra in `match_chord_pattern` |

What that changes for anyone picking this up:
- The D-range is now **D1–D26** (25 rules; D16 withdrawn — see DIVERGENCES K10).
- `tests/golden/rust-golden.json` was regenerated for D22–D25 and again for
  D26. **`classified-divergences.json` was NOT** — it is a 2026-07-29 snapshot
  of 5,057 rows, while the live Rust-vs-Python mismatch is **5,540**. Re-run
  `tests/golden/classify.py` before quoting its per-rule table.
- Whole workspace green at GUI 14 + engine 59 unit + 3 acceptance + 10 learning
  + differential(fast). The full 13,133-row differential is `#[ignore]`d — see §5.
- One-correction blast radius re-measured after D22–D26: **1,278 of 13,133
  (9.7%)**, up from the 1,182 (9.0%) quoted in §2a. (This said 1,279 until
  2026-08-13, when it was re-run twice here and once in a worktree at `49c6e8c`
  and came back 1,278 every time. The engine has not moved since D26; the old
  figure was simply off by one. Same lesson as the "543-byte placeholder icon"
  in §8 — re-run the measurement before repeating the number.)

### Remaining work (task 8 — FINALIZE only)
Everything above is committed. Packaging works; what is left is business +
platform coverage:
- **Not the icon.** `assets/ivory.png` is the ORIGINAL art, byte-identical to
  the Python app's — it blocks nothing. See §7 and `docs/RELEASE.md` gate 1.
  (This bullet used to say "543-byte placeholder — blocks release"; that was
  wrong for a week. See §8.)
- **Not Codeberg or `ivory-rust`.** Both done — see §7.
- macOS and Windows artifacts build (§7). **Linux has never been built**: run
  `scripts/build-linux-native.sh` on a Linux host, once per arch.
- Business decisions in `docs/RELEASE.md` (MIT retention, Developer ID signing
  + notarization, Windows signing, the Synthogy name collision) are the real
  gate on a public release.
- Optional polish: 6 cosmetic warnings in `ivory-core` — 5 unused variables
  (`pcs_set`, `best_root_pc`, `highest_pc`, `highest_note`, `matched_count`)
  and one non-snake-case `has_M3`. The `ivory` crate emits none.

---

## 2c. 2026-08-13 — renamed to Tangent, tester fixes, egui 0.33, fretboard core

**THE NAME.** The product is **Tangent**. "Ivory" is now the INTERNAL CODENAME
and is deliberately everywhere it is invisible: crate names (`ivory`,
`ivory-core`, `ivory-keygen`, `ivory-fulfil`), `~/.config/ivory/`, the settings
and overrides paths, `IVORY_*` env switches, `assets/ivory.*`, and above all
**`CFBundleIdentifier = org.codeberg.ganten1998.ivory`, which must never
change** (it would reset Gatekeeper trust on every signed build). The binary is
`tangent` via a `[[bin]]` entry, so the package stays `ivory`: **`cargo run -p
tangent` does NOT resolve, use `--bin tangent`.** Why Tangent: the clavichord's
tangent is the blade at the back of each key that strikes the string and stays
in contact for as long as the note sounds, which is what the display does with
a held note. Synthogy's Ivory is a famous virtual piano and a VST3 build would
have put us in the same browser. Three naming rounds and a trademark analysis
are behind this; do not reopen it.

**Artifacts renamed**, and `scripts/publish-github.sh` now uploads every
artifact THREE times: version-scoped, version-less alias, and under the OLD
`Ivory-*` names. That third set is not optional. Those exact links are in the
inbox of everyone who has already bought a supporter key, and they resolve by
exact asset name. The `Tangent-*` aliases were also back-filled onto the v2.2.0
release, because the fulfilment email was redeployed with new links before new
assets existed and 404'd for about ten minutes.

**egui/eframe are pinned at 0.33, deliberately, and must NOT be bumped.**
`nih_plug_egui` pins egui 0.31 through a stale `egui-baseview` rev; upstream
`egui-baseview` is on 0.33. 0.33 is the only version where the desktop app and
a nih-plug plugin editor can share one GUI. Bumping to 0.35 forks the GUI in
two. The API differences that bite: eframe 0.33 uses `App::update(ctx)` not
`App::ui(ui)`, `show_viewport_immediate` hands the closure a `Context` not a
`Ui`, `Context::run` not `run_ui`, and `Stroke::new` takes `impl Into<f32>` so
float literals need `_f32`. All of the Context-to-Ui bridging is one function,
**`ivory/src/shell.rs::viewport_ui`**, which is also the shape the plugin
editor needs. It is tested to hand over the whole viewport, because a default
`CentralPanel` adds margin and would silently shift the piano.

**2.2.0 tester-report fixes** are D-UI-10 through D-UI-14 in DIVERGENCES.md:
child windows centre on the parent, the detached chord window is no longer
slaved to the piano's width and remembers its geometry, long labels wrap to two
lines, the detached window has a border, and Teach's "Apply in all keys"
defaults on. **Tiling window managers are detected and excluded** from geometry
memory (`wm_overrode_size`, `WM_GRACE`): the owner runs AeroSpace, which tiled
the detached window to 853x1377 and got that recorded as a user preference in
the old code. That is where `detached_chord_height: 1377` came from.

**`ivory-core/src/fretboard.rs`** is the pure half of the guitar view:
geometry (rule of 18, fret 12 at exactly 0.5), tunings, capo-as-new-nut, and
candidate enumeration. Middle C is FIVE positions on a standard board and the
high E string cannot reach it at all, which is why a solver is needed. Output
is ordered and proven deterministic; out-of-range pitches fold to their pitch
class and are flagged rather than dropped. **Next: the voicing solver**, which
is where the taste lives and which the owner must play-test, since determinism
is testable and musicality is not.

**LICENSING.md** records the MIT/GPLv3 split: everything in the repo stays MIT
including the standalone binary; only the `.vst3` bundle is GPL-3.0-or-later,
because nih-plug's VST3 bindings are and copyleft attaches to the linking
binary. Shipping both in one installer is an aggregate under GPLv3 §5. Three
conditions keep that true and are listed there. The plugin must ship the GPLv3
text inside its bundle and releases must pin the exact nih-plug revision.

**PLUGIN DECISIONS ALREADY MADE.** VST3 only, one deliverable, shipped with the
standalone as an optional install. This means **Logic Pro is out** (AU only)
and so is Pro Tools (AAX). The plugin will have **no detached windows**: a VST3
editor is one host-owned child window, so `Caps::detachable = false` removes
those menu entries rather than faking them with an embedded `egui::Window`.

**WHERE TO PICK UP.** See §2d — the solver and the panel are both done.

## 2d. 2026-08-13 (later) — the voicing solver and the guitar view

Both of the next steps §2c named are done and committed (`aae49c9`, `ef67801`).
`cargo test --workspace` is **196 green**, `cargo build --workspace
--all-targets` is warning-free, and the full 13,133-row differential still
passes (the chord engine was not touched).

### `ivory-core/src/voicing.rs` — which position lights up

`fretboard.rs` says where a pitch CAN go; this says where it DOES go. Design
came from a four-proposal / three-judge panel whose numbers were executed
against the real candidate tables, not reasoned about.

**The one structural decision everything rests on: assignment is MONOTONE.**
Ascending sounding pitch goes on ascending strings. That is not a heuristic, it
is the search: it gives "at most one note per string" for free, forbids voice
crossings by construction, and collapses the space to `C(n + strings, strings)`
small enough to enumerate **exhaustively**. So there are no hand-position
windows, no beam, no DP table and no approximation anywhere. Measured:
**6.5 µs per solve**, 254 leaves on a ten-note piano voicing, against a 40,000
leaf budget.

**`note_slack` was the one place an approximation had crept in, and it is gone.**
The pre-cap trims the held set before the search; at the original 2 it threw
away a note the search could have placed on **11.5% of chords big enough to
trigger it** (measured over 14,500), so the board drew five dots where six would
fit. At 8 that is 0 of 14,500, for ~3x the leaves and still a fiftieth of a
frame. Note that raising it CHANGED THE COSTS in the acceptance table without
changing a single shape: a pre-capped note never entered the objective, so its
drop was never charged to anything. A cost is only comparable within one
`Weights`, which is why it is never shown to a user. The cost of it: deliberate crossing voicings (a fretted
9th under an open B) and thumb-over bass notes cannot be represented. Changing
that means rewriting the search.

**Costs are `i32`, never floats**, because float addition is not associative and
a reordered sum can flip a near-tie. `REACH_MILLI` is a frozen table rather than
a runtime `powf` for the same reason. Determinism is a shipped contract with
tests behind it, including one that solves from 100 freshly-built `HashSet`s
(`display_notes()` builds a new one every call and `RandomState` is seeded per
instance — that is a real trap, not a theoretical one).

**The drop tier is two orders of magnitude above the shape terms**, and
`a_note_with_nothing_doubling_it_is_never_shed_for_a_prettier_shape` asserts the
gap: the whole spread of shape costs is 6290 against a 15000 floor on dropping a
note whose pitch class sounds nowhere else. A nicer hand position can therefore
never cost you a note you played. Retire that gap and the test goes red.

**Every dial is in `Weights`**, in the order the owner will reach for them:
`position_per_fret` (30) first, then `finger_cum[5]` (the 5th finger costs 300),
then the `stretch` table, then `drop_highest`/`drop_lowest`. The ordering
(`drop >> fingers > stretch > position > skip > open`) is the part to defend;
the integers are the part to lose an argument about.

**Iterate it the way the chord engine is iterated:**
```bash
cargo run -p ivory-core --example voiceprobe --release             # the calibration set
cargo run -p ivory-core --example voiceprobe --release -- 60 64 67  # one voicing, with runners-up
cargo run -p ivory-core --example voiceprobe --release -- --tuning DADGAD --capo 3 57 58
cargo test -p ivory-core --test voicing_acceptance                 # the pinned shapes
```
`tests/voicing_acceptance.rs` is a **tripwire, not a law**: turning a dial will
light some rows up, and that is the point — it makes the blast radius of a taste
change a diff instead of something noticed in a demo three weeks later. When
play-testing disagrees with a row, the row changes, after the dial that moved it
is written down.

**The knife-edge cases to play first**, all verified by `voiceprobe`:
- rootless Dm9 `62,65,69,72,76` → `x 17 15 14 13 12`, honestly 5 fingers and
  labelled `TwoHands`. Every proposal in the design panel put this at frets
  17–22. Dials: `finger_cum[5]`, `position_high_per_fret`.
- Am7 close `57,60,64,67` → `x x 7 5 5 3` (628) vs `x 12 10 9 8 x` (723). 95
  points apart, where `position_per_fret` and `stretch` fight.
- E4+F4 `64,65` → `x x x 9 6 x` (424) vs `x x x x 5 1` (435). Eleven points.

### `ivory/src/fretboard_panel.rs` — the view (D-UI-15, full detail there)

Dumb by design, like `piano.rs`. The app owns exactly ONE `VoicingSession` and
re-solves on the same 100ms gate as chord detection. **Off by default**
(`show_fretboard: false`): turning it on makes the window taller, and a window
that grows on its own after an update is the D-UI-10/11 surprise again. One line
in `Settings::default()` to flip it once the view has been played with.

Menu **submenus are plural now** — `Entry::Submenu { label, items }` replaced the
hard-coded `Entry::SizeParent`, and Size is asserted to come out of the
generalised path unchanged. Tuning and Capo appear only while the view is on.

**Two things that only turned up on screen**, which is the argument for always
doing the `screencapture` pass:
1. The session drew a **different shape from `solve_cold`** on the same notes.
   Every note in a tick was getting its own arrival ordinal in pitch order, and
   the drop policy reads ordinals as age, so a ten-note voicing shed its bass
   because the bass "arrived first". Arrival is a property of the TICK.
   Regression-tested over 200 random chords.
2. The first palette drew dark strings on the piano's light background and
   looked like a spreadsheet. It is a dark fingerboard, light strings and a bone
   nut now; held notes use `white_key_active_color`, which the user already
   chose for a held piano key.

Watch for: `fret_x(22)` is **0.719**, not 1.0, because a 22-fret neck is only 72%
of the way to the bridge. The panel scales by `fret_x(frets)` so the last fret
lands on the right edge. Drawing straight into widget space leaves a quarter of
the band empty and puts every dot in the wrong place; the test
`the_last_fret_lands_on_the_right_edge` is what stops that coming back.

### 2026-08-13, later still: three woods, a popped-out neck, and D-UI-17

- **Three fingerboard woods** (Rosewood default, Maple, Ebony) under a `Wood`
  submenu. Each carries its WHOLE palette, not a fill colour: maple is pale, so
  on it the strings, wires, inlays and nut all go dark and note dots gain an
  edge ring. The wood does NOT follow dark mode — only the band around it does.
- **The neck pops out** (`Detach Fretboard`), mirroring the chord window
  exactly: close-to-reattach, right-click-anywhere menu, the same tiling-WM
  geometry guard. `Hide Fretboard` closes it rather than orphaning a window.
- **D-UI-17: the piano had twelve gaps in it.** Each white key was drawn
  `trunc(width/52)` = 31px wide while the keys step by 31.25, so wherever the
  fraction rolled over the key came up a pixel short and the background showed
  through. Spec §4 preserved those truncation slivers as Qt parity — but they
  are only invisible in DARK mode, where the background happens to equal the
  white-key colour. In light mode they were twelve grey slivers. Keys now run to
  where the next one starts; the separators land on the same pixels.
  **Found by decoding a screenshot to a BMP and counting pixel runs, not by
  squinting** — 52 key runs, 51 separators, and 12 runs of background colour
  that should not have been there. Worth repeating for any "does this look
  right" question; eyeballing a 1px sliver does not work.

### An unverified review, and what came of it

A five-dimension adversarial review ran twice. The first attempt died entirely
on API 529s; the second got three of five dimensions through and then lost
**every one of the eighteen refutation agents** to a session limit. Its
"0 of 6 findings survived" is therefore meaningless — nothing was verified.
The six claims were checked by hand instead:

| Claim | Verdict |
|---|---|
| Fingerboard slab painted outside its band on any tuning with < 6 strings | **REAL**, fixed. Bass (4) painted 8.6pt over the piano; the existing test only checked string positions, not the slab |
| Submenus have no monitor clamp | **REAL**, fixed. Size never hit it (first row, 7 tall); Capo is 10 rows near the bottom |
| Pre-cap discards a placeable note | **REAL**, fixed — see `note_slack` above, 11.5% measured |
| `caption()` derives its placed count from saturating `u8` counters | **REAL**, fixed by counting directly |
| Conflict rings for a folded note claim `folded: false` | **REAL**, fixed |
| `OctaveMerged` reported when the survivor is itself dropped | **REAL**, fixed — it says `Doubled` now, which is what is still true |

All six were genuine. That is a much higher hit rate than a review usually has,
and the reason is worth remembering: the module is new, so nothing had been shaken
out yet. **Re-run the review before 2.3.0 ships** — two dimensions (determinism,
panics) never reported at all. The panics dimension is partly covered by
`tests/voicing_stress.rs`, written here after a hostile sweep found a real
overflow panic (`note_slack: usize::MAX`), but determinism was never reviewed.

## 2e. 2026-08-14 — 2.3.0 shipped, and the plugin is half built

**2.3.0 IS PUBLIC** on all three platforms (macOS signed+notarized, Windows,
Linux x86_64 — the first Linux build this project has ever produced). Every
permalink, including the legacy `Ivory-*` names in supporter emails already
sent, was byte-verified as serving 2.3.0. The nine existing purchasers were
emailed via Resend; Gumroad refuses customer email below a payout or $100 in
sales, and `keys@ivorymidi.com` can send but not receive, so Reply-To matters.

Since then, unreleased on `main`: the fretboard became an INPUT (click the neck
with keytoggle on, positions are pinned so a hand-entered shape stays put),
keyboard shortcuts with a hold-to-view card, JetBrains Mono as a third face, and
the About box finally pointing at ganten.neocities.org.

### Plugin progress: steps 1-4b of docs/PLUGIN-PLAN.md are DONE

Read `docs/PLUGIN-PLAN.md` — it has the full plan, a verified-facts table that
corrects several confident wrong claims, and a progress log. Landed:

1-2. `NoteState` extracted and tested; `Settings::save_to` made atomic.
3. **`ivory-ui` crate exists** — nine GUI modules moved by `git mv`, 229 tests
   before and after. `scripts/check-firewall.sh` asserts `ivory-ui`/`ivory-core`
   cannot see eframe, midir, rfd or fd-lock and never call `process::exit`,
   and it is verified to FAIL when a violation is planted.
4a. **`Caps`** in `ivory-ui/src/host.rs`. The menu drops Size, Borderless,
   Select MIDI Input and both Detach pairs under `Caps::PLUGIN`; tests hold both
   the desktop-unchanged and plugin-safe ends.
4b. **All 11 dialogs** render either as an OS window or in-canvas, chosen by
   `caps.child_windows`, with a click-swallowing scrim so in-canvas is genuinely
   modal.

### WHAT IS LEFT, in order

5. **`app.rs` is the last file holding eframe.** It needs the `Shell` wrapper so
   its paint body can be driven by either host: route the remaining
   `send_viewport_cmd` sites (9) through a trait, and move `app.rs` into
   `ivory-ui` leaving `impl eframe::App` behind in the binary. The orphan rule
   forces a thin wrapper type in `ivory` — that is expected, not a problem.
6. **The quarantined `plugin/` workspace.** `plugin/Cargo.toml` opens with an
   empty `[workspace]` table, exactly like `tools/ivory-keygen`, so nih-plug and
   `vst3-sys` never enter the root `Cargo.lock`. That is not tidiness: without
   it `gen-third-party-licenses.sh` lists GPLv3 `vst3-sys` inside the MIT app's
   shipped `THIRD-PARTY-LICENSES`.
7. `nih_export_vst3!`, the editor, per-instance persisted state.
8. `scripts/build-plugin.sh`, then the INSTALLER (owner requirement, see the
   PLUGIN-PLAN addendum: all three platforms, standalone / plugin / both).

### The dependency question is SETTLED — do not re-derive it

Verified by compiling, not by reading:

```toml
nih_plug      = { git = "https://github.com/robbert-vdh/nih-plug.git",
                  rev = "28b149ec4d62757d0b448809148a0c3ca6e09a95",
                  features = ["vst3"] }
nih_plug_egui = { git = "https://github.com/BillyDM/egui-baseview.git" }
egui          = "0.33"
```

Clean build, ONE copy of each of egui 0.33.3, egui-baseview 0.7.0, nih_plug,
nih_plug_egui 0.1.0 and baseview. **No `[patch]`, no fork, no vendoring** —
`nih_plug_egui` 0.1.0 now lives INSIDE the egui-baseview repo and is already on
egui 0.33. Taking it from the nih-plug repo instead needs two coordinated
patches and two forks, because the two crates pin different `baseview` revs and
`WindowHandle` becomes two incompatible types. The `rev` on nih_plug is
mandatory: it must be the one egui-baseview's workspace pins, or there are two
`nih_plug` packages and `Editor` stops being `Editor`.

### The trap that decides step 5

`show_viewport_immediate` does NOT fail in a plugin. `embed_viewports: true`
makes it run INLINE, opening a second `CentralPanel` under an identical id and
painting garbage over the piano. Any seam must sit ABOVE `shell::viewport_ui`.
`ViewportCommand::InnerSize` is also honoured by egui-baseview, so `app.rs`'s
fixed-size enforcement would resize the host's window behind its back on frame
one — it must be GATED, not merely left to no-op.

### WHERE TO PICK UP NOW

1. **Play-test the solver.** It is provably deterministic and provably exhaustive
   over its own objective; it has never been judged by a guitarist. Start with
   the three knife-edge voicings above. Everything you would want to turn is in
   `Weights`, and `voiceprobe` prints the runners-up so a disagreement can be
   argued in points.
2. **Decide `show_fretboard`'s default** once (1) has happened.
3. **Cut 2.3.0**: bump the workspace version (it is still 2.2.0 — the version
   bump belongs to the release commit, as it did for 2.2.0), move
   `CHANGELOG.md`'s `[Unreleased]` under the new heading, then
   `scripts/release.sh`. Remember `scripts/publish-github.sh` must still upload
   the legacy `Ivory-*` names (§2c) — that is not optional.
4. Then the `Shell`/`Caps` refactor, then `ivory-plugin` with
   `nih_export_vst3!`, then the installer. `Caps::detachable = false` for the
   plugin; the fretboard panel needs no detached variant either.

## 2f. 2026-08-14 (later) — the plugin exists, and so do the installers

**The VST3 plugin is built, loads and instantiates.** Steps 5-8 of
`docs/PLUGIN-PLAN.md` are done. Also landed: the theory band (a new user
request), and a fix for a Windows bug that had shipped in every release.

### What is on `main` and unreleased

Everything below 2.3.0's tag: the fretboard as an input, keyboard shortcuts,
JetBrains Mono, the About URL, **the theory band**, **the plugin**, **the
installers**, and **the tangent.exe icon fix**. 261 tests, no warnings,
firewall intact.

### The plugin, in one paragraph

`plugin/` is its own workspace (empty `[workspace]` table). `plugin/src/lib.rs`
is a shell: everything on screen is `ivory_ui::app::IvoryApp`, the same code
the standalone runs. Notes cross from the audio thread on a pre-allocated
`crossbeam::ArrayQueue` — never a mutex, because the editor holds its state
locked for a whole frame. State persists into the DAW project as the same JSON
the settings file holds (`Settings::to_json`/`from_json`), one `#[persist]`
blob rather than a parameter each. The app is kept alive ACROSS editor open and
close, because `create_egui_editor` takes its state by value and closing a
window must not reset the tuning.

### Things that will bite the next person, all verified

**`send_viewport_cmd` is `egui`, not `eframe`.** The plan assumed a `Shell`
trait was needed to route it. It was not — `app.rs` moved into `ivory-ui`
keeping every call, gated on `caps`. The seam was three things, not nine:
`eframe::CreationContext` (now `&egui::Context`), `midir` (now behind
`ports::MidiPorts`), and `impl eframe::App` (now `ivory/src/desktop.rs`).

**`egui-baseview` HONOURS `ViewportCommand::InnerSize`** — it calls
`window.resize()`, `src/window.rs:369-373` — while swallowing Min and Max via
`_ => {}`. An ungated fixed-size triple resizes the DAW's editor on frame one
and keeps exactly the third of the mechanism that does damage.
`a_plugin_frame_never_commands_the_hosts_window` runs four frames under
`Caps::PLUGIN` and asserts zero commands; it fails if any one gate is removed.

**`llvm-rc` treats any argument starting with `/` as a switch.** An absolute
Unix input path is therefore not an input: `/Users/...` parses as `/U` and it
says "Exactly one input file should be provided" while pointing at nothing.
`/private/tmp/...` happens to work, which is why it reproduces on a real build
and not in a scratch directory. Run it from OUT_DIR with bare filenames.

**Linux cannot be cross-built, for the plugin either.** baseview links X11
through the `x11` crate, whose build script needs a pkg-config sysroot — the
same shape as alsa-sys for the standalone. `scripts/build-linux-remote.sh`
builds BOTH on the Linux host and rsyncs them back.

**`build-cross.sh` used to delete the Linux tarball before building it.** On
macOS that build always fails, so every run destroyed a good artifact that had
been built remotely — silently, before the error that made it look as if the
run had simply produced nothing. Fixed; it now stages and moves on success.

**`._` entries in `lsbom` output are not files.** They are AppleDouble
metadata records; `pkgutil --expand-full` extracts a tree with zero of them.
Checked, because they look exactly like litter.

### The theory band, after a play-test

It shipped following live MIDI and lighting two different things in one
colour. Both were wrong and both were found by looking at a real window.

**It no longer follows your playing by default** (`theory_follow_midi`, off).
A diagram that redraws on every note cannot be read while playing notes, and
reading it while playing is the entire use. It shows placed notes; "Follow
MIDI" in the Theory submenu restores live tracking.

**The diagrams are inputs.** `theory_panel::hit_test` is the exact inverse of
the drawing and is tested as such, node by node and vertex by vertex. Clicking
a chord vertex places the whole triad.

**The highlighting follows one rule: one meaning, one mark.** Sounding note =
filled disc in the piano's active colour. Root = that disc, ringed. Key that
fits = a neutral wash, and no wash at all for a partial fit. Major vs minor
triads differ by solid-vs-outlined in ONE hue, because orange used to be both
"root" and "minor triad".

The lattice is anchored at C rather than re-centred on the tonic: re-centring
looked clever and became wrong the moment it was clickable, since placing a
note moved the node out from under the pointer.

### Two process lessons from this round

**`kill %1` does nothing across tool calls.** Each Bash call is a new shell, so
a backgrounded app survives and the next screenshot is of the OLD binary. A
correct fix looked broken for three rounds because of it. Use `pkill -f`.

**`screencapture -R` grabs whatever is on top.** To photograph a background
window without stealing focus, get its CGWindowID and use `screencapture -l`.
There is a 40-line helper for it in the session scratchpad; it is worth
rewriting rather than re-deriving.

### What is NOT done

1. **Nobody has opened the plugin in a DAW.** It loads, instantiates and
   reports one event input and no audio buses under `scripts/verify-plugin.c`,
   which is real proof and is not the same as seeing it draw. Test it in
   Reaper: MIDI track, add Tangent, play.
2. **The Linux plugin has not been built.** No ssh key for the Void box exists
   on this machine (`~/.ssh` has only `codeberg_ed25519`). One
   `ssh-copy-id` and `scripts/build-linux-remote.sh <host>` does the rest.
3. **The macOS .pkg is unsigned.** It needs a *Developer ID Installer*
   certificate — a different certificate from the *Developer ID Application*
   one that already signs and notarizes the app, same account, separate
   download. `productsign` refuses the application identity outright.
   `release.sh` correctly blocks publication until this is fixed.
4. **The macOS `.pkg` is not signed** — see above. Everything else in
   `release.sh` passes.
5. **No release cut.** Bump the workspace version, move `[Unreleased]` under
   the new heading, then `scripts/release.sh`. `publish-github.sh` must still
   upload the legacy `Ivory-*` names (§2c).

### Commands added this round

```
scripts/build-plugin.sh macos|windows|linux    # the .vst3 bundle
scripts/build-installer.sh macos|windows|linux # the installers
cc -o /tmp/vp scripts/verify-plugin.c && /tmp/vp <bundle>/Contents/MacOS/Tangent
```

## 2g. 2026-08-18 — the built-in instrument, its patches, and two effect knobs

Shipped as **4.6.0**, committed, **not yet published**. 4.5.0 was the batch
before it (Space-to-audition, note names, the first FM built-in). 5.0 is still
reserved for after the full code audit the owner asked for.

### The instrument

`ivory/src/dx7/` is a faithful six-operator FM engine: `voice.rs` unpacks the
128 packed bytes of a patch, `sysex.rs` reads a 4104-byte cartridge,
`algorithms.rs` is the transcribed routing table, `synth.rs` is the DSP.
Validated against 11,756 real cartridges (`IVORY_SYX_CORPUS=... cargo test -p
ivory --bins the_whole_corpus -- --ignored --nocapture`).

`ivory/src/builtin.rs` is **gone** — the two-operator sketch it held is fully
superseded.

**The default patch is `Voice::electric_piano()`, written into the source.**
That is a licensing decision as much as a size one: the factory ROMs are
Yamaha's and the banks people trade are their authors'. Do not replace it with
a bundled cartridge without settling that.

**The landmine that was found here, by listening:** the envelope rate constants
(`FASTEST_SWEEP`, `RATE_HALVING`) were wrong — rate 99 at 1.5ms and rate 0 at
twenty seconds — so a held note died in a third of a second and *every*
cartridge played with the wrong decay. Invisible to any test that only asks
whether a sample is non-zero. Two tests now pin it:
`the_envelope_rate_curve_matches_a_real_dx7` and
`the_default_patch_sustains_a_held_note`.

### How it is reached

Not a bundled VST3 — a sentinel path (`dialogs::BUILTIN_PATH`) inserted at the
top of the existing slot picker. No second bundle to sign, notarize or install,
and no scan that can fail. `desktop.rs::reconcile_plugin` intercepts it.

`IvoryApp::open_slot_editor` forks on it: a VST3 gets its own window, the
built-in gets `Dialog::PatchPicker`. **Selecting is auditioning** — no Load
button, because a patch applies between buffers and dialing one in means
playing while you move down the list. `settings.dx7_cartridge` /
`dx7_patch` persist it; a missing cartridge falls back silently.

The host holds the `Cartridge`; the UI holds `ports::CartridgeInfo` (names
only). Same firewall as the plugin picker's paths-not-modules.

### The effects

`ivory/src/effects.rs`. Reverb is Schroeder-Moorer, delay is tempo-synced to a
dotted eighth. **Position in the chain is the feature**: applied to `self.mix`
in `Renderer::render`, right after `render_builtin` — downstream of every
instrument, upstream of the tap (so the take carries it), upstream of the click
and input monitor (so those stay dry). Off by default and free when off.

Two knobs on the recorder band, modelled on a Tascam 388. To pay for the width
the fader column went from 0.43 to 0.34 of the middle group; **the meters were
not touched** and must not be, their faces are what the band was rebuilt
around. `a_gain_reading_fits_the_box_reserved_for_it_at_the_smallest_band` is
the test that says where the dB reading stops fitting.

`Produces::hit` now takes the rect and the point, and `recorder_panel::
drag_axis` reports which way a control travels so the caller pins the *other*
axis. Pinning the wrong one does not make a knob fussy — every probe reports
the same value and it is dead.

### Screenshots when screen capture stops working

`screencapture -l <winid>` began returning "could not create image from window"
mid-session. `composite.rs` has an ignored test that renders a frame offscreen
through the real compositor and writes a PNG:

```
IVORY_SHOT=/tmp/x.png IVORY_SHOT_ROWS=210 \
  cargo test -p ivory --bins shot::window -- --ignored --nocapture
```

The readback is **BGRA**; the writer swaps channels. Forget that and tan panels
come out pale blue, which reads as a theme rather than a bug.

### Still open

- **The full code audit before 5.0** ("elegant inside and out, and optimized to
  a degree that is unnecessary"). 124 clippy warnings are the backlog:
  `unnecessary qualification` (28), `field assignment outside of initializer`
  (17), collapsible `if` (7), `is_multiple_of` (5).
- Dead since the take-is-the-window change: `paint_camera`, `WELCOME_SLACK`,
  `keys::family`, `PluginWriter::sample_rate`.
- `fetch-ffmpeg.sh` has no aarch64 Linux build.
- 4.6.0 artifacts are in `dist/` for every platform; nothing is published.
  Windows test kit staged at `~/Desktop/Tangent-windows-test/`.

## 2h. 2026-08-19 — six knobs, a true-peak limiter, and video that happens

Shipped as **4.12.0**. Driven entirely by the Windows tester's report on
4.11.3: the DX7 works end to end there (patches, SysEx, editor), record works,
`.wav` and `.mid` are both right — and two things were wrong.

### A take with no camera produced no video

`VideoMode::default()` was `None`, and the only thing that had ever switched
video on was `apply_video_default`, which fires **only if a camera was
chosen**. So a machine with no webcam recorded audio and MIDI, wrote no
`.mp4`, and said nothing about it. The default is now `Composite`, and
`SETTINGS_VERSION` went 7 → 8 to move existing files onto it once.

**The window is the take.** A camera is an inset when there is one; the piano,
the chord and the diagrams are the thing being recorded either way. The
decision now lives in one place — `ExportSpec::produces_video(camera_running)`
— which `begin_video` calls rather than re-deriving, so the test and the host
cannot disagree.

### The Windows camera picker offered a macOS privacy panel

There is **no Windows camera backend**: `ivory-record/src/camera.rs` has
`macos`, `linux` and `stub`, and Windows gets `stub`, so the list is empty on
every Windows machine whether or not a webcam is plugged in. The picker then
told the user to check "System Settings > Privacy & Security", which is not a
thing that exists on their operating system — so it read as a broken app
rather than an unbuilt feature. `DeviceKind::nothing_found` now has a Windows
arm saying plainly that camera recording is not available there yet and that
takes still record the window. **The `cfg` arm was checked by compiling for
`x86_64-pc-windows-msvc`**, per §8's rule about host-evaluated `cfg`.

`ivory/src/desktop.rs`'s "camera is open but sending no picture" advice is now
per-platform too; it was macOS text on all three.

### Six knobs, in two rows

`Fx` grew from three to six: REVERB / DELAY / CHORUS on top in Tascam blue,
then HPF and LPF in ivory and LIMITER in red. `METER_SHARE` went 0.62 → 0.46 to
pay for the second row — the meters gave up the height, because six knobs that
do not fit are worse than a shorter VU.

Three things collapsed on the way, and they are the interesting part:

- **`Hit::SetReverb/SetDelay/SetChorus` → `Hit::SetFx(Fx, f32)`**, and
  `NumField::Reverb/Delay/Chorus` → `NumField::Fx(Fx)`. There were 55 sites
  across two files; six knobs would have made 110. **This introduced a real
  bug and a test caught it**: `Hit::control_key` disambiguated by
  `mem::discriminant`, which cannot tell one `SetFx` from another, so grabbing
  REVERB and dragging down onto HPF would have set the high-pass. It carries
  `fx.index()` now, exactly as it already carried the slot index.
- **`EffectDefaults.divisions`/`default_division` → `choices: Vec<ChoiceParam>`.**
  A filter slope is the same shape of thing as the delay's time — a short list
  a row steps through — and `FxHit::NextDivision` became
  `FxHit::NextChoice { key }`. `Fx::rows()` returns `FxRow { key, label, step }`
  so the panel reads which rows step instead of matching on
  `key == "delay_division"`.
- **Three loose floats in three structs → `FxSends`.** Six knobs would have
  been eighteen fields kept in step by hand.

`lpf_slope` shipped for about ten minutes with no entry in the test fixture's
choice list, drawing an empty box that did nothing. The guard against that is
`the_host_offers_a_choice_for_every_stepped_row` in `desktop.rs`, which walks
`Fx::ALL` and asserts the host supplies a list for every `step` row and a
default value for every sliding one.

### The limiter, and what "true peak" cost

`limiter_ceiling` defaults to **-1.0 dBTP** exactly, with a 252 ms release and
a 4.5 dB knee. The knob is drive: 0 is bypass, full is 12 dB into the ceiling.

**The first design was wrong and the test said so.** It had no lookahead at
all — reconstruct the peak between two samples, reduce the gain on the spot —
and it let 0.5 dB past the ceiling. The reason is worth keeping: *the peak
between two samples is not made by those two samples.* A converter builds it
from twenty either side, so scaling one sample and leaving its neighbours
alone barely moves the curve they reconstruct together. The gain has to be
down across the whole kernel, which means it has to start going down before
the peak arrives. There is now a 1 ms lookahead with the required gain drawn
backwards as a straight line and kept where it is lowest; total latency is
`look + TP_CENTRE` = 58 samples, **1.2 ms at 48 kHz**.

Two of the three failures on the way there were **bugs in the ruler, not the
DSP**, and both are the kind that would have been "fixed" by loosening a
tolerance:

- The test's own reference reconstruction zero-padded off the ends of the
  buffer. That is a step into silence, and a truncated sinc rings on a step by
  about 9% — which read as the limiter overshooting by 0.9 dB. It measures the
  interior only.
- The slope test measured a low-pass at 16 kHz against 48 k and read 3.9 dB an
  octave for a perfectly good 6. That is bilinear warping. Both measurement
  points now stay under an eighth of the sample rate, and the response is
  checked against `1/sqrt(1 + r^2n)` — the Butterworth its name promises —
  rather than against an asymptote that would pass a filter of the right
  steepness and the wrong shape.

The detector is **8 phases of a 21-tap Blackman-windowed sinc**, verified to
find a pure sine's crest to 0.000 dB from 1 to 11 kHz. It is checked in the
test by an *independent* 8×/33-tap reconstruction, because a true-peak claim
checked with the same filter that made it is the detector agreeing with itself.

### Still open

- The filter knobs read **percent, not Hz**. Hz is what a musician wants, but
  the sweep constants (`HPF_HZ`, `LPF_HZ`) live in the binary behind the
  firewall and `RecorderView` has no route for them. It needs a display-unit
  hint on `EffectDefaults`; it is not hard, it was just more than the knob was
  worth today.
- `IVORY_SHOT_FX` renders no panel — for the reverb either, so it predates
  this work. The panels are covered by `every_panel_row_reaches_its_own_parameter`
  instead, which walks all six.
- Everything in §2g's "take next" that is still open, plus Void finding 1: the
  audio engine owns neither its sample rate, its buffer, nor its thread
  priority, and a failed stream open is silent. That is where the 5.0 audit
  starts.

---

## 2i. 2026-08-19 (later) — the master column

Shipped as **4.13.0**, on top of 2h in the same session.

### Filters read in hertz

"48%" on a corner frequency is a number about the knob rather than about the
sound. The blocker was the firewall — `HPF_HZ` and `LPF_HZ` are DSP constants
and `ivory-ui` cannot name them — so the host now hands over a
`ports::KnobUnit` per knob (`Percent`, or `Hertz { low, high }`) and the UI
does the exponential itself. That is a display mapping, not a filter.

Typing works in the same unit: `knob_typed` accepts `480`, `1.2k`, `800hz`,
clamps a wish that is off the end of the dial to the nearest thing the dial
has, and refuses nonsense rather than reading it as zero.

`a_filter_knob_reads_out_where_its_filter_actually_is` (in `desktop.rs`) checks
the advertised ends against the DSP's own constants **and** the rendered string
against the frequency the sweep produces, at both ends and the middle. A
readout that is confidently, precisely wrong is worse than the percentage it
replaced.

### The master column

To the right of the VU and the knobs: two output ladders, the limiter's gain
reduction beside them, dB readouts under both, and a master knob at the foot.

**It is a different signal from the VU.** `record.rs::meters()` shows what is
being RECORDED — the input when there is one. The new column reads
`Engine::meters()`, which meters the device mix after the effects, after the
limiter and after the master. That method existed and was called from nothing
but tests, so there is no contention over the read-and-clear peaks.

The ladders are segmented rather than smooth on purpose: a lit segment count is
a number you read without reading, and a continuous fill is a length you have
to measure against a scale. Green below -18 dBFS (the digital home of +4 dBu),
amber to -6, red above. **The gain-reduction ladder hangs DOWN from the top**,
in one colour all the way — reduction is not a level and no amount of it is
"good", so colouring the first few decibels green would be a claim about
somebody's music.

`Effects::gain_reduction_db()` is read-and-reset, like the meter peaks and for
the same reason: reduction is a transient a few samples long and the UI asks
sixty times a second.

### The master knob

`master_gain`, unity by default, **last on the instrument bus after the
limiter**, reaching both the device mix and the take — the same rule the
effects follow. Not the click, which has its own fader. It is a fader wearing a
knob: same `fader_to_gain` curve as the other four levels, reads in dB, types
in dB.

Turning it above unity CAN put the output past the ceiling the limiter just
guaranteed. That is what a master fader does on a desk, the meter shows it, and
the alternative — a master that feeds the limiter — is just a second drive
control.

The limiter's cap went red → **bottle green**; red is the master's now, alone,
because the master is the one control here that can undo what the other six
did. `the_limiter_and_the_master_are_told_apart_by_colour` asserts no effect
wears it.

### Two lints that were real

`clippy` flagged an orphaned doc block and a dangling `[`CLICK_SWITCHES`]`
intra-doc link — both left behind when that constant was removed in earlier
work, both predating this branch (checked against `HEAD` rather than assumed).
Removed, along with `TEMPO_ROW`, dead since the tempo became a knob.

---

## 2j. 2026-08-19 (later still) — one gesture set for eight knobs

Shipped as **4.14.0**.

### Right-click types, double-click resets, a tap does nothing

All eight knobs (six effects, tempo, master) now share one set:

| gesture | what it does |
| --- | --- |
| drag | sets the value, relatively, as before |
| right-click | opens the knob for typing, in the knob's own unit |
| double-click | puts it back to its resting value |
| tap | **nothing** |

A tap has to do nothing, and that is not an oversight: the first click of a
double-click is a tap, so a knob that opened a text box under every tap could
never be reset by one. Faders keep tap-to-type — a fader is a long thin thing
you can put a pointer on exactly, and it has no second gesture to protect.

Resting values are "nothing applied" wherever that means something: all six
effects to zero, which for the filters is a corner out at the edge of hearing
and for the limiter is a threshold of 0 dB. The master goes to unity (0 dB) and
the tempo to 120.

**The effect parameter panels moved to shift + right-click.** They were the
plain right-click and there is no third button; between the two, typing a value
is what somebody does mid-take and the panel is what they open once and leave
alone. Each knob's status line says so while a hand is on it. This is the one
judgement call in this batch and it is easy to move.

### The limiter knob is a threshold

`LIMITER_DB = (0.0, -30.0)`, read in decibels, typed in decibels. Zero is the
top and means the limiter is out of the circuit — the same rule every other
knob follows. `limiter_ceiling` is gone from `Params`: the knob IS the ceiling
now, and two ceilings would have been one of them wrong.

**No makeup gain, deliberately.** Something that got quieter because you asked
for it to be limited, then louder again because the limiter decided that is
what you meant, is a control that cannot be reasoned about. The master is two
inches away.

`ports::KnobUnit` gained `Decibels { low, high }` — **linear** in dB, where
`Hertz` is exponential, because decibels already are the logarithm and taking
it twice would put the useful half of a threshold in the last eighth of the
travel.

### The filters are violet, and the GR ladder is gone

The filter caps were ivory, which made two of the eight read as blank caps with
no colour rather than as a pair. Violet sits opposite the panel's warm brown
and competes with nothing else on it.

The gain-reduction ladder had a column of its own for a number that is usually
zero. It is now a wash that fills the master's readout **downward from the
top**, over the same dark recess the ladders sit in, with the output number in
cream on top of it. The number is one colour and it is not red: it has to read
on near-black AND on amber, and the ladder beside it already says whether the
level is a problem. The ladders got the freed width.

---

## 2k. 2026-08-19 (last) — makeup gain, and two bugs worth the space

Shipped as **4.15.0**.

### The limiter got its makeup gain, because it was wrong without it

Asked why lowering the threshold did not make it louder, and the answer was
that 2j built it without makeup on purpose and said so in the commit. **That
was the wrong call.** A threshold on a limiter is a loudness control: what it
takes off the top is given back to everything, so the quiet parts come up and
the loud parts stay at the ceiling. Without makeup it is an attenuator with
extra steps, and nobody would reach for it twice.

`makeup = -threshold_dB` exactly, so the output brickwalls at 0 dBTP however
far the dial is turned. A -1 dBTP delivery is the master at -1, which is the
next knob along.

### The dial runs the other way

`LIMITER_DB = (-48.0, 0.0)`: fully left -48, twelve o'clock **-24**, fully
right 0 dB and out of the circuit. This is the only knob in the band whose
resting position is the top of its travel, and it is not an inconsistency — a
threshold is off when it is above everything.

That inversion is load-bearing in five places, and the tests caught every one:
`Sends::default()` (a manual impl now, `limiter: 1.0`), `Shared::new`'s atomic,
`Effects::quiet()`, `Hit::reset_to`, and the settings default. **`SETTINGS_VERSION`
8 → 9**: a file written before this says `limiter_mix: 0.0`, which under the
new reading is a -48 dB threshold — every take slammed flat on a control the
user never touched.

### Gain reduction moved behind the scale

It was a wash inside the readout box, which needed the box held open to show
anything. It is a thin strip behind the dB scale now, hanging from 0 and
reaching down by however many decibels came off — **read against the numbers
that are already there**, so 6 dB of reduction reaches the -6 and it needs no
readout of its own. The readout box went back to the height of its own number
and the ladders took the space.

### The readout strip went with it

The reduction wash needed a box held open to show anything, so 4.15.0 gave it
one — a dark recess under the ladders with the output level in it. Once the
reduction moved behind the scale that recess was a leftover, and what it was
holding open was ladder. **Removed, number and all.** The scale beside the
ladders is the dB readout; a second one under them was the old meter's shape
outliving the old meter.

The strip is bounded by the WIDEST LABEL (`"-60".len() * ADV * size`) rather
than by the scale column, which is as wide as the gap it was given. It sits
under the ticks; it does not sweep the margin.

### Two bugs

**The DX7's slot fader did nothing.** Reported as "works for VSTs, not for the
built-in", and that is exactly what it was: `render_builtin` ADDS into the bus,
so the FM went straight on with no gain applied while every plugin beside it
went through `mix_in` with its own. The number reached the settings, the engine
and the meter, and never reached the audio. It has a scratch buffer and a
slewed gain now, the same shape as a plugin.
`the_builtin_is_moved_by_its_own_slot_fader` fails with "half gain came out at
1.000 of unity" if the multiply is removed.

**CLIPPED is clickable.** A latch that clears itself is one the performer never
sees; a latch with no way to clear it stays lit all session and stops meaning
anything. `Hit::DismissClip` clears all three latches — the live input tracker
(which is what paints the VU red), the take summary, and the instrument bus's
own atomic. Clearing two of three would be a button that appears not to work.
Its rect exists only while the warning is on screen, because a target with
nothing drawn in it swallows presses meant for the row underneath.

---

## 2l. 2026-08-19 (last) — the backing track

Shipped as **4.16.0**. A third fader under the click and the input: import an
audio file, trim it, and it rolls with the transport.

### Decoding, without a decoding crate

`ivory-record/src/decode.rs`, split per platform the same way `encode` is:

* **Not macOS** — `tangent-ffmpeg`, which the release already ships so video
  works on a machine with nothing installed. One command does format, channel
  layout and sample rate together.
* **macOS** — `/usr/bin/afconvert`, on every Mac ever sold. The mac build has
  no bundled ffmpeg (video is AVFoundation), so the alternatives were shipping
  76 MB of encoder for something that is not encoding, or CoreAudio FFI plus a
  resampler. It writes a temp WAV; `riff_data` **walks the chunks** rather than
  assuming offset 44, because `afconvert` writes an `FLLR` padding chunk and
  the fixed offset lands inside it — which sounds like a track that starts with
  a burst of noise.

Either way out comes interleaved stereo f32 at the device's rate, which is the
only shape the mixer wants.

### Playing

`Shared` carries the gain, the trim (in FRAMES) and a `pending_track` mutex the
audio thread only ever `try_lock`s — the same treatment `pending_voice` gets,
because a clip is a hundred megabytes and the render thread may not allocate.

`Renderer::mix_track` adds it **after the effects and before the master**:
after, because it arrived finished and a reverb on somebody else's mix is not a
thing anybody asked for; before, because the master is the master. It reaches
the tap as well as the device — a take of somebody playing along to a backing
track, with the track missing, is not a take of what happened.

**It rolls only while `Rolling`.** Not the count-in, which is what counts you IN
to the track; not `Finishing`, which is a file flushing after the performance
ended. Starting when the take starts writing is what makes the two line up.

Trim is stored in SECONDS in the settings and converted to frames at the push,
so it survives being opened on a machine whose device runs at a different rate.

### The panel

Right-click the waveform icon. Peak envelope (peak and not mean — a mean is a
grey sausage), the trimmed part lit and the rest dimmed rather than hidden,
draggable handles, and IN/OUT fields that take `12.5` or `1:12.5`. What the
panel prints is what the field accepts, which is asserted.

`MIN_TRIM` stops the handles crossing: an out-point before the in-point is a
track that plays nothing, discovered by pressing Record and hearing silence.

**It does not screenshot.** `IVORY_SHOT_TRACK=open` loads a fake clip, but the
compositor draws BANDS for the video and not overlay panels — the same gap
`IVORY_SHOT_FX` has. The panel is covered by hit-test and layout tests instead.

### Two things moved

Right-clicking the **microphone** icon opens the audio input picker; right
clicking the **waveform** icon opens the trim panel. Both mirror the metronome,
whose right-click sets whether the click lands in the file. The menu entries
stay where they were — nothing was taken away.

The master meter is **one split fader**: thinner ladders side by side with the
dB scale mirrored on BOTH sides, because one scale for a stereo pair means one
of the two channels is always being estimated. The master knob is centred
between the ladders rather than on the column, which carries a scale down each
edge and so is not centred on them.

---

## 2m. 2026-08-19 — Linux hardening, and one piece of advice reversed

Shipped as **4.17.0**, from `docs/LINUX-4.16-FINDINGS.md` — a measured report
off the Void box. Both findings were real and one of them contradicts what this
project had already written down.

### The file picker was dead on a portal-less box

`rfd` on Linux has two backends: xdg-desktop-portal, then a zenity subprocess.
With neither, `pick_file()` returns `None` — **which is exactly what it returns
when somebody presses Cancel**, so the app could not tell it had failed and
silently did nothing. That is every file dialog: the cartridge, the backing
track, the record folder, the plugin folder.

`Dialog::FileBrowser` is the fallback: a directory listing and nothing more, in
the same spirit as `PluginPicker`. The host lists the directory (disk I/O stays
out of `ivory-ui`) and the dialog draws rows. `native_dialogs_work()` decides
which to open, from two facts and no new dependency: does any installed
`.portal` file advertise `FileChooser` — the report's box had a Secret portal
and no file one, so "a portal exists" is the wrong question — and is `zenity`
on `PATH`. Wrong only in the direction that costs nothing: a false negative
opens our browser and still chooses the file.

### The underruns were buffer geometry, not scheduling

cpal's ALSA host reads `BufferSize::Fixed(v)` as the whole RING and divides it
into four. So `Fixed(256)` on Linux is not a 256-frame callback — it is a
256-frame ring refilled every **64 frames**, four times the callback rate the
number was chosen for, on a SCHED_OTHER thread. macOS and WASAPI read the same
call as the period. `BUFFER_PERIODS` multiplies by four on Linux so the two
agree; measured 6 underruns per 30 s before, 0 after.

**And no realtime promotion**, which is the part worth remembering.
`LINUX-4.11-FINDINGS.md` finding 1(c) asked for rtkit. Measured, it made things
an order of magnitude worse — 75 underruns against 6, starting the instant the
thread was promoted — because pipewire's own data loop is already at RT 83 and
a client at FIFO 70 inverts priority against its non-RT IPC thread. It is in
§8's trap list now so it does not get "fixed" again.

---

## 2n. 2026-08-19 — the clip lamp, the take report, and a fullscreen freeze

Shipped as **4.18.0**.

### CLIPPED was two indicators for one fact, and the wrong one worked

There was a `CLIPPED` word in the status strip AND a lamp on each VU face.
On Linux the word appeared and could not be cleared; on macOS **neither** ever
appeared, because `record.rs::meters()` answers `SILENT` when there is no audio
input and no plugin audio — which is a Mac with a piano plugged into it and the
built-in FM playing. The needle sat still and the lamp could never light.

The word is gone. The lamp is the indicator, **pressing the meter is what puts
it out**, and the VU falls back to the engine's own device-mix meters when the
session has no source of its own (`Session::has_meter_source`). Read once, so
the same numbers reach both meters rather than one of them getting the zero the
other's read-and-clear left behind.

### The take's report is a dialog now

It was the last `or_else` in the status chain, so it competed with live errors
in a one-line strip and stayed up until something replaced it. It is
`Dialog::TakeSummary`, raised once per take on the edge, with a **don't show
again** that suppresses the ordinary "recorded 2:14 of audio + MIDI" and
**never** a take that hit a problem — `Summary::is_problem()`, which counts a
silent take, because that is the failure nobody notices until they open the
file. It also never replaces a dialog already open.

### The preview is the camera picker

Left click, at rest, on the picture — which now says `NO CAMERA` / `Select
Camera` rather than pointing at the cog, a direction that had already stopped
being true twice. `Layout::camera` is gone; only `audio` is left as a
never-positive target.

### Choosing a file from fullscreen froze the app on Linux

Not a hang: `rfd::pick_file` blocks the main thread, and under i3 a fullscreen
window sits above everything — including the modal panel it was waiting on. The
app was frozen on a dialog that had opened underneath the window and could not
be seen, focused or dismissed, which is exactly "hangs with no console
messages".

`picker_needs_windowed()` drops fullscreen, defers the picker by one frame — the
change needs a frame to reach the WM — and restores fullscreen after, on cancel
as well as on choose. Linux only; macOS and Windows put a panel in front of a
fullscreen window themselves.

**Not verified here.** This one is reasoned from the symptom and the platform,
and there is no Linux box on this end to reproduce it on. It is the next thing
to confirm on dresden.

---

## 2o. 2026-08-19 — the fullscreen freeze, diagnosed on the box

Shipped as **4.19.0**. Three fixes; the first was reproduced and verified over
ssh on dresden rather than reasoned about, because 4.18.0's attempt at it was
reasoned about and made things worse.

### It was our own dialog, buried by the previous fix

The box has **no FileChooser portal and no zenity** (checked: only
`gnome-keyring.portal`), so `native_dialogs_work()` is false and the picker is
`Dialog::FileBrowser` — a CHILD VIEWPORT, a second X11 window. 4.18.0 left
fullscreen, opened it next frame, and then on the frame after found no pending
request and put fullscreen **straight back**. Under i3 a fullscreen window sits
above everything, so the browser was buried the instant it appeared; and
`app.rs` returns early from all main-window input while a dialog is open, so
`Z` did nothing either.

Not a hang. A modal nobody could see, with the keyboard locked out — which is
why force quit was the only way back and why nothing appeared in the console.
`restore_fullscreen` now waits for the dialog to close.

**Verified on the machine**, not inferred: fullscreen → click import → the main
window leaves fullscreen and "Choose a backing track" appears as a normal
window → Cancel → `_NET_WM_STATE_FULLSCREEN` is back → `Z` toggles it off. The
whole import path was driven through as well (navigate to `~/Music`, choose a
`.wav`, decode, waveform, IN/OUT panel) — the first end-to-end proof of the
backing track on Linux.

**How to do this again.** `ssh void`, `DISPLAY=:0`, `xdotool` to drive and
`import -window root` to see. Two traps cost time: `pkill -f <name>` matches
the ssh command itself and kills the session, and `xdotool key --window` uses
`XSendEvent`, which winit ignores — activate the window and use plain
`xdotool key` (XTEST).

### The clip lamp is about the signal path

With an input selected the VU meters the INPUT, so a built-in FM driven into
the ceiling clipped the output and lit nothing: "choose a mic and clipping is
not possible", on both platforms, one cause. The needle still answers one
question; the lamp beside it ORs the engine's device mix in now.

The "hot mic" theory was wrong and measuring killed it: dresden's input peaks
at **-16 dBFS**, so the mic was never the clip.

### The tempo knob was never relatively draggable

`Hit::with_value` had no arm for `SetTempo`, so every frame of a drag
re-applied the hit the PRESS produced — which is absolute. The knob jumped to
wherever it was first touched and then would not move. Pre-existing; the other
seven becoming consistent in 4.17.0 is what made it visible. The test asserts
every draggable control actually carries the value a drag hands it, which is
the general form of the bug.

---

## 2p. 2026-08-19 — the menu in compartments, no detach, and Setup

Shipped as **4.20.0**. A tidy-up, a pivot, and one thing put back that should
never have left.

### The menu is four compartments and nine rows shorter

It was twenty-five hovers: every subject the app has, whether or not the thing
it configures was on screen. It is now blocks joined by separators — what is
true everywhere, then the piano's, the theory band's, the guitar's and the
recorder's, each present only while its surface is.

What was deleted: **Show/Hide Note Names, the Recorder block, Sources, Time
signature, Count-in, the Keyboard block, Dark Mode, the typeface, and the
Theory toggles.** None of it is a feature loss — every one is a bound key (`U`,
`V`, `D`, `F`, `1`-`4`, `T`, `K`, `P`, `C`), and `no_menu_row_does_what_a_key_
already_does` asserts BOTH halves: the row is gone *and* the binding still
exists. Delete a binding from `keys.rs` and that test tells you the feature now
has no way in at all.

The line the deletions were drawn on: a row that only flips a switch is a toll
paid every time you open the menu; a row that opens a dialog you then have to
fill in is the front door somebody finds the feature through. So Select MIDI
Input, the teach block, Colors and Plugin folders stayed, and moved up.

Two categories are NOT in the owner's list and were kept anyway — **Chords**
and **Plugin folders**. Neither was named obsolete; Plugin folders has no key
and cannot have one ("my plugin is not in the list" is answered by a rescan, a
folder, or starting over). Say so when reporting: keeping something unnamed is
a decision, and it is the owner's to reverse.

`Key` is now offered whenever ANY theory diagram is up, not just the notation —
see below. `Follow MIDI` is the one theory row with no key of its own, so it
stayed; deleting it would have left no way to stop the band chasing the piano.

### The key signature drives the harmonic triangles

`draw_triangles` anchored I, IV and V on `input.tonic()` — whatever you had
just played. That made the one diagram with roman numerals on it the one that
could not tell you where you were: every chord was I, because the picture slid
under it. It is anchored on the KEY now (`key_tonic(s.staff_key)`, which is
`key` fifths from C), the black keys are spelled the way the signature spells
them, and the ring still marks the chord you are actually playing — so you see
that what you played *is* the IV.

`hit_test` lost its `Input` parameter in the same change, which is a stronger
guarantee than the test it replaces: a hit test that cannot see what is playing
cannot answer for it. The Circle and the Tonnetz are unchanged.

### Nothing detaches any more

Four surfaces could be popped into their own window. All four are retired: the
menu rows are gone, and `IvoryApp::new` clears all four `*_detached` flags
UNCONDITIONALLY on the way in. That clamp is the important half — the flags
persist, so an upgrade would otherwise zero a band, put it in a window nothing
opens, and offer no Attach row to undo it. That failure is not hypothetical; it
is what a plugin instance seeded from the desktop's settings file used to do,
which is what the clamp was originally written for when it was gated on
`caps.detachable`.

The viewport plumbing is left in place and inert — the owner's own words were
"remove (or make inactive)", and deleting ~1000 lines of window code is a much
larger, riskier change than the pivot asked for. `nothing_can_be_detached_any_
more` asserts absence over every `Caps` and both detached states.

### Setup came back, and had been unreachable for a release

**Read this one before touching the take-settings panel.** 4.19.0 moved the
camera and the audio input out of that panel onto the controls they feed, and
took the AUDIO STATUS row out with them — but did not move it anywhere.
`Hit::ShowAudioStatus` kept its variant, its tooltip and its arm in `app.rs`,
and had no rectangle in any layout. The panel that says what rate the two
streams are running at could not be opened from anywhere in the app. Nothing
failed, nothing warned, and it stayed that way for a release until the owner
noticed it was missing.

`every_take_settings_control_can_actually_be_clicked` is the guard: it walks
`SetupLayout::targets()` rect by rect through `setup_hit_test`, so a Hit with
no rectangle fails rather than disappearing.

It is now the seventh row of the take settings and it is **Setup**, holding
everything about the audio path rather than only reporting it:

- **SYSTEM** — `cpal::available_hosts()`, persisted by NAME in `audio_system`.
  Usually ONE entry, and the panel says so rather than pretending: cpal
  compiles in a single host per platform unless a cargo feature asks for more,
  and both extras (JACK, ASIO) need a development library present when the
  RELEASE is built. Turning either on is a change to what the release scripts
  can produce, not a change to this code. The chooser is real either way, and
  `ivory_record::audio::SYSTEM` is process-global because a `cpal::Host` is not
  `Sync` and cannot be held.
- **SAMPLE RATE** — `record_sample_rate`, applied to BOTH streams. The list is
  the six familiar rates intersected with what the input device actually
  reports (`audio::input_rates`), because a rate offered and then refused is a
  "could not open" every time somebody tries the biggest number. The output
  narrows through one of its own ranges and silently keeps its default if it
  cannot match — losing the app's sound over a preference would be worse than
  the mismatch the panel already warns about.
- **BUFFER SIZE** — unchanged, and the model the other two follow: written to
  settings, acted on by the host on an edge, never mid-take.
- **INPUT CHANNELS** — the multichannel answer. `ConfigWish::channels` already
  documented why this was needed: cpal numbers channels from the device's
  FIRST input, so asking an 18-in interface for two channels records inputs 1
  and 2 and silently ignores a piano plugged into 3. So the device is opened
  with everything it has and one channel is taken at
  `CaptureSource::accept` — the single point where the interleaved callback
  buffer enters the app. Everything downstream sees a mono stream and knows
  nothing about it, including `OpenConfig.channels`, which must be the SINK's
  count or the WAV declares 18 channels over mono samples.

The device half is a "Change..." button that opens the existing microphone
picker. One device chooser, not two lists of the same hardware to keep in step.

**The channel uid grammar** is `<device key>\u{1f}<channel>`, and the separator
is the argument: `#` is taken and escaped by `DeviceKey`, and `|`, `:` and `@`
all appear in real interface names. A C0 control appears in none, serialises as
`\u001f`, and is invisible in `settings.json`.
`a_channel_uid_round_trips_through_a_hostile_device_name` is what makes that an
argument rather than a hope — a uid that splits wrong resolves to a device that
does not exist, and the saved microphone then silently reverts every launch.

### What was verified, and how

The channel pick and the panel are both covered by tests that were *proved to
fail without the fix* — the stride bug (`* ch` instead of `* ch_in`) and the
missing target were each reintroduced and the tests went red.
`the_setup_panel_opens_and_draws_and_its_controls_act` drives real egui frames,
and a `panic!` planted in the panel body confirmed the body actually runs, so
it is not a vacuous "no panic" test.

**Not verified on hardware:** no multichannel interface was available, so the
channel rows have never been seen listing a real 18-in device, and no build has
more than one audio system to switch between. The DSP is unit-tested; the
enumeration is not.

---

## 2q. 2026-08-19 — the clip lamp that could not be reset

Shipped as **4.20.0**. Diagnosed by the owner on the Linux box, in a report
that reproduced it, filmed it at 60 fps and named the mechanism correctly
before any source was read. Three fixes, all of them theirs.

### Two latches in series, and Reset only cleared the downstream one

`Session::clear_clip` cleared the `AudioMeters` behind each mutex. Those are a
published **copy**. The original lives in the `LevelTracker` on the writer
thread, and `Writer::pump` copies it over the published one every cycle — about
every 4 ms — so the dismiss was undone before a single repaint could show it.
Not "usually undone": a 60 fps capture of two clicks caught **zero** dark
frames.

The symptom looked absurd and was exactly right: the lamp cleared only while NO
input was selected, because unselecting one destroys the thread that owns the
surviving latch. Starting a take did not clear it either — `arm()` clears the
TAKE's tracker, which `take.json` proves is a different object.

The comment above `clear_clip` said "All of them, or the light does not go
out", listed three latches, and was clearing the wrong three things. A comment
that names the invariant is not the same as code that holds it.

**Fix:** `Cmd::ClearClip` on the writer command channel, which both writers
handle. A command rather than a shared flag because the channel already exists
and the tracker lives on the writer thread, not in the audio callback — there
is nothing to make lock-free. The copies are still cleared synchronously,
because that is what makes the lamp go dark on THIS frame rather than at the
next poll; the command is what makes it stay dark.

Both `match` arms are exhaustive over `Cmd`, so a third writer cannot be added
without deciding what ClearClip means to it. That is the only structural guard
here — see the verification note below.

### A converter warming up is not a performance clipping

Second finding, same report: moving the system default source under an open
stream, in a way that forced a rate-and-channel converter swap (44.1k/2ch ->
48k/1ch), latched both lamps on a source that was **silent**. A move with no
format change stayed clean. One garbage buffer during warm-up is enough — and
with the latch un-clearable, that phantom was permanent. It is very likely
where "it says I clipped" came from for people who had not.

`LevelTracker::warm_up(ms)` holds the LATCH off for the first
`CLIP_WARMUP_MS` (50 ms) of a freshly opened stream. Peaks and RMS are
untouched throughout: a meter that went blind for 50 ms would be a worse lie
than the one this fixes. Opt-in at the site that knows a stream just opened, so
a tracker fed from a buffer in a test still latches from the first sample.

### The Linux input path never got the macOS rework

The "1 VU on macOS for a mono mic, 2 on Linux" observation is a real clue and
not itself a bug. Linux opens `default` through cpal/ALSA, which is
plug-routed: pipewire-alsa negotiates 2 channels and upmixes a genuinely mono
source. Verified on the box — a 1-channel 48 kHz virtual source came up as
`float32le, 2ch, front-left/front-right, 44.1 kHz`. Asking PipeWire for the
source's true channel count is almost certainly not worth it. What the clue
established is that the Linux input path did not get the metering rework the
macOS one did, which is where the reset had accidentally been fixed.

### One line that would have halved the investigation

The Linux build said **nothing** when a capture stream opened, even at
`IVORY_LOG=debug`, so establishing what the device actually negotiated took
inspecting the PipeWire graph from outside the app. There is a `debug!` at the
open now: device, channels, rate, format, buffer. What is ASKED for is nothing
like what arrives, and every future field report that mentions channels or rate
starts from this line.

### What is and is not verified

The trap is unit-tested where it lives —
`clearing_a_published_copy_is_undone_by_the_next_publish` asserts the
republish-over-the-clear explicitly, and it fails if the source is not cleared.
The warm-up has its own test, including that the meter still MOVES during it.

**The plumbing is not covered end to end.** Click -> `Cmd::ClearClip` ->
writer -> tracker needs a real capture device, which no unit test may open.
The owner's report carries a re-verification recipe (a PipeWire null sink fed a
full-scale square, so the input is guaranteed to clip with nothing audible and
no hardware involved); the pass criterion is row 3 of their matrix: **input
selected, stream open, source silent, click clears the lamp and it stays
clear.** Bonus check is row 8 — a live default-source switch across formats
should no longer latch a phantom.

---

## 2r. 2026-08-20 — the takes that never had the instrument in them

Found by the owner on the Linux box, from twelve take manifests: **every take
ever made on that machine said `sources: input` and `plugin: null`**, across
4.4.1, 4.17 and 4.20. The built-in DX7 was audibly playing, the monitor played
it, the meters moved, the `.mid` captured every note — and the `.wav` and the
video's audio track had the microphone and nothing else.

It is not a Linux bug. There is no `cfg` anywhere on this path.

### Two independent reasons, both silent

**The recorder tap was taken in exactly one place: the success branch of
loading a VST3.** `desktop.rs`'s built-in branch calls `set_builtin_slot` and
returns before reaching it. So with only the built-in loaded there was no tap
at all — nothing on the instrument bus had a path into the file. The backing
track and the click-into-take rode the same missing tap.

The tap is now taken when the ENGINE starts, which is what its own comment
already claimed ("the tap belongs to the engine rather than to any one
instrument"). `take_recorder_tap` is `Option::take`, so the VST3 branch's call
is now a harmless no-op and is left where it is.

**And `TakeSource::resolve` asked the wrong question.** It took
`plugin_loaded`, which came from `Engine::any_plugin_loaded` — VST3 slots only.
The built-in is a sentinel path that never writes `Engine::loaded`, so `auto`
saw no instrument, resolved to `Input`, and left the bus out even once a tap
existed. `Engine::any_instrument_loaded` now counts the built-in, and the
caller passes that.

### The backing track was the same bug found a second way

Reported the same evening: a take made while a backing track played contained
the player and not the track. Same cause — the track is on the instrument bus,
`resolve` did not know the bus had anything on it, and the bleed into the
microphone made the file sound *almost* right, which is the version of this
failure that survives a listen.

`resolve` now takes `track_loaded` as well, and everything downstream asks one
question: **is there anything on the instrument bus** — a VST3, the built-in,
or a backing track. Those were the same question until the built-in and the
track arrived, and every arm of that match was written when they were.

`Engine::track_loaded` is set in `Shared::set_track`, on the UI thread, not
when the renderer picks the clip up: a take is armed before the transport
rolls, and a flag set on the audio thread's next buffer is false for anybody
who loads a track and presses record in the same frame.

### The lesson

Three features reached the instrument bus over three releases and each one
assumed the plumbing that carried the first. **A predicate named for one
implementation of a thing (`any_plugin_loaded`) becomes wrong the moment a
second implementation exists, and nothing fails — the monitor still plays it.**
`anything_on_the_instrument_bus_is_worth_recording` asserts every case that
produced a wrong file, and fails if `bus` narrows back to `plugin_loaded`.

### The backing track's panel was shouting

Same round, unrelated: it is the widest panel in `recorder_panel.rs` — 720
points against an effect panel's 300 — and every piece of text in it was a
fraction of rows derived from that width. Title at 24 points, trim readouts at
33, against a band whose own readouts are 11 to 14. Sizing text off its own box
is right until one box is unusually large.
`the_backing_track_panel_is_sized_like_the_rest_of_the_app` asserts POINTS at
real window sizes, not the fractions, because the fractions are what went
wrong: each one looked reasonable against its own row.

---

## 2s. 2026-08-20 — the potato pass

Five findings from the owner's Linux box (2013 MacBook Air, 2 cores at
1.8 GHz, HD 4000 with no Vulkan driver), all root-caused there and fixed here.
The goal in their words: **"potato running like new".**

### The mic fader was connected to nothing

`gains.input` was packaged by the settings, drawn on the fader, written by the
drag, and read by NOBODY in `ivory/src`. `push_monitor_settings` pushed slots,
metronome, master, track gain and trim, and stopped.

It goes to the SESSION, not the engine: the engine has no input in it. Applied
on the writer thread to the input block before the tracker absorbs it, so the
fader moves the meter and the file together. Slewed per FRAME — stepping a pole
across an interleaved buffer applies a different gain to the left and right of
one frame, which swings the image while the fader moves.

### The camera never slept

35.6% of a core, measured, with no take rolling and the pane hidden — on the
machine that then dropped half the frames of a take. The conversion is the cost
(a 720p JPEG decode) and it was being paid thirty times a second for a preview
box a few hundred points wide.

The RATE is now demanded by whoever is looking: a take gets every frame, a
preview ten a second, a camera with nothing on screen none at all. The dequeue
still happens either way or the driver backs up.

Two bugs the tests caught rather than review: "converted at time zero" and
"never converted" were the same value, so a platform whose stamps start near
zero skipped its own first frames; and a strict minimum spacing is never
satisfied on the camera's own 33.3 ms grid, so a request for ten a second
delivered seven and a half. A frame that is nearly due counts as due.

### No GPU driver, so lower the defaults

`wgpu::AdapterInfo.device_type == Cpu`, probed once and logged. 15 fps joins
`FPS_CHOICES`. The lowering is a DEFAULT and behaves like one: once ever, only
from the shipped values, never against somebody who has been to the Export
dialog.

### The readback was the note lag

**Read this before touching `composite.rs`.** The module said the synchronous
readback "costs a few milliseconds at 1080p" and that hiding it would buy a
class of bug for no gain. That measurement was taken on a machine with a GPU.

`device.poll(Wait)` does not wait for a copy. It waits for the whole
submission — and where the adapter is a CPU rasteriser, that is the entire
rasterisation of the frame, on the UI thread, thirty times a second. Note input
enters through egui's event handling, which was queued behind it. That is the
whole of "the take was unusable".

The readback is now one frame behind: submit N, hand back N-1, which has had a
frame interval to finish. Two buffers, ping-ponged, and **`flush()` at the end
of the take** — forgetting it truncates every take by one frame, which is
exactly the class of bug the old comment was avoiding, now named and asserted.
The `pts` travels WITH its frame rather than being recomputed on the way out.
`the_pipeline_gives_back_every_frame_once_and_in_order` fails if it does not:
reintroducing that bug shifts the whole video by one frame with a duplicate at
the end, which is what it printed when it was checked.

Also: `stride`. A pump that overruns its budget composes every 2nd or 4th tick
and pads the rest, instead of spending the whole UI budget every pump and
leaving the window at four frames a second. Input has priority.

**Not done: moving the compositor to a worker thread.** The paint pass needs
`&IvoryApp`, which is not `Send`; the split would be UI-paints-shapes /
worker-renders, and the shapes are `Send` so it is possible. It was not needed
once the wait was gone, and it should be re-measured before it is attempted.

### ALSA cannot see an interface on a PipeWire machine

The picker offered one input called `pipewire`, which follows the desktop's
default source — so a Scarlett was plugged in and the laptop's own microphone
was recorded. cpal is not at fault: it walks the real cards and opens
`plughw:N`, and PipeWire holds every one of them exclusively. Measured:
`arecord -D plughw:1` says **"Device or resource busy"** for the Scarlett while
the built-in opens fine, and which of the two is busy depends on what PipeWire
is routing at that moment.

So the list comes from `pw-dump` and the binding from `PIPEWIRE_NODE`, both
verified on the box before any code was written. **Not `pipewire-rs`**: it
links libpipewire and needs its headers at BUILD time, and the Linux artifacts
are cross-compiled from a Mac — that would have traded a working release
process for a tidier lookup.

`PIPEWIRE_NODE` must wrap the ENUMERATION, not just the stream build: cpal's
ALSA host opens each device's PCM handles while listing them and the stream
reuses the handle that is already open.

### Input monitoring, and the one thing it may never do

Right-click the microphone icon; a record-red dot while it is on. **It is never
persisted anywhere.** Not a setting cleared on load — that is one migration
bug, one refactor or one hand-edited file away from coming back on — but
session state with no path to disk. The owner's requirement: *"if I forget and
turn my speakers on and relaunch, I get a head full of feedback."*
`input_monitoring_never_survives_a_relaunch` asserts the absence, including
that a file made to claim otherwise cannot turn it on.

The capture pushes into a shallow ring of its own (120 ms, not the take's four
seconds) which the renderer drains EVERY block whether or not anybody is
listening — so switching on plays now rather than a second of backlog.
Listen-only by construction: the take comes off a different ring.

---

## 2t. 2026-08-20 — the menu that opened the wrong submenu, and two Windows-only flashes

Shipped as **4.23.0**. Two bug reports and two changes the owner asked for
alongside them.

### Reaching for a submenu opened a different one, most often the top

The owner, on every platform: "main context is fine, but as soon as you try to
select a submenu, things get wonky and it sometimes just jumps to the top
option." This is the third attempt at this bug and the first one that is
actually about the mechanism rather than about the symptom.

**What was wrong, in two parts.**

1. `settle_submenu` had a branch that opened a submenu INSTANTLY when nothing
   was open yet, reasoning that with no panel showing there is no journey to
   protect. But the menu opens UNDER the cursor with nothing open, so "the
   first one" is whichever row the menu happened to land beneath — and
   `ARM_SLOP` is six points, less than half a row. Six points of movement in
   any direction and the top row's submenu was up. That IS the jump to the top.

2. `still` came from `input.pointer.velocity()`, which is not stillness.
   `egui/src/input_state/mod.rs` computes it as `Vec2::ZERO` until three
   positions have been sampled over at least ten milliseconds, and clears the
   history outright on `Event::PointerGone`. So it reads "stopped" for the
   first two frames of EVERY gesture, and again every time the pointer crosses
   between the menu window and its panel — which on the desktop is two separate
   OS windows with two separate pointer streams. The one moment a menu must not
   act on what it is over is exactly the moment egui said the pointer had
   arrived.

**The fix.** `note_rest` keeps the pointer's own anchor and timestamp: still
means "within `REST_SLOP` (one point) of one place for `REST_FOR` (60 ms)". A
hand pushing a mouse crosses one point per frame at a hundred points per second,
so a travelling pointer can never accumulate the time, and a gap in the samples
cannot fake it. `settle_submenu` then treats opening, switching and CLOSING as
one thing: the wanted state is remembered and commits on a rest or after
`SUB_SWITCH_DWELL`. Nothing is instant except a CLICK on a category, which is
new and is the deterministic escape hatch.

Closing waits now too, and that is not a detail: a plain row used to shut the
panel on the frame it was crossed, so travelling from one category to another
past an ordinary row DESTROYED the panel's window and built it again — see the
next section for what that costs on Windows.

Verified on the real app with `cliclick`, not only in tests: right-click, travel
down nine rows to Capo (Capo opens, nothing else did), move right into the panel
and down to Fret 3 (the panel stays), travel back up nine rows to Colors (Colors
opens). Screenshots at each step.

### Windows: white flashes, and two independent causes

Reported by a tester, Windows only, "app seems to MOSTLY work, but intermittent
white flashes occur when using it". Neither cause can be seen from a Mac or a
Linux box, which is why both had survived.

**1. Every subprocess opened a console window.** The app is
`#![windows_subsystem = "windows"]`, so it has NO console — and Windows gives a
console-subsystem child one of its own, with a window, unless the parent passes
`CREATE_NO_WINDOW`. ffmpeg is a console program. So starting a video take, muxing
one at the end, and loading a backing track each put a console on the screen.
Redirecting the child's handles does not help; the console is allocated at
process start regardless of where they point.

`ivory_record::proc::command` is now the only way this workspace spawns anything,
and `proc::tests::nothing_spawns_a_child_the_long_way_round` is a SOURCE SCAN
that fails the build if `Command::new` reappears outside test code. A scan
rather than a behaviour test on purpose: the bug is invisible on the two
platforms this is developed on, so neither a reviewer nor a running test can
catch it.

**2. Child viewports were born visible.** eframe starts its own main window with
`with_visible(false)` and reveals it after the first frame — the comment in
`eframe/src/native/glow_integration.rs` says, in as many words, "to fix white
flash on startup". A child viewport gets no such treatment, and this app's menu,
submenu panel and every dialog ARE child viewports that appear and disappear
while somebody is using it. `shell::surface` now creates them hidden and reveals
them on the next pass, tracking `(born, last drawn)` pass numbers in egui memory
— consecutive calls differ by exactly one pass, because not drawing a surface is
what destroys it, so a gap is a reliable "this one is new".

A surface that asked for focus is sent `ViewportCommand::Focus` on the reveal
pass. `with_active` is a creation-time attribute and an invisible window is not
one anybody can focus, so without this a dialog could come up unable to hear
Escape.

**Still unproven, and say so when reporting:** neither fix has been seen to work
on Windows, because there is no Windows machine here. The console-window one is
certain in mechanism; the viewport one is the documented cause of the same
symptom in eframe's own code. If flashes remain, the question to ask the tester
is WHEN — while a take runs (ffmpeg), or when a menu or dialog opens (viewport).

### The channel chooser is a set of tick boxes, not a choice

The owner: "the channel selector MUST be a checkbox — On the left downwards,
Mono Inputs to check. On the right downwards, Stereo to check — 1/2 2/3 3/4 etc
and when selected — the chosen ones — EVEN IF MULTIPLE e.g. i selected mono 6
and 1/2 4/5 must be exposed in the INPUT SELECTOR."

4.22.0's chooser answered a different question: WHICH input to record, one at a
time, hidden inside the selected device. That is wrong about the hardware. An
interface's inputs are not alternatives — the piano is on 1/2 and a room mic is
on 6, and which one a take wants changes between takes. So they are microphones,
and microphones belong in the microphone picker.

`Selection::exposed` is a set of channel uids spanning every device.
`AudioInputs::list` follows each device it can see with a row per exposed input
(`with_exposed`), named by `with_channel` — the same function the band's own
label uses, so the picker and the band can never word it differently. The
chooser is two columns of tick boxes: MONO 1..n down the left, STEREO 1/2, 2/3,
3/4 … down the right, any number ticked at once.

Two things worth knowing before changing it:

- **Unticking the input that is OPEN falls back to the whole device**
  (`set_exposed`). The row leaves the list the moment the box is cleared, so a
  selection left pointing at it points at nothing — and the difference is a
  silent take nobody notices until playback.
- **Other devices' boxes are never touched.** The chooser only ever shows one
  interface; a panel that quietly forgot the others would lose a setup every
  time somebody swapped a box over.

The uids are stored verbatim in `record_input_channels` and handed straight back
to `devices::restore` at startup, exactly as the chosen device's own uid already
is. The grammar stays on the host's side of the firewall; `ivory-ui` cannot
spell one and must not learn to.

`IVORY_INLINE=channels` opens it against a fake eighteen-in interface, because
the chooser only offers itself above two inputs and nobody developing this owns
such a box.

### The "Default" tick beside the folder was wired to itself

The owner asked what it did. The answer is nothing: `set_record_dir` wrote
`record_dir` unconditionally and `Settings::record_root` read it regardless, so
the folder was remembered whether the box was ticked or not. Its doc comment
described behaviour that did not exist. Removed — the setting, the `Hit`, the
`MenuAction` and the `remember` parameter threaded through the host — and the
FOLDER field took the width it was using (`0.00..0.79` where it had `0.00..0.54`).

---

## 2u. 2026-08-20 — the desk, and four bugs on the way to it

Shipped as **4.24.0**. The mixer plan, stages one and two, plus everything
that was reported while it was being built. The plan itself is an artifact the
owner has: routing, several inputs from one interface, and a home for user
effect plugins, staged.

### The effects are a bus now, and the limiter is on the master

**Read this before touching `Renderer::render`.** Six knobs used to be an
insert on the instrument bus, with the backing track and the input monitor
joining downstream of them — so nothing but the instrument could be
reverberated, and the limiter never saw the two sources most likely to clip
the output it was protecting.

They are two things now. **Reverb, delay and chorus are a BUS**: each strip
sends a percentage of itself and what comes back is added at the bus's own
fader. **High-pass, low-pass and the limiter are an INSERT on the master.**
That split is not a preference — only three of the six were ever wet amounts;
the other three are a corner frequency, a corner frequency and a threshold,
and a send knob into those is not a question anybody could answer. It is
visible in `effects::Sends`, whose name has been a misnomer since it was
written.

**The bug this nearly shipped with, and the one to remember.** Every effect in
`effects.rs` is ADDITIVE — `out = dry + wet * knob`, never a crossfade — which
is right for an insert and catastrophic for a send: a bus is handed a COPY of
signal that already reaches the master by another route, so returning the dry
with it adds that signal twice. Up to six decibels and a comb filter, at every
setting, which reads as "the reverb makes everything strange" rather than as a
routing mistake. `Effects::new_send` subtracts the dry from the sum at the end
— the chain still RUNS on it, because the reverb has to be fed something and
with the chorus at zero there would be nothing — and returns silence rather
than its input when nothing is switched on.

The defaults are the old routing written down: instrument sends everything,
nothing else sends anything, and `wet_only(x)` is exactly `insert(x) - x`, so
an upgrade with the mixer untouched is arithmetically the same signal. What
does change: the backing track and the microphone now pass through the
master's filters and limiter. That is the fix, not a side effect.

### The mixer is a view, and the band did not move

Tab swaps the piano, the theory band and the neck for six channel strips. The
recorder band stays exactly where it is, so the transport and the record
button are in the same place in both and you can cross over mid-take. The
window does not resize: the layout still asks for what the bands asked for, so
Tab changes what is drawn and no geometry at all.

**The band is the rack and the mixer is the routing.** That division is why
nothing in the band had to move: the six knobs stay where a hand reaches for
them, and the mixer draws none of its own. It shows what they never had — who
feeds them, how much, and what is heard.

Nothing in the mixer is a second copy of anything. A mixer fader and the
band's fader are ONE value in settings that both read
(`the_two_surfaces_move_one_value`). The master takes no send and cannot be
muted; the effects return takes no send either, because a bus that can feed
itself is a bus that howls.

`IVORY_INLINE=mixer` opens the view on the first frame. Use it: photographing
the view otherwise means sending a keystroke at a desktop that may not be
listening, and a stray keystroke on a shared machine lands somewhere else.

### A monitored microphone was being written twice

Monitoring puts the live input into the mix and the mix is what the tap
carries, so a take whose sources were `both` wrote the microphone from the
capture ring AND again inside the bus. Six decibels up and comb-filtered
against itself by the monitor ring's latency.

`TakeSource::resolve` takes `monitored` now and keeps the BUS when it is set,
which is the owner's rule made literal — if the effects were audible while it
was recorded, they are in the take. `Input` is left alone: somebody who asked
for the microphone ALONE cannot be handed a wet one, because the bus is a mix
and the input's share cannot be taken back out of it.

`mix_monitor`'s doc said the opposite of all this and had been wrong since the
day it was written.

### A plugin says whether it plays notes

The rack lists every VST3 on the machine, so a Pro-R or VintageVerb owner
loaded one, waited a second and got silence — a slot is fed MIDI and an effect
has no notes to play. The app could not tell the two apart:
`PClassInfo::category` is "Audio Module Class" for a synth and a reverb alike,
and `subCategories` is a `PClassInfo2` field behind `IPluginFactory2`.

Two rules keep the fix from doing harm. **Instrument wins a tie** — samplers
with an audio input declare `Fx|Instrument` and genuinely do play notes — and
**silence is not a no**: a factory too old to be asked tells us nothing, and
reading nothing as "not an instrument" would refuse plugins that have always
worked.

Refused at `Instance::create`, not filtered out of the picker, and that is a
cost decision worth keeping: the picker is a filesystem walk that never opens
a bundle, so labelling the LIST means reading `subCategories` for every VST3
on the machine — a scan with a cache and a crash boundary, which is a
subsystem rather than a check.

### The camera preview was capped at ten a second, everywhere

4.20.0's potato pass stopped the capture thread converting thirty frames a
second for a preview nobody frames a shot in — and picked ten a second for
every host, from a 2013 MacBook Air's JPEG decode. Only V4L2 decodes JPEG; on
macOS a conversion is a BGRA-to-RGBA copy costing a fraction of a millisecond,
so the cap bought nothing and spent two thirds of the preview on it.

The host says WHO IS LOOKING now (`FrameWant::None | Preview | Every`) and how
fast a preview can be is measured where the conversions happen, as a share of
one core scaled by core count and floored at twelve percent — which is exactly
what that Air was hard-coded to. The smoothing is asymmetric on purpose: an
eighth of the way up, half the way down. A preview that dips for a moment
costs nothing; one STUCK slow after the machine went quiet is the bug.

### Two DX7 reports

**No way back to the shipped bank.** An empty `dx7_cartridge` is what SELECTS
it and only `load_cartridge_at_launch` ever read it, so loading somebody
else's was a one-way door. There is a Factory button, offered only while there
is a cartridge to leave. `CartridgeInfo::factory` says which bank is loaded,
because the shipped one has a NAME like any other and an empty name never
meant "the factory bank".

**The file panel opened behind the instrument window**, and it is not about
sysex. Every dialog is created always-on-top so a modal one cannot end up
buried behind the main window — an app that has silently frozen, fixed twice
already — and the OS's panel is parented to the MAIN window, so nothing can
put it in front of a floating dialog. `SurfaceSpec::on_top` is a flag now, and
it goes false for exactly one thing: `native_panel_up`, set when a panel is
asked for and cleared by the HOST when it closes, including on cancel. The
host holds the request back one frame first, because a window level changes on
the frame AFTER the press.

### What is NOT done

Stages three and four of the plan, and they are releases of their own.

**Stage 3, inserts.** Each strip's insert holds a user effect plugin, which is
what makes Pro-R work rather than merely explain itself. The host already
instantiates VST3s and `Instance` already activates audio inputs; what does
not exist is a per-strip processing path, its latency reporting, and the
load-on-a-worker-thread swap protocol that `Engine` has for instrument slots.

**Stage 4, several strips from one interface.** Cheaper than it sounds on the
capture side — the callback already receives every channel and `ChannelPick`
already knows how to take one or a pair out of them; today exactly one pick is
kept and the rest of the buffer is discarded. It is NOT as cheap as first
claimed once followed through: with N input strips, what `TakeSource::Input`
means has to be answered, and the honest answer is probably "the bus", which
is a second decision.

**A second interface is declined, not deferred.** An aggregate device presents
as one device with one clock, so anyone with that rig arrives at stage 4's
cheap case anyway. The feature would be a worse version of what the OS does.

---

## 3. Repo layout

```
ivory/
├── Cargo.toml              workspace (ivory-core, ivory); version 2.1.0, MIT
├── ivory-core/             ── PURE ENGINE, no GUI deps, exhaustively tested ──
│   ├── src/
│   │   ├── lib.rs          public API re-exports
│   │   ├── patterns.rs     chord/scale tables (ORDER IS LOAD-BEARING)
│   │   ├── detector.rs     the scoring pipeline (all the hard logic)
│   │   ├── naming.rs       note naming / display formatting
│   │   ├── overrides.rs    teach layer (exact overrides + learning feature)
│   │   ├── fretboard.rs    guitar geometry + every place a pitch can be played
│   │   └── voicing.rs      which of those places lights up (§2d; ALL dials in `Weights`)
│   ├── tests/
│   │   ├── acceptance.rs   THE CONTRACT — 200+ (notes → name) vectors, both prefs
│   │   ├── differential.rs corpus regression guard (classifier writes this)
│   │   └── voicing_acceptance.rs  pinned fretboard shapes — a TRIPWIRE, not a law
│   └── examples/
│       ├── diffcorpus.rs   engine vs golden corpus (see §5)
│       ├── probe.rs        candidate-score dumper for one input (debug tool)
│       └── voiceprobe.rs   the same for shapes: winner + runners-up in points
├── ivory/                  ── GUI binary (eframe/egui 0.33 PINNED, midir 0.11) ──
│   └── src/                main, app, piano, chord_strip, fretboard_panel, menu,
│                           dialogs, midi, settings, fonts, shell
│                           (see docs/DESIGN.md; egui is pinned at 0.33, see §2c)
├── assets/                 ivory.png (128×128 original art), ivory.ico, ivory.desktop, fonts/
├── tests/golden/           corpus.json (13,133 rows), gen_corpus.py, + classifier outputs
├── scripts/                build-macos.sh, build-cross.sh, build-linux-native.sh, gen-third-party-licenses.sh
└── docs/                   DESIGN.md, DIVERGENCES.md, HANDOFF.md(this), RELEASE.md,
                            spec/ (extracted Python spec), reference/ (old ivory-rust GUI)
```

`docs/reference/` holds the old ivory-rust GUI files (egui 0.29 API, stale but
logic/geometry sound) — reference only, not compiled.

---

## 4. The chord engine — how it works and how to change it safely

The engine is the crown jewel and the riskiest thing to touch. **Never change
scoring to make the raw-Python corpus match — the divergences are intentional.**
The contract is `tests/acceptance.rs`, not the corpus.

### Pipeline (in `detector.rs::detect_chord`)
`< 2 notes → None` · `== 2 → interval string` · then: **D17** guard (≥8 unique
PCs never a chord → scale or `Chromatic Scale` or None) · scale pre-check ·
early special cases (m6-slash, dim7 upper-structure, half-dim vs m6, the D1
6-vs-relative-m7 third-in-bass case) · **main scoring loop** over every root ·
b9 override · dim/aug re-root · **rootless-dominant resolution (D9)** · slash /
simplification · final scale check.

### Scoring (`match_chord_pattern` + `special_bonus`)
Each `(root, pattern)` is scored: essential (60) + %match (40) + highest-note
(10) + completeness + rootless/root-in-bass/characteristic + dominant
adjustment + **a single overwritable special-bonus slot** (values −1000…10000)
+ inversion − penalties. **Pattern tables are ordered slices; earlier patterns
win ties (strict `>`).** The special-bonus slot OVERWRITES (`=`), so rule order
in `special_bonus` matters.

### The 25 fixes (D1–D26, no D16) — full rationale in `docs/DIVERGENCES.md`
Kept-identity behaviors are K1-K13; deliberate fixes span **D1–D26**. D16 was
withdrawn during review (it regressed vectors #44–#47; the behavior it proposed
is kept under K10), so the range holds 25 rules. D21 came from the differential
classifier; **D22–D26 landed 2026-08-10** from an owner bug report (see §2b).
The load-bearing / non-obvious ones:
- **D1** third-in-bass of a `[0,4,7,9]` set → `R6/bass` (only that case is
  special-cased; other basses resolve naturally).
- **D9 (the subtle one)** — *genuine-dominant-root principle*: a voicing like
  E-Bb-D reads on its bass as an `E7#11` shell, but E has no 3rd of its own; it
  is the M3 of an absent C dominant → `C9`. Fires only when the bass reading is
  a **false dominant shell** (m7 + tritone, no M3, no 11, no 13). A genuinely
  rooted dominant (whole-tone `D7#11`, where D has real M3+m7) is left alone.
  `name_rootless_dominant()` builds the name from the tensions present.
- **D13/D17** — no PC reduction; ≤7 unique PCs use all; ≥8 never a chord. The
  compact-vs-spread discriminator for all-12-PC input is **span** (`<12` →
  best scale; else `Chromatic Scale`).
- **D20** — the m11 "+8000 beat-the-scale" bonus is suppressed **only when the
  bass roots the competing chord**, i.e. bass interval ∈ {3 (relative-major
  6/9), 5 (the 9sus root)}. NOT a blunt root==bass gate — that flipped m11
  inversions (a Gm11 drop-2 with the 9th in the bass) to the relative major.
- **"13 needs its 13th"** skip: a `13*`-named pattern is skipped unless interval
  9 is actually present (else `[0,2,4,6,10]` mislabels as `13#11` instead of
  `7#11`). Added the exact `9#11_no5` pattern for that voicing.
- **D4** penalizes no5/shell readings from a non-bass root whenever the bass has
  M3+m7 (dropped the old requirement that the 5th also be present).
- **D5** constrains the `special_bonus` "blanket +380" so a bass that is itself
  a dominant root (M3+m7) yields `C7(b9)` not `Bbdim/C`.

### How to iterate on the engine (the tight loop I used)
```bash
cd ~/Dropbox/Projects/Apps/ivory
cargo test -p ivory-core --test acceptance 2>&1 | grep -E '^  \[|test result'   # see failures
cargo run -p ivory-core --example probe --release        # dump candidate scores for a case
#   edit probe.rs to list the note-sets you're debugging; detect_chord_debug shows scores
cargo run -p ivory-core --example diffcorpus --release              # corpus mismatch count
cargo run -p ivory-core --example diffcorpus --release -- --dump    # every mismatch, one/line
cargo run -p ivory-core --example diffcorpus --release -- --json /tmp/out.json  # rust output
```
The `IVDBG=1` env var + a temporary `eprintln!` in `match_chord_pattern` (gated
on `root_pc == <pc> && score > N`) is the fastest way to see why a specific root
loses — that's how the whole-tone `D13#11` mystery got solved. **Remove any such
instrumentation before committing.**

To reverse-engineer what Python actually does (its spec has bugs, so "differs
from Python" ≠ wrong — judge musical correctness):
```bash
cd ~/Dropbox/Archive/Ivory && python3 -c \
 "from chord_detector import ChordDetector as C; print(C().detect_chord({64,70,74,78}))"
```

### The golden corpus & differential guard
`tests/golden/corpus.json` = 13,133 note-sets run through the reference Python
detector (flat + sharp), regenerable with `tests/golden/gen_corpus.py`. Baseline
mismatch vs raw Python was 3621 before surgery, 5057 after the D1–D21 surgery,
and **5540 today** after D22–D26 (verified 2026-08-11) — the increase is
dominated by the 1001 rows with ≥8 PCs (D17, intended) plus bug-fixes where Rust
is *more* correct (e.g. a `pattern:7b13`-generated row: Python says `Eb7(#11)`,
Rust correctly says `F7(b13)`). `diffcorpus --json` freezes the post-audit
output as `rust-golden.json`, and `differential.rs` is what asserts against it;
`classify.py` maps each remaining divergence to a D-rule but is a **manual**
step and has not been re-run since 2026-07-29.

---

## 5. Build / run / test — every command you need

```bash
cd ~/Dropbox/Projects/Apps/ivory

# tests
cargo test -p ivory-core                       # stock engine: 55 unit + 3 acceptance + differential(fast)
cargo test -p ivory-core --features learning   # + the perceptron (teach layer)
cargo test -p ivory                            # GUI unit tests (settings, geometry, fonts, midi)
cargo test                                     # everything EXCEPT the #[ignore]d suites
# the two #[ignore]d suites, run explicitly:
cargo test -p ivory-core --test differential -- --ignored          # full 13,133-row sweep (~16s)
cargo test -p ivory-core --features learning --test blast_radius --release -- --ignored --nocapture

# run the app (needs no MIDI device to launch)
cargo run -p ivory                             # GUI
./target/debug/ivory --list                    # CLI: list MIDI ports (parity strings)
./target/debug/ivory -p "USB-MIDI"             # connect a specific port

# engine debugging (see §4)
cargo run -p ivory-core --example diffcorpus --release
cargo run -p ivory-core --example probe --release
```

**On-device GUI check:** launch, `screencapture -x -o /tmp/shot.png`, read it.
Synthetic mouse clicks (System Events / osascript) do NOT reliably drive egui
key-toggling on macOS (click-to-focus swallows them, no Quartz module) — verify
interactive paths by unit test or by hand. To force dark-mode/keytoggle for a
screenshot, seed `~/.config/ivory/settings.json` (13 keys, see DESIGN) — **back
it up first; it's the same file the Python app uses.**

**Toolchain gotcha (from [[navi-cross-platform]]):** Homebrew's rust shadows
rustup on PATH; the cross-build scripts prepend the rustup toolchain bin. Rust
1.97 (Homebrew) is what's on PATH here. Cross-targets installed: aarch64/x86_64
apple-darwin, aarch64/x86_64 linux-gnu, x86_64 windows-msvc. `cargo-zigbuild`
and `cargo-xwin` are installed.

---

## 6. Fonts & licensing (settled — don't relitigate)

Bundle **Courier Prime** (SIL OFL 1.1, already in `assets/fonts/` with
`OFL.txt`). It is metric/visually the Courier New match and 100% safe to sell.
**Never redistribute Courier New** (proprietary Monotype) — but naming it as a
fallback is fine (it resolves on mac/Windows, falls to monospace on Linux).
Verified: both Courier Prime weights cover `Δ` (U+0394) and `ø` (U+00F8), so
chord labels need no fallback font. Compliance: ship `OFL.txt` + `LICENSE`
(MIT) + `THIRD-PARTY-LICENSES` in every artifact; credit Courier Prime in the
About box (D-UI-6). Full research in `docs/spec/font-licensing.md`.

**Amendment (Rust stack, 2026-08).** `docs/spec/font-licensing.md` predates the
egui rewrite and covers Courier Prime only. `eframe`'s `default_fonts` feature
embeds four MORE fonts through `epaint_default_fonts`: Ubuntu Light (Ubuntu
Font Licence 1.0), Noto Emoji (OFL 1.1), Hack (MIT + Bitstream Vera),
emoji-icon-font (MIT). Verified byte-for-byte present in the stripped release
binary. Their texts live in `assets/font-licenses/` and ship as
`font-licenses/` in every artifact. Courier Prime covers only 383 codepoints,
so the egui defaults are load-bearing: `⏵` (U+23F5, the submenu arrow in
`menu.rs`) exists ONLY in emoji-icon-font, and user-typed taught chord names
rely on Ubuntu Light. Do not disable `default_fonts`.

---

## 7. Finalization checklist (task 8)

- [x] Icon — **NOT a placeholder** (corrected 2026-08-04). `assets/ivory.png`
      is byte-identical (sha256 `0dc37a25…`) to the Python app's
      `~/Dropbox/Archive/Ivory/icons/ivory.png`: the original piano-keys art,
      which the owner wants kept. It is only 128×128, so Dock/Finder sizes
      above that are upscaled and look soft. Optional polish, not a blocker:
      a nearest-neighbour 8× re-render to 1024px would keep the exact art and
      sharpen every frame. The build scripts now say this instead of shouting
      "unshippable placeholder".
- [x] `scripts/build-macos.sh` — **verified 2026-07-29 (v2.0.0), re-run
      2026-08-10 for v2.1.0**: builds `Ivory.app` (bundle id
      `com.github.ganten7.ivory`; the Info.plist version is interpolated from
      the root `Cargo.toml`, so it tracks the workspace automatically), ad-hoc
      codesigns, bundles LICENSE+OFL+THIRD-PARTY-LICENSES+fonts, stages the app
      together with READ-ME-FIRST.md, zip + dmg; the packaged app launches.
      (dist/ is gitignored.)
- [x] Windows cross-build — **verified**: `cargo xwin` produces `ivory.exe`
      (7.5MB) from this Mac. `build-cross.sh` Windows stage works.
- [ ] Linux build — cross-building is **BLOCKED**: `midir` links ALSA and
      `alsa-sys` can't cross-compile from macOS without a sysroot. Details +
      options in `docs/RELEASE.md` → "Cross-build blocker"; `build-cross.sh`
      Linux stage is now non-fatal (still emits the Windows zip). The
      sanctioned path is **`scripts/build-linux-native.sh` on a Linux host**
      (the owner's Void machine), run once per arch — it has not been run yet:
      `dist/` holds no Linux artifact.
- [x] Pushed to **Codeberg** (2026-07-29): `ganten1998/ivory` (already PRIVATE)
      was "taken over" — Rust 2.0 force-pushed to `main` + `master`; the Python
      app is preserved on the `python-legacy` branch and the immutable
      `v1.0.0`/`v1.1` tags. `git config core.sshCommand "ssh -o IdentityAgent=none"`
      is set locally so pushes work in this shell ([[codeberg-access]]). NOTE:
      default branch is still `master` (now Rust); flip to `main` in Codeberg
      Settings → Branches for tidiness (then `master` can be deleted). A stray
      `cursor/ai-stream-connection-error-b1a2` branch predates this and is
      harmless.
- [x] `ivory-rust` retired to Trash (recoverable); its guide PDF preserved at
      `docs/reference/Ivory-Rust-Guide.pdf`.
- [ ] Business decisions in `docs/RELEASE.md`: keep MIT? Developer-ID signing +
      notarization ($99/yr — macOS 15+ has no right-click-Open bypass, a real
      blocker for selling to strangers); Windows SmartScreen; the "Ivory" name
      collides with Synthogy's Ivory piano VST if commercialized.

---

## 8. Hard-won lessons / gotchas (so we don't rediscover them)

- **The corpus is not the contract.** `acceptance.rs` is. Raw Python has ~27
  catalogued bugs (B1-B27 in `docs/spec/chord-logic.md`); matching it blindly
  reintroduces them.
- **Reverse-engineer, don't guess.** Several "spec" expectations were wrong or
  ambiguous; running the actual Python + reading candidate scores (`probe`,
  `IVDBG`) is how the genuine rules were found (D9's genuine-dominant-root
  principle, the span discriminator for chromatic, the 13th-required skip).
- **Broad scoring changes have blast radius.** Every scoring edit was checked
  against the corpus mismatch delta; the D20 blunt gate silently broke 44 m11
  inversions until the delta caught it. Watch the `diffcorpus` count after any
  `special_bonus` change.
- **egui paint order matters.** The white-key separators vanished because each
  key's fill overpainted the previous key's line — fill all, then stroke.
- **Session limits kill workflows mid-run.** Two implementation launches died on
  usage limits before doing work; the third partially completed (GUI+packaging
  landed, engine/teach/classify died and were resumed/redone). Work in
  committable chunks; the engine surgery was ultimately done inline (tight loop)
  rather than via a subagent that would re-derive all context.
- **Don't edit files a running workflow owns.** Check `git status` before
  editing; the verify workflow was mid-write on detector/lib/overrides/menu.
- **The displayed chord name is not a candidate name.** Slash notation,
  rootless-dominant renaming and dim/aug re-rooting all rewrite the winner
  *after* scoring. Any code that reasons about "the reading that won" must go
  through the candidate capture, never string-match the final label — that
  assumption is what made the re-ranker silently untrainable (2.1.0 fix).
- **`set -e` does not apply inside a function invoked as `f || handler`.**
  bash suppresses errexit for the entire left operand of `||`, function body
  included. `scripts/build-cross.sh` shipped empty Linux tarballs for a week
  because of it. Check each step explicitly in any such function.
- **A release script that warns and carries on has not warned anybody.**
  `build-macos.sh` printed "signed but NOT notarized" and then built the .zip
  and the .dmg regardless — artifacts named exactly like the real thing and
  refused by Gatekeeper everywhere but the build machine. 4.17.0 shipped that
  way and was caught by reading the log. It refuses to package now, gated on
  `stapler validate` of the bundle — the OUTCOME, not the exit status of
  whichever step was taken, so it stays right when the steps change.
  `IVORY_ALLOW_UNNOTARIZED=1` overrides it, loudly.
- **Do NOT give the Linux audio thread realtime priority.** It is the textbook
  fix, `LINUX-4.11-FINDINGS.md` finding 1(c) asked for it, and it is wrong
  here: measured on a 2012 MacBook Air through pipewire-alsa, promoting
  `cpal_alsa_out` to FIFO 70 took 6 underruns per 30 s to **75**, starting the
  moment the thread was promoted. The plugin's own data loop already runs at
  RT 83 and a client above its non-RT IPC thread inverts priority against it.
  What actually fixed it was the buffer geometry — see `BUFFER_PERIODS`.
- **`rfd` on Linux fails silently and returns the same `None` as a cancel.**
  Two backends compiled in (xdg-desktop-portal, then a zenity subprocess); a
  box with neither is an ordinary minimal install, and every file dialog in the
  app was a button that did nothing with no way to detect it. There is an
  in-app browser behind `native_dialogs_work()` now. Anything new that reaches
  for `rfd` needs the same fallback.
- **A `cfg`-gated module is not compiled on the host, so `cargo build` proves
  nothing about it.** `decode.rs` reached into `encode::ffmpeg`, which is
  `#[cfg(not(macos))]` — the mac build never compiles it, every mac check
  passed, and the cross-build failed on `module ffmpeg is private`. Same family
  as the icon-less Windows exe. Anything touching a `cfg`-gated path gets a
  `cargo check --target` for a target that actually has it, before the release
  script finds out.
- **Clearing a published copy is not clearing the source.** Anything that
  snapshots state onto a shared struct every cycle will republish over whatever
  you just cleared, and the failure looks like the button is dead rather than
  like a race. The clip lamp could not be reset for a whole release because
  `clear_clip` cleared the `AudioMeters` copy and `pump()` copied the writer's
  own still-latched `clipped[]` back over it 250 times a second. If a value has
  a publisher, clear it at the publisher.
- **A control with no rectangle is invisible to everything except a user.**
  `Hit::ShowAudioStatus` kept its enum variant, its tooltip and its handler arm
  when its row left the take-settings panel in 4.19.0 — only the entry in
  `SetupLayout::targets()` went. Nothing failed: no dead code, no warning, no
  test, because every test asked whether a Hit *does the right thing* and none
  asked whether it can be *reached*. The whole Audio Status panel was gone for
  a release. Any layout that owns press regions needs a test that walks its
  targets rect by rect through its own hit test — see
  `every_take_settings_control_can_actually_be_clicked`.
- **Deleting a menu row is only safe if something else still reaches the
  feature, and "something else" has to be asserted.** Nine rows left the menu
  in 4.20.0 on the grounds that each duplicated a key binding. That is true
  today and is one line in `keys.rs` away from being false, so
  `no_menu_row_does_what_a_key_already_does` asserts the row is absent AND that
  `keys::binding_for_test` still finds the key.
- **Verify claims about assets before repeating them.** "543-byte placeholder
  icon" was in this document for a week; the file was the original artwork all
  along. One `shasum` against the Python app settled it.

---

## 9. Provenance of the specs

`docs/spec/*.md` were produced by 6 parallel readers over the Python source and
machine-verified (chord tables imported and counted; 146 test vectors executed
byte-for-byte). Trust them over the Python app's own `Ivory Info/` docs, which
are aspirational and self-contradictory in places. The reference Python app:
`~/Dropbox/Archive/Ivory` (`ivory.py` 2035 lines PySide6, `chord_detector.py`
2153 lines). A native arm64 reference build was extracted to the session
scratchpad `refapp/Ivory.app` for side-by-side comparison.
