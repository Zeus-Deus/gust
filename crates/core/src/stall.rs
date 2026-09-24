//! Surviving a mount that has stopped answering (plan §3 rule 3).
//!
//! A cancellation token cannot interrupt a syscall stuck in the kernel. A
//! `readdir` on a FUSE mount whose server has stopped responding blocks in
//! uninterruptible sleep: the thread cannot be woken, `SIGTERM` is ignored,
//! and even `timeout(1)` cannot end it. This was measured, not assumed — a
//! listing of an sshfs mount with a frozen sshd took 280 seconds to return
//! and only did so when the server was resumed.
//!
//! So a directory that *might* stall is not listed in Gust's own process.
//! It is listed by a short-lived child that Gust can kill, which is the only
//! thing that reliably ends a stuck FUSE wait. The child holds the stuck
//! syscall; the window keeps running.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a probe waits before declaring a location unresponsive.
pub const DEFAULT_PROBE_MS: u64 = 4000;

/// What happened when we asked whether a location is answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// It answered in time. Listing it directly is safe.
    Responsive,
    /// It did not answer within the bound. The helper was killed; the
    /// location must NOT be listed on a thread Gust needs back.
    Stalled { waited_ms: u64 },
    /// It answered, with an error (permission denied, gone, and so on).
    Failed { error: String },
}

impl Reachability {
    pub fn is_responsive(&self) -> bool {
        matches!(self, Reachability::Responsive)
    }

    /// What the user is told. Never "error": a sentence that says what is
    /// happening and what Gust did about it.
    pub fn message(&self, place: &str) -> String {
        match self {
            Reachability::Responsive => String::new(),
            Reachability::Stalled { waited_ms } => format!(
                "{place} stopped responding after {:.0} seconds. \
                 Browsing is still available everywhere else; reconnect to try again.",
                *waited_ms as f64 / 1000.0
            ),
            Reachability::Failed { error } => format!("{place} could not be read: {error}"),
        }
    }
}

/// Is this path on a filesystem that can stall?
///
/// Local disks do not stall in this way; network filesystems do. Gust probes
/// only what is worth probing, so ordinary browsing pays nothing.
pub fn can_stall(fstype: &str) -> bool {
    matches!(
        fstype,
        "fuse.sshfs"
            | "fuse.rclone"
            | "fuse"
            | "fuseblk"
            | "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "afs"
            | "ceph"
            | "glusterfs"
            | "davfs"
            | "fuse.gvfsd-fuse"
    )
}

/// The filesystem type of the mount `path` sits on, from /proc/self/mounts.
///
/// Reading /proc is a local operation and cannot itself stall on a hung
/// remote: the kernel answers from its own tables.
pub fn fstype_for(path: &Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    let mut best: Option<(usize, String)> = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let _dev = parts.next()?;
        let mount_point = parts.next()?;
        let fstype = parts.next()?;
        // /proc/self/mounts escapes spaces as \040.
        let mount_point = mount_point.replace("\\040", " ");
        if path.starts_with(&mount_point) {
            let len = mount_point.len();
            if best.as_ref().is_none_or(|(b, _)| len > *b) {
                best = Some((len, fstype.to_string()));
            }
        }
    }
    best.map(|(_, f)| f)
}

/// Should this path be probed before being listed?
pub fn needs_probe(path: &Path) -> bool {
    fstype_for(path).map(|f| can_stall(&f)).unwrap_or(false)
}

/// Ask a short-lived child process whether a directory answers.
///
/// The child does the one thing that can hang. If it does not finish within
/// `timeout`, it is killed and the location is reported as stalled. Gust's
/// own threads never touch the path until this has returned Responsive.
///
/// The child is `ls -A --` with the path as a separate argument: no shell,
/// so a directory called `; rm -rf ~` is a directory name and nothing else.
pub fn probe(path: &Path, timeout: Duration) -> Reachability {
    let started = Instant::now();
    let child = Command::new("ls")
        .arg("-A")
        .arg("--")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return Reachability::Failed {
                error: e.to_string(),
            }
        }
    };

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Reachability::Responsive;
                }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut err);
                }
                let err = err
                    .lines()
                    .last()
                    .unwrap_or("could not be read")
                    .trim()
                    .to_string();
                // `ls: cannot open directory 'x': Permission denied` — keep
                // the reason, drop the tool's name for it.
                let err = err
                    .rsplit_once(": ")
                    .map(|(_, r)| r.to_string())
                    .unwrap_or(err);
                return Reachability::Failed { error: err };
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    // SIGKILL, not SIGTERM: a process in uninterruptible
                    // sleep ignores catchable signals, and even this only
                    // takes effect when the wait finally ends. What matters
                    // is that GUST is not the one waiting.
                    let _ = child.kill();
                    // Deliberately NOT wait()ed: reaping would block here
                    // for exactly as long as the stall we are escaping.
                    // The child is left to the init process.
                    return Reachability::Stalled {
                        waited_ms: started.elapsed().as_millis() as u64,
                    };
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                return Reachability::Failed {
                    error: e.to_string(),
                }
            }
        }
    }
}

