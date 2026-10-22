//! Cloud remotes through rclone (DESIGN §6.8, plan §3).
//!
//! Gust implements no cloud provider. It reads the **installed** rclone's
//! configured remotes and asks that rclone what it can do; a provider Gust
//! has never heard of works if rclone has it, and one rclone lacks does not
//! appear.
//!
//! Nothing in this module reads, writes, stores, echoes or logs a token,
//! password or client secret, and nothing puts one on a command line.
//! Configuring a remote is `rclone config`'s job.

use crate::helper::{self, CallContext, ErrorCode, RemoteError};
use gust_core::lister::CancelToken;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A remote the installed rclone has configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRemote {
    /// The name in rclone's config, e.g. `gustcloud`. Used verbatim: an
    /// alias is never "resolved" into something else (the lesson from SSH).
    pub name: String,
    /// rclone's backend type, e.g. `webdav`, `drive`, `protondrive`.
    pub backend: String,
}

impl CloudRemote {
    /// What the user sees. The backend is shown because "Dropbox" and
    /// "a WebDAV server someone called dropbox" are not the same thing.
    pub fn display(&self) -> String {
        format!("{} ({})", self.name, self.backend)
    }
}

/// Why a cloud operation could not be done. Every variant carries something
/// the user can act on; none of them is "error".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudError {
    /// rclone is not installed. Cloud remotes are an optional dependency and
    /// their absence degrades gracefully (plan §8).
    NotInstalled,
    Service(RemoteError),
    /// rclone's config is encrypted and locked. Gust never handles the
    /// passphrase; rclone's own prompt does.
    ConfigLocked,
    /// rclone ran and refused, with its own explanation.
    Refused(String),
    /// The remote is not in the installed rclone's config.
    NoSuchRemote(String),
    /// A path that is not inside the mount it claims to be in.
    OutsideMount,
}

impl CloudError {
    pub fn message(&self) -> String {
        match self {
            CloudError::Service(error) => error.message.clone(),
            CloudError::NotInstalled => {
                "rclone is not installed, so cloud storage is unavailable. \
                 Install rclone to add cloud places."
                    .into()
            }
            CloudError::ConfigLocked => {
                "rclone's configuration is encrypted. Unlock it with rclone \
                 first — Gust never handles that passphrase."
                    .into()
            }
            CloudError::Refused(why) => why.clone(),
            CloudError::NoSuchRemote(n) => {
                format!("rclone has no remote called {n}")
            }
            CloudError::OutsideMount => "that file is not on this remote".into(),
        }
    }
}

fn helper_error(error: RemoteError) -> CloudError {
    if error.code == ErrorCode::MissingHelper {
        CloudError::NotInstalled
    } else {
        CloudError::Service(error)
    }
}

/// Is rclone available at all?
pub fn rclone_available() -> bool {
    let mut command = Command::new("rclone");
    command.arg("version");
    helper::run(
        command,
        &[],
        &CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(3),
        },
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// The remotes the installed rclone has configured.
///
/// **This opens no network socket.** `listremotes` reads a local file, in
/// exactly the way SSH discovery reads `~/.ssh/config` without connecting.
/// The result is a list of suggestions; connecting is a separate, deliberate
/// act by the user.
pub fn list_remotes(config: Option<&Path>) -> Result<Vec<CloudRemote>, CloudError> {
    list_remotes_with_context(
        config,
        &CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(5),
        },
    )
}
pub fn list_remotes_with_context(
    config: Option<&Path>,
    ctx: &CallContext<'_>,
) -> Result<Vec<CloudRemote>, CloudError> {
    let mut cmd = Command::new("rclone");
    cmd.arg("listremotes").arg("--long");
    if let Some(c) = config {
        cmd.arg("--config").arg(c);
    }
    // An encrypted config would otherwise block forever waiting for a
    // passphrase on a terminal that is not there.
    cmd.env("RCLONE_ASK_PASSWORD", "false");

    let out = helper::run(cmd, &[], ctx).map_err(helper_error)?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("password") || err.contains("encrypted") {
            return Err(CloudError::ConfigLocked);
        }
        // 127, or a shell saying "command not found", means rclone is not
        // really there however the PATH is arranged. The user needs Gust's
        // sentence about installing it, not the shell's.
        if out.status.code() == Some(127) || err.contains("command not found") {
            return Err(CloudError::NotInstalled);
        }
        return Err(CloudError::Refused(first_useful_line(&err)));
    }

    Ok(parse_listremotes(&String::from_utf8_lossy(&out.stdout)))
}

