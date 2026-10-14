//! Bulk rename (DESIGN §6.16, mockup C).
//!
//! The most dangerous ordinary operation in a file manager: one mistake
//! renames forty files at once and there is nothing left to compare
//! against. So this module decides everything first and shows it; the
//! caller only executes a plan the user has already seen.
//!
//! There is no regular expression and no shell here. The pattern language
//! is three tokens, which is small enough that nobody can be surprised by
//! it.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why one entry of a bulk rename cannot be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameProblem {
    /// The new name already exists on disk and is not part of this batch.
    Exists(String),
    /// Two entries in this batch want the same new name.
    Duplicate(String),
    /// The filesystem would reject it: empty, contains `/` or NUL, `.`, `..`.
    Invalid { name: String, why: String },
}

impl RenameProblem {
    pub fn message(&self) -> String {
        match self {
            RenameProblem::Exists(n) => format!("{n} already exists"),
            RenameProblem::Duplicate(n) => {
                format!("{n} would be used twice in this rename")
            }
            RenameProblem::Invalid { name, why } => format!("{name}: {why}"),
        }
    }
}

/// One planned rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRename {
    pub from: PathBuf,
    pub to: PathBuf,
    pub old_name: String,
    pub new_name: String,
    /// True when the name would not change. Not an error; just nothing to do.
    pub unchanged: bool,
}

/// Everything the user must see before anything happens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenamePlan {
    pub renames: Vec<PlannedRename>,
    pub problems: Vec<RenameProblem>,
    /// For each file, in order, what is wrong with its new name (if
    /// anything): the rename card marks that line as you type.
    pub line_problems: Vec<Option<String>>,
}

impl RenamePlan {
    /// May this plan be executed?
    ///
    /// All or nothing: a partial bulk rename leaves the user unable to tell
    /// which half happened, and running it again does something different.
    pub fn is_safe(&self) -> bool {
        self.problems.is_empty() && self.renames.iter().any(|r| !r.unchanged)
    }

    /// How many files would actually change.
    pub fn changing(&self) -> usize {
        self.renames.iter().filter(|r| !r.unchanged).count()
    }

    /// A sentence for the dialog.
    pub fn summary(&self) -> String {
        if !self.problems.is_empty() {
            // Count the lines that are wrong, not the kinds of problem: two
            // lines wanting one name are two names that would not work.
            let marked = self.line_problems.iter().filter(|l| l.is_some()).count();
            let n = marked.max(self.problems.len());
            return format!(
                "{} name{} would not work — nothing will be renamed",
                n,
                if n == 1 { "" } else { "s" }
            );
        }
        match self.changing() {
            0 => "No names would change".into(),
            1 => "1 file will be renamed".into(),
            n => format!("{n} files will be renamed"),
        }
    }
}

/// Split a file name into stem and extension, the way a person reads it.
///
/// A leading dot is part of the name, not an extension: `.bashrc` has no
/// extension, and renaming it must not produce `.{ext}` nonsense.
pub fn split_name(name: &str) -> (String, String) {
    let trimmed = name
        .strip_prefix('.')
        .map(|r| (true, r))
        .unwrap_or((false, name));
    let (leading_dot, rest) = trimmed;
    match rest.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            let stem = if leading_dot {
                format!(".{stem}")
            } else {
                stem.to_string()
            };
            (stem, ext.to_string())
        }
        _ => (name.to_string(), String::new()),
    }
}

