#!/usr/bin/env bash
# Build + package the Tangent Linux release NATIVELY (run this ON a Linux host).
#
# Cross-compiling from macOS fails because midir links ALSA and alsa-sys can't
# find an ALSA sysroot (see docs/RELEASE.md). Building on Linux sidesteps that
# entirely — this is the recommended Linux path.
#
#   scripts/build-linux-native.sh          # -> dist/tangent-<v>-linux-<arch>.tar.gz
#
# Build dependencies:
#   Void Linux:   sudo xbps-install -S base-devel rust cargo pkg-config \
#                    alsa-lib-devel libX11-devel libxcb-devel libxkbcommon-devel \
#                    wayland-devel MesaLib-devel libXcursor-devel libXrandr-devel \
#                    libXi-devel
#   Debian/Ubuntu: sudo apt install build-essential pkg-config libasound2-dev \
#                    libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
#                    libgl1-mesa-dev libxcursor-dev libxrandr-dev libxi-dev
# (Rust via rustup is fine too; the ALSA + X11/Wayland/GL -dev packages are the
#  ones that matter.)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ "$(uname -s)" != "Linux" ]; then
  echo "This script must run ON Linux (native build). For macOS/Windows use"
  echo "build-macos.sh / build-cross.sh. See docs/RELEASE.md."
  exit 1
fi

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
ARCH="$(uname -m)"   # x86_64 or aarch64

[ -f THIRD-PARTY-LICENSES ] || scripts/gen-third-party-licenses.sh

echo "==> Tangent $VERSION — Linux $ARCH (native)"
cargo build --release -p ivory

STAGE="dist/tangent-${VERSION}-linux-${ARCH}"
rm -rf "$STAGE"; mkdir -p "$STAGE/fonts"
cp target/release/tangent "$STAGE/"
cp assets/ivory.desktop "$STAGE/tangent.desktop"
cp assets/ivory.png "$STAGE/tangent.png"
cp assets/fonts/CourierPrime-Regular.ttf assets/fonts/CourierPrime-Bold.ttf \
   assets/fonts/OFL.txt "$STAGE/fonts/"
# Licences for the four fonts eframe's `default_fonts` embeds in the binary.
mkdir -p "$STAGE/font-licenses"
cp assets/font-licenses/*.txt "$STAGE/font-licenses/"
cp LICENSE THIRD-PARTY-LICENSES "$STAGE/"
# The same user-facing readme the macOS and Windows artifacts carry.
cp docs/ARTIFACT-README.md "$STAGE/README.txt"
tar -C dist -czf "${STAGE}.tar.gz" "tangent-${VERSION}-linux-${ARCH}"
rm -rf "$STAGE"
echo "==> ${STAGE}.tar.gz"
# `${STAGE}` and not a re-spelled path: this line said ivory-* long after the
# tar above started writing tangent-*, and under `set -e` a failed `ls` exits
# non-zero — so the script reported failure at the very end of a build that had
# completely succeeded. Derive the name, never retype it.
ls -lh "${STAGE}.tar.gz" | sed 's/^/    /'

# Prove the tarball actually contains a binary before anyone trusts it.
# build-cross.sh once shipped 85 KB Linux tarballs of nothing but fonts and
# licences for a week, because `set -e` does not apply inside a function called
# as `f || handler`. Cheap insurance against the same class of silence.
if ! tar -tzf "${STAGE}.tar.gz" | grep -q "/tangent\$"; then
  echo "FAIL: ${STAGE}.tar.gz contains no 'tangent' binary" >&2
  exit 1
fi
echo "    contains: $(tar -tzf "${STAGE}.tar.gz" | wc -l | tr -d ' ') entries, binary present"
