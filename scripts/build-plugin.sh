#!/usr/bin/env bash
# Build Tangent.vst3 and lay it out as a real VST3 bundle.
#
#   scripts/build-plugin.sh                 # host platform
#   scripts/build-plugin.sh macos           # universal (x86_64 + arm64), signed
#   scripts/build-plugin.sh windows         # cross-compiled from macOS
#   scripts/build-plugin.sh linux           # ON Linux only (see build_linux)
#
# Output: dist/plugin/<platform>/Tangent.vst3
#
# WHY THIS EXISTS RATHER THAN `cargo xtask bundle`
#
# nih-plug ships a bundler that produces the same directory layout, and this
# script was written against its source rather than instead of reading it. Two
# things stopped it being usable directly: it hardcodes
# `CFBundleIdentifier = com.nih-plug.<package>` and `CFBundleVersion = 1.0.0`,
# both of which would be false statements about this plugin, and adopting it
# means an `xtask` crate that would itself have to live outside the root
# workspace to keep the GPLv3 dependency tree quarantined. The layout below is
# taken verbatim from `nih_plug_xtask/src/lib.rs:660-730` and matches the VST3
# specification's bundle format.
#
# THE PLUGIN IS ITS OWN WORKSPACE. `plugin/Cargo.toml` opens with an empty
# `[workspace]` table so nih_plug, vst3-sys and baseview never enter the root
# Cargo.lock, and therefore never appear in the MIT app's THIRD-PARTY-LICENSES.
# Every cargo command below is run from `plugin/` for that reason.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

LIB="libtangent_vst3"       # cdylib basename cargo emits
NAME="Tangent"              # what the bundle and the host call it
VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
BUNDLE_ID="com.ganten.tangent.vst3"

# rustup's cargo, not Homebrew's: Homebrew rust shadows it on PATH and then
# cross-target std libraries are not found. Same guard as build-cross.sh.
TOOLCHAIN_BIN="$(rustup which cargo 2>/dev/null | xargs dirname || true)"
[ -n "$TOOLCHAIN_BIN" ] && export PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"

PLATFORM="${1:-$(uname -s | tr '[:upper:]' '[:lower:]')}"
case "$PLATFORM" in
  darwin) PLATFORM=macos ;;
esac

OUT="dist/plugin/$PLATFORM"
BUNDLE="$OUT/$NAME.vst3"

# write_plist — the macOS bundle metadata. `BNDL????` in PkgInfo and
# CFBundlePackageType is what tells macOS this is a plugin bundle rather than
# an application; a host that finds the wrong one silently skips the folder.
write_plist() {
  mkdir -p "$BUNDLE/Contents"
  printf 'BNDL????' > "$BUNDLE/Contents/PkgInfo"
  cat > "$BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>CFBundleExecutable</key>
    <string>$NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$NAME</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>NSHumanReadableCopyright</key>
    <string>Tangent $VERSION. GPL-3.0-or-later.</string>
    <key>NSHighResolutionCapable</key>
    <true/>
  </dict>
</plist>
PLIST
}

# ship_licences — the GPL obligation, travelling with the binary rather than
# assumed. This bundle is the one artifact in the project that is not MIT.
ship_licences() {
  mkdir -p "$BUNDLE/Contents/Resources"
  cp LICENSE-GPL-3.0 "$BUNDLE/Contents/Resources/LICENSE"
  cp LICENSING.md "$BUNDLE/Contents/Resources/LICENSING.md"
  cp assets/fonts/OFL.txt "$BUNDLE/Contents/Resources/OFL.txt"
}

