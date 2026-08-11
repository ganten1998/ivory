# Ivory 2.0 — Design

Rust rewrite of Ivory (MIDI keyboard monitor with chord detection). Goals, in
priority order:

1. **UI/UX parity** with the last working Python mac build (`ivory.py`,
   PySide6, v1.1.0) — same look, same behaviors, same settings file.
2. **Solidified chord engine** — the Python engine's *intended* behavior with
   every documented bug fixed and every edge case pinned by tests
   (per-bug policy in `DIVERGENCES.md`).
3. **Teachable chord naming** — exact voicing overrides ship in 2.0; the
   learned re-ranker became a user-facing option in 2.1.0 (D-UI-9). The
   `learning` cargo feature is always enabled by the `ivory` GUI crate, so
   every packaged binary carries it, and it stays inert (adjustment exactly
   0.0) until the user turns Chord Learning on.
4. **Sellable** under pay-what-you-can: clean licensing (bundled Courier
   Prime under SIL OFL 1.1; never redistribute Courier New), packaged for
   macOS, Linux, and Windows.
5. **Clean and lean** — two crates, no speculative abstractions, minimal deps.

Reference specs live in `docs/spec/` (extracted and machine-verified from the
Python source at `~/Dropbox/Archive/Ivory`). The prior Rust attempt
(`ivory-rust`) was audited; its `ivory-core` is the engine base (42 green
tests), its GUI is reference-only (`docs/reference/`). This design was
adversarially reviewed by a three-lens critic panel; decisions below are
post-review.

## Workspace

```
ivory/
├── Cargo.toml            workspace: ivory-core, ivory
├── ivory-core/           pure engine — no GUI deps, fully testable
│   └── src/
│       ├── lib.rs        public API: ChordDetector, Detection, prefs
│       ├── patterns.rs   chord patterns (95 − 2 hygiene deletions, D14),
│       │                 28+1 scales, interval names — ordered tables
│       ├── detector.rs   scoring pipeline (base + DIVERGENCES fixes)
│       ├── naming.rs     pitch-class → note names, display formatting
│       └── overrides.rs  teach layer (exact overrides; re-ranker behind
│                         the `learning` cargo feature)
├── ivory/                GUI binary
│   └── src/
│       ├── main.rs       entry, single-instance, CLI, panic-hook dialog
│       ├── app.rs        eframe App: layout, timers, resize mechanics
│       ├── piano.rs      88-key renderer + hit-testing (exact Qt geometry)
│       ├── chord_strip.rs black strip + detached viewport
│       ├── menu.rs       context menu (parity order) + teach items
│       ├── dialogs.rs    MIDI picker, color modals, About, teach dialogs
│       ├── midi.rs       midir thread → mpsc, auto-connect priority chain
│       ├── settings.rs   ~/.config/ivory/settings.json — Python-compatible
│       └── fonts.rs      embed Courier Prime (two families), custom font
├── ivory/build.rs        Windows-only: icon + version resource (winres)
├── assets/               ivory.png (128×128), ivory.ico, ivory.desktop, fonts/{CourierPrime-*.ttf, OFL.txt}
├── tests/                golden corpus + differential harness
├── scripts/              build-macos.sh, build-cross.sh, build-linux-native.sh (navi pattern)
└── docs/                 DESIGN.md, DIVERGENCES.md, RELEASE.md, spec/, reference/
```

## Engine (`ivory-core`)

- **API**: `ChordDetector::detect(&self, notes: &[u8]) -> Option<Detection>`
  where `Detection { label, kind: Chord|Interval|Scale, root, bass,
  pattern_key, candidates }`. GUI renders `label` verbatim; `candidates`
  powers the teach dialog and debug overlay. Formatting matches shipped
  Python exactly (Δ7 glyph, `m7b5`, parenthesized comma-joined tensions,
  `C(add9)`, `6/9`, `/Bass`, root-prefixed 2-note intervals).
- **Tables**: Python's ordered tables verbatim minus the two D14 hygiene
  deletions. First-listed pattern wins score ties — ordered slices, never
  hash maps.
- **Pipeline**: same stages as Python with the DIVERGENCES fixes. All
  scoring constants preserved unless a D-rule requires change; every
  changed constant is documented at the change site with its D-rule ID.
- **Determinism**: no PC-reduction step (D13/D17): ≤7 unique PCs use all;
  ≥8 unique PCs go to scale detection or None. No iteration-order
  dependence anywhere.
