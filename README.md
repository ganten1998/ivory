# Ivory

**See every note. Understand every chord.**

Ivory is a MIDI keyboard monitor with advanced chord detection. Plug in a MIDI
keyboard and Ivory renders all 88 keys (A0–C8) in real time while a
weighted-scoring chord engine names what you are playing — from plain triads
to altered dominants, rootless jazz voicings, slash chords, and scales.

Ivory 2.x is a ground-up Rust rewrite of the Python app (final Python release:
1.1.0), keeping the same look, behaviors, and settings file. It runs on
macOS 11 (Big Sur) and later, on Windows, and on Linux (glibc 2.32+, ALSA,
Wayland or X11).

## Features

- **Real-time 88-key display** — every key lights as you play it, with
  sustain-pedal visualization (held notes stay lit and recolor while the pedal
  is down).
- **Chord detection — 95 chord patterns against all 12 roots** — triads
  (maj/min/dim/aug), sevenths (maj7, m7, dom7, m7b5, dim7), extensions
  (9/11/13), altered dominants (b9, #9, #11, b13 and combinations), add chords,
  sus chords (sus2, sus4, 9sus, 13sus), 6 and 6/9, inversions, slash chords,
  and rootless voicings.
- **28 scales and modes** — clustered note runs are named as scales rather than
  chords: the seven major modes, the melodic and harmonic minor modes, both
  pentatonics, both blues scales, whole tone, and both diminished scales.
- **Readable labels** — parenthetical notation: `C(add9)`, `C7(b9,#11)`,
  `Cm(add9)/G`, `CΔ7`, `6/9`. Flats/sharps preference toggle.
- **Teach Ivory your names** — right-click → *Teach Chord Name…* pins your own
  name to the voicing you are holding; tick *Apply in all keys* to make the name
  follow the shape through every transposition. *Manage Taught Chords…* lists
  and deletes taught names. Overrides are consulted before detection and stored
  in `~/.config/ivory/overrides.json`.
- **Chord Learning** — *Correct Chord Name…* shows the readings Ivory actually
  weighed for the voicing you are holding, with their scores. Pick the one you
  would rather see and Ivory learns a general leaning, so similar voicings shift
  too — a measured ~1 chord in 10 across a 13,133-voicing corpus, often in other
  keys. *Forget Learning* in *Manage Taught Chords…* restores stock naming
  exactly; *Disable Chord Learning* silences it without erasing anything.
- **Detachable chord display** — pop the chord strip into its own
  independent window; close it to reattach.
- **Dark mode** with theme-aware context menus (ivory-on-black /
  black-on-ivory), customizable key and note colors.
- **Flexible window** — size presets from 50% to 200% (any percentage works in
  the settings file), borderless mode with drag-anywhere support.
- **MIDI device picker** — Ivory auto-connects at startup, preferring ports
  named "USB-MIDI", then "Scarlett" or "USB"+"MIDI", and otherwise the first
  port it finds; switch at any time with *Select MIDI Input…* (there is no
  auto-reconnect — reopen the picker if a device is unplugged mid-session).
  CLI flags for scripted setups: `ivory -l` lists MIDI input ports,
  `ivory -p "<port name>"` connects to a specific one.
- **Consistent typography everywhere** — the Courier Prime fonts are bundled
  and embedded, so the app looks identical on all three platforms.

The right-click context menu is the primary UI surface — every feature above
is reachable from it.

## Pay what you can

Ivory is free software (MIT) and free to download — pay what you can, including
nothing at all. If it earns a place in your practice room, a contribution keeps
it maintained.

## Privacy

Ivory makes no network connections and collects no data. Your settings and your
taught chords stay on your machine, in `~/.config/ivory/`. There is no telemetry,
no update check, and no account. The only link anywhere in the app — the
author's site, in the About box — hands the address to your browser if you click
it; Ivory itself never opens a socket.

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
- **Linux** — untar the release archive, `chmod +x ivory`, and run `./ivory`.
  MIDI goes through ALSA, so `libasound.so.2` must be present (`libasound2` on
  Debian/Ubuntu, `alsa-lib` elsewhere); PipeWire systems work unchanged, since
  the keyboard still appears as an ordinary ALSA sequencer port. The archive
  includes `ivory.desktop`, an icon, and the fonts if you want a desktop entry;
  the fonts themselves are already embedded in the binary. Wayland and X11 are
  both supported (some tiling/Wayland compositors treat Ivory's fixed window
  sizes as advisory).

## Fonts and licensing

Ivory's executable embeds six font files, all free/libre. Every release
artifact carries the full license text for each one.

**Courier Prime** (Regular and Bold) is the UI typeface — Copyright 2015 The
Courier Prime Project Authors, licensed under the
[SIL Open Font License 1.1](https://openfontlicense.org). Its license text
(`OFL.txt`) ships alongside the fonts. Ivory does **not** bundle or
redistribute Courier New.

**Terminess Nerd Font Mono** is bundled as an optional UI typeface, selectable
from the context menu — Copyright (C) 2020 Dimitar Toshkov Zhekov and (C) 2023
Tilman Blumenbach, also under the SIL Open Font License 1.1. Its license text
ships as `font-licenses/Terminess-OFL-1.1.txt`.

Four more fonts come in automatically with eframe/egui's `default_fonts`
feature (the `epaint_default_fonts` crate) and serve as glyph fallback for
anything Courier Prime doesn't cover — the context menu's submenu arrow (⏵),
for instance, exists only in `emoji-icon-font`. Their license texts ship in the
`font-licenses/` folder of every artifact:

| Font | Copyright | License | Text |
|---|---|---|---|
| Ubuntu Light | 2011 Canonical Ltd. | Ubuntu Font Licence 1.0 | `font-licenses/Ubuntu-Font-Licence-1.0.txt` |
| Noto Emoji | 2013 Google Inc. | SIL OFL 1.1 | `font-licenses/NotoEmoji-OFL-1.1.txt` |
| Hack | 2018 Source Foundry Authors; 2003 Bitstream, Inc. | MIT + Bitstream Vera | `font-licenses/Hack-LICENSE.txt` |
| emoji-icon-font | 2014 John Slegers | MIT | `font-licenses/emoji-icon-font-MIT.txt` |

No font is modified, subset, or sold standalone. Ivory's own MIT grant
([LICENSE](LICENSE)) covers Ivory's code only — the embedded fonts remain under
the licenses above. A `custom_font_path` setting lets you point Ivory at any
TTF/OTF installed on your own machine instead.

## Settings compatibility with Ivory 1.1.0

Ivory reads and writes the same settings file as the Python app —
`~/.config/ivory/settings.json`, at that literal path on **all** platforms —
with the same keys and formats. Upgrading from Python Ivory 1.1.0 carries all
your settings over untouched. Ivory 2.x adds a single optional key
(`custom_font_path`, edited by hand — there is no UI for it) and preserves any
keys it doesn't recognize.

One-way caveat: if you later run Python Ivory 1.1.0 again, it rewrites the
file with its fixed key set, discarding `custom_font_path` and any other
additions. Taught chord names live in a separate file (`overrides.json`) that
the Python app never touches.

## Changes

Release history, including the Python lineage, is in
[CHANGELOG.md](CHANGELOG.md).

## License

MIT — see [LICENSE](LICENSE). Copyright (c) 2025-2026 Ganten.
Licenses of the Rust crates Ivory links against are collected in
[THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES).

MIT means you may rebuild, modify and redistribute Ivory freely. The
development repository is not currently public; source is available on request.
