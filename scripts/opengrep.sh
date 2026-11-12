#!/usr/bin/env bash
# Run Tapedeck's Opengrep rules (.opengrep/) the way the PR guard does.
#
#   scripts/opengrep.sh            rule tests, then what this branch adds
#                                  over origin/main
#   scripts/opengrep.sh BASE       ...over BASE (a branch, tag or SHA)
#   scripts/opengrep.sh --all      every finding in the tree, old ones too
#
# Exits 1 when the branch adds an ERROR finding — the same thing that fails
# the tapedeck-pr/static status. WARNINGs are printed and never fail.
#
# Uses `opengrep` from $OPENGREP or PATH when it is the pinned version, and
# otherwise downloads the pinned release from GitHub into
# ${XDG_CACHE_HOME:-~/.cache}/tapedeck/opengrep-<version>/, refusing it unless
# its SHA-256 matches the one below. The guard pins the same version
# (td-pipelines vars/opengrep.groovy): change one, change the other.
set -euo pipefail

VERSION=1.33.0
# The SHA-256 of each release asset, as GitHub publishes it for v$VERSION.
pinned_sha256() {
  case "$1" in
    opengrep_osx_arm64)         echo 87d38a3ca198694a651e723b503e3ff8b0547429b4b9dfa2946f75fc43bb1962 ;;
    opengrep_osx_x86)           echo 838b74a056be35adfd1067ed7567e0d946b73066ba1b6be8fe5fe966b2142a7e ;;
    opengrep_manylinux_x86)     echo 2545812755572e1355ea60f96288f82a26eebed7fc8b512430d49d5667214c24 ;;
    opengrep_manylinux_aarch64) echo d58d332649c347678431647661d83f151aa81a0871648228e16a5767117dff92 ;;
  esac
}
TARGETS=(src web/src)

cd "$(git rev-parse --show-toplevel)"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

find_opengrep() {
  local cand
  for cand in "${OPENGREP:-}" "$(command -v opengrep || true)"; do
    if [ -n "$cand" ] && [ -x "$cand" ] && [ "$("$cand" --version 2>/dev/null | tail -1)" = "$VERSION" ]; then
      echo "$cand"; return
    fi
  done
  local asset
  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64)          asset=opengrep_osx_arm64 ;;
    Darwin/x86_64)         asset=opengrep_osx_x86 ;;
    Linux/x86_64)          asset=opengrep_manylinux_x86 ;;
    Linux/aarch64|Linux/arm64) asset=opengrep_manylinux_aarch64 ;;
    *) echo "No pinned Opengrep build for $(uname -s)/$(uname -m); install $VERSION and set OPENGREP." >&2; exit 2 ;;
  esac
  local dir="${XDG_CACHE_HOME:-$HOME/.cache}/tapedeck/opengrep-$VERSION"
  local bin="$dir/$asset"
  if [ ! -x "$bin" ]; then
    mkdir -p "$dir"
    echo "Downloading Opengrep $VERSION ($asset) from GitHub…" >&2
    curl -fsSL -o "$bin.part" "https://github.com/opengrep/opengrep/releases/download/v$VERSION/$asset"
    local got; got="$(sha256 "$bin.part")"
    if [ "$got" != "$(pinned_sha256 "$asset")" ]; then
      rm -f "$bin.part"
      echo "Refusing $asset: SHA-256 $got, expected $(pinned_sha256 "$asset")." >&2; exit 2
    fi
    chmod +x "$bin.part" && mv "$bin.part" "$bin"
  fi
  echo "$bin"
}

OG="$(find_opengrep)"
COMMON=(--config .opengrep --disable-version-check --quiet)

echo "── rule tests" >&2
"$OG" test .opengrep

if [ "${1:-}" = "--all" ]; then
  echo "── every finding in ${TARGETS[*]}" >&2
  "$OG" scan "${COMMON[@]}" "${TARGETS[@]}"
  exit 0
fi

BASE="$(git merge-base "${1:-origin/main}" HEAD)"
echo "── what this branch adds over ${1:-origin/main} (merge base ${BASE:0:8})" >&2
# The full report for reading, then the ERRORs alone for the verdict.
"$OG" scan "${COMMON[@]}" --baseline-commit "$BASE" "${TARGETS[@]}"
if ! "$OG" scan "${COMMON[@]}" --baseline-commit "$BASE" --severity ERROR --error "${TARGETS[@]}" >/dev/null 2>&1; then
  echo "✗ This branch adds an ERROR finding (above). The PR guard's tapedeck-pr/static would fail." >&2
  exit 1
fi
echo "✓ No new ERROR findings." >&2
