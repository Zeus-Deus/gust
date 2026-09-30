# shellcheck shell=bash
# Custom actions: a user's TOML file appears in the right-click menu and the
# palette for files it fits, runs as argv (never a shell), and a hostile
# filename stays one argument. A group-writable action file is ignored.

say "26a · a custom action runs on the selection, argv only"
d26="$FIXTURE/actions"
rm -rf "$d26"; mkdir -p "$d26" "$FIXTURE/.config/gust/actions"
out="$d26/ran.txt"
cat > "$FIXTURE/.config/gust/actions/stamp.toml" <<TOML
name = "Stamp it"
command = ["sh", "-c", "printf '%s\\\\n' \"\$1\" >> $out", "gust-action", "{file}"]
for = "files"
extensions = ["txt"]
TOML
chmod 644 "$FIXTURE/.config/gust/actions/stamp.toml"
cat > "$FIXTURE/.config/gust/actions/evil.toml" <<TOML
name = "Planted"
command = ["touch", "$d26/PLANTED"]
TOML
chmod 666 "$FIXTURE/.config/gust/actions/evil.toml"
hostile='a $(touch PWNED); b.txt'
printf x > "$d26/$hostile"; printf x > "$d26/pic.jpg"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$d26" || bad "could not open $d26"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "a \$"; sleep 1.2
expect_contains "cursor on the hostile name" "d['model']['cursor_name']" "PWNED"
palette_open; typ "Stamp"; sleep 0.8
expect_contains "the action is offered for a .txt" "d['paletteResults'][0]" "Stamp it"
key Return; sleep 1.5
[[ -f "$out" && "$(cat "$out")" == "$d26/$hostile" ]] && ok "it ran with the name as ONE literal argument" \
  || bad "action output: '$(cat "$out" 2>/dev/null)'"
[[ ! -e "$d26/PWNED" && ! -e "$FIXTURE/PWNED" ]] && ok "nothing in the name was executed" || bad "the filename was executed"
palette_open; typ "Planted"; sleep 0.8
# The palette always echoes the query as a "Filter this folder" row, so
# look for the action's own label exactly.
[[ "$(state "'Planted' in d['paletteResults']")" == "False" ]] && ok "a group-writable action file is not offered" \
  || bad "an untrusted action file was offered"
key Escape
typ "pic"; sleep 1.0
palette_open; typ "Stamp"; sleep 0.8
[[ "$(state "'Stamp it' in d['paletteResults']")" == "False" ]] && ok "not offered for a .jpg" || bad "offered for a file it does not fit"
key Escape
[[ ! -e "$d26/PLANTED" ]] && ok "the planted action never ran" || bad "a planted action ran"
stop_gust; rm -rf "$d26" "$FIXTURE/.config/gust/actions"; start_gust "$FIXTURE"
