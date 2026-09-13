#!/usr/bin/env bash
# A Rust line continuation inside a string literal (\ at end of line) eats the
# newline but NOT the next line's indentation, and cargo fmt then bakes those
# spaces into one long line. The result is a sentence shown to the user with a
# gap in the middle of it: "Disconnecting now would          lose that work."
#
# Nothing else in this codebase legitimately puts a run of spaces inside a
# user-facing string, so a run of three or more is the signal.
set -uo pipefail
cd "$(dirname "$0")/.."
FAIL=0
echo "checking user-facing strings for collapsed continuations..."
while IFS= read -r hit; do
  echo "    $hit"
  FAIL=1
done < <(grep -rnE '"[^"]*[a-z,.]   +[a-zA-Z]' crates/*/src/*.rs 2>/dev/null \
         | grep -vE '^\s*[^:]+:[0-9]+:\s*//' \
         | grep -vE 'r#"' \
         | grep -vE 'assert(_eq|_ne)?!' )
if [[ "$FAIL" == 1 ]]; then
  echo "    ^ a line continuation left spaces inside a sentence the user reads."
  echo "      Put the whole sentence on one line, or build it with format!()."
else
  echo "  ok: no collapsed continuations in user-facing strings"
fi
exit "$FAIL"
