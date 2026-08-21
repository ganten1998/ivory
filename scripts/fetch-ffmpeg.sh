#!/usr/bin/env bash
# Fetch the video encoder Tangent BUNDLES, so the artifact works on a machine
# with nothing installed.
#
#   scripts/fetch-ffmpeg.sh linux     -> dist/vendor/linux/tangent-ffmpeg
#   scripts/fetch-ffmpeg.sh windows   -> dist/vendor/windows/tangent-ffmpeg.exe
#
# macOS needs nothing here: its encoder is AVFoundation, in the OS.
#
# WHY BUNDLED AT ALL. The ffmpeg backend spawns an external `ffmpeg`, and
# "install it with your package manager" is exactly the step the packaging
# requirement says a user must never need. The app looks for `tangent-ffmpeg`
# beside its own binary before falling back to PATH — see `program()` in
# ivory-record/src/encode/ffmpeg.rs, including why the name is prefixed.
#
# PINNED BY CHECKSUM, CACHED IN dist/vendor. A moving "latest" URL is a supply
# chain risk and an unreproducible build in one; these are exact versioned
# files with their sha256 recorded here. Bumping the version means changing
# the URL AND the hash in the same edit, on purpose. The downloads are cached,
# so packaging is offline after the first run.
#
# LICENSING. Both builds are GPL ffmpeg. Tangent invokes the binary as a
# separate process, which keeps the app MIT (LICENSING.md's reasoning for the
# GPL plugin applies in reverse). The build's own licence text ships beside it
# in the artifact, and gen-third-party-licenses.sh names the bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# ffmpeg 8.1.2, linux64, BtbN's autobuilds — THE SAME BUILD AS WINDOWS, from
# the same immutable dated tag.
#
# **This was johnvansickle's static build, and that build has no VA-API in it.**
# Not a missing encoder: no `--enable-vaapi` in its configure line at all, and
# no `h264_vaapi` in the binary. Those builds are made for maximum portability
# and deliberately leave out anything that needs a runtime library.
#
# The consequence was invisible and expensive. Tangent resolves its encoder as
# `$IVORY_FFMPEG` -> `tangent-ffmpeg` beside its own binary -> `ffmpeg` on PATH,
# so the bundled one WINS — and every Linux install has therefore been encoding
# takes on the CPU whether or not the machine had a perfectly good hardware
# encoder and a system ffmpeg that could reach it. Measured on a 2012 Intel
# GPU: 25.54 s of CPU per 10 s of 720p against VA-API's 0.70, which is 36x, on
# a two-core machine that also has to run a synth.
#
# BtbN's build carries `--enable-vaapi --enable-libdrm --enable-vulkan`,
# `h264_vaapi`, `scale_vaapi` and `hwupload`, and it dlopens `libva.so.2` — so
# a machine without libva degrades to software instead of refusing to start.
# Its glibc floor is 2.28, which clears the 2.32 target `build-cross.sh` builds
# against and every distribution this app claims to support.
#
# The cost is size: 138 MB against 76. That is the whole trade, and hardware
# encode being IMPOSSIBLE out of the box is worth 62 MB.
LINUX_FILE="ffmpeg-n8.1.2-44-g7c533d0f86-linux64-gpl-8.1.tar.xz"
LINUX_URLS="https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-17-13-05/$LINUX_FILE"
LINUX_SHA="802a6ad62d310814a42c4aea4a95354f4a5e04bd3e792c4ca55970d25577808b"

# ffmpeg 8.1.2, win64, BtbN's autobuilds — an IMMUTABLE dated tag, never the
# rolling `latest` release, whose assets are rebuilt in place.
WIN_FILE="ffmpeg-n8.1.2-44-g7c533d0f86-win64-gpl-8.1.zip"
WIN_URLS="https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-17-13-05/$WIN_FILE"
WIN_SHA="19b9b43e6df8839473ba22c8e22bf14b937c1e2ca40ecbd19d58afedc83ac908"

PLATFORM="${1:-}"
case "$PLATFORM" in
    linux|windows) ;;
    *) echo "usage: scripts/fetch-ffmpeg.sh <linux|windows>" >&2; exit 2 ;;
