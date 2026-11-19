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

use std::ffi::{c_char, c_void, CStr, CString, OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};
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

    /// GVfs names an NFS mount after the exact export path.
    pub fn nfs_mount_point(&self, runtime_dir: &str) -> PathBuf {
        let path = if self.share.starts_with('/') {
            self.share.clone()
        } else {
            format!("/{}", self.share)
        };
        let name = format!("nfs:host={},prefix={}", self.server, percent_encode(&path));
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

pub const SMB_PATH_HELPER_ARG: &str = "--smb-local-path";

#[link(name = "gio-2.0")]
unsafe extern "C" {
    fn g_file_new_for_uri(uri: *const c_char) -> *mut c_void;
    fn g_file_get_path(file: *mut c_void) -> *mut c_char;
}

#[link(name = "gobject-2.0")]
unsafe extern "C" {
    fn g_object_unref(object: *mut c_void);
}

#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn g_free(ptr: *mut c_void);
}

/// Headless Gust entry point. Return the mount tracker's path as OS bytes
/// without querying the folder or formatting it for display.
pub fn serve_smb_local_path(uri: &OsStr) -> i32 {
    let bytes = uri.as_bytes();
    if !bytes.starts_with(b"smb://") {
        return 1;
    }
    let Ok(uri) = CString::new(bytes) else {
        return 1;
    };
    // SAFETY: GIO takes a NUL-terminated URI, returns an owned GFile and
    // either a new GLib-allocated path string or null. Release each once.
    let path = unsafe {
        let file = g_file_new_for_uri(uri.as_ptr());
        if file.is_null() {
            return 1;
        }
        let path = g_file_get_path(file);
        g_object_unref(file);
        if path.is_null() {
            return 1;
        }
        let bytes = CStr::from_ptr(path).to_bytes().to_vec();
        g_free(path.cast());
        bytes
    };
    i32::from(std::io::stdout().lock().write_all(&path).is_err())
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
    smb_helper: Option<&Path>,
) -> Result<PathBuf, ShareError> {
    if !gvfs_available() {
        return Err(ShareError::NoGvfs);
    }
    // The desktop uses its own executable before Qt starts. Fixture tests
    // pass the Gust binary they built, since their executable is libtest.
    let executable = smb_helper.unwrap_or_else(|| Path::new("/proc/self/exe"));
    mount_with_command(
        share,
        runtime_dir,
        mount_command(share, password.is_empty()),
        password,
        timeout,
        |uri, budget| smb_local_path(executable, uri, budget),
    )
}

