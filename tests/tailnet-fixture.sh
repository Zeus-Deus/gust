#!/usr/bin/env bash
# A private tailnet for testing Tailscale places, built in a disposable VM.
#
#   headscale (the open-source Tailscale control server) on the VM, the
#   VM's own tailscaled joined to it, and two containers ("nas", "studio")
#   each running tailscaled + sshd, joined as peers. No Tailscale account,
#   no real devices, nothing outside this machine.
#
# Containers: never privileged, every capability dropped except the six
# sshd needs to drop privileges, no-new-privileges, no host mounts, no
# devices (FOWNER: adduser sets up a home it does not own yet; SYS_CHROOT: sshd isolates each login). tailscaled runs in userspace-networking mode, so it needs no
# TUN device and no NET_ADMIN.
#
# THIS RECONFIGURES THE MACHINE'S TAILSCALE. Run it only in a throwaway VM.
#
#   tests/tailnet-fixture.sh start | stop | status
set -uo pipefail

FIX="${GUST_TAILNET_FIXTURE:-/var/tmp/gust-tailnet}"
HS_ADDR="${GUST_HEADSCALE_ADDR:-172.17.0.1:8080}"   # docker0: reachable from containers
DOMAIN="tail.gust.test"
IMAGE="${GUST_TAILNET_IMAGE:-docker.io/library/alpine:3.22}"
PEERS=(nas studio)
D="sudo -n docker"

need() { command -v "$1" >/dev/null 2>&1 || { echo "needs $1"; exit 1; }; }

start() {
  need headscale; need tailscale; need docker
  [[ "$(hostname)" == *vm* || -n "${GUST_TAILNET_FORCE:-}" ]] \
    || { echo "refusing: this reconfigures Tailscale; run it in a disposable VM"; return 1; }
  # Always a fresh tailnet: a rerun against the old database leaves the
  # previous containers registered, and new ones come up as "nas-1".
  for p in "${PEERS[@]}"; do $D rm -f "gust-tailnet-$p" >/dev/null 2>&1; done
  sudo -n pkill -f "headscale -c $FIX" >/dev/null 2>&1; sleep 0.5
  sudo -n rm -rf "$FIX/headscale"
  sudo -n mkdir -p "$FIX/headscale"
  sudo -n chown "$(id -u):$(id -g)" "$FIX"
  chmod 755 "$FIX"

  # --- control server -------------------------------------------------
  sudo -n tee "$FIX/headscale/config.yaml" >/dev/null <<EOF
server_url: http://$HS_ADDR
listen_addr: $HS_ADDR
metrics_listen_addr: 127.0.0.1:9099
grpc_listen_addr: 127.0.0.1:50499
grpc_allow_insecure: false
noise:
  private_key_path: $FIX/headscale/noise_private.key
prefixes:
  v4: 100.64.0.0/10
  v6: fd7a:115c:a1e0::/48
  allocation: sequential
derp:
  server:
    enabled: true
    region_id: 999
    region_code: gust
    region_name: gust fixture
    stun_listen_addr: "${HS_ADDR%:*}:3479"
    private_key_path: $FIX/headscale/derp_server_private.key
    automatically_add_embedded_derp_region: true
    ipv4: ${HS_ADDR%:*}
  urls: []
  paths: []
  auto_update_enabled: false
database:
  type: sqlite
  sqlite:
    path: $FIX/headscale/db.sqlite
log:
  level: warn
policy:
  mode: file
  path: ""
dns:
  magic_dns: true
  base_domain: $DOMAIN
  override_local_dns: false
  nameservers:
    global: []
unix_socket: $FIX/headscale/headscale.sock
unix_socket_permission: "0770"
EOF
  # Omarchy's ufw drops everything coming in from docker0. Open exactly the
  # control server and its relay to the containers, on that interface only;
  # stop removes both rules.
  if command -v ufw >/dev/null 2>&1; then
    sudo -n ufw allow in on docker0 to "${HS_ADDR%:*}" port "${HS_ADDR##*:}" proto tcp >/dev/null
    sudo -n ufw allow in on docker0 to "${HS_ADDR%:*}" port 3479 proto udp >/dev/null
  fi
  HS="sudo -n headscale -c $FIX/headscale/config.yaml"
  if ! pgrep -f "headscale -c $FIX" >/dev/null; then
    sudo -n sh -c "nohup headscale -c $FIX/headscale/config.yaml serve > $FIX/headscale/log 2>&1 &"
  fi
  for _ in $(seq 1 60); do
    curl -fsS -m 2 "http://$HS_ADDR/health" >/dev/null 2>&1 && break
    sleep 0.5
  done
  curl -fsS -m 2 "http://$HS_ADDR/health" >/dev/null 2>&1 \
    || { echo "headscale did not come up:"; sudo -n tail -20 "$FIX/headscale/log"; return 1; }

  $HS users list -o json 2>/dev/null | grep -q '"name": *"gust"' || $HS users create gust >/dev/null
  uid="$($HS users list -o json | python3 -c 'import json,sys; print(next(u["id"] for u in json.load(sys.stdin) if u["name"]=="gust"))')"
  KEY="$($HS preauthkeys create -u "$uid" --reusable -e 2h -o json | python3 -c 'import json,sys; print(json.load(sys.stdin)["key"])')"
  [[ -n "$KEY" ]] || { echo "no auth key"; return 1; }
  # The key goes to tailscale through a FILE, never argv.
  umask 077; printf '%s' "$KEY" > "$FIX/authkey"; umask 022
  sudo -n chmod 644 "$FIX/authkey"   # read by the containers' root

  # Client key for the peers' sshd: the "user's own SSH key" of the test.
  [[ -f "$FIX/id_ed25519" ]] || ssh-keygen -q -t ed25519 -N '' -C gust-tailnet -f "$FIX/id_ed25519"
  # ssh finds a user's key through the passwd home, not $HOME, so a test
  # with HOME pointed at a fixture still uses ~/.ssh of the account. In
  # this disposable VM that account has no key: install the fixture's as
  # its default key (never over an existing one; stop removes only ours).
  real_ssh="$(getent passwd "$(id -un)" | cut -d: -f6)/.ssh"
  mkdir -p "$real_ssh"; chmod 700 "$real_ssh"
  if [[ ! -e "$real_ssh/id_ed25519" ]]; then
    cp "$FIX/id_ed25519" "$real_ssh/id_ed25519"; cp "$FIX/id_ed25519.pub" "$real_ssh/id_ed25519.pub"
    chmod 600 "$real_ssh/id_ed25519"
  fi

  # --- this machine joins ----------------------------------------------
  sudo -n systemctl start tailscaled
  sudo -n tailscale up --reset --login-server "http://$HS_ADDR" \
      --auth-key "file:$FIX/authkey" --hostname gust-desk --accept-dns=true --timeout 60s \
      || { echo "this machine could not join"; return 1; }

  # --- peers -------------------------------------------------------------
  for p in "${PEERS[@]}"; do
    name="gust-tailnet-$p"
    $D rm -f "$name" >/dev/null 2>&1
    $D run -d --name "$name" --hostname "$p" \
      --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add DAC_OVERRIDE --cap-add FOWNER --cap-add SYS_CHROOT \
      --security-opt no-new-privileges \
      "$IMAGE" sh -c '
        set -e
        apk add --no-cache tailscale openssh openssh-sftp-server >/dev/null
        ssh-keygen -A
        adduser -D -s /bin/sh media
        passwd -u media >/dev/null 2>&1 || true
        mkdir -p /home/media/.ssh /home/media/Shared/photos
        echo "hello from $(hostname)"      > /home/media/Shared/hello.txt
        echo "notes kept on $(hostname)"   > /home/media/Shared/notes.md
        echo "a photo, really"             > /home/media/Shared/photos/beach.jpg
        echo "CANARY-ROOT-ONLY"            > /root/canary.txt
        chmod 700 /root
        while [ ! -s /home/media/.ssh/authorized_keys ]; do sleep 0.3; done
        chown -R media:media /home/media
        chmod 700 /home/media/.ssh; chmod 600 /home/media/.ssh/authorized_keys
        /usr/sbin/sshd -e
        mkdir -p /var/lib/tailscale /run/tailscale
        tailscaled --tun=userspace-networking --state=/var/lib/tailscale/tailscaled.state \
                   --socket=/run/tailscale/tailscaled.sock >/var/log/tailscaled.log 2>&1 &
        while [ ! -S /run/tailscale/tailscaled.sock ]; do sleep 0.3; done
        while [ ! -s /authkey ]; do sleep 0.3; done
        tailscale up --login-server "'"http://$HS_ADDR"'" --auth-key file:/authkey \
                     --hostname "$(hostname)" --timeout 60s
        rm -f /authkey
        exec tail -f /dev/null
      ' >/dev/null || { echo "could not start $p"; return 1; }
    for _ in $(seq 1 120); do
      $D exec "$name" test -d /home/media/.ssh 2>/dev/null && break; sleep 0.5
    done
    $D exec -i "$name" sh -c 'cat > /home/media/.ssh/authorized_keys' < "$FIX/id_ed25519.pub"
    $D exec -i "$name" sh -c 'cat > /authkey' < "$FIX/authkey"
  done

  # Wait until every peer answers on the tailnet AND its sshd is up:
  # "Online" in the status comes several seconds before either.
  for p in "${PEERS[@]}"; do
    for _ in $(seq 1 90); do
      tailscale ping -c 1 --timeout 3s "$p" >/dev/null 2>&1 \
        && (exec 3<>"/dev/tcp/$p.$DOMAIN/22") 2>/dev/null && break
      sleep 1
    done
  done
  status
}

