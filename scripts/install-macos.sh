#!/usr/bin/env bash
#
# Install the built Tangent.app into /Applications.
#
# ONE short command, and that is the whole point of it:
#
#     cd ~/Dropbox/Projects/Apps/ivory && ./scripts/install-macos.sh
#
# The three-line copy-paste this replaces was
#
#     sudo rm -rf /Applications/Tangent.app
#     sudo ditto dist/Tangent.app /Applications/Tangent.app
#     sudo chown -R root:wheel /Applications/Tangent.app
#
# and a terminal that wraps long pastes eventually joined the first two lines,
# so `rm -rf` ran with `dist/Tangent.app` among its arguments and deleted the
# build as well as the installation. A destructive command must never share a
# line with anything, and the reliable way to guarantee that is not to hand
# anybody a destructive command at all.
#
# No `sudo`. /Applications is drwxrwxr-x root:admin, so an admin user has
# always been able to write to it; the sudo was cargo-culted and it is what
# made a mistyped paste able to reach outside /Applications.

set -euo pipefail

cd "$(dirname "$0")/.."

APP="dist/Tangent.app"
DEST="/Applications/Tangent.app"

# **The source is checked BEFORE the destination is touched.** This is the
# whole safety property: the failure that prompted this script left the machine
# with no built app AND no installed app, because the removal happened first.
if [ ! -d "$APP" ]; then
  echo "==> $APP is missing; looking for a packaged build to restore it from"
  ZIP=$(ls -t dist/Tangent-*-macos-*.zip 2>/dev/null | head -1 || true)
  if [ -z "$ZIP" ]; then
    echo "error: no $APP and no dist/Tangent-*-macos-*.zip to restore from." >&2
    echo "       Run ./scripts/build-macos.sh first." >&2
    exit 1
  fi
  echo "    restoring from $ZIP"
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT
  ditto -xk "$ZIP" "$TMP"
  INNER=$(find "$TMP" -maxdepth 2 -name Tangent.app -type d | head -1)
  if [ -z "$INNER" ]; then
    echo "error: $ZIP does not contain a Tangent.app." >&2
    exit 1
  fi
  ditto "$INNER" "$APP"
fi

# Signed, and readable. A bundle that fails this is one that will be killed on
# launch by AMFI with nothing on screen to say why, which is a far worse
# afternoon than a failed install.
if ! codesign --verify --strict "$APP" >/dev/null 2>&1; then
  echo "error: $APP is not validly signed. Rebuild rather than installing it." >&2
  exit 1
fi

VERSION=$(defaults read "$PWD/$APP/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || echo "?")

# Replace, rather than copy over the top. `ditto` merges, so a file that
# existed in the old version and not in the new one would survive into the
# installed bundle and break its signature.
if [ -d "$DEST" ]; then
  OLD=$(defaults read "$DEST/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || echo "?")
  echo "==> Replacing $OLD with $VERSION"
  rm -rf "$DEST"
else
  echo "==> Installing $VERSION"
fi

ditto "$APP" "$DEST"

# Notarization is a fact about the build, not about this copy, and it is worth
# saying out loud: a signed-but-unnotarized bundle runs perfectly well here
# (nothing downloaded it, so it carries no quarantine attribute) and is refused
# on every machine that DID download it.
if xcrun stapler validate "$DEST" >/dev/null 2>&1; then
  echo "    notarized and stapled"
else
  echo "    signed but NOT notarized — fine on this machine, refused on any other"
fi

echo "==> $DEST is $(defaults read "$DEST/Contents/Info.plist" CFBundleShortVersionString)"
