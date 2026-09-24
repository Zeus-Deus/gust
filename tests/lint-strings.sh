#!/usr/bin/env bash
# A Rust line continuation inside a string literal (\ at end of line) eats the
# newline but NOT the next line's indentation, and cargo fmt then bakes those
# spaces into one long line. The result is a sentence shown to the user with a
# gap in the middle of it: "Disconnecting now would          lose that work."
#
# The rule is about sentences a USER reads, so it looks only at code outside
# #[cfg(test)] modules. Test fixtures legitimately contain column-aligned
# data — a bsdtar listing line is padded by design — and excluding them one
# pattern at a time was turning the rule into a list of apologies.
set -uo pipefail
cd "$(dirname "$0")/.."
FAIL=0
echo "checking user-facing strings for collapsed continuations..."

while IFS= read -r hit; do
  echo "    $hit"
  FAIL=1
done < <(
  python3 - <<'PY'
import re, pathlib

# Three or more spaces inside a string literal, after a word character.
BAD = re.compile(r'"[^"]*[a-z,.]   +[a-zA-Z]')

for path in sorted(pathlib.Path("crates").glob("*/src/*.rs")):
    in_test = False
    depth_at_test = 0
    depth = 0
    for n, line in enumerate(path.read_text().splitlines(), 1):
        stripped = line.strip()

        # Entering a test module: everything inside is fixture data.
        if stripped.startswith("#[cfg(test)]"):
            in_test = True
            depth_at_test = depth
        # A #[test] function body counts as test code too.
        if stripped.startswith("#[test]") and not in_test:
            in_test = True
            depth_at_test = depth

        if not in_test and not stripped.startswith("//") and "r#\"" not in line:
            if BAD.search(line):
                print(f"{path}:{n}:{stripped[:110]}")

        depth += line.count("{") - line.count("}")
        if in_test and depth <= depth_at_test and ("}" in line):
            in_test = False
PY
)

if [[ "$FAIL" == 1 ]]; then
  echo "    ^ a line continuation left spaces inside a sentence the user reads."
  echo "      Put the whole sentence on one line, or build it with format!()."
else
  echo "  ok: no collapsed continuations in user-facing strings"
fi
exit "$FAIL"