fn mount_with_command(
    share: &Share,
    runtime_dir: &str,
    command: Command,
    password: String,
    timeout: Duration,
    resolve_smb: impl Fn(&str, Duration) -> Option<PathBuf>,
) -> Result<PathBuf, ShareError> {
    let deadline = Instant::now() + timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let smb_uri = share.uri();
    let nfs_path = (share.kind == ShareKind::Nfs).then(|| share.nfs_mount_point(runtime_dir));

    let existing_mount = || {
        if let Some(path) = &nfs_path {
            return path.is_dir().then(|| Ok(path.clone()));
        }

        // Offer the canonical share root. The mount tracker can map an SMB
        // subfolder even when that subfolder has not been created.
        let root = share.share.split('/').next().unwrap_or_default();
        let root_uri = format!("smb://{}/{root}", share.server);
        let folder = resolve_smb(&root_uri, remaining().min(Duration::from_secs(1)))?;
        Some(Err(ShareError::AlreadyMounted(folder)))
    };
    if let Some(existing) = existing_mount() {
        return existing;
    }

    if let Err(error) = run_mount_command(share, command, password, remaining()) {
        return existing_mount().unwrap_or(Err(error));
    }
    if let Some(path) = nfs_path {
        return Ok(path);
    }

    let ready_by = Instant::now() + remaining().min(Duration::from_secs(1));
    let time_left = || ready_by.saturating_duration_since(Instant::now());
    let path = loop {
        if let Some(path) = resolve_smb(&smb_uri, time_left()) {
            break path;
        }
        if time_left().is_zero() {
            return Err(ShareError::Refused(
                "The share is mounted, but GIO could not resolve its local folder.".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(25).min(time_left()));
    };
    wait_for_smb_folder(path, remaining())
}

fn run_mount_command(
    share: &Share,
    mut command: Command,
    password: String,
    timeout: Duration,
) -> Result<(), ShareError> {
    if timeout.is_zero() {
        return Err(ShareError::Refused(
            "The server did not answer in time.".into(),
        ));
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
                    return Ok(());
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

fn smb_local_path(executable: &Path, uri: &str, timeout: Duration) -> Option<PathBuf> {
    let cancel = gust_core::lister::CancelToken::new();
    let ctx = crate::helper::CallContext {
        cancel: &cancel,
        timeout,
    };
    let mut command = Command::new(executable);
    command.args([SMB_PATH_HELPER_ARG, uri]);
    let output = crate::helper::run(command, &[], &ctx).ok()?;
    if !output.status.success() {
        return None;
    }
    path_from_helper(output.stdout)
}

fn path_from_helper(bytes: Vec<u8>) -> Option<PathBuf> {
    if bytes.contains(&0) {
        return None;
    }
    let path = PathBuf::from(OsString::from_vec(bytes));
    (path.is_absolute() && !path.components().any(|part| part == Component::ParentDir))
        .then_some(path)
}

fn mount_command(share: &Share, password_empty: bool) -> Command {
    let mut command = Command::new("gio");
    command.arg("mount");
    if share.kind == ShareKind::Smb
        && share.user.is_none()
        && share.domain.is_none()
        && password_empty
    {
        // This answers an authentication prompt. GVfs may have already
        // authenticated with session or keyring credentials before asking.
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
    fn automatic_smb_login_requests_guest_fallback() {
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
        for share in [&guest, &named, &nfs] {
            let path = match share.kind {
                ShareKind::Smb => runtime.join("gvfs/smb-share:server=nas,share=public"),
                ShareKind::Nfs => share.nfs_mount_point(runtime_dir),
            };
            let resolved = |_: &str, _: Duration| path.is_dir().then(|| path.clone());
            let expected = match share.kind {
                ShareKind::Smb => Err(ShareError::AlreadyMounted(path.clone())),
                ShareKind::Nfs => Ok(path.clone()),
            };

            std::fs::create_dir(&path).unwrap();
            let unused = Command::new(runtime.join("must-not-run"));
            assert_eq!(
                mount_with_command(
                    share,
                    runtime_dir,
                    unused,
                    String::new(),
                    Duration::from_secs(3),
                    resolved,
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
                    runtime_dir,
                    failed,
                    String::new(),
                    Duration::from_secs(3),
                    resolved,
                ),
                expected,
                "a failed command must also respect the reuse policy"
            );
            std::fs::remove_dir(&path).unwrap();
        }
        std::fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn smb_mount_allows_slow_directory_readiness_within_the_deadline() {
        let runtime =
            std::env::temp_dir().join(format!("gust-smb-readiness-{}", std::process::id()));
        std::fs::create_dir(&runtime).unwrap();
        let path = runtime.join("share");
        let share = parse_address("//nas/public", ShareKind::Smb).unwrap();
        let connect = |timeout| {
            let queries = std::cell::Cell::new(0);
            mount_with_command(
                &share,
                runtime.to_str().unwrap(),
                Command::new("true"),
                String::new(),
                timeout,
                |_, _| {
                    let attempts = queries.get();
                    queries.set(attempts + 1);
                    (attempts > 0).then(|| path.clone())
                },
            )
        };
        let unready = connect(Duration::from_millis(200));
        assert!(unready.unwrap_err().message().contains("still unavailable"));

        let fixture = path.clone();
        let publish = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1250));
            std::fs::create_dir(fixture).unwrap();
        });
        let mounted = connect(Duration::from_secs(4));
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
    fn smb_path_helper_preserves_folder_name_bytes() {
        let path = b"/run/user/1000/gvfs/smb-share:server=nas,share=public/Release\nNotes\xff";
        assert_eq!(
            path_from_helper(path.to_vec())
                .unwrap()
                .as_os_str()
                .as_bytes(),
            path
        );
        for unusable in [
            b"relative/path".as_slice(),
            b"/run/x/../y",
            b"/run/x\0y",
            b"",
        ] {
            assert_eq!(path_from_helper(unusable.to_vec()), None);
        }
    }

    #[test]
    fn nfs_has_its_own_scheme_and_mount_name() {
        let s = parse_address("//nas/export", ShareKind::Nfs).unwrap();
        assert_eq!(s.uri(), "nfs://nas/export");
        let mp = s
            .nfs_mount_point("/run/user/1000")
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
            s.nfs_mount_point("/run/user/1000").to_string_lossy(),
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