build_macos() {
  echo "==> $NAME $VERSION — macOS VST3 (universal)"
  ( cd plugin
    for t in x86_64-apple-darwin aarch64-apple-darwin; do
      rustup target add "$t" >/dev/null 2>&1 || true
      cargo build --release --target "$t"
    done )

  rm -rf "$BUNDLE"
  mkdir -p "$BUNDLE/Contents/MacOS"
  # One binary for both architectures. A DAW is whichever it is, and a user on
  # Rosetta should not have to know which build they downloaded.
  lipo -create -output "$BUNDLE/Contents/MacOS/$NAME" \
    "plugin/target/x86_64-apple-darwin/release/$LIB.dylib" \
    "plugin/target/aarch64-apple-darwin/release/$LIB.dylib"
  write_plist
  ship_licences

  # Signing. Same identity resolution as build-macos.sh, and the same fallback:
  # an ad-hoc signature is enough for the machine that built it, which is what
  # matters for a local test. Distribution needs the Developer ID.
  local sign_id
  sign_id="${IVORY_SIGN_ID:-$(security find-identity -v -p codesigning 2>/dev/null \
    | grep 'Developer ID Application' | head -1 | sed -E 's/.*"(.*)"/\1/')}"
  if [ -n "$sign_id" ]; then
    echo "    signing as: $sign_id"
    codesign --force --options runtime --timestamp -s "$sign_id" "$BUNDLE"
  else
    echo "    no Developer ID — signing ad-hoc (loads locally, not distributable)"
    codesign --force --deep -s - "$BUNDLE"
  fi
  codesign --verify --strict --verbose=2 "$BUNDLE"

  echo "    lipo: $(lipo -archs "$BUNDLE/Contents/MacOS/$NAME")"
  echo "    -> $BUNDLE"
}

build_windows() {
  echo "==> $NAME $VERSION — Windows VST3 (x86_64)"
  ( cd plugin
    rustup target add x86_64-pc-windows-msvc >/dev/null 2>&1 || true
    cargo xwin build --release --target x86_64-pc-windows-msvc )

  rm -rf "$BUNDLE"
  # Windows VST3 is a BUNDLE DIRECTORY, not a bare DLL. The .dll is renamed to
  # .vst3 and buried under Contents/x86_64-win. Hosts do still accept a loose
  # .vst3 file, but the bundle is what the spec says and what the installer
  # can place identically on all three platforms.
  mkdir -p "$BUNDLE/Contents/x86_64-win" "$BUNDLE/Contents/Resources"
  cp "plugin/target/x86_64-pc-windows-msvc/release/tangent_vst3.dll" \
     "$BUNDLE/Contents/x86_64-win/$NAME.vst3"
  ship_licences
  echo "    -> $BUNDLE"
}

build_linux() {
  # Linux CANNOT be cross-built from macOS, and the reason is worth stating so
  # nobody spends an afternoon on it again. `baseview` links X11 through the
  # `x11` crate, whose build script calls `pkg-config`, which refuses to
  # cross-compile without a target sysroot:
  #
  #   pkg-config has not been configured to support cross-compilation
  #
  # That is the same class of blocker as ALSA for the standalone
  # (docs/RELEASE.md, "Cross-build blocker"), and the same answer applies:
  # build it on Linux. `scripts/build-linux-remote.sh` does that in one
  # command and brings the result back.
  if [ "$(uname -s)" != "Linux" ]; then
    cat >&2 <<'MSG'
The Linux plugin cannot be cross-built from macOS: baseview links X11 and the
x11 crate's build script needs a target sysroot for pkg-config.

  Build it on a Linux host instead, in one command:

      scripts/build-linux-remote.sh <ssh-host>

  which builds BOTH the standalone tarball and this bundle over there and
  rsyncs them back into dist/.
MSG
    exit 1
  fi

  echo "==> $NAME $VERSION — Linux VST3 (x86_64)"
  ( cd plugin && cargo build --release --target x86_64-unknown-linux-gnu )

  rm -rf "$BUNDLE"
  mkdir -p "$BUNDLE/Contents/x86_64-linux" "$BUNDLE/Contents/Resources"
  cp "plugin/target/x86_64-unknown-linux-gnu/release/$LIB.so" \
     "$BUNDLE/Contents/x86_64-linux/$NAME.so"
  ship_licences
  echo "    -> $BUNDLE"
}

mkdir -p "$OUT"
case "$PLATFORM" in
  macos)   build_macos ;;
  windows) build_windows ;;
  linux)   build_linux ;;
  *) echo "unknown platform: $PLATFORM (want macos, windows or linux)" >&2; exit 2 ;;
esac

echo
echo "Bundle contents:"
find "$BUNDLE" -type f | sed "s|^|  |"
