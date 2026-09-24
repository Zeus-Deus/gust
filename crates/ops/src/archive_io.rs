//! Reading and writing archives (DESIGN §6.15, plan §4 and mockup C).
//!
//! The *policy* — what may be extracted and what must be refused — lives in
//! [`super`] and has been tested since early on. This module is the part
//! that was missing: getting a real archive's entries so the policy has
//! something to judge, and writing one back out.
//!
//! Listing never writes. Extraction vets every entry before a byte lands,
//! stages inside the destination, and commits with a rename.

use super::{ArchiveEntry, ArchiveEntryKind, ExtractPolicy, Rejection};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// At most this many entries are parsed for display. An archive with more is
/// still extractable; the *listing* is bounded so a malicious index cannot
/// make the UI chew through millions of lines.
pub const MAX_LISTED: usize = 2000;

/// Why an archive could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// bsdtar is not installed, so archives are unavailable.
    NoTool,
    /// The file is not an archive, or is a format this bsdtar cannot read.
    Unsupported(String),
    /// The tool ran and failed, with its own explanation.
    Failed(String),
}

impl ArchiveError {
    pub fn message(&self) -> String {
        match self {
            ArchiveError::NoTool => "Archives need bsdtar, which is not installed.".into(),
            ArchiveError::Unsupported(name) => {
                format!("Gust cannot read {name} as an archive.")
            }
            ArchiveError::Failed(why) => why.clone(),
        }
    }
}

/// Is the tool that reads and writes archives available?
pub fn tool_available() -> bool {
    Command::new("bsdtar")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Does this name look like something worth offering to open as an archive?
///
/// The name is a hint, never proof — the listing is what decides. Gust does
/// not run a file to find out what it is.
pub fn looks_like_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    const ENDINGS: &[&str] = &[
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst",
        ".zip", ".7z", ".rar", ".jar", ".gz", ".xz", ".zst",
    ];
    ENDINGS.iter().any(|e| lower.ends_with(e))
}

/// Parse one line of `bsdtar -tv` output into an entry.
///
/// The format is `mode links owner group size date... name`, with a link
/// target after " -> " for symlinks. Deliberately tolerant: a line that
/// cannot be parsed is skipped rather than guessed at.
pub fn parse_listing_line(line: &str) -> Option<ArchiveEntry> {
    let line = line.trim_end();
    if line.trim().is_empty() {
        return None;
    }
    let mut fields = line.split_whitespace();
    let mode_str = fields.next()?;
    if mode_str.len() < 10 {
        return None;
    }
    // Skip links, owner, group.
    let _links = fields.next()?;
    let _owner = fields.next()?;
    let _group = fields.next()?;
    let size: u64 = fields.next()?.parse().ok()?;

    // A date is "Mon DD HH:MM" or "Mon DD  YYYY" — three whitespace fields
    // after the size. Walk positionally: matching on " {size} " finds the
    // LINKS column when the size is 0, which swallowed the date into the
    // name.
    let mut cursor = line;
    for _ in 0..8 {
        let t = cursor.trim_start();
        let cut = t.find(char::is_whitespace)?;
        cursor = &t[cut..];
    }
    let name_part = cursor.trim_start().to_string();
    if name_part.is_empty() {
        return None;
    }

    let (name, link_target) = match name_part.split_once(" -> ") {
        Some((n, t)) => (n.to_string(), Some(t.to_string())),
        None => (name_part, None),
    };

    let kind = match mode_str.as_bytes()[0] {
        b'd' => ArchiveEntryKind::Directory,
        b'l' => ArchiveEntryKind::Symlink,
        b'h' => ArchiveEntryKind::Hardlink,
        b'p' | b'c' | b'b' | b's' => ArchiveEntryKind::Special,
        _ => ArchiveEntryKind::Regular,
    };

    Some(ArchiveEntry {
        name: name.trim_end_matches('/').to_string(),
        kind,
        link_target,
        size,
        mode: parse_mode(mode_str),
    })
}

