# Ivory — Definitive UI/UX Specification for Rust Rewrite

Source of truth: `/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory/ivory.py` (PySide6, VERSION = "1.1.0", 2035 lines, read in full).
Secondary: `ivory_pyqt5.py` (older PyQt5 Windows port — differences noted in §14; NOT authoritative).
Chord names come from `chord_detector.py` (`ChordDetector` class); its detection algorithm is out of scope here, but its *output text format* affects rendering (§5.6).

Every color below is given exactly as in source. All pixel values are logical (Qt device-independent) pixels. All timings in ms.

---

## 1. Application identity

- App/window title: **"Ivory"** (exact string; set on the main window at init, re-set on every `showEvent`, and re-set after borderless toggles; also set on the detached chord window).
- `QApplication.applicationName = "Ivory"`, `organizationName = "Ivory"`.
- Version string: `"1.1.0"` (shown in About dialog as `Version 1.1.0`).
- App/window icon: `icons/ivory.png` relative to the executable/script dir (PyInstaller `_MEIPASS`-aware via `resource_path()`). Set both as the application-wide window icon and on the main window. If the file is missing, silently no icon.
- No native menu bar, no toolbar, no status bar, no tray icon, no dock-specific code. All commands live in a right-click context menu (§6).

## 2. Process behavior

### 2.1 CLI arguments (argparse)
- Description: `"PySide6 MIDI keyboard monitor with chord detection"`.
- `-p, --port <str>` — MIDI input port name to open (exact-name; passed straight to `mido.open_input`).
- `-l, --list` — print available MIDI input ports and exit:
  ```
  Available MIDI Input Ports:
    0: <name>
    1: <name>
  ```
  or `  No MIDI input ports found!` if none. (Two-space indent, `index: name`.)

### 2.2 Single instance
- Mechanism: `QSharedMemory` with key **`"ivory-midi-monitor"`**.
  - Try `attach()`: success ⇒ another instance is running.
  - Else try `create(1)` (1 byte): success ⇒ this is first instance; failure ⇒ treat as already-running.
- If already running: show warning `QMessageBox` titled **"Ivory Already Running"** with text:
  `"Ivory is already running.\n\nOnly one instance can run at a time."` then `exit(0)`. No message passing/activation of the existing instance.
- On normal quit: detach shared memory, then `sys.exit(exit_code)`.
- (A `QSystemSemaphore` is imported but never used.)

### 2.3 Startup / shutdown
- After constructing the main window: `show()`, `raise_()`, `activateWindow()`.
- Top-level exception handler: any uncaught exception shows a critical `QMessageBox` titled **"Ivory Error"** with body `"Ivory encountered an error:\n\n{err}\n\n{traceback}"` (falls back to stderr), exit code 1.
- Window close (`closeEvent`): set MIDI thread stop flag, close the MIDI input port (ignore errors), close the detached chord window if open, accept. Closing the main window quits the app (default Qt behavior; `quitOnLastWindowClosed` untouched).
- Two harmless debug prints at init: `DEBUG init_ui: Set fixed size: {w}x{h}` and `DEBUG init_ui: Window visible: {bool}` (need not be reproduced).

## 3. Main window geometry & sizing model

The main window is **NOT user-resizable**. It is always `setFixedSize()`; the only way to change size is the context-menu **Size** submenu (50–200%). Programmatic constraints that still exist in code (largely vestigial because of fixed size): `setMinimumSize(200, 150)` at init; `_update_piano_height` (defined but never called) would set min width 200, max width 5000.

### 3.1 Layout structure
- Central widget: plain `QWidget`, contentsMargins 0, containing a `QVBoxLayout` (margins 0, spacing 0):
  1. **ChordLabelWidget** (only if chord detector module importable), stretch 1.
  2. **PianoWidget**, stretch 0.
- In practice the layout is overridden: `_position_widgets()` places both children with absolute `setGeometry`/`setFixedSize` calls so there is never any gap/white space. Recreate the *result*, not the layout mechanics:
  - Chord label (when attached & enabled): rect `(0, 0, W, chordH)`.
  - Piano: rect `(0, chordH, W, pianoH)` — or `(0, 0, W, pianoH)` when chord label hidden/detached/disabled.
  - Window height is forced to exactly `chordH + pianoH` (or `pianoH`); if the actual height differs by >1 px, `setFixedSize` corrects it.

