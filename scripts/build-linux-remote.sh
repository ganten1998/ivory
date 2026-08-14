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

# Extra ssh options, e.g. SSH_OPTS='-o IdentityAgent=none -p 2222'.
# `IdentityAgent=none` is worth knowing about: in some shells the agent hangs
# the connection instead of failing it, so a wrong key looks like a dead host
# rather than a refused login. Setting it makes ssh use key FILES only.
SSH_OPTS="${SSH_OPTS:-}"
# shellcheck disable=SC2086
ssh_() { ssh $SSH_OPTS "$@"; }

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
if ! ssh_ -o BatchMode=yes -o ConnectTimeout=10 "$HOST" true 2>/dev/null; then
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

REMOTE_OS="$(ssh_ "$HOST" 'uname -s' 2>/dev/null || echo unknown)"
[ "$REMOTE_OS" = "Linux" ] || { echo "'$HOST' reports '$REMOTE_OS', not Linux" >&2; exit 1; }
REMOTE_ARCH="$(ssh_ "$HOST" 'uname -m')"
echo "    remote: Linux $REMOTE_ARCH"

# ── Build dependencies, on request ───────────────────────────────────────────
# Named per distro rather than guessed: the ALSA and X11/Wayland/GL -dev
# packages are the ones that matter, and a missing one fails deep inside a
# build script rather than up front.
if [ "${DEPS:-0}" = "1" ]; then
  echo "==> Installing build dependencies"
  ssh_ -t "$HOST" 'set -e
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
if ! ssh_ "$HOST" 'command -v cargo >/dev/null'; then
  cat >&2 <<EOF
no 'cargo' on $HOST.

  Either:  DEPS=1 $0 $HOST          (installs the distro's rust)
  Or:      ssh $HOST "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"

EOF
  exit 1
fi

# ── Copy the tree ────────────────────────────────────────────────────────────
echo "==> Syncing source"
ssh_ "$HOST" "mkdir -p $REMOTE_DIR"
# No trailing slashes on target/dist. `target/` matches a DIRECTORY only, and
# `target` here is a SYMLINK out of Dropbox (see docs: Dropbox breaks cargo
# builds, so the real target lives in ~/Library/Caches). rsync happily copied
# the symlink itself, which arrived on Linux pointing at a macOS path that does
# not exist — and cargo failed with "Not a directory" rather than anything that
# named the cause.
rsync -az --delete ${SSH_OPTS:+-e "ssh $SSH_OPTS"} \
  --exclude 'target' --exclude 'dist' --exclude '.git' \
  --exclude '.DS_Store' --exclude '*.dmg' --exclude '*.zip' \
  ./ "$HOST:$REMOTE_DIR/"

# And clear any bad `target` a previous run left behind, so an existing build
# directory does not stay poisoned.
ssh_ "$HOST" "[ -L $REMOTE_DIR/target ] && rm -f $REMOTE_DIR/target || true"

# ── Build ────────────────────────────────────────────────────────────────────
echo "==> Building (first run compiles everything; later runs reuse the cache)"
ssh_ "$HOST" "cd $REMOTE_DIR && chmod +x scripts/*.sh && scripts/build-linux-native.sh"

# ── Bring it home ────────────────────────────────────────────────────────────
echo "==> Fetching the tarball"
mkdir -p dist
rsync -az ${SSH_OPTS:+-e "ssh $SSH_OPTS"} "$HOST:$REMOTE_DIR/dist/tangent-${VERSION}-linux-*.tar.gz" dist/

TARBALL="dist/tangent-${VERSION}-linux-${REMOTE_ARCH}.tar.gz"
[ -f "$TARBALL" ] || { echo "no $TARBALL came back" >&2; exit 1; }

# Prove it carries a binary. build-cross.sh once shipped 85 KB tarballs of
# nothing but fonts and licences for a week; "the file exists" means nothing.
# `grep -c`, not `grep -q`, and the reason is not style. With `set -o pipefail`
# a `grep -q` that MATCHES exits immediately, closes the pipe, and `tar` dies of
# SIGPIPE — so the pipeline reports failure exactly when the check succeeds, and
# `!` turns that into the error branch. The first real Linux build was rejected
# by this line while the tarball it was reading was perfectly good. `grep -c`
# consumes all its input, so tar always finishes.
if [ "$(tar -tzf "$TARBALL" | grep -c '/tangent$')" -eq 0 ]; then
  echo "FAIL: $TARBALL contains no 'tangent' binary" >&2
  tar -tzf "$TARBALL" | sed 's/^/    /' >&2
  exit 1
fi

echo "==> OK  $TARBALL"
ls -lh "$TARBALL" | sed 's/^/    /'
echo "    $(tar -tzf "$TARBALL" | wc -l | tr -d ' ') entries, binary present"
echo
echo "Next: scripts/release.sh --sums-only, then scripts/publish-github.sh"
