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
# xdg-open also returns 4 when a configured app fails to launch.
case "$1" in *.nohandler|*.patch) exit 4 ;; esac
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

# The opener starts, then finds no application: that must be said, not
# left looking like a successful open.
printf 'content fixture\n' > "$d48/orphan.nohandler"
sleep 1
key Escape; typ "orphan.nohandler"
expect "cursor on orphan.nohandler" "d['model']['cursor_name']" "orphan.nohandler"
key Return
wait_for "'orphan.nohandler' in d['alertText']" "True" 20 >/dev/null
# An opener failure names the file and offers a way to choose an app.
expect_contains "a file the desktop could not open says so" "d['alertText']" 'open "orphan.nohandler"'
shot 48-no-application
key Escape

key Escape; typ "IMG_0001.JPG"
palette_run "Run program"
expect "Run refuses content without a confirmation" "d['confirmOpen']" "False"
expect_contains "Run points back to Open" "d['alertText']" "use Open instead of Run"
shot 48-content-is-not-a-program
key Escape

# Only the administrator associates an editor, and only with the parent type.
# Keep this complete desktop registry separate from the rest of the fixture.
registry="$d48/.desktop"
mkdir -p "$registry"/{admin,user,data/applications,system,bin}
ln -s /usr/share/mime "$registry/system/mime"
ln -s "$(command -v gio)" "$registry/bin/gio"
cat > "$registry/admin/mimeapps.list" <<'ASSOCIATIONS'
[Added Associations]
text/plain=qa-editor.desktop;
ASSOCIATIONS
cat > "$registry/data/applications/qa-editor.desktop" <<'EDITOR'
[Desktop Entry]
Type=Application
Name=QA Editor
Exec=/usr/bin/true %f
Path=/nonexistent-gust-qa-editor-directory
EDITOR
printf '%s\n' '--- old' '+++ new' '@@ -1 +1 @@' '-old' '+new' > "$d48/fix.patch"
python3 - "$d48/fix.patch" "$registry/data/recently-used.xbel" <<'PYTHON'
from pathlib import Path
from sys import argv
from xml.etree.ElementTree import Element, SubElement, ElementTree
doc = Element("xbel")
SubElement(doc, "bookmark", href=Path(argv[1]).as_uri())
ElementTree(doc).write(argv[2])
PYTHON
registry_env=("XDG_CONFIG_HOME=$registry/user" "XDG_CONFIG_DIRS=$registry/admin"
              "XDG_DATA_HOME=$registry/data" "XDG_DATA_DIRS=$registry/system")
printf -v open_env '%q ' "${registry_env[@]}"
# Ask the real desktop resolver to verify the fixture before exercising Gust.
mime_report="$(env "${registry_env[@]}" LC_ALL=C gio mime text/x-diff)"
if [[ "${mime_report%%$'\n'*}" == *"qa-editor.desktop" ]]; then
  ok "the administrator's editor is the inherited default for patches"
else
  bad "the isolated desktop did not resolve the patch editor: $mime_report"
fi
stop_gust; QA_ENV="$open_env" start_gust "$d48"
key Escape; typ "fix.patch"
expect "cursor on fix.patch" "d['model']['cursor_name']" "fix.patch"
key Return
wait_for "'fix.patch' in d['alertText']" "True" 20 >/dev/null
expect_contains "failed associated app names the patch" "d['alertText']" 'Could not open "fix.patch"'
expect_contains "failed open offers an alternative" "d['alertText']" 'Open with'
shot 48-associated-launch-failure
key Escape

# Recent used to set an "Opened <full path>" hint as soon as the opener spawned.
palette_run "Recent files"
expect "Recent opened" "d['recentOpen']" "True"
key Return
wait_for "'fix.patch' in d['alertText'] and not d['recentOpen']" "True" 20 >/dev/null
expect_contains "Recent reports the failed open" "d['alertText']" 'Could not open "fix.patch"'
expect "Recent never announces success on spawn" "d['hintStatus'].startswith('Opened ')" "False"
shot 48-recent-launch-failure
key Home
expect "the warning was dismissed" "d['alertText']" ""
expect "dismissal leaves no success hint" "d['hintStatus'].startswith('Opened ')" "False"
shot 48-recent-dismissed

# Mixed file types dispatch separately and can fail after the count is shown.
open_mixed_pair() {
  key Escape; typ "$1"; ctrl space
  sleep 1.3
  typ "$2"; ctrl space
  expect "$1 and $2 selected" "d['selCount']" "2"
  key Return
}
open_mixed_pair "fix.patch" "report.PDF"
wait_for "'fix.patch' in d['alertText']" "True" 20 >/dev/null
expect_contains "mixed selection names the failed file" "d['hintShown']" 'Could not open "fix.patch"'
expect_contains "mixed selection counts submitted requests" "d['hintStatus']" 'Sent 2 files to the desktop opener'

