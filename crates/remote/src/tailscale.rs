//! Tailscale devices as places (DESIGN §6.4.1).
//!
//! The list comes from the LOCAL tailscale daemon (`tailscale status
//! --json` talks to tailscaled over its Unix socket). Reading it contacts
//! no device: discovery never connects, exactly as with the SSH config.
//! Connecting is an ordinary sshfs mount to the device's MagicDNS name (or
//! its Tailscale address), with the user's own SSH keys.
//!
//! The first connection to a device whose SSH host key is not on record is
//! not silently trusted: its fingerprint is shown and the user decides.
//! Tailscale has already authenticated the device itself, so this is a
//! check of the SSH key that device serves, written to known_hosts only
//! when the user says yes.

use crate::helper::{self, CallContext};
use gust_core::lister::CancelToken;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// One device on the tailnet, other than this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailnetPeer {
    /// Short name: the first label of its MagicDNS name, else its hostname.
    pub name: String,
    /// What to connect to: the MagicDNS name when MagicDNS is on, else the
    /// first Tailscale IPv4 address.
    pub target: String,
    /// Its Tailscale addresses (for matching against the SSH config).
    pub ips: Vec<String>,
    /// Full MagicDNS name without the trailing dot, if any.
    pub dns_name: Option<String>,
    pub os: String,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailnetError {
    /// The `tailscale` command is not installed.
    NotInstalled,
    /// Installed, but not logged in / not running.
    NotRunning(String),
    /// Anything else, already readable.
    Other(String),
}

impl TailnetError {
    pub fn message(&self) -> String {
        match self {
            TailnetError::NotInstalled => "Tailscale is not installed".into(),
            TailnetError::NotRunning(state) => {
                format!("Tailscale is not connected ({state}) — run tailscale up")
            }
            TailnetError::Other(w) => w.clone(),
        }
    }
}

/// Last successful local inventory, retained only for this process. A stopped
/// VPN must not erase known connection choices or claim cached peers are online.
#[derive(Default)]
pub struct PeerCache {
    peers: Vec<TailnetPeer>,
    available: bool,
}

pub struct PeerSnapshot {
    pub peers: Vec<TailnetPeer>,
    pub available: bool,
    pub error: String,
}

impl PeerCache {
    pub fn update(&mut self, result: Result<Vec<TailnetPeer>, TailnetError>) -> PeerSnapshot {
        match result {
            Ok(peers) => {
                self.peers = peers.clone();
                self.available = true;
                PeerSnapshot {
                    peers,
                    available: true,
                    error: String::new(),
                }
            }
            Err(error) => {
                if matches!(
                    error,
                    TailnetError::NotRunning(_) | TailnetError::NotInstalled
                ) {
                    self.available = false;
                }
                let mut peers = self.peers.clone();
                if !self.available {
                    for peer in &mut peers {
                        peer.online = false;
                    }
                }
                PeerSnapshot {
                    peers,
                    available: self.available,
                    error: error.message(),
                }
            }
        }
    }
}

/// Read the tailnet from the local daemon. Bounded: a wedged daemon gives
/// an error after `timeout`, never a hung caller.
pub fn peers(timeout: Duration) -> Result<Vec<TailnetPeer>, TailnetError> {
    peers_command(
        Command::new("tailscale").args(["status", "--json"]),
        timeout,
        None,
    )
}

/// Cancellable local discovery. Quiet commands observe cancellation within
/// 20ms plus scheduler latency; no tailnet device is contacted.
pub fn peers_with_cancel(
    timeout: Duration,
    cancel: &gust_core::lister::CancelToken,
) -> Result<Vec<TailnetPeer>, TailnetError> {
    peers_command(
        Command::new("tailscale").args(["status", "--json"]),
        timeout,
        Some(cancel),
    )
}

