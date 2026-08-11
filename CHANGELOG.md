# Changelog

All notable changes to Ivory are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versions 1.x are the original Python/Qt app; 2.x is the Rust rewrite.

## [Unreleased]

### Fixed

- Scales and modes keep their reading when the run is closed off with its own
  octave. `C-D-E-F-G-A-B-C` now reads `C Ionian` instead of `CΔ13`, and the same
  goes for the natural minor, both pentatonics and both blues scales. A spread
  tertian stack is still named as a chord, so real `CΔ13` and `C6/9` voicings
  are unaffected.
- `C-D-E` reads `C(add9)`. It was being pulled into `D7(#11)` by an identity
  bonus that fired without the tritone that defines a #11 chord; the bonus now
  requires that tritone to actually sound. Slash upper structures such as
  `C7(#11)/Gb` are unchanged.
- A major add9 with its 3rd in the bass now reads as sus2 over that bass, rooted
  on the add9's own root: `C-E-G-D` over E is `C2/E`, and `C-Ab-Bb-Eb` is
  `Ab2/C`. Minor add9 voicings never trip this and still read `Xm(add9)/bass`.
- A bass-rooted maj7 shell that also carries the 6th/13th reads as a major 13th:
  `B-D#-G#-A#` is `BΔ13`, not `G#m(add9)/B`.
- A scale must now account for every note you are sounding. A six-note
  `C-D-E-F-G-A` is no longer named as the five-note `C Major Pentatonic` it
  merely contains — it reads `Dm11`. Genuine pentatonics and exact six-note
  scales are unchanged; all twelve pitch classes together read
  `Chromatic Scale`.

### Changed

- Chord-engine internals rewritten: pitch-class set algebra now runs on 12-bit
  masks instead of hash sets, so chord detection — which runs on every MIDI
  event — is allocation-free on its hot path and about 5–6× faster. Every name
  the engine produces is byte-identical to the previous release across the full
  13,133-voicing test corpus, in both the stock and Chord Learning builds.

## [2.1.0] - 2026-08-04

### Added

- **Chord Learning.** *Correct Chord Name…* lists the readings Ivory actually
  weighed for the voicing you are holding, with their scores, and trains a
  general preference toward the one you pick — so similar voicings shift too,
  rather than just the chord in front of you. Measured reach: one correction
  changes 1,182 of 13,133 corpus voicings (9%), often in unrelated keys.
- *Enable / Disable Chord Learning* in the context menu, and a **Forget
  Learning** button in *Manage Taught Chords…* that restores stock naming
  exactly — verified to restore all 13,133 corpus readings.
- *Manage Taught Chords…* now shows whether learning is on, how many
  corrections have been made, and which leanings have been picked up.
- Every training attempt reports its outcome in plain language: learned,
  already wins, outranked by a later naming rule, too far behind to nudge, or
  not one of the readings Ivory weighed.

### Fixed

- The re-ranker could blank the chord display. Candidate admission was applied
  after the learned adjustment, so a few ordinary corrections could push every
  candidate for a voicing below the threshold. Admission now uses the
  unadjusted score and only ranking uses the adjusted one: learning reorders
  readings, it can never eliminate one.
- Training matched the winning candidate by its final printed label, but slash
  notation, rootless-dominant renaming and dim/aug re-rooting all rewrite that
  label after scoring — so corrections silently did nothing on exactly the
  ambiguous voicings worth correcting.
- `overrides.json` is written with write-then-rename, so an interrupted write
  can no longer take your taught chord names with it.
- Packaging: the Linux tarball could be produced without a binary. A failed
  build fell through to `tar` and emitted an archive of fonts and licenses that
  looked releasable; every step is now checked and a failure leaves no archive
  behind.
- Packaging: the macOS zip and dmg were built from the app alone, so the
  bundled instructions never left the build machine.

## [2.0.0] - 2026-07-29

Ground-up rewrite in Rust. Same look, same behaviors, same settings file — no
Python runtime, no Qt, a single self-contained executable per platform.

### Added

- **Teach Chord Name…** — pin your own name to an exact voicing, optionally in
  all keys. Taught names are consulted before detection and stored in
  `~/.config/ivory/overrides.json`, separate from your settings.