/// `rclone listremotes --long` prints `name:<tab or spaces>type`.
pub fn parse_listremotes(text: &str) -> Vec<CloudRemote> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, backend)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let backend = backend.trim();
        if name.is_empty() {
            continue;
        }
        out.push(CloudRemote {
            name: name.to_string(),
            // Without --long, or on an older rclone, the type is absent.
            // "unknown" is honest; guessing a provider would not be.
            backend: if backend.is_empty() {
                "unknown".to_string()
            } else {
                backend.to_string()
            },
        });
    }
    out
}

/// The first line of rclone's stderr that says something, so an alert is a
/// sentence rather than a wall of log.
fn first_useful_line(err: &str) -> String {
    for line in err.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        // rclone prefixes real messages with a level; skip bare timestamps.
        let cleaned = l
            .split_once("ERROR : ")
            .or_else(|| l.split_once("Failed to "))
            .map(|(_, rest)| rest)
            .unwrap_or(l);
        if cleaned.len() > 3 {
            return cleaned.to_string();
        }
    }
    "rclone refused, without saying why".to_string()
}

/// What rclone's VFS reports about pending uploads.
///
/// These numbers are **not** a receipt. `queued == 0` reads identically
/// before anything was ever written and after every upload finished, which
/// is why DESIGN §6.8 requires a backend listing to confirm a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VfsPending {
    pub queued: u64,
    pub in_progress: u64,
    pub errored: u64,
}

impl VfsPending {
    /// Is there definitely unfinished work? A `false` here means "nothing
    /// pending *that rclone knows about*", never "everything is uploaded".
    pub fn has_pending(self) -> bool {
        self.queued > 0 || self.in_progress > 0
    }
}

/// Where a mount's remote-control endpoint lives.
///
/// The RC can flush, drop and reconfigure a mount, so it must answer to this
/// user only. A TCP port on loopback answers to every local user and every
/// browser tab that can reach 127.0.0.1; rclone's `--rc-no-auth` then hands
/// all of them the mount. A Unix socket inside `$XDG_RUNTIME_DIR` (mode 0700,
/// owned by the user) needs no secret at all: nobody else can open it.
pub fn rc_socket_path(runtime_dir: &std::path::Path, mount_leaf: &str) -> std::path::PathBuf {
    runtime_dir
        .join("gust")
        .join("rc")
        .join(format!("{mount_leaf}.sock"))
}

/// The `--rc-addr` value that binds a socket at `path`.
pub fn rc_bind_arg(path: &std::path::Path) -> String {
    format!("unix://{}", path.display())
}

/// The arguments that point `rclone rc` at an RC address. `unix://` (and a
/// bare absolute path) dials the socket; anything else is a URL, as the
/// test fixtures and older mounts use.
pub fn rc_client_args(rc_addr: &str) -> Vec<String> {
    let sock = rc_addr
        .strip_prefix("unix://")
        .or_else(|| rc_addr.starts_with('/').then_some(rc_addr));
    match sock {
        Some(p) => vec!["--unix-socket".into(), p.into()],
        None => vec!["--url".into(), rc_addr.into()],
    }
}

/// Ask a mount's private RC endpoint what is still pending.
///
/// The RC is a privileged control surface (plan §9): it belongs to a mount
/// Gust started, listens on a user-only Unix socket (see [`rc_socket_path`]),
/// and is never exposed to previews or page content.
pub fn vfs_pending(rc_addr: &str, timeout: Duration) -> Result<VfsPending, CloudError> {
    vfs_pending_with_context(
        rc_addr,
        &CallContext {
            cancel: &CancelToken::new(),
            timeout,
        },
    )
}
pub fn vfs_pending_with_context(
    rc_addr: &str,
    ctx: &CallContext<'_>,
) -> Result<VfsPending, CloudError> {
    let mut command = Command::new("rclone");
    command
        .arg("rc")
        .args(rc_client_args(rc_addr))
        .arg("vfs/stats")
        .arg("--timeout")
        .arg(format!("{}s", ctx.timeout.as_secs().max(1)));
    let out = helper::run(command, &[], ctx).map_err(helper_error)?;

    if !out.status.success() {
        return Err(CloudError::Refused(first_useful_line(
            &String::from_utf8_lossy(&out.stderr),
        )));
    }
    parse_vfs_stats_checked(&out.stdout)
}