### 3.2 Size math
- Base width `_base_width = 1300`. Piano aspect ratio `piano_aspect = 1300/150 ≈ 8.6667` (width:height).
- Given size percentage `P` (persisted, default 100), `scale = P/100`:
  - `W = int(1300 * scale)`
  - `pianoH = int(W / piano_aspect)` (at 100%: 150)
  - `chordH = int(50 * scale)` if chord detection available AND enabled AND not detached, else `0` (at 100%: 50)
  - Window fixed size = `W × (chordH + pianoH)` (at 100% with chord: 1300×200).
- `_position_widgets` recomputes `scale = W / 1300` from the current width, so chord height is always proportional to width.
- When size % changes: window, piano, chord label, and central widget are all resized/positioned immediately (no animation), chord label min height set to `int(50*scale)`, max height `int(500*scale)`; then settings saved.
- `showEvent` schedules `_position_widgets` via a 10 ms single-shot timer. Toggling chord detection/attachment also uses 10 ms single-shot re-position timers.

### 3.3 Window background
There is no exposed window background: the two child widgets exactly tile the window and each paints its full rect (piano paints `bg_color`: `#1a1a1a` rgb(26,26,26) in dark mode / `#E8E8E8` rgb(232,232,232) in light mode; chord widget paints solid black `#000000`).

### 3.4 Borderless mode
- Toggled from context menu; persisted.
- On: `setWindowFlags(FramelessWindowHint)`; Off: `setWindowFlags(Qt.Window)`. After either, re-set title "Ivory" and `show()` (flag changes hide the window in Qt). Applied identically to the detached chord window if it exists.
- Dragging while borderless: left-button press anywhere (window or any child, via event filter) records `dragPos = globalPos − frameGeometry.topLeft`; mouse-move with left button moves window to `globalPos − dragPos`; release clears. In the event filter, press returns "not consumed" (so keytoggle clicks still work), move returns "consumed". Note interplay: when keytoggle is on and borderless is on, a click both toggles a key and arms dragging.

## 4. PianoWidget (88-key keyboard)

### 4.1 Range & key classification
- MIDI notes 21 (A0) through 108 (C8), 88 keys total, **52 white keys**.
- White iff `note % 12 ∈ {0,2,4,5,7,9,11}` (C D E F G A B); else black.

### 4.2 Widget sizing
- Size policy Expanding×Fixed within the main window; minimum size 200×50; heightForWidth = `max(50, int(width / 8.6667))`. In practice the parent fixes its exact geometry (§3.1). Its own `resizeEvent` self-corrects height if off by >2 px.

### 4.3 Painting (paintEvent, antialiasing ON)
Let `W = widget width`, `H = widget height`.
1. Fill entire rect with `bg_color` (from parent: `#1a1a1a` dark / `#E8E8E8` light). This background is only visible if children ever mismatch the window; normally keys cover everything.
2. Key dimensions:
   - `whiteKeyW = W / 52` (float)
   - `whiteKeyH = H`
   - `blackKeyW = whiteKeyW * 0.7`
   - `blackKeyH = H * 0.65`
3. **White keys**, left→right for each white note (index `idx` 0..51), `x = idx * whiteKeyW`:
   - Fill rect `(int(x), 0, int(whiteKeyW), int(whiteKeyH))` with:
     - active + sustain pedal down → `sustain_color` (default `#D2A36C`)
     - active, pedal up → `white_key_active_color` (default `#6C9BD2`)
     - idle → light mode: `white_key_idle_color` (default `#E8DCC0`); **dark mode: `black_key_idle_color`** (default `#1a1a1a`) — dark mode *swaps* the idle key colors.
   - Separator line after every white key except the last: 1 px pen, vertical line at `x + whiteKeyW` from y=0 to H. Color: light mode `rgb(92,63,31)` = `#5C3F1F`; dark mode `rgb(153,153,153)` = `#999999`. (There is no outer border around white keys, and no line before the first or after the last key.)
