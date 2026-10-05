# shellcheck shell=bash
# Connect to other computers and cloud storage, end to end (DESIGN §6.4.1).
#
# Needs tests/tailnet-fixture.sh running (a private tailnet: headscale, this
# VM, and two containers "nas" and "studio" with sshd). The rclone cloud is
# built here from `rclone serve webdav`. Everything is driven through the
# real window with a real pointer: the sidebar's Connect… button, the panel's
# buttons, the host-key dialog, drags to and from the remote, the sidebar's
# Disconnect.

pt_of() { state "' '.join(map(str, d['$1'].get('''$2''', [])))"; }
wclick() { local x y; read -r x y <<<"$2"; read -r WX WY WW WH <<<"$(win_geo)"; mouse click $((WX + x)) $((WY + y)) "$1"; }
mounted() { grep -q " $1 " /proc/self/mountinfo; }
peer() { sudo -n docker exec "gust-tailnet-$1" "${@:2}"; }

RUN="/run/user/$(id -u)/gust"
NAS="$RUN/tailnet/nas"
BOX="$RUN/places/box"
CLOUD="$RUN/cloud/gustcloud"
for m in "$NAS" "$RUN/tailnet/studio" "$BOX" "$CLOUD"; do
  mounted "$m" && { fusermount3 -u "$m" 2>/dev/null || fusermount -u "$m" 2>/dev/null; }
done

if ! tailscale status >/dev/null 2>&1 || ! sudo -n docker inspect gust-tailnet-nas >/dev/null 2>&1; then
  bad "the tailnet fixture is not running (tests/tailnet-fixture.sh start): section 35 did NOT run"
  return 0 2>/dev/null || exit 0
fi
[[ "$MOUSE_OK" == 1 ]] || { bad "no pointer injection: section 35 did NOT run"; return 0 2>/dev/null || exit 0; }

# --- fixture: an SSH-config computer and an rclone remote ------------------
TF="${GUST_TAILNET_FIXTURE:-/var/tmp/gust-tailnet}"
KEY="$(getent passwd "$(id -un)" | cut -d: -f6)/.ssh/id_ed25519"
rm -rf "$FIXTURE/.ssh" "$FIXTURE/.local/state/gust/tailnet-users"
mkdir -p "$FIXTURE/.ssh"; chmod 700 "$FIXTURE/.ssh"
cat > "$FIXTURE/.ssh/config" <<EOF
Host box
    HostName studio.tail.gust.test
    User media
    IdentityFile $KEY
    UserKnownHostsFile $FIXTURE/.ssh/box_known_hosts
EOF
chmod 600 "$FIXTURE/.ssh/config"
# A computer the user already trusts: its key is on record beforehand.
ssh-keyscan -T 5 studio.tail.gust.test 2>/dev/null > "$FIXTURE/.ssh/box_known_hosts"

CL="/var/tmp/gust-qa-cloud"; rm -rf "$CL"; mkdir -p "$CL/backend/albums"
echo "stored in the cloud" > "$CL/backend/cloud-hello.txt"
mkdir -p "$FIXTURE/.config/rclone"
printf '[gustcloud]\ntype = webdav\nurl = http://127.0.0.1:8199\nvendor = other\n' > "$FIXTURE/.config/rclone/rclone.conf"
chmod 600 "$FIXTURE/.config/rclone/rclone.conf"
rclone serve webdav "$CL/backend" --addr 127.0.0.1:8199 >"$CL/serve.log" 2>&1 &
CLPID=$!
for _ in $(seq 1 40); do (exec 3<>/dev/tcp/127.0.0.1/8199) 2>/dev/null && break; sleep 0.25; done

LOCAL="$FIXTURE/send"; rm -rf "$LOCAL"; mkdir -p "$LOCAL"; echo "made on the desk" > "$LOCAL/from-desk.txt"
rm -f "$FIXTURE/Documents/hello.txt" "$FIXTURE/Documents/cloud-hello.txt"
peer nas rm -f /home/media/from-desk.txt

stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null

say "35a · the sidebar's Connect… lists every device, computer and cloud, and connects nothing"
wait_for "len(d['places'].get('tailnet', []))" "2" 20 >/dev/null
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
expect "Connect… opened the Connect panel"           "d['connectOpen']" "True"
expect "Tailscale devices are listed by name"        "sorted(p['name'] for p in d['places']['tailnet'])" "['nas', 'studio']"
expect "with their MagicDNS names"                   "sorted(p['detail'] for p in d['places']['tailnet'])" "['nas.tail.gust.test', 'studio.tail.gust.test']"
expect "the SSH config's computer is listed"         "[p['name'] for p in d['places']['ssh']]" "['box']"
expect "the rclone remote is listed"                 "[p['name'] for p in d['places']['cloud']]" "['gustcloud']"
n="$(grep -c " $RUN/" /proc/self/mountinfo || true)"
[[ "$n" == 0 ]] && ok "opening the list connected to nothing" || bad "$n place(s) mounted by just looking"
shot 35a-connect-panel

