#!/usr/bin/env bash
# QML checks that matter, without drowning in qmllint's import noise.
#
# Two rules are enforced hard:
#   1. no colour / font-size / spacing literal in QML (AGENTS.md) — tokens only
#   2. no QtQuick.Controls import on the startup path (style engine + weight)
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI="$(cd "$HERE/../ui" && pwd)"
FAIL=0

echo "== token discipline (no literals in QML) =="
# Hex colours, named colours in a colour property, px font sizes, raw pixel
# margins. Tokens.qml is the one file allowed to define values.
while IFS= read -r hit; do
  echo "  LITERAL: $hit"
  FAIL=1
done < <(
  grep -nE '(color|border\.color)\s*:\s*"#[0-9a-fA-F]{3,8}"' "$UI"/*.qml \
    | grep -v '^\S*Tokens.qml'
  grep -nE 'font\.pixelSize\s*:\s*[0-9]+' "$UI"/*.qml \
    | grep -v '^\S*Tokens.qml'
  grep -nE '(anchors\.[a-zA-Z]*[Mm]argin|spacing)\s*:\s*[0-9]+(\s|$)' "$UI"/*.qml \
    | grep -vE '^\S*Tokens.qml' \
    | grep -vE ':\s*0(\s|$)'
)
[[ "$FAIL" == 0 ]] && echo "  ok: no colour/font/spacing literals"

echo
echo "== no QtQuick.Controls on the startup path =="
if grep -rn "^import QtQuick.Controls" "$UI"/*.qml; then
  echo "  FAIL: Controls pulls in a style engine that fights the theme tokens"
  FAIL=1
else
  echo "  ok: plain QtQuick only"
fi

echo
echo "== qmllint (syntax and resolvable references) =="
for f in "$UI"/*.qml; do
  out="$(/usr/lib/qt6/bin/qmllint "$f" 2>&1 \
        | grep -vE "was not found|Failed to import|Warnings occurred|^\s*$|import com.gust.app|Member \".*\" not found on type \"Tokens\"|Tokens is used but it is not resolved|unknown grouped property scope Accessible|Type .*(Tokens|DirModel|ThemeTokens|QaWriter).* not resolved" \
        | grep -E "^(Error|.*\.qml:[0-9]+:[0-9]+: (error|Error))" )"
  if [[ -n "$out" ]]; then
    echo "  $(basename "$f"):"
    echo "$out" | sed 's/^/    /'
    FAIL=1
  fi
done
[[ "$FAIL" == 0 ]] && echo "  ok: no QML syntax errors"

echo
echo "== brace balance =="
# qmllint gives up on a file whose imports it cannot resolve, and then
# reports success on syntactically broken QML. A mismatched brace once
# survived a PASS, so check the structure directly: this needs no imports
# and cannot be fooled by them.
for f in "$UI"/*.qml; do
  python3 - "$f" <<'PY' || FAIL=1
import re, sys
src = open(sys.argv[1]).read()
# Ignore braces inside strings and comments.
src = re.sub(r'//[^\n]*', '', src)
src = re.sub(r'/\*.*?\*/', '', src, flags=re.S)
src = re.sub(r'"(?:[^"\\]|\\.)*"', '""', src)
src = re.sub(r"'(?:[^'\\]|\\.)*'", "''", src)
depth = 0
for ch in src:
    if ch == '{':
        depth += 1
    elif ch == '}':
        depth -= 1
        if depth < 0:
            print(f"    {sys.argv[1]}: closing brace with nothing open")
            sys.exit(1)
if depth != 0:
    print(f"    {sys.argv[1]}: {depth} unclosed brace(s)")
    sys.exit(1)
PY
done
[[ "$FAIL" == 0 ]] && echo "  ok: every file's braces balance"

echo
if [[ "$FAIL" == 0 ]]; then
  echo "QML lint: PASS"
else
  echo "QML lint: FAIL"
fi
exit "$FAIL"
