#!/usr/bin/env bash
# Build the Linux release on a remote Linux host, from this Mac, in one command.
#
#   scripts/build-linux-remote.sh void                 # ssh host or user@host
#   scripts/build-linux-remote.sh void ~/build/tangent # explicit remote dir
#   DEPS=1 scripts/build-linux-remote.sh void          # install build deps first
#
# WHY THIS EXISTS. Linux cannot be cross-built from macOS: midir links ALSA and
# alsa-sys needs a real sysroot to find libasound (docs/RELEASE.md, "Cross-build
# blocker"). The sanctioned path has always been "run build-linux-native.sh on a
# Linux host", which in practice meant a dozen manual steps and a scp, so it
# never got run and Linux never shipped. This is those steps.
#
# It copies the working tree (not a clone), so what builds is exactly what you
# have here — including uncommitted changes. `target/` and `dist/` are excluded,
# so the remote keeps its own build cache and the second run is fast.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

HOST="${1:-}"
if [ -z "$HOST" ]; then
  cat >&2 <<'USAGE'
usage: scripts/build-linux-remote.sh <ssh-host> [remote-dir]

  <ssh-host>   anything ssh understands: a Host from ~/.ssh/config, user@ip,
               or a hostname. Try `ssh <host> true` first if unsure.
  [remote-dir] where to build, default ~/tangent-build

  DEPS=1       install the Void/Debian build dependencies before building
USAGE
  exit 2
fi
REMOTE_DIR="${2:-\$HOME/tangent-build}"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
echo "==> Tangent $VERSION -> $HOST:$REMOTE_DIR"

# ── Reachability, before anything slow ───────────────────────────────────────
if ! ssh -o BatchMode=yes -o ConnectTimeout=10 "$HOST" true 2>/dev/null; then
  cat >&2 <<EOF
cannot ssh to '$HOST' without a password.

  Check it is up and keyed:      ssh $HOST true
  If it asks for a password:     ssh-copy-id $HOST
  If the name is unknown, add it to ~/.ssh/config:

      Host void
        HostName 192.168.1.x
        User $(whoami)

EOF
  exit 1
fi

REMOTE_OS="$(ssh "$HOST" 'uname -s' 2>/dev/null || echo unknown)"
[ "$REMOTE_OS" = "Linux" ] || { echo "'$HOST' reports '$REMOTE_OS', not Linux" >&2; exit 1; }
REMOTE_ARCH="$(ssh "$HOST" 'uname -m')"
echo "    remote: Linux $REMOTE_ARCH"

# ── Build dependencies, on request ───────────────────────────────────────────
# Named per distro rather than guessed: the ALSA and X11/Wayland/GL -dev
# packages are the ones that matter, and a missing one fails deep inside a
# build script rather than up front.
if [ "${DEPS:-0}" = "1" ]; then
  echo "==> Installing build dependencies"
  ssh -t "$HOST" 'set -e
    if command -v xbps-install >/dev/null; then
      sudo xbps-install -Sy base-devel rust cargo pkg-config \
        alsa-lib-devel libX11-devel libxcb-devel libxkbcommon-devel \
        wayland-devel MesaLib-devel libXcursor-devel libXrandr-devel libXi-devel
    elif command -v apt >/dev/null; then
      sudo apt update && sudo apt install -y build-essential pkg-config \
        libasound2-dev libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
        libgl1-mesa-dev libxcursor-dev libxrandr-dev libxi-dev
    elif command -v dnf >/dev/null; then
      sudo dnf install -y gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel \
        libX11-devel libxcb-devel libxkbcommon-devel wayland-devel \
        mesa-libGL-devel libXcursor-devel libXrandr-devel libXi-devel
    else
      echo "unknown package manager — see the header of build-linux-native.sh" >&2
      exit 1
    fi'
fi

# Rust has to exist over there, and the message should say how to get it.
if ! ssh "$HOST" 'command -v cargo >/dev/null'; then
  cat >&2 <<EOF
no 'cargo' on $HOST.

  Either:  DEPS=1 $0 $HOST          (installs the distro's rust)
  Or:      ssh $HOST "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"

EOF
  exit 1
fi

# ── Copy the tree ────────────────────────────────────────────────────────────
echo "==> Syncing source"
ssh "$HOST" "mkdir -p $REMOTE_DIR"
rsync -az --delete \
  --exclude 'target/' --exclude 'dist/' --exclude '.git/' \
  --exclude '.DS_Store' --exclude '*.dmg' --exclude '*.zip' \
  ./ "$HOST:$REMOTE_DIR/"

# ── Build ────────────────────────────────────────────────────────────────────
echo "==> Building (first run compiles everything; later runs reuse the cache)"
ssh "$HOST" "cd $REMOTE_DIR && chmod +x scripts/*.sh && scripts/build-linux-native.sh"

# ── Bring it home ────────────────────────────────────────────────────────────
echo "==> Fetching the tarball"
mkdir -p dist
rsync -az "$HOST:$REMOTE_DIR/dist/tangent-${VERSION}-linux-*.tar.gz" dist/

TARBALL="dist/tangent-${VERSION}-linux-${REMOTE_ARCH}.tar.gz"
[ -f "$TARBALL" ] || { echo "no $TARBALL came back" >&2; exit 1; }

# Prove it carries a binary. build-cross.sh once shipped 85 KB tarballs of
# nothing but fonts and licences for a week; "the file exists" means nothing.
if ! tar -tzf "$TARBALL" | grep -q '/tangent$'; then
  echo "FAIL: $TARBALL contains no 'tangent' binary" >&2
  tar -tzf "$TARBALL" | sed 's/^/    /' >&2
  exit 1
fi

echo "==> OK  $TARBALL"
ls -lh "$TARBALL" | sed 's/^/    /'
echo "    $(tar -tzf "$TARBALL" | wc -l | tr -d ' ') entries, binary present"
echo
echo "Next: scripts/release.sh --sums-only, then scripts/publish-github.sh"