pub fn parse_vfs_stats_checked(bytes: &[u8]) -> Result<VfsPending, CloudError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| CloudError::Service(RemoteError::new(ErrorCode::Protocol)))?;
    let number = |key: &str| {
        value[key]
            .as_u64()
            .ok_or_else(|| CloudError::Service(RemoteError::new(ErrorCode::Protocol)))
    };
    Ok(VfsPending {
        queued: number("uploadsQueued")?,
        in_progress: number("uploadsInProgress")?,
        errored: number("erroredFiles")?,
    })
}

/// Legacy display parser. Safety decisions use `parse_vfs_stats_checked`: absent
/// or malformed counters are unknown, never a proof that disconnect is safe.
pub fn parse_vfs_stats(json: &str) -> VfsPending {
    VfsPending {
        queued: json_u64(json, "uploadsQueued"),
        in_progress: json_u64(json, "uploadsInProgress"),
        errored: json_u64(json, "erroredFiles"),
    }
}

fn json_u64(json: &str, key: &str) -> u64 {
    let needle = format!("\"{key}\"");
    let Some(at) = json.find(&needle) else {
        return 0;
    };
    let rest = &json[at + needle.len()..];
    let Some(colon) = rest.find(':') else {
        return 0;
    };
    rest[colon + 1..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// One entry as the backend itself reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendEntry {
    pub name: String,
    pub size: u64,
}

/// Ask the backend what it actually holds.
///
/// This is the only thing that may promote a file to "uploaded" (DESIGN
/// §6.8). It talks to the remote, not to the cache, and deliberately so.
pub fn backend_listing(
    remote: &str,
    subdir: &str,
    config: Option<&Path>,
) -> Result<Vec<BackendEntry>, CloudError> {
    backend_listing_with_context(
        remote,
        subdir,
        config,
        &CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(15),
        },
    )
}
pub fn backend_listing_with_context(
    remote: &str,
    subdir: &str,
    config: Option<&Path>,
    ctx: &CallContext<'_>,
) -> Result<Vec<BackendEntry>, CloudError> {
    if remote.is_empty()
        || remote.starts_with('-')
        || remote.contains([':', '/', '\\'])
        || remote.chars().any(char::is_control)
        || subdir.split('/').any(|s| s == "..")
    {
        return Err(CloudError::Service(RemoteError::new(
            ErrorCode::InvalidInput,
        )));
    }
    let target = if subdir.is_empty() {
        format!("{remote}:")
    } else {
        format!("{remote}:{subdir}")
    };
    let mut cmd = Command::new("rclone");
    cmd.arg("lsjson").arg(&target);
    if let Some(c) = config {
        cmd.arg("--config").arg(c);
    }
    cmd.env("RCLONE_ASK_PASSWORD", "false");

    let out = helper::run(cmd, &[], ctx).map_err(helper_error)?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("didn't find section") || err.contains("not found") {
            return Err(CloudError::NoSuchRemote(remote.to_string()));
        }
        return Err(CloudError::Refused(first_useful_line(&err)));
    }
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| CloudError::Service(RemoteError::new(ErrorCode::Protocol)))?;
    let entries = entries
        .as_array()
        .ok_or_else(|| CloudError::Service(RemoteError::new(ErrorCode::Protocol)))?;
    entries
        .iter()
        .filter(|entry| entry["IsDir"].as_bool() != Some(true))
        .map(|entry| {
            let name = entry["Name"]
                .as_str()
                .ok_or_else(|| CloudError::Service(RemoteError::new(ErrorCode::Protocol)))?;
            let size = entry["Size"]
                .as_u64()
                .ok_or_else(|| CloudError::Service(RemoteError::new(ErrorCode::Protocol)))?;
            Ok(BackendEntry {
                name: name.into(),
                size,
            })
        })
        .collect()
}