- **Tests, three layers**:
  1. Unit tests from the old Rust core (42) — kept green, updated only
     where a D-rule demands (each update cites its rule).
  2. The acceptance table: the 146 machine-verified vectors, with doc-§
     examples and Python's 30-case suite folded in as tagged rows (one
     expectation set, not three). Expectations updated per DIVERGENCES,
     each flip annotated with its D-rule.
  3. **Differential harness** (`tests/golden/`): runs the engine over
     `corpus.json` (13,133 sets × flat/sharp). The *enforced* invariant is
     `ivory-core/tests/differential.rs` vs the frozen `rust-golden.json`
     baseline — a 1500-row fast subset by default, with the full sweep behind
     `#[ignore]` (see RELEASE.md gate 3). `classified-divergences.json`
     (note-set → D-rule, both naming prefs) is a hand-audited snapshot
     regenerated by `classify.py`; **no test asserts against it.**

## Teach layer (`overrides.rs`)

- **Exact overrides (ships in 2.0)**: keyed by sorted interval-set from the
  bass. Value = name template; if the taught name starts with a
  recognizable note name and "apply in all keys" is checked, the root is
  parameterized (transposition-invariant); otherwise literal for that
  voicing. Consulted *before* detection scoring. Persisted to
  `~/.config/ivory/overrides.json` (schema versioned; also carries
  `learning_mode` — Python never touches this file).
- **Learned re-ranker (cargo feature `learning`; since 2.1.0 always enabled
  by the `ivory` GUI crate, and OFF at runtime until the user turns it on)**:
  online perceptron re-scorer over candidate features (pattern class,
  root-vs-bass interval, span, note count, cluster flag, inversion);
  corrections are training examples; bounded adjustment (±100 against raw
  scores in the hundreds-to-thousands, so it only ever flips near-ties);
  never overrides exact matches; resettable. No ML crates. The differential
  harness always constructs the detector with zero weights, and zero weights
  or `learning_mode: false` make the adjustment exactly 0.0 — detection is
  then bit-identical to stock (asserted by tests).
  `train_on_correction` takes up to 25 bounded perceptron steps toward the
  user's pick, judges success on the **displayed label** (winning the score
  loop is not sufficient: the D21 completeness rule can still swap in a
  note-complete reading within 12 points, and the slash step then appends the
  bass), and rolls the attempt back entirely if it cannot get there.
- **UI**: per D-UI-5 — `Teach Chord Name...` (notes + current label +
  input + "apply in all keys"; greyed when nothing held) and `Manage
  Taught Chords...` (list + delete), placed inside the detection block
  after Detach/Attach with their own separator.

## GUI (`ivory` crate)

