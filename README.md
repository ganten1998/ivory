# Ivory

**See every note. Understand every chord.**

Ivory is a MIDI keyboard monitor with advanced chord detection. Plug in a MIDI
keyboard and Ivory renders all 88 keys (A0–C8) in real time while a
weighted-scoring chord engine names what you are playing — from plain triads
to altered dominants, rootless jazz voicings, slash chords, and scales.

Ivory 2.0 is a ground-up Rust rewrite of the Python app (final Python release:
1.1.0), keeping the same look, behaviors, and settings file. It runs on macOS,
Linux, and Windows.

## Features

- **Real-time 88-key display** — every key lights as you play it, with
  sustain-pedal visualization (held notes stay lit and recolor while the pedal
  is down).
- **Chord detection, 100+ chord types** — triads (maj/min/dim/aug), sevenths
  (maj7, m7, dom7, m7b5, dim7), extensions (9/11/13), altered dominants (b9,
  #9, #11, b13 and combinations), add chords, sus chords (sus2, sus4, 9sus,
  13sus), 6 and 6/9, inversions, slash chords, and rootless voicings.
  Clustered note runs are recognized as scales instead of chords.
- **Readable labels** — parenthetical notation: `C(add9)`, `C7(b9,#11)`,
  `Cm(add9)/G`, `CΔ7`, `6/9`. Flats/sharps preference toggle.
- **Teach Ivory your names** — right-click → *Teach Chord Name…* renames the
  voicing you are holding; check *apply in all keys* to make the name follow
  the shape through every transposition. *Manage Taught Chords…* lists and
  deletes taught names. Overrides are consulted before detection and stored in
  `~/.config/ivory/overrides.json`.
- **Detachable chord display** — pop the chord strip into its own
  independent window; close it to reattach.
- **Dark mode** with theme-aware context menus (ivory-on-black /
  black-on-ivory), customizable key and note colors.
- **Flexible window** — size presets from 50% to 200% (any percentage works in
  the settings file), borderless mode with drag-anywhere support.
- **MIDI device picker** with automatic connection to the first real keyboard
  found; CLI flags for scripted setups: `ivory -l` lists MIDI input ports,
  `ivory -p "<port name>"` connects to a specific one.
- **Consistent typography everywhere** — the Courier Prime fonts are bundled
  and embedded, so the app looks identical on all three platforms.

The right-click context menu is the primary UI surface — every feature above
is reachable from it.

## Pay what you can

Ivory is free software (MIT). If it earns a place in your practice room, a
pay-what-you-can contribution keeps it maintained.
*(Payment link coming with the 2.0 release.)*

## Building from source

Requires a stable Rust toolchain (rustup recommended).

```sh
cargo run -p ivory            # debug build, run directly
cargo build --release -p ivory
cargo test --workspace        # engine + parity test suite
```

Release packaging lives in `scripts/` (`build-macos.sh` for the macOS app
bundle, `build-cross.sh` for Linux/Windows cross-builds); see
`docs/RELEASE.md` for the full release procedure.

## Platform notes

- **macOS** — release builds are ad-hoc signed, not notarized, so Gatekeeper
  blocks the first launch. On macOS 15 (Sequoia) and later there is no
  right-click → Open bypass anymore: attempt to open the app once, then go to
  **System Settings → Privacy & Security** and click **Open Anyway**.
  (Alternatively: `xattr -d com.apple.quarantine /Applications/Ivory.app`.)
- **Windows** — the executable is unsigned, so SmartScreen shows "Windows
  protected your PC" on first run: click **More info → Run anyway**.
- **Linux** — untar the release archive and run `./ivory`. The archive
  includes `ivory.desktop`, an icon, and the fonts if you want a desktop
  entry; the fonts themselves are already embedded in the binary. Wayland and
  X11 are both supported (some tiling/Wayland compositors treat Ivory's fixed
  window sizes as advisory).

## Fonts and licensing

Ivory bundles and embeds **Courier Prime** (Regular and Bold), Copyright 2015
The Courier Prime Project Authors, licensed under the
[SIL Open Font License 1.1](https://openfontlicense.org). The license text
(`OFL.txt`) ships in every release artifact alongside the fonts; the fonts are
never sold standalone. Ivory does **not** bundle or redistribute Courier New.
A `custom_font_path` setting lets you point Ivory at any TTF/OTF installed on
your own machine instead.

## Settings compatibility with Ivory 1.1.0

Ivory reads and writes the same settings file as the Python app —
`~/.config/ivory/settings.json`, at that literal path on **all** platforms —
with the same keys and formats. Upgrading from Python Ivory 1.1.0 carries all
your settings over untouched. Ivory 2.0 adds a single optional key
(`custom_font_path`) and preserves any keys it doesn't recognize.

One-way caveat: if you later run Python Ivory 1.1.0 again, it rewrites the
file with its fixed key set, discarding `custom_font_path` and any other
additions. Taught chord names live in a separate file (`overrides.json`) that
the Python app never touches.

## License

MIT — see [LICENSE](LICENSE). Copyright (c) 2025-2026 Ganten.
Licenses of the Rust crates Ivory links against are collected in
[THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES).
