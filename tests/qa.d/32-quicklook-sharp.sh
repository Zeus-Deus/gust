# shellcheck shell=bash
# Quick Look shows an image sharply: rendered large enough for the panel, and
# never drawn bigger than it was rendered (DESIGN §6.9). It used to show the
# 256px thumbnail stretched across the panel, which was visibly blurry.

say "32 · Quick Look images are sharp"
if ! command -v magick >/dev/null 2>&1; then
  bad "ImageMagick is not installed: the Quick Look image case did NOT run"
else
  ql="$FIXTURE/qlsharp"; rm -rf "$ql"; mkdir -p "$ql"
  magick -size 2400x1600 gradient:navy-orange -fill white -pointsize 90 \
    -draw "text 200,800 'Sharp text at 2400 px'" "$ql/big.png" 2>/dev/null
  magick -size 348x397 xc:gray20 -fill white -pointsize 22 \
    -draw "text 20,200 'small screenshot'" "$ql/small.png" 2>/dev/null
  stop_gust; start_gust "$FIXTURE"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  goto "$ql" || bad "could not open $ql"
  wait_for "d['model']['count']" "2" 10 >/dev/null
  for name in big small; do
    typ "$name"; sleep 1.2; key space
    wait_for "d['quickLookImage'] is not None" "True" 40 >/dev/null
    info="$(state "d['quickLookImage']")"
    rw="$(state "d['quickLookImage']['rendered'][0]")"
    dw="$(state "d['quickLookImage']['drawn'][0]")"
    echo "  $name: $info"
    [[ "$rw" =~ ^[0-9]+$ && "$dw" =~ ^[0-9]+$ ]] || { bad "$name: no preview image ($info)"; key Escape; sleep 0.6; continue; }
    [[ "$dw" -le "$rw" ]] && ok "$name: drawn at or below its rendered size (${dw} <= ${rw}px), never stretched" \
      || bad "$name: stretched past its pixels (${dw} > ${rw}px): blurry"
    if [[ "$name" == big ]]; then
      [[ "$rw" -ge 1000 ]] && ok "big: rendered at ${rw}px, enough for the panel" \
        || bad "big: rendered at only ${rw}px"
      shot 32-quicklook-big
    else
      [[ "$rw" -eq 348 ]] && ok "small: kept its own 348px (not enlarged)" \
        || bad "small: rendered at ${rw}px, not its own 348px"
      shot 32-quicklook-small
    fi
    key Escape; sleep 0.8
  done
  rm -rf "$ql"
fi
