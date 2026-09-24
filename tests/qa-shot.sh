#!/usr/bin/env bash
# Launch Gust against an isolated fixture and capture a screenshot.
#
# Usage: qa-shot.sh <fixture-dir> <out.png> [seconds-before-shot] [extra-env...]
#
# Runs with HOME pointed at the fixture so Gust starts there and can never
# reach the user's real files.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
FIXTURE="${1:?fixture dir}"
OUT="${2:?output png}"
DELAY="${3:-4}"
LOG="${LOG:-/tmp/gust-qa.log}"

mkdir -p "$(dirname "$OUT")"

# Stop any previous Gust we own (never any other process).
pkill -x gust 2>/dev/null || true
sleep 0.3

cat > /tmp/gust-qa-run.sh <<EOF
#!/usr/bin/env bash
exec > "$LOG" 2>&1
export HOME="$FIXTURE"
exec "$REPO/target/debug/gust"
EOF
chmod +x /tmp/gust-qa-run.sh

"$HERE/vmsession" /tmp/gust-qa-run.sh &
GUST_WRAPPER=$!
sleep "$DELAY"

# Prove the window really exists before claiming a screenshot means anything.
CLIENTS="$("$HERE/vmsession" hyprctl clients -j)"
echo "$CLIENTS" | python3 -c "
import sys, json
cs = json.load(sys.stdin)
gust = [c for c in cs if c.get('class') == 'gust']
if not gust:
    print('FAIL: no gust window on screen'); sys.exit(1)
c = gust[0]
print(f\"window: title={c['title']!r} size={c['size']}\")
"

"$HERE/vmsession" grim "$OUT"
echo "shot: $OUT"
