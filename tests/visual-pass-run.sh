#!/usr/bin/env bash
# Shoot every surface of the visual pass in one installed Omarchy theme,
# without touching the user's current theme (a private state dir, like
# tests/theme-matrix.sh).   tests/visual-pass-run.sh <theme> <outdir>
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
theme="${1:?theme name}"; out="${2:?outdir}"
src="${GUST_THEMES_DIR:-/usr/share/omarchy/themes}/$theme"
[[ -f "$src/colors.toml" ]] || { echo "no theme $theme" >&2; exit 2; }
state="/tmp/gust-vp-state-$theme"
rm -rf "$state"; mkdir -p "$state/current"
cp -r "$src" "$state/current/theme"
live="$HOME/.local/state/omarchy/current/theme/shell.toml"
[[ -f "$state/current/theme/shell.toml" || ! -f "$live" ]] || cp "$live" "$state/current/theme/shell.toml"
echo "$theme" > "$state/current/theme.name"
GUST_OMARCHY_STATE="$state" exec "$HERE/qa-one.sh" "$HERE/visual-pass.sh" "$out"
