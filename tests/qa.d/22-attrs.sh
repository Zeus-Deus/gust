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
palette_run "Permissions"
wait_for "d['permissions'] and d['permissions']['ready']" "True" 10 >/dev/null
expect "the Permissions card opens with the real bits" "d['permissions']['orig']" "[2, 2, 0, 2, 0, 0, 2, 0, 0]"
expect "in words, not numbers" "d['permissions']['labels']" "['Read', 'Edit', 'Run']"
expect "Apply waits for a change" "d['permissions']['changed']" "False"
# The first box (You: Read) has the keyboard; You: Run is two to the right.
key Right; key Right; key space
expect "only owner Run is asked for" "(d['permissions']['add'], d['permissions']['remove'])" "(64, 0)"
key Return
wait_for "d['permissionsOpen']" "False" 10 >/dev/null
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 744 && "$(stat -c %a "$FIXTURE/p2.sh")" == 744 ]] \
  && ok "owner Run reached both files (744)" || bad "modes are $(stat -c %a "$FIXTURE/p1.sh") $(stat -c %a "$FIXTURE/p2.sh")"
expect_contains "the hint line says so and offers undo" "d['hintStatus']" "Ctrl+Z puts them back"
# hintStatus is the message even when something else is drawn: check what is.
expect_contains "and shows it, over the selection count" "d['hintShown']" "Ctrl+Z puts them back"
expect "the selection is kept" "d['selCount']" "2"
expect "as news, not as a failure" "d['alertText']" ""
shot 22b-permissions-done
ctrl z; sleep 1.0
expect_contains "after Ctrl+Z the strip is back on the selection" "d['hintShown']" "2 selected"
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 644 && "$(stat -c %a "$FIXTURE/p2.sh")" == 644 ]] \
  && ok "Ctrl+Z put both back (644)" || bad "after undo: $(stat -c %a "$FIXTURE/p1.sh") $(stat -c %a "$FIXTURE/p2.sh")"

say "22c · A mixed selection keeps what nobody changed, special flags included"
chmod 2754 "$FIXTURE/p1.sh"; chmod 640 "$FIXTURE/p2.sh"
palette_run "Permissions"
wait_for "d['permissions'] and d['permissions']['ready']" "True" 10 >/dev/null
expect "owner Run and group Run are mixed" "d['permissions']['orig']" "[2, 2, 1, 2, 0, 1, 1, 0, 0]"
# Group: Edit is one down and one right of the first box.
key Down; key Right; key space
expect "only group Edit is asked for" "(d['permissions']['add'], d['permissions']['remove'])" "(16, 0)"
key Return
wait_for "d['permissionsOpen']" "False" 10 >/dev/null
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 2774 && "$(stat -c %a "$FIXTURE/p2.sh")" == 660 ]] \
  && ok "each kept its own bits and setgid (2774, 660)" || bad "modes are $(stat -c %a "$FIXTURE/p1.sh") $(stat -c %a "$FIXTURE/p2.sh")"
ctrl z; sleep 1.0
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 2754 && "$(stat -c %a "$FIXTURE/p2.sh")" == 640 ]] \
  && ok "Ctrl+Z put both back (2754, 640)" || bad "after undo: $(stat -c %a "$FIXTURE/p1.sh") $(stat -c %a "$FIXTURE/p2.sh")"

say "22d · Escape and an unchanged card write nothing"
palette_run "Permissions"
wait_for "d['permissions'] and d['permissions']['ready']" "True" 10 >/dev/null
key Return; sleep 0.4
expect "Enter with nothing changed keeps the card open" "d['permissionsOpen']" "True"
key space; key Escape; sleep 0.6
expect "Escape closes it" "d['permissionsOpen']" "False"
[[ "$(stat -c %a "$FIXTURE/p1.sh")" == 2754 && "$(stat -c %a "$FIXTURE/p2.sh")" == 640 ]] \
  && ok "nothing changed on disk" || bad "a cancelled change was applied"
rm -f "$FIXTURE/p1.sh" "$FIXTURE/p2.sh"