/// Expand one pattern for one file.
///
/// `{n}` is the counter, `{name}` the stem, `{ext}` the extension. Anything
/// else is a literal, including braces that do not form a token — a file
/// genuinely called `{n}` is possible.
pub fn expand(pattern: &str, original: &str, index: usize, width: usize) -> String {
    let (stem, ext) = split_name(original);
    let mut out = String::with_capacity(pattern.len() + 8);
    let bytes: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '{' {
            let rest: String = bytes[i..].iter().collect();
            if let Some(end) = rest.find('}') {
                let token = &rest[1..end];
                match token {
                    "n" => {
                        out.push_str(&format!("{index:0width$}"));
                        i += end + 1;
                        continue;
                    }
                    "name" => {
                        out.push_str(&stem);
                        i += end + 1;
                        continue;
                    }
                    "ext" => {
                        out.push_str(&ext);
                        i += end + 1;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Every file's new name under `pattern`, in the order given.
pub fn expand_all(files: &[PathBuf], pattern: &str) -> Vec<String> {
    // Zero-pad to the width of the largest number, so 1..10 sorts correctly
    // in every other tool the user will look at these files with.
    let width = files.len().to_string().len();
    files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let old = f
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            expand(pattern, &old, i + 1, width)
        })
        .collect()
}

/// Replace every occurrence of `find` in `name` with `with`. Literal text,
/// never a pattern: what you type is what is replaced.
pub fn replace_all(name: &str, find: &str, with: &str) -> String {
    if find.is_empty() {
        return name.to_string();
    }
    name.replace(find, with)
}

/// Build the plan. Touches nothing.
///
/// `files` is in the order the user is looking at them — the current sort —
/// so numbering matches the screen rather than the order the filesystem
/// happened to return.
pub fn plan(dir: &Path, files: &[PathBuf], pattern: &str) -> RenamePlan {
    if files.is_empty() || pattern.trim().is_empty() {
        return RenamePlan::default();
    }
    plan_names(dir, files, &expand_all(files, pattern))
}

/// Build the plan for an explicit new name per file (the rename card's
/// editable list). `names[i]` is the new name of `files[i]`. Touches nothing.
pub fn plan_names(dir: &Path, files: &[PathBuf], names: &[String]) -> RenamePlan {
    let mut out = RenamePlan::default();
    if files.is_empty() {
        return out;
    }
    out.line_problems = vec![None; files.len()];
    if names.len() != files.len() {
        // One name per line, one line per file: anything else means a line
        // was added or deleted and the pairing is no longer what was shown.
        out.problems.push(RenameProblem::Invalid {
            name: format!("{} names for {} files", names.len(), files.len()),
            why: "keep exactly one line per file".into(),
        });
        return out;
    }

    // Names being renamed away are available for reuse within the batch.
    let leaving: HashSet<String> = files
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    // Every name some line wants, to report BOTH lines of a duplicate.
    let mut wanted: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, n) in names.iter().enumerate() {
        let old = files[i]
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        if old.as_deref() != Some(n.as_str()) {
            *wanted.entry(n.clone()).or_default() += 1;
        }
    }
    let mut taken: HashSet<String> = HashSet::new();

    for (i, from) in files.iter().enumerate() {
        let Some(old_name) = from.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let new_name = names[i].clone();
        let flag = |out: &mut RenamePlan, p: RenameProblem| {
            out.line_problems[i] = Some(p.message());
            out.problems.push(p);
        };

        // The filesystem's own rules first.
        let as_os = OsString::from(&new_name);
        if let Err(e) = crate::recursive::validate_name(&as_os) {
            flag(
                &mut out,
                RenameProblem::Invalid {
                    name: if new_name.is_empty() {
                        "(empty)".into()
                    } else {
                        new_name.clone()
                    },
                    why: format!("{e}"),
                },
            );
            continue;
        }

        if new_name == old_name {
            out.renames.push(PlannedRename {
                from: from.clone(),
                to: from.clone(),
                old_name,
                new_name,
                unchanged: true,
            });
            continue;
        }

        // Two entries wanting the same name is a mistake the user cannot
        // see from the pattern alone. Both lines are marked.
        if wanted.get(&new_name).copied().unwrap_or(0) > 1 {
            if taken.insert(new_name.clone()) {
                out.problems
                    .push(RenameProblem::Duplicate(new_name.clone()));
            }
            out.line_problems[i] = Some(RenameProblem::Duplicate(new_name).message());
            continue;
        }
        // A name another line is renaming away from, but that the other
        // line keeps (unchanged): still taken.
        let kept_by_other = names
            .iter()
            .enumerate()
            .any(|(j, n)| j != i && *n == new_name && leaving.contains(n));

        // An existing file that is NOT part of this batch would be lost.
        let to = dir.join(&new_name);
        if kept_by_other || (to.symlink_metadata().is_ok() && !leaving.contains(&new_name)) {
            flag(&mut out, RenameProblem::Exists(new_name));
            continue;
        }

        out.renames.push(PlannedRename {
            from: from.clone(),
            to,
            old_name,
            new_name,
            unchanged: false,
        });
    }
    out
}

/// What a completed bulk rename did, for the journal and for undo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenameOutcome {
    /// (from, to) for every rename that happened, in order.
    pub done: Vec<(PathBuf, PathBuf)>,
    pub failed: Vec<String>,
}

impl RenameOutcome {
    pub fn summary(&self) -> String {
        let n = self.done.len();
        let mut s = match n {
            0 => "Nothing was renamed".to_string(),
            1 => "Renamed 1 file".to_string(),
            _ => format!("Renamed {n} files"),
        };
        if !self.failed.is_empty() {
            s.push_str(&format!(" · {} failed", self.failed.len()));
        }
        s
    }
}

/// A temporary name for the first phase. Unique per process and index, and
/// obviously not a real file name, so debris is recognisable.
fn temp_name(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!(".gust-rename-{}-{index}", std::process::id()))
}

