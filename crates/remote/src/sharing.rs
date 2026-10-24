//! Personal-device sharing through the local Tailscale daemon (DESIGN §6.4.1).
//! Reading state never contacts a peer; mutations require an explicit UI action.
use crate::helper::{self, CallContext, ErrorCode, RemoteError};
use serde::Serialize;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const SHARE_NAME: &str = "gust";

fn mutation_lease() -> Result<File, RemoteError> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| RemoteError::new(ErrorCode::PermissionDenied))?;
    mutation_lease_at(Path::new(&runtime))
}

fn mutation_lease_at(runtime: &Path) -> Result<File, RemoteError> {
    use rustix::fs::{Mode, OFlags};
    let refuse = || {
        error(
            ErrorCode::PermissionDenied,
            "Sharing needs a private desktop runtime directory owned by your account.",
        )
    };
    if !runtime.is_absolute() {
        return Err(refuse());
    }
    let directory = File::from(
        rustix::fs::open(
            runtime,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| refuse())?,
    );
    let meta = directory.metadata().map_err(|_| refuse())?;
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(refuse());
    }
    let lock = File::from(
        rustix::fs::openat(
            &directory,
            ".gust-device-sharing.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|_| refuse())?,
    );
    let meta = lock.metadata().map_err(|_| refuse())?;
    if !meta.is_file()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
    {
        return Err(refuse());
    }
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| RemoteError::new(ErrorCode::Busy))?;
    Ok(lock)
}

pub fn is_apple_mobile(os: &str) -> bool {
    os.eq_ignore_ascii_case("ios") || os.eq_ignore_ascii_case("ipados")
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SendTarget {
    pub address: String,
    pub name: String,
    pub online: bool,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub computer_name: String,
    pub targets: Vec<SendTarget>,
    pub send_error: String,
    pub drive_error: String,
    pub shared_path: Option<String>,
    pub shared_label: Option<String>,
    pub shared_token: Option<String>,
    pub default_path: PathBuf,
    pub default_label: String,
    pub default_token: Option<String>,
    pub inbox_path: PathBuf,
}

fn error(code: ErrorCode, message: &str) -> RemoteError {
    RemoteError {
        code,
        message: message.into(),
    }
}

fn remaining<'a>(ctx: &'a CallContext<'a>, started: Instant) -> CallContext<'a> {
    CallContext {
        cancel: ctx.cancel,
        timeout: ctx.timeout.saturating_sub(started.elapsed()),
    }
}

fn call(args: &[&str], ctx: &CallContext<'_>) -> Result<Vec<u8>, RemoteError> {
    let mut command = Command::new("tailscale");
    command.args(args);
    let output = helper::run(command, &[], ctx)?;
    if !output.status.success() {
        // CLI diagnostics can include private account data. Keep them out of the UI.
        return Err(error(ErrorCode::Failed,
            "Tailscale could not complete this action. Check that it is running and signed in, and that your account permits file sharing. For shared folders, review Taildrive setup."));
    }
    Ok(output.stdout)
}

/// Output of the documented CLI target command; addresses are used, never names.
pub fn parse_targets(bytes: &[u8]) -> Result<Vec<SendTarget>, RemoteError> {
    let text = std::str::from_utf8(bytes).map_err(|_| RemoteError::new(ErrorCode::Protocol))?;
    if bytes.len() > 256 * 1024 || text.lines().count() > 1024 {
        return Err(RemoteError::new(ErrorCode::Protocol));
    }
    let mut targets = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let fields: Vec<_> = line.split('\t').collect();
        if !(2..=3).contains(&fields.len())
            || fields[0].parse::<std::net::IpAddr>().is_err()
            || fields[1].is_empty()
            || fields[1].len() > 253
            || fields[1].chars().any(char::is_control)
        {
            return Err(RemoteError::new(ErrorCode::Protocol));
        }
        if targets.iter().any(|t: &SendTarget| t.address == fields[0]) {
            return Err(RemoteError::new(ErrorCode::Protocol));
        }
        targets.push(SendTarget {
            address: fields[0].into(),
            name: fields[1].into(),
            online: fields.len() == 2,
        });
    }
    Ok(targets)
}

