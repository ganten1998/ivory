# Ivory 2.0 — Handoff / Resume Document

**Last updated:** 2026-07-29. This file is the single source of truth for
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

**Repo:** `~/Dropbox/Projects/Apps/ivory` (git, branch `main`, local only — no
remote yet; Codeberg push is a remaining task, see §7). The old abandoned
attempt `~/Dropbox/Projects/Apps/ivory-rust` was mined for its engine and is to
be **trashed once 2.0 verifies** (do NOT push it anywhere).

---

## 2. Current status (2026-07-29)

### Done and committed
| Commit | What |
|---|---|
| `cf71ba6` | Scaffold: workspace, engine base copied from ivory-rust, specs, fonts, golden corpus |
| `c9e15a0` | Design + divergence policy (after a 3-lens adversarial critique) |
| `c8be947` | GUI parity port (9 modules), packaging scripts, engine acceptance contract |
| `5dbe50f` | **Engine surgery: all 20 D-rules land; acceptance + 42 unit tests green** |
| `290a73d` | Refine D20 (fixed 44 m11-inversion regressions) |
| `0d0926e` | Fix invisible white-key separators (bug the owner caught on-device) |
| `da487fa` | This HANDOFF doc |
| `60bc651` | **Verify phase: teach layer + differential classification + D21 note-drop fixes** |

**Engine + teach + verify are all DONE and green.** `cargo test --workspace` →
GUI 11 + engine 54 unit + 3 acceptance + differential(fast); `--features
learning` → 58 + 3. The verify workflow (classifier + adversarial reviewer +
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
  `tests/golden/README.md`). Corpus mismatch vs raw Python is now 5057.
- **Adversarial review done**: no misfire found in the 7 broad scoring changes.
- **D21 note-drop fixes**: the classifier caught 28 drop-2 voicings dropping a
  #5/#11; fixed via a completeness preference + a maj7#11 perfect-inversion
  boost. One residual non-blocking slash-simplify note-drop is documented in
  DIVERGENCES.md D21 and `tests/golden/UNEXPLAINED.md` (top note).

### Remaining work (task 8 — FINALIZE only)
Everything above is committed. What's left is packaging + release + cleanup:
- Real icon art (current `assets/ivory.png` is a 543-byte placeholder — **blocks
  release**), then packaging dry-run (`scripts/build-macos.sh`, `build-cross.sh`).
- Push to Codeberg (see §7), then trash `~/Dropbox/Projects/Apps/ivory-rust`.
- Optional polish: 7 cosmetic unused-variable warnings in `ivory-core` (inherited
  from the base); reconcile `docs/DESIGN.md`/`DIVERGENCES.md` prose with the
  final implemented mechanisms if desired (HANDOFF §4 is already accurate).

---

## 3. Repo layout

```
ivory/
├── Cargo.toml              workspace (ivory-core, ivory); version 2.0.0, MIT
├── ivory-core/             ── PURE ENGINE, no GUI deps, exhaustively tested ──
│   ├── src/
│   │   ├── lib.rs          public API re-exports
│   │   ├── patterns.rs     chord/scale tables (ORDER IS LOAD-BEARING)
│   │   ├── detector.rs     the scoring pipeline (all the hard logic)
│   │   ├── naming.rs       note naming / display formatting
│   │   └── overrides.rs    teach layer (exact overrides + learning feature)
│   ├── tests/
│   │   ├── acceptance.rs   THE CONTRACT — 200+ (notes → name) vectors, both prefs
│   │   └── differential.rs corpus regression guard (classifier writes this)
│   └── examples/
│       ├── diffcorpus.rs   engine vs golden corpus (see §5)
│       └── probe.rs        candidate-score dumper for one input (debug tool)
├── ivory/                  ── GUI binary (eframe/egui 0.35, midir 0.11) ──
│   └── src/                main, app, piano, chord_strip, menu, dialogs, midi,
│                           settings, fonts  (see docs/DESIGN.md for each)
├── assets/                 CourierPrime-{Regular,Bold}.ttf, OFL.txt, ivory.png(placeholder)
├── tests/golden/           corpus.json (13,133 rows), gen_corpus.py, + classifier outputs
├── scripts/                build-macos.sh, build-cross.sh, gen-third-party-licenses.sh
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

### The 20 fixes (D1-D20) — full rationale in `docs/DIVERGENCES.md`
Kept-identity behaviors are K1-K13; deliberate fixes are D1-D20. The load-
bearing / non-obvious ones:
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
mismatch vs raw Python was 3621 before surgery, ~5195 after — the increase is
dominated by the 1001 rows with ≥8 PCs (D17, intended) plus bug-fixes where Rust
is *more* correct (e.g. a `pattern:7b13`-generated row: Python says `Eb7(#11)`,
Rust correctly says `F7(b13)`). The classifier freezes the post-audit output as
`rust-golden.json` and asserts every remaining divergence maps to a D-rule.

---

## 5. Build / run / test — every command you need

```bash
cd ~/Dropbox/Projects/Apps/ivory

# tests
cargo test -p ivory-core                       # engine: 42 unit + acceptance
cargo test -p ivory-core --features learning   # + the perceptron (teach layer)
cargo test -p ivory                            # GUI unit tests (settings, geometry, fonts, midi)
cargo test                                     # everything

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

---

## 7. Finalization checklist (task 8)

- [ ] Real icon art — `assets/ivory.png` is a 543-byte placeholder. Needs a
      proper PNG → `.icns` (mac) + `.ico` (win, for `build.rs`). **Release
      blocker.** Scripts generate icns/ico from the png but will faithfully
      reproduce garbage.
- [x] `scripts/build-macos.sh` — **verified 2026-07-29**: builds `Ivory.app`
      (bundle id `com.github.ganten7.ivory`, v2.0.0), ad-hoc codesigns, bundles
      LICENSE+OFL+THIRD-PARTY-LICENSES+fonts, zip + dmg; the packaged app
      launches. (dist/ is gitignored.)
- [x] Windows cross-build — **verified**: `cargo xwin` produces `ivory.exe`
      (7.5MB) from this Mac. `build-cross.sh` Windows stage works.
- [ ] Linux cross-build — **BLOCKED**: `midir` links ALSA; `alsa-sys` can't
      cross-compile from macOS without a sysroot. Build on Linux (CI w/
      `libasound2-dev`) or provide a sysroot. Details + options in
      `docs/RELEASE.md` → "Cross-build blocker"; `build-cross.sh` Linux stage is
      now non-fatal (still emits the Windows zip).
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

---

## 9. Provenance of the specs

`docs/spec/*.md` were produced by 6 parallel readers over the Python source and
machine-verified (chord tables imported and counted; 146 test vectors executed
byte-for-byte). Trust them over the Python app's own `Ivory Info/` docs, which
are aspirational and self-contradictory in places. The reference Python app:
`~/Dropbox/Archive/Ivory` (`ivory.py` 2035 lines PySide6, `chord_detector.py`
2153 lines). A native arm64 reference build was extracted to the session
scratchpad `refapp/Ivory.app` for side-by-side comparison.