- **Manage Taught Chords…** — review and delete taught names.
- **Reset Settings to Default** in the context menu.
- `custom_font_path` — an optional settings key pointing at any TTF/OTF on your
  machine, loaded ahead of the bundled fonts in every text style.
- Courier Prime (Regular and Bold) is embedded in the binary on all three
  platforms, so Ivory looks identical everywhere. `OFL.txt` and the font files
  ship in every release artifact.
- Real application icons: an `.icns` bundle on macOS and an embedded `.ico` on
  Windows.
- Single-instance handling via a lock file. A crashed instance no longer blocks
  relaunch.
- Any panic now surfaces as an "Ivory Error" dialog with the message and a
  backtrace instead of vanishing.

### Changed

- **Chord engine corrections.** The rewrite reproduces the Python engine's
  shipped behavior exactly, except where that behavior was a documented bug or
  an indefensible musical reading. Twenty-odd corrections, each audited against
  a 13,133-voicing corpus. Among them: a root-position `A-C-E-G` reads `Am7`
  rather than `C6`; `C-F-G-Bb` reads `C7sus4`; `C-G-A-D` reads `C6/9`;
  rootless dominants that contain the defining tritone resolve to the implied
  root, so `E-Bb-D` reads `C9`; and a reading may no longer hide a note you are
  actually sounding.
- Rendering is event-driven rather than a busy loop — repaints are scheduled
  from MIDI and timer events at the same cadences the Python app used.
- `detached_chord_height` is honored on load (the Python app overwrote it with
  50 on every start).
- Release builds are ad-hoc signed on macOS. Gatekeeper still blocks the first
  launch, and macOS 15 removed the right-click → Open bypass, so the route is
  System Settings → Privacy & Security → Open Anyway.

### Removed

- The Python runtime dependency. PySide6, mido and python-rtmidi are gone; the
  Linux build no longer needs a `pip install` to start.
- Courier New is never bundled or redistributed. Courier Prime replaces it.

> **One-way caveat when downgrading.** Ivory 2.x reads Python 1.1.0's
> `~/.config/ivory/settings.json` in place, so upgrading is seamless. Going back
> to Python 1.1.0 is not: it rewrites that file with its own fixed set of 13
> keys, discarding `custom_font_path` and any other key it does not recognize.
> Taught chord names live in a separate file, `~/.config/ivory/overrides.json`,
> which the Python app never touches.

## [1.1.0] - 2025-12-31

Final Python release.

### Changed

- Migrated from PyQt5 to PySide6.
- Courier Prime is bundled with the Linux package and installed to the system
  font path, replacing the reliance on a system Courier New.

### Added

- A richer `.deb` recipe: icons generated at 16–256 px, bundled fonts with a
  font-cache refresh, and a startup check for PySide6.

## [1.0.0] - 2025-12-13

First tagged release, ported from the original GTK3 prototype to Qt.

### Added

- Full 88-key MIDI keyboard visualization (A0–C8).
- Chord detection engine covering triads, sevenths, extended chords (9/11/13),
  altered dominants (b9, #9, #11, b13 and combinations), add chords, suspended
  chords (sus2, sus4, 9sus, 13sus), 6/9, inversions, slash chords and rootless
  voicings.
- Scale detection for clustered notes.
- Detachable chord display window, with borderless support.
- Dark mode with theme-aware context menus.
- Sustain-pedal visualization and customizable note colors.
- MIDI device selection, and a flats/sharps preference.
- Window size presets (50%, 75%, 100%, 125%, 150%, 175%, 200%) and borderless
  mode with drag support.
- Settings persistence at `~/.config/ivory/settings.json`.
- Cross-platform builds for Linux, Windows and macOS.

### Changed

- Chord labels moved to parenthetical notation: `Cadd9` → `C(add9)`,
  `Cmadd9` → `Cm(add9)`, `C9sus` → `C9(sus)`, `C7b9` → `C7(b9)`,
  `C7b9#11` → `C7(b9,#11)`.

### Fixed

- Minor 6th versus major 6th conflicts in closed voicings.
- 9sus versus add9 ambiguity, resolved using chord span.
- Minor add9 slash notation, e.g. `Cm(add9)/G`.
- Inversion bonuses for triads and 7th chords.
- Rootless voicing detection, and scale-versus-chord detection for clustered
  notes.