/// `drive list` is a padded table, not whitespace-separated paths. Fail closed
/// if its header/columns change instead of guessing about a published folder.
pub fn parse_shares(bytes: &[u8]) -> Result<Vec<(String, String)>, RemoteError> {
    let text = std::str::from_utf8(bytes).map_err(|_| RemoteError::new(ErrorCode::Protocol))?;
    if bytes.len() > 256 * 1024 || text.lines().count() > 1024 {
        return Err(RemoteError::new(ErrorCode::Protocol));
    }
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
    let path_col = header
        .find("path")
        .filter(|n| *n >= 8)
        .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
    let as_col = header
        .find("as")
        .filter(|n| *n >= path_col + 8)
        .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
    if !header.starts_with("name")
        || header[as_col..].trim() != "as"
        || !lines
            .next()
            .is_some_and(|s| s.chars().all(|c| c == '-' || c == ' '))
    {
        return Err(RemoteError::new(ErrorCode::Protocol));
    }
    let mut shares = Vec::new();
    for line in lines {
        let chars: Vec<_> = line.chars().collect();
        if chars.len() < as_col {
            return Err(RemoteError::new(ErrorCode::Protocol));
        }
        let name: String = chars[..path_col]
            .iter()
            .collect::<String>()
            .trim_end()
            .into();
        let path: String = chars[path_col..as_col]
            .iter()
            .collect::<String>()
            .trim_end()
            .into();
        if name.is_empty()
            || !Path::new(&path).is_absolute()
            || name.chars().any(char::is_control)
            || path.chars().any(char::is_control)
            || shares.iter().any(|(n, _)| n == &name)
        {
            return Err(RemoteError::new(ErrorCode::Protocol));
        }
        shares.push((name, path));
    }
    Ok(shares)
}

fn shares(ctx: &CallContext<'_>) -> Result<Vec<(String, String)>, RemoteError> {
    parse_shares(&call(&["drive", "list"], ctx)?)
}

pub fn snapshot(home: &Path, ctx: &CallContext<'_>) -> Snapshot {
    let started = Instant::now();
    let targets = call(&["file", "cp", "--targets"], &remaining(ctx, started))
        .and_then(|s| parse_targets(&s));
    let drive = shares(&remaining(ctx, started));
    let (targets, send_error) = match targets {
        Ok(t) => (t, String::new()),
        Err(e) => (Vec::new(), e.message),
    };
    let (shared_path, drive_error) = match drive {
        Ok(s) => (
            s.into_iter().find(|(n, _)| n == SHARE_NAME).map(|(_, p)| p),
            String::new(),
        ),
        Err(e) => (None, e.message),
    };
    let computer_name = call(&["status", "--json"], &remaining(ctx, started))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|s| {
            s.get("Self")
                .and_then(|s| s.get("DNSName").or_else(|| s.get("HostName")))
                .and_then(|s| s.as_str())
                .map(|s| s.split('.').next().unwrap_or_default().to_owned())
        })
        .filter(|s| !s.is_empty() && s.len() <= 253 && !s.chars().any(char::is_control))
        .unwrap_or_else(|| "this computer".into());
    let shared_label = shared_path
        .as_ref()
        .map(|p| gust_core::location::escape_for_display(Path::new(p).as_os_str()));
    let shared_token = shared_path
        .as_ref()
        .and_then(|p| gust_core::location::PathToken::from_path(Path::new(p)))
        .map(|p| p.to_token());
    let default_path = home.join("Gust Shared");
    let default_label = gust_core::location::escape_for_display(default_path.as_os_str());
    let default_token =
        gust_core::location::PathToken::from_path(&default_path).map(|p| p.to_token());
    Snapshot {
        computer_name,
        targets,
        send_error,
        drive_error,
        shared_path,
        shared_label,
        shared_token,
        default_path,
        default_label,
        default_token,
        inbox_path: home.join("Gust Inbox"),
    }
}

/// Hold every ancestor while opening the next. Reject hidden directories and
/// symlinks, including a symlink swapped in between validation and opening.
fn folder(home: &Path, path: &Path) -> Result<File, RemoteError> {
    let refuse = || {
        error(ErrorCode::InvalidInput,
        "Choose a local folder inside your home, excluding hidden folders and your home itself. Symbolic links and network folders cannot be shared.")
    };
    if !home.is_absolute() || path == home || !path.is_absolute() {
        return Err(refuse());
    }
    let relative = path.strip_prefix(home).map_err(|_| refuse())?;
    let parts: Vec<_> = relative.components().collect();
    if parts.is_empty()
        || parts.iter().any(|c| match c {
            Component::Normal(n) => {
                n.as_encoded_bytes().starts_with(b".")
                    || n.to_str()
                        .is_none_or(|s| gust_core::location::escape_for_display(n) != s)
            }
            _ => true,
        })
    {
        return Err(refuse());
    }
    crate::send::check_source_location(path)?;
    let flags = rustix::fs::OFlags::RDONLY
        | rustix::fs::OFlags::DIRECTORY
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::CLOEXEC;
    let mut held =
        File::from(rustix::fs::open(home, flags, rustix::fs::Mode::empty()).map_err(|_| refuse())?);
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(refuse());
        };
        held = File::from(
            rustix::fs::openat(&held, name, flags, rustix::fs::Mode::empty())
                .map_err(|_| refuse())?,
        );
    }
    let meta = held.metadata().map_err(|_| refuse())?;
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(refuse());
    }
    Ok(held)
}

