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
}

/// Where claims live: private, per-user, and outside any directory Gust
/// browses, so a claim file can never itself be mistaken for user data.
pub fn claims_dir() -> PathBuf {
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
    writeln!(
        f,
        "path={}\nboot={}\npid={}\ndev={}\nino={}",
        staged.display(),
        boot_id(),
        std::process::id(),
        md.dev(),
        md.ino()
    )?;
    f.sync_all()?;
    Ok(())
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
        for line in text.lines() {
            match line.split_once('=') {
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
            });
        }
    }
    out
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
        std::env::set_var("XDG_STATE_HOME", &d);
        d
    }

    #[test]
    fn a_claim_records_the_exact_object_it_owns() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = isolated("boot");
        let c = StagingClaim {
            path: d.join("x"),
            dev: 1,
            ino: 2,
            boot_id: "a-different-boot-entirely".into(),
            pid: std::process::id(),
        };
        assert!(
            !owner_is_live(&c),
            "a live PID from another boot must not confer ownership"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn our_own_live_process_is_recognised_as_the_owner() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = isolated("live");
        let c = StagingClaim {
            path: d.join("x"),
            dev: 1,
            ino: 2,
            boot_id: boot_id(),
            pid: std::process::id(),
        };
        assert!(owner_is_live(&c));
        fs::remove_dir_all(&d).ok();
    }
}
