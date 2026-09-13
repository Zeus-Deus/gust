//! Archive extraction policy.
//!
//! The plan's fixture result (§10): the default extractor created a FIFO and
//! left partial output on failure. So Gust does not wrap a CLI and hope —
//! it decides per entry, stages into an owned directory, and validates the
//! result before committing it.

use std::path::{Component, Path, PathBuf};

#[path = "archive_io.rs"]
pub mod io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveEntryKind {
    Regular,
    Directory,
    Symlink,
    Hardlink,
    /// FIFO, device, socket — always rejected.
    Special,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveEntry {
    pub name: String,
    pub kind: ArchiveEntryKind,
    /// For links: the recorded target.
    pub link_target: Option<String>,
    pub size: u64,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Rejection {
    /// `../` escape, absolute path, or a traversal after normalisation.
    PathTraversal(String),
    /// A symlink pointing outside the extraction root.
    EscapingLink { name: String, target: String },
    /// FIFO/device/socket.
    SpecialFile(String),
    /// Would exceed the expansion budget (zip bomb).
    ExpansionLimit { name: String, total: u64 },
    /// setuid/setgid bits present.
    PrivilegedMode(String),
    /// Too many entries.
    EntryLimit,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::PathTraversal(n) => write!(f, "{n}: path escapes the destination"),
            Rejection::EscapingLink { name, target } => {
                write!(f, "{name}: link target {target} escapes the destination")
            }
            Rejection::SpecialFile(n) => write!(f, "{n}: special files are not extracted"),
            Rejection::ExpansionLimit { name, total } => {
                write!(f, "{name}: expansion limit exceeded at {total} bytes")
            }
            Rejection::PrivilegedMode(n) => write!(f, "{n}: setuid/setgid is not extracted"),
            Rejection::EntryLimit => write!(f, "archive has too many entries"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractPolicy {
    pub max_total_bytes: u64,
    pub max_entries: usize,
    pub allow_symlinks: bool,
}

impl Default for ExtractPolicy {
    fn default() -> Self {
        ExtractPolicy {
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_entries: 200_000,
            allow_symlinks: true,
        }
    }
}

/// Normalise an archive member path and refuse anything that escapes.
///
/// Returns the safe relative path, or `None` if it must be rejected.
pub fn safe_member_path(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            // Any `..` is refused outright — not "resolved", refused.
            Component::ParentDir => return None,
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    Some(out)
}

/// Would this link target escape the extraction root?
pub fn link_escapes(member_dir: &Path, target: &str) -> bool {
    let t = Path::new(target);
    if t.is_absolute() {
        return true;
    }
    // Walk the target relative to its containing directory and see if depth
    // ever goes negative.
    let mut depth: i64 = member_dir.components().count() as i64;
    for c in t.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return true,
        }
    }
    false
}

/// Vet every entry BEFORE extracting anything.
///
/// Returns the accepted entries with their safe paths, plus every rejection.
/// A caller that gets any rejection must show it — never silently skip.
pub fn vet_entries(
    entries: &[ArchiveEntry],
    policy: &ExtractPolicy,
) -> (Vec<(ArchiveEntry, PathBuf)>, Vec<Rejection>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    let mut total: u64 = 0;

    if entries.len() > policy.max_entries {
        bad.push(Rejection::EntryLimit);
        return (ok, bad);
    }

    for e in entries {
        if e.kind == ArchiveEntryKind::Special {
            bad.push(Rejection::SpecialFile(e.name.clone()));
            continue;
        }
        let Some(rel) = safe_member_path(&e.name) else {
            bad.push(Rejection::PathTraversal(e.name.clone()));
            continue;
        };
        if e.mode & 0o6000 != 0 {
            bad.push(Rejection::PrivilegedMode(e.name.clone()));
            continue;
        }
        if matches!(
            e.kind,
            ArchiveEntryKind::Symlink | ArchiveEntryKind::Hardlink
        ) {
            if !policy.allow_symlinks {
                bad.push(Rejection::SpecialFile(e.name.clone()));
                continue;
            }
            let target = e.link_target.clone().unwrap_or_default();
            let dir = rel.parent().unwrap_or(Path::new(""));
            if link_escapes(dir, &target) {
                bad.push(Rejection::EscapingLink {
                    name: e.name.clone(),
                    target,
                });
                continue;
            }
        }
        total = total.saturating_add(e.size);
        if total > policy.max_total_bytes {
            bad.push(Rejection::ExpansionLimit {
                name: e.name.clone(),
                total,
            });
            continue;
        }
        ok.push((e.clone(), rel));
    }
    (ok, bad)
}

/// Staging directory for an extraction, inside the destination so the commit
/// is a same-filesystem rename.
pub fn staging_dir(dest: &Path) -> PathBuf {
    dest.join(format!(".gust-extract-{}", std::process::id()))
}

/// A failed extraction must not leave partial output looking like success.
pub fn cleanup_staging(staging: &Path) -> std::io::Result<()> {
    if staging.exists() {
        std::fs::remove_dir_all(staging)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: ArchiveEntryKind) -> ArchiveEntry {
        ArchiveEntry {
            name: name.into(),
            kind,
            link_target: None,
            size: 10,
            mode: 0o644,
        }
    }

    #[test]
    fn rejects_dotdot_traversal() {
        assert!(safe_member_path("../../etc/passwd").is_none());
        assert!(safe_member_path("a/../../b").is_none());
        assert!(safe_member_path("/etc/passwd").is_none());
    }

    #[test]
    fn accepts_ordinary_nested_paths() {
        assert_eq!(
            safe_member_path("dir/sub/file.txt").unwrap(),
            PathBuf::from("dir/sub/file.txt")
        );
        assert_eq!(
            safe_member_path("./dir/file").unwrap(),
            PathBuf::from("dir/file")
        );
    }

    #[test]
    fn rejects_fifo_entries() {
        let (ok, bad) = vet_entries(
            &[entry("pipe", ArchiveEntryKind::Special)],
            &ExtractPolicy::default(),
        );
        assert!(ok.is_empty());
        assert_eq!(bad, vec![Rejection::SpecialFile("pipe".into())]);
    }

    #[test]
    fn rejects_symlink_escaping_the_destination() {
        let mut e = entry("sub/evil", ArchiveEntryKind::Symlink);
        e.link_target = Some("../../../../etc/passwd".into());
        let (ok, bad) = vet_entries(&[e], &ExtractPolicy::default());
        assert!(ok.is_empty());
        assert!(matches!(bad[0], Rejection::EscapingLink { .. }));
    }

    #[test]
    fn allows_symlink_staying_inside() {
        let mut e = entry("sub/ok", ArchiveEntryKind::Symlink);
        e.link_target = Some("../other/file".into());
        let (ok, bad) = vet_entries(&[e], &ExtractPolicy::default());
        assert_eq!(ok.len(), 1, "rejections: {bad:?}");
    }

    #[test]
    fn rejects_absolute_link_target() {
        let mut e = entry("l", ArchiveEntryKind::Symlink);
        e.link_target = Some("/etc/shadow".into());
        let (_, bad) = vet_entries(&[e], &ExtractPolicy::default());
        assert!(matches!(bad[0], Rejection::EscapingLink { .. }));
    }

    #[test]
    fn rejects_setuid_entries() {
        let mut e = entry("x", ArchiveEntryKind::Regular);
        e.mode = 0o4755;
        let (ok, bad) = vet_entries(&[e], &ExtractPolicy::default());
        assert!(ok.is_empty());
        assert_eq!(bad, vec![Rejection::PrivilegedMode("x".into())]);
    }

    #[test]
    fn enforces_expansion_budget() {
        let mut e = entry("bomb", ArchiveEntryKind::Regular);
        e.size = 10_000;
        let policy = ExtractPolicy {
            max_total_bytes: 5_000,
            ..Default::default()
        };
        let (ok, bad) = vet_entries(&[e], &policy);
        assert!(ok.is_empty());
        assert!(matches!(bad[0], Rejection::ExpansionLimit { .. }));
    }

    #[test]
    fn enforces_entry_count_limit() {
        let entries: Vec<_> = (0..5)
            .map(|i| entry(&format!("f{i}"), ArchiveEntryKind::Regular))
            .collect();
        let policy = ExtractPolicy {
            max_entries: 3,
            ..Default::default()
        };
        let (ok, bad) = vet_entries(&entries, &policy);
        assert!(ok.is_empty());
        assert_eq!(bad, vec![Rejection::EntryLimit]);
    }

    #[test]
    fn mixed_archive_reports_every_rejection_and_keeps_good_entries() {
        let mut evil = entry("sub/evil", ArchiveEntryKind::Symlink);
        evil.link_target = Some("../../../../etc/passwd".into());
        let entries = vec![
            entry("good.txt", ArchiveEntryKind::Regular),
            entry("../escape.txt", ArchiveEntryKind::Regular),
            entry("pipe", ArchiveEntryKind::Special),
            evil,
        ];
        let (ok, bad) = vet_entries(&entries, &ExtractPolicy::default());
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].1, PathBuf::from("good.txt"));
        assert_eq!(bad.len(), 3, "every rejection is reported: {bad:?}");
    }

    #[test]
    fn staging_is_removed_on_cleanup() {
        let d = std::env::temp_dir().join(format!("gust-ex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let s = staging_dir(&d);
        std::fs::create_dir_all(s.join("partial")).unwrap();
        std::fs::write(s.join("partial/half.txt"), b"incomplete").unwrap();
        cleanup_staging(&s).unwrap();
        assert!(!s.exists(), "partial output must not survive a failure");
        std::fs::remove_dir_all(&d).ok();
    }
}
