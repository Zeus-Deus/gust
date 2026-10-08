//! What a drop means, decided before anything touches the filesystem.
//!
//! A drop is an ordinary copy or move through the same engine, journal and
//! undo as any other operation (DESIGN.md §6.1). This module answers only
//! the question "what, if anything, should happen?" — so every refusal is a
//! pure function that can be tested without a window, a mouse, or real
//! files being at risk.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What the user is asking a drop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAction {
    /// The default for a cross-location drop. The safe operation wins.
    Copy,
    /// Only ever explicit — never inferred from where something came from.
    Move,
    /// "Link here": a symbolic link to each source, made in the target.
    /// Only ever explicit, like Move. Nothing is copied or moved.
    Link,
}

impl DropAction {
    pub fn verb(self) -> &'static str {
        match self {
            DropAction::Copy => "Copy",
            DropAction::Move => "Move",
            DropAction::Link => "Link",
        }
    }
}

/// The decision: either a described operation, or a refusal with a reason.
#[derive(Debug, Clone, PartialEq)]
pub enum DropPlan {
    /// Do this, and tell the user exactly this.
    Perform {
        action: DropAction,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        /// Shown before the operation runs: "Copy 3 items to backups".
        description: String,
    },
    /// Nothing to do. Not an error — a drop onto the current folder.
    Nothing,
    /// Refused, with a reason the user can act on.
    Refused(String),
}

/// Decide what a drop should do.
///
/// `destination` is whatever was under the pointer: a folder, a file, or the
/// empty space of the current view. A file resolves to its parent, because
/// an accidental drop must never be read as "replace this".
pub fn plan_drop(
    sources: &[PathBuf],
    destination: &Path,
    dest_is_dir: bool,
    action: DropAction,
) -> DropPlan {
    if sources.is_empty() {
        return DropPlan::Nothing;
    }

    // A drop onto a file means its containing folder.
    let target: PathBuf = if dest_is_dir {
        destination.to_path_buf()
    } else {
        match destination.parent() {
            Some(p) => p.to_path_buf(),
            None => return DropPlan::Refused("that is not a folder".into()),
        }
    };

    let mut accepted: Vec<PathBuf> = Vec::new();
    for src in sources {
        // Dropping something onto the folder it already lives in.
        if src.parent() == Some(target.as_path()) {
            continue;
        }
        // A folder cannot be dropped into itself.
        if src == &target {
            return DropPlan::Refused(format!(
                "\"{}\" cannot be dropped into itself",
                display_name(src)
            ));
        }
        // ...nor into its own descendant, which would recurse forever.
        if target.starts_with(src) {
            return DropPlan::Refused(format!(
                "\"{}\" cannot be dropped into a folder inside it",
                display_name(src)
            ));
        }
        accepted.push(src.clone());
    }

    if accepted.is_empty() {
        // Everything was already here: a no-op, not a self-copy that
        // produces "file (copy)" clutter.
        return DropPlan::Nothing;
    }

    let description = match action {
        DropAction::Link => format!(
            "Link {} in {}",
            count_phrase(accepted.len()),
            display_name(&target)
        ),
        _ => format!(
            "{} {} to {}",
            action.verb(),
            count_phrase(accepted.len()),
            display_name(&target)
        ),
    };

    DropPlan::Perform {
        action,
        sources: accepted,
        destination: target,
        description,
    }
}

/// The mount points listed in a `/proc/self/mounts` text, longest first.
///
/// Read once when a drag starts. `/proc` answers from the kernel's own
/// tables, so this never touches (or stalls on) the files being dragged.
pub fn mount_points(mounts: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = mounts
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|m| PathBuf::from(unescape_mount(m)))
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    out.dedup();
    out
}

/// `/proc/self/mounts` escapes space, tab, newline and backslash as octal.
fn unescape_mount(m: &str) -> String {
    m.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// The mount a path lives on: the longest mount point that contains it.
pub fn mount_of<'a>(mounts: &'a [PathBuf], p: &Path) -> Option<&'a Path> {
    mounts
        .iter()
        .find(|m| p.starts_with(m))
        .map(|m| m.as_path())
}

