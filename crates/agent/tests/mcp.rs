//! `gust --mcp` over real pipes, against stand-ins for the system's side
//! only: a fake sysfs/udev tree with an iPhone plugged in, a `gio` that
//! speaks gio's own Trust protocol (as in `just qa` §37), and `lsblk` and
//! `tailscale` scripts. Everything from protocol to copy is Gust's code.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;

const UDID: &str = "00008030-000A1B2C3D4E5F60";
const SERIAL: &str = "00008030000A1B2C3D4E5F60";
const MODERN: &str = "2026-07-28";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("gust-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in [
            "home/Pictures",
            "home/.ssh",
            "sys/1-1",
            "udev",
            "gvfs",
            "bin",
            "state",
            "taildrop",
            "tmp",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let m = root.join("mnt");
        std::fs::create_dir_all(m.join(format!("afc:host={UDID},port=3/VLC"))).unwrap();
        std::fs::create_dir_all(m.join(format!("afc:host={UDID},port=3/Documents"))).unwrap();
        std::fs::create_dir_all(m.join(format!("afc:host={UDID},port=3/.hidden"))).unwrap();
        std::fs::write(root.join("gvfs/afc.mount"), "").unwrap();
        // What the kernel and udev write for a plugged-in iPhone.
        let dev = root.join("sys/1-1");
        for (f, v) in [
            ("idVendor", "05ac"),
            ("product", "iPhone"),
            ("serial", SERIAL),
            ("busnum", "1"),
            ("devnum", "5"),
        ] {
            std::fs::write(dev.join(f), format!("{v}\n")).unwrap();
        }
        std::fs::write(
            root.join("udev/c189:4"),
            format!("E:USBMUX_SUPPORTED=1\nE:ID_SERIAL=Apple_Inc._iPhone_{SERIAL}\n"),
        )
        .unwrap();
        std::fs::write(root.join("home/Pictures/beach.jpg"), b"jpeg bytes").unwrap();
        std::fs::write(root.join("home/.ssh/id_ed25519"), b"secret").unwrap();

        let t = root.display();
        script(
            &root.join("bin/gio"),
            &format!(
                r#"#!/usr/bin/env bash
T="{t}"; M="$T/mnt"
dir_of() {{ case "$1" in
  afc://*:3/) h="${{1#afc://}}"; h="${{h%:3/}}"; echo "$M/afc:host=$h,port=3" ;;
  afc://*) h="${{1#afc://}}"; h="${{h%/}}"; echo "$M/afc:host=$h" ;;
  *) return 1 ;; esac; }}
case "$1 $2" in
  "mount -li") printf '%s\n' "Volume(0): Documents on Alex's iPhone" "  uuid={UDID}" "  activation_root=afc://{UDID}:3/"; exit 0 ;;
  "mount --") uri="$3"; d="$(dir_of "$uri")" || exit 1
    echo "mount $uri" >> "$T/gio-calls"
    while [[ ! -e "$T/trusted" ]]; do
      printf 'Untrusted Device\nThe device “iPhone” is not trusted yet.\n\nSelect “Trust” on the device and click “Try again”.\n[1] Try again\n[2] Cancel\nChoice: '
      read -r ans || exit 1
      [[ "$ans" == 2 ]] && {{ echo "gio: $uri: Operation was cancelled" >&2; exit 1; }}
    done
    touch "$d.mounted"; exit 0 ;;
  "info -a") uri="$5"; d="$(dir_of "$uri")" || exit 1
    [[ -e "$d.mounted" ]] || {{ echo "gio: $uri: not mounted" >&2; exit 1; }}
    printf 'name: /\nlocal path: %s\nuri: %s\n' "$d" "$uri"; exit 0 ;;
