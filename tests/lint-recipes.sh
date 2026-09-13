#!/usr/bin/env bash
# Every script a Justfile recipe invokes must exist and be executable.
#
# `just bench` shipped pointing at ./bench/run.sh, which was never created —
# the real script is tests/bench.sh. A documented command that fails on a
# missing file is worse than no command, and nothing caught it because the
# recipe was never run in a gate.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
cd "$REPO"

FAIL=0
while read -r script; do
  [[ -z "$script" ]] && continue
  if [[ ! -f "$script" ]]; then
    printf '  MISSING  %s (referenced by a Justfile recipe)\n' "$script"
    FAIL=1
  elif [[ ! -x "$script" ]]; then
    printf '  NOT EXEC %s\n' "$script"
    FAIL=1
  fi
done < <(grep -oE '\./[a-z]+/[a-z0-9._-]+\.sh' Justfile | sort -u)

# `just` must also be able to parse the file at all.
if ! just --list >/dev/null 2>&1; then
  echo "  Justfile does not parse"
  FAIL=1
fi

if [[ "$FAIL" -eq 0 ]]; then
  echo "Justfile recipes: PASS"
else
  echo "Justfile recipes: FAIL"
  exit 1
fi
