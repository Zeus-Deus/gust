#!/usr/bin/env bash
# Field bench. Measures what the plan states as product targets, on THIS
# machine, and prints the numbers next to the target.
#
# It never claims a target is met on hardware it did not run on. The VM used
# for development has software rendering and little RAM, so a miss here is
# reported as a measurement, not a verdict on the design.
#
#   ./tests/bench.sh [outdir]
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
OUT="${1:-/tmp/gust-bench}"
BIG="${GUST_BENCH_DIR:-/tmp/gust-bench-tree}"
BIN="$ROOT/target/release/gust"
[[ -x "$BIN" ]] || BIN="$ROOT/target/debug/gust"

mkdir -p "$OUT"
STATE="$OUT/state.json"
RESULTS="$OUT/results.txt"
: > "$RESULTS"

say()  { printf '\n\033[1m== %s\033[0m\n' "$*"; }
line() { printf '  %-42s %12s   target %-12s %s\n' "$1" "$2" "$3" "$4"; \
         printf '%-42s %12s target %-12s %s\n' "$1" "$2" "$3" "$4" >> "$RESULTS"; }

vm() { "$HERE/vmsession" "$@"; }

# --- fixture ---------------------------------------------------------------
COUNT="${GUST_BENCH_COUNT:-100000}"
if [[ ! -d "$BIG" ]] || [[ "$(find "$BIG" -maxdepth 1 | head -n 2001 | wc -l)" -lt 2000 ]]; then
  say "building a $COUNT-entry directory (once)"
  rm -rf "$BIG"; mkdir -p "$BIG"
  # seq | xargs touch is far faster than a bash loop for six figures.
  seq -f "$BIG/entry-%06g.dat" 1 "$COUNT" | xargs -d '\n' -n 2000 touch
fi
ACTUAL="$(find "$BIG" -maxdepth 1 -type f | wc -l)"
say "fixture: $ACTUAL entries in $BIG"

# --- 1. cold listing -------------------------------------------------------
# gust-core's lister, measured directly: no GUI, no compositor noise.
say "1 · listing throughput (gust-core, no UI)"
cat > "$OUT/bench_list.rs" <<'EOF'
// Measures the two real phases: names (what the user sees first) and
// metadata (what makes the columns true).
fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let key = gust_core::location::LocationKey::local(std::path::Path::new(&dir));
    let cancel = gust_core::lister::CancelToken::new();

    let t = std::time::Instant::now();
    let mut snap = gust_core::lister::list_names(&key, 0, &cancel, |_| {});
    let names_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t2 = std::time::Instant::now();
    gust_core::lister::fill_metadata(&mut snap, &cancel);
    let meta_ms = t2.elapsed().as_secs_f64() * 1000.0;

    println!("{} {:.1} {:.1}", snap.entries.len(), names_ms, names_ms + meta_ms);
}
EOF
mkdir -p "$ROOT/crates/core/examples"
cp "$OUT/bench_list.rs" "$ROOT/crates/core/examples/bench_list.rs"
if (cd "$ROOT" && cargo build --release -p gust-core --example bench_list >/dev/null 2>&1); then
  # Warm the page cache, then take the best of three.
  "$ROOT/target/release/examples/bench_list" "$BIG" >/dev/null 2>&1
  BEST=999999
  for _ in 1 2 3; do
    R="$("$ROOT/target/release/examples/bench_list" "$BIG" 2>/dev/null)"
    MS="$(echo "$R" | awk '{print $3}')"
    NAMES="$(echo "$R" | awk '{print $2}')"
    N="$(echo "$R" | awk '{print $1}')"
    BEST="$(python3 -c "print(min($BEST,$MS))")"
  done
  line "names only, $N entries (first paint)" "${NAMES} ms" "< 400 ms" \
       "$(python3 -c "print('PASS' if ${NAMES:-99999} < 400 else 'OVER')")"
  line "names + metadata, $N entries" "${BEST} ms" "< 1500 ms" \
       "$(python3 -c "print('PASS' if $BEST < 1500 else 'OVER')")"
else
  line "list (gust-core)" "BUILD FAILED" "< 1500 ms" "UNKNOWN"
fi

# --- 2. first window -------------------------------------------------------
say "2 · time to first painted window"
FIRST_RUN="$OUT/first.log"
REAL_HOME="$HOME"
cat > "$OUT/first.sh" <<EOF
#!/usr/bin/env bash
REAL_HOME="$REAL_HOME"
exec > "$FIRST_RUN" 2>&1
export HOME=$OUT/home
export XDG_CONFIG_HOME=$OUT/home/.config
export XDG_STATE_HOME=$OUT/home/.local/state
export XDG_DATA_HOME=$OUT/home/.local/share
export XDG_CACHE_HOME=$OUT/home/.cache
export GUST_OMARCHY_STATE=\${GUST_OMARCHY_STATE:-\$REAL_HOME/.local/state/omarchy}
export GUST_QA_STATE=$STATE
# Compare renderers on machines without a GPU: "software" is Qt's raster
# scene graph (no LLVM); unset uses the platform default (llvmpipe here).
${GUST_BENCH_QT_BACKEND:+export QT_QUICK_BACKEND=$GUST_BENCH_QT_BACKEND}
exec "$BIN" "$BIG"
EOF
chmod +x "$OUT/first.sh"
mkdir -p "$OUT/home"
pkill -x gust 2>/dev/null; sleep 0.5
rm -f "$STATE"
T0="$(date +%s.%N)"
setsid "$HERE/vmsession" "$OUT/first.sh" </dev/null >/dev/null 2>&1 &
BENCH_PID=$!
FIRST_MS=""
for _ in $(seq 1 200); do
  if [[ -f "$STATE" ]]; then
    T1="$(date +%s.%N)"
    FIRST_MS="$(python3 -c "print(f'{($T1-$T0)*1000:.0f}')")"
    break
  fi
  sleep 0.05
