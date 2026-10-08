//! Mounting a remote place, safely and on somebody's explicit instruction.
//!
//! Discovery produces *suggestions*; this module is the only thing that
//! opens a connection, and only when asked (docs/DESIGN.md §6.4).
//!
//! The rules that shape every function here:
//!
//! - host identity is verified, never bypassed;
//! - no secret reaches argv, the environment, a log or a config file;
//! - a blocked mount must not block Gust, so everything is bounded;
//! - "mounted" is only claimed after the mount point is observably a
//!   different filesystem, not because a command exited zero.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a remote place is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Another computer over SSH. Uses the user's existing keys.
    Sshfs,
}

impl Backend {
    pub fn tool(self) -> &'static str {
        match self {
            Backend::Sshfs => "sshfs",
        }
    }

    /// Whether the tool this backend needs is installed.
    pub fn available(self) -> bool {
        which(self.tool()).is_some()
    }
}

fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
}

/// What a remote place needs to connect. Never contains a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSpec {
    /// Label shown in the sidebar, e.g. `studio`.
    pub alias: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Path on the remote machine. `None` means the login directory.
    pub remote_path: Option<String>,
    /// Use the alias verbatim as the ssh target instead of `user@host`.
    ///
    /// An alias names a whole `Host` block: IdentityFile, UserKnownHostsFile,
    /// Port, ProxyJump. Resolving it to a bare hostname silently discards all
    /// of that, and the connection then fails in a way that looks like a
    /// changed host key. When the alias came from the user's config, it is
    /// the only correct target.
    pub use_alias_as_target: bool,
    /// An explicit identity file, by PATH. The key itself never appears here.
    pub identity_file: Option<PathBuf>,
    /// An alternate ssh_config to read (used by tests; normally the user's).
    pub config_file: Option<PathBuf>,
    /// A known_hosts file to verify against.
    pub known_hosts: Option<PathBuf>,
}

impl RemoteSpec {
    pub fn new(alias: impl Into<String>, host: impl Into<String>) -> Self {
        RemoteSpec {
            alias: alias.into(),
            host: host.into(),
            user: None,
            port: None,
            remote_path: None,
            identity_file: None,
            config_file: None,
            known_hosts: None,
            use_alias_as_target: false,
        }
    }

    /// What sshfs expects as its source.
    ///
    /// With `use_alias_as_target`, this is `alias:/path` so ssh applies the
    /// whole `Host` block. Otherwise `user@host:/path`.
    pub fn source(&self) -> String {
        let mut s = String::new();
        if self.use_alias_as_target {
            s.push_str(&self.alias);
            s.push(':');
            s.push_str(self.remote_path.as_deref().unwrap_or(""));
            return s;
        }
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&self.host);
        s.push(':');
        s.push_str(self.remote_path.as_deref().unwrap_or(""));
        s
    }
}

/// Why a connection did not happen. Each maps to something a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountError {
    /// The tool is not installed.
    ToolMissing(&'static str),
    /// The host key did not match what is on record.
    HostKeyChanged,
    /// The server refused our credentials.
    AuthFailed,
    /// No answer in the time allowed.
    Timeout,
    /// The mount point is unusable.
    BadMountPoint(String),
    /// Anything else, already made readable.
    Other(String),
}

impl MountError {
    /// A sentence for somebody who does not use a terminal.
    pub fn message(&self) -> String {
        match self {
            MountError::ToolMissing(t) => {
                format!("Connecting to other computers needs {t} — install it to enable this")
            }
            MountError::HostKeyChanged => "This computer's identity has changed. \
                 Connection refused — check with whoever runs it before trying again."
                .into(),
            MountError::AuthFailed => {
                "That computer refused the login. Check the username and your SSH key.".into()
            }
            MountError::Timeout => {
                "That computer did not answer in time. It may be offline.".into()
            }
            MountError::BadMountPoint(w) => format!("Cannot use the folder for this place: {w}"),
            MountError::Other(w) => w.clone(),
        }
    }
}

