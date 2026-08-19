#!/usr/bin/env bash
# Cross-build Tangent release artifacts for Linux and Windows from a macOS host.
#
#   scripts/build-cross.sh        # builds + packages everything into dist/
#   scripts/build-cross.sh ico    # regenerate assets/ivory.ico only (dry-test hook)
#   scripts/build-cross.sh check-linux   # TYPE-CHECK the Linux target from macOS
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

# check_linux — compile the Linux target from macOS WITHOUT an ALSA sysroot.
#
# Linux release binaries still cannot be cross-built here (see the note above
# package_linux), but a release build is not what is usually wanted: what is
# wanted is to know whether the Linux target still COMPILES before pushing, and
# for that the answer is yes.
#
# `cargo check` never links, so libasound.so does not have to exist — only
# `pkg-config --exists alsa` has to succeed, because that is all alsa-sys's
# build script tests. A three-line stub .pc file satisfies it.
#
# ALSA_NO_PKG_CONFIG does NOT work for this: alsa-sys panics on it deliberately
# ("Aborted because ALSA_NO_PKG_CONFIG is set", build.rs:13).
#
# Without this, Rust code that is Linux-only — the `#[cfg(unix)]` statvfs
# branch, the non-macOS camera stub — is never type-checked on this machine at
# all, and the first thing that finds a mistake in it is the Linux box.
check_linux() {
  local target="${1:-x86_64-unknown-linux-gnu}" pcdir
  pcdir="$(mktemp -d)"
  cat > "$pcdir/alsa.pc" <<'PC'
prefix=/usr
libdir=${prefix}/lib
includedir=${prefix}/include

Name: alsa
Description: stub for cross TYPE-CHECKING only; never linked against
Version: 1.2.11
Libs: -L${libdir} -lasound
Cflags: -I${includedir}
PC
  echo "==> type-checking $target (stub alsa.pc, no sysroot)"
  PKG_CONFIG_ALLOW_CROSS=1 \
  PKG_CONFIG_PATH="$pcdir" \
  PKG_CONFIG_SYSROOT_DIR=/ \
    cargo check --workspace --target "$target"
  local rc=$?
  rm -rf "$pcdir"
  return $rc
}

if [ "${1:-}" = "check-linux" ]; then
  check_linux "${2:-x86_64-unknown-linux-gnu}"
  exit $?
fi

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

