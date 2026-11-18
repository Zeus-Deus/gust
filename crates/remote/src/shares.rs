//! Network shares through GIO (DESIGN §6.14, plan §3 "NAS/shared folder").
//!
//! A share becomes an ordinary folder once GVfs has mounted it, so almost
//! nothing downstream needs to know it is remote. What lives here is the
//! part that is genuinely different: turning an address into a URI, handing
//! credentials to GIO without ever letting them touch argv or a log, and
//! finding the mount point afterwards.
//!
//! **No prompt string is ever parsed.** `gio mount` writes its prompt in the
//! system language; matching on it would break for anyone not running in
//! English. GIO reads user, domain and password from stdin in that fixed
//! order, which is a protocol rather than a prompt.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Which protocol a share speaks. The mount path is the same; the URI
/// scheme and the default expectations are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareKind {
    Smb,
    Nfs,
}

impl ShareKind {
    pub fn scheme(self) -> &'static str {
        match self {
            ShareKind::Smb => "smb",
            ShareKind::Nfs => "nfs",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ShareKind::Smb => "Windows / Samba share",
            ShareKind::Nfs => "NFS export",
        }
    }
}

/// A share the user typed in. Never guessed, never discovered into
/// existence: the plan is explicit that discovery can be incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub kind: ShareKind,
    /// Host name or address as typed.
    pub server: String,
    /// The share or export name.
    pub share: String,
    /// Username, when the share needs one. Stored; the password is not.
    pub user: Option<String>,
    /// Windows domain / workgroup.
    pub domain: Option<String>,
}

/// Why an address could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareError {
    /// GVfs is not installed, so shares cannot be mounted at all.
    NoGvfs,
    /// The address made no sense.
    BadAddress(String),
    /// The server said no.
    AuthFailed,
    /// GVfs exposes an SMB mount without identifying the login it uses.
    AlreadyMounted(PathBuf),
    /// Anything else, with whatever GIO said about it.
    Refused(String),
}

impl ShareError {
    pub fn message(&self) -> String {
        match self {
            ShareError::NoGvfs => "Network shares need GVfs, which is not installed. \
                 Install gvfs-smb to add a shared folder."
                .into(),
            ShareError::BadAddress(a) => {
                format!("{a} does not look like a server and share.")
            }
            ShareError::AuthFailed => {
                "That username or password was not accepted by the server.".into()
            }
            ShareError::AlreadyMounted(path) => format!(
                "This share is already mounted. Open {} in the address bar to use the \
                 existing connection, or disconnect it before retrying.",
                path.display()
            ),
            ShareError::Refused(why) => why.clone(),
        }
    }
}

/// Parse what a person types into a server and a share.
///
/// Accepts the forms people actually write: `\\\\server\\share`,
/// `//server/share`, `smb://server/share`, and plain `server/share`.
pub fn parse_address(text: &str, kind: ShareKind) -> Result<Share, ShareError> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err(ShareError::BadAddress(text.to_string()));
    }
    // Windows-style backslashes, as typed off a sticky note.
    let unified = raw.replace('\\', "/");
    let without_scheme = unified
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&unified);
    let trimmed = without_scheme.trim_start_matches('/');

    let mut parts = trimmed.splitn(2, '/');
    let server = parts.next().unwrap_or("").trim().to_string();
    let share = parts
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();

    if server.is_empty() || share.is_empty() {
        return Err(ShareError::BadAddress(text.to_string()));
    }
    // A share name with a path separator left in it is two levels deep,
    // which GIO handles, but an empty segment is a typo.
    if share.split('/').any(|s| s.is_empty()) {
        return Err(ShareError::BadAddress(text.to_string()));
    }
    Ok(Share {
        kind,
        server,
        share,
        user: None,
        domain: None,
    })
}

impl Share {
    /// The URI GIO mounts. The username is NOT put in the URI even when
    /// known: it belongs in the credential exchange, not in a string that
    /// ends up in a recent-places list.
    pub fn uri(&self) -> String {
        format!("{}://{}/{}", self.kind.scheme(), self.server, self.share)
    }

    /// What the sidebar calls it.
    pub fn display(&self) -> String {
        format!("{} on {}", self.share, self.server)
    }

