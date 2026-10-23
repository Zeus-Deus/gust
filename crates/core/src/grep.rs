//! Scoped content search. The legacy absolute traversal is coverage-unverified.
//! New callers use the detailed report rather than inferring completeness.
use crate::lister::CancelToken;
use crate::search::SearchScope;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

mod matcher;
mod report;
pub use matcher::{
    match_buffer, ContentMatchError, ContentMatches, ContentQueryError, ContentSnippet,
    MAX_QUERY_BYTES, MAX_SNIPPET_BYTES,
};
pub use report::*;

pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHit {
    pub path: PathBuf,
    pub lines: u32,
    /// First matching line, one-based.
    pub first_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepOutcome {
    /// Legacy walk ended; coverage remains unverified. Use the detailed report.
    Complete,
    /// A resource bound stopped the compatibility walk; use the report for its reason.
    HitLimit,
    Cancelled,
}

pub fn looks_binary(head: &[u8]) -> bool {
    head.contains(&0)
}

/// Compatibility counts. Use `match_buffer` to retain errors and byte-limit state.
pub fn count_matches(data: &[u8], needle: &str) -> (u32, u32) {
    match_buffer(data, needle, &CancelToken::new())
        .map(|matched| (matched.lines, matched.first_line))
        .unwrap_or((0, 0))
}

/// Compatibility adapter. Detailed reports expose partial coverage and omissions.
pub fn grep_contents(
    scope: &SearchScope,
    needle: &str,
    cancel: &CancelToken,
    mut on_hit: impl FnMut(&ContentHit),
) -> (Vec<ContentHit>, GrepOutcome) {
    let report = grep_with_report(scope, needle, cancel, |hit| on_hit(&hit.hit));
    let outcome = match report.state {
        ContentSearchState::Cancelled => GrepOutcome::Cancelled,
        ContentSearchState::Limited(_) => GrepOutcome::HitLimit,
        _ => GrepOutcome::Complete,
    };
    (
        report.hits.into_iter().map(|hit| hit.hit).collect(),
        outcome,
    )
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
