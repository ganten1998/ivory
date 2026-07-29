# Ivory — Release Checklist & Business Notes

This is the business/operational side of shipping Ivory 2.x. Technical design
is in `DESIGN.md`; historical CI war stories are in `spec/product-docs.md` §3.

## Hard gates before any public release

1. **Placeholder icon.** `assets/ivory.png` is the historical 543-byte,
   128x128 placeholder. Every icon artifact (`.icns`, `.ico`, Linux PNG) is
   generated from it, and both build scripts print a warning while it is in
   place. Replace it with real artwork at **1024x1024 or larger**, then rerun
   `scripts/build-macos.sh` / `scripts/build-cross.sh` (the `.ico` is
   regenerated automatically; `build.rs` embeds it into `ivory.exe`).
2. **Trademark decision.** "Ivory" collides with **Synthogy Ivory**, a
   well-known commercial piano VST — the same musicians-with-MIDI-keyboards
   market this app targets. Distributing a free hobby tool under the name is
   low-risk; *charging money* under it invites a dispute Synthogy would win on
   seniority in music software. Before turning on payments, decide: rename,
   add a qualifier, or knowingly accept the risk. Avoid marketing copy that
   pairs "Ivory" with "piano" in ways that suggest the VST.
3. **Test suite green.** `cargo test --workspace`, including the differential
   golden corpus.
4. **THIRD-PARTY-LICENSES current.** Regenerate after any dependency change:
   `scripts/gen-third-party-licenses.sh`.

## Code signing & install friction

### macOS — the practical blocker

Release builds are **ad-hoc signed only** (`codesign --force --deep -s -`,
done unconditionally by `build-macos.sh` — an unsigned binary is killed
outright on Apple Silicon). Ad-hoc is not enough for strangers:

- Gatekeeper blocks the first launch of any downloaded, un-notarized app.
- **macOS 15 (Sequoia) and later removed the right-click → Open bypass.**
  Users must attempt the launch, then System Settings → Privacy & Security →
  "Open Anyway", then confirm again. Most non-technical users give up before
  step two. This is the #1 documented support issue from the Python era.
- Fix: Apple Developer Program (**$99/year**) → Developer ID Application
  certificate → sign → `notarytool submit` → staple. Decide before promoting
  Ivory anywhere strangers will download it. Until then, README carries the
  Open-Anyway instructions and the zip stays the primary artifact (DMG is
  best-effort).

### Windows

Unsigned exes trip **SmartScreen** ("Windows protected your PC" → More info →
Run anyway) and occasionally Defender heuristics. Reputation accrues slowly to
an unsigned binary and resets on every release. OV code-signing certs run
~$100–400/yr (EV more, but instant SmartScreen reputation). Acceptable to ship
unsigned initially; revisit if download volume grows. A native Rust exe is far
less AV-prone than the old PyInstaller onefile bundles were.

### Linux

No signing story required. tar.gz now; `.deb` (with a proper
`/usr/share/doc/ivory/copyright` folding MIT + OFL) once the tar.gz path is
proven.

**Cross-build blocker (verified 2026-07-29):** `midir` links **ALSA** on Linux,
and `alsa-sys`'s build script cannot cross-compile from macOS — pkg-config has
no ALSA sysroot for the Linux target (`cargo zigbuild` fails on `alsa-sys`).
macOS (native) and Windows (`cargo xwin`, no ALSA) both cross/native-build fine
from this Mac; only Linux is affected. Fix options, pick one before shipping
Linux:
1. **Build Linux on Linux** — a GitHub Actions / Codeberg CI job on
   `ubuntu-latest` with `libasound2-dev` installed (simplest; recommended, and
   matches the deferred-CI plan below).
2. Install an ALSA sysroot for the target and export `PKG_CONFIG_SYSROOT_DIR` +
   `PKG_CONFIG_PATH` + `PKG_CONFIG_ALLOW_CROSS=1` before `build-cross.sh`.
3. A prebuilt cross toolchain that bundles ALSA headers (e.g. a `cross` Docker
   image with `libasound2-dev`).
`scripts/build-cross.sh` still produces the Windows zip; its Linux stage will
fail loudly until one of the above is in place.

## Licensing & pay-what-you-can