fn peers_command(
    command: &mut Command,
    timeout: Duration,
    cancel: Option<&gust_core::lister::CancelToken>,
) -> Result<Vec<TailnetPeer>, TailnetError> {
    let output = crate::command::capture_stdout(command, timeout, 8 * 1024 * 1024, cancel)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => TailnetError::NotInstalled,
            std::io::ErrorKind::TimedOut => {
                TailnetError::Other("Tailscale did not answer in time".into())
            }
            std::io::ErrorKind::Interrupted => {
                TailnetError::Other("Tailscale discovery was cancelled".into())
            }
            _ => TailnetError::Other(error.to_string()),
        })?;
    // Still parse BackendState on failure: tailscale reports NeedsLogin in
    // valid JSON even when status exits unsuccessfully.
    parse_status(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `tailscale status --json`. Pure, so it is tested without a tailnet.
pub fn parse_status(json: &str) -> Result<Vec<TailnetPeer>, TailnetError> {
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|_| TailnetError::Other("Tailscale gave an unreadable answer".into()))?;
    let state = v.get("BackendState").and_then(|s| s.as_str()).unwrap_or("");
    if state != "Running" {
        let s = if state.is_empty() { "stopped" } else { state };
        return Err(TailnetError::NotRunning(s.to_string()));
    }
    let magic = v
        .get("CurrentTailnet")
        .and_then(|t| t.get("MagicDNSEnabled"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let mut out = Vec::new();
    if let Some(peers) = v.get("Peer").and_then(|p| p.as_object()) {
        for p in peers.values() {
            let host = p.get("HostName").and_then(|s| s.as_str()).unwrap_or("");
            let dns = p
                .get("DNSName")
                .and_then(|s| s.as_str())
                .map(|s| s.trim_end_matches('.').to_string())
                .filter(|s| !s.is_empty());
            let ips: Vec<String> = p
                .get("TailscaleIPs")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let v4 = ips.iter().find(|ip| !ip.contains(':')).cloned();
            let name = dns
                .as_deref()
                .and_then(|d| d.split('.').next())
                .filter(|s| !s.is_empty())
                .unwrap_or(host)
                .to_string();
            let target = match (&dns, magic, &v4) {
                (Some(d), true, _) => d.clone(),
                (_, _, Some(ip)) => ip.clone(),
                (Some(d), false, None) => d.clone(),
                (None, _, None) => continue,
            };
            if !safe_name(&name) || !safe_target(&target) {
                continue;
            }
            out.push(TailnetPeer {
                name,
                target,
                ips,
                dns_name: dns,
                os: p
                    .get("OS")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(16)
                    .collect(),
                online: p.get("Online").and_then(|b| b.as_bool()).unwrap_or(false),
            });
        }
    }
    out.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out.dedup_by(|a, b| a.name == b.name);
    Ok(out)
}

/// A device name is shown and becomes part of a mount point: letters,
/// digits, `-`, `_` only, and never empty.
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A connection target goes to ssh as one argument: it must not be able to
/// look like an option or carry a user/path.
pub fn safe_target(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == ':')
}

/// A login name typed by the user: what a Unix account name may contain.
pub fn safe_user(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Where a device is mounted: `…/gust/tailnet/<name>`, beside SSH places
/// (`…/gust/places/<alias>`), so a device and an SSH alias of the same name
/// cannot collide and the folder still reads as the device's own name.
pub fn mount_point(name: &str) -> Option<PathBuf> {
    if !safe_name(name) {
        return None;
    }
    let places = crate::mount::mount_point_for("x")?;
    Some(places.parent()?.parent()?.join("tailnet").join(name))
}

// --- host keys --------------------------------------------------------------

/// The user's known_hosts: `$HOME/.ssh/known_hosts`.
pub fn known_hosts_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ssh/known_hosts"))
}

/// Is a key for `host` already on record? `ssh-keygen -F` reads the file,
/// hashed entries included; nothing touches the network.
pub fn host_known(known_hosts: &Path, host: &str) -> bool {
    if !known_hosts.is_file() {
        return false;
    }
    let mut command = Command::new("ssh-keygen");
    command.arg("-F").arg(host).arg("-f").arg(known_hosts);
    helper::run(
        command,
        &[],
        &CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(3),
        },
    )
    .map(|out| out.status.success())
    .unwrap_or(false)
}

/// The SSH host keys a device presents, for the user to decide on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedKeys {
    /// known_hosts lines, exactly as `ssh-keyscan` printed them.
    pub lines: Vec<String>,
    /// `ED25519\nSHA256:…` per key (type above hash), for the dialog.
    pub fingerprints: Vec<String>,
}

