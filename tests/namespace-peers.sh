#!/usr/bin/env bash
# Run Gust's SSHFS and WebDAV integration tests with disposable Linux peers.
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in bwrap cargo python3 rclone ssh-keygen sshfs fusermount3 ss mountpoint; do
    command -v "$tool" >/dev/null || { echo "missing $tool" >&2; exit 77; }
done
[[ -r /dev/fuse && -w /dev/fuse ]] || { echo "/dev/fuse unavailable" >&2; exit 77; }
manifest=$(mktemp)
trap 'rm -f "$manifest"' EXIT
flock /tmp/gust-build.lock cargo test --no-run --locked -p gust-remote \
    --test sshfs_live --test rclone_live --test rclone_failure_matrix \
    --message-format=json-render-diagnostics > "$manifest"

for suite in sshfs_live rclone_live rclone_failure_matrix; do
    binary=$(python3 - "$manifest" "$suite" <<'PY'
import json, sys
with open(sys.argv[1]) as stream:
    matches = [msg["executable"] for msg in map(json.loads, stream)
               if msg.get("executable") and msg["target"]["name"] == sys.argv[2]]
if len(matches) != 1:
    sys.exit("missing or ambiguous Cargo test executable")
print(matches[0])
PY
    )
    # UID 0 and mount capability exist only inside the new user namespace.
    # Share only system tools, the chosen test executable, and the FUSE device.
    bwrap --unshare-all --uid 0 --gid 0 --cap-add CAP_SYS_ADMIN \
        --new-session --die-with-parent --ro-bind /usr /usr \
        --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/lib /lib64 \
        --proc /proc --dev /dev --dev-bind /dev/fuse /dev/fuse \
        --tmpfs /tmp --dir /run --dir /etc --dir /root \
        --ro-bind "$binary" /suite --ro-bind tests/rclone-fixture.sh /rclone-fixture \
        --clearenv --setenv PATH /usr/bin:/bin --setenv HOME /root --setenv USER gust \
        --setenv LANG C --setenv GUST_ALL_REQUIRED 1 --setenv XDG_RUNTIME_DIR /run/user/0 \
        bash -eu -s -- "$suite" <<'GUEST'
printf 'root:x:0:0:root:/root:/bin/sh\n' > /etc/passwd
printf 'root:x:0:\n' > /etc/group
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
if [[ $1 == sshfs_live ]]; then
    fix=/tmp/gust-remote-fixture
    peer=/tmp/sftp-root
    mkdir -p "$fix/etc" "$peer$fix/chroot/shared"
    ln -s "$peer$fix/chroot" "$fix/chroot"
    echo "remote fixture ready" > "$fix/chroot/shared/hello.txt"
    ssh-keygen -qt ed25519 -f "$fix/etc/host" -N ''
    ssh-keygen -qt ed25519 -f "$fix/etc/id_test" -N ''
    sed 's/^/[127.0.0.1]:2222 /' "$fix/etc/host.pub" > "$fix/etc/known_hosts"
    rclone serve sftp "$peer" --addr 127.0.0.1:2222 --key "$fix/etc/host" \
        --user gust --authorized-keys "$fix/etc/id_test.pub" > /tmp/sftp.log 2>&1 &
    echo $! > "$fix/etc/sshd.pid"
    for _ in {1..100}; do
        if ss -ltn | grep -q ':2222 '; then break; fi
        sleep 0.05
    done
else
    bash /rclone-fixture start
fi
/suite --test-threads=1
GUEST
done
