#!/usr/bin/env bash
# A real rclone remote, built out of nothing but this machine.
#
# `rclone serve webdav` publishes a directory over HTTP; a second rclone
# mounts that WebDAV endpoint with a VFS write cache. The result behaves like
# a cloud remote in the way that matters for the plan's §3 rule 1: a write
# returns as soon as the bytes are in the local cache, and the upload to the
# "backend" happens afterwards and can be made to fail.
#
# Nothing here touches a real account, and no credential exists to leak:
# the endpoint is unauthenticated and bound to loopback only.
set -uo pipefail

ROOT="${GUST_RCLONE_FIXTURE:-/tmp/gust-rclone-fixture}"
PORT="${GUST_RCLONE_PORT:-8188}"
BACKEND="$ROOT/backend"          # what the "cloud" actually holds
CACHE="$ROOT/cache"              # rclone's VFS cache
CONF="$ROOT/rclone.conf"
MNT="$ROOT/mnt"
LOG="$ROOT/log"

start() {
  mkdir -p "$BACKEND" "$CACHE" "$MNT" "$LOG"

  # The config names one remote pointing at the local WebDAV server. No
  # password, no token: there is nothing to keep secret, which is the point.
  cat > "$CONF" <<EOF
[gustcloud]
type = webdav
url = http://127.0.0.1:$PORT
vendor = other
EOF
  chmod 600 "$CONF"

  if ss -ltn 2>/dev/null | grep -q ":$PORT "; then
    echo "port $PORT already in use" >&2
    return 1
  fi

  rclone serve webdav "$BACKEND" \
      --addr "127.0.0.1:$PORT" \
      --log-file "$LOG/serve.log" --log-level INFO \
      >/dev/null 2>&1 &
  echo $! > "$ROOT/serve.pid"

  for _ in $(seq 1 40); do
    ss -ltn 2>/dev/null | grep -q ":$PORT " && break
    sleep 0.25
  done
  if ! ss -ltn 2>/dev/null | grep -q ":$PORT "; then
    echo "webdav server did not come up; see $LOG/serve.log" >&2
    return 1
  fi

  # --vfs-cache-mode writes is the whole point: a write lands in the cache
  # and is uploaded afterwards, so "saved" and "uploaded" are different
  # moments and Gust has to tell them apart.
  rclone mount "gustcloud:" "$MNT" \
      --config "$CONF" \
      --vfs-cache-mode writes \
      --vfs-write-back "${GUST_RCLONE_WRITEBACK:-5s}" \
      --cache-dir "$CACHE" \
      --rc --rc-addr "127.0.0.1:$((PORT + 1))" --rc-no-auth \
      --log-file "$LOG/mount.log" --log-level INFO \
      >/dev/null 2>&1 &
  echo $! > "$ROOT/mount.pid"

  for _ in $(seq 1 40); do
    mountpoint -q "$MNT" && break
    sleep 0.25
  done
  mountpoint -q "$MNT" || { echo "mount did not appear; see $LOG/mount.log" >&2; return 1; }

  echo "rclone fixture up:"
  echo "  backend  $BACKEND"
  echo "  mount    $MNT"
  echo "  config   $CONF"
  echo "  rc       127.0.0.1:$((PORT + 1))"
}

stop() {
  mountpoint -q "$MNT" && fusermount3 -u "$MNT" 2>/dev/null
  for f in mount serve; do
    [[ -f "$ROOT/$f.pid" ]] && kill "$(cat "$ROOT/$f.pid")" 2>/dev/null
    rm -f "$ROOT/$f.pid"
  done
  sleep 0.5
  pkill -f "rclone serve webdav $BACKEND" 2>/dev/null
  pkill -f "rclone mount gustcloud:" 2>/dev/null
  echo "rclone fixture stopped"
}

status() {
  echo "  serving: $(ss -ltn 2>/dev/null | grep -c ":$PORT ") listener(s) on $PORT"
  echo "  mounted: $(mountpoint -q "$MNT" && echo yes || echo no)"
  echo "  backend: $(ls -1 "$BACKEND" 2>/dev/null | wc -l) entries"
}

case "${1:-}" in
  start)  start ;;
  stop)   stop ;;
  status) status ;;
  *) echo "usage: $0 {start|stop|status}" >&2; exit 2 ;;
esac
