//! Who owns a staging file.
//!
//! A staging filename is not an ownership record. The sweep that cleans up
//! crash debris used to decide by NAME SHAPE plus "is that PID alive?", which
//! deletes a user's file called `.gust-staging-999999-report.txt` the moment
//! no process 999999 exists. PIDs are also recycled, and a remote filesystem's
//! PIDs mean nothing locally.
//!
//! So ownership is recorded BEFORE the staging file is created, in a private
//! per-boot directory, and revalidated against the exact object (device +
//! inode) before anything is removed. No record, no removal — the file is
//! left alone and reported instead.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One staging file this process created and has not yet committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingClaim {
    /// Absolute path of the staging file.
    pub path: PathBuf,
    /// Device and inode of the staging file as we created it.
    pub dev: u64,
    pub ino: u64,
    /// The boot this claim belongs to. A claim from an earlier boot can never
    /// match a live PID, and its files are genuinely abandoned.
    pub boot_id: String,
    pub pid: u32,
    /// The claim file this was read from, so a sweep that removes the
    /// staging file can drop the record with it.
    pub claim_file: PathBuf,
    /// Staging file's mtime when the claim was confirmed.
    pub since: i64,
    /// Created on a network filesystem, where inode numbers are per mount.
    pub net: bool,
    /// Bytes written before a failure left the file stranded; 0 otherwise.
    pub written: u64,
}

/// Where claims live: private, per-user, and outside any directory Gust
/// browses, so a claim file can never itself be mistaken for user data.
pub fn claims_dir() -> PathBuf {
    // Tests run in parallel threads and several point XDG_STATE_HOME at a
    // folder they later delete; each test thread gets its own store instead.
    #[cfg(test)]
    {
        if let Some(d) = fault::DIR.with(|d| d.borrow().clone()) {
            return d.join("gust/staging-claims");
        }
        return std::env::temp_dir().join(format!("gust-test-claims-{}", std::process::id()));
    }
    #[allow(unreachable_code)]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("gust/staging-claims")
}

/// This boot's identifier. Ties a claim to a kernel lifetime so a recycled PID
/// from a later boot cannot inherit an old claim's authority.
pub fn boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown-boot".to_string())
}

fn claim_file_for(path: &Path) -> PathBuf {
    // Hash the path so the claim filename cannot collide or leak a long name.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    claims_dir().join(format!("{}-{h:016x}.claim", std::process::id()))
}

/// Record ownership of a staging file BEFORE creating it.
///
/// Written and fsynced first: a crash between the claim and the file leaves a
/// harmless stale claim, whereas a file with no claim would be undeletable
/// debris forever.
pub fn claim(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(test)]
    if fault::CLAIM.with(|c| c.get()) {
        return Err(std::io::Error::from_raw_os_error(28));
    }
    let dir = claims_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let cf = claim_file_for(path);
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&cf)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(fs::Permissions::from_mode(0o600));
    }
    // dev/ino are filled in by `confirm` once the file exists; the path and
    // provenance are what must be durable before creation.
    writeln!(
        f,
        "path={}\nboot={}\npid={}\ndev=0\nino=0",
        path.display(),
        boot_id(),
        std::process::id()
    )?;
    f.sync_all()?;
    Ok(cf)
}

/// Complete a claim with the identity of the file actually created.
pub fn confirm(claim_path: &Path, staged: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    #[cfg(test)]
    if fault::CONFIRM.with(|c| c.get()) {
        return Err(std::io::Error::from_raw_os_error(28));
    }
    let md = staged.symlink_metadata()?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(claim_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(fs::Permissions::from_mode(0o600));
    }
    // A network filesystem (sshfs without use_ino, SMB, rclone) numbers
    // inodes per mount, so after a reconnect dev+ino can never match. Say
    // so now, while the mount is known to be good.
    let net = gust_core::stall::fstype_for(staged).is_some_and(|t| gust_core::stall::can_stall(&t));
    writeln!(
        f,
        "path={}\nboot={}\npid={}\ndev={}\nino={}\nsince={}\nnet={}",
        staged.display(),
        boot_id(),
        std::process::id(),
        md.dev(),
        md.ino(),
        md.mtime(),
        u8::from(net)
    )?;
    f.sync_all()?;
    Ok(())
}

/// A copy failed and its staging file could not be removed (the place it
/// was on stopped answering). Record how many bytes Gust had written, so a
/// later sweep can recognise the file on a network place whose inode
/// numbers changed with the reconnect.
pub fn mark_unremoved(claim_path: &Path, written: u64) {
    if let Ok(mut f) = fs::OpenOptions::new().append(true).open(claim_path) {
        let _ = writeln!(f, "written={written}");
        let _ = f.sync_all();
    }
}

/// Is `md` (what is at a claim's path now) the file this claim describes?
///
/// Normally dev+inode. On a network place, where inode numbers do not
/// survive a remount, a weaker but still specific proof is accepted — only
/// for a copy that failed with the file stranded: a regular file at the
/// exact staging path (a name containing the owner's process id), no
/// larger than what Gust wrote — the last writes may have died in flight,
/// but a larger file means somebody else wrote to it — and not older than
/// the claim.
pub fn describes(c: &StagingClaim, md: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    if c.dev != 0 && c.ino != 0 && md.dev() == c.dev && md.ino() == c.ino {
        return true;
    }
    c.net && c.written > 0 && md.is_file() && md.len() <= c.written && md.mtime() >= c.since
}

