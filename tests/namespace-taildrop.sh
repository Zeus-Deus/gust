#!/usr/bin/env bash
# Usage: tests/namespace-taildrop.sh target/release/gust
# Requires Bubblewrap, headscale, Tailscale, iproute2, curl, jq and Python.
set -euo pipefail
binary=$(realpath "${1:?pass the path to a built gust executable}")
[[ -f $binary && -x $binary ]] || { echo "Gust executable unavailable" >&2; exit 1; }
headscale_bin=$(command -v "${HEADSCALE_BIN:-headscale}") || { echo "missing headscale" >&2; exit 77; }
for tool in bwrap tailscale tailscaled ip curl jq python3 timeout; do
    command -v "$tool" >/dev/null || { echo "missing $tool" >&2; exit 77; }
done
# The two daemons share a private network. NET_ADMIN is confined to its user
# namespace and only sets up the test link. No host sockets or devices enter.
exec timeout 100 bwrap --unshare-all --uid 0 --gid 0 --cap-add CAP_NET_ADMIN \
    --new-session --die-with-parent --ro-bind /usr /usr --symlink usr/bin /bin \
    --symlink usr/lib /lib --symlink usr/lib /lib64 --proc /proc --dev /dev \
    --tmpfs /tmp --dir /run --symlink /run /var/run --dir /etc --dir /root \
    --ro-bind "$headscale_bin" /headscale --ro-bind "$binary" /gust --clearenv --setenv PATH /usr/bin:/bin \
    --setenv HOME /root --setenv USER root --setenv LANG C \
    --setenv TS_NO_LOGS_NO_SUPPORT true bash -euo pipefail -s <<'GUEST'
umask 077
echo 'root:x:0:0:root:/root:/bin/sh' > /etc/passwd
echo 'root:x:0:' > /etc/group
mkdir -p /tmp/mesh/{desk,studio,received} /run/tailscale
ip link add lab0 type dummy
ip address add 10.88.0.1/24 dev lab0
ip link set lab0 up
cat > /tmp/mesh/config.yaml <<'CONFIG'
server_url: http://10.88.0.1:8080
listen_addr: 10.88.0.1:8080
metrics_listen_addr: 127.0.0.1:9090
noise:
  private_key_path: /tmp/mesh/noise.key
prefixes:
  v4: 100.64.0.0/10
  v6: fd7a:115c:a1e0::/48
derp:
  server:
    enabled: true
    region_id: 999
    region_code: gust
    region_name: Gust fixture
    stun_listen_addr: 10.88.0.1:3478
    private_key_path: /tmp/mesh/derp.key
    ipv4: 10.88.0.1
    automatically_add_embedded_derp_region: true
  urls: []
  auto_update_enabled: false
database:
  type: sqlite
  sqlite:
    path: /tmp/mesh/controller.sqlite
dns:
  magic_dns: false
  override_local_dns: false
unix_socket: /tmp/mesh/controller.sock
taildrop:
  enabled: true
CONFIG
controller=(/headscale -c /tmp/mesh/config.yaml)
"${controller[@]}" serve >/tmp/mesh/controller.log 2>&1 &
for _ in {1..100}; do
    if curl -fsS http://10.88.0.1:8080/health >/dev/null 2>&1; then break; fi
    sleep 0.1
done
curl -fsS http://10.88.0.1:8080/health >/dev/null || { echo "headscale failed to start" >&2; exit 1; }
"${controller[@]}" users create lab >/dev/null
user_id=$("${controller[@]}" users list -o json | jq -er '.[] | select(.name == "lab") | .id')
for node in desk studio; do
    socket=/tmp/mesh/studio/socket
    if [[ $node == desk ]]; then socket=/run/tailscale/tailscaled.sock; fi
    tailscaled --tun=userspace-networking --statedir="/tmp/mesh/$node" \
        --state="/tmp/mesh/$node/state" --socket="$socket" --port=0 >"/tmp/mesh/$node.log" 2>&1 &
    for _ in {1..50}; do [[ -S $socket ]] && break; sleep 0.1; done
    "${controller[@]}" preauthkeys create -u "$user_id" -e 5m -o json | jq -er .key |
        tailscale --socket="$socket" up --auth-key=file:/dev/stdin --hostname="$node" \
            --login-server=http://10.88.0.1:8080 --accept-dns=false --timeout=20s
done
for _ in {1..60}; do
    if tailscale file cp --targets | grep -qw studio; then break; fi
    sleep 0.2
done

python3 - <<'PY'
import json
import subprocess
from pathlib import Path

def call(server, request_id, name, arguments):
    params = {"name": name, "arguments": arguments, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    }}
    request = {"jsonrpc": "2.0", "id": request_id, "method": "tools/call", "params": params}
    print(json.dumps(request), file=server.stdin, flush=True)
    for line in server.stdout:
        reply = json.loads(line)
        if reply.get("id") == request_id:
            result = reply["result"]
            if result["isError"]:
                raise RuntimeError(result["content"])
            return result["structuredContent"]
    raise RuntimeError(f"Gust exited before answering {name}")

source = Path("/root/Transfer # 1.txt")
payload = b"Gust remote receipt\n"
source.write_bytes(payload)
with subprocess.Popen(["/gust", "--mcp"], stdin=subprocess.PIPE,
                      stdout=subprocess.PIPE, text=True) as server:
    devices = call(server, 1, "list_devices", {})["devices"]
    peers = [d for d in devices if d["id"].startswith("taildrop:") and d["state"] == "ready"]
    assert len(peers) == 1, peers
    receipt = call(server, 2, "send_files", {"device": peers[0]["id"], "paths": [str(source)]})
    assert receipt["transport"] == "taildrop" and receipt["sent"][0]["name"] == source.name, receipt
    server.stdin.close()
    assert server.wait(timeout=5) == 0
subprocess.run(["tailscale", "--socket=/tmp/mesh/studio/socket", "file", "get",
                "/tmp/mesh/received"], check=True, timeout=15)
assert (Path("/tmp/mesh/received") / source.name).read_bytes() == payload
assert source.read_bytes() == payload
print("PASS Gust discovered the receiver, sent the file, and the second daemon delivered the exact bytes")
PY
GUEST
