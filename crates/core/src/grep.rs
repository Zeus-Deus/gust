//! Content search — `:grep` (DESIGN §6.10, plan §2 and §4).
//!
//! Names are matched by [`super::find_by_name`]; this matches what is *in*
//! a file. Both are explicitly scoped, bounded and cancellable, and neither
//! ever starts on its own.
//!
//! The bounds are not tuning knobs, they are the feature: a content search
//! that reads a 4 GB disk image, or walks a cloud mount, is a search that
//! nobody can use.

use crate::lister::CancelToken;
use crate::search::SearchScope;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Never read more than this from one file. A match beyond 8 MiB into a
/// file is not worth making every other file wait.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// How much of the head decides "is this binary".
const SNIFF_BYTES: usize = 8 * 1024;

/// One file that contained the needle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHit {
    pub path: PathBuf,
    /// How many lines matched, not the lines themselves: Gust counts rather
    /// than building a giant buffer it would then have to show somewhere.
    pub lines: u32,
    /// The first matching line number, so "open it and look here" is
    /// possible later without re-reading now.
    pub first_line: u32,
}

/// Why a search stopped, so the UI can say so rather than implying it
/// finished when it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepOutcome {
    /// The whole scope was searched.
    Complete,
    /// Stopped at `max_results`.
    HitLimit,
    /// The user cancelled it.
    Cancelled,
}

/// Does this look like a file whose contents are text?
///
/// A NUL byte in the head is the standard, cheap signal. Being wrong in the
/// conservative direction (skipping something that was technically text)
/// costs a missed hit; being wrong the other way costs reading a disk image.
pub fn looks_binary(head: &[u8]) -> bool {
    head.contains(&0)
}

/// Read at most the head of a file for sniffing plus matching.
fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    let meta = f.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let n = meta.len().min(limit) as usize;
    let mut buf = Vec::with_capacity(n.min(1 << 20));
    f.take(limit).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Count matching lines in `data`, case-insensitively for ASCII.
///
/// Bytes are compared, not characters: a file that is not valid UTF-8 must
/// still be searchable, and must never panic the search.
pub fn count_matches(data: &[u8], needle: &str) -> (u32, u32) {
    if needle.is_empty() {
        return (0, 0);
    }
    let needle_lower: Vec<u8> = needle.as_bytes().to_ascii_lowercase();
    let mut lines = 0u32;
    let mut first = 0u32;
    for (i, line) in data.split(|b| *b == b'\n').enumerate() {
        let lower = line.to_ascii_lowercase();
        if window_contains(&lower, &needle_lower) {
            lines += 1;
            if first == 0 {
                first = (i + 1) as u32;
            }
        }
    }
    (lines, first)
}