/// Drop a claim once the staging file has been committed or removed.
pub fn release(claim_path: &Path) {
    let _ = fs::remove_file(claim_path);
}

/// Read every claim on disk.
pub fn load_all() -> Vec<StagingClaim> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(claims_dir()) else {
        return out;
    };
    for e in rd.flatten() {
        if e.path().extension().and_then(|s| s.to_str()) != Some("claim") {
            continue;
        }
        let Ok(text) = fs::read_to_string(e.path()) else {
            continue;
        };
        let mut path = None;
        let mut boot = String::new();
        let mut pid = 0u32;
        let mut dev = 0u64;
        let mut ino = 0u64;
        let mut since = 0i64;
        let mut net = false;
        let mut written = 0u64;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("since", v)) => since = v.parse().unwrap_or(0),
                Some(("net", v)) => net = v == "1",
                Some(("written", v)) => written = v.parse().unwrap_or(0),
                Some(("path", v)) => path = Some(PathBuf::from(v)),
                Some(("boot", v)) => boot = v.to_string(),
                Some(("pid", v)) => pid = v.parse().unwrap_or(0),
                Some(("dev", v)) => dev = v.parse().unwrap_or(0),
                Some(("ino", v)) => ino = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        if let Some(p) = path {
            out.push(StagingClaim {
                path: p,
                dev,
                ino,
                boot_id: boot,
                pid,
                claim_file: e.path(),
                since,
                net,
                written,
            });
        }
    }
    out
}

/// Staging files this process is writing right now. A claim owned by this
/// process whose path is NOT here belongs to a copy that already failed —
/// typically one whose cleanup could not reach a remote that had gone —
/// and is debris even though its owner (us) is alive.
static ACTIVE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Mark a staging path as being written by this process.
pub fn begin(path: &Path) {
    if let Ok(mut a) = ACTIVE.lock() {
        a.push(path.to_path_buf());
    }
}

/// The copy writing `path` has finished, one way or another.
pub fn end(path: &Path) {
    if let Ok(mut a) = ACTIVE.lock() {
        if let Some(i) = a.iter().position(|p| p == path) {
            a.swap_remove(i);
        }
    }
}

/// May a sweep remove this claim's file? Never while any process —
/// including this one — is still writing it.
pub fn is_abandoned(c: &StagingClaim) -> bool {
    if c.boot_id == boot_id() && c.pid == std::process::id() {
        return !ACTIVE.lock().map(|a| a.contains(&c.path)).unwrap_or(true);
    }
    !owner_is_live(c)
}

/// Is this claim's owning process still running in this same boot?
pub fn owner_is_live(c: &StagingClaim) -> bool {
    if c.boot_id != boot_id() {
        // A different kernel lifetime: that PID cannot be the same process.
        return false;
    }
    if c.pid == 0 {
        return true; // unknown: conservative, never reclaim
    }
    Path::new(&format!("/proc/{}", c.pid)).exists()
}

/// Per-thread failure injection, so a test can make the claim store fail
/// without pointing the process-wide XDG_STATE_HOME at something broken
/// under every other test running in parallel.
#[cfg(test)]
pub(crate) mod fault {
    use std::cell::Cell;
    thread_local! {
        pub(crate) static CLAIM: Cell<bool> = const { Cell::new(false) };
        pub(crate) static CONFIRM: Cell<bool> = const { Cell::new(false) };
        pub(crate) static DIR: std::cell::RefCell<Option<std::path::PathBuf>> =
            const { std::cell::RefCell::new(None) };
    }

    /// This test thread's claim store lives under `state` (as
    /// XDG_STATE_HOME would put it).
    pub(crate) fn state_home(state: &std::path::Path) {
        DIR.with(|d| *d.borrow_mut() = Some(state.to_path_buf()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gust-claims-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fault::state_home(&d);
        d
    }

    #[test]
    fn a_claim_records_the_exact_object_it_owns() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = isolated("exact");
        let staged = d.join(".gust-staging-1-x");
        let cf = claim(&staged).unwrap();
        fs::write(&staged, b"partial").unwrap();
        confirm(&cf, &staged).unwrap();

        let all = load_all();
        let mine: Vec<_> = all.iter().filter(|c| c.path == staged).collect();
        assert_eq!(mine.len(), 1, "exactly one claim for the staged file");
        assert!(mine[0].dev != 0 && mine[0].ino != 0, "identity recorded");
        assert_eq!(mine[0].boot_id, boot_id());
        release(&cf);
        assert!(
            !load_all().iter().any(|c| c.path == staged),
            "released claim is gone"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_claim_from_another_boot_is_never_treated_as_live() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = isolated("boot");
        let c = StagingClaim {
            path: d.join("x"),
            dev: 1,
            ino: 2,
            boot_id: "a-different-boot-entirely".into(),
            pid: std::process::id(),
            claim_file: PathBuf::new(),
            since: 0,
            net: false,
            written: 0,
        };
        assert!(
            !owner_is_live(&c),
            "a live PID from another boot must not confer ownership"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn our_own_live_process_is_recognised_as_the_owner() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = isolated("live");
        let c = StagingClaim {
            path: d.join("x"),
            dev: 1,
            ino: 2,
            boot_id: boot_id(),
            pid: std::process::id(),
            claim_file: PathBuf::new(),
            since: 0,
            net: false,
            written: 0,
        };
        assert!(owner_is_live(&c));
        fs::remove_dir_all(&d).ok();
    }
}