/// Turn `-rwsr-xr-x` into 0o4755, so setuid entries can be refused.
fn parse_mode(s: &str) -> u32 {
    let b = s.as_bytes();
    if b.len() < 10 {
        return 0;
    }
    let bit = |i: usize, ch: u8| if b[i] == ch { 1 } else { 0 };
    let mut mode = 0u32;
    mode |= bit(1, b'r') << 8;
    mode |= bit(2, b'w') << 7;
    mode |= u32::from(b[3] == b'x' || b[3] == b's') << 6;
    mode |= bit(4, b'r') << 5;
    mode |= bit(5, b'w') << 4;
    mode |= u32::from(b[6] == b'x' || b[6] == b's') << 3;
    mode |= bit(7, b'r') << 2;
    mode |= bit(8, b'w') << 1;
    mode |= u32::from(b[9] == b'x' || b[9] == b't');
    // setuid / setgid / sticky, which the policy refuses.
    if b[3] == b's' || b[3] == b'S' {
        mode |= 0o4000;
    }
    if b[6] == b's' || b[6] == b'S' {
        mode |= 0o2000;
    }
    mode
}

/// What is inside this archive? Reads only; writes nothing.
pub fn list(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    if !tool_available() {
        return Err(ArchiveError::NoTool);
    }
    let out = Command::new("bsdtar")
        .arg("-tvf")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ArchiveError::Failed(e.to_string()))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if err.contains("Unrecognized") || err.contains("Unable to open") {
            return Err(ArchiveError::Unsupported(name));
        }
        return Err(ArchiveError::Failed(
            err.lines()
                .next_back()
                .unwrap_or("could not be read")
                .trim()
                .to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .take(MAX_LISTED)
        .filter_map(parse_listing_line)
        .collect())
}

/// A name in `dir` that is not taken: `photos.tar.zst`, then
/// `photos-2.tar.zst`, and so on.
///
/// Compressing twice must not silently replace the first archive.
pub fn unique_name(dir: &Path, wanted: &str) -> PathBuf {
    let candidate = dir.join(wanted);
    if !candidate.exists() {
        return candidate;
    }
    // Split on the FULL compound extension, so "a.tar.zst" becomes
    // "a" + ".tar.zst" rather than "a.tar" + ".zst".
    let (stem, ext) = match wanted.find(".tar.") {
        Some(i) => (&wanted[..i], &wanted[i..]),
        None => match wanted.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (wanted, ""),
        },
    };
    let dot = if ext.starts_with('.') || ext.is_empty() {
        ""
    } else {
        "."
    };
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}{dot}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(wanted)
}

/// What an extraction did, for reporting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extracted {
    pub files: usize,
    pub dirs: usize,
    /// Entries the policy refused. Shown to the user, never swallowed.
    pub refused: Vec<String>,
    /// Entries that clashed with something already there and landed under a
    /// free name instead, as `(archive name, name used)`. The existing file
    /// is the user's and is never replaced.
    pub renamed: Vec<(String, String)>,
}