fn window_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Search file *contents* within an explicit scope.
///
/// Calls `on_hit` as results arrive so the UI can show them streaming, and
/// checks `cancel` between every file.
pub fn grep_contents(
    scope: &SearchScope,
    needle: &str,
    cancel: &CancelToken,
    mut on_hit: impl FnMut(&ContentHit),
) -> (Vec<ContentHit>, GrepOutcome) {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return (hits, GrepOutcome::Complete);
    }
    let Some(root) = scope.root.as_path() else {
        return (hits, GrepOutcome::Complete);
    };
    let root_dev = std::fs::metadata(root).ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.dev()
    });

    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut outcome = GrepOutcome::Complete;

    while let Some((dir, depth)) = stack.pop() {
        if cancel.is_cancelled() {
            return (hits, GrepOutcome::Cancelled);
        }
        if hits.len() >= scope.max_results {
            outcome = GrepOutcome::HitLimit;
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if cancel.is_cancelled() {
                return (hits, GrepOutcome::Cancelled);
            }
            if hits.len() >= scope.max_results {
                outcome = GrepOutcome::HitLimit;
                break;
            }
            let name = entry.file_name();
            let is_hidden = name.to_string_lossy().starts_with('.');
            if is_hidden && !scope.include_hidden {
                continue;
            }
            // Symlinks are NOT followed: a link into / would turn a scoped
            // search into a whole-disk one, and a link loop would never end.
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();

            if meta.is_dir() {
                if !scope.recursive || depth + 1 >= scope.max_depth {
                    continue;
                }
                if scope.stay_on_device {
                    use std::os::unix::fs::MetadataExt;
                    if root_dev.is_some_and(|d| meta.dev() != d) {
                        // A different filesystem: on a remote this would mean
                        // downloading everything to search it (DESIGN §6.10).
                        continue;
                    }
                }
                stack.push((path, depth + 1));
                continue;
            }
            if !meta.is_file() {
                continue;
            }

            let Some(data) = read_bounded(&path, MAX_FILE_BYTES) else {
                continue;
            };
            let head = &data[..data.len().min(SNIFF_BYTES)];
            if looks_binary(head) {
                continue;
            }
            let (lines, first_line) = count_matches(&data, needle);
            if lines > 0 {
                let hit = ContentHit {
                    path,
                    lines,
                    first_line,
                };
                on_hit(&hit);
                hits.push(hit);
            }
        }
    }
    (hits, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::LocationKey;

    fn scope_for(dir: &Path, recursive: bool) -> SearchScope {
        SearchScope {
            root: LocationKey::local(dir),
            recursive,
            max_depth: if recursive { 8 } else { 1 },
            stay_on_device: true,
            include_hidden: false,
            max_results: 500,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-grep-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_nul_byte_means_binary() {
        assert!(looks_binary(b"\x7fELF\0\0\0"));
        assert!(!looks_binary(b"plain text, no nul"));
        assert!(!looks_binary(b""));
    }

    #[test]
    fn matches_are_counted_per_line() {
        let data = b"alpha\nbeta needle\ngamma\nneedle again\n";
        let (lines, first) = count_matches(data, "needle");
        assert_eq!(lines, 2);
        assert_eq!(first, 2);
    }

    #[test]
    fn matching_is_case_insensitive_for_ascii() {
        let (lines, _) = count_matches(b"The Needle Here\n", "needle");
        assert_eq!(lines, 1);
    }

    #[test]
    fn an_empty_needle_matches_nothing() {
        // Otherwise ":grep " would report every file in the tree.
        let (lines, _) = count_matches(b"anything at all\n", "");
        assert_eq!(lines, 0);
    }

    #[test]
    fn invalid_utf8_does_not_panic_and_still_matches() {
        let mut data = b"needle ".to_vec();
        data.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        data.push(b'\n');
        let (lines, _) = count_matches(&data, "needle");
        assert_eq!(lines, 1);
    }

    #[test]
    fn finds_a_match_in_a_real_file() {
        let d = tmp("basic");
        std::fs::write(d.join("a.txt"), "nothing here\n").unwrap();
        std::fs::write(d.join("b.txt"), "the needle is here\n").unwrap();
        let (hits, outcome) =
            grep_contents(&scope_for(&d, false), "needle", &CancelToken::new(), |_| {});
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path.file_name().unwrap(), "b.txt");
        assert_eq!(hits[0].lines, 1);
        assert_eq!(outcome, GrepOutcome::Complete);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_binary_file_is_skipped_even_when_it_contains_the_bytes() {
        let d = tmp("binary");
        let mut blob = b"needle".to_vec();
        blob.push(0);
        blob.extend_from_slice(b"more");
        std::fs::write(d.join("thing.bin"), blob).unwrap();
        let (hits, _) = grep_contents(&scope_for(&d, false), "needle", &CancelToken::new(), |_| {});
        assert!(hits.is_empty(), "a binary file must not be searched");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn recursion_is_opt_in() {
        let d = tmp("recurse");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/deep.txt"), "needle down here\n").unwrap();

        let (shallow, _) =
            grep_contents(&scope_for(&d, false), "needle", &CancelToken::new(), |_| {});
        assert!(
            shallow.is_empty(),
            "a non-recursive search must not descend"
        );

        let (deep, _) = grep_contents(&scope_for(&d, true), "needle", &CancelToken::new(), |_| {});
        assert_eq!(deep.len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_is_not_followed() {
        let d = tmp("symlink");
        let outside = tmp("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "needle in another tree\n").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();

        let (hits, _) = grep_contents(&scope_for(&d, true), "needle", &CancelToken::new(), |_| {});
        assert!(
            hits.is_empty(),
            "following a symlink would turn a scoped search into a whole-disk one"
        );
        std::fs::remove_dir_all(&d).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn cancelling_stops_and_says_so() {
        let d = tmp("cancel");
        for i in 0..50 {
            std::fs::write(d.join(format!("f{i}.txt")), "needle\n").unwrap();
        }
        let cancel = CancelToken::new();
        cancel.cancel();
        let (hits, outcome) = grep_contents(&scope_for(&d, false), "needle", &cancel, |_| {});
        assert!(hits.is_empty());
        assert_eq!(outcome, GrepOutcome::Cancelled);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn hitting_the_limit_is_reported_not_hidden() {
        let d = tmp("limit");
        for i in 0..10 {
            std::fs::write(d.join(format!("f{i}.txt")), "needle\n").unwrap();
        }
        let mut scope = scope_for(&d, false);
        scope.max_results = 3;
        let (hits, outcome) = grep_contents(&scope, "needle", &CancelToken::new(), |_| {});
        assert_eq!(hits.len(), 3);
        assert_eq!(
            outcome,
            GrepOutcome::HitLimit,
            "stopping early must be visible, not silently look like 'that is all'"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn hidden_files_are_skipped_unless_asked_for() {
        let d = tmp("hidden");
        std::fs::write(d.join(".secret"), "needle\n").unwrap();
        let (hits, _) = grep_contents(&scope_for(&d, false), "needle", &CancelToken::new(), |_| {});
        assert!(hits.is_empty());

        let mut scope = scope_for(&d, false);
        scope.include_hidden = true;
        let (with_hidden, _) = grep_contents(&scope, "needle", &CancelToken::new(), |_| {});
        assert_eq!(with_hidden.len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn results_stream_as_they_are_found() {
        let d = tmp("stream");
        for i in 0..5 {
            std::fs::write(d.join(format!("f{i}.txt")), "needle\n").unwrap();
        }
        let mut seen = 0;
        let (hits, _) = grep_contents(&scope_for(&d, false), "needle", &CancelToken::new(), |_| {
            seen += 1;
        });
        assert_eq!(seen, hits.len(), "every hit must be announced as it lands");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_huge_file_is_read_only_up_to_the_bound() {
        let d = tmp("huge");
        // The needle sits past the cap, so a bounded read must NOT find it.
        let mut data = vec![b'x'; (MAX_FILE_BYTES + 4096) as usize];
        data.extend_from_slice(b"\nneedle\n");
        std::fs::write(d.join("big.txt"), &data).unwrap();
        let (hits, _) = grep_contents(&scope_for(&d, false), "needle", &CancelToken::new(), |_| {});
        assert!(
            hits.is_empty(),
            "reading past the bound would make one file stall the whole search"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