/// The "drive" a path is on, for deciding Move or Copy. Usually its mount;
/// but every GVfs share (SMB, NFS, phones) sits under the ONE FUSE mount
/// `…/gvfs`, so there the share's own folder is the drive: two shares on
/// two servers are never "the same drive".
fn drive_of(mounts: &[PathBuf], p: &Path) -> Option<PathBuf> {
    let m = mount_of(mounts, p)?;
    if m.file_name().is_some_and(|n| n == "gvfs") {
        if let Ok(rest) = p.strip_prefix(m) {
            if let Some(first) = rest.components().next() {
                return Some(m.join(first));
            }
        }
    }
    Some(m.to_path_buf())
}

/// What a plain drop (no modifier) does: **Move** when every source is on
/// the same mount as the target, **Copy** when anything crosses to another
/// one (a USB stick, a network share, another disk). This is what every
/// mainstream file manager does, and a move by drop is undoable (Ctrl+Z).
/// Unknown mounts count as different: the uncertain case copies.
pub fn default_action(mounts: &[PathBuf], sources: &[PathBuf], target: &Path) -> DropAction {
    let Some(t) = drive_of(mounts, target) else {
        return DropAction::Copy;
    };
    let same = !sources.is_empty()
        && sources
            .iter()
            .all(|s| drive_of(mounts, s).is_some_and(|m| m == t));
    if same {
        DropAction::Move
    } else {
        DropAction::Copy
    }
}

/// URIs from another application, reduced to paths Gust can actually use.
///
/// Only `file://` URIs are accepted. Text dropped from a browser or an
/// editor is not a file operation, and a URI naming a host we are not is
/// not our filesystem.
pub fn paths_from_uri_list(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("file://") else {
            continue;
        };
        // file://host/path — only an empty or localhost authority is ours.
        let path_part = match rest.find('/') {
            Some(0) => rest,
            Some(i) => {
                let host = &rest[..i];
                if host != "localhost" {
                    continue;
                }
                &rest[i..]
            }
            None => continue,
        };
        if let Some(p) = percent_decode(path_part) {
            if p.is_absolute() {
                out.push(p);
            }
        }
    }
    out
}

/// Decode %XX escapes into OS bytes.
///
/// Returns None on a malformed escape rather than guessing: a path is not
/// something to be creative about.
fn percent_decode(s: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() {
                return None;
            }
            let hi = (b[i + 1] as char).to_digit(16)?;
            let lo = (b[i + 2] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    // A NUL byte cannot appear in a path and signals something hostile.
    if out.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(out)))
}

