# Audit: abandoned Rust rewrite of Ivory (`ivory-rust`)

Audited 2026-07-28.
Rust code: `/Users/ganten/Library/CloudStorage/Dropbox/Projects/Apps/ivory-rust/`
Python original: `/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory/` (PySide6, `ivory.py` 2035 lines, `chord_detector.py` 2153 lines)

Repo state: single git commit ("Initial commit: Ivory Rust rewrite"), clean tree, remote `dropbox`.

---

## 1. Architecture and dependency choices

**Layout: sound.** Two-crate workspace:

- `ivory-core` — pure, zero-dependency library (patterns.rs 391 lines, detector.rs 1265 lines incl. 42 unit tests). Fully testable without a GUI. This separation is the single best structural decision in the attempt and should be kept in any rewrite.
- `ivory` — binary: `main.rs` (20 lines), `app.rs` (317), `piano_widget.rs` (184), `midi.rs` (67), `settings.rs` (95).

**Dependencies (Cargo.toml → locked → current on crates.io):**

| Crate | Declared | Locked | Current | Verdict |
|---|---|---|---|---|
| egui / eframe | 0.29 | 0.29.1 | **0.35.0** | Right library, badly stale. 6 minor versions behind with real API churn (`Frame::none()`→`Frame::NONE`, `ui.close_menu()` removed, `rect_stroke` gained `StrokeKind`, menu API rework). Upgrading is a mechanical but nontrivial port. |
| midir | 0.9 | 0.9.1 | **0.11.0** | Right crate (CoreMIDI backend on macOS); 2 versions behind, small API surface, easy bump. |
| serde / serde_json | 1 | — | 1 | Fine. |
| dirs-next | 2 | — | deprecated | Dead fork; use `dirs` 6.0 (or keep hard-coded `~/.config/ivory` for Python parity — the Python app hard-codes that path too). |

Edition 2021 (2024 is current). No release profile tuning, no app bundle / icon story (macOS .app packaging never addressed).

**Runtime architecture: sound with one wart.** midir delivers callbacks on a CoreMIDI thread → `mpsc::channel` → drained in `update()` via `try_recv` (`app.rs:76-102`). Clean RAII connection guard. The wart: `ctx.request_repaint()` unconditionally every frame (`app.rs:314`) = permanent ~60 fps busy loop for an app that is idle most of the time. Qt only repaints on events. A rewrite should use `request_repaint_after(...)` or repaint-on-MIDI-event via `ctx.request_repaint()` from the callback side.

**egui vs alternatives:** for a single-window piano+label app, egui is a reasonable fit (custom painting is easy, `piano_widget.rs` proves it). But note the Python app's marquee features are *windowing* features (detachable chord window, borderless drag, native menus/dialogs) — exactly egui's weak spot. Multi-viewport exists in modern eframe (0.32+) and would cover the detached chord window, but native menus/color pickers/dialogs need extra crates (`rfd` etc.) or hand-rolled egui equivalents.

---

## 2. Quality of the detector port

**Verdict: faithful and slightly *better* than the Python — this is the valuable half of the project.**

Verified by mechanical comparison (script over both sources):

- `CHORD_PATTERNS`: 95 entries in both, **identical names, identical iteration order** (order is detection priority), identical interval sets — with **one content diff**: `altered` is `{0,1,3,4,6,7,8,10}` in Python but `{0,1,3,4,6,8,10}` in Rust (P5 dropped). Possibly deliberate (altered scale has no natural 5th), but it is an undocumented divergence.
- `ESSENTIAL_INTERVALS` / `OPTIONAL_INTERVALS`: ported as `essential_for()` / `optional_for()` match functions — **zero diffs** across all 95 chord types.
- `SCALE_PATTERNS`: 28 in both, identical.
- Scoring engine (`match_chord_pattern` + `special_bonus`): same weights everywhere I checked — essential 60 / percentage 40 / highest-note 10 / completeness 30-700 / extra ×3 / essential-missing ×40 / rootless 15 / root-in-bass 15 / inversion 35/40/45/−40 / dominant-quality ±500/600 / all the magic special-case bonuses (1500 m6-slash, 6200/4200/150 add9-span, 9000/9500/10000 6-9 family, 8000 minor11, −1000 dim-with-4-notes, Bb6/C voicing ±200/250...). The Rust condenses Python's 760-line scoring function to ~415 lines without changing the numbers.
- `detect_chord` control flow mirrors Python exactly: 2-note interval path, >7-note trimming via most-common pitch classes, 7-pc scale fallback, early special cases (m6 slash `[0,1,7,10]` and `[0,1,5,7,10]`, dim7-upper-structure→7(b9)/dim7-slash, half-dim vs m6), dim/aug symmetry re-rooting, slash-chord + simplification logic, clustered-note scale check.

