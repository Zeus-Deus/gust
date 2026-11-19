#!/usr/bin/env bash
# Run ONE qa.d section against a fresh fixture, with the same setup, lock
# and helpers as the full suite:  tests/qa-one.sh tests/qa.d/20-keys.sh [outdir]
set -uo pipefail
if [[ ! -f "${1:-}" || ! -r "${1:-}" ]]; then
  printf 'Expected a readable QA section file, got %q\n' "${1:-}" >&2
  exit 1
fi
IFS= read -r -d '' SECTION < <(realpath -ez -- "$1") || exit 1
shift
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/qa-lib.sh"
start_gust "$FIXTURE" || { echo "no window"; exit 1; }
source "$SECTION"
section_status=$?
if (( section_status != 0 )); then
  bad "section $SECTION returned status $section_status"
fi
stop_gust
if grep -qiE "TypeError|is not a function|ReferenceError|QML .*: error" "$LOG" 2>/dev/null; then
  bad "QML runtime errors in $LOG"; grep -iE "TypeError|is not a function|ReferenceError" "$LOG" | sort -u | head -5
fi
if ! printf '%s\n' "${RESULTS[@]}" > "$OUTDIR/results.txt"; then
  printf 'Could not save QA results to %s/results.txt\n' "$OUTDIR" >&2
  exit 1
fi
printf '\n  passed: %d\n  failed: %d\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
finish_fixture
