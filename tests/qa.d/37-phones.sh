# shellcheck shell=bash
# Phones (DESIGN §6.13.1). No phone is plugged into a test VM, so the system's
# side is replaced, and only it:
#  - sysfs and the udev database are a fake tree (GUST_SYSFS_USB, GUST_UDEV_DATA)
#    holding what the kernel writes for a real iPhone and a real Android phone;
#  - "plugging in" is a file appearing in the watched USB folder (GUST_USB_WATCH),
#    which is what /dev/bus/usb does when a device arrives;
#  - `gio` is a stand-in that speaks gio's own protocol: the iPhone asks the
#    same "Untrusted Device … [1] Try again [2] Cancel" question on stdin until
#    the test "taps Trust", says no when told to, and mounts a folder.
# Gust's code from detection to eject is the real code.

T="$FIXTURE/phones"; rm -rf "$T"
SYS="$T/sys"; UDEV="$T/udev"; MNTS="$T/gvfs-mounts"; USB="$T/usb"; BIN37="$T/bin"; M="$T/mnt"
mkdir -p "$SYS" "$UDEV" "$MNTS" "$USB/001" "$BIN37" "$M"
touch "$MNTS/afc.mount" "$MNTS/mtp.mount"
UDID=00008030-000A1B2C3D4E5F60
IPHONE="apple:$UDID"

cat > "$BIN37/gio" <<STUB
#!/usr/bin/env bash
# Stand-in for gio: phones only; anything else goes to the real one.
T="$T"; M="$M"
dir_of() {
  case "\$1" in
    afc://*:3/) h="\${1#afc://}"; h="\${h%:3/}"; echo "\$M/afc:host=\$h,port=3" ;;
    afc://*)    h="\${1#afc://}"; h="\${h%/}";   echo "\$M/afc:host=\$h" ;;
    mtp://*)    h="\${1#mtp://}"; h="\${h%/}";   echo "\$M/mtp:host=\$h" ;;
    *) return 1 ;;
  esac
}
args=("\$@")
case "\$1 \$2" in
  "mount -li")
    printf '%s\n' "Volume(0): iPhone" "  Type: GProxyVolume (GProxyVolumeMonitorGPhoto2)" \
      "  activation_root=gphoto2://Apple_Inc._iPhone_00008030000A1B2C3D4E5F60/" \
      "Volume(1): Documents on Alex's iPhone" "  Type: GProxyVolume (GProxyVolumeMonitorAfc)" \
      "  uuid=$UDID" "  activation_root=afc://$UDID:3/" \
      "Volume(2): Pixel 8" "  Type: GProxyVolume (GProxyVolumeMonitorMTP)" \
      "  activation_root=mtp://Google_Pixel_8_37281FDJH00A2B/"
    exit 0 ;;
  "mount --")
    uri="\$3"; d="\$(dir_of "\$uri")" || exec /usr/bin/gio "\${args[@]}"
    echo "\$uri" >> "\$T/gio-calls"
    if [[ "\$uri" == afc://* ]]; then
      if [[ -e "\$T/refuse" ]]; then
        echo "gio: \$uri: User refused to trust this computer" >&2; exit 2
      fi
      while [[ ! -e "\$T/trusted" ]]; do
        printf 'Untrusted Device\nThe device “iPhone” is not trusted yet.\n\nSelect “Trust” on the device and click “Try again”.\n[1] Try again\n[2] Cancel\nChoice: '
        read -r ans || exit 1
        echo "answer \$ans" >> "\$T/gio-calls"
        [[ "\$ans" == 2 ]] && { echo "gio: \$uri: Operation was cancelled" >&2; exit 1; }
      done
    fi
    mkdir -p "\$d"; touch "\$d.mounted"; exit 0 ;;
  "mount -u")
    uri="\$4"; d="\$(dir_of "\$uri")" || exec /usr/bin/gio "\${args[@]}"
    echo "unmount \$uri" >> "\$T/gio-calls"
    rm -f "\$d.mounted"; exit 0 ;;
  "info -a")
    uri="\$5"; d="\$(dir_of "\$uri")" || exec /usr/bin/gio "\${args[@]}"
    [[ -e "\$d.mounted" ]] || { echo "gio: \$uri: not mounted" >&2; exit 1; }
    printf 'name: /\nlocal path: %s\nuri: %s\n' "\$d" "\$uri"; exit 0 ;;
esac
exec /usr/bin/gio "\${args[@]}"
STUB
chmod 755 "$BIN37/gio"

# What the kernel writes for a device: sysfs attributes, and udev's database
# entry named after the char device (189:(bus-1)*128+dev-1).
plug() { # plug <sysname> <devnum> <vendor> <product> <serial> <udev props>
  local d="$SYS/$1"; mkdir -p "$d"
  printf '%s\n' "$3" > "$d/idVendor"; printf '%s\n' "$4" > "$d/product"
  printf '%s\n' "$5" > "$d/serial"; echo 1 > "$d/busnum"; echo "$2" > "$d/devnum"
  printf '%b' "$6" > "$UDEV/c189:$(( $2 - 1 ))"
  touch "$USB/001/$(printf '%03d' "$2")"
}
unplug() { rm -rf "$SYS/$1"; rm -f "$UDEV/c189:$(( $2 - 1 ))" "$USB/001/$(printf '%03d' "$2")"; }

