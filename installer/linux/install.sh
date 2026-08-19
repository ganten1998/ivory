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

# Written out rather than sed'd out of the header comment above. That trick
# needs a line range, the range was 2..26, and the comment grew — so --help
# printed the usage AND the first half of an essay about .deb packages.
usage() {
    cat <<'USAGE'
Install Tangent — the application, the VST3 plugin, or both.

  ./install.sh                 both, into your home directory (no root)
  ./install.sh --app           the application only
  ./install.sh --vst3          the plugin only
  ./install.sh --system        into /usr/local and /usr/lib/vst3 (needs root)
  ./install.sh --prefix DIR    somewhere else entirely
  ./install.sh --uninstall     take it all back out
  ./install.sh --dry-run       print what would happen and do nothing

The default needs no root. It puts the application in ~/.local/bin and the
plugin in ~/.vst3, which is one of the directories the VST3 specification
tells hosts to scan.
USAGE
}

WANT_APP=1
WANT_VST3=1
# Whether the plugin was ASKED for, as opposed to being on because the default
# is "both". The two artifacts are not the same shape: the full release carries
# `Tangent.vst3` beside this script, and the plain cross-built tarball is the
# application on its own. Treating "no plugin in this archive" as a failure
# made the default invocation exit 1 on the app-only tarball — which is the
# tarball most people have.
VST3_EXPLICIT=0
DRY=0
UNINSTALL=0
MODE=user
PREFIX=""

while [ $# -gt 0 ]; do
    case "$1" in
        --app)       WANT_APP=1; WANT_VST3=0 ;;
        --vst3)      WANT_APP=0; WANT_VST3=1; VST3_EXPLICIT=1 ;;
        --both)      WANT_APP=1; WANT_VST3=1; VST3_EXPLICIT=1 ;;
        --system)    MODE=system ;;
        --user)      MODE=user ;;
        --prefix)    shift; [ $# -gt 0 ] || { echo "--prefix needs a directory" >&2; exit 2; }
                     PREFIX="$1"; MODE=prefix ;;
        --uninstall) UNINSTALL=1 ;;
        --dry-run|-n) DRY=1 ;;
        -h|--help)   usage; exit 0 ;;
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

# The nearest ancestor of $1 that exists.
#
# `--prefix /tmp/somewhere-new` is a directory that is SUPPOSED not to exist
# yet, so testing its parent for writability tested a directory that was also
# absent — `-w` said no and the script refused to install anywhere it had been
# asked to create. Walking up to something real is the question actually
# worth asking: may I create this?
nearest_existing() {
    d="$1"
    while [ ! -e "$d" ] && [ "$d" != "/" ] && [ "$d" != "." ]; do
        d="$(dirname "$d")"
    done
    printf '%s' "$d"
}

# Refuse early and clearly, rather than failing on the first copy.
if [ "$DRY" = "0" ] && [ "$MODE" != "user" ]; then
    if [ ! -w "$(nearest_existing "$BIN_DIR")" ] && [ "$(id -u)" != "0" ]; then
        echo "cannot write to $BIN_DIR — the nearest existing directory" >&2
        echo "($(nearest_existing "$BIN_DIR")) is not writable by you." >&2
        if [ "$MODE" = "system" ]; then
            echo "Re-run with sudo, or drop --system to install into your home" >&2
            echo "directory instead (no root needed)." >&2
        else
            echo "Re-run with sudo, or choose a --prefix you own." >&2
        fi
        exit 1
    fi
fi

if [ "$UNINSTALL" = "1" ]; then
    echo "Removing $NAME"
    run rm -f  "$BIN_DIR/tangent"
    run rm -f  "$BIN_DIR/tangent-ffmpeg"
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

    # The bundled video encoder. Prefixed `tangent-`, never plain `ffmpeg`:
    # ~/.local/bin is on most users' PATH and an unprefixed copy there would
    # shadow the system ffmpeg for every shell they own. Tangent looks for
    # this name beside its own binary, so video works with nothing installed.
    if [ -f "$SELF_DIR/tangent-ffmpeg" ]; then
        echo "  encoder     -> $BIN_DIR/tangent-ffmpeg"
        run cp "$SELF_DIR/tangent-ffmpeg" "$BIN_DIR/tangent-ffmpeg"
        run chmod 755 "$BIN_DIR/tangent-ffmpeg"
    fi

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
    ON_PATH=1
    case ":${PATH}:" in
        *":$BIN_DIR:"*) ;;
        *) ON_PATH=0
           echo "  NOTE: $BIN_DIR is not on your PATH."
           echo "        Add it, or use the full path below." ;;
    esac
fi

if [ "$WANT_VST3" = "1" ] && [ ! -d "$SELF_DIR/$NAME.vst3" ]; then
    if [ "$VST3_EXPLICIT" = "1" ]; then
        echo "no '$NAME.vst3' next to this script" >&2
        exit 1
    fi
    # Asked for by the default rather than by name, and it is not here: say so
    # once and carry on with the application. This archive is the app on its
    # own, and refusing to install it because a thing it never contained is
    # absent helps nobody.
    echo "  plugin      -> not in this archive (application only)"
    WANT_VST3=0
fi

if [ "$WANT_VST3" = "1" ]; then
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
# The full path when `tangent` would not resolve. Printing "Run it with:
# tangent" four lines under "that directory is not on your PATH" is telling
# someone to do the thing you just told them would not work.
if [ "$WANT_APP" = "1" ]; then
    if [ "${ON_PATH:-1}" = "1" ]; then
        echo "Run it with:  tangent"
    else
        echo "Run it with:  $BIN_DIR/tangent"
    fi

    # Filming a take composites on the GPU through Vulkan. Everything else in
    # Tangent works without it, so a missing driver is a note with the exact
    # command, not an error. ICD manifests are how the loader finds drivers;
    # no manifest anywhere means takes will record audio and MIDI but no video.
    have_vulkan=0
    for d in /usr/share/vulkan/icd.d /usr/local/share/vulkan/icd.d /etc/vulkan/icd.d; do
        if [ -d "$d" ] && [ -n "$(ls "$d"/*.json 2>/dev/null)" ]; then
            have_vulkan=1
            break
        fi
    done
    if [ "$have_vulkan" = "0" ]; then
        echo
        echo "  NOTE: no Vulkan driver was found, so takes will record audio and"
        echo "        MIDI but not video. Any Vulkan driver fixes it - mesa's"
        echo "        lavapipe works on every GPU:"
        . /etc/os-release 2>/dev/null || ID=""
        case "$ID" in
            debian|ubuntu|linuxmint|pop)
                     echo "          sudo apt install mesa-vulkan-drivers" ;;
            fedora)  echo "          sudo dnf install mesa-vulkan-drivers" ;;
            arch|manjaro|endeavouros)
                     echo "          sudo pacman -S vulkan-swrast" ;;
            void)    echo "          sudo xbps-install mesa-vulkan-lavapipe" ;;
            opensuse*|suse*)
                     echo "          sudo zypper install libvulkan1 Mesa-vulkan-device-select" ;;
            *)       echo "          (install your distribution's mesa Vulkan package)" ;;
        esac
    fi
fi
[ "$WANT_VST3" = "1" ] && echo "Your DAW will find the plugin the next time it scans for plugins."
echo "Hold H in either one to see every keyboard shortcut."
