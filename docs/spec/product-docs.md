# Ivory — Product & Context Documentation Distillation

Source repo: `/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory` (git, branch `main`, 30 commits).
Compiled 2026-07-28 from: README.md, CHANGELOG.md, `Ivory Info/IVORY_PROJECT_COMPLETE.md`, `Ivory Info/03_GitHub_and_Software_Center_Descriptions.md`, INSTALLATION_NOTES.md, PYQT5_PORT_SUMMARY.md, README_WINDOWS_TESTING.md, build_deb.sh, `.github/workflows/release.yml`, `fonts/`, `icons/`, and git history.

---

## 1. What the product is

**Ivory** — "Simple MIDI Keyboard Monitor with Advanced Chord Detection" (the About-dialog wording; marketing copy uses "Professional MIDI keyboard monitor"). A cross-platform desktop app that:

- Renders all **88 piano keys (A0–C8)** and lights them in real time from MIDI input.
- Runs the input through a **chord-detection engine (100+ chord types)**: triads (maj/min/dim/aug); 7ths (maj7, m7, dom7, half-dim, dim7); extensions (9, 11, 13); altered dominants (b9, #9, #11, b13 and combinations, incl. C9(b13)); add chords (add9, add11); sus (sus2, sus4, 9sus, 13sus); 6/9; inversions; slash chords; rootless voicings; scale detection for clustered notes.
- Chord labels use **parenthetical notation**: `C(add9)`, `Cm(add9)`, `C9(sus)`, `C7(b9)`, `C7(b9,#11)` — this was a deliberate v1.0.0 notation change from `Cadd9`/`C7b9` style.
- UI features: **detachable chord display window** (independent, borderless-capable), **dark mode** with theme-aware context menus (ivory-on-black / black-on-ivory), **sustain-pedal visualization**, customizable key/note colors, MIDI device selection, flats/sharps preference toggle, window size presets (50/75/100/125/150/175/200%), **borderless mode with drag support**, settings persistence.
- Settings: JSON at `~/.config/ivory/settings.json` (Linux/macOS) or `%USERPROFILE%\.config\ivory\settings.json` (Windows). Persists window size, borderless mode, chord-window state, colors, dark mode.
- CLI flags: `-p "<port name>"` to pick a MIDI port, `-l` to list ports.
- Keyboard shortcuts (README): Ctrl+D dark mode, Ctrl+C chord detection toggle, Ctrl+W chord window toggle, Ctrl+Q quit. Right-click opens the context menu (primary UI surface).
- Window title "Ivory"; blank allowed in borderless mode. `StartupWMClass=Ivory`.
- Stack today: **Python + PySide6** (`ivory.py`, ~89 KB) + `chord_detector.py` (~114 KB, the engine) + `mido` + `python-rtmidi`. License **MIT**. Author "Ganten" <ganten7@github.com>, GitHub `ganten7/ivory`.
- Claimed non-functional targets: **<5 ms latency** ("zero latency" in marketing), low CPU usage, ~5,000 LOC, 50+ documented test cases, 9 major chord-ambiguity special cases resolved (documented in `Ivory Info/01_Special_Cases_and_Resolutions.md`; engine internals in `02_Code_and_Logic_Summary.md`).

### Chord-engine subtleties fixed in 1.0.0 (rewrite must preserve)
- Minor 6th vs major 6th conflicts in closed voicings.
- 9sus vs add9 ambiguity resolved via **chord span**.
- Minor add9 slash notation, e.g. `Cm(add9)/G`.
- Inversion bonuses for triads and 7th chords (weighted-scoring pattern matcher with essential/optional intervals; recognizes jazz voicings that omit root/5th).
- Rootless voicing detection; scale-vs-chord disambiguation for clustered notes.

---

## 2. Version history / feature evolution

Three GUI-toolkit generations, all preserving the same chord engine and pixel-identical keyboard rendering:

1. **GTK3 original** (pre-1.0, file was `midi-monitor-chord.py`): GTK3 + Cairo + Pango + python-gi, GNOME-native, single-instance via `application_id`, X11 WM_CLASS/icon tricks. Marketing docs from this era still say "GTK3" in places — treat those mentions as stale.
2. **v1.0.0 (2025-12-13, tagged, released) — PyQt5 port**: `ivory_pyqt5.py` written for Windows testing, then became `ivory.py`. Mapping: Gtk.ApplicationWindow→QMainWindow, DrawingArea→QWidget.paintEvent, Cairo→QPainter, Pango→QFont/QFontMetrics, GLib timers→QTimer, contextMenuEvent for menus. Same JSON settings format. Lost vs GTK3: single-instance support (noted as addable via QSharedMemory), X11 icon/WM_CLASS specifics. Added: borderless mode, size presets, theme-aware menus, "Courier New font throughout" (bold for UI, non-bold for chord display).
3. **v1.1.0 (working tree, ~2025-12-31, UNCOMMITTED) — PySide6 migration**: `ivory.py` header now says "built with PySide6"; `requirements_pyqt5.txt` (name kept, contents changed) = `PySide6>=6.0.0`, `mido>=1.2.10`, `python-rtmidi>=1.4.9`. New `build_deb.sh` (default VERSION=1.1.0) bundles **Courier Prime** fonts and multi-size icons; `ivory_1.1.0_all.deb` built and present. **None of the 1.1.0 work is committed** (git status: modified ivory.py/requirements, untracked build_deb.sh, fonts/, Ivory Info/, build-deb/, release-artifacts/). CHANGELOG.md still only documents 1.0.0. The release workflow was never updated for PySide6 — it still installs/collects PyQt5.
   - Font shift across generations: Courier New (v1.0.0) → **Courier Prime** bundled (v1.1.0). Rewrite should standardize on Courier Prime (OFL, redistributable) since Courier New is not.

Marketing docs reference a "v1.0.1" (Flatpak AppData release notes, GitHub release-notes template, `Last Updated 2025-12-13, Version 1.0.1`) — apparently a planned patch (C9(b13), minor-add9 slash fix, parenthetical labels) that was folded into 1.0.0; no 1.0.1 tag exists.

### Planned features (CHANGELOG "Unreleased")
Polychord detection; quartal harmony support; recording & playback; MIDI output (chord suggestions); learning mode with interactive tutorials; cloud sync for settings; plugin system for custom chord patterns.

---

## 3. How each platform build was produced

### GitHub Actions (`.github/workflows/release.yml`)
Triggers: push of tag `v*`, or `workflow_dispatch`. Three build jobs + a release job. All use Python 3.10, checkout@v3, setup-python@v4, upload/download-artifact@v4, softprops/action-gh-release@v1 with `generate_release_notes: true`. Release job runs only on tags; manual runs fall back to VERSION=1.0.0.

**Linux (.deb, arch `all`)** — built inline in YAML (predates build_deb.sh):
- Layout: `usr/bin/ivory` (ivory.py, chmod +x), `usr/bin/chord_detector.py`, `usr/share/applications/ivory.desktop`, single 128x128 icon at `usr/share/icons/hicolor/128x128/apps/ivory.png`, `usr/share/doc/ivory/`.
- control: `Section: sound`, `Depends: python3 (>= 3.6), python3-pyqt5, python3-pyqt5.qtsvg` (stale — app is PySide6 now).
- Desktop entry: `Categories=AudioVideo;Audio;MIDI;Music;` (note: `MIDI` is not a valid freedesktop category; build_deb.sh corrected to `AudioVideo;Audio;Music;`), `Keywords=midi;keyboard;piano;music;chord;detection;ivory;monitor;88-key;visualizer;`, StartupWMClass=Ivory.
- Version extracted from `$GITHUB_REF` (`refs/tags/vX.Y.Z` → X.Y.Z).

**Local .deb (`build_deb.sh`, the newer/richer v1.1.0 recipe):**
- Adds icon sizes 16/32/48/64/128/256 generated from `icons/ivory.png` via ImageMagick `convert`, falling back to PIL, falling back to plain copy.
- Bundles Courier Prime `.ttf`/`.otf` into `usr/share/fonts/truetype/courier-prime/`.
- `DEBIAN/postinst`: `update-desktop-database`, `gtk-update-icon-cache -f /usr/share/icons/hicolor`, `fc-cache` for the bundled fonts, and a **PySide6 presence check** that prints: install with `sudo pip3 install --break-system-packages PySide6` (needed on externally-managed-environment Debian/Ubuntu; PySide6 has no apt package they relied on). Depends only on `python3 (>= 3.6)`, `Recommends: python3-mido, python3-rtmidi` — i.e. the .deb deliberately does NOT hard-depend on the GUI toolkit.
- Output `ivory_${VERSION}_all.deb` (~145–150 KB — app is tiny; the runtime is the heavy part).

**Windows (.exe)** — PyInstaller on windows-latest, PowerShell:
```
python -m PyInstaller --onefile --windowed --name ivory --clean --noupx --noconfirm
  --hidden-import chord_detector --collect-submodules chord_detector
  --hidden-import PyQt5.QtCore --hidden-import PyQt5.QtGui --hidden-import PyQt5.QtWidgets
  --hidden-import PyQt5.QtWidgets.QApplication --hidden-import PyQt5.sip
  --hidden-import mido --hidden-import mido.backends.rtmidi --hidden-import rtmidi
  --collect-all mido --collect-all rtmidi
  --exclude-module tkinter --exclude-module matplotlib --exclude-module numpy
  --exclude-module scipy --exclude-module pandas
  ivory.py
```
Post-step verifies `dist/ivory.exe` exists and is a PE (a past broken release shipped something that threw "JavaScript/Node.js error" — see pitfalls). README's manual recipe adds `--icon=icons/ivory.png --add-data "chord_detector.py;."` (`;` separator on Windows, `:` on mac). Expected size ~50–100 MB.

**macOS (.app → .zip primary, .dmg best-effort)** — PyInstaller on macos-latest:
- `--onedir --windowed --name Ivory` (onedir, unlike Windows' onefile), `--additional-hooks-dir=hooks` (copies `hook-PyQt5.QtBluetooth.py` in), `--add-data "icons:icons"`, same hidden-imports/collect-alls as Windows, plus excludes for problem Qt frameworks: QtBluetooth, QtNfc, QtWebSockets, QtWebEngine(Widgets), QtQuick, QtQml, Qt3D, QtGamepad, QtLocation, QtPositioning. **No icon flag** ("requires PIL and can cause issues" — .app ships without a proper .icns).
- Elaborate fallback in-YAML: if PyInstaller fails with the **QtBluetooth framework symlink FileExistsError**, it strips the offending `.framework` dirs and hand-assembles `Ivory.app` (Contents/MacOS + executable + `_internal` + QtCore/QtGui/QtWidgets frameworks, Info.plist written via `defaults write` + `plutil -convert xml1`, CFBundleIdentifier `com.github.ganten7.ivory`, LSMinimumSystemVersion 10.13, NSHighResolutionCapable).
- Verification: executable exists/exec-bit, `otool -L` shows Qt, Frameworks dir present; `xattr -cr Ivory.app` (pointless — quarantine is applied on download, not build).
- Packaging: `zip -r Ivory.zip Ivory.app` (primary, per INSTALLATION_NOTES "ZIP — Recommended"); `hdiutil create -volname "Ivory" -srcfolder Ivory.app -ov -format UDZO Ivory.dmg || continue` (DMG allowed to fail). **Unsigned, un-notarized** — users must right-click→Open or `xattr -rd com.apple.quarantine`.
- `IVORY_PROJECT_COMPLETE.md` says "macOS .dmg via py2app" — stale; actual builds are PyInstaller.

### CI pitfalls (30 commits, ~24 of them CI fixes — the war stories)
1. **artifact actions v3→v4** breakage right after initial commit.
2. **Windows/macOS builds disabled for the actual v1.0.0 ship** — Linux-only release; desktop builds were fought into shape afterward.
3. A stray attempt at "Windows build with GTK3 bundling" before committing to the Qt port.
4. **Windows "JavaScript/Node.js error"**: a release `ivory.exe` was mis-packaged (documented in INSTALLATION_NOTES as a known issue); fixes were `--noupx --clean` + explicit PE verification + `mido.backends.rtmidi` hidden import. Windows Defender also flags PyInstaller onefile exes.
5. **macOS `--collect-all PyQt5` = death by framework symlinks**: FileExistsError on `Versions/Current/Resources` in QtBluetooth et al. Fix evolution: targeted hidden-imports instead of collect-all → exclude-module list → custom PyInstaller hook (`hook-PyQt5.QtBluetooth.py`) → clean build dirs first → in-YAML manual .app assembly fallback.
6. **YAML heredoc hell**: multiple commits fixing inline Python/XML heredocs inside workflow YAML (Info.plist generation). Final answer: use macOS `defaults write` instead of heredoc'd Python/XML. Lesson: don't embed multi-line scripts in workflow YAML; ship script files.
7. **mido's rtmidi backend is loaded dynamically** — PyInstaller misses it without `--hidden-import mido.backends.rtmidi` (bit both Windows and macOS).
8. Temporary push-to-main trigger added for CI debugging, then removed (release on tags only).
9. Manual `workflow_dispatch` runs crashed on version extraction until a no-tag fallback was added.
10. Release job made to download the three artifacts separately for reliability.

### Rewrite-relevant distribution deltas
- Workflow still builds PyQt5 while the app is PySide6 → tagged 1.1.0 CI build would produce a broken or mismatched bundle. Workflow lacks the fonts and the multi-size icons that build_deb.sh has; unify on one .deb recipe.
- No code signing/notarization anywhere (macOS Gatekeeper friction is the #1 documented support issue), no Windows signing, no .icns/.ico proper icons.
- macOS Gatekeeper lore in INSTALLATION_NOTES: right-click→Open; `xattr -cr` / `sudo xattr -rd com.apple.quarantine`; run `Contents/MacOS/Ivory` from Terminal for diagnostics; `chmod +x Contents/MacOS/*` for permission issues. Windows: VC++ Redistributable pointer for "failed to initialize"; run-from-source as fallback.

---

## 4. Fonts and icons shipped

**Fonts** (`fonts/`, uncommitted, used only by the v1.1.0 build_deb.sh path):
- `CourierPrime-Regular.ttf`, `CourierPrime-Bold.ttf`, `CourierPrime-Italic.ttf`, `CourierPrime-BoldItalic.ttf` (also duplicated in `fonts/courier-prime-files/`).
- Full upstream source `fonts/CourierPrime-master/` (with **OFL.txt** — SIL Open Font License, redistribution OK) and `courier-prime.zip`. Upstream: https://github.com/quoteunquoteapps/CourierPrime.
- Referenced by: build_deb.sh copies `.ttf`/`.otf` to `/usr/share/fonts/truetype/courier-prime/`; postinst runs `fc-cache`. The app itself asks for a monospaced Courier-family font (v1.0.0 docs say "Courier New throughout; bold for UI, non-bold for chord display"). Windows/macOS builds do NOT bundle fonts (rely on system Courier New).
- Rewrite note: embed Courier Prime on all platforms for consistent metrics and clean licensing.

**Icons** (`icons/`):
- Single file `icons/ivory.png`, **543 bytes**, 128×128 RGBA. (This entry originally read "i.e. a tiny placeholder, not production art" — **wrong**, corrected 2026-08-11: it IS the shipped piano-keys artwork, merely very small. sha256 `0dc37a25…`; the Rust repo's `assets/ivory.png` is byte-identical.) Referenced by: README header image, PyInstaller `--icon=icons/ivory.png` (README manual recipes), macOS `--add-data "icons:icons"`, .deb hicolor icons (workflow: only 128x128 copy; build_deb.sh: resized 16–256 px from this one source), `Icon=ivory` in the .desktop file.
- No .icns, no .ico, no SVG. A rewrite needs real icon art at ≥256px (ideally 1024 for macOS).

---

## 5. Marketing copy / store descriptions worth preserving

All in `Ivory Info/03_GitHub_and_Software_Center_Descriptions.md` (keep the file; highlights below):

- **GitHub repo description (≤160 ch)**: "Professional MIDI keyboard monitor with advanced chord detection. 88-key visualization, chord analysis, dark mode. Linux, Windows, macOS."
- **Topics**: midi, keyboard, music, chord-detection, piano, gtk3 (stale), python, music-theory, midi-monitor, chord-analyzer, 88-keys, music-software.
- **Tagline**: "See every note. Understand every chord. Master music theory." One-liner: "🎹 Professional MIDI keyboard monitor with AI-powered chord detection."
- **GNOME/Ubuntu Software short (78 ch)**: "Professional MIDI monitor with 88-key display and intelligent chord detection". **Flathub (100 ch)** variant also present.
- **AppStream XML long description** (full feature list; ends with a GTK3/GNOME sentence — stale for the Qt app).
- **Flatpak AppData**: id `com.github.ganten7.Ivory`, categories Audio/Music/Utility, keywords, screenshot URL `raw.githubusercontent.com/ganten7/ivory/main/screenshots/main.png` (screenshots never made — README says "Coming soon!"), OARS content rating, a 1.0.1 release entry.
- **Snapcraft summary/description** ready to paste.
- **APT control description** (matches shipped .deb).
- **Microsoft Store listing** (title "Ivory - MIDI Keyboard Monitor", long description with full chord-type checklist, "MIDI input support via USB and Bluetooth", "PERFECT FOR" audience list) and **Mac App Store listing** (subtitle "88-Key MIDI Visualizer & Chord Detector", mentions CoreMIDI integration, native interface) — written speculatively; app never shipped to either store, and MAS would require signing/sandboxing the current build doesn't have.
- **Social templates**: Twitter/X 280-char launch post, LinkedIn announcement, Reddit r/python post (tech-stack framing: Python 3.6+, ~5k LOC, scoring-based chord matcher, real-time 88-key rendering).
- **Elevator pitch (30 s)** and "Feature Highlights" bullet block (SEE EVERY NOTE / UNDERSTAND EVERY CHORD / CUSTOMIZE / PROFESSIONAL QUALITY / CROSS-PLATFORM).
- **SEO keywords**: primary "MIDI keyboard monitor / chord detection software / piano visualizer / music theory tool"; long-tail like "software that detects piano chords", "jazz chord detection software". **Meta description (155 ch)** included.
- **GitHub release-notes template** with download-file naming convention: `ivory_X.Y.Z_all.deb`, `ivory-X.Y.Z.exe`, `Ivory-X.Y.Z.dmg`.
- Target audience, consistently: musicians, producers, music teachers, students learning music theory; jazz emphasis.

Caveats when reusing: purge GTK3/GNOME/gtk3 mentions, the py2app mention, and "AI-powered"/"zero latency" claims (it's a weighted pattern matcher; docs elsewhere say <5 ms).

---

## 6. Distribution / selling notes

- **Price/model**: free and open source, MIT, no monetization anywhere in the docs. GitHub Releases is the only actual channel shipped (v1.0.0: .deb only at launch; Windows/macOS artifacts added by later CI fixes).
- **Channels prepared but never used**: Flathub (AppData ready), Snapcraft (yaml snippet ready), Microsoft Store, Mac App Store (both listings drafted). AppStream metadata exists in the shipped .deb per IVORY_PROJECT_COMPLETE.md.
- **Blockers for real store distribution**: no signing (macOS Developer ID + notarization; MAS sandbox), no Windows code signing (SmartScreen/Defender friction already documented), the 543-byte 128×128 icon (real art, but too small to scale cleanly), no screenshots, PySide6-vs-workflow mismatch, PyInstaller-onefile AV false positives.
- **Support surface documented**: MIDI-device-not-found triage (both platforms), Gatekeeper bypass, VC++ redistributable, run-from-source fallback (`pip install -r requirements_pyqt5.txt && python ivory.py`).
- **Naming collision risk** (not in docs, worth flagging): "Ivory" clashes with Synthogy Ivory (a well-known commercial piano VST) — a rename or qualifier may be needed if selling.
- Repo housekeeping for a rewrite team: the canonical current source is the **uncommitted working tree** (PySide6 `ivory.py`, `chord_detector.py`, build_deb.sh, fonts/); git HEAD is the older PyQt5 state. `ivory_pyqt5.py` and `hook-PyQt5.QtBluetooth.py` are historical. `Ivory Info/01_Special_Cases_and_Resolutions.md` and `02_Code_and_Logic_Summary.md` are the chord-engine spec/test corpus — the single most valuable asset to carry into a rewrite.
