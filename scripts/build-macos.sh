#!/usr/bin/env bash
# Build Ivory.app for macOS and package it as a zip (primary) + DMG (best-effort).
#
# Usage:
#   scripts/build-macos.sh                 # release build, host arch
#   ARCH=universal scripts/build-macos.sh  # arm64 + x86_64 universal binary
#   scripts/build-macos.sh icns <out.icns> # icon generation only (dry-test hook)
#
# Output: dist/Ivory.app
#         dist/Ivory-<version>-macos-<arch>.zip
#         dist/Ivory-<version>-macos-<arch>.dmg   (best-effort)
#
# Requirements: rustup toolchain; Xcode CLT (codesign, ditto, sips, iconutil,
# hdiutil). For ARCH=universal:
#   rustup target add x86_64-apple-darwin aarch64-apple-darwin
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Homebrew rust shadows rustup's cargo on PATH; prefer the rustup toolchain
# so cross/std target libs resolve (navi pattern).
TOOLCHAIN_BIN="$(rustup which cargo 2>/dev/null | xargs dirname || true)"
[ -n "$TOOLCHAIN_BIN" ] && export PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
# Info.plist's version keys accept at most three period-separated integers, so a
# semver pre-release or build suffix ("2.2.0-beta.1", "2.2.0+ci7") is invalid
# there and notarization rejects it. Artifact FILENAMES keep the full string;
# only the bundle gets the numeric core. No-op on a plain release version.
VERSION_NUM="${VERSION%%-*}"; VERSION_NUM="${VERSION_NUM%%+*}"
# macOS icons are rounded squircles with padding, not full-bleed squares — using
# the raw square art makes Ivory look oversized next to every other Dock icon.
# assets/ivory-macos.png is the same artwork pre-composited into that shape.
# (build-cross.sh keeps assets/ivory.png: Windows/Linux want the square.)
ICON_SRC="assets/ivory-macos.png"

warn_placeholder_icon() {
  # assets/ivory.png is the ORIGINAL piano-keys icon, byte-identical to the
  # Python app's icons/ivory.png (verified 2026-08-04) — not placeholder art.
  # It is only 128x128, so Dock/Finder sizes above that are upscaled.
  if [ -f "$ICON_SRC" ] && [ "$(stat -f%z "$ICON_SRC")" -lt 4096 ]; then
    echo "NOTE: $ICON_SRC is the original 128x128 icon; sizes above 128 are"
    echo "      upscaled and will look soft in the Dock. A nearest-neighbour"
    echo "      1024px re-render of the same art would fix that (see"
    echo "      docs/RELEASE.md); the artwork itself is intentional."
  fi
}

