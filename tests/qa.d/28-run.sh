# shellcheck shell=bash
# Run… : a program starts only after a prompt that names it, with Cancel
# focused — from the menu, the palette, or Enter on it. A program without +x
# is offered "Allow and run" (u+x, undoable); content is refused. The program
# runs in the default terminal (xdg-terminal-exec), never through a shell.

say "28a · Run… asks first; Enter cancels; Run runs it"
d28="$FIXTURE/run"
marker=/tmp/gust-fixture-RAN
rm -rf "$d28" "$marker"; mkdir -p "$d28"
printf '#!/bin/sh\ntouch %s\n' "$marker" > "$d28/hello.sh"; chmod 755 "$d28/hello.sh"
printf '#!/bin/sh\ntouch %s\n' "$marker" > "$d28/plain.sh"; chmod 644 "$d28/plain.sh"
printf 'just words\n' > "$d28/words.txt"; chmod 755 "$d28/words.txt"
# A stand-in terminal launcher: records its argv, runs what follows "--"
# directly, opens no window. A real terminal would take the keyboard and
# swallow the next section's keys; the argv Gust builds is what is tested.
stub="$d28/.bin"; mkdir -p "$stub"
cat > "$stub/xdg-terminal-exec" <<'STUB'
#!/bin/sh
printf '%s\n' "$@" > /tmp/gust-fixture-TERMARGV
while [ $# -gt 0 ] && [ "$1" != "--" ]; do shift; done
[ "$1" = "--" ] && shift
exec "$@"
STUB
chmod 755 "$stub/xdg-terminal-exec"
rm -f /tmp/gust-fixture-TERMARGV
export QA_PATH_PREPEND="$stub"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$d28" || bad "could not open $d28"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "hello"; sleep 1.0
expect "cursor on hello.sh" "d['model']['cursor_name']" "hello.sh"
key Return; sleep 0.8
expect "Enter on a program asks first" "d['confirmTitle']" "Run hello.sh?"
key Return; sleep 0.8
expect "and Enter again cancels" "d['confirmOpen']" "False"
[[ ! -e "$marker" ]] && ok "Enter Enter on a program runs nothing" || bad "Enter RAN a program"
palette_run "Run program"; sleep 0.8
expect "a confirmation opens" "d['confirmOpen']" "True"
shot 28a-run-confirm
hk="$(state "d['hintKeys']")"
[[ "$hk" == *"Esc cancel"* && "$hk" != *"Enter open"* ]] \
  && ok "the hint strip shows the dialog's keys ($hk)" || bad "the hint strip shows '$hk' under a dialog"
key Return; sleep 0.8
expect "Enter cancels (Cancel has focus)" "d['confirmOpen']" "False"
[[ ! -e "$marker" ]] && ok "cancelled: nothing ran" || bad "a cancelled Run ran the program"
palette_run "Run program"; sleep 0.8
key Tab; key Return; sleep 3.0
[[ -e "$marker" ]] && ok "confirmed Run started the program" || bad "confirmed Run did not start it"
argv="$(tr '\n' ' ' < /tmp/gust-fixture-TERMARGV 2>/dev/null)"
[[ "$argv" == "--hold --dir=$d28 -- $d28/hello.sh " ]] \
  && ok "it went to the terminal launcher as argv: $argv" || bad "terminal launcher got '$argv'"
[[ "$(vm hyprctl activewindow -j | python3 -c 'import json,sys;print(json.load(sys.stdin).get("class",""))')" == gust ]] \
  && ok "Gust still has the keyboard" || bad "another window took the keyboard"

say "28b · a program without the x bit is offered the permission, undoably"
rm -f "$marker" /tmp/gust-fixture-TERMARGV
hyprctl dispatch 'hl.dsp.focuswindow({ "class:gust" })' >/dev/null 2>&1; sleep 0.4
typ "plain"; sleep 1.0
expect "cursor on plain.sh" "d['model']['cursor_name']" "plain.sh"
key Return; sleep 0.8
expect_contains "Enter offers the permission" "d['confirmTitle']" "Allow plain.sh to run?"
shot 28b-allow-confirm
key Return; sleep 0.8
expect "Enter cancels (Cancel has focus)" "d['confirmOpen']" "False"
[[ ! -e "$marker" && "$(stat -c %a "$d28/plain.sh")" == 644 ]] \
  && ok "cancelled: nothing ran, mode still 644" || bad "a cancelled Allow changed or ran something"
palette_run "Run program"; sleep 0.8
expect_contains "the palette asks the same" "d['confirmTitle']" "Allow plain.sh to run?"
key Tab; key Return; sleep 3.0
[[ -e "$marker" ]] && ok "Allow and run started it" || bad "Allow and run did not start it"
[[ "$(stat -c %a "$d28/plain.sh")" == 744 ]] && ok "it gained u+x only" \
  || bad "mode after Allow is $(stat -c %a "$d28/plain.sh")"
hyprctl dispatch 'hl.dsp.focuswindow({ "class:gust" })' >/dev/null 2>&1; sleep 0.4
ctrl z; sleep 1.0
[[ "$(stat -c %a "$d28/plain.sh")" == 644 ]] && ok "Ctrl+Z takes the permission back" \
  || bad "Ctrl+Z left mode $(stat -c %a "$d28/plain.sh")"

say "28c · Properties holds the switch; content is refused"
rm -f "$marker"
key space; sleep 0.8
expect "Quick Look shows it as a program to allow" "d['quickLookProgram']" "allow"
shot 28c-quicklook-allow
key x; sleep 0.8
expect "x allows it" "d['quickLookProgram']" "run"
[[ "$(stat -c %a "$d28/plain.sh")" == 744 ]] && ok "and the file has u+x" || bad "x left mode $(stat -c %a "$d28/plain.sh")"
key x; sleep 0.8
expect "x again stops it, and the switch stays" "d['quickLookProgram']" "allow"
[[ "$(stat -c %a "$d28/plain.sh")" == 644 ]] && ok "back to 644" || bad "x off left mode $(stat -c %a "$d28/plain.sh")"
key Escape; sleep 0.4
[[ ! -e "$marker" ]] && ok "the switch ran nothing" || bad "the Properties switch ran the program"
typ "words"; sleep 1.0
expect "cursor on words.txt" "d['model']['cursor_name']" "words.txt"
palette_run "Run program"; sleep 0.8
expect "no confirmation for content" "d['confirmOpen']" "False"
expect_contains "and it says why" "d['alertText']" "use Open instead of Run"
key Escape
unset QA_PATH_PREPEND
stop_gust; rm -rf "$d28" "$marker" /tmp/gust-fixture-TERMARGV; start_gust "$FIXTURE"