esac
exit 1
"#
            ),
        );
        script(
            &root.join("bin/lsblk"),
            "#!/bin/sh\necho '{\"blockdevices\":[]}'\n",
        );
        script(
            &root.join("bin/tailscale"),
            &format!(
                r#"#!/usr/bin/env bash
T="{t}"
if [[ "$*" == "file cp --targets" ]]; then
  printf '100.64.0.2\tAlex-iPad\n100.64.0.3\told-laptop\toffline\n'; exit 0
fi
if [[ "$1 $2 $3" == "file cp --name" && "$5" == "--" ]]; then
  [[ "$4" == fail.jpg ]] && exit 1
  echo "$7" >> "$T/taildrop-calls"; cat "$6" > "$T/taildrop/$4"; exit 0
fi
exit 1
"#
            ),
        );
        Fixture { root }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }

    fn apps(&self) -> PathBuf {
        self.path(&format!("mnt/afc:host={UDID},port=3"))
    }

    fn trust(&self) {
        std::fs::write(self.path("trusted"), "").unwrap();
    }

    fn start(&self) -> Server {
        let r = &self.root;
        let mut child = Command::new(env!("CARGO_BIN_EXE_gust-mcp"))
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", r.join("bin").display()))
            .env("HOME", r.join("home"))
            .env("USER", "nobody-test")
            .env("TMPDIR", r.join("tmp"))
            .env("XDG_STATE_HOME", r.join("state"))
            .env("GUST_SYSFS_USB", r.join("sys"))
            .env("GUST_UDEV_DATA", r.join("udev"))
            .env("GUST_GVFS_MOUNTS", r.join("gvfs"))
            .env("GUST_GIO", r.join("bin/gio"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // Every stdout line must be one JSON-RPC message.
                let v: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|_| panic!("stdout carried a non-JSON line: {line:?}"));
                assert_eq!(v["jsonrpc"], "2.0", "{line}");
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        Server {
            stdin: Some(child.stdin.take().unwrap()),
            child,
            rx,
            next: 0,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    next: u64,
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

impl Server {
    fn send(&mut self, v: &Value) {
        let s = self.stdin.as_mut().unwrap();
        writeln!(s, "{v}").unwrap();
        s.flush().unwrap();
    }

    fn raw(&mut self, line: &str) {
        let s = self.stdin.as_mut().unwrap();
        writeln!(s, "{line}").unwrap();
        s.flush().unwrap();
    }

    /// Send a request and return its response, collecting notifications.
    fn request(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.response_to(id)
    }

    fn response_to(&mut self, id: u64) -> (Value, Vec<Value>) {
        let mut notes = Vec::new();
        loop {
            let v = self
                .rx
                .recv_timeout(Duration::from_secs(30))
                .expect("no response in time");
            if v.get("id") == Some(&json!(id)) {
                return (v, notes);
            }
            notes.push(v);
        }
    }

    fn tool(&mut self, name: &str, args: Value) -> (Value, Vec<Value>) {
        let (v, notes) = self.request(
            "tools/call",
            json!({"name": name, "arguments": args, "_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN,
                "io.modelcontextprotocol/clientCapabilities": {},
                "progressToken": "p",
            }}),
        );
        let result = v["result"].clone();
        assert_eq!(result["resultType"], "complete", "{v}");
        (result, notes)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn error_code(result: &Value) -> String {
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let v: Value = serde_json::from_str(text).unwrap();
    v["error"]["code"].as_str().unwrap().to_string()
}

fn structured(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    let s = result["structuredContent"].clone();
    // The text block mirrors the structured result for older clients.
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, s);
    s
}

#[test]
fn a_legacy_client_completes_the_initialize_handshake_and_lists_three_tools() {
    let f = Fixture::new("legacy");
    let mut s = f.start();
    let (v, _) = s.request(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}
        }),
    );
    assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(v["result"]["serverInfo"]["name"], "gust");
    assert!(v["result"]["capabilities"]["tools"].is_object());
    assert!(v["result"]["instructions"]
        .as_str()
        .unwrap()
        .starts_with("Send files"));
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let (v, _) = s.request("ping", json!({}));
    assert_eq!(v["result"], json!({}));
    let (v, _) = s.request("tools/list", json!({}));
    let tools = v["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["list_devices", "list_device_folders", "send_files"]);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert_eq!(t["outputSchema"]["type"], "object");
        assert_eq!(t["annotations"]["destructiveHint"], false, "{t}");
    }
    // Legacy results carry no stateless-revision fields.
    assert!(v["result"].get("resultType").is_none());

    // An unknown revision gets the newest one this server speaks.
    let (v, _) = s.request(
        "initialize",
        json!({"protocolVersion": "2099-01-01", "capabilities": {}}),
    );
    assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn a_modern_client_discovers_the_server_and_protocol_errors_are_exact() {
    let f = Fixture::new("modern");
    let mut s = f.start();
    let (v, _) = s.request("server/discover", json!({"_meta": meta()}));
    let r = &v["result"];
    assert_eq!(r["resultType"], "complete");
    assert_eq!(r["supportedVersions"][0], MODERN);
    assert!(r["capabilities"]["tools"].is_object());
    assert_eq!(r["ttlMs"], 0);
    assert_eq!(r["cacheScope"], "public");
    assert_eq!(
        r["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "gust"
    );

    let (v, _) = s.request("tools/list", json!({"_meta": meta()}));
    assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(v["result"]["resultType"], "complete");
    assert_eq!(v["result"]["cacheScope"], "public");

    // Without a version and without a handshake: invalid params.
    let (v, _) = s.request("tools/list", json!({}));
    assert_eq!(v["error"]["code"], -32602);
    // Missing client capabilities: invalid params.
    let (v, _) = s.request(
        "tools/list",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": MODERN}}),
    );
    assert_eq!(v["error"]["code"], -32602);
    // An unknown revision names the supported ones.
    let (v, _) = s.request(
        "server/discover",
        json!({"_meta": {
            "io.modelcontextprotocol/protocolVersion": "1900-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}),
    );
    assert_eq!(v["error"]["code"], -32022);
    assert_eq!(v["error"]["data"]["requested"], "1900-01-01");
    assert!(v["error"]["data"]["supported"]
        .as_array()
        .unwrap()
        .contains(&json!(MODERN)));
    let (v, _) = s.request("resources/list", json!({"_meta": meta()}));
    assert_eq!(v["error"]["code"], -32601);
    let (v, _) = s.request(
        "tools/call",
        json!({"name": "delete_everything", "_meta": meta()}),
    );
    assert_eq!(v["error"]["code"], -32602);

    // ping belongs to the handshake era: a modern ping is validated and shaped.
    let (v, _) = s.request("ping", json!({}));
    assert_eq!(v["error"]["code"], -32602);
    let (v, _) = s.request("ping", json!({"_meta": meta()}));
    assert_eq!(v["result"]["resultType"], "complete");
    // Not JSON-RPC 2.0: refused, not served.
    s.raw(r#"{"id":"x","method":"server/discover","params":{}}"#);
    let v = s.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(v["error"]["code"], -32600);
    assert_eq!(v["id"], "x");

    s.raw("{not json");
    let v = s.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(v["error"]["code"], -32700);
    s.raw("[]");
    let v = s.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(v["error"]["code"], -32600);

    // The server is still answering, and exits when stdin closes.
    let (v, _) = s.request("server/discover", json!({"_meta": meta()}));
    assert!(v.get("result").is_some());
    drop(s.stdin.take());
    let exited = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        matches!(s.child.try_wait(), Ok(Some(_)))
    });
    assert!(exited, "the server kept running after stdin closed");
}

#[test]
fn list_devices_names_the_iphone_and_taildrop_peers_without_contacting_them() {
    let f = Fixture::new("list");
    let mut s = f.start();
    let (r, _) = s.tool("list_devices", json!({}));
    let out = structured(&r);
    let devices = out["devices"].as_array().unwrap();
    let phone = devices
        .iter()
        .find(|d| d["id"] == format!("apple:{UDID}"))
        .expect("iPhone listed");
    assert_eq!(phone["name"], "Alex's iPhone");
    assert_eq!(phone["kind"], "apple");
    assert_eq!(phone["state"], "ready");
    assert_eq!(phone["needs_folder"], true);
    assert!(phone["lands_in"]
        .as_str()
        .unwrap()
        .contains("not added to the Photos library"));
    let ipad = devices
        .iter()
        .find(|d| d["id"] == "taildrop:100.64.0.2")
        .expect("Taildrop peer listed");
    assert_eq!(ipad["state"], "ready");
    let laptop = devices
        .iter()
        .find(|d| d["id"] == "taildrop:100.64.0.3")
        .unwrap();
    assert_eq!(laptop["state"], "offline");
    assert!(!f.path("gio-calls").exists(), "listing mounted something");
}

#[test]
fn an_untrusted_iphone_is_reported_with_what_the_person_must_do() {
    let f = Fixture::new("trust");
    let mut s = f.start();
    let (r, notes) = s.tool(
        "list_device_folders",
        json!({"device": format!("apple:{UDID}"), "wait_seconds": 0}),
    );
    assert_eq!(error_code(&r), "needs_trust");
    let progress: Vec<&Value> = notes
        .iter()
        .filter(|n| n["method"] == "notifications/progress")
        .collect();
    assert_eq!(progress.len(), 1, "{notes:?}");
    assert_eq!(progress[0]["params"]["progressToken"], "p");
    assert_eq!(
        progress[0]["params"]["message"],
        "Unlock your Apple device and tap Trust"
    );
}

#[test]
fn a_trusted_iphone_lists_its_app_folders_and_receives_a_copy_that_never_replaces() {
    let f = Fixture::new("send");
    f.trust();
    let mut s = f.start();
    let id = format!("apple:{UDID}");
    let (r, _) = s.tool("list_device_folders", json!({"device": id}));
    let out = structured(&r);
    assert_eq!(
        out["folders"],
        json!([{"path": "Documents", "name": "Documents"}, {"path": "VLC", "name": "VLC"}])
    );
    assert_eq!(out["truncated"], false);

    let photo = f.path("home/Pictures/beach.jpg");
    let send = json!({"device": id, "paths": [photo], "folder": "VLC"});
    let (r, _) = s.tool("send_files", send.clone());
    let out = structured(&r);
    assert_eq!(
        out["sent"],
        json!([{"source": photo, "name": "beach.jpg", "bytes": 10}])
    );
    assert_eq!(out["transport"], "usb");
    assert_eq!(
        std::fs::read(f.apps().join("VLC/beach.jpg")).unwrap(),
        b"jpeg bytes"
    );

    // The same name again: the file on the phone is kept, the new one renamed.
    std::fs::write(f.apps().join("VLC/beach.jpg"), b"the phone's own").unwrap();
    let (r, _) = s.tool("send_files", send.clone());
    let out = structured(&r);
    let renamed = out["sent"][0]["name"].as_str().unwrap().to_string();
    assert_ne!(renamed, "beach.jpg");
    assert_eq!(
        std::fs::read(f.apps().join("VLC/beach.jpg")).unwrap(),
        b"the phone's own"
    );
    assert_eq!(
        std::fs::read(f.apps().join("VLC").join(&renamed)).unwrap(),
        b"jpeg bytes"
    );

    // Skip leaves it alone and says so.
    let mut skip = send.clone();
    skip["on_conflict"] = json!("skip");
    let (r, _) = s.tool("send_files", skip);
    let out = structured(&r);
    assert_eq!(out["sent"], json!([]));
    assert_eq!(out["skipped"], json!([photo]));
    assert_eq!(
        std::fs::read(f.apps().join("VLC/beach.jpg")).unwrap(),
        b"the phone's own"
    );
    // The source is never touched.
    assert_eq!(std::fs::read(&photo).unwrap(), b"jpeg bytes");
}

#[test]
fn refused_sends_change_nothing_and_say_why() {
    let f = Fixture::new("refuse");
    f.trust();
    let mut s = f.start();
    let id = format!("apple:{UDID}");
    let photo = f.path("home/Pictures/beach.jpg");
    let key = f.path("home/.ssh/id_ed25519");
    let cases = [
        (
            json!({"device": id, "paths": [key], "folder": "VLC"}),
            "source_refused",
        ),
        (
            json!({"device": id, "paths": ["/etc/hostname"], "folder": "VLC"}),
            "source_refused",
        ),
        (
            json!({"device": id, "paths": [photo, photo], "folder": "VLC"}),
            "source_refused",
        ),
        (json!({"device": id, "paths": [photo]}), "folder_required"),
        (
            json!({"device": id, "paths": [photo], "folder": "../.."}),
            "folder_not_found",
        ),
        (
            json!({"device": id, "paths": [photo], "folder": "Pages"}),
            "folder_not_found",
        ),
        (
            json!({"device": "apple:0000", "paths": [photo], "folder": "VLC"}),
            "unknown_device",
        ),
        (
            json!({"device": "taildrop:100.64.0.3", "paths": [photo]}),
            "device_offline",
        ),
        (
            json!({"device": id, "paths": [photo], "folder": "VLC", "replace": true}),
            "invalid_arguments",
        ),
        (
            json!({"device": id, "paths": [photo], "folder": "VLC", "on_conflict": "replace"}),
            "invalid_arguments",
        ),
    ];
    for (args, code) in cases {
        let (r, _) = s.tool("send_files", args.clone());
        assert_eq!(error_code(&r), code, "{args}");
    }
    let landed: Vec<_> = std::fs::read_dir(f.apps().join("VLC")).unwrap().collect();
    assert!(landed.is_empty(), "a refused send wrote to the phone");
}

#[test]
fn a_taildrop_send_hands_each_file_to_tailscale_and_says_where_it_lands() {
    let f = Fixture::new("taildrop");
    let mut s = f.start();
    let photo = f.path("home/Pictures/beach.jpg");
    let (r, _) = s.tool(
        "send_files",
        json!({"device": "taildrop:100.64.0.2", "paths": [photo]}),
    );
    let out = structured(&r);
    assert_eq!(out["transport"], "taildrop");
    assert_eq!(out["sent"][0]["name"], "beach.jpg");
    assert!(out["next_step"]
        .as_str()
        .unwrap()
        .contains("Files › Tailscale"));
    assert_eq!(
        std::fs::read_to_string(f.path("taildrop-calls")).unwrap(),
        "100.64.0.2:\n"
    );
    assert_eq!(
        std::fs::read(f.path("taildrop/beach.jpg")).unwrap(),
        b"jpeg bytes"
    );
}

#[test]
fn a_cancelled_call_gets_no_answer_and_stops_waiting_for_the_phone() {
    let f = Fixture::new("cancel");
    let mut s = f.start();
    s.next += 1;
    let id = s.next;
    s.send(
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": "list_device_folders",
            "arguments": {"device": format!("apple:{UDID}"), "wait_seconds": 120},
            "_meta": meta(),
        }}),
    );
    // Wait until the stand-in is asking for Trust, then cancel.
    let asked = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        f.path("gio-calls").exists()
    });
    assert!(asked);
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id}}));
    // Nothing more arrives for the cancelled request…
    match s.rx.recv_timeout(Duration::from_secs(4)) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("a cancelled request was answered: {other:?}"),
    }
    // …and the server keeps serving.
    let (v, _) = s.request("server/discover", json!({"_meta": meta()}));
    assert!(v.get("result").is_some());
}

