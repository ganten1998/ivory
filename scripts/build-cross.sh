#!/usr/bin/env bash
# Cross-build Tangent release artifacts for Linux and Windows from a macOS host.
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
    echo "NOTE: $ICON_SRC is the original 128x128 piano-keys icon (identical to"
    echo "      the Python app's). Frames above 128px are upscaled and look"
    echo "      soft; the artwork itself is intentional."
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

# sign_windows_exe <path/to/ivory.exe> — Authenticode-sign via Azure Artifact
# Signing (formerly Trusted Signing) using jsign, which unlike Microsoft's
# signtool runs on macOS/Linux. No certificate ever touches this machine: jsign
# sends the file hash to Azure, Microsoft's HSM signs it and returns the
# signature.
#
# Unconfigured => the exe ships UNSIGNED (today's behavior) and the build says
# so once. PARTIALLY configured => hard failure, because silently shipping an
# unsigned binary while believing it is signed is the worst outcome here.
#
# Setup (see docs/RELEASE.md § Windows):
#   brew install jsign            # needs a JRE
#   brew install azure-cli        # only for the default token path
#   export TRUSTED_SIGNING_ENDPOINT=https://eus.codesigning.azure.net
#   export TRUSTED_SIGNING_ACCOUNT=<your-account>
#   export TRUSTED_SIGNING_PROFILE=<your-certificate-profile>
#   # token: taken from `az account get-access-token` unless you set
#   # TRUSTED_SIGNING_TOKEN yourself (e.g. from a CI federated credential).
sign_windows_exe() {
  local exe="$1"
  local set_count=0
  for v in "${TRUSTED_SIGNING_ENDPOINT:-}" "${TRUSTED_SIGNING_ACCOUNT:-}" \
           "${TRUSTED_SIGNING_PROFILE:-}"; do
    [ -n "$v" ] && set_count=$((set_count + 1))
  done

  if [ "$set_count" = 0 ]; then
    echo "    (unsigned: TRUSTED_SIGNING_* not set — Windows will show SmartScreen)"
    return 0
  fi
  if [ "$set_count" != 3 ]; then
    echo "TRUSTED_SIGNING_* is only partially set — refusing to ship a silently" >&2
    echo "unsigned exe. Set ENDPOINT, ACCOUNT and PROFILE, or none of them." >&2
    return 1
  fi

  command -v jsign >/dev/null 2>&1 || {
    echo "jsign not found (brew install jsign) but TRUSTED_SIGNING_* is set." >&2
    return 1
  }

  local token="${TRUSTED_SIGNING_TOKEN:-}"
  if [ -z "$token" ]; then
    command -v az >/dev/null 2>&1 || {
      echo "Need an access token: install azure-cli, or set TRUSTED_SIGNING_TOKEN." >&2
      return 1
    }
    token="$(az account get-access-token \
      --resource https://codesigning.azure.net \
      --query accessToken -o tsv)" || {
      echo "az could not get a token — run 'az login' first." >&2; return 1; }
  fi

  echo "==> Signing $(basename "$exe") via Azure Artifact Signing"
  # RFC-3161 timestamp so the signature outlives the short-lived certificate.
  jsign --storetype TRUSTEDSIGNING \
        --keystore "$TRUSTED_SIGNING_ENDPOINT" \
        --storepass "$token" \
        --alias "${TRUSTED_SIGNING_ACCOUNT}/${TRUSTED_SIGNING_PROFILE}" \
        --tsaurl http://timestamp.acs.microsoft.com \
        --name "Tangent" \
        "$exe" || { echo "jsign failed — exe NOT signed" >&2; return 1; }

  # Best-effort local check; real proof is a first launch on Windows.
  if command -v osslsigncode >/dev/null 2>&1; then
    osslsigncode verify "$exe" 2>&1 | grep -iE "signature|signer|timestamp" \
      | head -4 | sed 's/^/    /' || true
  else
    echo "    (install osslsigncode to verify the signature here)"
  fi
}

