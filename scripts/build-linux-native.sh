#!/usr/bin/env bash
# Build + package the Ivory Linux release NATIVELY (run this ON a Linux host).
#
# Cross-compiling from macOS fails because midir links ALSA and alsa-sys can't
# find an ALSA sysroot (see docs/RELEASE.md). Building on Linux sidesteps that
# entirely — this is the recommended Linux path.
#
#   scripts/build-linux-native.sh          # -> dist/ivory-<v>-linux-<arch>.tar.gz
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

echo "==> Ivory $VERSION — Linux $ARCH (native)"
cargo build --release -p ivory

STAGE="dist/ivory-${VERSION}-linux-${ARCH}"
rm -rf "$STAGE"; mkdir -p "$STAGE/fonts"
cp target/release/ivory "$STAGE/"
cp assets/ivory.desktop "$STAGE/"
cp assets/ivory.png "$STAGE/"
cp assets/fonts/CourierPrime-Regular.ttf assets/fonts/CourierPrime-Bold.ttf \
   assets/fonts/OFL.txt "$STAGE/fonts/"
cp LICENSE THIRD-PARTY-LICENSES "$STAGE/"
mkdir -p dist
tar -C dist -czf "${STAGE}.tar.gz" "ivory-${VERSION}-linux-${ARCH}"
rm -rf "$STAGE"
echo "==> ${STAGE}.tar.gz"
ls -lh "dist/ivory-${VERSION}-linux-${ARCH}.tar.gz" | sed 's/^/    /'