/// Execute a plan that has already been shown to the user.
///
/// Two phases, because `a→b` and `b→a` in one batch is a legitimate swap and
/// renaming in sequence destroys one of them. Everything moves to a unique
/// temporary name first, then to its final name.
///
/// Every rename refuses to replace an existing name (`RENAME_NOREPLACE`).
/// The plan was checked when it was shown, but time passes while a preview
/// is on screen: a file that appears under a target name in the meantime is
/// someone's work and is never overwritten. If any step is refused, the
/// whole batch is put back, so the folder is never left half renamed.
pub fn execute(dir: &Path, plan: &RenamePlan) -> RenameOutcome {
    use crate::anchor::rename_noreplace_paths as mv;
    let mut out = RenameOutcome::default();
    if !plan.is_safe() {
        return out;
    }
    let work: Vec<&PlannedRename> = plan.renames.iter().filter(|r| !r.unchanged).collect();

    // Put everything back to its original name: finished renames first go
    // back to their temporary names, then every temporary name to its origin.
    let roll_back =
        |staged: &[(PathBuf, &PlannedRename)], finished: usize, out: &mut RenameOutcome| {
            for (tmp, r) in staged[..finished].iter().rev() {
                if let Err(e) = mv(&r.to, tmp) {
                    out.failed
                        .push(format!("{}: could not be put back ({e})", r.new_name));
                }
            }
            for (tmp, r) in staged.iter().rev() {
                if tmp.symlink_metadata().is_ok() {
                    if let Err(e) = mv(tmp, &r.from) {
                        out.failed.push(format!(
                            "{}: left as {} ({e})",
                            r.old_name,
                            tmp.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
            }
        };

    // Phase one: everything to a temporary name.
    let mut staged: Vec<(PathBuf, &PlannedRename)> = Vec::new();
    for (i, r) in work.iter().enumerate() {
        let tmp = temp_name(dir, i);
        match mv(&r.from, &tmp) {
            Ok(()) => staged.push((tmp, r)),
            Err(e) => {
                out.failed.push(format!("{}: {e}", r.old_name));
                // A half-done first phase must not leave temporary names.
                roll_back(&staged, 0, &mut out);
                return out;
            }
        }
    }

    // Phase two: temporary names to their finals.
    for (n, (tmp, r)) in staged.iter().enumerate() {
        if let Err(e) = mv(tmp, &r.to) {
            let why = if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!(
                    "{}: {} appeared meanwhile — nothing was renamed",
                    r.old_name, r.new_name
                )
            } else {
                format!("{}: {e} — nothing was renamed", r.old_name)
            };
            out.failed.push(why);
            roll_back(&staged, n, &mut out);
            out.done.clear();
            return out;
        }
        out.done.push((r.from.clone(), r.to.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-bulk-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| {
                let p = dir.join(n);
                std::fs::write(&p, "x").unwrap();
                p
            })
            .collect()
    }

    #[test]
    fn names_split_the_way_a_person_reads_them() {
        assert_eq!(split_name("photo.jpg"), ("photo".into(), "jpg".into()));
        assert_eq!(
            split_name("archive.tar.gz"),
            ("archive.tar".into(), "gz".into())
        );
        assert_eq!(split_name("README"), ("README".into(), "".into()));
        // A leading dot is part of the name, not an extension.
        assert_eq!(split_name(".bashrc"), (".bashrc".into(), "".into()));
        assert_eq!(
            split_name(".config.toml"),
            (".config".into(), "toml".into())
        );
    }

    #[test]
    fn the_three_tokens_expand() {
        assert_eq!(expand("{name}.{ext}", "a.txt", 1, 1), "a.txt");
        assert_eq!(expand("holiday-{n}.{ext}", "a.jpg", 3, 1), "holiday-3.jpg");
        assert_eq!(expand("{n}-{name}", "photo.png", 2, 1), "2-photo");
    }

    #[test]
    fn the_counter_is_padded_to_the_widest_number() {
        // 1..10 must sort correctly everywhere else the user looks at them.
        assert_eq!(expand("{n}", "x", 1, 2), "01");
        assert_eq!(expand("{n}", "x", 10, 2), "10");
    }

    #[test]
    fn anything_that_is_not_a_token_is_a_literal() {
        assert_eq!(expand("{nope}", "a.txt", 1, 1), "{nope}");
        assert_eq!(expand("a{b", "x.txt", 1, 1), "a{b");
        assert_eq!(expand("100% {name}", "a.txt", 1, 1), "100% a");
    }

    #[test]
    fn a_plan_touches_nothing() {
        let d = tmp("readonly");
        let files = make(&d, &["a.txt", "b.txt"]);
        let before: usize = std::fs::read_dir(&d).unwrap().flatten().count();
        let _ = plan(&d, &files, "x-{n}.{ext}");
        let after: usize = std::fs::read_dir(&d).unwrap().flatten().count();
        assert_eq!(before, after, "planning must not create or remove anything");
        assert!(d.join("a.txt").exists(), "the originals must be untouched");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_collision_with_an_existing_file_is_refused() {
        let d = tmp("collide");
        let files = make(&d, &["a.txt", "b.txt"]);
        std::fs::write(d.join("x-1.txt"), "in the way").unwrap();

        let p = plan(&d, &files, "x-{n}.{ext}");
        assert!(!p.is_safe(), "a collision must make the whole plan unsafe");
        assert!(p
            .problems
            .iter()
            .any(|pr| matches!(pr, RenameProblem::Exists(n) if n == "x-1.txt")));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn two_files_wanting_the_same_name_is_refused() {
        let d = tmp("dup");
        let files = make(&d, &["a.txt", "b.txt"]);
        // No counter: both want "same.txt".
        let p = plan(&d, &files, "same.{ext}");
        assert!(!p.is_safe());
        assert!(p
            .problems
            .iter()
            .any(|pr| matches!(pr, RenameProblem::Duplicate(_))));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_invalid_name_is_refused_before_anything_happens() {
        let d = tmp("invalid");
        let files = make(&d, &["a.txt"]);
        for bad in ["", "a/b", "."] {
            let p = plan(&d, &files, bad);
            assert!(!p.is_safe(), "{bad:?} should not be plannable");
        }
        assert!(d.join("a.txt").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_name_that_does_not_change_is_not_an_error() {
        let d = tmp("same");
        let files = make(&d, &["a.txt"]);
        let p = plan(&d, &files, "{name}.{ext}");
        assert!(p.problems.is_empty());
        assert_eq!(p.changing(), 0);
        assert!(!p.is_safe(), "there is nothing to do, so nothing runs");
        assert_eq!(p.summary(), "No names would change");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_ordinary_rename_runs_and_reports() {
        let d = tmp("run");
        let files = make(&d, &["one.txt", "two.txt", "three.txt"]);
        let p = plan(&d, &files, "file-{n}.{ext}");
        assert!(p.is_safe(), "{:?}", p.problems);
        assert_eq!(p.summary(), "3 files will be renamed");

        let done = execute(&d, &p);
        assert_eq!(done.done.len(), 3);
        assert!(done.failed.is_empty());
        for n in ["file-1.txt", "file-2.txt", "file-3.txt"] {
            assert!(d.join(n).is_file(), "{n} should exist");
        }
        assert!(!d.join("one.txt").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    /// The case that a naive sequential rename destroys.
    #[test]
    fn a_swap_does_not_lose_a_file() {
        let d = tmp("swap");
        std::fs::write(d.join("a.txt"), "content A").unwrap();
        std::fs::write(d.join("b.txt"), "content B").unwrap();
        let files = vec![d.join("a.txt"), d.join("b.txt")];

        // Reverse the numbering so a→b-2 and b→b-1... construct a true swap
        // by planning it directly.
        let p = RenamePlan {
            renames: vec![
                PlannedRename {
                    from: d.join("a.txt"),
                    to: d.join("b.txt"),
                    old_name: "a.txt".into(),
                    new_name: "b.txt".into(),
                    unchanged: false,
                },
                PlannedRename {
                    from: d.join("b.txt"),
                    to: d.join("a.txt"),
                    old_name: "b.txt".into(),
                    new_name: "a.txt".into(),
                    unchanged: false,
                },
            ],
            problems: Vec::new(),
            ..Default::default()
        };
        assert!(p.is_safe());

        let done = execute(&d, &p);
        assert_eq!(done.done.len(), 2, "both renames must happen");
        assert_eq!(
            std::fs::read_to_string(d.join("a.txt")).unwrap(),
            "content B",
            "the swap must have exchanged the two files"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("b.txt")).unwrap(),
            "content A"
        );
        let _ = files;
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_unsafe_plan_renames_nothing() {
        let d = tmp("refuse");
        let files = make(&d, &["a.txt", "b.txt"]);
        let p = plan(&d, &files, "same.{ext}"); // duplicate
        assert!(!p.is_safe());

        let done = execute(&d, &p);
        assert!(
            done.done.is_empty(),
            "an unsafe plan must do nothing at all"
        );
        assert!(d.join("a.txt").exists());
        assert!(d.join("b.txt").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn no_temporary_names_survive_a_successful_run() {
        let d = tmp("clean");
        let files = make(&d, &["a.txt", "b.txt"]);
        let p = plan(&d, &files, "z-{n}.{ext}");
        let _ = execute(&d, &p);
        let debris: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".gust-rename"))
            .collect();
        assert!(debris.is_empty(), "temporary names survived: {debris:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn numbering_follows_the_order_it_was_given() {
        let d = tmp("order");
        // Deliberately not alphabetical: the caller passes the user's sort.
        let files = vec![
            {
                let p = d.join("zebra.txt");
                std::fs::write(&p, "x").unwrap();
                p
            },
            {
                let p = d.join("apple.txt");
                std::fs::write(&p, "x").unwrap();
                p
            },
        ];
        let p = plan(&d, &files, "{n}-{name}.{ext}");
        assert_eq!(p.renames[0].new_name, "1-zebra.txt");
        assert_eq!(p.renames[1].new_name, "2-apple.txt");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reusing_a_name_from_within_the_batch_is_allowed() {
        // b.txt is being renamed away, so a.txt may take its name.
        let d = tmp("reuse");
        let files = make(&d, &["a.txt", "b.txt"]);
        let p = RenamePlan {
            renames: vec![PlannedRename {
                from: d.join("a.txt"),
                to: d.join("b.txt"),
                old_name: "a.txt".into(),
                new_name: "b.txt".into(),
                unchanged: false,
            }],
            problems: Vec::new(),
            ..Default::default()
        };
        assert!(p.is_safe());
        let _ = files;
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn summaries_say_what_will_happen() {
        let one = RenamePlan {
            renames: vec![PlannedRename {
                from: "/a".into(),
                to: "/b".into(),
                old_name: "a".into(),
                new_name: "b".into(),
                unchanged: false,
            }],
            problems: Vec::new(),
            ..Default::default()
        };
        assert_eq!(one.summary(), "1 file will be renamed");

        let bad = RenamePlan {
            renames: Vec::new(),
            problems: vec![RenameProblem::Exists("x".into())],
            ..Default::default()
        };
        assert!(bad.summary().contains("nothing will be renamed"));
    }

    #[test]
    fn an_edited_list_renames_each_file_to_its_own_line() {
        let d = tmp("names");
        let files = make(&d, &["IMG_1.jpg", "IMG_2.jpg", "notes.txt"]);
        let names: Vec<String> = ["beach.jpg", "IMG_2.jpg", "notes.md"]
            .map(String::from)
            .to_vec();
        let p = plan_names(&d, &files, &names);
        assert!(p.is_safe(), "{:?}", p.problems);
        assert_eq!(p.changing(), 2);
        assert_eq!(p.line_problems, [None, None, None]);
        let done = execute(&d, &p);
        assert!(done.failed.is_empty(), "{:?}", done.failed);
        assert!(d.join("beach.jpg").is_file() && d.join("notes.md").is_file());
        assert!(d.join("IMG_2.jpg").is_file());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn each_bad_line_is_marked_and_nothing_runs() {
        let d = tmp("names-bad");
        let files = make(&d, &["a.txt", "b.txt", "c.txt", "d.txt"]);
        std::fs::write(d.join("taken.txt"), "someone else's").unwrap();
        let names: Vec<String> = ["same.txt", "same.txt", "taken.txt", "x/y"]
            .map(String::from)
            .to_vec();
        let p = plan_names(&d, &files, &names);
        assert!(!p.is_safe());
        let marked: Vec<bool> = p.line_problems.iter().map(|l| l.is_some()).collect();
        assert_eq!(marked, [true, true, true, true], "{:?}", p.line_problems);
        assert!(p.line_problems[0].as_deref().unwrap().contains("twice"));
        assert!(p.line_problems[2]
            .as_deref()
            .unwrap()
            .contains("already exists"));
        assert!(
            p.summary().starts_with("4 names would not work"),
            "{}",
            p.summary()
        );
        assert!(execute(&d, &p).done.is_empty());
        assert!(d.join("a.txt").exists() && d.join("d.txt").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_name_kept_by_another_line_is_still_taken() {
        // b.txt stays b.txt, so a.txt cannot become b.txt.
        let d = tmp("names-kept");
        let files = make(&d, &["a.txt", "b.txt"]);
        let names: Vec<String> = ["b.txt", "b.txt"].map(String::from).to_vec();
        let p = plan_names(&d, &files, &names);
        assert!(!p.is_safe());
        assert!(p.line_problems[0].is_some());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_swap_through_the_list_works() {
        let d = tmp("names-swap");
        std::fs::write(d.join("a.txt"), "A").unwrap();
        std::fs::write(d.join("b.txt"), "B").unwrap();
        let files = vec![d.join("a.txt"), d.join("b.txt")];
        let names: Vec<String> = ["b.txt", "a.txt"].map(String::from).to_vec();
        let p = plan_names(&d, &files, &names);
        assert!(p.is_safe(), "{:?}", p.problems);
        let done = execute(&d, &p);
        assert!(done.failed.is_empty());
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(d.join("b.txt")).unwrap(), "A");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_missing_or_extra_line_is_refused() {
        let d = tmp("names-count");
        let files = make(&d, &["a.txt", "b.txt"]);
        let p = plan_names(&d, &files, &["x.txt".to_string()]);
        assert!(!p.is_safe());
        assert!(p.summary().contains("nothing will be renamed"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn replace_is_literal_text() {
        assert_eq!(
            replace_all("IMG_0001.JPG", "IMG_", "beach-"),
            "beach-0001.JPG"
        );
        assert_eq!(replace_all("a.b.c", ".", "-"), "a-b-c");
        // No pattern language: these are just characters.
        assert_eq!(replace_all("x(1).txt", "(1)", ""), "x.txt");
        assert_eq!(replace_all("a*b", "*", "+"), "a+b");
        assert_eq!(replace_all("same", "", "x"), "same");
    }

    #[test]
    fn problems_explain_themselves() {
        assert!(RenameProblem::Exists("x.txt".into())
            .message()
            .contains("already exists"));
        assert!(RenameProblem::Duplicate("x.txt".into())
            .message()
            .contains("twice"));
    }
}