say "35b · a Tailscale device: first connection shows its key; Cancel trusts and mounts nothing"
wclick left "$(pt_of connectPoints tailnet:nas)"
wait_for "d['promptOpen']" "True" 10 >/dev/null
expect_contains "it asks which login to use"         "d['promptTitle']" "nas"
ctrl a; typ media; key Return
wait_for "d['confirmOpen']" "True" 30 >/dev/null
expect_contains "a first connection asks about the key" "d['confirmTitle']" "First connection to nas"
fp="$(ssh-keyscan -T 5 -t ed25519 nas.tail.gust.test 2>/dev/null | ssh-keygen -lf - | awk '{print $2}')"
expect_contains "it shows the device's real fingerprint" "d['confirmBody']" "$fp"
shot 35b-host-key
wclick left "$(state "' '.join(map(str, d['confirmButtons']['cancel']))")"; sleep 1
mounted "$NAS" && bad "Cancel still connected" || ok "Cancel connected nothing"
[[ ! -s "$FIXTURE/.ssh/known_hosts" ]] && ok "and recorded no key" || bad "Cancel wrote a key: $(cat "$FIXTURE/.ssh/known_hosts")"

say "35c · Trust and connect: the device opens like a folder"
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
wclick left "$(pt_of connectPoints tailnet:nas)"
wait_for "d['promptOpen']" "True" 10 >/dev/null
ctrl a; typ media; key Return
wait_for "d['confirmOpen']" "True" 30 >/dev/null
wclick left "$(state "' '.join(map(str, d['confirmButtons']['confirm']))")"
wait_for "d['model']['location']" "$NAS" 60 >/dev/null
expect "connecting browsed nas"                      "d['model']['location']" "$NAS"
mounted "$NAS" && ok "a real filesystem is mounted" || bad "nothing mounted at $NAS"
grep -q "nas.tail.gust.test" "$FIXTURE/.ssh/known_hosts" && ok "the trusted key was recorded" || bad "no key recorded"
expect "the sidebar lists nas as connected"          "d['connectedPlaces']" "['nas']"
expect "no alert"                                    "d['alertText']" ""
goto "$NAS/Shared"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "the remote files are listed"                 "sorted(d['rowPoints'].keys())" "['hello.txt', 'notes.md', 'photos']"
shot 35c-browsing-nas

say "35d · drag from the device into a local folder, and from a local folder onto the device"
read -r WX WY WW WH <<<"$(win_geo)"
read -r ax ay <<<"$(pt_of rowPoints hello.txt)"; read -r bx by <<<"$(pt_of placePoints Documents)"
( mouse drag $((WX + ax)) $((WY + ay)) $((WX + bx)) $((WY + by)) 2.5 ) & mp=$!
sleep 2.2; held="$(state "d['hintStatus']")"; shot 35d-drag-from-nas; wait "$mp"; sleep 3
[[ "$held" == "Copy 1 item to Documents" ]] && ok "over Documents it said: $held" || bad "over Documents it said '$held'"
[[ "$(cat "$FIXTURE/Documents/hello.txt" 2>/dev/null)" == "hello from nas" ]] \
  && ok "hello.txt came down from nas with its contents" || bad "hello.txt did not arrive"
[[ "$(peer nas cat /home/media/Shared/hello.txt 2>/dev/null)" == "hello from nas" ]] \
  && ok "and nas still has it (another drive: copy)" || bad "the original on nas is gone"

goto "$LOCAL"
wait_for "d['model']['settled']" "True" 20 >/dev/null
read -r ax ay <<<"$(pt_of rowPoints from-desk.txt)"; read -r bx by <<<"$(pt_of placePoints remote:nas)"
( mouse drag $((WX + ax)) $((WY + ay)) $((WX + bx)) $((WY + by)) 2.5 ) & mp=$!
sleep 2.2; held="$(state "d['hintStatus']")"; shot 35d-drag-onto-nas; wait "$mp"; sleep 3
[[ "$held" == "Copy 1 item to nas" ]] && ok "over nas in the sidebar it said: $held" || bad "over nas it said '$held'"
[[ "$(peer nas cat /home/media/from-desk.txt 2>/dev/null)" == "made on the desk" ]] \
  && ok "from-desk.txt landed on the device itself (checked inside nas)" || bad "nothing reached nas"
[[ -f "$LOCAL/from-desk.txt" ]] && ok "the local original stayed" || bad "the local original is gone"
expect "the job finished and cleared"               "d['jobRunning']" "False"