fn error_details(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().unwrap();
    serde_json::from_str::<Value>(text).unwrap()["error"]["details"].clone()
}

#[test]
fn a_partial_taildrop_send_is_an_error_that_still_lists_what_arrived() {
    let f = Fixture::new("partial");
    let mut s = f.start();
    let good = f.path("home/Pictures/beach.jpg");
    let bad = f.path("home/Pictures/fail.jpg");
    std::fs::write(&bad, b"x").unwrap();
    let (r, _) = s.tool(
        "send_files",
        json!({"device": "taildrop:100.64.0.2", "paths": [good, bad]}),
    );
    // Taildrop cannot say whether a refused file partly arrived.
    assert_eq!(error_code(&r), "delivery_uncertain");
    let d = error_details(&r);
    assert_eq!(d["sent"][0]["name"], "beach.jpg");
    assert_eq!(d["failed"][0]["source"], json!(bad));
    assert_eq!(d["delivery_uncertain"], true);
}

#[test]
fn a_source_swapped_while_waiting_for_trust_is_refused_and_nothing_is_sent() {
    let f = Fixture::new("swap");
    let mut s = f.start();
    let photo = f.path("home/Pictures/beach.jpg");
    s.next += 1;
    let id = s.next;
    s.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
        "name": "send_files",
        "arguments": {"device": format!("apple:{UDID}"), "paths": [photo], "folder": "VLC", "wait_seconds": 60},
        "_meta": meta(),
    }}));
    let asked = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        f.path("gio-calls").exists()
    });
    assert!(asked);
    // While the phone waits for Trust, the file becomes a folder of secrets.
    std::fs::remove_file(&photo).unwrap();
    std::fs::create_dir_all(photo.join(".ssh")).unwrap();
    std::fs::write(photo.join(".ssh/id_ed25519"), b"secret").unwrap();
    f.trust();
    let (v, _) = s.response_to(id);
    assert_eq!(error_code(&v["result"]), "source_refused");
    let landed: Vec<_> = std::fs::read_dir(f.apps().join("VLC")).unwrap().collect();
    assert!(landed.is_empty(), "a swapped source reached the phone");
}

#[test]
fn an_oversized_message_is_refused_and_the_server_reads_on() {
    let f = Fixture::new("oversized");
    let mut s = f.start();
    let big = "a".repeat(6 * 1024 * 1024);
    s.raw(&big);
    let v = s.rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(v["error"]["code"], -32600);
    let (v, _) = s.request("server/discover", json!({"_meta": meta()}));
    assert!(v.get("result").is_some());
}

#[test]
fn a_client_that_stops_reading_stdout_cannot_keep_the_server_from_exiting() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_gust-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Far more answers than a pipe holds, and nobody reads them.
    let mut stdin = child.stdin.take().unwrap();
    for id in 0..100 {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {"_meta": meta()}});
        writeln!(stdin, "{req}").unwrap();
    }
    drop(stdin);
    let exited = (0..160).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        matches!(child.try_wait(), Ok(Some(_)))
    });
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        exited,
        "a full stdout kept the server from seeing stdin close"
    );
}
