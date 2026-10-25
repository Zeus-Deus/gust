# shellcheck shell=bash
# AFC and non-Unix mounts can mark every file +x. Opening content must reach
# the desktop handler, while Run must not offer to execute that content.
say "48 · phone-style executable permissions do not turn content into programs"
d48="$FIXTURE/content-open"
mkdir -p "$d48/.bin"
for name in IMG_0001.JPG photo.HEIF clip.MOV memo.M4A report.PDF notes.txt; do
  printf 'content fixture\n' > "$d48/$name"
  chmod 700 "$d48/$name"
done
cat > "$d48/.bin/xdg-open" <<'STUB'
#!/bin/sh
printf '%s\n' "$@" >> "$(dirname "$0")/opened"
STUB
cat > "$d48/.bin/mpv" <<'STUB'
#!/usr/bin/python3
import pathlib, sys, urllib.parse
start = next(int(a.split('=', 1)[1]) for a in sys.argv if a.startswith('--playlist-start='))
uris = [line for line in sys.stdin.read().splitlines() if line and not line.startswith('#')]
path = urllib.parse.unquote(urllib.parse.urlsplit(uris[start]).path)
with (pathlib.Path(__file__).parent / 'opened').open('a') as output:
    output.write(path + '\n')
STUB
chmod 755 "$d48/.bin/xdg-open" "$d48/.bin/mpv"
export QA_PATH_PREPEND="$d48/.bin"
stop_gust; start_gust "$d48"
wait_for "d['model']['settled']" "True" 20 >/dev/null

for name in IMG_0001.JPG photo.HEIF clip.MOV memo.M4A report.PDF notes.txt; do
  key Escape
  typ "$name"
  expect "cursor on $name" "d['model']['cursor_name']" "$name"
  key Return
  if grep -Fxq "$d48/$name" "$d48/.bin/opened" 2>/dev/null; then
    ok "$name with mode 0700 reached the desktop opener"
  else
    bad "$name was not opened: $(state "d['hintStatus']")"
  fi
  expect "opening $name needs no Run confirmation" "d['confirmOpen']" "False"
done

key Escape; typ "IMG_0001.JPG"
palette_run "Run program"
expect "Run refuses content without a confirmation" "d['confirmOpen']" "False"
expect_contains "Run points back to Open" "d['alertText']" "use Open instead of Run"
shot 48-content-is-not-a-program
key Escape

unset QA_PATH_PREPEND
stop_gust; start_gust "$FIXTURE"
