#!/usr/bin/env bash
# Isolated backend peers in Docker, acting as genuinely OTHER machines.
#
# Why containers and not the local sshd fixture: `internal-sftp -d <dir>`
# only selects a starting directory, it is NOT confinement (absolute paths,
# `..` and symlinks can leave it). A container has a real filesystem
# boundary, so a canary placed outside the share genuinely cannot be reached,
# and the peer's storage is independent of the client's mount and cache —
# which is what makes a receipt check meaningful.
#
# Hardening (never --privileged, never the docker socket, never $HOME):
#   --cap-drop ALL plus only the capabilities sshd actually needs
#   --security-opt no-new-privileges
#   loopback-only published ports
#   named volumes for backend data, no host bind mounts
#
#   tests/docker-peers.sh start | stop | status | ssh-args
set -uo pipefail

PREFIX="${GUST_PEER_PREFIX:-gust-peer}"
NET="${PREFIX}-net"
SSH_PORT="${GUST_PEER_SSH_PORT:-2230}"
DAV_PORT="${GUST_PEER_DAV_PORT:-2231}"
FIX="${GUST_PEER_FIXTURE:-/tmp/gust-docker-peers}"
IMAGE_SSH="${GUST_PEER_IMAGE:-docker.io/library/alpine:3.20}"

# sudo -n keeps this working without a group re-login inside the VM.
D="sudo -n docker"

have_docker() { $D info >/dev/null 2>&1; }

start() {
  have_docker || { echo "docker not available"; return 1; }

  mkdir -p "$FIX"
  chmod 700 "$FIX"

  # Client key pair, fixture-owned. Never the user's ~/.ssh.
  if [[ ! -f "$FIX/id_peer" ]]; then
    ssh-keygen -q -t ed25519 -f "$FIX/id_peer" -N '' -C gust-docker-peer
  fi

  $D network inspect "$NET" >/dev/null 2>&1 || \
    $D network create --internal=false "$NET" >/dev/null

  # ---- SSH/SFTP peer -----------------------------------------------------
  if ! $D inspect "${PREFIX}-ssh" >/dev/null 2>&1; then
    # The share lives in a named volume owned by the peer, so the client's
    # mount/cache cannot be mistaken for the backend's own bytes.
    $D volume create "${PREFIX}-data" >/dev/null

    # sshd needs CHOWN/SETUID/SETGID/DAC_OVERRIDE to drop privileges, and
    # SYS_CHROOT for the sftp chroot. Everything else is dropped.
    $D run -d --name "${PREFIX}-ssh" \
      --network "$NET" \
      -p "127.0.0.1:${SSH_PORT}:22" \
      --cap-drop ALL \
      --cap-add CHOWN --cap-add SETUID --cap-add SETGID \
      --cap-add DAC_OVERRIDE --cap-add SYS_CHROOT \
      --security-opt no-new-privileges \
      -v "${PREFIX}-data:/srv/share" \
      "$IMAGE_SSH" sh -c '
        set -e
        apk add --no-cache openssh openssh-sftp-server >/dev/null
        ssh-keygen -A
        adduser -D -s /bin/sh peer
        passwd -u peer 2>/dev/null || true
        mkdir -p /home/peer/.ssh && chmod 700 /home/peer/.ssh
        # Fixture content, plus a canary OUTSIDE the share that a correct
        # client can never read.
        mkdir -p /srv/share/subdir
        echo "peer fixture ready"      > /srv/share/hello.txt
        echo "backend-owned bytes"     > /srv/share/notes.md
        echo nested                    > /srv/share/subdir/deep.txt
        echo "CANARY-MUST-NOT-BE-READ" > /root/canary-outside-share.txt
        chmod 700 /root
        chown -R peer:peer /srv/share
        while [ ! -s /home/peer/.ssh/authorized_keys ]; do sleep 0.3; done
        chown -R peer:peer /home/peer/.ssh
        exec /usr/sbin/sshd -D -e
      ' >/dev/null

    # Install the client key without it ever passing through argv.
    for _ in $(seq 1 60); do
      $D exec "${PREFIX}-ssh" test -d /home/peer/.ssh 2>/dev/null && break
      sleep 0.5
    done
    $D exec -i "${PREFIX}-ssh" sh -c 'cat > /home/peer/.ssh/authorized_keys' \
        < "$FIX/id_peer.pub"
    $D exec "${PREFIX}-ssh" chmod 600 /home/peer/.ssh/authorized_keys
    $D exec "${PREFIX}-ssh" chown peer:peer /home/peer/.ssh/authorized_keys
  fi

  # Wait for the port, then pin the host key: a test must fail on a CHANGED
  # key, which is impossible if the client is told not to check.
  for _ in $(seq 1 60); do
    (exec 3<>/dev/tcp/127.0.0.1/"$SSH_PORT") 2>/dev/null && break
    sleep 0.5
  done
  ssh-keyscan -p "$SSH_PORT" -t ed25519 127.0.0.1 2>/dev/null > "$FIX/known_hosts"
  if [[ ! -s "$FIX/known_hosts" ]]; then
    echo "peer sshd did not come up; logs:"; $D logs --tail 30 "${PREFIX}-ssh"
    return 1
  fi

  # ---- rclone WebDAV peer ------------------------------------------------
  # A real cloud-like backend with an upload delay, so "cached" and
  # "uploaded" are genuinely different states. No account, so no credential.
  if command -v rclone >/dev/null 2>&1; then
    if ! pgrep -f "rclone serve webdav $FIX/dav-backend" >/dev/null 2>&1; then
      mkdir -p "$FIX/dav-backend"
      echo "webdav backend file" > "$FIX/dav-backend/remote-hello.txt"
      nohup rclone serve webdav "$FIX/dav-backend" \
        --addr "127.0.0.1:${DAV_PORT}" \
        > "$FIX/dav.log" 2>&1 &
      echo $! > "$FIX/dav.pid"
    fi
  fi

  status
}

stop() {
  have_docker && {
    # Only ever this fixture's own containers; never a global prune.
    $D rm -f "${PREFIX}-ssh" >/dev/null 2>&1
    $D volume rm -f "${PREFIX}-data" >/dev/null 2>&1
    $D network rm "$NET" >/dev/null 2>&1
  }
  [[ -f "$FIX/dav.pid" ]] && kill "$(cat "$FIX/dav.pid")" 2>/dev/null
  rm -f "$FIX/dav.pid"
  echo "peers stopped"
}

status() {
  local ok=0
  if have_docker && $D inspect -f '{{.State.Running}}' "${PREFIX}-ssh" 2>/dev/null | grep -q true; then
    echo "ssh peer:    running on 127.0.0.1:${SSH_PORT} (container ${PREFIX}-ssh)"
    ok=1
  else
    echo "ssh peer:    not running"
  fi
  if [[ -f "$FIX/dav.pid" ]] && kill -0 "$(cat "$FIX/dav.pid")" 2>/dev/null; then
    echo "webdav peer: running on 127.0.0.1:${DAV_PORT}"
  else
    echo "webdav peer: not running"
  fi
  [[ -f "$FIX/known_hosts" ]] && echo "known_hosts: $FIX/known_hosts"
  [[ -f "$FIX/id_peer" ]]     && echo "identity:    $FIX/id_peer"
  return $(( ok == 1 ? 0 : 1 ))
}

case "${1:-status}" in
  start) start ;;
  stop) stop ;;
  status) status ;;
  *) echo "usage: $0 start|stop|status"; exit 2 ;;
esac
