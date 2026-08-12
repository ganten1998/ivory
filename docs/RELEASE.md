# Ivory — Release Checklist & Business Notes

This is the business/operational side of shipping Ivory 2.x. Technical design
is in `DESIGN.md`; historical CI war stories are in `spec/product-docs.md` §3.

## Hard gates before any public release

1. **Icon — informational, NOT a gate** (kept in this list so nobody re-files
   it as one). `assets/ivory.png` is the historical 543-byte, 128x128 art —
   verified 2026-08-04 to be **byte-identical to the Python app's**
   `icons/ivory.png` (sha256 `0dc37a25…`), i.e. the original piano-keys icon,
   which the owner wants kept. It was mislabelled a "placeholder" in earlier
   docs. Not a hard gate — but it is small, so icons scale up soft (a
   nearest-neighbour 8× re-render to 1024px would keep the art exactly and
   sharpen every frame). If a crisper icon is
   ever wanted, drop a **1024x1024+** PNG at that path and rerun
   `scripts/build-macos.sh` / `scripts/build-cross.sh` (the `.ico`/`.icns` are
   regenerated automatically; `build.rs` embeds the `.ico` into `ivory.exe`).
   The scripts print a size warning while the small art is in place — expected.
2. **Trademark decision.** "Ivory" collides with **Synthogy Ivory**, a
   well-known commercial piano VST — the same musicians-with-MIDI-keyboards
   market this app targets. Distributing a free hobby tool under the name is
   low-risk; *charging money* under it invites a dispute Synthogy would win on
   seniority in music software. Before turning on payments, decide: rename,
   add a qualifier, or knowingly accept the risk. Avoid marketing copy that
   pairs "Ivory" with "piano" in ways that suggest the VST.
3. **Test suite green.** `cargo test --workspace` alone is NOT this gate — it
   misses two things:
   - `differential::full` (the 13,133-row golden sweep) is `#[ignore]`d, so
     `--workspace` runs only `differential::fast_subset`, the first 1500 rows.
   - `--workspace` unifies features, so `ivory-core` is always compiled WITH
     `learning`; the stock engine path is never exercised.

   All three must be green:

   ```sh
   cargo test --workspace                                     # 14 GUI + 59 engine + 3 acceptance + 10 learning + differential(fast)
   cargo test -p ivory-core                                   # stock engine, no learning: 55 unit + 3 acceptance + differential(fast)
   cargo test -p ivory-core --test differential -- --ignored   # full 13,133-row golden sweep (~16 s)
   ```

   `ivory-core/tests/blast_radius.rs` is `#[ignore]`d too, but it is a
   measurement rather than a gate — run it only when the learning re-ranker
   changed: `cargo test -p ivory-core --features learning --test blast_radius
   --release -- --ignored --nocapture`.
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
an unsigned binary and resets on every release. A native Rust exe is far less
AV-prone than the old PyInstaller onefile bundles were.

**Signing landscape — checked 2026-08-11. None of this is verifiable from this
repo; it is external, it moves, re-confirm before spending money.**

- **EV no longer buys instant reputation.** Microsoft changed how SmartScreen
  treats EV code-signing certificates in **March 2024**. EV- and OV-signed
  builds now both have to accrue reputation like anything else. Do not buy EV
  expecting the warning to vanish on day one — that advice is pre-2024, and it
  is the single most common stale claim in signing write-ups (this document
  carried it until 2026-08-11).
- **Hardware is mandatory for OV too, not just EV.** Since **1 June 2023** the
  CA/Browser Forum requires code-signing private keys to be generated and held
  in a FIPS 140-2 Level 2 / Common Criteria EAL4+ crypto module — a CA-shipped
  USB token, an HSM, or a CA-hosted signing service. There is no "PFX on the
  laptop" option any more, so a cert purchase now implies token logistics.
  Retail OV is roughly **$200–400/yr**, and certificate lifetimes were capped
  at about one year in early 2026.