stop() {
  for p in "${PEERS[@]}"; do $D rm -f "gust-tailnet-$p" >/dev/null 2>&1; done
  sudo -n tailscale down >/dev/null 2>&1
  sudo -n tailscale logout >/dev/null 2>&1
  sudo -n systemctl stop tailscaled >/dev/null 2>&1
  sudo -n pkill -f "headscale -c $FIX" >/dev/null 2>&1
  if command -v ufw >/dev/null 2>&1; then
    sudo -n ufw delete allow in on docker0 to "${HS_ADDR%:*}" port "${HS_ADDR##*:}" proto tcp >/dev/null 2>&1
    sudo -n ufw delete allow in on docker0 to "${HS_ADDR%:*}" port 3479 proto udp >/dev/null 2>&1
  fi
  real_ssh="$(getent passwd "$(id -un)" | cut -d: -f6)/.ssh"
  if [[ -f "$FIX/id_ed25519.pub" ]] && cmp -s "$FIX/id_ed25519.pub" "$real_ssh/id_ed25519.pub"; then
    rm -f "$real_ssh/id_ed25519" "$real_ssh/id_ed25519.pub"
  fi
  echo "tailnet stopped"
}

status() {
  tailscale status 2>&1 | head -10
  local ok=1
  for p in "${PEERS[@]}"; do
    if tailscale ping -c 1 --timeout 5s "$p" >/dev/null 2>&1; then echo "$p: answers on the tailnet"
    else echo "$p: NOT reachable"; ok=0; fi
  done
  [[ $ok == 1 ]]
}

case "${1:-status}" in
  start) start ;;
  stop) stop ;;
  status) status ;;
  *) echo "usage: $0 start|stop|status"; exit 2 ;;
esac