impl Extracted {
    pub fn summary(&self) -> String {
        let mut s = match (self.files, self.dirs) {
            (0, 0) => "Nothing was extracted".to_string(),
            (f, 0) => format!("Extracted {f} file{}", plural(f)),
            (0, d) => format!("Extracted {d} folder{}", plural(d)),
            (f, d) => format!(
                "Extracted {f} file{} and {d} folder{}",
                plural(f),
                plural(d)
            ),
        };
        if !self.renamed.is_empty() {
            s.push_str(&format!(
                " · {} renamed to keep existing file{}",
                self.renamed.len(),
                plural(self.renamed.len())
            ));
        }
        if !self.refused.is_empty() {
            s.push_str(&format!(
                " · refused {} unsafe entr{}",
                self.refused.len(),
                if self.refused.len() == 1 { "y" } else { "ies" }
            ));
        }
        s
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The argv for extracting into a staging directory.
///
/// `--` ends option parsing, so an archive named `--one-file-system` is a
/// file name rather than a flag. `-P` is deliberately NOT passed: without
/// it bsdtar refuses absolute paths itself, which is a second line of
/// defence behind the policy.
pub fn extract_args(archive: &Path, staging: &Path) -> Vec<String> {
    vec![
        "-xf".into(),
        archive.to_string_lossy().into_owned(),
        "-C".into(),
        staging.to_string_lossy().into_owned(),
        "--no-same-owner".into(),
        "--no-same-permissions".into(),
    ]
}

/// The argv for compressing `names` (relative to `dir`) into `archive`.
///
/// Every name is a separate argument after `--`, so a file called
/// `--checkpoint-action=exec=sh` cannot become an option.
pub fn compress_args(dir: &Path, archive: &Path, names: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-c".into(),
        "--zstd".into(),
        "-f".into(),
        archive.to_string_lossy().into_owned(),
        "-C".into(),
        dir.to_string_lossy().into_owned(),
        "--".into(),
    ];
    a.extend(names.iter().cloned());
    a
}

/// Extract an archive into `dest`, refusing anything the policy rejects.
///
/// Stages inside the destination so the commit is a same-filesystem rename,
/// and removes the staging directory on failure: a half-extracted tree that
/// looks like success is worse than an error.
pub fn extract(
    archive: &Path,
    dest: &Path,
    policy: &ExtractPolicy,
) -> Result<Extracted, ArchiveError> {
    let entries = list(archive)?;
    let (allowed, refused) = super::vet_entries(&entries, policy);

    if allowed.is_empty() && !refused.is_empty() {
        return Ok(Extracted {
            files: 0,
            dirs: 0,
            refused: refused.iter().map(describe_rejection).collect(),
            ..Default::default()
        });
    }

    let staging = super::staging_dir(dest);
    let _ = super::cleanup_staging(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| ArchiveError::Failed(e.to_string()))?;

    let out = Command::new("bsdtar")
        .args(extract_args(archive, &staging))
        .stdin(Stdio::null())
        .output();

    let out = match out {
        Ok(o) => o,
        Err(e) => {
            let _ = super::cleanup_staging(&staging);
            return Err(ArchiveError::Failed(e.to_string()));
        }
    };
    if !out.status.success() {
        let _ = super::cleanup_staging(&staging);
        return Err(ArchiveError::Failed(
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next_back()
                .unwrap_or("extraction failed")
                .trim()
                .to_string(),
        ));
    }

    // Move only what the policy allowed. Anything bsdtar produced that the
    // policy did not sanction stays in staging and is deleted with it.
    let mut done = Extracted {
        refused: refused.iter().map(describe_rejection).collect(),
        ..Default::default()
    };
    for (entry, rel) in &allowed {
        let from = staging.join(rel);
        let to = dest.join(rel);
        if !from.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if entry.kind == ArchiveEntryKind::Directory {
            if std::fs::create_dir_all(&to).is_ok() {
                done.dirs += 1;
            }
            continue;
        }
        // Never onto an existing name: a same-named file already in the
        // folder is the user's. The archive's copy takes a free name beside
        // it, chosen and taken atomically so a race cannot clobber either.
        let wanted = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = to.parent().map(Path::to_path_buf).unwrap_or_else(|| dest.to_path_buf());
        let mut target = to.clone();
        for _ in 0..16 {
            match crate::anchor::rename_noreplace_paths(&from, &target) {
                Ok(()) => {
                    done.files += 1;
                    if target != to {
                        done.renamed.push((
                            rel.to_string_lossy().into_owned(),
                            target.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                        ));
                    }
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    target = unique_name(&parent, &wanted);
                }
                Err(_) => break,
            }
        }
    }
    let _ = super::cleanup_staging(&staging);
    Ok(done)
}

fn describe_rejection(r: &Rejection) -> String {
    match r {
        Rejection::PathTraversal(n) => format!("{n} (writes outside the folder)"),
        Rejection::SpecialFile(n) => format!("{n} (not a normal file)"),
        Rejection::PrivilegedMode(n) => format!("{n} (asks for extra privilege)"),
        Rejection::EscapingLink { name, target } => {
            format!("{name} (links out to {target})")
        }
        Rejection::ExpansionLimit { name, .. } => format!("{name} (too large)"),
        Rejection::EntryLimit => "too many entries".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_line_is_parsed() {
        let e = parse_listing_line("-rw-r--r--  0 user   user        2 Sep 13 19:48 a.txt")
            .expect("should parse");
        assert_eq!(e.name, "a.txt");
        assert_eq!(e.size, 2);
        assert_eq!(e.kind, ArchiveEntryKind::Regular);
    }

    #[test]
    fn a_directory_is_recognised_and_its_slash_dropped() {
        let e = parse_listing_line("drwxr-xr-x  0 user   user        0 Sep 13 19:48 sub/").unwrap();
        assert_eq!(e.kind, ArchiveEntryKind::Directory);
        assert_eq!(e.name, "sub");
    }

    #[test]
    fn a_symlink_keeps_its_target() {
        let e = parse_listing_line(
            "lrwxrwxrwx  0 user   user        0 Sep 13 19:48 link -> ../outside",
        )
        .unwrap();
        assert_eq!(e.kind, ArchiveEntryKind::Symlink);
        assert_eq!(e.name, "link");
        assert_eq!(e.link_target.as_deref(), Some("../outside"));
    }

    #[test]
    fn a_fifo_is_recognised_as_special() {
        let e = parse_listing_line("prw-r--r--  0 user   user        0 Sep 13 19:48 pipe").unwrap();
        assert_eq!(e.kind, ArchiveEntryKind::Special);
    }

    #[test]
    fn a_setuid_entry_carries_its_privileged_bit() {
        let e = parse_listing_line("-rwsr-xr-x  0 root   root     1234 Sep 13 19:48 evil").unwrap();
        assert_ne!(e.mode & 0o4000, 0, "setuid must survive parsing");
    }

    #[test]
    fn a_name_with_spaces_survives() {
        let e = parse_listing_line(
            "-rw-r--r--  0 user   user       10 Sep 13 19:48 my holiday photos.txt",
        )
        .unwrap();
        assert_eq!(e.name, "my holiday photos.txt");
    }

    #[test]
    fn a_year_dated_entry_parses_too() {
        let e =
            parse_listing_line("-rw-r--r--  0 user   user       10 Jan  1  2020 old.txt").unwrap();
        assert_eq!(e.name, "old.txt");
    }

    #[test]
    fn junk_is_skipped_not_guessed_at() {
        assert!(parse_listing_line("").is_none());
        assert!(parse_listing_line("   ").is_none());
        assert!(parse_listing_line("not a listing line").is_none());
    }

    #[test]
    fn archive_names_are_recognised_by_ending_only() {
        assert!(looks_like_archive("backup.tar.zst"));
        assert!(looks_like_archive("photos.ZIP"));
        assert!(!looks_like_archive("notes.txt"));
        // A name is a hint, never proof — nothing is run to find out.
        assert!(!looks_like_archive("tar"));
    }

    #[test]
    fn no_name_can_become_an_option() {
        let args = compress_args(
            Path::new("/tmp"),
            Path::new("/tmp/out.tar.zst"),
            &["--checkpoint-action=exec=sh".to_string()],
        );
        let dashdash = args.iter().position(|a| a == "--").expect("-- present");
        let name_at = args
            .iter()
            .position(|a| a == "--checkpoint-action=exec=sh")
            .unwrap();
        assert!(
            name_at > dashdash,
            "every name must come after --, or it is an option"
        );
    }

    #[test]
    fn extraction_does_not_preserve_owner_or_permissions() {
        let args = extract_args(Path::new("/tmp/a.tar"), Path::new("/tmp/stage"));
        assert!(args.iter().any(|a| a == "--no-same-owner"));
        assert!(args.iter().any(|a| a == "--no-same-permissions"));
        // -P would let absolute paths through; it must never be passed.
        assert!(!args.iter().any(|a| a == "-P"));
    }

    #[test]
    fn a_summary_says_what_was_refused() {
        let e = Extracted {
            files: 3,
            dirs: 1,
            refused: vec!["evil (asks for extra privilege)".into()],
            ..Default::default()
        };
        let s = e.summary();
        assert!(s.contains("3 files"));
        assert!(s.contains("1 folder"));
        assert!(s.contains("refused 1 unsafe entry"));
    }

    #[test]
    fn an_empty_extraction_says_so() {
        assert_eq!(Extracted::default().summary(), "Nothing was extracted");
    }

    #[test]
    fn a_second_archive_does_not_replace_the_first() {
        let d = std::env::temp_dir().join("gust-arch-unique");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        // Nothing there yet: the wanted name is free.
        assert_eq!(unique_name(&d, "photos.tar.zst"), d.join("photos.tar.zst"));

        std::fs::write(d.join("photos.tar.zst"), "x").unwrap();
        // The compound extension must survive intact.
        assert_eq!(
            unique_name(&d, "photos.tar.zst"),
            d.join("photos-2.tar.zst")
        );

        std::fs::write(d.join("photos-2.tar.zst"), "x").unwrap();
        assert_eq!(
            unique_name(&d, "photos.tar.zst"),
            d.join("photos-3.tar.zst")
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn errors_are_actionable() {
        assert!(ArchiveError::NoTool.message().contains("bsdtar"));
        assert!(ArchiveError::Unsupported("x.bin".into())
            .message()
            .contains("x.bin"));
    }
}
