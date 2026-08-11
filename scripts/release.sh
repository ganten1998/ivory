#!/usr/bin/env bash
# Orchestrate one Ivory release build on THIS host: verify the version, purge
# artifacts left over from other versions, run whichever build scripts this OS
# can run, sanity-check the results, and write a version-scoped SHA256SUMS.
#
# Usage:
#   scripts/release.sh                  # build everything this host can
#   ARCH=universal scripts/release.sh   # macOS: universal binary, not host arch
#   scripts/release.sh --sums-only      # re-check + re-emit SHA256SUMS only
#   SKIP_WINDOWS=1 scripts/release.sh   # macOS: skip build-cross.sh
#   KEEP_STALE=1 scripts/release.sh     # leave other versions' artifacts in dist/
#
#   macOS host -> scripts/build-macos.sh + scripts/build-cross.sh (Windows zip)
#   Linux host -> scripts/build-linux-native.sh
#
# Run it on each OS (and each Linux arch), copy the artifacts into one dist/,
# then `scripts/release.sh --sums-only` there so SHA256SUMS covers everything.
# It builds and checks only — tagging, signing for distribution and uploading
# (docs/RELEASE.md steps 8-10) stay deliberately manual.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SUMS_ONLY=0
case "${1:-}" in
  "")          ;;
  --sums-only) SUMS_ONLY=1 ;;
  *) echo "unknown argument: $1 (expected --sums-only)" >&2; exit 2 ;;
esac

FAIL=0
note() { echo "!! $*"; FAIL=1; }
sha256() {  # shasum is Perl's and is not on every minimal Linux install
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$@"
  else sha256sum "$@"; fi
}

# ── Verify the version ───────────────────────────────────────────────────────
VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
  echo "could not read a sane version from Cargo.toml (got '$VERSION')" >&2
  exit 2
fi

echo "==> Ivory $VERSION  host=$(uname -s)/$(uname -m)"
case "$VERSION" in
  *-*|*+*) echo "    pre-release: artifacts say $VERSION, Info.plist says ${VERSION%%[-+]*}" ;;
esac

# Cargo.lock records the workspace version too; if it disagrees the lockfile was
# never refreshed after the bump and a --locked build would fail.
LOCK_VERSION="$(awk '/^name = "ivory"$/{getline; gsub(/[",]/,""); print $3; exit}' Cargo.lock)"
[ "$LOCK_VERSION" = "$VERSION" ] || note \
  "Cargo.lock says ivory $LOCK_VERSION, Cargo.toml says $VERSION — run 'cargo check' and commit the lockfile"
[ -z "$(git status --porcelain 2>/dev/null)" ] \
  || echo "    NOTE: working tree is dirty — these artifacts match no commit"
git rev-parse -q --verify "refs/tags/v${VERSION}" >/dev/null 2>&1 \
  && echo "    NOTE: tag v${VERSION} exists — you are rebuilding a shipped version"

# ── Purge artifacts belonging to other versions ──────────────────────────────
# Left in place they get swept up by any `Ivory-*` / `ivory-*` glob at checksum
# or upload time. Only the four known artifact name shapes are ever touched.
mkdir -p dist
if [ "${KEEP_STALE:-0}" != "1" ]; then
  shopt -s nullglob
  STALE=()
  for f in dist/Ivory-*-macos-*.zip dist/Ivory-*-macos-*.dmg \
           dist/ivory-*-linux-*.tar.gz dist/ivory-*-windows-*.zip; do
    case "$f" in *"-${VERSION}-"*) continue ;; esac
    STALE+=("$f")
  done
  shopt -u nullglob
  if [ "${#STALE[@]}" -gt 0 ]; then
    echo "==> Purging ${#STALE[@]} artifact(s) from other versions"
    printf '    rm %s\n' "${STALE[@]}"
    rm -f "${STALE[@]}"
  fi
fi
rm -f dist/SHA256SUMS   # always regenerated below

# ── Build ────────────────────────────────────────────────────────────────────
if [ "$SUMS_ONLY" = 0 ]; then
  case "$(uname -s)" in
    Darwin)
      scripts/build-macos.sh
      if [ "${SKIP_WINDOWS:-0}" = "1" ]; then
        echo "==> Skipping build-cross.sh (SKIP_WINDOWS=1)"
      else
        # Its Linux stage is expected to fail on the ALSA cross-build and says
        # so loudly; only a missing Windows zip is a real failure, caught below.
        scripts/build-cross.sh || note "build-cross.sh exited non-zero"
      fi
      ;;
    Linux)
      scripts/build-linux-native.sh
      ;;
    *)
      echo "no build path for $(uname -s); see docs/RELEASE.md" >&2
      exit 2
      ;;
  esac
fi

# ── Inspect what actually got built ──────────────────────────────────────────
# Every artifact must carry the user-facing readme and the licence texts, and a
# Linux tarball without a binary is the exact thing that shipped before 2.1.0.
shopt -s nullglob
ARTIFACTS=()
for f in dist/*-"${VERSION}"-*; do [ -f "$f" ] && ARTIFACTS+=("$f"); done
shopt -u nullglob
if [ "${#ARTIFACTS[@]}" -eq 0 ]; then
  echo "no artifacts for $VERSION in dist/ — nothing to check or checksum" >&2
  exit 1
fi

echo "==> Checking ${#ARTIFACTS[@]} artifact(s)"
for a in "${ARTIFACTS[@]}"; do
  case "$a" in
    *.zip)    list="$(unzip -Z1 "$a")" ;;
    *.tar.gz) list="$(tar -tzf "$a")" ;;
    *)        continue ;;   # .dmg: mounting to inspect is not worth it
  esac
  # (^|/) so an AppleDouble "._LICENSE" can never satisfy the LICENSE check.
  printf '%s\n' "$list" | grep -Eq '(^|/)README\.txt$' || note "$a: no README.txt"
  printf '%s\n' "$list" | grep -Eq '(^|/)LICENSE$'           || note "$a: no LICENSE"
  printf '%s\n' "$list" | grep -Eq '(^|/)OFL\.txt$'          || note "$a: no OFL.txt (font licence)"
  case "$a" in
    *macos*)
      printf '%s\n' "$list" | grep -Eq '^__MACOSX'           && note "$a: carries __MACOSX junk"
      printf '%s\n' "$list" | grep -Eq '/MacOS/ivory$'       || note "$a: no Ivory.app executable"
      ;;
    *windows*)
      printf '%s\n' "$list" | grep -Eq '^ivory\.exe$'        || note "$a: no ivory.exe"
      ;;
    *linux*)
      printf '%s\n' "$list" | grep -Eq '/ivory$'             || note "$a: NO BINARY (the pre-2.1.0 empty-tarball bug)"
      ;;
  esac
done

# ── Checksums ────────────────────────────────────────────────────────────────
echo "==> Writing dist/SHA256SUMS"
( cd dist && sha256 "${ARTIFACTS[@]#dist/}" > SHA256SUMS )
sed 's/^/    /' dist/SHA256SUMS
ls -lh "${ARTIFACTS[@]}" | sed 's/^/    /'

if [ "$FAIL" = 0 ]; then
  echo "==> OK. Next: docs/RELEASE.md step 6 (smoke test), 8 (tag), 9 (publish)."
else
  echo "==> FINISHED WITH PROBLEMS — see the !! lines above. Do not publish."
  exit 1
fi