/// Ask ssh (not sshfs) whether the host key verifies.
///
/// sshfs reports a rejected connection as `read: Connection reset by peer`,
/// which is the symptom, not the cause. A bounded `ssh -o BatchMode` probe
/// gets the real reason so a changed host key is never shown as a vague
/// network error. Never disables any checking; it only asks.
fn host_key_verdict(spec: &RemoteSpec) -> Option<MountError> {
    let mut c = Command::new("ssh");
    c.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg("-o")
        .arg("PasswordAuthentication=no");
    if let Some(p) = spec.port {
        c.arg("-p").arg(p.to_string());
    }
    if let Some(id) = &spec.identity_file {
        c.arg("-o").arg(format!("IdentityFile={}", id.display()));
        c.arg("-o").arg("IdentitiesOnly=yes");
    }
    if let Some(kh) = &spec.known_hosts {
        c.arg("-o")
            .arg(format!("UserKnownHostsFile={}", kh.display()));
    }
    if let Some(cfg) = &spec.config_file {
        c.arg("-F").arg(cfg);
    }
    let target = match &spec.user {
        Some(u) => format!("{u}@{}", spec.host),
        None => spec.host.clone(),
    };
    let out = c
        .arg(target)
        .arg("true")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        return None;
    }
    let err = String::from_utf8_lossy(&out.stderr);
    match classify(&err) {
        // Only report the diagnoses this probe can establish.
        e @ (MountError::HostKeyChanged | MountError::AuthFailed) => Some(e),
        _ => None,
    }
}

/// Classify sshfs/ssh stderr into something actionable.
///
/// Deliberately conservative: anything unrecognised stays `Other` with the
/// original text rather than being guessed at.
pub fn classify(stderr: &str) -> MountError {
    let low = stderr.to_ascii_lowercase();
    // Order matters: a changed host key also mentions "host key".
    if low.contains("remote host identification has changed")
        || low.contains("host key verification failed")
        || low.contains("host key for") && low.contains("changed")
    {
        return MountError::HostKeyChanged;
    }
    if low.contains("permission denied")
        || low.contains("authentication failed")
        || low.contains("too many authentication failures")
    {
        return MountError::AuthFailed;
    }
    if low.contains("connection timed out")
        || low.contains("operation timed out")
        || low.contains("timeout")
    {
        return MountError::Timeout;
    }
    if low.contains("connection refused") {
        return MountError::Other("That computer refused the connection.".into());
    }
    if low.contains("could not resolve hostname") || low.contains("name or service not known") {
        return MountError::Other("That computer's name could not be looked up.".into());
    }
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        MountError::Other("The connection failed.".into())
    } else {
        // Keep the tool's own words rather than inventing a diagnosis.
        MountError::Other(trimmed.lines().last().unwrap_or(trimmed).to_string())
    }
}

/// Is `path` the root of a mounted filesystem?
///
/// Checked by comparing device numbers with the parent, which is true only
/// when something is really mounted there. A command exiting zero is not
/// evidence that a filesystem appeared.
pub fn is_mounted(path: &Path) -> bool {
    // The kernel's mount table, not `stat`: a FUSE mount whose server died
    // fails every stat with ENOTCONN/ECONNREFUSED, which read as "not
    // mounted" — so disconnect did nothing and every reconnect failed with
    // "File exists" until a reboot.
    if let Some(listed) = listed_in_mountinfo(path) {
        return listed;
    }
    use std::os::unix::fs::MetadataExt;
    let Ok(here) = std::fs::metadata(path) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(up) = std::fs::metadata(parent) else {
        return false;
    };
    here.dev() != up.dev()
}

/// Is `path` a mount point in `/proc/self/mountinfo`? `None` when the table
/// cannot be read (not Linux, or /proc hidden), so the caller falls back.
fn listed_in_mountinfo(path: &Path) -> Option<bool> {
    let table = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let want = path.to_string_lossy();
    let want = match want.trim_end_matches('/') {
        "" => "/",
        w => w,
    };
    Some(table.lines().any(|l| {
        // Field 5 is the mount point, with space, tab, newline and
        // backslash octal-escaped.
        l.split(' ')
            .nth(4)
            .is_some_and(|mp| unescape_mountinfo(mp) == want)
    }))
}