esac

# macOS (where the Windows artifact is packaged) ships `shasum`, Linux ships
# `sha256sum`; this is the one place the difference matters.
sha_ok() { # sha_ok <file> <sha256>
    local got
    if command -v sha256sum >/dev/null 2>&1; then
        got="$(sha256sum "$1" | awk '{print $1}')"
    else
        got="$(shasum -a 256 "$1" | awk '{print $1}')"
    fi
    [ "$got" = "$2" ]
}

fetch() { # fetch <file> <sha256> <url...>
    local file="$1" sha="$2" url
    shift 2
    if [ -f "dist/vendor/$file" ] && sha_ok "dist/vendor/$file" "$sha"; then
        return 0
    fi
    mkdir -p dist/vendor
    for url in "$@"; do
        echo "==> fetching $url"
        if curl -fL --retry 3 -o "dist/vendor/$file.part" "$url"; then
            mv "dist/vendor/$file.part" "dist/vendor/$file"
            break
        fi
    done
    sha_ok "dist/vendor/$file" "$sha" || {
        echo "FAIL: dist/vendor/$file does not match its pinned sha256" >&2
        echo "      (a new upstream build needs a deliberate URL+hash bump here)" >&2
        rm -f "dist/vendor/$file"
        exit 1
    }
}

provenance() { # provenance <dir> <url> <sha>
    cat > "$1/SOURCE.txt" <<EOF
This ffmpeg build is bundled unmodified. Tangent invokes it as a separate
process; it is GPL-licensed (see the licence text beside this file) and its
source code is available from https://ffmpeg.org/ and from the build's page:

  $2
  sha256 of the downloaded archive: $3
EOF
}

case "$PLATFORM" in
linux)
    # Word-splitting the URL list is deliberate; the URLs contain no spaces.
    # shellcheck disable=SC2086
    fetch "$LINUX_FILE" "$LINUX_SHA" $LINUX_URLS
    OUT="dist/vendor/linux"
    rm -rf "$OUT"; mkdir -p "$OUT/ffmpeg-licenses"
    # `bin/ffmpeg` and a top-level `LICENSE.txt`, the same layout as the
    # Windows zip — two components deep for the binary, one for the licence,
    # so they are extracted separately.
    tar -xJf "dist/vendor/$LINUX_FILE" -C "$OUT" --strip-components=2 \
        "${LINUX_FILE%.tar.xz}/bin/ffmpeg"
    tar -xJf "dist/vendor/$LINUX_FILE" -C "$OUT" --strip-components=1 \
        "${LINUX_FILE%.tar.xz}/LICENSE.txt"
    mv "$OUT/ffmpeg" "$OUT/tangent-ffmpeg"
    chmod 755 "$OUT/tangent-ffmpeg"
    mv "$OUT/LICENSE.txt" "$OUT/ffmpeg-licenses/LICENSE.txt"
    provenance "$OUT/ffmpeg-licenses" "${LINUX_URLS%%$'\n'*}" "$LINUX_SHA"
    echo "==> $OUT/tangent-ffmpeg"
    ;;
windows)
    # shellcheck disable=SC2086
    fetch "$WIN_FILE" "$WIN_SHA" $WIN_URLS
    OUT="dist/vendor/windows"
    rm -rf "$OUT"; mkdir -p "$OUT/ffmpeg-licenses"
    unzip -j -q "dist/vendor/$WIN_FILE" \
        "${WIN_FILE%.zip}/bin/ffmpeg.exe" "${WIN_FILE%.zip}/LICENSE.txt" -d "$OUT"
    mv "$OUT/ffmpeg.exe" "$OUT/tangent-ffmpeg.exe"
    mv "$OUT/LICENSE.txt" "$OUT/ffmpeg-licenses/LICENSE.txt"
    provenance "$OUT/ffmpeg-licenses" "$WIN_URLS" "$WIN_SHA"
    echo "==> $OUT/tangent-ffmpeg.exe"
    ;;
esac