**Deliberate divergences (Rust improvements, not omissions):**

1. **Case 3b** (detector.rs:172-183): early return for minor-triad first inversion (`{Eb,G,C}` → `Cm/Eb` not `Eb6`). Not in Python.
2. **Rootless tritone-implied root detection** (detector.rs:317-353): `{E,Bb,D}` → `C9` via M3+m7 tritone pair with absent root. Not in Python; the `t_rootless_dom9` test depends on it.
3. **Slash suppressed for rootless voicings** (detector.rs:359-360 `root_in_notes` gate). Not in Python.
4. **Shell/no5 penalty −600** when a non-bass root competes against a bass with full dominant voicing (detector.rs:1056-1066). Not in Python.
5. **Python's "Am7 closed voicing → C6" reinterpretation removed** — the Rust test `t_cm7_not_am6` asserts `Cm7` stays `Cm7`, i.e. this Python behavior was classified as a bug and deleted.
6. 9sus/13sus bonus lost the Python span≥12 condition (Rust grants 6400 for any perfect root-in-bass match); `t_9sus` (span 10) passes because of it.
7. Cosmetic: Python's early dim7-structure path returns `"C7b9"`; Rust returns `"C7(b9)"` (consistent with the formatter).

**Formatting** (`format_chord_name`, detector.rs:1124-1193) reproduces Python's display names (`Δ7`, `m7b5`, `9(sus)`, `6/9`, `(add9)` etc.) via a single clean match — much better than Python's 180-line elif chain.

---

## 3. The "failing tests" — README is stale

README claims `cargo test -p ivory-core` → "33/42 pass (9 known detection bugs)".

**Actual result (run 2026-07-28, rustc stable): `42 passed; 0 failed`.**

