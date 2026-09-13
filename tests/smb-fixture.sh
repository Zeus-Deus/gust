#!/usr/bin/env bash
# A real SMB server, built out of nothing but this machine.
#
# An unprivileged smbd on a high port, serving one directory, with its own
# private user database. Nothing here touches the system's samba config, no
# system service is started, and the only account is a throwaway created for
# the test — so there is no credential of the user's to leak.
#
# The password IS written to a file here, because smbpasswd needs one. It is
# a random value generated per run, used only by this fixture, stored under
# the fixture root with mode 600, and destroyed on stop. Gust itself never
# reads it: the test types it into the GIO prompt exactly as a person would.
set -uo pipefail

ROOT="${GUST_SMB_FIXTURE:-/tmp/gust-smb-fixture}"
# 445 by default: GVfs's SMB backend ignores a port in the URI, so a share
# on any other port can be reached by smbclient but NOT by the code path
# Gust uses. Below 1024 needs privilege; the fixture asks for it explicitly
# rather than quietly testing something else.
PORT="${GUST_SMB_PORT:-445}"
SHARE="$ROOT/share"
PRIV="$ROOT/private"
CONF="$ROOT/smb.conf"
LOGD="$ROOT/log"
USER="$(id -un)"

start() {
  mkdir -p "$SHARE" "$PRIV" "$LOGD" "$ROOT/lock" "$ROOT/state" "$ROOT/cache" \
           "$ROOT/ncalrpc"
  chmod 700 "$PRIV"

  # A random password, per run, for a throwaway account.
  local pw
  pw="$(head -c 18 /dev/urandom | base64 | tr -d '/+=' | head -c 20)"
  printf '%s' "$pw" > "$ROOT/password"
  chmod 600 "$ROOT/password"

  cat > "$CONF" <<EOF
[global]
    workgroup = GUSTTEST
    server string = Gust test share
    # Everything private to this fixture: no system paths are touched.
    private dir = $PRIV
    lock directory = $ROOT/lock
    state directory = $ROOT/state
    cache directory = $ROOT/cache
    pid directory = $ROOT/lock
    ncalrpc dir = $ROOT/ncalrpc
    log file = $LOGD/smbd.log
    logging = file
    log level = 1
    # Loopback only. This share must not be reachable from anywhere else.
    bind interfaces only = yes
    interfaces = 127.0.0.1
    smb ports = $PORT
    # No printing, no browsing, no discovery beyond this machine.
    load printers = no
    printing = bsd
    disable spoolss = yes
    smb1 unix extensions = no
    server min protocol = SMB2
    passdb backend = tdbsam:$PRIV/passdb.tdb

[shared]
    path = $SHARE
    read only = no
    guest ok = no
    valid users = $USER
    force user = $USER
EOF
  chmod 600 "$CONF"

  # A samba account for the current user, with the generated password.
  # `smbpasswd -a` refuses unless run as root; pdbedit creates the account
  # directly in the fixture's own passdb without privilege. The password
  # goes in on stdin, never in argv where `ps` would show it.
  printf '%s\n%s\n' "$pw" "$pw" | \
    pdbedit --configfile="$CONF" -t -a -u "$USER" >>"$LOGD/pdbedit.log" 2>&1
  if ! pdbedit --configfile="$CONF" -L 2>/dev/null | grep -q "^$USER:"; then
    echo "could not create the fixture samba account; see $LOGD/pdbedit.log" >&2
    return 1
  fi

  if ss -ltn 2>/dev/null | grep -q ":$PORT "; then
    echo "port $PORT already in use" >&2
    return 1
  fi

  # Foreground smbd in the background of this script, so we own the pid.
  # A port below 1024 needs privilege. Everything else about the fixture
  # stays unprivileged and self-contained: its own config, passdb, logs and
  # lock directories, and an account that exists only here.
  if [[ "$PORT" -lt 1024 ]]; then
    if ! sudo -n true 2>/dev/null; then
      echo "port $PORT needs privilege and sudo -n is unavailable." >&2
      echo "Re-run with GUST_SMB_PORT=4450 to serve on a high port," >&2
      echo "but note that GVfs cannot reach it, so the Gust path is untested." >&2
      return 1
    fi
    sudo -n smbd --configfile="$CONF" --foreground --no-process-group \
         --debuglevel=1 >"$LOGD/smbd.out" 2>&1 &
  else
    smbd --configfile="$CONF" --foreground --no-process-group \
         --debuglevel=1 >"$LOGD/smbd.out" 2>&1 &
  fi
  echo $! > "$ROOT/smbd.pid"

  for _ in $(seq 1 40); do
    ss -ltn 2>/dev/null | grep -q ":$PORT " && break
    sleep 0.25
  done
  if ! ss -ltn 2>/dev/null | grep -q ":$PORT "; then
    echo "smbd did not come up; see $LOGD/smbd.log" >&2
    return 1
  fi

  # Seed the share so a listing has something in it.
  printf 'hello from the smb share\n' > "$SHARE/hello.txt"
  mkdir -p "$SHARE/subdir"
  printf 'nested\n' > "$SHARE/subdir/inner.txt"

  echo "smb fixture up:"
  echo "  share    $SHARE"
  echo "  address  //127.0.0.1/shared  (port $PORT)"
  echo "  user     $USER"
  echo "  config   $CONF"
}

stop() {
  [[ -f "$ROOT/smbd.pid" ]] && kill "$(cat "$ROOT/smbd.pid")" 2>/dev/null
  rm -f "$ROOT/smbd.pid"
  sleep 0.5
  # Only this fixture's smbd, matched by its own config path.
  pkill -f "smbd --configfile=$CONF" 2>/dev/null
  sudo -n pkill -f "smbd --configfile=$CONF" 2>/dev/null
  # The generated password dies with the fixture.
  rm -f "$ROOT/password"
  echo "smb fixture stopped"
}

status() {
  echo "  listening: $(ss -ltn 2>/dev/null | grep -c ":$PORT ") on $PORT"
  echo "  share:     $(ls -1 "$SHARE" 2>/dev/null | wc -l) entries"
}

case "${1:-}" in
  start)  start ;;
  stop)   stop ;;
  status) status ;;
  *) echo "usage: $0 {start|stop|status}" >&2; exit 2 ;;
esac