/// Ask the device for its host keys (the first network traffic, and only
/// on an explicit Connect). Bounded by ssh-keyscan's own timeout.
pub fn scan_host_keys(host: &str) -> Result<ScannedKeys, String> {
    scan_host_keys_with_context(
        host,
        &CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(10),
        },
    )
}
pub fn scan_host_keys_with_context(
    host: &str,
    ctx: &CallContext<'_>,
) -> Result<ScannedKeys, String> {
    let started = std::time::Instant::now();
    if !safe_target(host) {
        return Err("That is not a usable address".into());
    }
    let mut command = Command::new("ssh-keyscan");
    command.args(["-T", "6", "-t", "ed25519,ecdsa,rsa", "--", host]);
    let out = helper::run(command, &[], ctx).map_err(|e| e.message)?;
    let lines = keyscan_lines(&String::from_utf8_lossy(&out.stdout), host);
    if lines.is_empty() {
        return Err("That device did not answer on SSH. Is SSH turned on there?".into());
    }
    let mut fingerprints = Vec::new();
    for l in &lines {
        let remaining = CallContext {
            cancel: ctx.cancel,
            timeout: ctx.timeout.saturating_sub(started.elapsed()),
        };
        if let Some(fp) = fingerprint(l, &remaining) {
            fingerprints.push(fp);
        }
    }
    Ok(ScannedKeys {
        lines,
        fingerprints,
    })
}

/// Keep only well-formed `host keytype base64` lines for exactly `host`.
pub fn keyscan_lines(text: &str, host: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| {
            let mut f = l.split_whitespace();
            let (Some(h), Some(t), Some(k), None) = (f.next(), f.next(), f.next(), f.next()) else {
                return false;
            };
            h == host
                && (t.starts_with("ssh-") || t.starts_with("ecdsa-"))
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        })
        .map(str::to_string)
        .collect()
}

fn fingerprint(line: &str, ctx: &CallContext<'_>) -> Option<String> {
    let mut command = Command::new("ssh-keygen");
    command.args(["-l", "-f", "-"]);
    let out = helper::run(command, line.as_bytes(), ctx).ok()?;
    // "256 SHA256:abc… host (ED25519)"
    let s = String::from_utf8_lossy(&out.stdout);
    let mut f = s.split_whitespace();
    let _bits = f.next()?;
    let fp = f.next()?;
    let kind = s.split_whitespace().last()?;
    // The key type on its own line above its hash: joined on one line, the
    // 50-character hash filled the dialog's width and "(ED25519)" wrapped
    // onto a line of its own, looking like a separate key.
    let kind = kind.trim_start_matches('(').trim_end_matches(')');
    Some(format!("{kind}\n{fp}"))
}

/// Record keys the user accepted. Appends; never rewrites the file.
pub fn trust_host_keys(known_hosts: &Path, lines: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(dir) = known_hosts.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc_o_nofollow())
        .open(known_hosts)?;
    for l in lines {
        writeln!(f, "{l}")?;
    }
    f.sync_all()
}

const fn libc_o_nofollow() -> i32 {
    0o400000 // O_NOFOLLOW on Linux
}

// --- remembered login names -------------------------------------------------

const MAX_USERS: usize = 200;

/// `$XDG_STATE_HOME/gust/tailnet-users` (or `~/.local/state/…`).
pub fn users_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x).join("gust/tailnet-users"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state/gust/tailnet-users"))
}

/// The login name last used for `device`, if any.
pub fn remembered_user(path: &Path, device: &str) -> Option<String> {
    read_users(path)
        .into_iter()
        .find(|(d, _)| d == device)
        .map(|(_, u)| u)
}

fn read_users(path: &Path) -> Vec<(String, String)> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Vec::new();
    };
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return Vec::new();
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (d, u) = l.split_once('\t')?;
            (safe_name(d) && safe_user(u)).then(|| (d.to_string(), u.to_string()))
        })
        .take(MAX_USERS)
        .collect()
}

