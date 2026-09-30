#!/usr/bin/env bash
# Run the library tests and report honest counts.
#
# libtest has no runtime skip: a fixture test that returns early prints `ok`.
# Every such early return goes through gust_testkit::skip!, which appends to a
# ledger; this script reads the ledger back, reports those cases as SKIP and
# subtracts them from the pass count. "533 passed" is a claim; "521 passed,
# 12 skipped" is a fact.
#
#   tests/run-tests.sh                 # ordinary run; fixtures may skip
#   GUST_ALL_REQUIRED=1 tests/run-tests.sh   # release gate: any skip fails
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
cd "$REPO"
mkdir -p target
export GUST_SKIP_LEDGER="$REPO/target/test-skips.log"
: > "$GUST_SKIP_LEDGER"
LOG="$REPO/target/test-run.log"
PKGS=(-p gust-core -p gust-ops -p gust-remote -p gust-thumbs -p gust-testkit)
cargo test --no-fail-fast "${PKGS[@]}" "$@" 2>&1 | tee "$LOG"
status=${PIPESTATUS[0]}
read -r passed failed ignored < <(grep -E '^test result' "$LOG" \
  | awk '{p+=$4; f+=$6; i+=$8} END {print p+0, f+0, i+0}')
skipped=$(grep -c '^SKIP' "$GUST_SKIP_LEDGER" || true)
skipped=${skipped:-0}
echo
echo "── gust test summary ──────────────────────────────"
if [[ "$skipped" -gt 0 ]]; then
  cut -f2,3,4 "$GUST_SKIP_LEDGER" | sort | awk -F'\t' '{printf "  SKIP  [%s] %s — %s\n", $1, $2, $3}'
fi
printf '  passed: %d   failed: %d   skipped: %d   ignored: %d\n' \
  "$((passed - skipped))" "$failed" "$skipped" "$ignored"
exit "$status"