done
if [[ -n "$FIRST_MS" ]]; then
  line "first state written (window is up)" "${FIRST_MS} ms" "< 400 ms" \
       "$(python3 -c "print('PASS' if $FIRST_MS < 400 else 'OVER')")"
else
  line "first window" "NEVER APPEARED" "< 400 ms" "FAIL"
fi

# --- 3. settle on the big directory ---------------------------------------
say "3 · time until the big directory is fully listed"
SETTLE_MS=""
for _ in $(seq 1 400); do
  S="$(python3 -c "
import json
try:
    d=json.load(open('$STATE')); print(d['model']['settled'], d['model']['count'])
except Exception: print('False 0')" 2>/dev/null)"
  if [[ "$S" == True* ]]; then
    T2="$(date +%s.%N)"
    SETTLE_MS="$(python3 -c "print(f'{($T2-$T0)*1000:.0f}')")"
    SETTLE_N="$(echo "$S" | awk '{print $2}')"
    break
  fi
  sleep 0.05
done
if [[ -n "$SETTLE_MS" ]]; then
  line "listed+settled $SETTLE_N entries" "${SETTLE_MS} ms" "< 2000 ms" \
       "$(python3 -c "print('PASS' if $SETTLE_MS < 2000 else 'OVER')")"
else
  line "settle on $ACTUAL entries" "NEVER SETTLED" "< 2000 ms" "FAIL"
fi

# --- 4. memory -------------------------------------------------------------
say "4 · resident memory with the big directory open"
GPID="$(pgrep -x gust | head -1)"
if [[ -n "$GPID" ]]; then
  PSS="$(awk '/^Pss:/ {s+=$2} END {print s}' "/proc/$GPID/smaps_rollup" 2>/dev/null \
        || awk '/^Pss:/ {s+=$2} END {print s}' "/proc/$GPID/smaps" 2>/dev/null)"
  RSS="$(awk '/^VmRSS:/ {print $2}' "/proc/$GPID/status")"
  PSS_MB="$(python3 -c "print(f'{${PSS:-0}/1024:.1f}')")"
  RSS_MB="$(python3 -c "print(f'{${RSS:-0}/1024:.1f}')")"
  line "PSS with $ACTUAL entries open" "${PSS_MB} MiB" "< 90 MiB" \
       "$(python3 -c "print('PASS' if ${PSS_MB} < 90 else 'OVER')")"
  line "RSS with $ACTUAL entries open" "${RSS_MB} MiB" "(informational)" ""
else
  line "memory" "NO PROCESS" "< 90 MiB" "FAIL"
fi

# --- 5. responsiveness while listing --------------------------------------
say "5 · the window still answers keys in a huge folder"
# Wait for the listing to exist before asking the cursor to move to its end.
for _ in $(seq 1 100); do
  C="$(python3 -c "
import json
try: print(json.load(open('$STATE'))['model']['count'])
except Exception: print(0)")"
  [[ "${C:-0}" -gt 1000 ]] && break
  sleep 0.1
done
vm wtype -k End >/dev/null 2>&1
sleep 1.5
CUR="$(python3 -c "
import json
try: print(json.load(open('$STATE'))['model']['cursor'])
except Exception: print(-1)")"
if [[ "$CUR" -gt 1000 ]]; then
  line "End key reached row $CUR" "responsive" "responds" "PASS"
else
  line "End key" "cursor=$CUR" "responds" "FAIL"
fi

pkill -x gust 2>/dev/null
wait $BENCH_PID 2>/dev/null

say "environment (numbers are only valid for THIS machine)"
{
  echo "host:     $(uname -srm)"
  echo "cpu:      $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
  echo "cores:    $(nproc)"
  echo "memory:   $(free -m | awk '/^Mem:/ {print $2" MiB"}')"
  echo "renderer: $(grep -o 'llvmpipe\|virgl\|radeonsi\|iris' <<<"$(glxinfo 2>/dev/null | head -40)" | head -1 || echo 'unknown (likely software in a VM)')"
  echo "build:    $([[ "$BIN" == *release* ]] && echo release || echo DEBUG)"
} | tee -a "$RESULTS"

printf '\n\033[1m== results saved to %s\033[0m\n' "$RESULTS"