# ensure_alsa_sysroot — the thing that made Linux cross-builds impossible here.
#
# midir and cpal both link ALSA, and alsa-sys's build script asks pkg-config for
# it. On a macOS host there is no ALSA for a Linux target, so every Linux stage
# of this script failed for the app's whole life and the artifacts were built on
# a Linux box instead. A sysroot fixes it: headers and a `libasound.so` to link
# against, which is all the linker wants — the real library comes from the
# user's own machine at run time.
#
# **Debian BULLSEYE, not bookworm, and that is the entire trick.** Bookworm's
# libasound is built against glibc 2.36 and its .so references `lstat64@2.33`,
# `dlsym@2.34` and friends; linking it while targeting the 2.32 floor this
# script sets fails with a page of undefined references that look like a broken
# toolchain and are actually a version skew. Bullseye is glibc 2.31, below the
# floor, so nothing it references is newer than what we target.
#
# Cached, because it is 400 kB a time and nobody wants a download in a release
# build. Delete the directory to refresh it.
ALSA_CACHE="${ALSA_CACHE:-$HOME/.cache/tangent/alsa}"
ensure_alsa_sysroot() { # $1 = debian arch (amd64|arm64)
  local darch="$1" root="$ALSA_CACHE/bullseye-$1"
  if [ -d "$root/usr/include" ]; then echo "$root"; return 0; fi
  local idx tmp fn base
  tmp="$(mktemp -d)" || return 1
  idx="$tmp/Packages.gz"
  curl -sSL --max-time 180 \
    "http://deb.debian.org/debian/dists/bullseye/main/binary-$darch/Packages.gz" \
    -o "$idx" || { rm -rf "$tmp"; return 1; }
  mkdir -p "$root" || { rm -rf "$tmp"; return 1; }
  # `libasound2` carries the .so.2, `libasound2-dev` the headers and the
  # development symlink. Both are needed: the linker follows the symlink and
  # the build script reads the headers.
  for pkg in libasound2 libasound2-dev; do
    fn="$(gunzip -c "$idx" | awk -v p="Package: $pkg" '''$0==p{f=1} f&&/^Filename: /{print $2; exit}''')"
    [ -n "$fn" ] || { rm -rf "$tmp" "$root"; return 1; }
    base="$tmp/$(basename "$fn")"
    curl -sSL --max-time 300 "http://deb.debian.org/debian/$fn" -o "$base" \
      || { rm -rf "$tmp" "$root"; return 1; }
    ( cd "$tmp" && ar x "$base" && tar -xf data.tar.* -C "$root" ) \
      || { rm -rf "$tmp" "$root"; return 1; }
  done
  rm -rf "$tmp"
  echo "$root"
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
  # Clear the STAGE, never the tarball. On a macOS host the Linux build is
  # EXPECTED to fail (alsa-sys has no sysroot), so removing the artifact up
  # front meant every single `build-cross.sh` run on this machine destroyed a
  # perfectly good Linux release that had been built on the Linux box and
  # rsynced back — silently, and before the failure that made it look like the
  # run had simply not produced one. The replacement happens at the end, and
  # only on success.
  rm -rf "$stage"
  # The sysroot, and the multiarch directory name inside it — which is the
  # Debian spelling of the target, not the Rust one.
  local darch multiarch sysroot
  case "$target" in
    x86_64-unknown-linux-gnu)  darch=amd64; multiarch=x86_64-linux-gnu ;;
    aarch64-unknown-linux-gnu) darch=arm64; multiarch=aarch64-linux-gnu ;;
    *) echo "!! no ALSA sysroot mapping for $target"; return 1 ;;
  esac
  sysroot="$(ensure_alsa_sysroot "$darch")" || {
    echo "!! could not fetch the ALSA sysroot for $darch"; return 1;
  }
  PKG_CONFIG_ALLOW_CROSS=1 \
  PKG_CONFIG_SYSROOT_DIR="$sysroot" \
  PKG_CONFIG_PATH="$sysroot/usr/lib/$multiarch/pkgconfig" \
  RUSTFLAGS="-L $sysroot/usr/lib/$multiarch" \
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
  # **The encoder, which this path shipped without for two releases.**
  #
  # Video on Linux is an ffmpeg subprocess, resolved exe-adjacent before PATH.
  # `build-linux-remote.sh` packed it and this one never did — so a tarball
  # built here was 10 MB instead of 45, video worked on any box that happened
  # to have distro ffmpeg, and broke on every clean one. Found by a tester on
  # a clean box; see docs/LINUX-4.11-FINDINGS.md, finding 2.
  #
  # x86_64 only: there is no aarch64 Linux build in `fetch-ffmpeg.sh` yet, and
  # shipping the wrong architecture is worse than shipping none — so that arch
  # says so out loud rather than looking complete.
  if [ "$arch" = "x86_64" ]; then
    scripts/fetch-ffmpeg.sh linux || { rm -rf "$stage"; return 1; }
    cp dist/vendor/linux/tangent-ffmpeg "$stage/" || { rm -rf "$stage"; return 1; }
    chmod 0755 "$stage/tangent-ffmpeg" || { rm -rf "$stage"; return 1; }
    cp -R dist/vendor/linux/ffmpeg-licenses "$stage/ffmpeg-licenses" \
      || { rm -rf "$stage"; return 1; }
  else
    echo "   (no aarch64 ffmpeg build yet: video needs ffmpeg on PATH there)"
  fi
  # The same user-facing readme the macOS and Windows artifacts carry.
  cp docs/ARTIFACT-README.md "$stage/README.txt" \
    || { rm -rf "$stage"; return 1; }
  # **The installer the README tells them to run.**
  #
  # `build-installer.sh` copied this in and this path never did, so every
  # tarball built here shipped a readme whose Linux section says
  # `./install.sh` next to a directory that did not contain one. The script
  # handles an app-only archive: the plugin is added by the installer path,
  # and its absence here is a note rather than a failure.
  cp installer/linux/install.sh "$stage/install.sh" || { rm -rf "$stage"; return 1; }
  chmod 0755 "$stage/install.sh" || { rm -rf "$stage"; return 1; }
  # **Everything the readme promises is actually here.** The readme is one
  # file shared by three platforms, so a name in it is a claim this stage has
  # to be able to meet — and the way that breaks is silently, one release at a
  # time, on the platform nobody packs by hand.
  for promised in tangent install.sh README.txt LICENSE; do
    if [ ! -e "$stage/$promised" ]; then
      echo "!! the readme promises $promised and the stage has no such file"
      rm -rf "$stage"; return 1
    fi
  done
  # **Readable by somebody other than the person who packed it.** A root
  # install of a tarball whose files are 0600 gives every other user on the
  # box an app they cannot read. Directories need the bit too, or nobody can
  # list them.
  chmod -R a+rX "$stage" || { rm -rf "$stage"; return 1; }
  # Written beside the real name and moved into place, so a tar that fails
  # half way leaves the previous artifact intact rather than a truncated one.
  # **No mac metadata in a Linux tarball.** bsdtar writes the host's extended
  # attributes as pax headers — Dropbox's, Apple's provenance, the lot — and GNU
  # tar on the far side prints a warning for every one of them. Forty lines of
  # "Ignoring unknown extended header keyword" is what somebody sees instead of
  # the file list, and it reads like the archive is damaged.
  COPYFILE_DISABLE=1 \
  tar --no-xattrs --no-mac-metadata \
      -C dist -czf "${stage}.tar.gz.new" "tangent-${VERSION}-linux-${arch}" || {
    rm -rf "$stage" "${stage}.tar.gz.new"; return 1;
  }
  mv -f "${stage}.tar.gz.new" "${stage}.tar.gz" || {
    rm -rf "$stage" "${stage}.tar.gz.new"; return 1;
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
# The bundled encoder, so video needs nothing installed — the app looks for
# `tangent-ffmpeg.exe` beside `tangent.exe` before falling back to PATH.
# Checksum-pinned and cached; offline after the first fetch.
scripts/fetch-ffmpeg.sh windows
cp dist/vendor/windows/tangent-ffmpeg.exe "$WINSTAGE/"
cp -R dist/vendor/windows/ffmpeg-licenses "$WINSTAGE/ffmpeg-licenses"
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