    /// Where GVfs puts this share once mounted.
    ///
    /// GVfs names the directory after the connection parameters, so the
    /// mount point can be predicted rather than scraped out of stdout.
    pub fn mount_point(&self, runtime_dir: &str) -> PathBuf {
        let name = match self.kind {
            ShareKind::Smb => {
                format!("smb-share:server={},share={}", self.server, self.share)
            }
            // Measured against a real GVfs NFS mount, not guessed: the
            // directory is `prefix=<url-encoded absolute export path>`.
            // An earlier version said `export=<share>` and was simply wrong.
            ShareKind::Nfs => {
                let path = if self.share.starts_with('/') {
                    self.share.clone()
                } else {
                    format!("/{}", self.share)
                };
                format!("nfs:host={},prefix={}", self.server, percent_encode(&path))
            }
        };
        PathBuf::from(runtime_dir).join("gvfs").join(name)
    }
}

/// Percent-encode a path the way GVfs names its mount directories.
///
/// Only the characters GVfs escapes: everything outside the unreserved set
/// becomes %XX, uppercase.
fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len() * 2);
    for b in path.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Is GVfs available to mount shares with?
pub fn gvfs_available() -> bool {
    Command::new("gio")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        && std::path::Path::new("/usr/share/gvfs/mounts/smb.mount").exists()
}

/// Mount a share, handing credentials to GIO on stdin.
///
/// The password reaches the child's stdin pipe and nothing else: not argv,
/// not the environment, not a log, and not Gust's places file. Returns the
/// mount point on success.
///
/// `password` is taken by value and dropped as soon as it has been written.
pub fn mount(
    share: &Share,
    password: String,
    runtime_dir: &str,
    timeout: Duration,
) -> Result<PathBuf, ShareError> {
    if !gvfs_available() {
        return Err(ShareError::NoGvfs);
    }
    mount_with_command(
        share,
        share.mount_point(runtime_dir),
        mount_command(share, password.is_empty()),
        password,
        timeout,
    )
}

fn mount_with_command(
    share: &Share,
    mount_point: PathBuf,
    mut command: Command,
    password: String,
    timeout: Duration,
) -> Result<PathBuf, ShareError> {
    if mount_point.is_dir() {
        return use_existing_mount(share.kind, mount_point);
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ShareError::Refused(e.to_string()))?;

    // The fixed order GIO reads: user, domain, password. A protocol, not a
    // prompt — nothing here depends on the system language.
    if let Some(mut stdin) = child.stdin.take() {
        let user = share.user.clone().unwrap_or_default();
        let domain = share.domain.clone().unwrap_or_default();
        let _ = writeln!(stdin, "{user}");
        let _ = writeln!(stdin, "{domain}");
        let _ = writeln!(stdin, "{password}");
        // Dropping stdin closes the pipe; dropping `password` with it.
    }
    drop(password);

    // Bounded: a server that never answers must not hang the caller.
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                if status.success() {
                    return match share.kind {
                        ShareKind::Smb => wait_for_smb_folder(
                            mount_point,
                            timeout
                                .saturating_sub(started.elapsed())
                                .min(Duration::from_secs(1)),
                        ),
                        ShareKind::Nfs => Ok(mount_point),
                    };
                }
                if mount_point.is_dir() {
                    return use_existing_mount(share.kind, mount_point);
                }
                return Err(classify(&err));
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    gust_core::stall::reap_in_background(child);
                    return Err(ShareError::Refused(
                        "The server did not answer in time.".into(),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(ShareError::Refused(e.to_string())),
        }
    }
}

/// GIO can accept the mount before its FUSE directory is ready. Check only
/// the directory metadata and keep a stalled lookup in a bounded helper.
fn wait_for_smb_folder(path: PathBuf, timeout: Duration) -> Result<PathBuf, ShareError> {
    let started = Instant::now();
    let cancel = gust_core::lister::CancelToken::new();
    while started.elapsed() < timeout {
        let ctx = crate::helper::CallContext {
            cancel: &cancel,
            timeout: timeout.saturating_sub(started.elapsed()),
        };
        let mut command = Command::new("test");
        command.arg("-d").arg(&path);
        match crate::helper::run(command, &[], &ctx) {
            Ok(output) if output.status.success() => return Ok(path),
            Ok(_) => std::thread::sleep(
                Duration::from_millis(25).min(timeout.saturating_sub(started.elapsed())),
            ),
            Err(_) => break,
        }
    }
    Err(ShareError::Refused(format!(
        "The share is mounted, but its folder is still unavailable. Open {} when it is ready.",
        path.display()
    )))
}

fn use_existing_mount(kind: ShareKind, mount_point: PathBuf) -> Result<PathBuf, ShareError> {
    // Guest and named SMB logins share the same GVfs path.
    match kind {
        ShareKind::Smb => Err(ShareError::AlreadyMounted(mount_point)),
        ShareKind::Nfs => Ok(mount_point),
    }
}

