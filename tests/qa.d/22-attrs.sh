# shellcheck shell=bash
# Make link and Permissions: each with its undo, and undo refusing when the
# thing changed since (AGENTS.md: destructive paths ship with the undo test).

say "22a · Make link, and Ctrl+Z takes back only that link"
printf 'real\n' > "$FIXTURE/linkme.txt"
rm -f "$FIXTURE/linkme.txt (link)"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "linkm"; sleep 1.2
expect "cursor on linkme.txt" "d['model']['cursor_name']" "linkme.txt"
palette_run "Make link"; sleep 1.0
[[ -L "$FIXTURE/linkme.txt (link)" && "$(readlink "$FIXTURE/linkme.txt (link)")" == "$FIXTURE/linkme.txt" ]] \
  && ok "a link to linkme.txt landed on disk" || bad "no link on disk"
wait_for "d['model']['cursor_name']" "linkme.txt (link)" 10 >/dev/null
expect "the cursor is on the new link" "d['model']['cursor_name']" "linkme.txt (link)"
ctrl z; sleep 1.0
[[ ! -e "$FIXTURE/linkme.txt (link)" && ! -L "$FIXTURE/linkme.txt (link)" ]] && ok "Ctrl+Z removed the link" || bad "the link is still there"
[[ "$(cat "$FIXTURE/linkme.txt")" == "real" ]] && ok "and never touched the target" || bad "the target changed"
# Undo refuses a link the user replaced with real work.
typ "linkm"; sleep 1.2
palette_run "Make link"; sleep 1.0
rm -f "$FIXTURE/linkme.txt (link)"; printf 'my work\n' > "$FIXTURE/linkme.txt (link)"; sleep 0.8
ctrl z; sleep 1.0
[[ "$(cat "$FIXTURE/linkme.txt (link)" 2>/dev/null)" == "my work" ]] && ok "undo left the replacement alone" || bad "undo destroyed a file it did not make"
expect_contains "and says why" "d['alertText']" "left as it is"
key Escape
rm -f "$FIXTURE/linkme.txt" "$FIXTURE/linkme.txt (link)"

say "22b · Permissions for several files, and Ctrl+Z puts them back"
for f in p1.sh p2.sh; do printf '#!/bin/sh\n' > "$FIXTURE/$f"; chmod 644 "$FIXTURE/$f"; done
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "p1.s"; sleep 1.2; ctrl space; key Down; ctrl space
expect "two selected" "d['selCount']" "2"
palette_run "Permissions"; sleep 0.8
expect "a Permissions prompt opens" "d['promptMode']" "permissions"
ctrl a; typ "u+x"; key Return; sleep 1.0
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 744 && "$(stat -c %a "$FIXTURE/p2.sh")" == 744 ]] \
  && ok "u+x reached both files (744)" || bad "modes are $(stat -c %a "$FIXTURE/p1.sh") $(stat -c %a "$FIXTURE/p2.sh")"
ctrl z; sleep 1.0
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 644 && "$(stat -c %a "$FIXTURE/p2.sh")" == 644 ]] \
  && ok "Ctrl+Z put both back (644)" || bad "after undo: $(stat -c %a "$FIXTURE/p1.sh") $(stat -c %a "$FIXTURE/p2.sh")"
# A setuid request is refused, never applied.
palette_run "Permissions"; sleep 0.8
ctrl a; typ "4755"; key Return; sleep 0.8
expect "setuid is refused and the prompt stays" "d['promptOpen']" "True"
expect_contains "with the reason" "d['promptError']" "setuid"
key Escape
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 644 ]] && ok "nothing changed on disk" || bad "a refused mode was applied"
rm -f "$FIXTURE/p1.sh" "$FIXTURE/p2.sh"
