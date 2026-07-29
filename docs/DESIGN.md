# Ivory 2.0 — Design

Rust rewrite of Ivory (MIDI keyboard monitor with chord detection). Goals, in
priority order:

1. **UI/UX parity** with the last working Python mac build (`ivory.py`,
   PySide6, v1.1.0) — same look, same behaviors, same settings file.
2. **Solidified chord engine** — the Python engine's *intended* behavior with
   every documented bug fixed and every edge case pinned by tests
   (see `DIVERGENCES.md` for the per-bug policy).
3. **Teachable chord naming** — users can override names for specific voicings
   and optionally let the app generalize from their corrections.
4. **Sellable** under pay-what-you-can: clean licensing (bundled Courier
   Prime under SIL OFL 1.1; never redistribute Courier New), packaged for
   macOS, Linux, and Windows.
5. **Clean and lean** — two crates, no speculative abstractions, minimal deps.

Reference specs live in the session scratchpad (`specs/*.md`) and were
extracted from the Python source at
`~/Dropbox/Archive/Ivory` (ivory.py 2035 lines, chord_detector.py 2153 lines).
The prior Rust attempt (`~/Dropbox/Projects/Apps/ivory-rust`) is mined as a
base for the engine (all 42 of its tests pass; it already fixes several Python
bugs) and retired afterwards.

## Workspace

```
ivory/
├── Cargo.toml            workspace: ivory-core, ivory
├── ivory-core/           pure engine — no GUI deps, fully testable
│   └── src/
│       ├── lib.rs        public API: ChordEngine, Detection, NamePrefs
│       ├── patterns.rs   95 chord patterns, 28+1 scales, interval names —
│       │                 ordered tables (order is load-bearing for ties)
│       ├── detector.rs   scoring pipeline (port of Python + DIVERGENCES fixes)
│       ├── naming.rs     pitch-class → note names, display formatting
│       └── overrides.rs  teach layer: exact overrides + learned re-ranker
├── ivory/                GUI binary
│   └── src/
│       ├── main.rs       entry, single-instance, CLI (-p/--port, -l/--list)
│       ├── app.rs        eframe App: layout, timers, mode state
│       ├── piano.rs      88-key renderer + hit-testing (exact Qt geometry)
│       ├── chord_strip.rs black strip, Courier text, shrink-to-fit, detach
│       ├── menu.rs       context menu (parity order) + teach items
│       ├── dialogs.rs    MIDI picker, color pickers, About, teach dialog
│       ├── midi.rs       midir thread → mpsc, auto-connect priority chain
│       ├── settings.rs   ~/.config/ivory/settings.json — Python-compatible
│       └── fonts.rs      embed Courier Prime, custom-font setting
├── assets/               CourierPrime-{Regular,Bold}.ttf, OFL.txt, ivory.png
├── tests/                golden corpus + differential harness (see below)
├── scripts/              build-macos.sh, build-cross.sh (navi pattern)
└── docs/                 DESIGN.md, DIVERGENCES.md, RELEASE.md
```

## Engine (`ivory-core`)

- **API**: `ChordEngine::detect(&self, notes: &[u8]) -> Option<Detection>`
  where `Detection { label: String, kind: Chord|Interval|Scale, root, bass,
  pattern_key, candidates: Vec<Candidate> }`. The GUI renders `label`
  verbatim; `candidates` powers the teach dialog and a debug overlay.
  `NamePrefs { prefer_flats }` controls naming; formatting matches shipped
  Python exactly (Δ7 glyph, `m7b5`, parenthesized comma-joined tensions,
  `C(add9)`, `6/9`, `/Bass` slashes, root-prefixed 2-note intervals).
- **Tables**: identical to Python's 95 patterns / essential / optional maps
  (ordered `&[(&str, ...)]` slices — first-listed pattern wins score ties),
  except the cleanups listed in DIVERGENCES (duplicate/shadowed patterns).
- **Pipeline**: same stages as Python (interval → scale-precheck → >7-note
  reduction → early special cases → per-root pattern scoring → symmetric
  re-rooting → slash/simplification → final scale check), with the fixes in
  DIVERGENCES. All scoring constants preserved unless a fix requires change,
  and every changed constant is documented with a test.
- **Determinism**: >7-note reduction uses a specified tie-break (ascending
  pitch-class, bass PC always kept). No iteration-order dependence anywhere.
- **Tests, four layers**:
  1. Unit tests ported from the old Rust core (42) — kept green.
  2. The 146-vector machine-verified acceptance table from the Python spec —
     with expectations updated per DIVERGENCES (each marked vector links to
     its D-rule).
  3. Python's own 30-case test corpus + every example in
     `01_Special_Cases_and_Resolutions.md` — expected values = doc intent.
  4. **Differential harness**: `tests/golden/corpus.json` holds 13,133 note
     sets with the Python reference output (flat + sharp). A harness runs the
     Rust engine over all of them; every mismatch must be *classified* by a
     DIVERGENCES rule (each rule carries a matcher). Unclassified mismatches
     fail CI. The audited post-fix outputs are then frozen as
     `tests/golden/rust-golden.json` for regression from that point on.

