# shellcheck shell=bash
# Ctrl+L folder jump: a few letters reach a folder ranked across bookmarks,
# zoxide and recent files; Tab completes a typed path. All three sources are
# seeded inside the fixture home, never the real user's.

say "23a · Ctrl+L jumps to a folder by a few letters"
deep="$FIXTURE/Projects/clients/acme-invoices"
mkdir -p "$deep" "$FIXTURE/Projects/clients/beta-site" "$FIXTURE/Music/albums"
mkdir -p "$FIXTURE/.local/share/zoxide" "$FIXTURE/.config/gtk-3.0"
env HOME="$FIXTURE" XDG_DATA_HOME="$FIXTURE/.local/share" _ZO_DATA_DIR="$FIXTURE/.local/share/zoxide" \
  zoxide add "$deep" >/dev/null 2>&1
printf 'file://%s\n' "$FIXTURE/Music/albums" > "$FIXTURE/.config/gtk-3.0/bookmarks"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
ctrl l; sleep 1.2
expect "Ctrl+L opens the palette" "d['paletteOpen']" "True"
typ "acme"; sleep 1.0
expect_contains "the zoxide folder is offered first" "d['paletteResults'][0]" "acme-invoices"
shot 23a-jump
key Return
wait_for "d['model']['location']" "$deep" 10 >/dev/null
expect "Enter lands in it" "d['model']['location']" "$deep"

ctrl l; sleep 1.0; typ "albums"; sleep 1.0
expect_contains "a bookmark is offered too" "d['paletteResults'][0]" "Music/albums"
key Escape

say "23b · Tab completes a typed path"
ctrl l; sleep 1.0
typ "~/Proj"; key Tab; sleep 1.0
expect "Tab finishes the folder name" "d['paletteText']" "~/Projects/"
typ "cl"; key Tab; sleep 1.0
expect "and goes one deeper" "d['paletteText']" "~/Projects/clients/"
typ "b"; key Tab; sleep 1.0
expect "a unique prefix picks the one folder" "d['paletteText']" "~/Projects/clients/beta-site/"
key Return
wait_for "d['model']['location']" "$FIXTURE/Projects/clients/beta-site" 10 >/dev/null
expect "Enter goes there" "d['model']['location']" "$FIXTURE/Projects/clients/beta-site"
rm -rf "$FIXTURE/Projects/clients" "$FIXTURE/Music/albums" "$FIXTURE/.local/share/zoxide" "$FIXTURE/.config/gtk-3.0/bookmarks"