fn count_phrase(n: usize) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn mounts() -> Vec<PathBuf> {
        mount_points(
            "/dev/nvme0n1p2 / btrfs rw 0 0\n\
             tmpfs /tmp tmpfs rw 0 0\n\
             /dev/sdb1 /run/media/me/USB\\040STICK vfat rw 0 0\n\
             /dev/nvme0n1p2 /home btrfs rw 0 0\n",
        )
    }

    #[test]
    fn a_plain_drop_on_the_same_disk_moves() {
        let m = mounts();
        assert_eq!(
            default_action(
                &m,
                &[pb("/home/me/a.txt"), pb("/home/me/b")],
                Path::new("/home/me/Documents")
            ),
            DropAction::Move
        );
    }

    #[test]
    fn a_plain_drop_onto_another_disk_copies() {
        let m = mounts();
        // The USB stick's mount point has an escaped space in /proc.
        assert_eq!(
            default_action(
                &m,
                &[pb("/home/me/a.txt")],
                Path::new("/run/media/me/USB STICK/backup")
            ),
            DropAction::Copy
        );
        assert_eq!(
            default_action(&m, &[pb("/tmp/x")], Path::new("/home/me")),
            DropAction::Copy
        );
    }

    #[test]
    fn one_source_from_elsewhere_makes_the_whole_drop_a_copy() {
        let m = mounts();
        assert_eq!(
            default_action(
                &m,
                &[pb("/home/me/a"), pb("/tmp/b")],
                Path::new("/home/me/c")
            ),
            DropAction::Copy
        );
    }

    #[test]
    fn two_network_shares_are_two_drives() {
        // RED before: both live under the one gvfs FUSE mount, so a drop
        // between two servers was planned as a Move.
        let m = mount_points(
            "/dev/nvme0n1p2 / btrfs rw 0 0\n\
             gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0\n",
        );
        let a = "/run/user/1000/gvfs/smb-share:server=a,share=x";
        let b = "/run/user/1000/gvfs/smb-share:server=b,share=y";
        assert_eq!(
            default_action(&m, &[pb(&format!("{a}/f.txt"))], Path::new(b)),
            DropAction::Copy
        );
        // Inside one share it is still one drive.
        assert_eq!(
            default_action(
                &m,
                &[pb(&format!("{a}/f.txt"))],
                Path::new(&format!("{a}/sub"))
            ),
            DropAction::Move
        );
    }

    #[test]
    fn an_unknown_mount_table_copies() {
        assert_eq!(
            default_action(&[], &[pb("/a")], Path::new("/b")),
            DropAction::Copy
        );
    }

    #[test]
    fn a_cross_folder_drop_defaults_to_copy_and_says_so() {
        let plan = plan_drop(
            &[pb("/a/one.txt"), pb("/a/two.txt")],
            Path::new("/b"),
            true,
            DropAction::Copy,
        );
        match plan {
            DropPlan::Perform {
                action,
                sources,
                destination,
                description,
            } => {
                assert_eq!(action, DropAction::Copy, "the safe operation is default");
                assert_eq!(sources.len(), 2);
                assert_eq!(destination, pb("/b"));
                assert_eq!(description, "Copy 2 items to b");
            }
            other => panic!("expected a copy, got {other:?}"),
        }
    }

    #[test]
    fn move_is_only_ever_explicit() {
        let plan = plan_drop(&[pb("/a/f")], Path::new("/b"), true, DropAction::Move);
        match plan {
            DropPlan::Perform {
                action,
                description,
                ..
            } => {
                assert_eq!(action, DropAction::Move);
                assert_eq!(description, "Move 1 item to b");
            }
            other => panic!("expected a move, got {other:?}"),
        }
    }

    #[test]
    fn the_operation_and_destination_are_named_before_it_runs() {
        // A drop that does not say where things went is a data loss report
        // waiting to happen.
        let plan = plan_drop(
            &[pb("/a/f")],
            Path::new("/x/backups"),
            true,
            DropAction::Copy,
        );
        let DropPlan::Perform { description, .. } = plan else {
            panic!("expected a plan");
        };
        assert!(description.contains("Copy"), "no operation named");
        assert!(description.contains("backups"), "no destination named");
    }

    #[test]
    fn dropping_onto_the_folder_things_already_live_in_does_nothing() {
        let plan = plan_drop(
            &[pb("/a/one.txt"), pb("/a/two.txt")],
            Path::new("/a"),
            true,
            DropAction::Copy,
        );
        assert_eq!(
            plan,
            DropPlan::Nothing,
            "a self-drop must not produce \"file (copy)\" clutter"
        );
    }

    #[test]
    fn a_mixed_drop_keeps_only_what_actually_moves() {
        let plan = plan_drop(
            &[pb("/a/stays.txt"), pb("/elsewhere/moves.txt")],
            Path::new("/a"),
            true,
            DropAction::Copy,
        );
        match plan {
            DropPlan::Perform {
                sources,
                description,
                ..
            } => {
                assert_eq!(sources, vec![pb("/elsewhere/moves.txt")]);
                assert_eq!(description, "Copy 1 item to a");
            }
            other => panic!("expected one accepted source, got {other:?}"),
        }
    }

    #[test]
    fn a_drop_onto_a_file_targets_its_folder() {
        // An accidental drop must never be read as "replace this file".
        let plan = plan_drop(
            &[pb("/src/note.txt")],
            Path::new("/dest/existing.txt"),
            false,
            DropAction::Copy,
        );
        match plan {
            DropPlan::Perform { destination, .. } => {
                assert_eq!(destination, pb("/dest"), "must target the parent folder");
            }
            other => panic!("expected the parent as destination, got {other:?}"),
        }
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_itself() {
        let plan = plan_drop(
            &[pb("/a/work")],
            Path::new("/a/work"),
            true,
            DropAction::Copy,
        );
        match plan {
            DropPlan::Refused(why) => assert!(why.contains("itself"), "unclear reason: {why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_its_own_descendant() {
        // This would recurse forever, copying into the thing being copied.
        let plan = plan_drop(
            &[pb("/a/work")],
            Path::new("/a/work/deep/inner"),
            true,
            DropAction::Copy,
        );
        match plan {
            DropPlan::Refused(why) => {
                assert!(why.contains("inside it"), "unclear reason: {why}")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_mistaken_for_a_descendant() {
        // /a/work2 is NOT inside /a/work, even though the string starts the
        // same way. A naive prefix check on strings would refuse this.
        let plan = plan_drop(
            &[pb("/a/work")],
            Path::new("/a/work2"),
            true,
            DropAction::Copy,
        );
        match plan {
            DropPlan::Perform { destination, .. } => assert_eq!(destination, pb("/a/work2")),
            other => panic!("a sibling was wrongly refused: {other:?}"),
        }
    }

    #[test]
    fn an_empty_drop_does_nothing() {
        assert_eq!(
            plan_drop(&[], Path::new("/b"), true, DropAction::Copy),
            DropPlan::Nothing
        );
    }

    #[test]
    fn only_resolvable_file_uris_become_paths() {
        let text = "file:///home/u/a.txt\n\
                    file://localhost/home/u/b.txt\n\
                    file://otherhost/home/u/c.txt\n\
                    https://example.com/d.txt\n\
                    just some dragged text\n\
                    #comment\n";
        let got = paths_from_uri_list(text);
        assert_eq!(
            got,
            vec![pb("/home/u/a.txt"), pb("/home/u/b.txt")],
            "only local file URIs may become file operations"
        );
    }

    #[test]
    fn percent_escapes_decode_to_real_names() {
        let got = paths_from_uri_list("file:///tmp/my%20file%20(1).txt\n");
        assert_eq!(got, vec![pb("/tmp/my file (1).txt")]);
    }

    #[test]
    fn a_malformed_or_hostile_uri_is_dropped_not_guessed_at() {
        for bad in [
            "file:///tmp/bad%zz\n",     // not hex
            "file:///tmp/trunc%4\n",    // truncated escape
            "file:///tmp/nul%00name\n", // embedded NUL
            "file://relative/path\n",   // non-local host
            "file:notabsolute\n",
        ] {
            assert!(
                paths_from_uri_list(bad).is_empty(),
                "accepted a bad URI: {bad:?}"
            );
        }
    }

    #[test]
    fn a_non_utf8_uri_survives_decoding() {
        // %ff is not valid UTF-8 — the path must still come through intact.
        let got = paths_from_uri_list("file:///tmp/pi%ffcs\n");
        assert_eq!(got.len(), 1);
        use std::os::unix::ffi::OsStrExt;
        assert!(
            got[0].as_os_str().as_bytes().contains(&0xff),
            "a non-UTF-8 path was mangled by the URI decoder"
        );
    }

    #[test]
    fn link_here_is_planned_and_described_like_the_others() {
        let srcs = vec![pb("/home/u/a.txt"), pb("/home/u/b.txt")];
        match plan_drop(&srcs, Path::new("/home/u/shelf"), true, DropAction::Link) {
            DropPlan::Perform {
                action,
                description,
                destination,
                ..
            } => {
                assert_eq!(action, DropAction::Link);
                assert_eq!(destination, pb("/home/u/shelf"));
                assert_eq!(description, "Link 2 items in shelf");
            }
            other => panic!("{other:?}"),
        }
        // The same refusals hold: a folder never links into itself.
        assert!(matches!(
            plan_drop(&[pb("/a/f")], Path::new("/a/f/sub"), true, DropAction::Link),
            DropPlan::Refused(_)
        ));
        // A link into the folder it already lives in is a no-op.
        assert_eq!(
            plan_drop(&[pb("/a/x")], Path::new("/a"), true, DropAction::Link),
            DropPlan::Nothing
        );
    }
}
