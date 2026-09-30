#!/usr/bin/env bash
# Run ONE qa.d section against a fresh fixture, with the same setup, lock
# and helpers as the full suite:  tests/qa-one.sh tests/qa.d/20-keys.sh [outdir]
set -uo pipefail
SECTION="$(realpath "$1")"; shift
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/qa-lib.sh"
start_gust "$FIXTURE" || { echo "no window"; exit 1; }
source "$SECTION"
stop_gust
if grep -qiE "TypeError|is not a function|ReferenceError|QML .*: error" "$LOG" 2>/dev/null; then
  bad "QML runtime errors in $LOG"; grep -iE "TypeError|is not a function|ReferenceError" "$LOG" | sort -u | head -5
fi
printf '%s\n' "${RESULTS[@]}" > "$OUTDIR/results.txt"
printf '\n  passed: %d\n  failed: %d\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
