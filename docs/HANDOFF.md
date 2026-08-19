# Ivory 2.0 — Handoff / Resume Document

**Last updated:** 2026-08-18. **The app is now called TANGENT.** Newest work is
§2g: the six-operator FM built-in, its patch picker, and the reverb/delay knobs.
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
