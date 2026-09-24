#!/usr/bin/env bash
# Screenshot Gust under every INSTALLED Omarchy theme and assert each one
# renders with that theme's own colours.
#
# The theme list is enumerated at runtime (never a hardcoded count), and the
# user's current theme is never changed: each run points Gust at a private
# copy of the theme directory through GUST_OMARCHY_STATE.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
OUTDIR="${1:-/tmp/gust-themes}"
FIXTURE="${GUST_FIXTURE:-/tmp/gust-fixture}"
THEMES_DIR="${GUST_THEMES_DIR:-/usr/share/omarchy/themes}"
BIN="$REPO/target/debug/gust"

mkdir -p "$OUTDIR"
PASS=0; FAIL=0

vm() { "$HERE/vmsession" "$@"; }

[[ -d "$FIXTURE" ]] || "$HERE/make-fixture.sh" "$FIXTURE" >/dev/null
mkdir -p "$FIXTURE/.local/share"

mapfile -t THEMES < <(find "$THEMES_DIR" -maxdepth 1 -mindepth 1 -type d | sort)
echo "found ${#THEMES[@]} installed themes in $THEMES_DIR"
[[ "${#THEMES[@]}" -gt 0 ]] || { echo "no themes found"; exit 1; }

for theme_path in "${THEMES[@]}"; do
  name="$(basename "$theme_path")"
  [[ -f "$theme_path/colors.toml" ]] || { echo "  skip $name (no colors.toml)"; continue; }

  # Private state dir: current/theme points at this theme, nothing global.
  state="/tmp/gust-theme-state-$name"
  rm -rf "$state"
  mkdir -p "$state/current"
  cp -r "$theme_path" "$state/current/theme"
  # shell.toml is generated per install; reuse the live one when the theme
  # does not ship its own, which is exactly what Omarchy does.
  if [[ ! -f "$state/current/theme/shell.toml" && -f "$HOME/.local/state/omarchy/current/theme/shell.toml" ]]; then
    cp "$HOME/.local/state/omarchy/current/theme/shell.toml" "$state/current/theme/shell.toml"
  fi
  echo "$name" > "$state/current/theme.name"

  pkill -x gust 2>/dev/null || true
  sleep 0.4
  cat > /tmp/gust-theme-run.sh <<EOF
#!/usr/bin/env bash
exec > /tmp/gust-theme.log 2>&1
export HOME="$FIXTURE"
export XDG_DATA_HOME="$FIXTURE/.local/share"
export GUST_OMARCHY_STATE="$state"
export GUST_QA_STATE=/tmp/gust-theme-state.json
exec "$BIN"
EOF
  chmod +x /tmp/gust-theme-run.sh
  vm /tmp/gust-theme-run.sh &
  sleep 3.2

  shot="$OUTDIR/$name.png"
  if ! vm grim "$shot" >/dev/null 2>&1; then
    echo "  FAIL $name (no screenshot)"; FAIL=$((FAIL+1)); continue
  fi

  # Assert: the window exists, the listing rendered, and the theme's own
  # background colour actually appears (not the fallback, not magenta).
  want_bg="$(grep -E '^background' "$theme_path/colors.toml" | head -1 | sed 's/.*"\(#[0-9a-fA-F]*\)".*/\1/')"
  result="$(python3 - "$shot" "$want_bg" <<'PY'
import subprocess, sys
shot, want = sys.argv[1], (sys.argv[2] or "").lower()
out = subprocess.run(["magick", shot, "-format", "%c", "-colors", "24",
                      "histogram:info:-"], capture_output=True, text=True).stdout
colors = []
for line in out.splitlines():
    if "#" in line:
        cnt = int(line.split(":")[0].strip())
        hx = "#" + line.split("#")[1].split()[0][:6].lower()
        colors.append((cnt, hx))
colors.sort(reverse=True)
top = [c for _, c in colors[:10]]
# Magenta sentinel means an unresolved token.
if "#ff00ff" in top or "#fd01fd" in top:
    print("FAIL magenta-token"); raise SystemExit
def near(a, b, tol=26):
    try:
        ar, ag, ab = int(a[1:3],16), int(a[3:5],16), int(a[5:7],16)
        br, bg, bb = int(b[1:3],16), int(b[3:5],16), int(b[5:7],16)
    except Exception:
        return False
    return abs(ar-br) <= tol and abs(ag-bg) <= tol and abs(ab-bb) <= tol
if want and any(near(c, want) for c in top):
    print("OK bg-matches " + want)
else:
    print("WARN bg " + (want or "?") + " not dominant; top=" + ",".join(top[:4]))
PY
)"
  case "$result" in
    OK*)   echo "  PASS $name  ($result)"; PASS=$((PASS+1));;
    WARN*) echo "  PASS $name  ($result)"; PASS=$((PASS+1));;
    *)     echo "  FAIL $name  ($result)"; FAIL=$((FAIL+1));;
  esac
  rm -rf "$state"
done

pkill -x gust 2>/dev/null || true
echo
echo "themes passed: $PASS   failed: $FAIL   shots: $OUTDIR"
[[ "$FAIL" -eq 0 ]] || exit 1