fn mount_command(share: &Share, password_empty: bool) -> Command {
    let mut command = Command::new("gio");
    command.arg("mount");
    if share.kind == ShareKind::Smb
        && share.user.is_none()
        && share.domain.is_none()
        && password_empty
    {
        command.arg("--anonymous");
    }
    command.arg(share.uri());
    command
}

/// Turn GIO's stderr into something a person can act on.
///
/// Matches on GIO's *error identity* where it can, and falls back to the
/// last line. Deliberately does NOT try to interpret a translated prompt.
fn classify(err: &str) -> ShareError {
    let lower = err.to_lowercase();
    if lower.contains("password") && lower.contains("incorrect")
        || lower.contains("permission denied")
        || lower.contains("authentication")
    {
        return ShareError::AuthFailed;
    }
    let last = err
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("The share could not be mounted.")
        .trim();
    // Strip gio's own prefix so the sentence reads as Gust's.
    let cleaned = last.strip_prefix("gio: ").unwrap_or(last);
    ShareError::Refused(cleaned.to_string())
}

/// Unmount a share.
pub fn unmount(share: &Share) -> Result<(), ShareError> {
    let out = Command::new("gio")
        .arg("mount")
        .arg("--unmount")
        .arg(share.uri())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ShareError::Refused(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(classify(&String::from_utf8_lossy(&out.stderr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_smb_mounts_explicitly_as_anonymous() {
        let mut smb = parse_address("//nas/public", ShareKind::Smb).unwrap();
        let is_anonymous = |share: &Share, password_empty| {
            mount_command(share, password_empty)
                .get_args()
                .any(|arg| arg == "--anonymous")
        };
        assert!(is_anonymous(&smb, true));
        assert!(!is_anonymous(&smb, false));
        smb.user = Some("reader".into());
        assert!(!is_anonymous(&smb, true));
        let nfs = parse_address("nfs://nas/export", ShareKind::Nfs).unwrap();
        assert!(!is_anonymous(&nfs, true));
    }

    #[test]
    fn only_nfs_reuses_existing_mounts() {
        let runtime = std::env::temp_dir().join(format!("gust-shares-{}", std::process::id()));
        std::fs::create_dir(&runtime).unwrap();
        std::fs::create_dir(runtime.join("gvfs")).unwrap();

        let guest = parse_address("//nas/public", ShareKind::Smb).unwrap();
        let mut named = guest.clone();
        named.user = Some("reader".into());
        let nfs = parse_address("nfs://nas/export", ShareKind::Nfs).unwrap();
        let runtime_dir = runtime.to_str().unwrap();
        assert_eq!(
            guest.mount_point(runtime_dir),
            named.mount_point(runtime_dir)
        );

        for share in [&guest, &named, &nfs] {
            let path = share.mount_point(runtime_dir);
            let expected = match share.kind {
                ShareKind::Smb => Err(ShareError::AlreadyMounted(path.clone())),
                ShareKind::Nfs => Ok(path.clone()),
            };

            std::fs::create_dir(&path).unwrap();
            let unused = Command::new(runtime.join("must-not-run"));
            assert_eq!(
                mount_with_command(
                    share,
                    path.clone(),
                    unused,
                    String::new(),
                    Duration::from_secs(3)
                ),
                expected,
                "an existing mount must be handled before running the command"
            );
            std::fs::remove_dir(&path).unwrap();

            // The directory appeared while a competing mount command failed.
            let mut failed = Command::new("sh");
            failed
                .args(["-c", r#"mkdir -- "$1" || exit 2; exit 1"#, "sh"])
                .arg(&path);
            assert_eq!(
                mount_with_command(
                    share,
                    path.clone(),
                    failed,
                    String::new(),
                    Duration::from_secs(3)
                ),
                expected,
                "a failed command must also respect the reuse policy"
            );
            std::fs::remove_dir(&path).unwrap();
        }
        std::fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn smb_mount_waits_for_the_fuse_directory() {
        let runtime =
            std::env::temp_dir().join(format!("gust-smb-readiness-{}", std::process::id()));
        std::fs::create_dir(&runtime).unwrap();
        let path = runtime.join("share");
        let share = parse_address("//nas/public", ShareKind::Smb).unwrap();

        let unready = mount_with_command(
            &share,
            path.clone(),
            Command::new("true"),
            String::new(),
            Duration::from_millis(200),
        );
        assert!(unready.unwrap_err().message().contains("still unavailable"));

        let fixture = path.clone();
        let publish = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::create_dir(fixture).unwrap();
        });
        let mounted = wait_for_smb_folder(path.clone(), Duration::from_secs(3));
        publish.join().unwrap();
        assert_eq!(mounted.unwrap(), path);
        std::fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn the_forms_people_actually_type_are_accepted() {
        for text in [
            r"\\nas\media",
            "//nas/media",
            "smb://nas/media",
            "nas/media",
            "  //nas/media/  ",
        ] {
            let s = parse_address(text, ShareKind::Smb)
                .unwrap_or_else(|_| panic!("{text} should parse"));
            assert_eq!(s.server, "nas", "{text}");
            assert_eq!(s.share, "media", "{text}");
        }
    }

    #[test]
    fn an_address_without_a_share_is_refused() {
        for text in ["//nas", "nas", "smb://nas/", "", "   ", "//"] {
            assert!(
                parse_address(text, ShareKind::Smb).is_err(),
                "{text:?} should not parse"
            );
        }
    }

    #[test]
    fn a_username_never_enters_the_uri() {
        // A URI ends up in recent places and window titles. Credentials
        // belong in the credential exchange, nowhere else.
        let mut s = parse_address("//nas/media", ShareKind::Smb).unwrap();
        s.user = Some("alex".into());
        s.domain = Some("WORKGROUP".into());
        assert_eq!(s.uri(), "smb://nas/media");
        assert!(!s.uri().contains("alex"));
        assert!(!s.uri().contains('@'));
    }

    #[test]
    fn the_mount_point_is_predicted_not_scraped() {
        let s = parse_address("//127.0.0.1/shared", ShareKind::Smb).unwrap();
        let mp = s.mount_point("/run/user/1000");
        assert_eq!(
            mp.to_string_lossy(),
            "/run/user/1000/gvfs/smb-share:server=127.0.0.1,share=shared"
        );
    }

    #[test]
    fn nfs_has_its_own_scheme_and_mount_name() {
        let s = parse_address("//nas/export", ShareKind::Nfs).unwrap();
        assert_eq!(s.uri(), "nfs://nas/export");
        let mp = s
            .mount_point("/run/user/1000")
            .to_string_lossy()
            .to_string();
        assert!(mp.contains("nfs:host=nas"), "got {mp}");
        // Measured from a real GVfs mount: `prefix=` with the export path
        // percent-encoded, NOT `export=<share>`.
        assert!(mp.contains("prefix=%2Fexport"), "got {mp}");
    }

    #[test]
    fn the_nfs_mount_point_matches_a_real_gvfs_directory() {
        // Captured from a live mount of 127.0.0.1:/tmp/gust-nfs-fixture/export.
        let s = parse_address("//127.0.0.1/tmp/gust-nfs-fixture/export", ShareKind::Nfs).unwrap();
        assert_eq!(
            s.mount_point("/run/user/1000").to_string_lossy(),
            "/run/user/1000/gvfs/nfs:host=127.0.0.1,\
prefix=%2Ftmp%2Fgust-nfs-fixture%2Fexport"
                .replace("\\\n", "")
        );
    }

    #[test]
    fn percent_encoding_matches_gvfs() {
        assert_eq!(percent_encode("/tmp/x"), "%2Ftmp%2Fx");
        assert_eq!(percent_encode("abc-1_2.3~"), "abc-1_2.3~");
    }

    #[test]
    fn a_rejected_login_says_so_plainly() {
        let e = classify("gio: Password is incorrect for share");
        assert_eq!(e, ShareError::AuthFailed);
        assert!(e.message().contains("not accepted"));
    }

    #[test]
    fn permission_denied_is_an_auth_failure_too() {
        assert_eq!(
            classify("gio: smb://nas/x: Permission denied"),
            ShareError::AuthFailed
        );
    }

    #[test]
    fn an_unknown_failure_keeps_gios_words_without_its_prefix() {
        let e = classify("gio: Failed to mount: no route to host");
        match e {
            ShareError::Refused(m) => {
                assert!(m.contains("no route to host"));
                assert!(!m.starts_with("gio: "));
            }
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    #[test]
    fn no_translated_prompt_is_ever_matched() {
        // The exact English prompt GIO prints. If this were being parsed,
        // classify() would find meaning in it — it must not.
        let prompt = "Enter user and password for share \"shared\" on \"nas\":";
        match classify(prompt) {
            ShareError::Refused(_) => {}
            other => panic!("a prompt must never be read as {other:?}"),
        }
    }

    #[test]
    fn errors_are_actionable() {
        assert!(ShareError::NoGvfs.message().contains("gvfs-smb"));
        assert!(ShareError::BadAddress("x".into()).message().contains('x'));
    }

    #[test]
    fn display_names_the_share_and_the_server() {
        let s = parse_address(r"\\nas\family-photos", ShareKind::Smb).unwrap();
        assert_eq!(s.display(), "family-photos on nas");
    }
}