## Teach layer (`overrides.rs`)

- **Exact overrides**: keyed by (sorted interval-set from bass). Value =
  name template. If the taught name starts with a recognizable note name, the
  root is parameterized and an "apply in all keys" flag makes the override
  transposition-invariant; otherwise it's stored literal for that voicing.
  Overrides are consulted *before* detection scoring. Persisted to
  `~/.config/ivory/overrides.json` (schema versioned).
- **Learned re-ranker (experiment)**: online logistic/perceptron re-scorer
  over candidate features (pattern class, root-vs-bass interval, span,
  note count, cluster flag, inversion). Every user correction is a training
  example (chosen candidate positive, displaced candidate negative). Applies
  a bounded score adjustment (never overrides exact matches; can be reset).
  Off by default; enabled by `"learning_mode": true`. Weights persisted in
  `overrides.json`. No ML crate deps — ~100 lines, explainable, resettable.
- **UI**: two context-menu items when the detector is on: `Teach Chord
  Name...` (shows held notes + current label, input for preferred name,
  "apply in all keys" checkbox) and `Manage Taught Chords...` (list +
  delete + reset learning). These are the only intentional menu additions
  over the Python app.

## GUI (`ivory` crate)

- **Stack**: current `eframe`/`egui` (0.35.x), glow backend, x11+wayland
  features for Linux. Detached chord window = deferred viewport
  (`egui::ViewportBuilder`), matching Python behavior (close re-attaches,
  width follows main window with debounce, height persisted — honored, see
  D-UI-1). Fixed-size window; Size submenu 50–200% presets. Borderless mode
  via viewport decorations toggle + drag-anywhere.
- **Rendering parity**: exact Qt geometry math from the UI spec (52 white
  keys, `blackW = 0.7·whiteW`, `blackH = 0.65·H`, black key centered on the
  boundary, int-truncated rects, 1px separators, per-mode colors, dark mode
  swaps idle key colors only, sustain recolors all held keys while pedal
  down, keytoggle latching with black-keys-first hit-test).
- **Chord strip**: always-black, `#E8DCC0` text, font size `max(12,
  0.6·height)` with single-pass shrink at 95% width, centered baseline math
  as specced.
- **Fonts**: embed CourierPrime Regular + Bold at startup as the primary
  proportional+monospace family; egui's default fonts remain as glyph
  fallback (covers Δ if Courier Prime lacks it — verified at build time by a
  unit test reading the cmap via `ttf-parser`). Optional
  `"custom_font_path"` settings key loads a user TTF/OTF at highest
  priority. No Courier New file is ever shipped; the *name* is irrelevant at
  runtime since we embed. OFL.txt ships in every artifact; About box credits
  "Courier Prime © The Courier Prime Project Authors, SIL OFL 1.1".
- **Settings**: same file, same 13 keys, same formats (hex color strings,
  `window_size_percent`, etc.) so the Python app's settings carry over
  untouched. New keys are additive: `custom_font_path`, `learning_mode`.
  Unknown keys preserved on save (round-trip via `serde_json::Value` merge).
- **MIDI**: `midir` 0.11, one reader thread → `mpsc`. Same auto-connect
  priority ("USB-MIDI" → "Scarlett"/"USB"+"MIDI" → first port), same
  note-on/off + CC64 sustain semantics, channels ignored. Manual re-pick
  dialog for reconnects (parity: no auto-reconnect), but a dead connection
  is *detected* and shown as "(disconnected)" in the picker.
- **Single instance**: lock file (`~/.config/ivory/ivory.lock` via
  `fd-lock`) — same UX as Python (warning box + exit), immune to the
  stale-QSharedMemory relaunch bug.
- **Timers**: egui repaint scheduling ≈ Python cadence (50ms GUI /100ms
  detection); repaint-on-demand (no unconditional busy loop — fixes the old
  Rust attempt's wart).

## Packaging & release

- `scripts/build-macos.sh`: release build → `Ivory.app` (bundle id
  `com.github.ganten7.ivory`, .icns generated from icon asset) → zip + DMG.
  Unsigned initially; notarization documented in RELEASE.md as a
  pre-store-release step.
- `scripts/build-cross.sh`: navi-proven pipeline — `cargo zigbuild` for
  x86_64/aarch64 Linux (glibc 2.32 floor) → tar.gz with .desktop + icon +
  fonts license; `cargo xwin` for Windows x86_64 → zip. A `.deb` layout
  mirroring the Python `build_deb.sh` (with OFL.txt this time) comes after
  the tar.gz path works.
- CI: GitHub Actions on tag, one job per OS, artifacts named
  `ivory-X.Y.Z-{platform}`. Kept minimal; local scripts are the source of
  truth (they run on this machine).
- **License**: code stays MIT (as the Python app was) unless the owner
  decides otherwise before release; pay-what-you-can is compatible with MIT
  (free downloads + optional payment). Flagged as an open business decision
  in RELEASE.md, along with the "Ivory" vs Synthogy Ivory trademark-collision
  note.

## Version

2.0.0 (new major: new implementation language, teach layer). The Python line
ends at 1.1.0.
