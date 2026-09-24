#!/usr/bin/env bash
# A private SSH server for testing remote places.
#
# It is entirely self-contained: its own host key, its own client key, its
# own authorized_keys and known_hosts, listening only on loopback on a
# private port. It NEVER reads or writes the user's ~/.ssh, and it never
# touches a host they actually use.
#
#   tests/remote-fixture.sh start | stop | status
set -uo pipefail

R="${GUST_REMOTE_FIXTURE:-/tmp/gust-remote-fixture}"
PORT="${GUST_REMOTE_PORT:-2222}"
SFTP_SERVER=""
for c in /usr/lib/ssh/sftp-server /usr/lib/openssh/sftp-server /usr/libexec/sftp-server; do
  [[ -x "$c" ]] && SFTP_SERVER="$c" && break
done

start() {
  if status >/dev/null 2>&1; then
    echo "already running on $PORT"
    return 0
  fi
  SSHD="$(command -v sshd)" || { echo "sshd not installed"; return 1; }
  [[ "$SSHD" = /* ]] || SSHD="/usr/bin/sshd"
  [[ -n "$SFTP_SERVER" ]] || { echo "no sftp-server found"; return 1; }

  sudo -n rm -rf "$R" 2>/dev/null || rm -rf "$R"
  mkdir -p "$R/etc"
  chmod 755 "$R"

  # The SFTP session is CHROOTED into $R/chroot, so it cannot see anything
  # outside the fixture — not the user's home, not the rest of the disk.
  # Without this, "connect" lands in the real login directory and a test
  # would browse somebody's personal files.
  # sshd requires the chroot root to be owned by root and not group/other
  # writable; the share inside it belongs to the test user.
  sudo -n mkdir -p "$R/chroot/shared/subdir" || {
    echo "needs passwordless sudo to build a chrooted fixture"; return 1; }
  sudo -n chown root:root "$R/chroot"
  sudo -n chmod 755 "$R/chroot"
  sudo -n chown -R "$(id -un):$(id -gn)" "$R/chroot/shared"

  ssh-keygen -q -t ed25519 -f "$R/etc/host_ed25519" -N '' -C gust-test-host
  ssh-keygen -q -t ed25519 -f "$R/etc/id_test"      -N '' -C gust-test-client
  cp "$R/etc/id_test.pub" "$R/etc/authorized_keys"
  chmod 600 "$R/etc/authorized_keys"

  cat > "$R/etc/sshd_config" <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey $R/etc/host_ed25519
AuthorizedKeysFile $R/etc/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
PidFile $R/etc/sshd.pid
StrictModes no
LogLevel VERBOSE
# internal-sftp is required for ChrootDirectory: an external binary would
# not exist inside the chroot.
Subsystem sftp internal-sftp
# Must be LAST: every directive after a Match belongs to that block.
# An UNPRIVILEGED sshd cannot chroot ("server lacks privileges to chroot"),
# so the session is pinned with internal-sftp -d instead: every path is
# resolved relative to the fixture share, so a connection can never reach
# the user's real home directory even by accident.
Match User ${USER:-$(id -un)}
    ForceCommand internal-sftp -d $R/chroot/shared
    AllowTcpForwarding no
EOF

  echo "remote fixture ready" > "$R/chroot/shared/hello.txt"
  echo "second file"         > "$R/chroot/shared/notes.md"
  echo nested                > "$R/chroot/shared/subdir/deep.txt"

  "$SSHD" -f "$R/etc/sshd_config" -E "$R/etc/sshd.log" || {
    echo "sshd failed to start:"; tail -5 "$R/etc/sshd.log"; return 1; }

  for _ in $(seq 1 20); do
    ss -ltn 2>/dev/null | grep -q ":$PORT " && break
    sleep 0.2
  done

  # Record the server's real key so a test can tell a CHANGED key from an
  # unknown one. Written by us, never merged into the user's known_hosts.
  ssh-keyscan -p "$PORT" -t ed25519 127.0.0.1 2>/dev/null \
    | sed "s|^127.0.0.1|[127.0.0.1]:$PORT|" > "$R/etc/known_hosts"

  cat > "$R/etc/ssh_config" <<EOF
Host gust-test
    HostName 127.0.0.1
    Port $PORT
    User ${USER:-$(id -un)}
    IdentityFile $R/etc/id_test
    UserKnownHostsFile $R/etc/known_hosts
    IdentitiesOnly yes
EOF
  echo "  (sftp is chrooted to $R/chroot — /shared is the share)"

  status
}

stop() {
  # Unmount anything still attached, or the mount points outlive the server.
  for d in "${TMPDIR:-/tmp}"/gust-mnt/*; do
    [[ -d "$d" ]] && fusermount -u "$d" 2>/dev/null
  done
  if [[ -f "$R/etc/sshd.pid" ]]; then
    kill "$(cat "$R/etc/sshd.pid")" 2>/dev/null
    sleep 0.5
  fi
  pkill -f "sshd -f $R/etc/sshd_config" 2>/dev/null
  sudo -n rm -rf "$R" 2>/dev/null || rm -rf "$R"
  echo "stopped"
}

status() {
  if [[ -f "$R/etc/sshd.pid" ]] && kill -0 "$(cat "$R/etc/sshd.pid")" 2>/dev/null; then
    echo "running on 127.0.0.1:$PORT (pid $(cat "$R/etc/sshd.pid"))"
    return 0
  fi
  echo "not running"
  return 1
}

case "${1:-status}" in
  start)  start ;;
  stop)   stop ;;
  status) status ;;
  *) echo "usage: $0 {start|stop|status}"; exit 2 ;;
esac