package_linux() { # $1 = rust target, $2 = artifact arch name
  local target="$1" arch="$2"
  local stage="dist/tangent-${VERSION}-linux-${arch}"
  # NOTE: this function is invoked as `package_linux ... || handler`, which
  # DISABLES `set -e` inside the whole body (bash: errexit is suppressed in a
  # command that is the left operand of ||). Every failure must therefore be
  # checked by hand — otherwise a failed cargo build sails on and tar cheerfully
  # ships an archive containing licences and fonts but no program. That is
  # exactly what happened before 2.1.0.
  rm -rf "$stage" "${stage}.tar.gz"
  cargo zigbuild --release --target "${target}.${GLIBC}" -p ivory || return 1
  if [ ! -f "target/${target}/release/tangent" ]; then
    echo "!! no binary at target/${target}/release/tangent"
    return 1
  fi
  mkdir -p "$stage/fonts" || return 1
  cp "target/${target}/release/tangent" "$stage/" || { rm -rf "$stage"; return 1; }
  cp assets/ivory.desktop "$stage/tangent.desktop" || { rm -rf "$stage"; return 1; }
  cp assets/ivory.png "$stage/tangent.png" || { rm -rf "$stage"; return 1; }
  cp assets/fonts/CourierPrime-Regular.ttf assets/fonts/CourierPrime-Bold.ttf \
     assets/fonts/OFL.txt "$stage/fonts/" || { rm -rf "$stage"; return 1; }
  # Licences for the four fonts eframe's `default_fonts` embeds in the binary.
  # (errexit is off in this function — every command needs its own handler.)
  mkdir -p "$stage/font-licenses" || { rm -rf "$stage"; return 1; }
  cp assets/font-licenses/*.txt "$stage/font-licenses/" || { rm -rf "$stage"; return 1; }
  cp LICENSE THIRD-PARTY-LICENSES "$stage/" || { rm -rf "$stage"; return 1; }
  # The same user-facing readme the macOS and Windows artifacts carry.
  cp docs/ARTIFACT-README.md "$stage/README.txt" \
    || { rm -rf "$stage"; return 1; }
  tar -C dist -czf "${stage}.tar.gz" "tangent-${VERSION}-linux-${arch}" || {
    rm -rf "$stage" "${stage}.tar.gz"; return 1;
  }
  rm -rf "$stage"
  echo "==> ${stage}.tar.gz"
}

# Linux stages are non-fatal: midir links ALSA, and alsa-sys cannot cross-compile
# from macOS without an ALSA sysroot (see docs/RELEASE.md → "Cross-build blocker").
# Build Linux on Linux (CI with libasound2-dev) or provide a sysroot. Windows
# (no ALSA) still builds below regardless.
LINUX_OK=1
echo "==> Tangent $VERSION — Linux x86_64"
package_linux x86_64-unknown-linux-gnu x86_64 || { LINUX_OK=0; echo "!! Linux x86_64 build failed (ALSA sysroot? see docs/RELEASE.md)"; }

echo "==> Tangent $VERSION — Linux aarch64"
package_linux aarch64-unknown-linux-gnu aarch64 || { LINUX_OK=0; echo "!! Linux aarch64 build failed (ALSA sysroot? see docs/RELEASE.md)"; }

echo "==> Tangent $VERSION — Windows x86_64"
cargo xwin build --release --target x86_64-pc-windows-msvc -p ivory
WINSTAGE="dist/tangent-${VERSION}-windows-x86_64"
WINZIP="${WINSTAGE}.zip"
rm -rf "$WINSTAGE" "$WINZIP"
mkdir -p "$WINSTAGE"
cp target/x86_64-pc-windows-msvc/release/tangent.exe "$WINSTAGE/"
sign_windows_exe "$WINSTAGE/tangent.exe"
cp LICENSE THIRD-PARTY-LICENSES "$WINSTAGE/"
cp assets/fonts/OFL.txt "$WINSTAGE/"
# Licences for the four fonts eframe's `default_fonts` embeds in ivory.exe.
mkdir -p "$WINSTAGE/font-licenses"
cp assets/font-licenses/*.txt "$WINSTAGE/font-licenses/"
# User-facing instructions travel with the build (SmartScreen steps, connecting a
# keyboard, the teach/correct features, where settings live). Shipped as .txt, not
# .md: Windows has no default handler for .md.
cp docs/ARTIFACT-README.md "$WINSTAGE/README.txt"
# Archive whatever was staged above. A hand-maintained file list silently drops
# anything newly added — that is how the readme went missing from the macOS
# artifacts before 2.1.0.
(cd "$WINSTAGE" && zip -qr "$ROOT/$WINZIP" . -x '.DS_Store' '._*')
rm -rf "$WINSTAGE"
echo "==> $WINZIP"

ls -lh dist/tangent-"${VERSION}"-* 2>/dev/null | sed 's/^/    /'
[ "$LINUX_OK" = 1 ] || echo "NOTE: Linux artifacts were skipped (ALSA cross-build). Windows built OK."