say "35e · disconnect from the sidebar"
goto "$NAS"
wait_for "d['model']['settled']" "True" 20 >/dev/null
wclick right "$(pt_of placePoints remote:nas)"
wait_for "d['menuOpen']" "True" 10 >/dev/null
expect_contains "right-click offers Disconnect"      "' | '.join(d['menuItems'])" "Disconnect"
wclick left "$(pt_of menuPoints Disconnect)"
for _ in $(seq 1 40); do mounted "$NAS" || break; sleep 0.3; done
mounted "$NAS" && bad "still mounted after Disconnect" || ok "Disconnect unmounted nas"
wait_for "d['connectedPlaces']" "[]" 20 >/dev/null
expect "the sidebar no longer lists it"              "d['connectedPlaces']" "[]"
expect_contains "Gust left the folder it was showing" "d['model']['location']" "$FIXTURE"

say "35f · a computer from the SSH config, from the panel"
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
wclick left "$(pt_of connectPoints ssh:box)"
wait_for "d['model']['location']" "$BOX" 60 >/dev/null
expect "Connect opened box"                          "d['model']['location']" "$BOX"
[[ "$(cat "$BOX/Shared/hello.txt" 2>/dev/null)" == "hello from studio" ]] \
  && ok "box is studio, over the tailnet, with the config's key" || bad "box shows the wrong machine"
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
expect "the panel shows box as connected"            "next(p['connected'] for p in d['places']['ssh'] if p['name']=='box')" "True"
shot 35f-panel-connected
wclick left "$(pt_of connectPoints ssh:box:off)"
for _ in $(seq 1 40); do mounted "$BOX" || break; sleep 0.3; done
mounted "$BOX" && bad "box still mounted" || ok "the panel's Disconnect unmounted box"
key Escape; sleep 0.5

say "35g · cloud storage through rclone"
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
wclick left "$(pt_of connectPoints cloud:gustcloud)"
wait_for "d['model']['location']" "$CLOUD" 60 >/dev/null
expect "Connect opened the cloud remote"             "d['model']['location']" "$CLOUD"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "its files are listed"                        "sorted(d['rowPoints'].keys())" "['albums', 'cloud-hello.txt']"
read -r WX WY WW WH <<<"$(win_geo)"
read -r ax ay <<<"$(pt_of rowPoints cloud-hello.txt)"; read -r bx by <<<"$(pt_of placePoints Documents)"
mouse drag $((WX + ax)) $((WY + ay)) $((WX + bx)) $((WY + by)); sleep 3
[[ "$(cat "$FIXTURE/Documents/cloud-hello.txt" 2>/dev/null)" == "stored in the cloud" ]] \
  && ok "a file dragged out of the cloud arrived" || bad "nothing came down from the cloud"
[[ -f "$CL/backend/cloud-hello.txt" ]] && ok "and the cloud kept it" || bad "the cloud lost it"
shot 35g-cloud
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
wclick left "$(pt_of connectPoints cloud:gustcloud:off)"
for _ in $(seq 1 60); do mounted "$CLOUD" || break; sleep 0.3; done
mounted "$CLOUD" && bad "the cloud is still mounted" || ok "Disconnect unmounted the cloud"
key Escape; sleep 0.5

say "35h · Tailscale stopped: the panel says so instead of an empty list"
stub="$FIXTURE/.tsbin"; mkdir -p "$stub"
printf '#!/bin/sh\necho %s\n' "'{\"BackendState\":\"Stopped\"}'" > "$stub/tailscale"; chmod 755 "$stub/tailscale"
export QA_PATH_PREPEND="$stub"
stop_gust; start_gust "$FIXTURE"
wait_for "d['places'].get('tailnetError','')" "Tailscale is not connected (Stopped) — run tailscale up" 20 >/dev/null
wclick left "$(pt_of placePoints connect)"
wait_for "d['connectOpen']" "True" 10 >/dev/null
expect "it explains that Tailscale is off"           "d['places']['tailnetError']" "Tailscale is not connected (Stopped) — run tailscale up"
shot 35h-tailscale-stopped
key Escape
unset QA_PATH_PREPEND

stop_gust
kill "$CLPID" 2>/dev/null
for m in "$NAS" "$BOX" "$CLOUD"; do mounted "$m" && fusermount3 -u "$m" 2>/dev/null; done
peer nas rm -f /home/media/from-desk.txt
rm -rf "$CL" "$LOCAL" "$stub" "$FIXTURE/.ssh" "$FIXTURE/.config/rclone" "$FIXTURE/Documents/hello.txt" "$FIXTURE/Documents/cloud-hello.txt"
start_gust "$FIXTURE"