# make_icns <output.icns> — render every Finder/Dock size with sips, then pack
# with iconutil; fall back to a hand-packed icns (navi pattern) because
# iconutil on recent macOS rejects iconsets whose files carry extended
# attributes it didn't write itself ("Invalid Iconset").
make_icns() {
  local out="$1" tmp iconset spec px name
  warn_placeholder_icon
  tmp="$(mktemp -d)"
  iconset="$tmp/ivory.iconset"
  mkdir -p "$iconset"
  for spec in 16:icon_16x16.png 32:icon_16x16@2x.png 32:icon_32x32.png \
              64:icon_32x32@2x.png 128:icon_128x128.png \
              256:icon_128x128@2x.png 256:icon_256x256.png \
              512:icon_256x256@2x.png 512:icon_512x512.png \
              1024:icon_512x512@2x.png; do
    px="${spec%%:*}"; name="${spec#*:}"
    sips -z "$px" "$px" -s format png --out "$iconset/$name" "$ICON_SRC" >/dev/null
  done
  if iconutil -c icns -o "$out" "$iconset" 2>/dev/null; then
    echo "==> $out (iconutil)"
  else
    echo "warn: iconutil rejected the iconset; packing icns manually"
    /usr/bin/python3 - "$iconset" "$out" <<'PY'
import os, struct, sys
src, out = sys.argv[1], sys.argv[2]
chunks = [  # standard icns chunk-type -> pixel-size mapping
    ('icp4', 'icon_16x16.png'),        # 16
    ('ic11', 'icon_16x16@2x.png'),     # 32
    ('icp5', 'icon_32x32.png'),        # 32
    ('ic12', 'icon_32x32@2x.png'),     # 64
    ('ic07', 'icon_128x128.png'),      # 128
    ('ic13', 'icon_128x128@2x.png'),   # 256
    ('ic08', 'icon_256x256.png'),      # 256
    ('ic14', 'icon_256x256@2x.png'),   # 512
    ('ic09', 'icon_512x512.png'),      # 512
    ('ic10', 'icon_512x512@2x.png'),   # 1024
]
body = b''
for code, fname in chunks:
    with open(os.path.join(src, fname), 'rb') as f:
        data = f.read()
    body += code.encode('ascii') + struct.pack('>I', len(data) + 8) + data
with open(out, 'wb') as f:
    f.write(b'icns' + struct.pack('>I', len(body) + 8) + body)
print(f"  icns: {len(chunks)} chunks -> {out}")
PY
  fi
  rm -rf "$tmp"
}

# Dry-test hook: `scripts/build-macos.sh icns <out.icns>` runs only icon
# generation (exactly the shipped code path) and exits.
if [ "${1:-}" = "icns" ]; then
  make_icns "${2:?usage: build-macos.sh icns <out.icns>}"
  exit 0
fi

ARCH="${ARCH:-host}"
case "$ARCH" in
  host)      ARCH_NAME="$(uname -m)" ;;
  universal) ARCH_NAME="universal" ;;
  *) echo "unknown ARCH=$ARCH (expected host|universal)" >&2; exit 2 ;;
esac

APP="dist/Ivory.app"
ZIP="dist/Ivory-${VERSION}-macos-${ARCH_NAME}.zip"
DMG="dist/Ivory-${VERSION}-macos-${ARCH_NAME}.dmg"

echo "==> Ivory $VERSION  arch=$ARCH"
mkdir -p dist

# License bundle must exist before packaging.
[ -f THIRD-PARTY-LICENSES ] || scripts/gen-third-party-licenses.sh

# ── Build ────────────────────────────────────────────────────────────────────
case "$ARCH" in
  host)
    cargo build --release -p ivory
    BIN="target/release/ivory"
    ;;
  universal)
    rustup target add x86_64-apple-darwin aarch64-apple-darwin >/dev/null 2>&1 || true
    cargo build --release -p ivory --target x86_64-apple-darwin
    cargo build --release -p ivory --target aarch64-apple-darwin
    mkdir -p target/universal/release
    BIN="target/universal/release/ivory"
    lipo -create -output "$BIN" \
      target/x86_64-apple-darwin/release/ivory \
      target/aarch64-apple-darwin/release/ivory
    ;;
esac

# ── Bundle ───────────────────────────────────────────────────────────────────
echo "==> Assembling $APP"
rm -rf "$APP" "$ZIP" "$DMG"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/ivory"
chmod +x "$APP/Contents/MacOS/ivory"

make_icns "$APP/Contents/Resources/AppIcon.icns"

# OFL compliance: license texts + the bundled fonts accompany every copy.
cp assets/fonts/CourierPrime-Regular.ttf assets/fonts/CourierPrime-Bold.ttf \
   assets/fonts/OFL.txt LICENSE THIRD-PARTY-LICENSES \
   "$APP/Contents/Resources/"
