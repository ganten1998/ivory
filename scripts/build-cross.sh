#!/usr/bin/env bash
# Cross-build Ivory release artifacts for Linux and Windows from a macOS host.
#
#   scripts/build-cross.sh        # builds + packages everything into dist/
#   scripts/build-cross.sh ico    # regenerate assets/ivory.ico only (dry-test hook)
#
# Requirements (all via Homebrew/cargo):
#   brew install zig
#   cargo install cargo-zigbuild cargo-xwin
#   rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-msvc
#
# Note: if Homebrew's rust is also installed, its cargo/rustc shadow rustup's
# on PATH and cross-target std libs won't be found — hence the PATH prefix.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
GLIBC="2.32"   # floor for released Linux binaries
ICON_SRC="assets/ivory.png"

TOOLCHAIN_BIN="$(rustup which cargo 2>/dev/null | xargs dirname || true)"
[ -n "$TOOLCHAIN_BIN" ] && export PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"

# gen_ico — produce assets/ivory.ico from assets/ivory.png so ivory/build.rs
# (winres) can embed it in ivory.exe. ImageMagick when available (BMP frames
# for small sizes = maximum shell compatibility), else sips + a hand-packed
# ICO of PNG frames (valid on Windows Vista+).
gen_ico() {
  local out="assets/ivory.ico" tmp px
  if [ "$(stat -f%z "$ICON_SRC")" -lt 4096 ]; then
    echo "WARNING: $ICON_SRC is the 543-byte placeholder art. Icons built from"
    echo "         it will be blurry/unshippable. Replace with real >=1024px"
    echo "         artwork before releasing (see docs/RELEASE.md)."
  fi
  if command -v magick >/dev/null 2>&1; then
    magick "$ICON_SRC" -define icon:auto-resize=256,128,64,48,32,16 "$out"
  elif command -v convert >/dev/null 2>&1; then
    convert "$ICON_SRC" -define icon:auto-resize=256,128,64,48,32,16 "$out"
  else
    tmp="$(mktemp -d)"
    for px in 16 24 32 48 64 128 256; do
      sips -z "$px" "$px" -s format png --out "$tmp/$px.png" "$ICON_SRC" >/dev/null
    done
    /usr/bin/python3 - "$tmp" "$out" <<'PY'
import os, struct, sys
src, out = sys.argv[1], sys.argv[2]
sizes = [16, 24, 32, 48, 64, 128, 256]
entries, body = [], b''
offset = 6 + 16 * len(sizes)
for px in sizes:
    with open(os.path.join(src, f'{px}.png'), 'rb') as f:
        data = f.read()
    b = 0 if px == 256 else px  # 0 means 256 in ICONDIRENTRY
    entries.append(struct.pack('<BBBBHHII', b, b, 0, 0, 1, 32, len(data), offset))
    offset += len(data)
    body += data
with open(out, 'wb') as f:
    f.write(struct.pack('<HHH', 0, 1, len(sizes)) + b''.join(entries) + body)
print(f"  ico: {len(sizes)} frames -> {out}")
PY
    rm -rf "$tmp"
  fi
  echo "==> $out"
}

# Dry-test hook: regenerate the .ico only (exactly the shipped code path).
if [ "${1:-}" = "ico" ]; then
  gen_ico
  exit 0
fi

mkdir -p dist

# License bundle must exist before packaging.
[ -f THIRD-PARTY-LICENSES ] || scripts/gen-third-party-licenses.sh

# Windows icon must exist BEFORE cargo builds so build.rs embeds it.
gen_ico

package_linux() { # $1 = rust target, $2 = artifact arch name
  local target="$1" arch="$2"
  local stage="dist/ivory-${VERSION}-linux-${arch}"
  cargo zigbuild --release --target "${target}.${GLIBC}" -p ivory
  rm -rf "$stage"; mkdir -p "$stage/fonts"
  cp "target/${target}/release/ivory" "$stage/"
  cp assets/ivory.desktop "$stage/"
  cp assets/ivory.png "$stage/"
  cp assets/fonts/CourierPrime-Regular.ttf assets/fonts/CourierPrime-Bold.ttf \
     assets/fonts/OFL.txt "$stage/fonts/"
  cp LICENSE THIRD-PARTY-LICENSES "$stage/"
  tar -C dist -czf "${stage}.tar.gz" "ivory-${VERSION}-linux-${arch}"
  rm -rf "$stage"
  echo "==> ${stage}.tar.gz"
}

echo "==> Ivory $VERSION — Linux x86_64"
package_linux x86_64-unknown-linux-gnu x86_64

echo "==> Ivory $VERSION — Linux aarch64"
package_linux aarch64-unknown-linux-gnu aarch64

echo "==> Ivory $VERSION — Windows x86_64"
cargo xwin build --release --target x86_64-pc-windows-msvc -p ivory
WINSTAGE="dist/ivory-${VERSION}-windows-x86_64"
WINZIP="${WINSTAGE}.zip"
rm -rf "$WINSTAGE" "$WINZIP"
mkdir -p "$WINSTAGE"
cp target/x86_64-pc-windows-msvc/release/ivory.exe "$WINSTAGE/"
cp LICENSE THIRD-PARTY-LICENSES "$WINSTAGE/"
cp assets/fonts/OFL.txt "$WINSTAGE/"
(cd "$WINSTAGE" && zip -q "$ROOT/$WINZIP" ivory.exe LICENSE THIRD-PARTY-LICENSES OFL.txt)
rm -rf "$WINSTAGE"
echo "==> $WINZIP"

ls -lh dist/ivory-"${VERSION}"-* 2>/dev/null | sed 's/^/    /'
