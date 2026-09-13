#!/usr/bin/env bash
# A real NFS export, served from this machine to itself.
#
# Unlike the SMB and SSH fixtures this one genuinely needs root: NFS is a
# kernel service and there is no unprivileged equivalent. It is confined as
# far as it can be — the export is loopback-only, read-write to 127.0.0.1
# and nothing else, and both the export table entry and the service are
# removed on stop.
#
# There are no credentials here at all: NFSv4 with AUTH_SYS authorises by
# address, which is exactly why the export is bound to loopback.
set -uo pipefail

ROOT="${GUST_NFS_FIXTURE:-/tmp/gust-nfs-fixture}"
EXPORT="$ROOT/export"
EXPORTS_LINE="$EXPORT 127.0.0.1(rw,sync,no_subtree_check,insecure,fsid=0)"

start() {
  if ! sudo -n true 2>/dev/null; then
    echo "NFS is a kernel service and needs root; sudo -n is unavailable." >&2
    return 1
  fi
  mkdir -p "$EXPORT"
  chmod 777 "$EXPORT"

  printf 'hello from the nfs export\n' > "$EXPORT/hello.txt"
  mkdir -p "$EXPORT/subdir"
  printf 'nested\n' > "$EXPORT/subdir/inner.txt"

  # nfsd first: an export with no server behind it is not reachable.
  # Remember whether we started it, so stop does not tear down a server
  # somebody else was using.
  # rpc.nfsd registers with rpcbind and fails SILENTLY (exit 1, errno 111
  # only in dmesg) when rpcbind is not running.
  if ! pgrep -x rpcbind >/dev/null 2>&1; then
    sudo -n systemctl start rpcbind >/dev/null 2>&1
    echo started > "$ROOT/we-started-rpcbind"
    sleep 1
  fi
  if ! pgrep -x nfsd >/dev/null 2>&1; then
    sudo -n modprobe nfsd 2>/dev/null
    if ! sudo -n rpc.nfsd 8 2>/dev/null; then
      echo "rpc.nfsd failed; check: sudo dmesg | tail" >&2
      return 1
    fi
    echo started > "$ROOT/we-started-nfsd"
    sleep 1
  fi

  # Add ONLY this export, at runtime, so /etc/exports is never written.
  if ! sudo -n exportfs -v 2>/dev/null | grep -qE "^$EXPORT([[:space:]]|$)"; then
    sudo -n exportfs -o rw,sync,no_subtree_check,insecure,fsid=0 \
         "127.0.0.1:$EXPORT" >/dev/null 2>&1
  fi
  # NOT `exportfs -r`: it re-reads /etc/exports and discards runtime-only
  # exports, which is exactly what this fixture's export is. The whole
  # point of adding it at runtime is to leave the system file untouched.

  # `exportfs -v` prints the path on one line and its options on the next,
  # so match the path line on its own.
  for _ in $(seq 1 30); do
    sudo -n exportfs -v 2>/dev/null | grep -qx "$EXPORT" && break
    sudo -n exportfs -v 2>/dev/null | grep -q "^$EXPORT[[:space:]]" && break
    sleep 0.25
  done
  if ! sudo -n exportfs -v 2>/dev/null | grep -qE "^$EXPORT([[:space:]]|\$)"; then
    echo "the export did not appear; see exportfs -v" >&2
    return 1
  fi

  echo "nfs fixture up:"
  echo "  export   $EXPORT"
  echo "  address  127.0.0.1:/  (NFSv4 root)"
  sudo -n exportfs -v 2>/dev/null | grep "^$EXPORT" | sed 's/^/  /'
}

stop() {
  sudo -n true 2>/dev/null || { echo "no privilege to stop"; return 0; }
  # Remove exactly the export we added.
  sudo -n exportfs -u "127.0.0.1:$EXPORT" >/dev/null 2>&1
  # Only stop nfsd if this fixture started it: another user's export must
  # not be torn down by our cleanup.
  if [[ -f "$ROOT/we-started-nfsd" ]]; then
    sudo -n rpc.nfsd 0 >/dev/null 2>&1
    rm -f "$ROOT/we-started-nfsd"
  fi
  if [[ -f "$ROOT/we-started-rpcbind" ]]; then
    sudo -n systemctl stop rpcbind >/dev/null 2>&1
    rm -f "$ROOT/we-started-rpcbind"
  fi
  echo "nfs fixture stopped"
}

status() {
  echo "  exported: $(sudo -n exportfs -v 2>/dev/null | grep -cE "^$EXPORT([[:space:]]|$)")"
  echo "  nfsd:     $(pgrep -c nfsd 2>/dev/null || echo 0) threads"
  echo "  contents: $(ls -1 "$EXPORT" 2>/dev/null | wc -l) entries"
}

case "${1:-}" in
  start)  start ;;
  stop)   stop ;;
  status) status ;;
  *) echo "usage: $0 {start|stop|status}" >&2; exit 2 ;;
esac
