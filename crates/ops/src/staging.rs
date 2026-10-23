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

fn encode_path(path: &Path) -> String {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn decode_path(hex: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let decoded: Option<Vec<u8>> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect();
    Some(std::ffi::OsString::from_vec(decoded?).into())
}

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
    /// Bytes written before alias cleanup failed; diagnostic, not ownership proof.
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
    let c = StagingClaim {
        path: path.to_path_buf(),
        dev: 0,
        ino: 0,
        boot_id: boot_id(),
        pid: std::process::id(),
        claim_file: cf.clone(),
        since: 0,
        net: true,
        written: 0,
    };
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&cf)?;
    write_record(&mut f, &c)?;
    fs::File::open(&dir)?.sync_all()?;
    Ok(cf)
}

fn write_claim(c: &StagingClaim) -> std::io::Result<()> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // Updates replace only a validated claim owned by this operation. A
    // separate synced record keeps the previous complete record on failure.
    let mut previous = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&c.claim_file)?;
    let before = previous.metadata()?;
    let mut old = String::new();
    previous.read_to_string(&mut old)?;
    let prior = decode_claim(&old, c.claim_file.clone());
    if !before.is_file()
        || !prior.is_some_and(|p| p.path == c.path && p.pid == c.pid && p.boot_id == c.boot_id)
    {
        return Err(std::io::Error::other(
            "staging claim changed; record retained",
        ));
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = c
        .claim_file
        .with_extension(format!("{}-{sequence}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        write_record(&mut f, c)?;
        #[cfg(test)]
        if fault::UPDATE.with(|v| v.get()) {
            return Err(std::io::Error::other("interrupted claim update"));
        }
        use std::os::unix::fs::MetadataExt;
        let current = fs::symlink_metadata(&c.claim_file)?;
        if !current.is_file()
            || before.dev() != current.dev()
            || before.ino() != current.ino()
            || before.mtime() != current.mtime()
            || before.mtime_nsec() != current.mtime_nsec()
            || before.len() != current.len()
        {
            return Err(std::io::Error::other(
                "staging claim changed; record retained",
            ));
        }
        // This rename updates a private local record, never a public file.
        fs::rename(&temporary, &c.claim_file)?;
        #[cfg(test)]
        if fault::UPDATE_AFTER.with(|v| v.get()) {
            return Err(std::io::Error::other("interrupted after claim update"));
        }
        fs::File::open(c.claim_file.parent().unwrap())?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_record(f: &mut fs::File, c: &StagingClaim) -> std::io::Result<()> {
    writeln!(
        f,
        "version=2\npath_hex={}\nboot={}\npid={}\ndev={}\nino={}\nsince={}\nnet={}\nwritten={}",
        encode_path(&c.path),
        c.boot_id,
        c.pid,
        c.dev,
        c.ino,
        c.since,
        u8::from(c.net),
        c.written
    )?;
    f.sync_all()
}

/// Complete a claim with the identity of the file actually created.
pub fn confirm(claim_path: &Path, staged: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    #[cfg(test)]
    if fault::CONFIRM.with(|c| c.get()) {
        return Err(std::io::Error::from_raw_os_error(28));
    }
    let md = staged.symlink_metadata()?;
    // Missing mount evidence is not proof of a stable local inode namespace.
    let net = gust_core::stall::fstype_for(staged).is_none_or(|t| gust_core::stall::can_stall(&t));
    write_claim(&StagingClaim {
        path: staged.to_path_buf(),
        dev: md.dev(),
        ino: md.ino(),
        boot_id: boot_id(),
        pid: std::process::id(),
        claim_file: claim_path.to_path_buf(),
        since: md.mtime(),
        net,
        written: 0,
    })
}

/// Keep the amount written as diagnostic evidence, never as ownership proof.
pub fn mark_unremoved(claim_path: &Path, written: u64) {
    let Ok(text) = fs::read_to_string(claim_path) else {
        return;
    };
    let Some(mut c) = decode_claim(&text, claim_path.to_path_buf()) else {
        return;
    };
    c.written = written;
    let _ = write_claim(&c);
}

/// Is `md` the object a recovery sweep can prove this claim owns?
/// Network inode numbers can change or be reused after reconnection. Neither
/// a matching synthetic inode nor path/size/time proves remote ownership.
/// Retain these claims and their debris rather than deleting a replacement.
pub fn describes(c: &StagingClaim, md: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    !c.net && c.dev != 0 && c.ino != 0 && md.dev() == c.dev && md.ino() == c.ino
}

/// Drop a claim once the staging file has been committed or removed.
pub fn release(claim_path: &Path) {
    let _ = fs::remove_file(claim_path);
}

/// Decode only complete version-2 records. Legacy or uncertain records stay
/// on disk, but confer no deletion authority. No field is defaulted, repeated,
/// or silently ignored; a torn write cannot turn into a different claim.
fn decode_claim(text: &str, claim_file: PathBuf) -> Option<StagingClaim> {
    const KEYS: [&str; 9] = [
        "version", "path_hex", "boot", "pid", "dev", "ino", "since", "net", "written",
    ];
    if !text.ends_with('\n') {
        return None;
    }
    let mut fields = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=')?;
        if !KEYS.contains(&key) || fields.insert(key, value).is_some() {
            return None;
        }
    }
    if fields.len() != KEYS.len() || fields["version"] != "2" {
        return None;
    }
    let path = decode_path(fields["path_hex"])?;
    if !path.is_absolute()
        || encode_path(&path) != fields["path_hex"]
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    let net = match fields["net"] {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let pid = fields["pid"].parse().ok()?;
    if pid == 0 || fields["boot"].is_empty() {
        return None;
    }
    Some(StagingClaim {
        path,
        dev: fields["dev"].parse().ok()?,
        ino: fields["ino"].parse().ok()?,
        boot_id: fields["boot"].to_owned(),
        pid,
        claim_file,
        since: fields["since"].parse().ok()?,
        net,
        written: fields["written"].parse().ok()?,
    })
}

/// Read validated claims only; uncertain records are retained without action.
pub fn load_all() -> Vec<StagingClaim> {
    use std::io::Read;
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(claims_dir()) else {
        return out;
    };
    for e in rd.flatten() {
        if e.path().extension().and_then(|s| s.to_str()) != Some("claim") {
            continue;
        }
        let Ok(fd) = rustix::fs::open(
            e.path(),
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) else {
            continue;
        };
        let mut file = fs::File::from(fd);
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        let mut text = String::new();
        if file.read_to_string(&mut text).is_err() {
            continue;
        }
        if let Some(c) = decode_claim(&text, e.path()) {
            out.push(c);
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
        pub(crate) static UPDATE: Cell<bool> = const { Cell::new(false) };
        pub(crate) static UPDATE_AFTER: Cell<bool> = const { Cell::new(false) };
        pub(crate) static DIR: std::cell::RefCell<Option<std::path::PathBuf>> =
            const { std::cell::RefCell::new(None) };
    }

    /// This test thread's claim store lives under `state` (as
    /// XDG_STATE_HOME would put it).
    pub(crate) fn state_home(state: &std::path::Path) {
        DIR.with(|d| *d.borrow_mut() = Some(state.to_path_buf()));
    }

    pub(crate) fn rewrite_claim(
        path: &std::path::Path,
        edit: impl FnOnce(&mut super::StagingClaim),
    ) {
        let text = std::fs::read_to_string(path).unwrap();
        let mut claim = super::decode_claim(&text, path.to_path_buf()).unwrap();
        edit(&mut claim);
        let mut f = std::fs::File::create(path).unwrap();
        super::write_record(&mut f, &claim).unwrap();
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

#[cfg(test)]
mod byte_path_tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn claims_round_trip_non_utf8_and_newline_paths_without_field_injection() {
        let root = std::env::temp_dir().join(format!("gust-byte-claims-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fault::state_home(&root.join("state"));
        let path = root.join(std::ffi::OsString::from_vec(
            b"stage-\xff\nnet=1\npid=0".to_vec(),
        ));
        let claim = claim(&path).unwrap();
        fs::write(&path, b"complete").unwrap();
        confirm(&claim, &path).unwrap();
        let records = load_all();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, path);
        assert_eq!(records[0].pid, std::process::id());
        assert!(!records[0].net);
        assert!(describes(&records[0], &path.symlink_metadata().unwrap()));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod record_validation_tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-claim-validation-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&p).unwrap();
        fault::state_home(&p.join("state"));
        p
    }

    #[test]
    fn legacy_malformed_duplicate_and_torn_records_never_authorize_cleanup() {
        let root = root("invalid");
        let path = root.join("stage");
        let other = root.join("bystander");
        fs::write(&path, b"owned bytes").unwrap();
        fs::write(&other, b"BYSTANDER").unwrap();
        let cf = claim(&path).unwrap();
        confirm(&cf, &path).unwrap();
        let valid = fs::read_to_string(&cf).unwrap();
        let encoded = encode_path(&path);
        let legacy = valid.replace("version=2\n", "").replace(
            &format!("path_hex={encoded}"),
            &format!("path={}", path.display()),
        );
        let cases = [
            legacy,
            valid.replace("version=2", "version=1"),
            valid.replace(&format!("path_hex={encoded}"), "path_hex=zz"),
            valid.replace(&format!("path_hex={encoded}"), "path_hex=2"),
            valid.trim_end().to_owned(),
            format!("{valid}net=0\n"),
            format!("{valid}path_hex={}\n", encode_path(&other)),
            format!("{valid}path={}\n", other.display()),
            format!("{valid}extra=unrecognized\n"),
            valid.replace("net=0", "net=unknown"),
            valid.replace("written=0\n", ""),
            valid.replace("written=0", "written=invalid"),
        ];
        for text in cases {
            fs::write(&cf, &text).unwrap();
            assert!(load_all().is_empty());
            assert_eq!(crate::engine::sweep_stale_staging(&root), 0);
            assert_eq!(crate::engine::prune_dead_claims(), 0);
            mark_unremoved(&cf, 8);
            assert_eq!(
                fs::read_to_string(&cf).unwrap(),
                text,
                "an invalid claim must not be repaired into authority"
            );
            assert_eq!(fs::read(&path).unwrap(), b"owned bytes");
            assert_eq!(fs::read(&other).unwrap(), b"BYSTANDER");
        }
        // An exact complete current record retains ordinary local cleanup.
        fs::write(&cf, valid).unwrap();
        assert_eq!(crate::engine::sweep_stale_staging(&root), 1);
        assert!(!cf.exists());
        assert_eq!(fs::read(other).unwrap(), b"BYSTANDER");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn equal_lossy_path_labels_cannot_redirect_a_claim_to_another_file() {
        let root = root("collision");
        let path = root.join(std::ffi::OsString::from_vec(b"stage-\xff".to_vec()));
        let other = root.join(std::ffi::OsString::from_vec(b"stage-\xfe".to_vec()));
        assert_eq!(path.to_string_lossy(), other.to_string_lossy());
        assert_ne!(path, other);
        fs::write(&path, b"owned bytes").unwrap();
        fs::write(&other, b"BYSTANDER").unwrap();
        let cf = claim(&path).unwrap();
        confirm(&cf, &path).unwrap();
        assert_eq!(load_all()[0].path, path);
        assert_eq!(crate::engine::sweep_stale_staging(&root), 1);
        assert!(!path.exists());
        assert_eq!(fs::read(other).unwrap(), b"BYSTANDER");
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod exclusive_update_tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-claim-update-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&p).unwrap();
        fault::state_home(&p.join("state"));
        fs::create_dir_all(claims_dir()).unwrap();
        p
    }

    #[test]
    fn initial_claim_never_replaces_an_existing_record_or_symlink() {
        let root = root("exclusive");
        let path = root.join("stage");
        let cf = claim_file_for(&path);
        fs::write(&cf, b"existing unresolved record").unwrap();
        assert!(claim(&path).is_err());
        assert_eq!(fs::read(&cf).unwrap(), b"existing unresolved record");
        fs::remove_file(&cf).unwrap();
        let other = root.join("bystander");
        fs::write(&other, b"BYSTANDER").unwrap();
        std::os::unix::fs::symlink(&other, &cf).unwrap();
        assert!(claim(&path).is_err());
        fs::write(&path, b"complete").unwrap();
        assert!(confirm(&cf, &path).is_err());
        assert_eq!(fs::read_link(cf).unwrap(), other);
        assert_eq!(fs::read(other).unwrap(), b"BYSTANDER");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_updates_leave_complete_prior_or_new_record() {
        let root = root("interrupt");
        let path = root.join("stage");
        let cf = claim(&path).unwrap();
        let before = fs::read(&cf).unwrap();
        fs::write(&path, b"complete").unwrap();
        fault::UPDATE.with(|v| v.set(true));
        let interrupted = confirm(&cf, &path);
        fault::UPDATE.with(|v| v.set(false));
        assert!(interrupted.is_err());
        assert_eq!(fs::read(&cf).unwrap(), before);
        assert_eq!(load_all()[0].dev, 0);
        assert_eq!(crate::engine::sweep_stale_staging(&root), 0);
        fault::UPDATE_AFTER.with(|v| v.set(true));
        let interrupted = confirm(&cf, &path);
        fault::UPDATE_AFTER.with(|v| v.set(false));
        assert!(interrupted.is_err());
        let after = fs::read(&cf).unwrap();
        assert_ne!(before, after);
        assert!(describes(&load_all()[0], &path.symlink_metadata().unwrap()));
        assert_eq!(fs::read(&path).unwrap(), b"complete");
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod symlink_record_tests {
    use super::*;

    #[test]
    fn a_symlink_to_a_valid_claim_confers_no_cleanup_authority() {
        let root = std::env::temp_dir().join(format!("gust-linked-claim-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fault::state_home(&root.join("state"));
        let stage = root.join("stage");
        fs::write(&stage, b"complete").unwrap();
        let cf = claim(&stage).unwrap();
        confirm(&cf, &stage).unwrap();
        let saved = root.join("saved-record");
        fs::rename(&cf, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &cf).unwrap();
        assert!(load_all().is_empty());
        assert_eq!(crate::engine::sweep_stale_staging(&root), 0);
        assert_eq!(crate::engine::prune_dead_claims(), 0);
        assert_eq!(fs::read(&stage).unwrap(), b"complete");
        assert_eq!(fs::read_link(cf).unwrap(), saved);
        fs::remove_dir_all(root).unwrap();
    }
}