fn same_folder(held: &File, path: &Path) -> bool {
    held.metadata()
        .ok()
        .zip(fs::symlink_metadata(path).ok())
        .is_some_and(|(a, b)| b.is_dir() && a.dev() == b.dev() && a.ino() == b.ino())
}

/// Called only after the displayed folder and existing policy were confirmed.
pub fn publish(home: &Path, path: &Path, ctx: &CallContext<'_>) -> Result<(), RemoteError> {
    let _lease = mutation_lease()?;
    let started = Instant::now();
    ctx.check()?;
    let text = path
        .to_str()
        .ok_or_else(|| RemoteError::new(ErrorCode::InvalidInput))?;
    let existing = shares(&remaining(ctx, started))?
        .into_iter()
        .find(|(n, _)| n == SHARE_NAME);
    if existing.as_ref().is_some_and(|(_, p)| p != text) {
        return Err(error(ErrorCode::InvalidInput,
            "A folder is already shared as gust. Stop sharing that folder before choosing another one."));
    }
    if path == home.join("Gust Shared") {
        crate::send::check_source_location(path)?;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(RemoteError::new(ErrorCode::Failed)),
        }
    }
    let held = folder(home, path)?;
    ctx.check()?;
    if !same_folder(&held, path) {
        return Err(RemoteError::new(ErrorCode::InvalidInput));
    }
    if existing.is_some() {
        return Ok(());
    }
    call(
        &["drive", "share", SHARE_NAME, text],
        &remaining(ctx, started),
    )?;
    if !same_folder(&held, path) {
        return Err(error(ErrorCode::Failed, "The folder changed while sharing was enabled. Review Tailscale sharing before continuing; no rollback was assumed."));
    }
    let actual = shares(&remaining(ctx, started))?;
    if !actual.iter().any(|(n, p)| n == SHARE_NAME && p == text) {
        return Err(error(
            ErrorCode::Failed,
            "Sharing could not be confirmed. Check again before retrying.",
        ));
    }
    Ok(())
}

