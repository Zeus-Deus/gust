#!/usr/bin/env bash
# Flea's own field bench (thisisgm/flea tools/flea-field-bench) with a Gust
# entrant (bench/harness/gust-entrant.diff). Runs the 100k and the media
# fixture, three cold runs per entrant, and writes CSVs + medians.
#
#   just bench-field                  # standard profile
#   PROFILE=lowend just bench-field   # label only: boot the VM as 2 vCPU / 2 GB
#                                     # with QT_QUICK_BACKEND=software first
#
# Needs: FLEA_REPO (a built Flea checkout); fixtures from Flea's
# tools/flea-bench and tools/flea-media-fixture under /home/flea-sandbox
# (btrfs, outside $HOME); passwordless sudo for cache drops (stdin is
# /dev/null, so no password is ever read); a Wayland session.
# Never run it on a desktop you are using.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
: "${FLEA_REPO:?set FLEA_REPO to a built Flea checkout}"
PROFILE="${PROFILE:-std}"
STAMP="$(date +%F)"
OUT="$REPO/bench/field"; mkdir -p "$OUT"
export GUST_BIN="${GUST_BIN:-$REPO/target/release/gust}"
export STRATA_BIN="${STRATA_BIN:-/usr/bin/strata}"
[[ -x "$GUST_BIN" ]] || { echo "build first: cargo build --release -p gust-app"; exit 1; }
run() { # name fixture expected-files
  local csv="$OUT/$STAMP-$1-$PROFILE.csv"
  FIXTURE="$2" EXPECT_FILES="$3" DROP_THUMBS=yes \
    bash "$HERE/harness/flea-field-bench-with-gust" "$csv" </dev/null
  echo "== $1 ($PROFILE)"
  python3 "$HERE/harness/aggregate.py" "$csv" | tee "${csv%.csv}.txt"
}
run scale /home/flea-sandbox/flea-bench-btrfs 100000
run media /home/flea-sandbox/flea-media-btrfs 2000