4. **Black keys** on top. For each black note, compute `whiteKeysBefore` (number of white keys to its left):
   - Octave block `note//12 == 1` (i.e., A#0, note 22): `whiteKeysBefore = 0`.
   - Otherwise: start at 2 (for A0,B0), add `7 * (note//12 − 2)` for full octaves, then add per pitch class: C#→+0, D#→+1, F#→+3, G#→+4, A#→+5.
   - Skip unless both `whiteKeysBefore` and `whiteKeysBefore+1` are < 52 (all 36 black keys qualify).
   - X position: centered on the boundary between white keys `whiteKeysBefore` and `whiteKeysBefore+1`:
     ```
     gapCenter = (whiteKeysBefore*whiteKeyW + whiteKeyW + (whiteKeysBefore+1)*whiteKeyW) / 2
               = (whiteKeysBefore + 1) * whiteKeyW          // i.e., exactly on the separator line
     x = gapCenter − blackKeyW/2
     ```
     (Note: black keys are symmetrically centered on the boundary — no realistic C#/D# offset.)
   - Fill rect `(int(x), 0, int(blackKeyW), int(blackKeyH))` with:
     - active + pedal → `sustain_color`
     - active → `black_key_active_color` (default `#6C9BD2`, same as white active)
     - idle → light mode: `black_key_idle_color` (`#1a1a1a`); **dark mode: `white_key_idle_color`** (`#E8DCC0`) — swapped.
   - Then stroke a 1 px rectangle outline over the same rect: light mode `rgb(139,115,85)` = `#8B7355`; dark mode `rgb(204,204,204)` = `#CCCCCC`. Only black keys get outlines.
5. Velocity: MIDI velocity is *stored* per note (`{'velocity', 'time'}`) but **never affects rendering** — active keys are flat single-color fills. Manual (keytoggle) notes get velocity 64.

### 4.4 Highlight state semantics
- `active_notes: {note → {velocity, time}}` supplied by the main window every GUI tick; when keytoggle is enabled the widget merges `manual_notes` in (velocity 64, current time) before storing.
- Sustain pedal flag: while true, **every active key** (not just sustained ones) is drawn in `sustain_color`.
- Repaint via `update()` on every state set (effectively every 50 ms tick plus interactions).

### 4.5 Mouse interaction — keytoggle mode
- Only active when `keytoggle_enabled` (context menu toggle; persisted). Left click hit-tests a note and toggles membership in `manual_notes` (a set); notifies the main window, which refreshes GUI + chord detection immediately.
- Hit-testing (`_get_note_from_position(x, y)`), matching drawing math:
  - Recompute `whiteKeyW = W/52`, `blackKeyW = 0.7*whiteKeyW`, `blackKeyH = 0.65*H`.
  - If `y < blackKeyH`: check every black key (same x math as §4.3.4); if `keyX ≤ x ≤ keyX+blackKeyW` return that black note.
  - Else / no black hit: `whiteIdx = int(x / whiteKeyW)`; if in 0..51: when `y ≥ blackKeyH` return that white note directly; when `y < blackKeyH` return it only if x is NOT within any adjacent black key's span (checks black keys whose `whiteKeysBefore` equals `whiteIdx` or `whiteIdx`; the code compares `whiteKeysBefore == idx or whiteKeysBefore+1 == idx`).
  - Return None outside any key.
- Disabling keytoggle clears `manual_notes`.
- Right-click anywhere on the piano opens the app context menu (widget uses CustomContextMenu and forwards to the main window with global coords).
- No hover effects, no key press animation, no mouse-driven MIDI output, no drag-glissando (each toggle requires a fresh click).

## 5. Chord display (ChordLabelWidget)

### 5.1 Placement
- Attached mode: strip across the **top** of the main window, above the piano, full width, height `int(50 * scale)` (50 px at 100%).
- Hidden entirely when chord detection is disabled or the module is missing or the display is detached.

### 5.2 Widget constraints
- minHeight 50, maxHeight 500 (scaled by window % when attached: `int(50*scale)`/`int(500*scale)`), size policy Expanding×Expanding (attached; switched to Expanding×Preferred on re-attach). Contents margins 0. Right-click forwards to the app context menu.

### 5.3 Painting
- Antialiasing ON. Fill entire rect with **pure black `#000000`** — always, in both light and dark modes.
- If no current chord: nothing else drawn (solid black strip).
- Text color: **`#E8DCC0`** rgb(232,220,192) — always, both modes.

### 5.4 Font
- Point size: `max(12, int(height * 0.6))` (at 50 px height → 30 pt).
- Family selection, in order, using Qt `exactMatch()` (i.e., must resolve exactly, else next): **"Courier Prime"** → **"Courier New"** → **"monospace"**. Weight: **Normal (non-bold)**.
- Bundled font files exist in repo `fonts/CourierPrime-Regular.ttf` (+Bold/Italic/BoldItalic) but ivory.py never registers them with the font database — it relies on system installation. A faithful Rust port should bundle/load Courier Prime Regular.

### 5.5 Text layout / scaling
- Measure with font metrics `boundingRect(text)`.
- If `textWidth > 0.95 * widgetWidth`: shrink once — `fontSize = int(fontSize * (0.95*width)/textWidth)`, re-measure. (Single-pass scale, not a loop; no elision, no wrapping.)
- Draw at baseline: `x = (width − textWidth)/2`, `y = (height + textHeight)/2 − descent` (ints). Horizontally and vertically centered single line.

### 5.6 Chord text content
- Plain strings from `ChordDetector.detect_chord(set_of_note_numbers)`, e.g. `C`, `Cm`, `F#m7b5`, `Bb7(b9,#11,b13)`, `C6/9`, `CΔ7#11`, `C Ionian` (scale names for clustered note sets), interval names for 2 notes.
- Accidentals are **ASCII**: sharp = `#`, flat = lowercase `b` (root names via `NOTE_NAMES` / `NOTE_NAMES_FLAT` = C, Db, D, Eb, E, F, Gb, G, Ab, A, Bb, B). No ♯/♭ glyphs. Some qualities include the Greek **Δ** (U+0394) for maj7 — the chosen font must cover it.
- Flats vs sharps controlled by `prefer_flats` (default **true** = flats); toggling calls `chord_detector.set_note_preference(prefer_flats)` and refreshes the display immediately.
- Displayed chord is None (blank) when no notes are held.

### 5.7 Detached chord window
- Separate top-level `QMainWindow`, title **"Ivory"**, min size 300×100, margins 0, containing one ChordLabelWidget (minHeight 50, maxHeight unbounded 16777215, Expanding×Expanding). **User-resizable**, unlike the main window.
- Initial size: width = current main-window width; height = persisted `detached_chord_height` (default 150 used if the persisted value ≤ 0; persisted default is 50 — code path: `default_height = 150` overridden by `_detached_chord_height` when `> 0`, and it is initialized to 50 at UI init, so effectively 50 unless changed).
- Honors borderless mode (frameless + drag-anywhere with same drag logic; drag handlers installed via event filter on the central widget and chord widget).
- Right-click anywhere in it opens the main app context menu (right-button press is converted to a context-menu event and forwarded; the chord widget itself has NoContextMenu).
- When main window resizes (size % change), a 100 ms single-shot debounce timer resizes the detached window's *width* to match the main window (height preserved) if the difference > 5 px.
- Closing the detached window manually = re-attach: state flag cleared & saved, its height saved as `detached_chord_height`, chord label restored in main window (visible if detection enabled) with proportional height, main window resized to `chordH + pianoH`.
- Menu "Detach Chord Window": saves current attached label height into `_detached_chord_height`, hides the label, resizes main window down to piano-only height, creates the window. "Attach Chord Window": saves detached window height, closes it, restores label, resizes main window up.
- If `chord_window_detached` was persisted true, the detached window is recreated 100 ms after startup (single-shot timer), given detection available & enabled.
- Both the attached label and detached window are updated with the same chord string every detection tick; when detached, the attached label is not updated.

## 6. Context menu (the entire UI surface)

Opened by right-click on: main window, piano, chord label, or detached chord window (always at the global cursor position). Rebuilt from scratch each time (labels reflect current state; **no checkmarks anywhere** — toggles rename themselves).

### 6.1 Stylesheet (exact)
Dark mode: bg `#000000`, text `#E8DCC0`, separator `#E8DCC0`, selected-item bg `#1a1a1a`.
Light mode: bg `#E8DCC0`, text `#000000`, separator `#000000`, selected-item bg `#d4c8b0`.
```
QMenu { background-color: <bg>; color: <text>; border: 1px solid <bg>;
        font-family: "Courier Prime", "Courier New", Courier, monospace; font-weight: bold; }
QMenu::item { background-color: transparent; padding: 4px 20px 4px 20px;
              font-family: "Courier Prime", "Courier New", Courier, monospace; font-weight: bold; }
QMenu::item:selected { background-color: <selectedBg>; }
QMenu::separator { height: 1px; background-color: <sep>; margin: 1px 0px 1px 0px; }
```
(Border is 1 px solid in the same color as the background, i.e., visually borderless. Menu font is **bold** Courier Prime.)

### 6.2 Structure (top to bottom)
1. **Size** ▸ submenu: `50%`, `75%`, `100%`, `125%`, `150%`, `175%`, `200%` (each sets window size % — §3.2 — and saves).
2. ─ separator
3. **`Borderless`** (label when currently bordered) / **`Bordered`** (label when currently borderless) — toggles borderless mode.
4. ─ separator
5. **`Select MIDI Input...`** → dialog §7.1.
6. ─ separator
7. **`Set White Key Color...`** → color picker, title "Choose White Key Color", edits `white_key_idle_color`.
8. **`Set Black Key Color...`** → "Choose Black Key Color", edits `black_key_idle_color`.
9. ─ separator
10. **`Set Active Key Color...`** → "Choose Active Key Color", sets **both** `white_key_active_color` and `black_key_active_color` to the same chosen color (initial swatch = white active).
11. **`Set Sustain Color...`** → "Choose Sustain Pedal Color", edits `sustain_color`.
12. ─ separator
13. **`Dark Mode`** (when currently light) / **`Light Mode`** (when currently dark) — toggles dark mode.
14. ─ separator
15. **`Enable Keytoggle`** / **`Disable Keytoggle`** — toggles keytoggle mode.
16. *(Only if chord detector available:)*
    - ─ separator
    - **`Use Sharps (A#)`** (shown while flats preferred) / **`Use Flats (Bb)`** (shown while sharps preferred).
    - ─ separator
    - If detached: **`Attach Chord Window`**.
      Else: **`Disable Chord Detection`** / **`Enable Chord Detection`**; plus, when detection enabled, **`Detach Chord Window`**.
17. ─ separator
18. **`About`** → §7.3.
19. **`Reset Settings to Default`** → §9.
- Color pickers: native/Qt `QColorDialog.getColor(initial, parent, title)`; only applied if a valid color returned (Cancel = no-op). Every mutation immediately repaints and saves settings.
- Toggling chord detection also: shows/hides the label, clears displayed chord when disabling, and resizes the window (with chord: `chordH+pianoH`; without: `pianoH`) via the fixed-size + 10 ms reposition pattern.

## 7. Dialogs

### 7.1 Select MIDI Input
- If MIDI libs unavailable: information box, title **"MIDI Not Available"**, text `"MIDI libraries are not available.\n\nPlease install python3-mido and python3-rtmidi."`
- If no ports: information box, title **"No MIDI Input"**, text `"No MIDI input ports found!\n\nYou can still use keytoggle mode by enabling it in the context menu."`
- Otherwise modal `QDialog`, title **"Select MIDI Input"**, min width 400, default styling (NOT themed), vertical layout:
  1. Label `Select MIDI input port:`
  2. Label `Current: <port name>` (only if a port is currently open)
  3. `QListWidget` of port names (single selection)
  4. OK/Cancel `QDialogButtonBox`.
- On OK with a selection: stop thread flag, close old port (ignoring errors), open new port, remember its name, start a fresh reader thread. On failure: warning box titled **"MIDI Error"**, text `"Error opening MIDI port:\n{e}\n\nYou can still use keytoggle mode."` and the app is left with no port.

### 7.2 Already-running warning — see §2.2.

### 7.3 About dialog
- Modal `QDialog`, title **"About Ivory"**, min 400×150, themed by dark mode:
  - Dark: bg `#000000`, text `#E8DCC0`, button bg `#1a1a1a`, button hover `#2a2a2a`, button border `#E8DCC0`.
  - Light: bg `#E8DCC0`, text `#000000`, button bg `#d4c8b0`, button hover `#c0b49c`, button border `#000000`.
  - Stylesheet: QDialog/QLabel get bg+text colors; labels use font-family `"Courier Prime", "Courier New", Courier, monospace`, bold; QPushButton: themed bg/text, `border: 1px solid <buttonBorder>`, `padding: 4px 12px`, same bold monospace family, hover bg as above.
- Vertical layout, all fonts chosen with exactMatch fallback chain **Courier Prime → Courier New → Courier → monospace**, all **Bold**:
  1. `Ivory` — 16 pt, centered.
  2. `Simple MIDI Keyboard Monitor with Advanced Chord Detection` — 10 pt, centered.
  3. Link label, 10 pt, centered, external-link enabled, HTML: `<a href="https://shambhaline.neocities.org" style="color: <text>;">shambhaline@neocities.org</a>`.
  4. Stretch.
  5. `Version 1.1.0` — 8 pt, left-aligned.
  6. `QDialogButtonBox(Ok)` → closes.

## 8. Settings persistence

- File: **`~/.config/ivory/settings.json`** (all platforms — literally `Path.home()/".config"/"ivory"/"settings.json"`; parent dirs created on save). JSON, `indent=2`. Read once at startup; **any** parse/read error ⇒ all defaults. Written after every mutation (each toggle, color pick, size change, detach/attach, reset). Write errors silently ignored.
- Keys (exact names), types, defaults:

| key | type | default |
|---|---|---|
| `dark_mode` | bool | `false` |
| `white_key_idle_color` | string `#rrggbb` | `"#E8DCC0"` |
| `black_key_idle_color` | string | `"#1a1a1a"` |
| `white_key_active_color` | string | `"#6C9BD2"` |
| `black_key_active_color` | string | `"#6C9BD2"` |
| `sustain_color` | string | `"#D2A36C"` |
| `prefer_flats` | bool | `true` |
| `chord_detection_enabled` | bool | `true` |
| `window_size_percent` | int | `100` |
| `borderless_mode` | bool | `false` |
| `chord_window_detached` | bool | `false` |
| `detached_chord_height` | int (px) | `50` |
| `keytoggle_enabled` | bool | `false` |

- Colors are serialized via `QColor.name()` (lowercase `#rrggbb`); loading accepts any QColor-parsable string.
- Note: loaded `detached_chord_height` is overwritten to 50 during `init_ui` (assignment after load) — a quirk; the persisted value only survives within a session.

## 9. Reset Settings to Default
Sets every field in §8 to its default, applies borderless-off (re-show window), re-applies 100% size, resets chord detector flats preference, closes detached chord window if open (its close handler then re-attaches), shows chord label if enabled, clears keytoggle state in the piano, repaints colors, saves.

## 10. MIDI subsystem

- Library: `mido` + `python-rtmidi` backend, imported lazily. If import fails at startup connect time, the app **continues without MIDI** (keytoggle-only). (`--list` and the picker dialog surface the missing-dependency message instead.)
- **Auto-connect at startup** (when no `-p` given), priority over `mido.get_input_names()`:
  1. first port whose name contains `"USB-MIDI"`;
  2. else first containing `"Scarlett"` OR (`"USB"` AND `"MIDI"`);
  3. else the first port.
  If no ports, or open fails: run with no MIDI (no error dialog at startup).
- **No reconnect logic**: hot-plug is not detected; if the port dies the reader thread just exits silently. Reconnection is manual via the picker dialog.
- **Threading model**: one daemon thread blocking-iterates messages from the open input port (`for msg in inport`), mutating shared state (`active_notes` dict, `notes_to_release` set, `sustain_pedal_active` bool) without locks; the GUI thread reads that state on timers. Thread stops when `midi_thread_running` flips false or the port closes (exceptions swallowed).
- **Message handling** (channel is ignored — all channels merged):
  - `note_on` with velocity > 0 → `active_notes[note] = {velocity, time.time()}`; remove note from `notes_to_release`.
  - `note_off`, or `note_on` with velocity 0 →
    - pedal down: if note active, add to `notes_to_release` (keep sounding/highlighted);
    - pedal up: delete from `active_notes`, discard from `notes_to_release`.
  - `control_change` controller **64** (sustain): pedal state = `value ≥ 64`. On transition down→up: delete every `notes_to_release` member from `active_notes`, clear the set.
  - Everything else ignored. No aftertouch, no pitch bend, no all-notes-off handling.

## 11. Timers (complete list)

| Timer | Interval | Purpose |
|---|---|---|
| `update_timer` | **50 ms** repeating | Push `active_notes` + pedal state into PianoWidget (triggers repaint). |
| `chord_timer` | **100 ms** repeating (only if detector available) | Run chord detection over active ∪ manual notes; update attached label and/or detached window. |
| showEvent positioner | 10 ms single-shot | `_position_widgets` after window shown. |
| chord toggle repositioners | 10 ms single-shot | `_position_widgets` after enable/disable chord detection. |
| detached-window width sync | 100 ms single-shot (debounced restart) | Match detached chord window width to main window. |
| startup detach restore | 100 ms single-shot | Recreate detached chord window from persisted state. |

Keytoggle clicks additionally trigger an immediate (off-timer) GUI + chord-detection update.

## 12. Chord detection data flow
- Inputs: keys of `active_notes`, plus `manual_notes` (velocity 64) when keytoggle on.
- Empty set → chord None → blank display(s).
- Non-empty → `chord_detector.detect_chord(set)` → string or None → attached label (if not detached) and detached window (if present).
- Disabling detection blanks and hides the label; the 100 ms timer keeps running but returns early.

## 13. macOS specifics
- **There is no mac-specific code whatsoever** in ivory.py: no native menu bar entries, no dock/tray handling, no `.icns`/bundle metadata, no Cmd-key shortcuts, no `AA_*` attributes, no style/Fusion forcing, no HiDPI code (Qt6 handles DPR automatically — a Rust port must draw in logical pixels and honor the display scale factor). Repo ships only `icons/ivory.png` and Linux `.deb` packaging.
- Consequences to preserve on mac: default system menu bar shows just the app menu Qt auto-generates (About/Quit are NOT wired there — About only exists in the context menu); Cmd+Q from the system menu quits via normal close path; fixed-size window means the zoom (green) button behavior is disabled/no-op; frameless mode removes traffic lights entirely.
- Right-click = `contextMenuEvent`; ctrl-click on mac also triggers it (Qt default) — keep that.

## 14. ivory_pyqt5.py differences (older Windows port — for awareness only, do NOT copy)
- Window title `"Ivory - MIDI Keyboard Monitor"`; window is **resizable**, initial `resize(1300, 200)`, min 200×150.
- Chord font chain: Courier Prime → **Courier** → monospace, **Bold** (PySide6 version is Normal weight and inserts Courier New).
- Lacks: size % submenu, borderless mode, keytoggle, detached chord window, and the related settings keys (settings file has only dark_mode, 5 colors, prefer_flats, chord_detection_enabled).
- Same colors, same aspect math, same key drawing, same timers (50/100 ms), same single-instance key, same MIDI logic. Nothing mac-relevant in it.

## 15. Color reference table (defaults)

| Role | Light mode | Dark mode |
|---|---|---|
| White key idle | `#E8DCC0` | `#1a1a1a` (swapped) |
| Black key idle | `#1a1a1a` | `#E8DCC0` (swapped) |
| Active key (white & black) | `#6C9BD2` | `#6C9BD2` |
| Sustain-active key | `#D2A36C` | `#D2A36C` |
| Piano widget bg | `#E8E8E8` | `#1a1a1a` |
| White-key separator line | `#5C3F1F` | `#999999` |
| Black-key outline | `#8B7355` | `#CCCCCC` |
| Chord strip bg | `#000000` | `#000000` |
| Chord text | `#E8DCC0` | `#E8DCC0` |
| Menu/About bg | `#E8DCC0` | `#000000` |
| Menu/About text | `#000000` | `#E8DCC0` |
| Menu selected / button bg | `#d4c8b0` | `#1a1a1a` |
| Button hover | `#c0b49c` | `#2a2a2a` |

Custom key colors replace the defaults everywhere they're referenced, including the dark-mode swap (dark mode always renders white keys with `black_key_idle_color` and vice versa).
