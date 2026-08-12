#!/usr/bin/env bash
# Publish this version's artifacts to the GitHub release, in BOTH spellings:
# the version-scoped names, and the version-less aliases that every permanent
# download link resolves through.
#
# Usage:
#   scripts/publish-github.sh --notes-file <file>   # create the release
#   scripts/publish-github.sh                       # assets only, release exists
#   DRY_RUN=1 scripts/publish-github.sh             # print, upload nothing
#
# WHY THE ALIASES EXIST — read before "tidying" them away.
# The permalink https://github.com/<repo>/releases/latest/download/<name>
# matches by EXACT asset name against whatever release is currently latest.
# The Gumroad post-purchase page, the supporter-key email and README.md all
# link through it, using the version-less names:
#
#   Ivory-macos-arm64.dmg   Ivory-macos-arm64.zip
#   ivory-windows-x86_64.zip   ivory-linux-x86_64.tar.gz   SHA256SUMS
#
# Ship a release carrying only `Ivory-2.3.0-macos-arm64.dmg` and every one of
# those links 404s for everybody, including people who have already paid, with
# nothing in any log to say so. That is the whole reason this script exists:
# uploading the aliases is not optional polish, it is the release contract.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

NOTES_ARGS=()
case "${1:-}" in
  "")           ;;
  --notes-file) [ -n "${2:-}" ] || { echo "--notes-file needs a path" >&2; exit 2; }
                NOTES_ARGS=(--notes-file "$2") ;;
  *) echo "unknown argument: $1 (expected --notes-file <file>)" >&2; exit 2 ;;
esac

command -v gh >/dev/null 2>&1 || { echo "gh CLI not installed" >&2; exit 2; }

# The repo is the GitHub mirror, never `origin` (that is Codeberg).
REMOTE_URL="$(git remote get-url github 2>/dev/null || true)"
[ -n "$REMOTE_URL" ] || { echo "no 'github' remote — see docs/RELEASE.md" >&2; exit 2; }
REPO="$(printf '%s' "$REMOTE_URL" | sed -E 's#.*github\.com[:/]##; s#\.git$##')"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
  echo "could not read a sane version from Cargo.toml (got '$VERSION')" >&2
  exit 2
fi
TAG="v${VERSION}"

echo "==> Ivory $VERSION -> $REPO $TAG"

RELEASE_EXISTS=0
gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1 && RELEASE_EXISTS=1

# Only a NEW release needs the tag to exist here first. Re-uploading assets to a
# published release does not, and `gh release create` will happily mint a tag on
# the remote that this clone has never seen — which is how v2.2.0 came to exist
# on GitHub but nowhere else. If that has happened, say so rather than fail.
if [ "$RELEASE_EXISTS" = 0 ]; then
  git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1 \
    || { echo "tag ${TAG} does not exist locally — docs/RELEASE.md step 8" >&2; exit 1; }
elif ! git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1; then
  echo "    NOTE: ${TAG} exists on GitHub but not in this clone — it was never"
  echo "          tagged locally or pushed to Codeberg (docs/RELEASE.md step 8)"
fi

# ── Collect this version's artifacts ─────────────────────────────────────────
shopt -s nullglob
ARTIFACTS=()
for f in dist/Ivory-"${VERSION}"-macos-*.zip dist/Ivory-"${VERSION}"-macos-*.dmg \
         dist/ivory-"${VERSION}"-linux-*.tar.gz dist/ivory-"${VERSION}"-windows-*.zip; do
  [ -f "$f" ] && ARTIFACTS+=("$f")
done
shopt -u nullglob
if [ "${#ARTIFACTS[@]}" -eq 0 ]; then
  echo "no $VERSION artifacts in dist/ — run scripts/release.sh first" >&2
  exit 1
