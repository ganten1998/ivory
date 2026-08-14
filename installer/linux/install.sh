#!/bin/sh
# Install Tangent — the application, the VST3 plugin, or both.
#
#   ./install.sh                 both, into your home directory (no root)
#   ./install.sh --app           the application only
#   ./install.sh --vst3          the plugin only
#   ./install.sh --system        into /usr/local and /usr/lib/vst3 (needs root)
#   ./install.sh --prefix DIR    somewhere else entirely
#   ./install.sh --uninstall     take it all back out
#   ./install.sh --dry-run       print what would happen and do nothing
#
# WHY A SHELL SCRIPT AND NOT A .deb
#
# A .deb reaches Debian and Ubuntu. The same argument then repeats for rpm, for
# Arch, and for Void — which is the machine this build is made on. This runs on
# all of them, needs no root for the default, and can be read before it is run,
# which is what a Linux user is going to do anyway.
#
# POSIX sh, not bash: /bin/sh is dash on Debian and Ubuntu, and a bashism here
# would fail on the two distributions most people are running.
set -eu

SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
NAME="Tangent"

WANT_APP=1
WANT_VST3=1
DRY=0
UNINSTALL=0
MODE=user
PREFIX=""

while [ $# -gt 0 ]; do
    case "$1" in
        --app)       WANT_APP=1; WANT_VST3=0 ;;
        --vst3)      WANT_APP=0; WANT_VST3=1 ;;
        --both)      WANT_APP=1; WANT_VST3=1 ;;
        --system)    MODE=system ;;
        --user)      MODE=user ;;
        --prefix)    shift; [ $# -gt 0 ] || { echo "--prefix needs a directory" >&2; exit 2; }
                     PREFIX="$1"; MODE=prefix ;;
        --uninstall) UNINSTALL=1 ;;
        --dry-run|-n) DRY=1 ;;
        -h|--help)   sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1  (try --help)" >&2; exit 2 ;;
    esac
    shift
done

# Where things go.
#
#   ~/.vst3 and /usr/lib/vst3 are the VST3 specification's own Linux search
#   paths. A plugin anywhere else is one the host has to be told about, which
#   is the same as one that does not work.
case "$MODE" in
    user)
        BIN_DIR="$HOME/.local/bin"
        VST3_DIR="$HOME/.vst3"
        DESKTOP_DIR="$HOME/.local/share/applications"
        ICON_DIR="$HOME/.local/share/icons/hicolor/128x128/apps"
        ;;
    system)
        BIN_DIR="/usr/local/bin"
        VST3_DIR="/usr/lib/vst3"
        DESKTOP_DIR="/usr/share/applications"
        ICON_DIR="/usr/share/icons/hicolor/128x128/apps"
        ;;
    prefix)
        BIN_DIR="$PREFIX/bin"
        VST3_DIR="$PREFIX/lib/vst3"
        DESKTOP_DIR="$PREFIX/share/applications"
        ICON_DIR="$PREFIX/share/icons/hicolor/128x128/apps"
        ;;
esac

run() {
    if [ "$DRY" = "1" ]; then
        printf '  would: %s\n' "$*"
    else
        "$@"
    fi
}

# Refuse early and clearly, rather than failing on the first copy.
if [ "$DRY" = "0" ] && [ "$MODE" != "user" ]; then
    if [ ! -w "$(dirname "$BIN_DIR")" ] && [ "$(id -u)" != "0" ]; then
        echo "installing to $BIN_DIR needs root. Re-run with sudo, or drop --system" >&2
        echo "to install into your home directory instead (no root needed)." >&2
        exit 1
    fi
fi

if [ "$UNINSTALL" = "1" ]; then
    echo "Removing $NAME"
    run rm -f  "$BIN_DIR/tangent"
    run rm -rf "$VST3_DIR/$NAME.vst3"
    run rm -f  "$DESKTOP_DIR/tangent.desktop"
    run rm -f  "$ICON_DIR/tangent.png"
    echo
    echo "Removed. Your settings are still in ~/.config/ivory — taught chord"
    echo "names and a supporter key live there, so uninstalling does not throw"
    echo "them away. Delete that directory yourself if you want them gone."
    exit 0
fi

[ "$DRY" = "1" ] && echo "(dry run — nothing will be written)"
echo "Installing $NAME"

if [ "$WANT_APP" = "1" ]; then
    if [ ! -f "$SELF_DIR/tangent" ]; then
        echo "no 'tangent' binary next to this script" >&2
        exit 1
    fi
    echo "  application -> $BIN_DIR/tangent"
    run mkdir -p "$BIN_DIR"
    run cp "$SELF_DIR/tangent" "$BIN_DIR/tangent"
    run chmod 755 "$BIN_DIR/tangent"

    # The menu entry, when the tarball carries one.
    if [ -f "$SELF_DIR/tangent.desktop" ]; then
        echo "  menu entry  -> $DESKTOP_DIR/tangent.desktop"
        run mkdir -p "$DESKTOP_DIR"
        run cp "$SELF_DIR/tangent.desktop" "$DESKTOP_DIR/tangent.desktop"
    fi
    if [ -f "$SELF_DIR/tangent.png" ]; then
        run mkdir -p "$ICON_DIR"
        run cp "$SELF_DIR/tangent.png" "$ICON_DIR/tangent.png"
    fi

    # Say so rather than leaving them to find out by typing `tangent`.
    case ":${PATH}:" in
        *":$BIN_DIR:"*) ;;
        *) echo "  NOTE: $BIN_DIR is not on your PATH."
           echo "        Add it, or run $BIN_DIR/tangent directly." ;;
    esac
fi

if [ "$WANT_VST3" = "1" ]; then
    if [ ! -d "$SELF_DIR/$NAME.vst3" ]; then
        echo "no '$NAME.vst3' next to this script" >&2
        exit 1
    fi
    echo "  plugin      -> $VST3_DIR/$NAME.vst3"
    run mkdir -p "$VST3_DIR"
    # Remove the old bundle first. Copying over it leaves behind any file that
    # used to be in the bundle and is not any more, which is how a host ends up
    # loading a mix of two versions.
    run rm -rf "$VST3_DIR/$NAME.vst3"
    run cp -R "$SELF_DIR/$NAME.vst3" "$VST3_DIR/$NAME.vst3"
    run find "$VST3_DIR/$NAME.vst3" -name '*.so' -exec chmod 755 {} +
fi

echo
if [ "$DRY" = "1" ]; then
    echo "Nothing was written. Drop --dry-run to install."
    exit 0
fi
[ "$WANT_APP" = "1" ]  && echo "Run it with:  tangent"
[ "$WANT_VST3" = "1" ] && echo "Your DAW will find the plugin the next time it scans for plugins."
echo "Hold H in either one to see every keyboard shortcut."