- **MIT retention.** The Python releases (through 1.1.0) are public under MIT,
  and 2.0 keeps MIT for continuity (`LICENSE`, Copyright 2025-2026 Ganten).
  Implication: anyone may legally rebuild, fork, and redistribute Ivory for
  free, forever. Pay-what-you-can therefore sells **convenience and
  goodwill** (prebuilt, signed artifacts; support; the name) — not exclusivity.
  Do not promise otherwise in marketing. Relicensing later only binds new
  versions; it cannot recall what's already shipped under MIT.
- **Fonts.** Courier Prime is OFL 1.1: bundling with sold software is
  explicitly permitted at any price point, but every artifact must carry
  `OFL.txt` (all three build outputs do), the fonts may never be sold
  standalone, and modified/subset fonts would need renaming. **Never ship
  Courier New files** — referencing the name in a fallback is fine;
  redistributing Monotype's font software is not. See
  `spec/font-licensing.md`.

## Settings one-way caveat (release-notes item)

2.0 reads Python 1.1.0's `~/.config/ivory/settings.json` in place — upgrade
is seamless. **Downgrading back to Python 1.1.0 rewrites the file with its
fixed key set**, discarding `custom_font_path` and any unknown keys. Taught
chords (`overrides.json`) are untouched by Python. State this in the release
notes for 2.0.0.

## Release artifacts & naming

Built into `dist/` by the scripts; version is read from the root
`Cargo.toml` (`[workspace.package] version`):

| Artifact | Name |
|---|---|
| macOS app zip (primary) | `Ivory-<v>-macos-<arch>.zip` (`arm64`, `x86_64`, or `universal`) |
| macOS DMG (best-effort) | `Ivory-<v>-macos-<arch>.dmg` |
| Linux x86_64 | `ivory-<v>-linux-x86_64.tar.gz` |
| Linux aarch64 | `ivory-<v>-linux-aarch64.tar.gz` |
| Windows x86_64 | `ivory-<v>-windows-x86_64.zip` |
| Checksums | `SHA256SUMS` |

Linux binaries target glibc ≥ 2.32. Every artifact contains `LICENSE`,
`THIRD-PARTY-LICENSES`, and `OFL.txt` (macOS/Linux also carry the Courier
Prime TTFs; on Windows the fonts are embedded in the exe only).

## Release procedure

1. **Bump version** in the root `Cargo.toml` (`[workspace.package]`) — the
   single authority; scripts and the About dialog read it. Commit.
2. **Regenerate licenses**: `scripts/gen-third-party-licenses.sh`; commit if
   changed.
3. **Test**: `cargo test --workspace` (all three layers green).
4. **Build macOS**: `scripts/build-macos.sh` (host arm64) and/or
   `ARCH=universal scripts/build-macos.sh`. Heed any placeholder-icon warning
   (gate #1).
5. **Build Linux + Windows**: `scripts/build-cross.sh`
   (needs `zig`, `cargo-zigbuild`, `cargo-xwin`, and the three rustup targets
   — the script header lists the install commands).
6. **Smoke-test locally**: `open dist/Ivory.app`; unzip the Windows artifact
   under CrossOver if available; `tar -tzf` the Linux archives.
7. **Checksums**: `cd dist && shasum -a 256 Ivory-* ivory-* > SHA256SUMS`.
8. **Tag**: `git tag v<v> && git push --tags` (push to Codeberg — source of
   truth — and the GitHub mirror).
9. **GitHub release** (public download channel):
   `gh release create v<v> dist/Ivory-* dist/ivory-* dist/SHA256SUMS
   --title "Ivory <v>" --notes-file <notes>`. Release notes must include the
   macOS Open-Anyway steps, the Windows SmartScreen note, and (for 2.0.0) the
   settings one-way caveat. Mirror the release on Codeberg or link the GitHub
   release from it.
10. **Verify as a stranger**: download each artifact on a clean machine/VM
    (quarantine attribute present!) and confirm the documented first-launch
    paths actually work.

## CI

Deferred until after the first successful local-script release. The scripts in
`scripts/` are the source of truth and run on this machine. The Python era
burned ~24 of 30 commits on workflow YAML (framework symlinks, heredoc
escaping, artifact-action churn — `spec/product-docs.md` §3); when CI returns,
the workflow must only *call these scripts*, never inline build logic in YAML.