fi
# Pick the checksum file, then PROVE it describes this version. `dist/` keeps a
# bare SHA256SUMS that a previous release wrote, so "the file exists" means
# nothing — publishing 2.1.0's sums alongside 2.3.0's binaries is worse than
# publishing none, because it reads as a failed integrity check.
SUMS="dist/SHA256SUMS-${VERSION}"
[ -f "$SUMS" ] || SUMS="dist/SHA256SUMS"
[ -f "$SUMS" ] || { echo "no checksum file in dist/ — scripts/release.sh --sums-only" >&2; exit 1; }
for a in "${ARTIFACTS[@]}"; do
  grep -Fq "  ${a#dist/}" "$SUMS" \
    || { echo "$SUMS does not cover ${a#dist/} (stale?) — scripts/release.sh --sums-only" >&2; exit 1; }
done
echo "==> Checksums: $SUMS"

# ── Build the alias set in a temp dir ────────────────────────────────────────
# `Ivory-2.2.0-macos-arm64.dmg` -> `Ivory-macos-arm64.dmg`, and the checksums
# go up twice: version-scoped for the record, bare for the permalink.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

UPLOADS=()
for a in "${ARTIFACTS[@]}"; do
  base="${a#dist/}"
  alias_name="${base/-${VERSION}-/-}"
  if [ "$alias_name" = "$base" ]; then
    echo "refusing to guess an alias for '$base' (no -${VERSION}- in the name)" >&2
    exit 1
  fi
  cp "$a" "$STAGE/$alias_name"
  UPLOADS+=("$a" "$STAGE/$alias_name")
done
cp "$SUMS" "$STAGE/SHA256SUMS-${VERSION}"
cp "$SUMS" "$STAGE/SHA256SUMS"
UPLOADS+=("$STAGE/SHA256SUMS-${VERSION}" "$STAGE/SHA256SUMS")

echo "==> ${#UPLOADS[@]} assets (${#ARTIFACTS[@]} artifacts, each in both spellings)"
for u in "${UPLOADS[@]}"; do echo "    $(basename "$u")"; done

if [ "${DRY_RUN:-0}" = "1" ]; then
  echo "==> DRY_RUN=1, nothing uploaded"
  exit 0
fi

# ── Create or update the release ─────────────────────────────────────────────
if [ "$RELEASE_EXISTS" = 1 ]; then
  echo "==> Release $TAG exists — uploading assets (--clobber)"
  if [ "${#NOTES_ARGS[@]}" -gt 0 ]; then
    echo "    NOTE: release exists, ignoring --notes-file (edit notes in the web UI)"
  fi
else
  if [ "${#NOTES_ARGS[@]}" -eq 0 ]; then
    echo "release $TAG does not exist and no --notes-file given" >&2
    exit 2
  fi
  echo "==> Creating release $TAG"
  gh release create "$TAG" --repo "$REPO" --title "Ivory ${VERSION}" "${NOTES_ARGS[@]}"
fi

gh release upload "$TAG" --repo "$REPO" --clobber "${UPLOADS[@]}"

# ── Prove the permalinks a customer will click actually resolve ──────────────
# Checking the release's own asset list is not enough: `latest` is a separate
# pointer, and a draft or a newer release moves it.
echo "==> Verifying /releases/latest/download/ permalinks"
FAIL=0
for u in "${UPLOADS[@]}"; do
  name="$(basename "$u")"
  case "$name" in *"-${VERSION}"*) continue ;; esac   # aliases only
  url="https://github.com/${REPO}/releases/latest/download/${name}"
  # A ranged GET for one byte, NOT curl -I. The redirect lands on GitHub's
  # asset CDN, which answers HEAD with an intermittent 503 even when the asset
  # is fine — that false alarm costs an hour of chasing a non-bug.
  code="$(curl -sL -r 0-0 -o /dev/null -w '%{http_code}' "$url" || echo 000)"
  if [ "$code" = "200" ] || [ "$code" = "206" ]; then
    echo "    ok   $name"
  else
    echo "    !!   $name -> HTTP $code  ($url)"
    FAIL=1
  fi
done

if [ "$FAIL" = 0 ]; then
  echo "==> OK. Every permanent download link resolves to $VERSION."
else
  echo "==> PERMALINKS BROKEN — customers' download links are dead. Fix before announcing." >&2
  exit 1
fi