# Give only Gust a PATH containing real gio and no xdg-open. The valid editor's
# missing working directory makes GIO return 2 during launch.
printf -v gio_path 'PATH=%q' "$registry/bin"
stop_gust; QA_ENV="$open_env$gio_path" start_gust "$d48"
key Escape; typ "fix.patch"
key Return
wait_for "'fix.patch' in d['alertText']" "True" 20 >/dev/null
expect_contains "gio fallback reports launch failure" "d['alertText']" 'Could not open "fix.patch"'
shot 48-gio-launch-failure

# With the last opener gone, even dispatch fails. Keep that specific reason.
mv "$registry/bin/gio" "$registry/gio"
key Escape; typ "fix.patch"
key Return
expect_contains "missing desktop opener explains the refusal" "d['hintShown']" 'no desktop opener was found'

# Keep MIME detection working while Gust's PATH has no opener. Without it,
# Enter could fall back to the cursor row and miss the multi-file path.
cat > "$registry/bin/xdg-mime" <<'STUB'
#!/bin/sh
PATH=/usr/bin exec xdg-mime "$@"
STUB
chmod 755 "$registry/bin/xdg-mime"
open_mixed_pair "fix.patch" "report.PDF"
expect_contains "both files refused before dispatch" "d['alertText']" '2 files'
expect_contains "mixed selection reports the missing opener" "d['hintShown']" 'no desktop opener was found'
shot 48-mixed-no-opener

# With only handlr available, inspect the actual argv and its launch result.
cat > "$registry/bin/handlr" <<'STUB'
#!/bin/sh
printf '%s\0' "$@" > "${0%/*}/argv"
case "$3" in
  *.patch) exit 1 ;;
  *.PDF) /usr/bin/flock -s -w 20 "$0.lock" /usr/bin/true; exit 1 ;;
esac
STUB
chmod 755 "$registry/bin/handlr"
key Escape; typ "fix.patch"; key Return
if cmp -s "$registry/bin/argv" <(printf '%s\0' open -- "$d48/fix.patch"); then
  ok "handlr received open, -- and the exact file path"
else
  bad "handlr received incorrect arguments"
fi
wait_for "'fix.patch' in d['alertText']" "True" 20 >/dev/null
expect_contains "handlr dispatch failure reaches the window" "d['alertText']" 'Could not open "fix.patch"'
shot 48-handlr-launch-failure

# A dangling link fails before spawn. Keep that refusal after the PDF fails.
ln -s "$d48/missing-target" "$d48/unavailable.txt"
wait_for "'unavailable.txt' in d['model']['visible']" "True" 20 >/dev/null
# Release the PDF only after the earlier result is visible.
exec {pdf_gate}>"$registry/bin/handlr.lock"
flock -x "$pdf_gate"
open_mixed_pair "unavailable.txt" "report.PDF"
expect_contains "immediate refusal appears before the PDF exits" "d['hintShown']" 'unavailable.txt'
flock -u "$pdf_gate"
wait_for "'1 more' in d['hintShown']" "True" 20 || bad "the delayed PDF failure was not queued behind the refusal"
expect_contains "partial dispatch counts submitted files" "d['alertText']" 'Sent 1 of 2 files'
expect_contains "partial dispatch identifies the refused file" "d['hintShown']" 'unavailable.txt'
expect_contains "partial dispatch explains the refusal" "d['hintShown']" 'the file no longer exists'
shot 48-mixed-partial-dispatch
key Home
expect_contains "dismissing the refusal reveals the delayed failure" "d['hintShown']" 'Could not open "report.PDF"'
shot 48-mixed-delayed-failure

# Both openers start successfully, then exit unsuccessfully on their workers.
flock -x "$pdf_gate"
open_mixed_pair "fix.patch" "report.PDF"
wait_for "'fix.patch' in d['alertText']" "True" 20 >/dev/null
flock -u "$pdf_gate"
exec {pdf_gate}>&-
wait_for "'1 more' in d['hintShown']" "True" 20 >/dev/null
expect_contains "first launcher failure survives the second" "d['hintShown']" 'Could not open "fix.patch"'
expect_contains "the strip indicates another failure is waiting" "d['hintShown']" '1 more'
shot 48-two-launch-failures-first
key Home
expect_contains "the second launcher failure is readable" "d['hintShown']" 'Could not open "report.PDF"'
shot 48-two-launch-failures-second
key End
expect "both failures have been dismissed" "d['alertText']" ""

unset QA_PATH_PREPEND
stop_gust; start_gust "$FIXTURE"
