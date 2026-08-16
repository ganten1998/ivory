#!/usr/bin/env bash
# The shared crates must not be able to reach the host.
#
# `ivory-ui` and `ivory-core` are linked by BOTH the desktop binary and the
# VST3 plugin. A plugin has no window to own, no MIDI device to open, no native
# file dialog to raise, and above all no business calling `process::exit` —
# which in a plugin means killing the user's DAW, mid-session, with their work
# open.
#
# Leaving those crates out of the dependency list makes all of it a COMPILE
# ERROR rather than a code-review question. This script asserts that the
# arrangement is still true, because the way it stops being true is somebody
# adding one convenient dependency for one good reason.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
say() { printf '  %-46s %s\n' "$1" "$2"; }

# ── Forbidden dependencies ───────────────────────────────────────────────────
#
# `--target all` is LOAD-BEARING and was missing until 2026-08-15. `cargo tree`
# filters to the HOST platform by default, so a dependency declared under
# `[target.'cfg(windows)'.dependencies]` is invisible to this script on the
# machine it is actually run on. Proof, in this repo: `ivory/Cargo.toml`
# declares `windows-sys` exactly that way, and the old idiom found nothing here
# while `--target all` finds it. A Windows-only camera or audio crate added to
# `ivory-ui` would therefore have passed this check forever, on the one platform
# nobody can test locally.
for crate in ivory-ui ivory-core; do
  # eframe/midir/rfd/fd-lock/winit: the original five — a window, a MIDI device,
  # a native dialog, a single-instance lock.
  # cpal/nokhwa/objc2/windows/ivory-record: the recorder's half. A camera, an
  # audio device and an encoder are exactly the same category of thing, and the
  # reason to name them here is that the list is literal crate names rather than
  # a category, so nothing else would stop them.
  # vst3/clack-host/ivory-host: the plugin host, for the same reason.
  for dep in eframe midir rfd fd-lock winit \
             cpal nokhwa objc2 windows ivory-record \
             vst3 clack-host ivory-host; do
    # Match the crate NAME field of a cargo-tree line, without trying to
    # describe cargo's box-drawing prefix in a regex.
    if cargo tree -p "$crate" --edges normal --prefix none --target all 2>/dev/null \
         | awk '{print $1}' | grep -qx "$dep"; then
      say "$crate must not depend on $dep" "FAIL"
      fail=1
    else
      say "$crate has no $dep" "ok"
    fi
  done
done

# ── Forbidden calls ──────────────────────────────────────────────────────────
# `process::exit` unwinds nothing and runs no destructor. In the standalone it
# is a legitimate way to refuse a second instance; in a plugin it is a crash
# report against someone else's software.
for crate in ivory-ui ivory-core; do
  hits=$(grep -rnE '\bprocess::exit\b|\bstd::process::abort\b' "$crate/src" 2>/dev/null || true)
  if [ -n "$hits" ]; then
    say "$crate must not call process::exit" "FAIL"
    echo "$hits" | sed 's/^/      /'
    fail=1
  else
    say "$crate never exits the process" "ok"
  fi
done

# ── The plugin has no recorder ───────────────────────────────────────────────
# A VST3 has no business capturing a camera, opening an audio device, writing a
# take directory, or hosting a second plugin inside itself. The DAW already owns
# every one of those, and does them better.
#
# This is structurally true rather than merely intended: `ivory-record` and
# `ivory-host` are dependencies of the `ivory` BINARY, and the plugin depends on
# `ivory-ui` and `ivory-core` only, so there is no path. Asserted anyway,
# because "there is no path" is a property of today's manifests and somebody
# adding one convenient dependency is exactly how it stops being true.
#
# The plugin is its own workspace with its own lock file, so this has to run
# from inside plugin/ — a `-p tangent-vst3` from the root cannot resolve it, and
# that inability IS the GPL quarantine working.
if [ -f plugin/Cargo.toml ]; then
  plugin_tree="$( (cd plugin && cargo tree --edges normal --prefix none --target all 2>/dev/null) | awk '{print $1}' )"
  if [ -z "$plugin_tree" ]; then
    say "plugin tree unavailable (offline?), skipped" "--"
  else
    for dep in ivory-record ivory-host cpal midly rtrb vst3; do
      if printf '%s\n' "$plugin_tree" | grep -qx "$dep"; then
        say "the plugin must not depend on $dep" "FAIL"
        fail=1
      else
        say "the plugin has no $dep" "ok"
      fi
    done
  fi
fi

# ── Direction of dependency ──────────────────────────────────────────────────
# ivory -> ivory-ui -> ivory-core, never the reverse.
if cargo tree -p ivory-ui --edges normal --prefix none --target all 2>/dev/null \
     | awk '{print $1}' | grep -qx 'ivory'; then
  say "ivory-ui must not depend on the binary" "FAIL"; fail=1
else
  say "ivory-ui does not depend on the binary" "ok"
fi

if [ "$fail" = 0 ]; then
  echo "  firewall intact"
else
  echo "  FIREWALL BREACHED — see docs/PLUGIN-PLAN.md" >&2
  exit 1
fi
