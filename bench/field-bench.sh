#!/usr/bin/env bash
# Flea's own field bench (thisisgm/flea tools/flea-field-bench) with a Gust
# entrant (bench/harness/gust-entrant.diff). Runs the 100k and the media
# fixture, three cold runs per entrant, and writes CSVs + medians.
#
#   just bench-field                  # standard profile
#   PROFILE=lowend just bench-field   # 2 CPUs, 2 GiB, software rendering
#
# The low-end profile is applied here, not by resizing the machine: the run
# re-enters itself pinned to two CPUs (taskset, inherited by every entrant),
# inside a user scope capped at 2 GiB (MemoryMax, swap off for the scope),
# with QT_QUICK_BACKEND=software for every Qt entrant. Caches are dropped
# before every run by the harness, as on the standard profile.
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
if [[ "$PROFILE" == lowend && -z "${GUST_LOWEND_APPLIED:-}" ]]; then
  export GUST_LOWEND_APPLIED=1 QT_QUICK_BACKEND=software
  # The unit name matches the harness's own scope pattern, so it measures
  # inside this capped scope instead of re-entering a fresh, uncapped one.
  exec systemd-run --user --scope --quiet -p Delegate=yes -p MemoryMax=2G \
    -p MemorySwapMax=0 "--unit=flea-field-bench-lowend-$$" -- \
    taskset -c 0,1 bash "$0" "$@"
fi
STAMP="$(date +%F)"
OUT="$REPO/bench/field"; mkdir -p "$OUT"
export GUST_BIN="${GUST_BIN:-$REPO/target/release/gust}"
export STRATA_BIN="${STRATA_BIN:-/usr/bin/strata}"
[[ -x "$GUST_BIN" ]] || { echo "build first: cargo build --release -p gust-app"; exit 1; }
# Flea's harness evals its private `omarchy-drive env`, which Omarchy does not
# package. Without it the run continues with a half-set session environment,
# so a fresh machine gets the repo's stand-in (harness/omarchy-drive-env-shim).
if ! command -v omarchy-drive >/dev/null 2>&1; then
  SHIM_DIR="$(mktemp -d)"; trap 'rm -rf "$SHIM_DIR"' EXIT
  ln -s "$HERE/harness/omarchy-drive-env-shim" "$SHIM_DIR/omarchy-drive"
  export PATH="$SHIM_DIR:$PATH"
fi
run() { # name fixture expected-files
  local csv="$OUT/$STAMP-$1-$PROFILE.csv"
  FIXTURE="$2" EXPECT_FILES="$3" DROP_THUMBS=yes \
    bash "$HERE/harness/flea-field-bench-with-gust" "$csv" </dev/null
  echo "== $1 ($PROFILE)"
  python3 "$HERE/harness/aggregate.py" "$csv" | tee "${csv%.csv}.txt"
}
run scale /home/flea-sandbox/flea-bench-btrfs 100000
run media /home/flea-sandbox/flea-media-btrfs 2000