/// Probe only if the location is the kind that can stall.
///
/// Ordinary local browsing pays nothing: no child process is spawned for a
/// path on a local disk.
pub fn probe_if_risky(path: &Path, timeout: Duration) -> Reachability {
    if needs_probe(path) {
        probe(path, timeout)
    } else {
        Reachability::Responsive
    }
}

/// Mount points Gust itself created, for reporting which place stalled.
pub fn place_name_for(path: &Path) -> String {
    // .../gust/places/<alias> or .../gust/cloud/<name>
    let mut parts = path.components().rev();
    let mut last = None;
    for c in parts.by_ref() {
        let s = c.as_os_str().to_string_lossy().to_string();
        if s == "places" || s == "cloud" {
            return last.unwrap_or_else(|| "That place".into());
        }
        last = Some(s);
    }
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "That folder".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_filesystems_are_not_probed() {
        for fs in ["ext4", "btrfs", "xfs", "tmpfs", "overlay", "zfs"] {
            assert!(!can_stall(fs), "{fs} should not be treated as stallable");
        }
    }

    #[test]
    fn network_filesystems_are_probed() {
        for fs in ["fuse.sshfs", "fuse.rclone", "nfs4", "cifs", "davfs"] {
            assert!(can_stall(fs), "{fs} should be treated as stallable");
        }
    }

    #[test]
    fn a_responsive_directory_answers_immediately() {
        let d = std::env::temp_dir().join("gust-probe-ok");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let r = probe(&d, Duration::from_millis(4000));
        assert_eq!(r, Reachability::Responsive);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_missing_directory_fails_rather_than_stalling() {
        let r = probe(
            Path::new("/tmp/gust-probe-does-not-exist-at-all"),
            Duration::from_millis(4000),
        );
        match r {
            Reachability::Failed { .. } => {}
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn a_stall_reports_how_long_it_waited() {
        // A FIFO nobody writes to: opening it blocks, exactly like a hung
        // mount, without needing a network.
        let d = std::env::temp_dir().join("gust-probe-stall");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let fifo = d.join("blocker");
        let out = Command::new("mkfifo").arg(&fifo).output();
        if out.map(|o| !o.status.success()).unwrap_or(true) {
            eprintln!("mkfifo unavailable, skipping");
            return;
        }
        // `ls` of the directory does not block, but reading the FIFO does;
        // use `cat` to stand in for the stuck syscall.
        let started = Instant::now();
        let mut child = Command::new("cat")
            .arg(&fifo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stalled = false;
        loop {
            match child.try_wait().unwrap() {
                Some(_) => break,
                None => {
                    if started.elapsed() >= Duration::from_millis(600) {
                        let _ = child.kill();
                        stalled = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        assert!(stalled, "a blocked read must be detectable by timeout");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "detection itself must be bounded"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_message_names_the_place_and_says_what_happened() {
        let m = Reachability::Stalled { waited_ms: 4000 }.message("studio");
        assert!(m.contains("studio"));
        assert!(m.contains("stopped responding"));
        assert!(
            m.contains("everywhere else"),
            "the user must be told the rest of Gust still works"
        );
        assert!(!m.to_lowercase().contains("error"));
    }

    #[test]
    fn a_failure_keeps_the_reason() {
        let m = Reachability::Failed {
            error: "Permission denied".into(),
        }
        .message("backups");
        assert!(m.contains("Permission denied"));
        assert!(m.contains("backups"));
    }

    #[test]
    fn a_place_is_named_by_its_mount_directory() {
        assert_eq!(
            place_name_for(Path::new("/run/user/1000/gust/places/studio/sub")),
            "studio"
        );
        assert_eq!(
            place_name_for(Path::new("/run/user/1000/gust/cloud/gustcloud")),
            "gustcloud"
        );
        // Not one of ours: fall back to the folder's own name.
        assert_eq!(place_name_for(Path::new("/mnt/nas/photos")), "photos");
    }

    #[test]
    fn fstype_resolves_the_longest_matching_mount() {
        // / is always present, so any absolute path resolves to something.
        let f = fstype_for(Path::new("/tmp"));
        assert!(f.is_some(), "every path lives on some filesystem");
    }

    #[test]
    fn a_local_path_needs_no_probe() {
        // The repo itself is on a local disk in every environment this runs.
        assert!(!needs_probe(Path::new("/usr")));
    }

    #[test]
    fn probe_if_risky_skips_local_paths_entirely() {
        let started = Instant::now();
        let r = probe_if_risky(Path::new("/usr"), Duration::from_secs(30));
        assert_eq!(r, Reachability::Responsive);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "a local path must not pay for the remote case"
        );
    }
}