fn unescape_mountinfo(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 4 <= b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The arguments for a mount. Separated from running it so the safety
/// properties can be tested without a network.
///
/// Nothing secret is ever placed here: an identity is passed by PATH, and a
/// password would go to the agent, never to argv where `ps` would show it.
pub fn sshfs_args(spec: &RemoteSpec, mount_point: &Path, timeout_s: u32) -> Vec<String> {
    let mut a = vec![spec.source(), mount_point.to_string_lossy().into_owned()];

    // Host identity is verified. These are the two settings that would turn
    // that off, and they are never emitted; the test below pins that.
    a.push("-o".into());
    a.push("StrictHostKeyChecking=yes".into());

    if let Some(p) = spec.port {
        if !spec.use_alias_as_target {
            a.push("-o".into());
            a.push(format!("port={p}"));
        }
    }
    if let Some(id) = &spec.identity_file {
        a.push("-o".into());
        a.push(format!("IdentityFile={}", id.display()));
        // With an explicit identity, do not wander through the agent's keys:
        // several wrong keys in a row look like an attack to sshd.
        a.push("-o".into());
        a.push("IdentitiesOnly=yes".into());
    }
    if let Some(cfg) = &spec.config_file {
        a.push("-F".into());
        a.push(cfg.to_string_lossy().into_owned());
    }
    if let Some(kh) = &spec.known_hosts {
        a.push("-o".into());
        a.push(format!("UserKnownHostsFile={}", kh.display()));
    }

    // A blocked mount must not block Gust: fail rather than hang forever.
    a.push("-o".into());
    a.push(format!("ConnectTimeout={timeout_s}"));
    // Never prompt on a terminal Gust does not own.
    a.push("-o".into());
    a.push("BatchMode=yes".into());
    // Read-ahead and reconnect make a slow link usable, and `reconnect`
    // without `ServerAliveInterval` hides a dead server indefinitely.
    a.push("-o".into());
    a.push("reconnect".into());
    a.push("-o".into());
    a.push("ServerAliveInterval=15".into());
    a.push("-o".into());
    a.push("ServerAliveCountMax=3".into());
    a
}

/// Connect a remote place. Blocking, bounded, and never interactive.
///
/// Call this on a worker: it is the one function here that touches a network.
pub fn mount(spec: &RemoteSpec, mount_point: &Path, timeout: Duration) -> Result<(), MountError> {
    mount_recovering(spec, mount_point, timeout).map(|_| ())
}

/// [`mount`], also saying where files stranded in the mount point went.
///
/// `Ok(Some(dir))`: the place is connected, and files that were sitting in
/// the bare mount-point directory were moved to `dir` first (see
/// [`prepare_mount_point`]). The caller must tell the user.
pub fn mount_recovering(
    spec: &RemoteSpec,
    mount_point: &Path,
    timeout: Duration,
) -> Result<Option<PathBuf>, MountError> {
    if !Backend::Sshfs.available() {
        return Err(MountError::ToolMissing("sshfs"));
    }
    if is_mounted(mount_point) {
        return Ok(None); // already connected
    }
    let recovered = prepare_mount_point(mount_point, &spec.alias)?;
    mount_prepared(spec, mount_point, timeout)?;
    Ok(recovered)
}

/// Make `mount_point` an empty directory a filesystem can be mounted on.
///
/// A file written through a place moments before it was disconnected lands
/// in the bare mount-point directory. It may be the only copy of that work,
/// so it is NEVER deleted here. In one of Gust's own mount points it is moved
/// to `$XDG_DATA_HOME/gust/recovered/<place> <time>/` and that folder is
/// returned; anywhere else the mount is refused and nothing is touched.
/// Call on a worker.
pub fn prepare_mount_point(mount_point: &Path, label: &str) -> Result<Option<PathBuf>, MountError> {
    let Some(runtime) = runtime_base() else {
        return Err(MountError::BadMountPoint(
            "no runtime folder to mount in".into(),
        ));
    };
    let Some(recovered_root) = recovered_root() else {
        return Err(MountError::BadMountPoint(
            "no folder to keep recovered files in".into(),
        ));
    };
    prepare_mount_point_in(mount_point, &runtime, &recovered_root, label, &stamp_now())
}

/// [`prepare_mount_point`] with every location passed in, so it is tested
/// without touching the real runtime or data folders.
pub fn prepare_mount_point_in(
    mount_point: &Path,
    runtime: &Path,
    recovered_root: &Path,
    label: &str,
    stamp: &str,
) -> Result<Option<PathBuf>, MountError> {
    if let Err(e) = std::fs::create_dir_all(mount_point) {
        return Err(MountError::BadMountPoint(e.to_string()));
    }
    let leftovers: Vec<PathBuf> = match std::fs::read_dir(mount_point) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(e) => return Err(MountError::BadMountPoint(e.to_string())),
    };
    if leftovers.is_empty() {
        return Ok(None);
    }
    if !is_gust_mount_point_in(mount_point, runtime) {
        // Somebody else's folder: mounting over it would hide their files.
        return Err(MountError::BadMountPoint("the folder is not empty".into()));
    }
    let safe_label: String = label
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let mut dest = recovered_root.join(format!("{} {stamp}", safe_label.trim_matches('.')));
    let mut n = 2;
    while dest.exists() {
        dest = recovered_root.join(format!("{} {stamp} ({n})", safe_label.trim_matches('.')));
        n += 1;
    }
    {
        use std::os::unix::fs::DirBuilderExt;
        // Owner-only: what somebody was copying to a server is private.
        if let Err(e) = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dest)
        {
            return Err(MountError::BadMountPoint(format!(
                "files were left in the folder and could not be set aside ({e})"
            )));
        }
    }
    let cancel = gust_core::lister::CancelToken::default();
    for p in &leftovers {
        let Some(name) = p.file_name() else { continue };
        // move_tree renames when it can and otherwise copies, verifies and
        // only then removes the original: a failure leaves it where it was.
        if let Err(e) = gust_ops::recursive::move_tree(
            p,
            &dest.join(name),
            gust_ops::ConflictPolicy::KeepBoth,
            &cancel,
            &mut |_, _, _, _| {},
        ) {
            return Err(MountError::BadMountPoint(format!(
                "files were left in the folder and could not all be set aside ({e}); \
                 nothing was deleted, see {}",
                dest.display()
            )));
        }
    }
    Ok(Some(dest))
}