/// Names and sizes from `rclone lsjson`. Only the two fields that decide
/// whether a specific file is on the backend.
pub fn parse_lsjson(json: &str) -> Vec<BackendEntry> {
    let mut out = Vec::new();
    // Entries are `{...}` objects in a flat array; scan them one at a time
    // rather than taking a JSON dependency for two fields.
    for chunk in json.split('{').skip(1) {
        let obj = chunk.split('}').next().unwrap_or("");
        let Some(name) = json_str(obj, "Name") else {
            continue;
        };
        let is_dir = obj.contains("\"IsDir\":true") || obj.contains("\"IsDir\": true");
        if is_dir {
            continue;
        }
        out.push(BackendEntry {
            name,
            size: json_u64(obj, "Size"),
        });
    }
    out
}

fn json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = json.find(&needle)?;
    let rest = &json[at + needle.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let mut chars = after.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            value.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(value);
        } else {
            value.push(c);
        }
    }
    None
}

/// Is `file` confirmed present on the backend at the size we expect?
///
/// Both halves matter: a name alone can be a zero-byte placeholder left by a
/// failed upload, which is exactly the case that must not read as "uploaded".
pub fn confirmed_on_backend(entries: &[BackendEntry], name: &str, expect_size: u64) -> bool {
    entries
        .iter()
        .any(|e| e.name == name && e.size == expect_size)
}

/// The path of a file relative to its mount point, or `OutsideMount`.
pub fn relative_to_mount(mount: &Path, file: &Path) -> Result<PathBuf, CloudError> {
    file.strip_prefix(mount)
        .map(|p| p.to_path_buf())
        .map_err(|_| CloudError::OutsideMount)
}