# The phone's own files, as GVfs would show them once it is open.
mkdir -p "$M/afc:host=$UDID/DCIM/100APPLE" "$M/afc:host=$UDID,port=3/Pages"
printf 'jpeg' > "$M/afc:host=$UDID/DCIM/100APPLE/IMG_0001.JPG"
printf 'jpeg' > "$M/afc:host=$UDID/DCIM/100APPLE/IMG_0002.HEIC"
echo "draft" > "$M/afc:host=$UDID,port=3/Pages/notes.pages"
mkdir -p "$M/mtp:host=Google_Pixel_8_37281FDJH00A2B/Internal shared storage/DCIM/Camera"
printf 'jpeg' > "$M/mtp:host=Google_Pixel_8_37281FDJH00A2B/Internal shared storage/DCIM/Camera/PXL_1.jpg"

pt_of() { state "' '.join(map(str, d['$1'].get('''$2''', [])))"; }
wclick() { local x y; read -r x y <<<"$2"; read -r WX WY WW WH <<<"$(win_geo)"; mouse click $((WX + x)) $((WY + y)) "$1"; }
# A row that just appeared is first published before the sidebar has laid
# it out (the Devices header appears with it and pushes it down). Click only
# once the same position has been read twice, half a second apart.
pt_wait() {
  local p q="" i
  for i in $(seq 1 30); do
    p="$(pt_of "$1" "$2")"
    if [[ -n "$p" && "$p" != ERR* && "$p" == "$q" ]]; then echo "$p"; return 0; fi
    q="$p"; sleep 0.5
  done
  return 1
}
devs() { state "' ;; '.join(d['devices'])"; }
wait_dev() { # wait_dev <substring> <seconds>
  local i; for i in $(seq 1 $(( $2 * 4 ))); do [[ "$(devs)" == *"$1"* ]] && return 0; sleep 0.25; done; return 1; }