pub fn unpublish(expected_path: &str, ctx: &CallContext<'_>) -> Result<(), RemoteError> {
    let _lease = mutation_lease()?;
    let started = Instant::now();
    let current = shares(&remaining(ctx, started))?;
    if !current
        .iter()
        .any(|(n, p)| n == SHARE_NAME && p == expected_path)
    {
        return Err(error(
            ErrorCode::InvalidInput,
            "The shared folder changed. Check again before stopping sharing.",
        ));
    }
    call(&["drive", "unshare", SHARE_NAME], &remaining(ctx, started))?;
    if shares(&remaining(ctx, started))?
        .iter()
        .any(|(n, _)| n == SHARE_NAME)
    {
        return Err(error(
            ErrorCode::Failed,
            "Stopping sharing could not be confirmed. Check again.",
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct Received {
    pub path: PathBuf,
    pub files: usize,
}

/// Claim incoming files only on an explicit request, into a new private batch.
pub fn receive(home: &Path, ctx: &CallContext<'_>) -> Result<Received, RemoteError> {
    let _lease = mutation_lease()?;
    ctx.check()?;
    let inbox = home.join("Gust Inbox");
    crate::send::check_source_location(&inbox)?;
    match fs::DirBuilder::new().mode(0o700).create(&inbox) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(RemoteError::new(ErrorCode::Failed)),
    }
    let inbox_fd = folder(home, &inbox)?;
    if inbox_fd
        .metadata()
        .map_err(|_| RemoteError::new(ErrorCode::Failed))?
        .mode()
        & 0o077
        != 0
    {
        return Err(error(
            ErrorCode::PermissionDenied,
            "Gust Inbox must be a private folder accessible only to your account.",
        ));
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let batch = inbox.join(format!("Received-{nonce}"));
    let private_path = format!("/proc/{}/fd/{}", std::process::id(), inbox_fd.as_raw_fd());
    fs::DirBuilder::new()
        .mode(0o700)
        .create(Path::new(&private_path).join(batch.file_name().unwrap()))
        .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
    let held = folder(home, &batch)?;
    if !same_folder(&inbox_fd, &inbox) || !same_folder(&held, &batch) {
        return Err(RemoteError::new(ErrorCode::InvalidInput));
    }
    let destination = format!("/proc/{}/fd/{}", std::process::id(), held.as_raw_fd());
    let result = call(&["file", "get", "--conflict=rename", &destination], ctx);
    if let Err(mut e) = result {
        e.message
            .push_str(" Some files may already be in Gust Inbox; check it before retrying.");
        return Err(e);
    }
    if !same_folder(&inbox_fd, &inbox) || !same_folder(&held, &batch) {
        return Err(error(
            ErrorCode::Failed,
            "The receiving folder changed. Files may have arrived; no rollback was assumed.",
        ));
    }
    let files = fs::read_dir(&destination)
        .map_err(|_| RemoteError::new(ErrorCode::Failed))?
        .count();
    Ok(Received { path: batch, files })
}

/// Recheck eligibility immediately before sending, without contacting the peer.
pub fn eligible_target(address: &str, ctx: &CallContext<'_>) -> Result<(), RemoteError> {
    let targets = parse_targets(&call(&["file", "cp", "--targets"], ctx)?)?;
    if targets.iter().any(|t| t.address == address && t.online) {
        Ok(())
    } else {
        Err(error(ErrorCode::PermissionDenied, "That device is not an online Taildrop receiver for this account. Check Tailscale on both devices, then check again."))
    }
}

pub fn discovery_timeout() -> Duration {
    Duration::from_secs(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_workers_cannot_mutate_sharing_together_or_follow_a_lock_symlink() {
        let base = std::env::temp_dir().join(format!(
            "gust-sharing-lease-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
        let lease = mutation_lease_at(&base).unwrap();
        assert_eq!(mutation_lease_at(&base).unwrap_err().code, ErrorCode::Busy);
        drop(lease);
        drop(mutation_lease_at(&base).unwrap());
        fs::remove_file(base.join(".gust-device-sharing.lock")).unwrap();
        fs::write(base.join("sentinel"), b"keep").unwrap();
        std::os::unix::fs::symlink(
            base.join("sentinel"),
            base.join(".gust-device-sharing.lock"),
        )
        .unwrap();
        assert!(mutation_lease_at(&base).is_err());
        assert_eq!(fs::read(base.join("sentinel")).unwrap(), b"keep");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn mobile_os_is_not_a_hostname_guess() {
        assert!(is_apple_mobile("iOS"));
        assert!(is_apple_mobile("iPadOS"));
        assert!(!is_apple_mobile("macOS"));
        assert!(!is_apple_mobile("iphone"));
    }

    #[test]
    fn targets_distinguish_eligibility_from_online_state_and_reject_junk() {
        let t =
            parse_targets(b"100.64.0.2\tiphone-15\n100.64.0.3\tipad\toffline; last seen 1h ago\n")
                .unwrap();
        assert!(t[0].online);
        assert!(!t[1].online);
        for bad in [
            "--option\tphone\n",
            "100.64.0.2\t\n",
            "100.64.0.2\tphone\n100.64.0.2\tother\n",
            "broken",
        ] {
            assert!(parse_targets(bad.as_bytes()).is_err());
        }
    }

    #[test]
    fn share_columns_preserve_spaces_and_reject_changed_or_duplicate_tables() {
        let table = b"name    path                      as\n----    ----------------------    --\ngust    /home/alex/Gust Shared    alex\n";
        assert_eq!(
            parse_shares(table).unwrap(),
            vec![("gust".into(), "/home/alex/Gust Shared".into())]
        );
        assert!(parse_shares(b"unexpected output").is_err());
        let mut twice = table.to_vec();
        twice.extend_from_slice(b"gust    /home/alex/Gust Shared    alex\n");
        assert!(parse_shares(&twice).is_err());
        assert_eq!(
            parse_shares(b"name    path    as\n----    ----    --\n")
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn folder_admission_refuses_home_secrets_parent_paths_and_symlinks() {
        let base = std::env::temp_dir().join(format!(
            "gust-sharing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("ordinary")).unwrap();
        fs::create_dir(base.join("ordinary/sub")).unwrap();
        fs::create_dir(base.join(".ssh")).unwrap();
        fs::create_dir(base.join("two\nlines")).unwrap();
        fs::create_dir(base.join("two<U+000A>lines")).unwrap();
        std::os::unix::fs::symlink(base.join("ordinary"), base.join("link")).unwrap();
        assert!(folder(&base, &base.join("ordinary")).is_ok());
        assert!(folder(&base, &base.join("two<U+000A>lines")).is_ok());
        for p in [
            base.clone(),
            base.join(".ssh"),
            base.join("link"),
            base.join("link/sub"),
            base.join("ordinary/../.ssh"),
            base.join("two\nlines"),
            base.join("bidi\u{202e}folder"),
        ] {
            assert!(folder(&base, &p).is_err(), "{p:?}");
        }
        fs::remove_dir_all(base).unwrap();
    }
}