- **Stack**: eframe/egui 0.35, glow backend, x11+wayland features, **no
  eframe persistence** (settings.json is the sole geometry/state
  authority). `rfd` for pre-event-loop native message boxes ("Ivory
  Already Running", MIDI errors, panic-hook "Ivory Error" dialog). midir
  0.11, dirs 6, fd-lock 4.
- **Window mechanics**: fixed-size; on any size change send
  `ViewportCommand::{MinInnerSize, MaxInnerSize, InnerSize}` together
  (min=max=target). `W = int(1300·P/100)`, `pianoH = int(W/(1300/150))`,
  `chordH = int(50·W/1300)` — integer truncation exactly as the spec;
  settings are read *before* building the ViewportBuilder so the first
  frame is right-sized. Sizes computed in logical points (Qt pt == logical
  px on macOS, the parity reference). `window_size_percent` accepts any
  positive int, not just the 7 presets. Some Linux tiling/Wayland WMs
  treat fixed sizes as advisory — accepted, not fought.
- **Borderless + keytoggle**: on primary press: keytoggle hit-test/toggle
  first (if enabled), then `ViewportCommand::StartDrag` issued directly
  from the press handler (StartDrag only works immediately after a press).
  Decorations via `ViewportCommand::Decorations` on both windows.
- **Detached chord window**: `show_viewport_immediate` (deferred viewports
  need Arc<Mutex> plumbing for zero benefit here — both windows repaint on
  the same cadence). Each frame the viewport records its inner size;
  close_requested → save height, clear detached flag (that IS
  close-to-reattach). Width-sync: 100ms debounce (restarted on changes),
  fires `InnerSize(main_width, last_known_height)` when |Δw| > 5px.
  Min size 300×100; title "Ivory". A first-milestone spike confirms a
  second glow viewport renders cleanly on macOS before the full build-out.
- **Piano / chord strip**: exact spec geometry (52 whites, 0.7/0.65
  ratios, boundary-centered blacks, int truncation, 1px separators,
  background fill kept — sub-pixel slivers are part of the look, dark mode
  swaps idle colors only, sustain recolors all held keys, black-first
  hit-test). Chord strip: always-black, #E8DCC0 text, `max(12, 0.6·h)`,
  single-pass 95% shrink, centered baseline math as specced.
- **Context menu chrome**: egui menu with rounding 0, shadow off, 1px
  bg-colored stroke, button_padding (20,4), 1px separator rects, exact
  spec colors per mode, toggle items rename (no checkmarks), submenu for
  Size. Ctrl-click-as-right-click on macOS goes on the verification
  checklist (spec §13) rather than being assumed.
- **Fonts (`fonts.rs`)**: two named families — "courier" (CourierPrime-
  Regular + egui defaults as glyph fallback) and "courier-bold"
  (CourierPrime-Bold + fallback). Menu/About/dialog styles map to bold;
  chord strip and piano text to regular (spec: chord text is Normal
  weight). Build-time cmap test (ttf-parser dev-dep) asserts Δ/ø coverage
  in both. `custom_font_path` (settings.json) loads a user TTF/OTF at top
  priority in both families. No Courier New file is ever shipped.
- **Settings**: literal `~/.config/ivory/settings.json` on ALL platforms
  (Python hard-codes Path.home()/.config — do not "improve" to
  dirs::config_dir() or carryover fails). Same 13 keys, lowercase #rrggbb
  color strings (parse case-insensitively), `serde_json` pretty 2-space
  with preserve_order, unknown keys preserved via Value merge. Additive:
  `custom_font_path` only (learning_mode lives in overrides.json; Python
  downgrade rewrites settings.json with fixed keys — one-way caveat
  documented in RELEASE.md).
- **MIDI**: one reader thread → mpsc. Parity auto-connect chain, note
  on/off + CC64 semantics, channels merged. Manual re-pick only; the
  "(disconnected)" indicator was **not** built — it did not fall out of the
  connection code for free, so the picker shows a freshly enumerated port list
  plus `Current: <port>` and nothing else (D-UI-4, corrected 2026-08-11).
  CLI parity strings: argparse description,
  "Available MIDI Input Ports:", two-space "  {i}: {name}", "  No MIDI
  input ports found!". The Python "MIDI Not Available" (mido missing)
  branch is structurally impossible with midir — dropped.
- **Lifecycle**: single instance via fd-lock (same dialog + exit UX).
  macOS Cmd+Q runs the same shutdown as window close (MIDI thread stopped,
  detached viewport closed, settings saved). Note: README/marketing claims
  of Ctrl+D/C/W shortcuts are stale — ivory.py has none; do not "restore".
- **Windows specifics**: `#![windows_subsystem = "windows"]`; build.rs
  (target-gated) embeds icon + version via winres; AttachConsole on launch
  from a terminal so `-l`/`-p` output still prints (navi pattern).

## Packaging & release

- `scripts/build-macos.sh`: release build → `Ivory.app` (bundle id
  `com.github.ganten7.ivory`, .icns from icon asset) → always ad-hoc
  `codesign --force --deep -s -` → zip + DMG, both wrapping a staging folder
  that holds `Ivory.app` + `READ-ME-FIRST.md` (since 2.1.0). OFL.txt + LICENSE +
  THIRD-PARTY-LICENSES in Contents/Resources.
- `scripts/build-cross.sh`: navi-proven — `cargo zigbuild` for
  x86_64/aarch64 Linux (glibc 2.32 floor) → tar.gz with .desktop, icon,
  license files; `cargo xwin` for Windows x86_64 → zip. `.deb` (with
  proper /usr/share/doc/ivory/copyright folding MIT + OFL) after the
  tar.gz path works.
- THIRD-PARTY-LICENSES generated via cargo-about/cargo-license; ships in
  every artifact alongside LICENSE (MIT) and OFL.txt.
- CI deferred until after the first local-script release (local scripts are
  the source of truth; they run on this machine).
- RELEASE.md carries the business checklist: Developer ID signing +
  notarization (macOS 15+ has no right-click-Open bypass — practical
  blocker for distributing to strangers; $99/yr decision), Windows
  SmartScreen note for unsigned exes, MIT-retention implications for
  pay-what-you-can, the Synthogy "Ivory" trademark-collision note, the icon
  note (the 543-byte PNG is the ORIGINAL 128×128 art, byte-identical to the
  Python app's — small, so it upscales soft; explicitly NOT a gate), and the
  settings one-way caveat.

## Version

**2.1.0.** The single authority is `[workspace.package] version` in the root
`Cargo.toml`; both crates inherit it, the build scripts read it, and the About
dialog renders `env!("CARGO_PKG_VERSION")` (ivory/src/dialogs.rs). 2.0.0 was
the new major (new implementation language, teach layer); 2.1.0 made Chord
Learning a user-facing option (D-UI-9). The Python line ends at 1.1.0. About
dialog per D-UI-6. Nothing in the 2.x line has been publicly released yet —
the newest tags are the Python-era `v1.0.0` / `v1.1`.
