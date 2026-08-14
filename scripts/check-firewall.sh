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
for crate in ivory-ui ivory-core; do
  for dep in eframe midir rfd fd-lock winit; do
    # Match the crate NAME field of a cargo-tree line, without trying to
    # describe cargo's box-drawing prefix in a regex.
    if cargo tree -p "$crate" --edges normal --prefix none 2>/dev/null \
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

# ── Direction of dependency ──────────────────────────────────────────────────
# ivory -> ivory-ui -> ivory-core, never the reverse.
if cargo tree -p ivory-ui --edges normal --prefix none 2>/dev/null \
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