- **The cheap path is Microsoft's own service:** Azure **Trusted Signing**,
  renamed **Azure Artifact Signing** in 2026. Basic tier is about **$9.99/month**
  for up to 5,000 signatures; keys live in Microsoft's HSMs, so there is no
  token to buy, ship, or lose, and it drives `signtool` / CI directly.
  **Eligibility is the catch and it has changed repeatedly** — it has been
  restricted to US/Canada entities with at least three years of verifiable
  history, and individual-developer onboarding (opened Nov 2024) has been
  paused at least once. A solo author may or may not qualify on any given day:
  check <https://azure.microsoft.com/en-us/products/artifact-signing> and the
  Microsoft Learn docs BEFORE planning around it.

Recommendation unchanged: ship unsigned initially, keep the SmartScreen steps
in the README and in the shipped `README.txt`, and revisit if download volume
grows. If signing does become worth it, price Artifact Signing first — the
hardware mandate makes a retail cert materially more annoying than it was in
2022.

**The plumbing is already in place** (`sign_windows_exe` in
`scripts/build-cross.sh`). It uses **jsign**, not `signtool`: signtool is
Windows-only, and Ivory's Windows binary is cross-built from macOS, so signtool
would force a Windows VM or CI runner into the release path. jsign runs on
macOS/Linux and talks to Artifact Signing directly — the build machine never
holds a certificate; it sends the file hash and Microsoft's HSM returns the
signature.

To turn it on:

```sh
brew install jsign          # needs a JRE
brew install azure-cli      # only for the default token path; then: az login
export TRUSTED_SIGNING_ENDPOINT=https://eus.codesigning.azure.net   # your region
export TRUSTED_SIGNING_ACCOUNT=<account>
export TRUSTED_SIGNING_PROFILE=<certificate-profile>
scripts/build-cross.sh      # or scripts/release.sh
```

With none of those set the exe ships unsigned exactly as before, and the build
says so once. With *some* of them set the build **fails** rather than quietly
producing an unsigned binary you believe is signed. A token may be supplied
directly as `TRUSTED_SIGNING_TOKEN` (e.g. a CI federated credential) instead of
via `az`. Signatures are RFC-3161 timestamped so they outlive the short-lived
certificate. `osslsigncode`, if installed, prints a local verification — but the
real proof is a first launch on a real Windows machine.

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

**Fixed 2026-08-04:** that stage used to fail *silently*. `package_linux` is
called as `package_linux … || handler`, and bash suppresses `set -e` inside the
left operand of `||`, so the failed `cargo zigbuild` fell through to `tar` and
emitted an 85 KB `ivory-<v>-linux-<arch>.tar.gz` containing fonts and licences
but **no binary** — an artifact that looks releasable. Each step is now checked
explicitly and a failure leaves no archive behind. If you ever see a Linux
tarball under ~1 MB, it is empty: check it with `tar -tzvf` before uploading.

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
- **Fonts, part 2 — the egui defaults.** `eframe`'s `default_fonts` feature
  (enabled in `ivory/Cargo.toml`) statically embeds FOUR more fonts in the
  binary via `epaint_default_fonts`: **Ubuntu Light** (Ubuntu Font Licence
  1.0), **Noto Emoji** (OFL 1.1), **Hack** (MIT + Bitstream Vera), and
  **emoji-icon-font** (MIT). `lto` + `strip` do NOT remove them — all four are
  byte-for-byte present in `target/release/ivory`. Every artifact must
  therefore carry `font-licenses/` (all three build scripts do). Note that
  epaint's own `OFL.txt`/`UFL.txt` contain no copyright line, so the copyright
  notices those licences require live in `THIRD-PARTY-LICENSES` and
  `assets/font-licenses/NOTICE.txt`. **Do not turn `default_fonts` off** — the
  context-menu submenu arrow `⏵` (U+23F5, `menu.rs`) exists in *none* of the
  other five fonts.

## Settings one-way caveat (release-notes item)

2.0 reads Python 1.1.0's `~/.config/ivory/settings.json` in place — upgrade
is seamless. **Downgrading back to Python 1.1.0 rewrites the file with its
fixed key set**, discarding `custom_font_path` and any unknown keys. Taught
chords (`overrides.json`) are untouched by Python. State this in the release
notes for the **first public 2.x release** — nothing in the 2.x line has
shipped yet (newest tags are the Python-era `v1.0.0` / `v1.1`), so this caveat
lands with 2.1.0 or whatever version goes out first.

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