There is no failing test to enumerate. The repo has exactly one commit, so the README text was written mid-development and the nine fixes (very likely divergences #1–#6 above plus special-bonus retuning: minor-inv early case, rootless tritone detection, root-in-notes slash gate, shell penalty, Am7→C6 removal, 9sus span change) landed in the same commit without the README being updated. The 42 tests cover: 11 intervals, 6 triads, 5 sevenths, 3 extended, 2 sus, 4 altered dominants, 3 inversions, 1 rootless voicing, 3 scales, 3 half-dim/m6 disambiguations, 1 Cm7-not-Am6 regression.

Caveat: 42 tests is thin coverage for a 95-pattern engine whose behavior is defined by ~40 interacting magic bonuses. Python's own `test_chord_detector()` (chord_detector.py:2077+) has additional cases never ported. Only warnings from the build: 8 style lints (non-snake-case test names, unused parens), nothing substantive.

---

## 4. Worth reusing vs rewriting

**Reuse as-is (high value, done, verified):**

- `ivory-core/src/patterns.rs` — the entire data layer: 95 chord patterns in priority order, essential/optional tables, 28 scales, mode lists. This encodes many hours of tuning against the Python. (Restore the `7` in `altered` or document its removal.)
- `ivory-core/src/detector.rs` — the whole detection engine incl. its 42 tests. It is a *superset* of the Python's correctness. Port more Python test cases into it rather than re-porting the engine.
- `ivory/src/midi.rs` — small, correct (velocity-0 = NoteOff, sustain CC64 threshold 64, RAII guard). Needs only: midir 0.9→0.11 bump, port *selection* (currently first-port-only), reconnect handling.
- `ivory/src/piano_widget.rs` — correct 88-key geometry (A0 edge cases handled), hit-testing, aspect-ratio helper. Keep the math; the draw function needs its signature widened (see gaps).

**Rewrite / discard:**

- `ivory/src/app.rs` — keep as a reference skeleton only. It has two real rendering bugs (below), an unconditional-repaint busy loop, a no-op font setup (`FontDefinitions::default()` then `set_fonts` — comment claims it installs Unicode-capable fonts; it installs nothing), and it targets egui 0.29 APIs that no longer exist (`Frame::none`, `close_menu`).
- `ivory/src/settings.rs` — rewrite. It reads/writes the **same file as the Python app** (`~/.config/ivory/settings.json`) but with an incompatible color encoding (`{r,g,b}` object vs Python's `"#rrggbb"` string): loading a Python-written file fails silently → defaults → Save clobbers the user's Python settings. Either serialize colors as hex strings for round-trip compatibility or use a different path. Also `chord_window_detached` key is missing, and four of its fields (window_size_percent, borderless_mode, detached_chord_height) are loaded but wired to nothing.
- `main.rs`, workspace Cargo.tomls — trivial, recreate.

**Known bugs in the GUI crate (found in this audit):**

1. **Sustain coloring wrong** (app.rs + piano_widget.rs): `draw_piano` receives only `active_notes` + a global `sustain_active` flag; while the pedal is down *every* pressed key renders in sustain color, and pedal-held vs finger-held keys are indistinguishable. `sustained_notes` is tracked in app state but never passed to the renderer. Python passes per-note dicts (`set_active_notes(notes: Dict[int, Dict])`) with per-note sustain status.
2. **Keytoggle notes invisible**: clicks toggle `keytoggle_notes` and chord detection includes them, but `draw_piano` never receives them — latched notes get no highlight.

---

## 5. UI-parity gaps vs the PySide6 app

Python `ivory.py` features absent from the Rust GUI:

| Feature | Python | Rust status |
|---|---|---|
| MIDI input selection dialog + reconnect | `select_midi_input()` | Missing — auto-connects first port, no hotplug |
| Detachable chord window (frameless, draggable, resizable, own context menu, persisted) | `create_chord_window()` etc. | Missing (setting field exists, unused). Needs eframe multi-viewport (0.32+) |
| Color pickers (white/black idle, active, sustain) | QColorDialog ×4 | Missing — colors only editable by hand-editing JSON |
| Window size percent (scaling) | `set_window_size_percent` | Missing (setting unused) |
| Borderless mode + drag-to-move | `toggle_borderless_mode` | Missing (setting unused) |
| Full right-click menu (MIDI select, colors, size, borderless, detach chord, flats/sharps, about, reset) | ~15 items | 3 items (Settings, Debug, Clear) + a small checkbox window |
| About dialog | `show_about()` | Missing |
| Reset settings | `reset_settings()` | Missing |
| Single-instance guard | `SingleApplication` (QLocalSocket) | Missing |
| Bundled Courier Prime font for chord label | `fonts/CourierPrime-*.ttf` | Missing — egui default fonts; `FontId::monospace` maps to egui's built-in mono. `Δ`/`ø` glyph coverage in egui defaults is unverified — must embed a font and test |
| Per-note sustain coloring | per-note dict | Broken (bug #1 above) |
| Flats/sharps toggle | menu item | Present (settings window) |
| Keytoggle mode | present | Half-present (no visual, bug #2) |
| Dark mode | present | Present |
| Debug candidates panel | print/debug | Present — Rust's is arguably nicer (`detect_chord_debug` top-N scores overlay) |

---

## 6. Verdict

**Reuse-as-base for `ivory-core`; fresh-start the GUI crate with the old one open as a reference.**

Reasoning:

- `ivory-core` is finished, verified work: byte-for-byte-faithful data tables, a scoring engine that matches the Python's numerology exactly where it matters and deliberately improves it where the Python was wrong, and a green 42-test suite documenting intended behavior. Re-porting this from the 2153-line Python again would be pure waste and would likely re-introduce the bugs the Rust already fixed. Carry the crate forward unchanged (fix/document the `altered` pattern diff, rename lint-offending tests, port more Python test cases).
- The GUI crate is ~25% of the Python app's surface, pinned to a dead egui API (0.29 vs 0.35), with two real rendering bugs, a settings file that corrupts the Python app's config, a busy-loop repaint, and none of the windowing features that define the app (detached chord window, MIDI selection, color pickers, borderless). Upgrading egui 0.29→0.35 already forces touching most lines of `app.rs`; combined with the missing feature list, starting `app.rs`/`settings.rs` clean on current eframe (multi-viewport for the chord window) is less work than incrementally patching. Keep `midi.rs` and the `piano_widget.rs` geometry/hit-test math — both are small, correct, and toolkit-agnostic in spirit.
- Update the README first thing: the "33/42, 9 known bugs" claim is false for the committed code and will mislead any future reader (it misled this audit's premise).

Open risks for the fresh rewrite: egui glyph coverage for `Δ`/`ø` (embed Courier Prime as Python did), macOS packaging (.app bundle) never addressed, and thin detector test coverage relative to the 40-odd interacting score bonuses (port Python's `test_chord_detector()` corpus before touching any scoring number).
