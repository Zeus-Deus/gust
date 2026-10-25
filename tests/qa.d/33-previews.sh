# shellcheck shell=bash
# Quick Look previews every everyday kind of file (DESIGN §6.9.1): Markdown
# rendered, every page of a PDF, audio played (decoded in the sandbox), and
# a video's frame with Enter handing it to the real player.

pv="$FIXTURE/previews"
rm -rf "$pv"; mkdir -p "$pv"
need() { command -v "$1" >/dev/null 2>&1; }

cat > "$pv/notes.md" <<'MD'
# Release notes

Gust now previews **Markdown**, *PDF pages* and `audio`.

- rendered headings
- [x] task lists

<img src="http://tracker.invalid/pixel.gif"> stays text.
MD

printf 'not audio' > "$pv/broken.mp3"

# A stand-in opener: records what Enter asked it to open; opens no window.
stub="$pv/.bin"; mkdir -p "$stub"
cat > "$stub/xdg-open" <<'STUB'
#!/bin/sh
printf '%s\n' "$@" > /tmp/gust-fixture-OPENED
STUB
chmod 755 "$stub/xdg-open"
# Media activation now sends the visible playlist to mpv. Keep this handoff
# assertion isolated; media-navigation.py exercises the real player.
cat > "$stub/mpv" <<'STUB'
#!/usr/bin/env python3
import pathlib, sys, urllib.parse
start = int(next(arg.split("=", 1)[1] for arg in sys.argv if arg.startswith("--playlist-start=")))
paths = [line for line in sys.stdin.read().splitlines() if line.startswith("file://")]
pathlib.Path("/tmp/gust-fixture-OPENED").write_bytes(urllib.parse.unquote_to_bytes(paths[start][7:]) + b"\n")
STUB
chmod 755 "$stub/mpv"
rm -f /tmp/gust-fixture-OPENED
export QA_PATH_PREPEND="$stub"

if need magick; then
  magick -size 600x800 xc:white -fill black -pointsize 60 -draw "text 100,400 'Page one'" \
         -size 600x800 xc:white -fill black -pointsize 60 -draw "text 100,400 'Page two'" \
         -size 600x800 xc:white -fill black -pointsize 60 -draw "text 100,400 'Page three'" \
         "$pv/report.pdf" 2>/dev/null
fi
if need ffmpeg; then
  ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=330:duration=40" \
    -metadata title="Fixture tone" -metadata artist="Gust QA" "$pv/song.flac"
  ffmpeg -loglevel error -y -f lavfi -i "testsrc=size=640x360:rate=25:duration=3" \
    -pix_fmt yuv420p "$pv/clip.mp4"
fi

stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$pv" || bad "could not open $pv"
wait_for "d['model']['settled']" "True" 20 >/dev/null

say "33a · Markdown is rendered, and its HTML is shown, not obeyed"
typ "notes"; sleep 0.8; key space
wait_for "d['quickLookVisual']" "markdown" 15 >/dev/null
expect "Quick Look renders Markdown"             "d['quickLookVisual']" "markdown"
expect_contains "the heading is a heading"       "d['quickLookMarkdown']" "<h2>Release notes</h2>"
expect_contains "bold is bold"                   "d['quickLookMarkdown']" "<b>Markdown</b>"
expect_contains "the task list is a checkbox"    "d['quickLookMarkdown']" "☑ task lists"
expect "the file's own <img> stays text"         "'<img' not in d['quickLookMarkdown'] and '&lt;img' in d['quickLookMarkdown']" "True"
shot 33a-markdown
key Escape; sleep 0.6

say "33b · a PDF turns its pages"
if need magick && need pdftoppm && need pdfinfo; then
  typ "report"; sleep 0.8; key space
  wait_for "d['quickLookPage'][1]" "3" 30 >/dev/null
  expect "page 1 of 3"                           "d['quickLookPage']" "[1, 3]"
  key Right
  wait_for "d['quickLookPage'][0]" "2" 20 >/dev/null
  expect "Right turns to page 2"                 "d['quickLookPage']" "[2, 3]"
  key Next
  wait_for "d['quickLookPage'][0]" "3" 20 >/dev/null
  expect "Page Down turns to page 3"             "d['quickLookPage']" "[3, 3]"
  key Right; sleep 1
  expect "the last page stays the last"          "d['quickLookPage']" "[3, 3]"
  expect "the page is a sharp render"            "d['quickLookImage'] is not None and d['quickLookImage']['rendered'][1] >= 1000" "True"
  shot 33b-pdf-page3
  key Left
  wait_for "d['quickLookPage'][0]" "2" 20 >/dev/null
  expect "Left turns back"                       "d['quickLookPage']" "[2, 3]"
  key Escape; sleep 0.6
else
  bad "ImageMagick or poppler is missing: the PDF case did NOT run"
fi