/// Where recovered files are kept: `$XDG_DATA_HOME/gust/recovered`. On disk,
/// not in the runtime folder, which is emptied at logout.
pub fn recovered_root() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("gust").join("recovered"))
}

/// Local time as `YYYY-MM-DD HH.MM`, safe in a file name.
fn stamp_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    gust_core::properties::full_time(now, gust_core::properties::local_offset_seconds())
        .replace(':', ".")
}

fn runtime_base() -> Option<PathBuf> {
    mount_point_for("probe")?
        .parent()?
        .parent()?
        .parent()
        .map(Path::to_path_buf)
}

fn mount_prepared(
    spec: &RemoteSpec,
    mount_point: &Path,
    timeout: Duration,
) -> Result<(), MountError> {
    let args = sshfs_args(spec, mount_point, timeout.as_secs().max(1) as u32);
    let mut child = Command::new("sshfs")
        .args(&args)
        // No terminal: a password prompt must never appear on Gust's stdin.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| MountError::Other(e.to_string()))?;

    // sshfs daemonises, so waiting on the child is bounded by our own clock.
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut err);
                }
                if status.success() {
                    break;
                }
                let first = classify(&err);
                // A vague transport error may really be a rejected host key.
                if matches!(first, MountError::Other(_)) {
                    if let Some(real) = host_key_verdict(spec) {
                        return Err(real);
                    }
                }
                return Err(first);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    gust_core::stall::reap_in_background(child);
                    return Err(MountError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(MountError::Other(e.to_string())),
        }
    }

    // "Mounted" is only true when the mount point is really a different
    // filesystem. sshfs can exit 0 and leave nothing behind.
    let settle = Instant::now() + Duration::from_secs(3);
    while Instant::now() < settle {
        if is_mounted(mount_point) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(MountError::Other(
        "The connection reported success but nothing was mounted.".into(),
    ))
}

/// Disconnect a remote place.
///
/// Never a lazy unmount by default: detaching a filesystem with unwritten
/// data is how people lose work (plan §9 — do not lazy-unmount dirty mounts).
pub fn unmount(mount_point: &Path) -> Result<(), MountError> {
    if !is_mounted(mount_point) {
        return Ok(());
    }
    // FUSE 3 renamed the tool. A system with only `fusermount3` would let
    // somebody connect and then never disconnect — found by testing the
    // unmount against a real mount rather than assuming it worked.
    let Some(tool) = ["fusermount3", "fusermount"]
        .into_iter()
        .find(|t| which(t).is_some())
    else {
        return Err(MountError::ToolMissing("fusermount3"));
    };
    let out = Command::new(tool)
        .args(["-u", &mount_point.to_string_lossy()])
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            if err.to_ascii_lowercase().contains("busy") {
                Err(MountError::Other(
                    "Something is still using this place. Close it and try again.".into(),
                ))
            } else {
                Err(classify(&err))
            }
        }
        Err(e) => Err(MountError::Other(e.to_string())),
    }
}