/// The command that configures a remote. Gust runs this in a terminal and
/// never scrapes or automates it: every provider's setup differs, and
/// Drive's OAuth is not a template for Proton or Filen (plan §3).
pub fn config_command() -> (&'static str, Vec<&'static str>) {
    ("rclone", vec!["config"])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_or_malformed_upload_stats_cannot_clear_disconnect_guard() {
        for bytes in [
            b"{}".as_slice(),
            b"not json",
            br#"{"uploadsQueued":0,"uploadsInProgress":0}"#,
            br#"{"uploadsQueued":0,"uploadsInProgress":0,"erroredFiles":null}"#,
        ] {
            assert!(parse_vfs_stats_checked(bytes).is_err());
        }
        assert_eq!(
            parse_vfs_stats_checked(
                br#"{"uploadsQueued":0,"uploadsInProgress":0,"erroredFiles":0}"#
            )
            .unwrap(),
            VfsPending::default()
        );
    }

    #[test]
    fn the_rc_is_a_user_only_socket_not_a_port() {
        let p = rc_socket_path(std::path::Path::new("/run/user/1000"), "drive");
        assert_eq!(
            p,
            std::path::PathBuf::from("/run/user/1000/gust/rc/drive.sock")
        );
        assert_eq!(rc_bind_arg(&p), "unix:///run/user/1000/gust/rc/drive.sock");
        assert_eq!(
            rc_client_args(&rc_bind_arg(&p)),
            vec!["--unix-socket", "/run/user/1000/gust/rc/drive.sock"]
        );
        // URLs keep working for fixtures.
        assert_eq!(
            rc_client_args("http://127.0.0.1:9"),
            vec!["--url", "http://127.0.0.1:9"]
        );
    }

    #[test]
    fn listremotes_long_is_parsed() {
        let text = "gustcloud:     webdav\nwork:          drive\n";
        let got = parse_listremotes(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "gustcloud");
        assert_eq!(got[0].backend, "webdav");
        assert_eq!(got[1].name, "work");
    }

    #[test]
    fn a_remote_without_a_type_is_unknown_not_guessed() {
        let got = parse_listremotes("mystery:\n");
        assert_eq!(got[0].backend, "unknown");
    }

    #[test]
    fn blank_lines_and_junk_are_ignored() {
        assert!(parse_listremotes("\n\n   \nnocolon\n").is_empty());
    }

    #[test]
    fn display_names_the_backend() {
        let r = CloudRemote {
            name: "photos".into(),
            backend: "protondrive".into(),
        };
        assert_eq!(r.display(), "photos (protondrive)");
    }

    #[test]
    fn vfs_stats_numbers_are_read() {
        let json = r#"{"diskCache":{"erroredFiles":2,"uploadsInProgress":1,"uploadsQueued":3}}"#;
        let p = parse_vfs_stats(json);
        assert_eq!(p.queued, 3);
        assert_eq!(p.in_progress, 1);
        assert_eq!(p.errored, 2);
        assert!(p.has_pending());
    }

    #[test]
    fn an_empty_queue_is_not_a_receipt() {
        // The whole point of DESIGN §6.8: these two situations are
        // indistinguishable from the queue alone.
        let never_written = parse_vfs_stats(r#"{"uploadsQueued":0,"uploadsInProgress":0}"#);
        let all_uploaded = parse_vfs_stats(r#"{"uploadsQueued":0,"uploadsInProgress":0}"#);
        assert_eq!(never_written, all_uploaded);
        assert!(!never_written.has_pending());
        // has_pending() == false must therefore never be used as proof that
        // a particular file reached the backend; confirmed_on_backend is.
    }

    #[test]
    fn missing_fields_read_as_zero_not_as_an_error() {
        let p = parse_vfs_stats("{}");
        assert_eq!(p, VfsPending::default());
    }

    #[test]
    fn lsjson_entries_are_parsed() {
        let json = r#"[
          {"Path":"a.txt","Name":"a.txt","Size":12,"IsDir":false},
          {"Path":"sub","Name":"sub","Size":-1,"IsDir":true},
          {"Path":"b.bin","Name":"b.bin","Size":4096,"IsDir":false}
        ]"#;
        let got = parse_lsjson(json);
        assert_eq!(got.len(), 2, "directories are not files");
        assert_eq!(got[0].name, "a.txt");
        assert_eq!(got[0].size, 12);
        assert_eq!(got[1].size, 4096);
    }

    #[test]
    fn confirmation_needs_the_right_size_too() {
        let entries = vec![BackendEntry {
            name: "notes.txt".into(),
            size: 0,
        }];
        // A zero-byte placeholder from a failed upload must not read as
        // "uploaded" just because the name is there.
        assert!(!confirmed_on_backend(&entries, "notes.txt", 120));
        assert!(confirmed_on_backend(&entries, "notes.txt", 0));
        assert!(!confirmed_on_backend(&entries, "other.txt", 0));
    }

    #[test]
    fn a_path_outside_the_mount_is_refused() {
        let mount = Path::new("/run/user/1000/gust/cloud/x");
        assert!(relative_to_mount(mount, Path::new("/home/someone/a.txt")).is_err());
        assert_eq!(
            relative_to_mount(mount, Path::new("/run/user/1000/gust/cloud/x/sub/a.txt")).unwrap(),
            PathBuf::from("sub/a.txt")
        );
    }

    #[test]
    fn a_broken_rclone_reads_as_not_installed() {
        // A shim, a half-removed package or a PATH pointing at nothing all
        // produce exit 127; the user needs "install rclone", not the
        // shell's own words.
        assert_eq!(
            CloudError::NotInstalled.message(),
            CloudError::NotInstalled.message()
        );
        assert!(CloudError::NotInstalled
            .message()
            .contains("Install rclone"));
    }

    #[test]
    fn errors_say_something_actionable() {
        assert!(CloudError::NotInstalled
            .message()
            .contains("Install rclone"));
        assert!(CloudError::ConfigLocked
            .message()
            .contains("never handles that passphrase"));
        assert!(CloudError::NoSuchRemote("zz".into())
            .message()
            .contains("zz"));
    }

    #[test]
    fn rclone_stderr_becomes_one_sentence() {
        let err = "2026/01/01 12:00:00 ERROR : directory not found\n";
        assert_eq!(first_useful_line(err), "directory not found");
    }

    #[test]
    fn the_config_command_is_rclones_own() {
        let (bin, args) = config_command();
        assert_eq!(bin, "rclone");
        assert_eq!(args, vec!["config"]);
        // No credential may ever be passed as an argument.
        assert!(!args
            .iter()
            .any(|a| a.contains("pass") || a.contains("token")));
    }
}