say "33c · audio plays, pauses and skips, decoded in the sandbox"
if need ffmpeg && need bwrap && { need pw-play || need paplay; }; then
  typ "song"; sleep 0.8; key space
  wait_for "d['quickLookAudio'] != '' and __import__('json').loads(d['quickLookAudio'])['state']" "playing" 20 >/dev/null
  aud() { state "__import__('json').loads(d['quickLookAudio'])$1"; }
  expect "it is playing"                         "__import__('json').loads(d['quickLookAudio'])['state']" "playing"
  expect "the title comes from the file's tags"  "__import__('json').loads(d['quickLookAudio'])['title']" "Fixture tone"
  expect "the duration is known"                 "round(__import__('json').loads(d['quickLookAudio'])['duration'])" "40"
  # A real sound stream reaches the desktop's audio server.
  streams="$(vm pw-dump 2>/dev/null | grep -cE '"application.name": "(pw-play|paplay)"')"
  [[ "$streams" -ge 1 ]] && ok "the desktop's audio server is playing it ($streams stream)" \
    || bad "no audio stream reached PipeWire"
  # The decoder itself (comm "ffmpeg", not the bwrap launcher whose argv
  # also mentions it) runs in its own mount namespace with no audio socket.
  dpid=""
  for p in $(pgrep -x ffmpeg); do
    tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null | grep -q '/in/input' && { dpid="$p"; break; }
  done
  if [[ -n "$dpid" ]]; then
    [[ "$(sudo -n readlink "/proc/$dpid/ns/mnt")" != "$(readlink /proc/self/ns/mnt)" ]] \
      && ok "the decoder runs in its own sandbox (pid $dpid)" || bad "the decoder shares Gust's filesystem view"
    seen="$(sudo -n ls -A "/proc/$dpid/root/" 2>/dev/null | tr '\n' ' ')"
    echo "  decoder's / : $seen"
    if sudo -n test -e "/proc/$dpid/root/run"; then
      leak="$(sudo -n find "/proc/$dpid/root/run" \( -name 'pipewire-*' -o -name native \) 2>/dev/null | head -1)"
    else
      leak=""
    fi
    [[ -z "$leak" ]] && ok "the sandboxed decoder has no audio socket" || bad "audio socket visible in the sandbox: $leak"
    net="$(sudo -n cat "/proc/$dpid/net/dev" 2>/dev/null | grep -cvE 'Inter|face|lo:')"
    [[ "$net" == 0 ]] && ok "the decoder has no network" || bad "the decoder sees $net network interface(s)"
  else
    bad "no sandboxed decoder process found"
  fi
  shot 33c-audio-playing
  p0="$(aud "['pos']")"; sleep 1.5; p1="$(aud "['pos']")"
  python3 -c "import sys; d=float('$p1')-float('$p0'); sys.exit(0 if 1.0<d<2.2 else 1)" \
    && ok "it plays in real time (${p0}s -> ${p1}s in 1.5 s)" || bad "playback pace is off (${p0}s -> ${p1}s in 1.5 s)"
  key p; sleep 0.8
  expect "p pauses"                              "__import__('json').loads(d['quickLookAudio'])['state']" "paused"
  a="$(aud "['pos']")"; sleep 1.2; b="$(aud "['pos']")"
  [[ "$a" == "$b" ]] && ok "paused means paused ($a s)" || bad "moved while paused ($a -> $b)"
  key p; sleep 0.5
  vm wtype -M shift -k Right -m shift; sleep 0.8
  c="$(aud "['pos']")"
  python3 -c "import sys; sys.exit(0 if float('$c')-float('$b')>=4.0 else 1)" \
    && ok "Shift+Right skips five seconds ($b -> $c)" || bad "Shift+Right did not skip ($b -> $c)"
  # Clicking the position bar near its end jumps there.
  read -r WX WY WW WH <<<"$(win_geo)"
  if [[ "$MOUSE_OK" == 1 ]] && pt="$(state "' '.join(map(str, d['audioBarPoint']))" 2>/dev/null)" && [[ "$pt" != ERR* ]]; then
    read -r AX AY <<<"$pt"
    mouse click $((WX + AX)) $((WY + AY)); sleep 0.8
    e="$(aud "['pos']")"
    python3 -c "import sys; sys.exit(0 if float('$e')>30 else 1)" \
      && ok "clicking the bar near its end jumps there (${e}s)" || bad "the bar click did not seek (${e}s)"
  fi
  key Escape; sleep 1.2
  expect "closing Quick Look stops the sound"    "d['quickLookAudio']" ""
  left="$(vm pw-dump 2>/dev/null | grep -cE '"application.name": "(pw-play|paplay)"')"
  [[ "$left" == 0 ]] && ok "no stream is left playing" || bad "$left stream(s) still playing after close"
  [[ -z "$(pgrep -f 'ffmpeg.*/in/input')" ]] && ok "the decoder is gone" || bad "the decoder outlived Quick Look"
else
  bad "ffmpeg, bwrap or a PCM player is missing: the audio case did NOT run"
fi

say "33d · a damaged audio file says so, and plays nothing"
typ "broken"; sleep 0.8; key space
wait_for "d['quickLookAudio'] != '' and __import__('json').loads(d['quickLookAudio'])['state']" "failed" 20 >/dev/null
expect "it reports that it cannot play"          "__import__('json').loads(d['quickLookAudio'])['state']" "failed"
shot 33d-audio-broken
key Escape; sleep 0.6

say "33e · a video shows its frame; Enter hands it to the player"
if need ffmpegthumbnailer; then
  typ "clip"; sleep 0.8; key space
  wait_for "d['quickLookVisual']" "image" 30 >/dev/null
  expect "the frame is shown"                    "d['quickLookVisual']" "image"
  expect "it says Enter plays it"                "d['quickLookVideo']" "True"
  shot 33e-video
  key Return; sleep 1.2
  expect "Enter closed Quick Look"               "d['quickLookOpen']" "False"
  if [[ -f /tmp/gust-fixture-OPENED ]] && grep -q "clip.mp4" /tmp/gust-fixture-OPENED; then
    ok "Enter handed clip.mp4 to the media player"
  else
    bad "Enter did not hand the video to the opener"
  fi
else
  bad "ffmpegthumbnailer is missing: the video case did NOT run"
fi

unset QA_PATH_PREPEND
stop_gust
rm -rf "$pv" /tmp/gust-fixture-OPENED
start_gust "$FIXTURE"