/// Remember `user` for `device`, newest first, bounded, 0600, staged.
pub fn remember_user(path: &Path, device: &str, user: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if !safe_name(device) || !safe_user(user) {
        return Ok(());
    }
    let mut all = read_users(path);
    all.retain(|(d, _)| d != device);
    all.insert(0, (device.to_string(), user.to_string()));
    all.truncate(MAX_USERS);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        for (d, u) in &all {
            writeln!(f, "{d}\t{u}")?;
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{
      "BackendState": "Running",
      "CurrentTailnet": {"Name": "me@example", "MagicDNSSuffix": "tail1.ts.net", "MagicDNSEnabled": true},
      "Self": {"HostName": "desk", "DNSName": "desk.tail1.ts.net.", "TailscaleIPs": ["100.64.0.1"], "Online": true},
      "Peer": {
        "nodekey:a": {"HostName": "Media Box", "DNSName": "nas-box.tail1.ts.net.", "OS": "linux",
                      "TailscaleIPs": ["100.64.0.2", "fd7a::2"], "Online": true},
        "nodekey:b": {"HostName": "old", "DNSName": "old-laptop.tail1.ts.net.", "OS": "windows",
                      "TailscaleIPs": ["100.64.0.3"], "Online": false},
        "nodekey:c": {"HostName": "studio", "DNSName": "studio.tail1.ts.net.", "OS": "macOS",
                      "TailscaleIPs": ["100.64.0.4"], "Online": true},
        "nodekey:d": {"HostName": "evil", "DNSName": "-oProxyCommand=x.tail1.ts.net.", "OS": "linux",
                      "TailscaleIPs": ["100.64.0.9"], "Online": true}
      }
    }"#;

    #[test]
    fn peers_come_from_the_status_and_exclude_this_computer() {
        let p = parse_status(STATUS).unwrap();
        let names: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        // Online first, then by name; this computer is not a place.
        assert_eq!(names, ["nas-box", "studio", "old-laptop"]);
        assert_eq!(p[0].target, "nas-box.tail1.ts.net");
        assert_eq!(p[0].os, "linux");
        assert!(p[0].online && !p[2].online);
    }

    #[test]
    fn local_discovery_drains_stdout_without_waiting_for_stderr() {
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "head -c 131072 /dev/zero >&2; printf '%s' \"$1\"",
                "fixture",
            ])
            .arg(STATUS);
        assert_eq!(
            peers_command(&mut command, Duration::from_secs(1), None).unwrap(),
            parse_status(STATUS).unwrap()
        );
    }

    #[test]
    fn local_discovery_keeps_stopped_state_on_unsuccessful_exit() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf '%s' '{\"BackendState\":\"NeedsLogin\"}'; exit 1",
        ]);
        assert_eq!(
            peers_command(&mut command, Duration::from_secs(1), None),
            Err(TailnetError::NotRunning("NeedsLogin".into()))
        );
    }

    #[test]
    fn local_discovery_refuses_oversized_answers_and_pre_cancel() {
        let mut flood = Command::new("head");
        flood.args(["-c", "8388609", "/dev/zero"]);
        let error = peers_command(&mut flood, Duration::from_secs(2), None).unwrap_err();
        assert!(error.message().contains("output exceeded limit"));
        let cancel = gust_core::lister::CancelToken::new();
        cancel.cancel();
        assert_eq!(
            peers_with_cancel(Duration::from_secs(1), &cancel),
            Err(TailnetError::Other(
                "Tailscale discovery was cancelled".into()
            ))
        );
    }

    #[test]
    fn a_name_that_looks_like_an_option_is_dropped() {
        let p = parse_status(STATUS).unwrap();
        assert!(p
            .iter()
            .all(|x| !x.target.starts_with('-') && !x.name.starts_with('-')));
    }

    #[test]
    fn without_magic_dns_the_address_is_the_target() {
        let s = STATUS.replace("\"MagicDNSEnabled\": true", "\"MagicDNSEnabled\": false");
        let p = parse_status(&s).unwrap();
        assert_eq!(p[0].target, "100.64.0.2");
    }

    #[test]
    fn stopped_tailnet_retains_routes_but_marks_them_unavailable() {
        let mut cache = PeerCache::default();
        let running = cache.update(parse_status(STATUS));
        assert!(running.available);
        assert_eq!(running.peers.len(), 3);
        let stopped = cache.update(Err(TailnetError::NotRunning("Stopped".into())));
        assert!(!stopped.available);
        assert_eq!(stopped.peers.len(), 3);
        assert!(stopped.peers.iter().all(|p| !p.online));
        assert!(stopped.error.contains("tailscale up"));
        // A new successful inventory replaces, rather than resurrects, old devices.
        let empty = cache.update(Ok(Vec::new()));
        assert!(empty.available);
        assert!(empty.peers.is_empty());
    }

    #[test]
    fn a_transient_daemon_error_preserves_the_last_known_availability() {
        let mut cache = PeerCache::default();
        let peer = TailnetPeer {
            name: "studio".into(),
            target: "100.64.0.2".into(),
            ips: vec!["100.64.0.2".into()],
            dns_name: None,
            os: "linux".into(),
            online: true,
        };
        cache.update(Ok(vec![peer]));
        let transient = cache.update(Err(TailnetError::Other("Timed out".into())));
        assert!(transient.available);
        assert!(transient.peers[0].online);
        cache.update(Err(TailnetError::NotRunning("Stopped".into())));
        assert!(
            !cache
                .update(Err(TailnetError::Other("Timed out".into())))
                .available
        );
    }

    #[test]
    fn a_stopped_daemon_says_so() {
        let e = parse_status(r#"{"BackendState":"NeedsLogin"}"#).unwrap_err();
        assert_eq!(e, TailnetError::NotRunning("NeedsLogin".into()));
        assert!(e.message().contains("tailscale up"));
        assert!(matches!(
            parse_status("not json"),
            Err(TailnetError::Other(_))
        ));
    }

    #[test]
    fn keyscan_output_is_filtered_to_real_key_lines_for_the_host() {
        let text = "# nas:22 SSH-2.0-OpenSSH_9.9\n\
                    nas ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIABC+/=\n\
                    other ssh-ed25519 AAAAC3Nz\n\
                    nas ssh-ed25519 AAAA;rm -rf\n\
                    nas ecdsa-sha2-nistp256 AAAAE2Vj\n";
        let l = keyscan_lines(text, "nas");
        assert_eq!(l.len(), 2);
        assert!(l[0].starts_with("nas ssh-ed25519 "));
    }

    #[test]
    fn user_names_are_checked() {
        assert!(safe_user("media") && safe_user("first.last"));
        assert!(!safe_user("-oProxyCommand=x") && !safe_user("a b") && !safe_user("a@b"));
    }

    #[test]
    fn remembered_users_round_trip_bounded_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("gust-ts-users-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("gust/tailnet-users");
        assert_eq!(remembered_user(&p, "nas-box"), None);
        remember_user(&p, "nas-box", "media").unwrap();
        remember_user(&p, "studio", "me").unwrap();
        remember_user(&p, "nas-box", "admin").unwrap();
        assert_eq!(remembered_user(&p, "nas-box").as_deref(), Some("admin"));
        assert_eq!(remembered_user(&p, "studio").as_deref(), Some("me"));
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // Junk in the file is ignored, never trusted.
        std::fs::write(&p, "nas-box\t-oProxyCommand=x\nok\tfine\n").unwrap();
        assert_eq!(remembered_user(&p, "nas-box"), None);
        assert_eq!(remembered_user(&p, "ok").as_deref(), Some("fine"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trusted_keys_are_appended_never_rewritten() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("gust-ts-kh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let kh = d.join(".ssh/known_hosts");
        std::fs::create_dir_all(kh.parent().unwrap()).unwrap();
        std::fs::write(&kh, "# mine\nold ssh-ed25519 AAAA\n").unwrap();
        trust_host_keys(&kh, &["nas ssh-ed25519 BBBB".to_string()]).unwrap();
        let t = std::fs::read_to_string(&kh).unwrap();
        assert!(t.starts_with("# mine\nold ssh-ed25519 AAAA\n"));
        assert!(t.ends_with("nas ssh-ed25519 BBBB\n"));
        // A fresh file is created private.
        let kh2 = d.join("fresh/.ssh/known_hosts");
        trust_host_keys(&kh2, &["x ssh-ed25519 C".to_string()]).unwrap();
        let mode = std::fs::metadata(&kh2).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn device_mount_points_cannot_collide_with_ssh_aliases() {
        let a = mount_point("studio").unwrap();
        let b = crate::mount::mount_point_for("studio").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("tailnet/studio"));
        assert_eq!(mount_point("../x"), None);
    }
}