/// Is this one of Gust's own mount-point directories?
///
/// `<runtime>/gust/{places,tailnet,cloud}/<name>` qualify. Anywhere else — a
/// folder the user chose, a home directory — is never touched.
#[cfg(test)]
fn is_gust_mount_point(path: &Path) -> bool {
    runtime_base().is_some_and(|rt| is_gust_mount_point_in(path, &rt))
}

fn is_gust_mount_point_in(path: &Path, runtime: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if path.file_name().is_none() || path.components().any(|c| c.as_os_str() == "..") {
        return false;
    }
    ["places", "tailnet", "cloud"]
        .iter()
        .any(|kind| parent == runtime.join("gust").join(kind))
}

/// Where a place's mount point lives: under the user's runtime dir, never
/// inside their home where a stale mount would look like real folders.
pub fn mount_point_for(alias: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    // An alias is user input: keep it to a single safe path segment so it
    // cannot escape the base directory.
    let safe: String = alias
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_matches('.').to_string();
    if safe.is_empty() {
        return None;
    }
    Some(base.join("gust").join("places").join(safe))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RemoteSpec {
        let mut s = RemoteSpec::new("studio", "studio.example");
        s.user = Some("alex".into());
        s.port = Some(2222);
        s.identity_file = Some(PathBuf::from("/home/alex/.ssh/id_ed25519"));
        s
    }

    #[test]
    fn host_identity_checking_is_never_disabled() {
        let a = sshfs_args(&spec(), Path::new("/run/mnt"), 10).join(" ");
        assert!(
            a.contains("StrictHostKeyChecking=yes"),
            "host key checking was not enabled: {a}"
        );
        assert!(
            !a.contains("StrictHostKeyChecking=no"),
            "host key checking was DISABLED: {a}"
        );
        assert!(
            !a.contains("/dev/null"),
            "known_hosts was pointed at /dev/null: {a}"
        );
    }

    #[test]
    fn no_secret_ever_reaches_the_command_line() {
        let mut s = spec();
        s.identity_file = Some(PathBuf::from("/home/alex/.ssh/id_ed25519"));
        let a = sshfs_args(&s, Path::new("/run/mnt"), 10).join(" ");
        // The identity is referenced by PATH, which is not a secret...
        assert!(a.contains("IdentityFile=/home/alex/.ssh/id_ed25519"));
        // ...and nothing that looks like a password option is ever emitted.
        for bad in [
            "password",
            "Password",
            "-o password_stdin",
            "sshpass",
            "PubkeyAuthentication=no",
        ] {
            assert!(!a.contains(bad), "{bad:?} appeared in argv: {a}");
        }
    }

    #[test]
    fn a_mount_is_always_bounded() {
        let a = sshfs_args(&spec(), Path::new("/run/mnt"), 7).join(" ");
        assert!(a.contains("ConnectTimeout=7"), "no connect timeout: {a}");
        assert!(
            a.contains("BatchMode=yes"),
            "could prompt interactively: {a}"
        );
        // reconnect without keepalives hides a dead server forever.
        assert!(a.contains("ServerAliveInterval="), "no keepalive: {a}");
    }

    #[test]
    fn an_alias_is_used_verbatim_so_its_whole_config_block_applies() {
        // Resolving `studio` to its hostname discards IdentityFile,
        // UserKnownHostsFile, Port and ProxyJump from that Host block, and
        // the connection then fails looking like a changed host key.
        let mut s = spec();
        s.use_alias_as_target = true;
        s.remote_path = Some("/srv".into());
        assert_eq!(s.source(), "studio:/srv");

        let a = sshfs_args(&s, Path::new("/run/mnt"), 10).join(" ");
        assert!(
            !a.contains("port=2222"),
            "an explicit port overrode the Host block: {a}"
        );
        assert!(a.contains("StrictHostKeyChecking=yes"));
    }

    #[test]
    fn the_source_is_built_correctly() {
        assert_eq!(spec().source(), "alex@studio.example:");
        let mut s = spec();
        s.remote_path = Some("/srv/data".into());
        assert_eq!(s.source(), "alex@studio.example:/srv/data");
        let mut anon = RemoteSpec::new("nas", "nas.local");
        anon.remote_path = Some("/vol".into());
        assert_eq!(anon.source(), "nas.local:/vol");
    }

    #[test]
    fn a_changed_host_key_is_recognised_and_never_bypassed() {
        let real = "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\n\
                    Someone could be eavesdropping on you right now";
        assert_eq!(classify(real), MountError::HostKeyChanged);
        let m = classify(real).message();
        assert!(m.contains("identity has changed"), "unhelpful: {m}");
        assert!(
            !m.to_lowercase().contains("known_hosts"),
            "leaked a terminal instruction at somebody who does not use one: {m}"
        );
    }

    #[test]
    fn failures_are_told_apart() {
        assert_eq!(
            classify("Permission denied (publickey)."),
            MountError::AuthFailed
        );
        assert_eq!(
            classify("ssh: connect to host x port 22: Connection timed out"),
            MountError::Timeout
        );
        assert!(matches!(
            classify("ssh: Could not resolve hostname nope"),
            MountError::Other(_)
        ));
    }

    #[test]
    fn an_unknown_error_keeps_the_tools_own_words() {
        // Never invent a diagnosis for something we do not recognise.
        let e = classify("sshfs: something entirely novel went wrong");
        match e {
            MountError::Other(w) => assert!(w.contains("entirely novel"), "words lost: {w}"),
            other => panic!("guessed a diagnosis: {other:?}"),
        }
    }

    #[test]
    fn every_error_speaks_human() {
        for e in [
            MountError::ToolMissing("sshfs"),
            MountError::HostKeyChanged,
            MountError::AuthFailed,
            MountError::Timeout,
            MountError::BadMountPoint("nope".into()),
        ] {
            let m = e.message();
            assert!(!m.is_empty());
            assert!(
                !m.contains("os error") && !m.contains("errno"),
                "raw system text reached the user: {m}"
            );
        }
    }

    #[test]
    fn a_mount_point_cannot_escape_its_directory() {
        // The alias is user input and must stay one path segment.
        let p = mount_point_for("../../etc/passwd").unwrap();
        // Note: the sanitised name may still CONTAIN the characters "..",
        // e.g. `_.._etc_passwd`. That is a harmless ordinary filename. What
        // matters is that no component IS `..`, which is what traverses.
        assert!(
            !p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "an alias produced a real parent-directory component: {}",
            p.display()
        );
        // What matters is containment, not the exact mangling: the result
        // must be one segment under gust/places and must not reach /etc.
        let base = mount_point_for("x")
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let rel = p.strip_prefix(&base).expect("alias escaped the places dir");
        assert_eq!(
            rel.components().count(),
            1,
            "an alias became more than one path segment: {}",
            rel.display()
        );
        assert!(
            !p.to_string_lossy().contains("/etc/passwd"),
            "alias reached a real system path: {}",
            p.display()
        );

        let p2 = mount_point_for("studio").unwrap();
        assert!(p2.ends_with("gust/places/studio"), "{}", p2.display());
        assert_eq!(
            mount_point_for("..."),
            None,
            "a dots-only alias was allowed"
        );
    }

    #[test]
    fn a_mount_point_is_never_inside_the_home_directory() {
        let p = mount_point_for("studio").unwrap();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            // A stale mount inside home would look like the user's own files.
            assert!(
                !p.starts_with(&home) || p.to_string_lossy().contains("/run/"),
                "mount point sits in the home directory: {}",
                p.display()
            );
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-prep-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn files_stranded_in_a_mount_point_are_set_aside_never_deleted() {
        // RED on the old code: mount() removed these with remove_dir_all.
        for kind in ["places", "tailnet", "cloud"] {
            let root = scratch(kind);
            let runtime = root.join("run");
            let rec = root.join("data/gust/recovered");
            let mp = runtime.join("gust").join(kind).join("nas");
            std::fs::create_dir_all(mp.join("half-sent")).unwrap();
            std::fs::write(mp.join("report.odt"), b"the only copy").unwrap();
            std::fs::write(mp.join("half-sent/a.txt"), b"a").unwrap();

            let got = prepare_mount_point_in(&mp, &runtime, &rec, "nas", "2026-10-08 10.00")
                .expect("prepare");
            let dir = got.expect("a recovered folder is reported");
            assert_eq!(dir, rec.join("nas 2026-10-08 10.00"));
            assert_eq!(
                std::fs::read(dir.join("report.odt")).unwrap(),
                b"the only copy"
            );
            assert_eq!(std::fs::read(dir.join("half-sent/a.txt")).unwrap(), b"a");
            assert_eq!(
                std::fs::read_dir(&mp).unwrap().count(),
                0,
                "{kind}: mount point empty"
            );
            let mode = std::os::unix::fs::PermissionsExt::mode(
                &std::fs::metadata(&dir).unwrap().permissions(),
            );
            assert_eq!(mode & 0o777, 0o700);

            // A second stranding the same minute gets its own folder.
            std::fs::write(mp.join("report.odt"), b"second").unwrap();
            let again = prepare_mount_point_in(&mp, &runtime, &rec, "nas", "2026-10-08 10.00")
                .unwrap()
                .unwrap();
            assert_eq!(again, rec.join("nas 2026-10-08 10.00 (2)"));
            assert_eq!(
                std::fs::read(dir.join("report.odt")).unwrap(),
                b"the only copy"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn an_empty_mount_point_reports_nothing_and_a_foreign_folder_is_refused_untouched() {
        let root = scratch("foreign");
        let runtime = root.join("run");
        let rec = root.join("rec");
        let mp = runtime.join("gust/places/box");
        assert_eq!(
            prepare_mount_point_in(&mp, &runtime, &rec, "box", "t").unwrap(),
            None
        );
        assert!(!rec.exists(), "nothing recovered, nothing created");

        let theirs = root.join("Documents");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("keep.txt"), b"k").unwrap();
        let err = prepare_mount_point_in(&theirs, &runtime, &rec, "x", "t").unwrap_err();
        assert!(matches!(err, MountError::BadMountPoint(_)));
        assert_eq!(std::fs::read(theirs.join("keep.txt")).unwrap(), b"k");
        // Nested or traversing paths are not Gust's either.
        for p in ["gust/places/a/b", "gust/places/../places/a", "gust/other/a"] {
            assert!(!is_gust_mount_point_in(&runtime.join(p), &runtime), "{p}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_gusts_own_mount_points_are_ever_cleared() {
        // A file left in the mount point by a write that raced a disconnect
        // would otherwise block every future connection forever.
        let ours = mount_point_for("sweeptest").unwrap();
        assert!(is_gust_mount_point(&ours), "{}", ours.display());

        // Anywhere else is never cleared automatically.
        for hostile in [
            "/home/someone/Documents",
            "/tmp/not-gust",
            "/run/user/1000/gust/places/a/b",
        ] {
            assert!(
                !is_gust_mount_point(Path::new(hostile)),
                "would have cleared {hostile}"
            );
        }
        if let Some(home) = std::env::var_os("HOME") {
            assert!(!is_gust_mount_point(Path::new(&home)));
        }
    }

    #[test]
    fn is_mounted_is_false_for_an_ordinary_directory() {
        let d = std::env::temp_dir().join("gust-mount-probe");
        let _ = std::fs::create_dir_all(&d);
        assert!(
            !is_mounted(&d),
            "an ordinary directory claimed to be a mount"
        );
        let _ = std::fs::remove_dir(&d);
    }

    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(
            unescape_mountinfo(r"/run/media/me/My\040Disk"),
            "/run/media/me/My Disk"
        );
        assert_eq!(unescape_mountinfo(r"/a\134b"), r"/a\b");
        assert_eq!(unescape_mountinfo(r"/trailing\04"), r"/trailing\04");
    }

    #[test]
    fn the_root_is_a_mount_point() {
        assert!(is_mounted(Path::new("/")) || listed_in_mountinfo(Path::new("/")).is_none());
    }

    #[test]
    fn unmounting_something_that_is_not_mounted_is_harmless() {
        let d = std::env::temp_dir().join("gust-unmount-probe");
        let _ = std::fs::create_dir_all(&d);
        assert!(unmount(&d).is_ok());
        // The directory must still be there: unmount is not a delete.
        assert!(d.exists(), "unmount removed a real directory");
        let _ = std::fs::remove_dir(&d);
    }
}