# eframe's `default_fonts` feature statically embeds FOUR more fonts in the
# binary (Ubuntu-Light, Noto Emoji, Hack, emoji-icon-font — verified present in
# the stripped release binary). Their licences must accompany every copy too.
mkdir -p "$APP/Contents/Resources/font-licenses"
cp assets/font-licenses/*.txt "$APP/Contents/Resources/font-licenses/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>Ivory</string>
    <key>CFBundleDisplayName</key>       <string>Ivory</string>
    <key>CFBundleExecutable</key>        <string>ivory</string>
    <key>CFBundleIconFile</key>          <string>AppIcon</string>
    <key>CFBundleIconName</key>          <string>AppIcon</string>
    <key>CFBundleIdentifier</key>        <string>org.codeberg.ganten1998.ivory</string>
    <key>CFBundleInfoDictionaryVersion</key> <string>6.0</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <key>CFBundleShortVersionString</key> <string>${VERSION_NUM}</string>
    <key>CFBundleVersion</key>           <string>${VERSION_NUM}</string>
    <key>LSMinimumSystemVersion</key>    <string>11.0</string>
    <key>NSHighResolutionCapable</key>   <true/>
    <key>LSApplicationCategoryType</key> <string>public.app-category.music</string>
    <key>NSPrincipalClass</key>          <string>NSApplication</string>
</dict>
</plist>
PLIST

# ALWAYS ad-hoc codesign. Gatekeeper still blocks first launch for downloads
# (macOS 15+ has no right-click-Open bypass — see docs/RELEASE.md), but an
# unsigned binary is strictly worse (killed outright on Apple Silicon).
codesign --force --deep -s - "$APP"
codesign --verify --strict "$APP"

# Bump bundle mtime so Icon Services notices a fresh app and reloads the icon.
touch "$APP" "$APP/Contents/Info.plist"

# ── Package ──────────────────────────────────────────────────────────────────
# Stage the app together with the tester instructions. ditto/hdiutil take a
# single source, so packaging "$APP" directly (as this did before 2.1.0) left
# READ-ME-FIRST.md sitting on the build machine while the comment claimed the
# hand-off explained itself. Windows testers got it; macOS testers did not.
STAGE="dist/Ivory-${VERSION}-macos-${ARCH_NAME}"
rm -rf "$STAGE"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Ivory.app"
# User-facing instructions ride alongside the .app (inside the bundle they would
# be invisible), so the zip/dmg hand-off explains itself. Shipped as .txt, not
# .md: every platform opens .txt on double-click; .md often has no handler.
cp docs/ARTIFACT-README.md "$STAGE/README.txt"

echo "==> Packaging $ZIP"
ditto -c -k --sequesterRsrc --keepParent "$STAGE" "$ZIP"
# ditto writes an AppleDouble stub for every entry (macOS 14+ stamps an
# unremovable com.apple.provenance xattr, and this tree lives in Dropbox, which
# adds more), which Windows/Linux users see as junk. KEEP --sequesterRsrc:
# without it those stubs land INLINE as "._ivory" beside the real files, which
# is worse; xattr -cr does not help either (provenance survives it). Delete the
# sequestered copy instead — verified the signature still validates after both
# `ditto -x -k` and `unzip`. zip -d exits 12 when nothing matches, hence || true.
zip -q -d "$ZIP" '__MACOSX*' >/dev/null 2>&1 || true

# The DMG gets an /Applications alias so the README's "drag Ivory.app into your
# Applications folder" has a visible target. Added AFTER the zip is written so
# the zip does not carry a stray symlink (it would be meaningless there).
ln -s /Applications "$STAGE/Applications"

echo "==> Packaging $DMG (best-effort)"
hdiutil create -volname "Ivory" -srcfolder "$STAGE" -ov -format UDZO "$DMG" \
  || echo "warn: DMG creation failed; the zip is the primary artifact"
rm -rf "$STAGE"

du -h "$APP/Contents/MacOS/ivory" "$ZIP" 2>/dev/null | sed 's/^/    /'
[ -f "$DMG" ] && du -h "$DMG" | sed 's/^/    /'
echo "==> Done. Open with:  open $APP"