title() { vm hyprctl clients -j | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c.get('class')=='gust': print(c['title']); break"; }

export QA_PATH_PREPEND="$BIN37"
export QA_ENV="GUST_SYSFS_USB=$SYS GUST_UDEV_DATA=$UDEV GUST_GVFS_MOUNTS=$MNTS GUST_USB_WATCH=$USB"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null

say "37a · plugging in an iPhone shows it under Devices by its own name, contacting nothing"
[[ "$(devs)" != *iPhone* ]] && ok "no phone is listed before one is plugged in" || bad "a phone was listed already: $(devs)"
plug 1-1 5 05ac iPhone "${UDID//-/}" 'E:USBMUX_SUPPORTED=1\nE:ID_SERIAL=Apple_Inc._iPhone_00008030000A1B2C3D4E5F60\n'
if wait_dev "Alex's iPhone|Click to open" 6; then ok "it appears as \"Alex's iPhone\" — Click to open"; else bad "after plugging in, Devices shows: $(devs)"; fi
[[ ! -e "$T/gio-calls" ]] && ok "finding it mounted nothing and asked the phone nothing" || bad "gio was asked to mount on plug-in: $(cat "$T/gio-calls")"
shot 37a-iphone-plugged

if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "no pointer injection: the rest of section 37 did NOT run"
else
  say "37b · clicking it asks for Trust in words, and keeps asking until it is tapped"
  wclick left "$(pt_wait placePoints "device:$IPHONE")"
  if wait_dev "Unlock your iPhone and tap Trust" 8; then ok "the row says: Unlock your iPhone and tap Trust"; else bad "while untrusted the row says: $(devs)"; fi
  shot 37b-iphone-trust
  sleep 4
  [[ "$(grep -c '^answer 1' "$T/gio-calls" 2>/dev/null)" -ge 1 ]] && ok "Gust answered gio's question with Try again on its own" || bad "the question was never answered ($(cat "$T/gio-calls" 2>/dev/null))"
  touch "$T/trusted"   # the person taps Trust on the phone
  if wait_dev "Alex's iPhone|Open" 10; then ok "after Trust the row says it is open"; else bad "after Trust the row says: $(devs)"; fi
  wait_for "d['model']['settled']" "True" 10 >/dev/null
  expect "the window shows the iPhone's photos folder" "d['model']['location']" "$M/afc:host=$UDID"
  expect_contains "DCIM is listed" "' '.join(d['model']['visible'])" "DCIM"
  t="$(title)"; [[ "$t" == "Alex's iPhone — Gust" ]] && ok "the title names the phone, not its serial ($t)" || bad "the title is '$t'"
  goto "$M/afc:host=$UDID/DCIM/100APPLE"; wait_for "d['model']['settled']" "True" 10 >/dev/null
  t="$(title)"; [[ "$t" == "Alex's iPhone/DCIM/100APPLE — Gust" ]] && ok "deeper in, the path still starts at the phone ($t)" || bad "the title is '$t'"
  shot 37b-iphone-photos

  say "37c · a photo copies off the phone"
  typ "IMG_0001"; sleep 1.0; ctrl c; sleep 0.6
  goto "$FIXTURE/Documents"; wait_for "d['model']['settled']" "True" 10 >/dev/null
  ctrl v; sleep 2.0
  [[ -f "$FIXTURE/Documents/IMG_0001.JPG" && -f "$M/afc:host=$UDID/DCIM/100APPLE/IMG_0001.JPG" ]] \
    && ok "IMG_0001.JPG copied to Documents and stayed on the phone" || bad "the photo did not copy"
  rm -f "$FIXTURE/Documents/IMG_0001.JPG"

  say "37d · the right-click menu opens an iPhone's app files"
  wclick right "$(pt_wait placePoints "device:$IPHONE")"; sleep 0.6
  expect "the phone menu opened" "d['menuOpen']" "True"
  shot 37d-iphone-menu
  wclick left "$(pt_wait menuPoints "Open app files")"
  sleep 2.0; wait_for "d['model']['settled']" "True" 10 >/dev/null
  expect "the app files are shown" "d['model']['location']" "$M/afc:host=$UDID,port=3"
  t="$(title)"; [[ "$t" == "Alex's iPhone · app files — Gust" ]] && ok "titled: $t" || bad "the title is '$t'"

  say "37e · Eject leaves the phone and says it is safe to unplug"
  goto "$M/afc:host=$UDID/DCIM"; wait_for "d['model']['settled']" "True" 10 >/dev/null
  wclick right "$(pt_wait placePoints "device:$IPHONE")"; sleep 0.6
  wclick left "$(pt_wait menuPoints "Eject")"; sleep 2.0
  [[ "$(state "d['model']['location']")" != "$M/afc"* ]] && ok "the window left the phone before ejecting" || bad "the window is still on the phone"
  grep -q "^unmount afc://$UDID/" "$T/gio-calls" && ok "the phone was unmounted (never forced)" || bad "no unmount was asked for"
  expect_contains "it says it is safe to unplug" "d['hintStatus']" "Safe to unplug"
  if wait_dev "Alex's iPhone|Click to open" 5; then ok "the row is back to Click to open"; else bad "after eject: $(devs)"; fi
  shot 37e-iphone-ejected

  say "37f · waiting for Trust can be stopped; a phone that says no is told so"
  rm -f "$T/trusted" "$T/gio-calls"
  wclick left "$(pt_wait placePoints "device:$IPHONE")"
  wait_dev "tap Trust" 8 || bad "it did not ask for Trust again"
  wclick left "$(pt_wait placePoints "device:$IPHONE")"; sleep 4.5
  grep -q '^answer 2' "$T/gio-calls" && ok "a second click answered Cancel" || bad "Cancel was never sent ($(cat "$T/gio-calls"))"
  if wait_dev "Alex's iPhone|Click to open" 5; then ok "and the row went back to Click to open"; else bad "after stopping: $(devs)"; fi
  touch "$T/refuse"
  wclick left "$(pt_wait placePoints "device:$IPHONE")"
  if wait_dev "This iPhone said no" 6; then ok "Don't Trust is told plainly: This iPhone said no…"; else bad "after Don't Trust: $(devs)"; fi
  shot 37f-iphone-said-no
  rm -f "$T/refuse"
fi

say "37g · an Android phone in File transfer mode shows and opens"
plug 1-2 7 18d1 "Pixel 8" 37281FDJH00A2B 'E:ID_MTP_DEVICE=1\nE:ID_MEDIA_PLAYER=1\nE:ID_SERIAL=Google_Pixel_8_37281FDJH00A2B\n'
if wait_dev "Pixel 8|Click to open" 6; then ok "Pixel 8 appears"; else bad "Devices shows: $(devs)"; fi
if [[ "$MOUSE_OK" -eq 1 ]]; then
  wclick left "$(pt_wait placePoints "device:android:Google_Pixel_8_37281FDJH00A2B")"; sleep 2.5
  wait_for "d['model']['settled']" "True" 10 >/dev/null
  expect_contains "its storage is listed" "' '.join(d['model']['visible'])" "Internal shared storage"
  t="$(title)"; [[ "$t" == "Pixel 8 — Gust" ]] && ok "titled: $t" || bad "the title is '$t'"
  shot 37g-android-open
fi

say "37h · unplugging removes it; a missing backend is named, with the command"
unplug 1-2 7
if wait_dev "Pixel 8" 1; then sleep 3; fi
[[ "$(devs)" != *"Pixel 8"* ]] && ok "the Pixel is gone from Devices once unplugged" || bad "still listed: $(devs)"
rm -f "$MNTS/afc.mount"; touch "$USB/001/poke"
if wait_dev "Needs gvfs-afc — sudo pacman -S gvfs-afc" 6; then ok "without iPhone support it says: Needs gvfs-afc — sudo pacman -S gvfs-afc"; else bad "without gvfs-afc: $(devs)"; fi
shot 37h-needs-package

unset QA_PATH_PREPEND QA_ENV
stop_gust
rm -rf "$T"
start_gust "$FIXTURE"
