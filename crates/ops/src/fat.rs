//! Copying onto FAT (USB sticks, SD cards, camera cards).
//!
//! FAT32 cannot hold a file of 4 GiB or more, and it refuses names with
//! `< > : " / \ | ? *`, control characters, or a trailing dot or space.
//! Other file managers find out halfway through a copy (a 4.3 GB video fails
//! at 99 %) or silently drop the file. Gust checks before a byte is written:
//! too-large files refuse the transfer up front, and bad names are rewritten
//! to a safe spelling that is reported, never dropped.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// Largest file FAT32 can store: 4 GiB − 1 byte.
pub const FAT32_MAX_FILE: u64 = 0xFFFF_FFFF;

/// Is this `/proc/mounts` filesystem type FAT (where these rules apply)?
/// exFAT has no 4 GiB limit but shares the name rules.
pub fn is_fat(fstype: &str) -> bool {
    matches!(fstype, "vfat" | "msdos" | "fat" | "exfat")
}

pub fn has_size_limit(fstype: &str) -> bool {
    matches!(fstype, "vfat" | "msdos" | "fat")
}

/// The FAT-safe spelling of a name, or `None` when it is already safe.
/// Each forbidden character becomes `_`; trailing dots and spaces are
/// removed (Windows and the kernel's vfat both reject them); a name that
/// becomes empty is `_`.
pub fn safe_name(name: &OsStr) -> Option<OsString> {
    let s = name.to_string_lossy();
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        out.push('_');
    }
    // Lossy conversion changed invalid UTF-8: that is a rename too.
    if out == s && s.as_ref() == name.to_str().unwrap_or("\u{0}") {
        None
    } else {
        Some(OsString::from(out))
    }
}

/// Map every component of a relative path to its safe spelling.
pub fn safe_rel(rel: &Path) -> PathBuf {
    rel.components()
        .map(|c| match c {
            Component::Normal(n) => safe_name(n).unwrap_or_else(|| n.to_os_string()),
            other => other.as_os_str().to_os_string(),
        })
        .collect()
}

/// What a copy onto FAT would have to change or refuse, found up front.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Preflight {
    /// Files at or over 4 GiB: the transfer is refused before it starts.
    pub too_large: Vec<(PathBuf, u64)>,
    /// (original relative path, the name it will get).
    pub renamed: Vec<(PathBuf, PathBuf)>,
}

impl Preflight {
    /// The refusal the user sees, naming the first files.
    pub fn refusal(&self) -> Option<String> {
        if self.too_large.is_empty() {
            return None;
        }
        let names: Vec<String> = self
            .too_large
            .iter()
            .take(3)
            .map(|(p, _)| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        let more = self.too_large.len().saturating_sub(3);
        Some(format!(
            "This drive is FAT32, which cannot hold files of 4 GB or more: {}{}. Nothing was copied. \
             Reformat the drive as exFAT to copy them.",
            names.join(", "),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        ))
    }

    /// One sentence about renames, for the finished transfer.
    pub fn renamed_note(&self) -> Option<String> {
        match self.renamed.len() {
            0 => None,
            1 => Some(format!(
                "Renamed 1 item for this drive: {} → {}",
                self.renamed[0].0.display(),
                self.renamed[0].1.display()
            )),
            n => Some(format!(
                "Renamed {n} items for this drive (characters FAT cannot store became _)"
            )),
        }
    }
}

/// Check a planned copy: `files` are (relative path, size) inside one
/// source (for a single file, its own name), `dirs` and `links` the rest.
pub fn preflight<'a>(
    fstype: &str,
    files: impl IntoIterator<Item = (&'a Path, u64)>,
    others: impl IntoIterator<Item = &'a Path>,
) -> Preflight {
    let mut p = Preflight::default();
    if !is_fat(fstype) {
        return p;
    }
    let limit = has_size_limit(fstype);
    let mut note = |rel: &Path| {
        let safe = safe_rel(rel);
        if safe != rel {
            p.renamed.push((rel.to_path_buf(), safe));
        }
    };
    let mut big = Vec::new();
    for (rel, size) in files {
        if limit && size > FAT32_MAX_FILE {
            big.push((rel.to_path_buf(), size));
        }
        note(rel);
    }
    for rel in others {
        note(rel);
    }
    p.too_large = big;
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_replace_what_fat_forbids() {
        assert_eq!(safe_name(OsStr::new("ok.txt")), None);
        assert_eq!(safe_name(OsStr::new("a:b?.txt")).unwrap(), "a_b_.txt");
        assert_eq!(safe_name(OsStr::new("Q&A: why?")).unwrap(), "Q&A_ why_");
        assert_eq!(safe_name(OsStr::new("trailing. ")).unwrap(), "trailing");
        assert_eq!(safe_name(OsStr::new("...")).unwrap(), "_");
        assert_eq!(safe_name(OsStr::new("tab\there")).unwrap(), "tab_here");
        // Unicode is fine on FAT (long names are UTF-16).
        assert_eq!(safe_name(OsStr::new("café 日本.jpg")), None);
    }

    #[test]
    fn invalid_utf8_is_renamed_not_dropped() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"bad\xffname");
        let s = safe_name(raw).expect("must be renamed");
        assert!(s.to_str().is_some());
    }

    #[test]
    fn every_path_component_is_mapped() {
        assert_eq!(
            safe_rel(Path::new("Album: 2024/track?.mp3")),
            PathBuf::from("Album_ 2024/track_.mp3")
        );
    }

    #[test]
    fn a_file_over_4_gib_refuses_the_whole_copy_up_front() {
        let big = Path::new("film.mkv");
        let small = Path::new("notes.txt");
        let p = preflight(
            "vfat",
            [(big, FAT32_MAX_FILE + 1), (small, 10)],
            std::iter::empty(),
        );
        assert_eq!(p.too_large.len(), 1);
        let msg = p.refusal().unwrap();
        assert!(
            msg.contains("film.mkv") && msg.contains("Nothing was copied"),
            "{msg}"
        );
    }

    #[test]
    fn exactly_the_limit_fits() {
        let p = preflight(
            "vfat",
            [(Path::new("a"), FAT32_MAX_FILE)],
            std::iter::empty(),
        );
        assert!(p.refusal().is_none());
    }

    #[test]
    fn exfat_has_no_size_limit_but_the_name_rules() {
        let p = preflight(
            "exfat",
            [(Path::new("a?.mkv"), FAT32_MAX_FILE * 3)],
            std::iter::empty(),
        );
        assert!(p.too_large.is_empty());
        assert_eq!(p.renamed.len(), 1);
    }

    #[test]
    fn other_filesystems_are_untouched() {
        let p = preflight(
            "btrfs",
            [(Path::new("a:b"), FAT32_MAX_FILE * 2)],
            std::iter::empty(),
        );
        assert_eq!(p, Preflight::default());
    }
}