Linux binaries target glibc ≥ 2.32. Every artifact contains `README.txt` (the
user-facing guide generated from `docs/ARTIFACT-README.md` — per-platform first
launch, MIDI setup, privacy note), `LICENSE`, `THIRD-PARTY-LICENSES`, `OFL.txt`,
and the `font-licenses/` folder (four texts for the fonts egui embeds).
macOS/Linux also carry the Courier Prime TTFs; on Windows all six fonts are
embedded in the exe only — which is still redistribution, so the licence texts
ship there too. On macOS the licence files live inside
`Ivory.app/Contents/Resources/`; `README.txt` sits beside the app in the
zip/dmg.

## Release procedure

1. **Bump version** in the root `Cargo.toml` (`[workspace.package]`) — the
   single authority; scripts and the About dialog read it. Commit.
2. **Regenerate licenses**: `scripts/gen-third-party-licenses.sh`; commit if
   changed.
3. **Test**: all three commands from gate 3 — `cargo test --workspace`, then
   `cargo test -p ivory-core` (stock engine, no `learning` feature), then
   `cargo test -p ivory-core --test differential -- --ignored` (the full
   13,133-row golden sweep; `#[ignore]`d, so `--workspace` never runs it).
4. **Build macOS**: `scripts/build-macos.sh` (host arm64) and/or
   `ARCH=universal scripts/build-macos.sh`. The small-icon NOTE is expected
   (gate #1).
5. **Build Windows** (from mac): `scripts/build-cross.sh` (needs `zig`,
   `cargo-zigbuild`, `cargo-xwin`, the rustup targets — header lists them). It
   emits the Windows zip; its Linux stage is expected to fail on the ALSA
   cross-build (non-fatal).
   **Build Linux natively** on a Linux host (the owner's Void machine):
   `scripts/build-linux-native.sh` — installs are in that script's header
   (`alsa-lib-devel` + X11/Wayland/GL `-dev` packages). This is the recommended
   Linux path; run it once per arch (x86_64, aarch64) on matching hardware.
6. **Smoke-test locally**: `open dist/Ivory.app`; unzip the Windows artifact
   under CrossOver if available; `tar -tzf` the Linux archives.
7. **Checksums** — *version-scoped*. A bare `Ivory-*` sweeps in every previous
   release still sitting in `dist/`:
   `cd dist && shasum -a 256 *-<v>-* > SHA256SUMS`.
   Easier: `scripts/release.sh` does steps 4-7 in one pass — it purges other
   versions' artifacts, builds what this host can, checks every archive for the
   binary, readme and licences, and writes `SHA256SUMS` itself.
8. **Tag**: `git tag v<v> && git push --tags` (push to Codeberg — source of
   truth — and the GitHub mirror).
9. **GitHub release** (public download channel):
   `scripts/publish-github.sh --notes-file <notes>`.
   It uploads every artifact **twice** — version-scoped, and again under the
   version-less alias (`Ivory-macos-arm64.dmg`, `ivory-windows-x86_64.zip`, …)
   plus a bare `SHA256SUMS`. Those aliases are what
   `releases/latest/download/<name>` resolves by exact name, and that permalink
   is the download link handed to customers on the Gumroad post-purchase page,
   in the supporter-key email and in `README.md`. **A release published without
   the aliases 404s for everyone who has already paid, silently** — nothing logs
   it. The script re-checks every permalink after upload and exits non-zero if
   one is dead, so do not treat a red run as cosmetic.
   Release notes must include the Windows SmartScreen note and (for the first
   public 2.x release) the settings one-way caveat. The old macOS "Open Anyway"
   steps no longer apply: since 2.2.0 the build is Developer ID signed and
   notarized, so telling buyers to expect a Gatekeeper block is now wrong.
   Mirror the release on Codeberg or link the GitHub release from it.
10. **Verify as a stranger**: download each artifact on a clean machine/VM
    (quarantine attribute present!) and confirm the documented first-launch
    paths actually work.

## CI

Deferred until after the first successful local-script release. The scripts in
`scripts/` are the source of truth and run on this machine. The Python era
burned ~24 of 30 commits on workflow YAML (framework symlinks, heredoc
escaping, artifact-action churn — `spec/product-docs.md` §3); when CI returns,
the workflow must only *call these scripts*, never inline build logic in YAML.
