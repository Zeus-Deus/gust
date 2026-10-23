use gust_core::{grep::*, location::LocationKey, search::SearchScope, CancelToken};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gust-content-details-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn scope(&self) -> SearchScope {
        SearchScope::current_dir(LocationKey::local(&self.0))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn matched(data: &[u8], needle: &str) -> ContentMatches {
    match_buffer(data, needle, &CancelToken::new()).unwrap()
}

#[test]
fn unicode_simple_lower_matching_preserves_counts_and_one_based_lines() {
    let data = "nothing\nCAFÉ café\nÉLAN\nİSTANBUL\nΛΑΜΠΑΔΑ\n";
    for (needle, line) in [
        ("café", 2),
        ("élan", 3),
        ("i\u{307}stanbul", 4),
        ("λαμπαδα", 5),
    ] {
        let hit = matched(data.as_bytes(), needle);
        assert_eq!((hit.lines, hit.first_line), (1, line));
        assert!(!hit.ascii_fallback);
        assert!(!hit.truncated);
    }
    assert_eq!(matched(b"Needle needle\nNEEDLE\n", "needle").lines, 2);
    assert_eq!(matched("Straße".as_bytes(), "STRASSE").lines, 0);
    assert_eq!(matched("é".as_bytes(), "e\u{301}").lines, 0);
}

#[test]
fn invalid_utf8_keeps_ascii_byte_behavior_instead_of_lossy_unicode_matching() {
    let bytes = b"CAF\xc3\x89 Needle \xff\n";
    let ascii = matched(bytes, "needle");
    assert_eq!(ascii.lines, 1);
    assert!(ascii.ascii_fallback);
    assert_eq!(matched(bytes, "café").lines, 0);
    assert_eq!(matched(bytes, "CAFÉ").lines, 1);
    assert_eq!(ascii.snippet.unwrap().text, "CAFÉ Needle ·");
}

#[test]
fn snippets_use_bounded_sanitized_context_near_the_first_match() {
    let line = "\t".repeat(1000) + "\u{202e}needle<script>\t" + &"😀".repeat(1000);
    let hit = matched(
        ("none\n".to_owned() + &line + "\nneedle again").as_bytes(),
        "needle",
    );
    assert_eq!((hit.lines, hit.first_line), (2, 2));
    let snippet = hit.snippet.unwrap();
    assert!(snippet.text.len() <= MAX_SNIPPET_BYTES);
    assert!(snippet.truncated);
    assert!(snippet.text.contains("needle<script>"));
    assert!(!snippet.text.contains(['\t', '\u{202e}', '\n']));
    assert!(std::str::from_utf8(snippet.text.as_bytes()).is_ok());
    assert_eq!(
        matched(("n".to_owned() + &"x".repeat(255)).as_bytes(), "n")
            .snippet
            .unwrap(),
        ContentSnippet {
            text: "n".to_owned() + &"x".repeat(255),
            truncated: false
        }
    );
    assert!(
        matched(("n".to_owned() + &"x".repeat(256)).as_bytes(), "n")
            .snippet
            .unwrap()
            .truncated
    );
}

#[test]
fn byte_bound_is_explicit_and_does_not_match_beyond_the_prefix() {
    let mut data = vec![b'x'; MAX_FILE_BYTES as usize];
    data.extend_from_slice(b"\nneedle\n");
    let hit = matched(&data, "needle");
    assert_eq!(hit.lines, 0);
    assert!(hit.truncated);
    assert_eq!(hit.bytes, MAX_FILE_BYTES as usize);
    let mut exact = vec![b'x'; MAX_FILE_BYTES as usize];
    exact[0] = b'n';
    let hit = matched(&exact, "n");
    assert_eq!(hit.lines, 1);
    assert!(!hit.truncated);
}

#[test]
fn invalid_queries_and_cancellation_are_typed_before_any_scope_read() {
    let scope = SearchScope::current_dir(LocationKey::local(Path::new(
        "/synthetic/no-content-source",
    )));
    for (query, error) in [
        (String::new(), ContentQueryError::Empty),
        ("x".repeat(MAX_QUERY_BYTES + 1), ContentQueryError::TooLong),
    ] {
        let report = grep_with_report(&scope, &query, &CancelToken::new(), |_| panic!("no hit"));
        assert_eq!(report.state, ContentSearchState::InvalidQuery(error));
        assert_eq!(report.stats.read_errors, 0);
        assert_eq!(report.stats.entries, 0);
        assert_eq!(
            match_buffer(b"text", &query, &CancelToken::new()),
            Err(ContentMatchError::InvalidQuery(error))
        );
    }
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        match_buffer(b"needle", "needle", &cancel),
        Err(ContentMatchError::Cancelled)
    );
    let report = grep_with_report(&scope, "needle", &cancel, |_| panic!("no hit"));
    assert_eq!(report.state, ContentSearchState::Cancelled);
    assert_eq!(report.stats.read_errors, 0);
}

#[test]
fn captured_details_stream_and_survive_removal_without_reopening_a_path() {
    let fixture = Fixture::new();
    let name = OsString::from_vec(b"raw\xff.txt".to_vec());
    let path = fixture.0.join(name);
    std::fs::write(&path, b"first\nNeedle original\nneedle again\n").unwrap();
    let mut captured = Vec::new();
    let report = grep_with_report(&fixture.scope(), "needle", &CancelToken::new(), |hit| {
        captured.push(hit.clone());
        std::fs::remove_file(&hit.hit.path).unwrap();
    });
    assert_eq!(report.state, ContentSearchState::Partial);
    assert!(report.stats.scope_unverified);
    assert_eq!(report.hits, captured);
    assert_eq!(report.hits[0].hit.path, path);
    assert_eq!(
        (report.hits[0].hit.lines, report.hits[0].hit.first_line),
        (2, 2)
    );
    assert_eq!(report.hits[0].snippet.text, "Needle original");
    assert!(!path.exists());
}

#[test]
fn omissions_and_binary_head_reads_are_counted_without_claiming_completion() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("match"), b"needle").unwrap();
    std::fs::write(fixture.0.join(".hidden"), b"needle").unwrap();
    std::fs::create_dir(fixture.0.join("sub")).unwrap();
    std::os::unix::fs::symlink("match", fixture.0.join("link")).unwrap();
    let binary = std::fs::File::create(fixture.0.join("binary")).unwrap();
    binary.set_len(MAX_FILE_BYTES * 2).unwrap();
    std::fs::write(fixture.0.join("invalid"), b"needle\xff").unwrap();
    let report = grep_with_report(&fixture.scope(), "needle", &CancelToken::new(), |_| {});
    assert_eq!(report.state, ContentSearchState::Partial);
    assert_eq!(report.hits.len(), 2);
    assert_eq!(
        (
            report.stats.binary_files,
            report.stats.hidden_pruned,
            report.stats.depth_pruned,
            report.stats.symlink_pruned,
            report.stats.ascii_fallback_files
        ),
        (1, 1, 1, 1, 1)
    );
    assert!(report.stats.read_bytes < 16 * 1024);
    let missing = SearchScope::current_dir(LocationKey::local(fixture.0.join("missing")));
    let report = grep_with_report(&missing, "needle", &CancelToken::new(), |_| {});
    assert_eq!(report.state, ContentSearchState::Partial);
    assert!(report.stats.read_errors > 0);
}

#[test]
fn requested_hit_limit_and_cancel_callback_keep_explicit_terminal_states() {
    let fixture = Fixture::new();
    for name in ["a", "b"] {
        std::fs::write(fixture.0.join(name), b"needle").unwrap();
    }
    let mut scope = fixture.scope();
    scope.max_results = 1;
    let report = grep_with_report(&scope, "needle", &CancelToken::new(), |_| {});
    assert_eq!(
        report.state,
        ContentSearchState::Limited(ContentSearchLimit::Hits)
    );
    assert_eq!(report.hits.len(), 1);
    let cancel = CancelToken::new();
    let token = cancel.clone();
    let report = grep_with_report(&fixture.scope(), "needle", &cancel, |_| token.cancel());
    assert_eq!(report.state, ContentSearchState::Cancelled);
    assert_eq!(report.hits.len(), 1);
}

#[test]
fn large_file_omissions_and_serialized_states_are_explicit() {
    let fixture = Fixture::new();
    let data = "n".repeat(MAX_FILE_BYTES as usize + 1);
    std::fs::write(fixture.0.join("large"), data).unwrap();
    let report = grep_with_report(&fixture.scope(), "n", &CancelToken::new(), |_| {});
    assert_eq!(report.stats.file_byte_limits, 1);
    assert_eq!(report.hits.len(), 1);
    assert!(report.hits[0].snippet.truncated);
    for (state, word, detail) in [
        (ContentSearchState::Partial, "partial", None),
        (
            ContentSearchState::Limited(ContentSearchLimit::Entries),
            "limited",
            Some("entries"),
        ),
        (
            ContentSearchState::InvalidQuery(ContentQueryError::TooLong),
            "invalid_query",
            Some("too_long"),
        ),
        (ContentSearchState::Cancelled, "cancelled", None),
    ] {
        let value = serde_json::to_value(state).unwrap();
        assert_eq!(value["state"], word);
        if let Some(detail) = detail {
            assert_eq!(value["detail"], detail);
        }
    }
    assert_eq!(
        serde_json::to_value(report.stats).unwrap()["scope_unverified"],
        true
    );
}

#[test]
fn overlapping_unicode_patterns_agree_with_simple_lower_substrings() {
    let alphabet = ["a", "A", "é", "É", "İ", "i", "\u{307}", "ß", "😀", "x"];
    for seed in 0..120 {
        let text = (0..32)
            .map(|i| alphabet[(seed * 7 + i * i + 3 * i) % alphabet.len()])
            .collect::<String>();
        for query in ["a", "aa", "é", "İ", "i\u{307}", "ß", "ss", "😀x", "aé"] {
            let lower = text
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            let query_lower = query
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            assert_eq!(
                matched(text.as_bytes(), query).lines,
                u32::from(lower.contains(&query_lower)),
                "{text:?} {query:?}"
            );
        }
    }
    assert_eq!(matched(b"aaaaab", "aab").lines, 1);
}

#[test]
fn exact_query_limit_is_accepted_and_byte_cut_unicode_is_not_invalid_data() {
    let query = "a".repeat(MAX_QUERY_BYTES);
    assert_eq!(matched(query.as_bytes(), &query).lines, 1);
    let mut data = "é\n".as_bytes().to_vec();
    data.resize(MAX_FILE_BYTES as usize - 1, b' ');
    data.extend_from_slice("😀".as_bytes());
    let result = matched(&data, "É");
    assert!(result.truncated);
    assert!(!result.ascii_fallback);
    assert_eq!((result.lines, result.first_line), (1, 1));
}

#[test]
fn production_hit_ceiling_applies_even_if_scope_requests_more() {
    let fixture = Fixture::new();
    for i in 0..=MAX_CONTENT_HITS {
        std::fs::write(fixture.0.join(format!("hit{i}")), b"n").unwrap();
    }
    let mut scope = fixture.scope();
    scope.max_results = MAX_CONTENT_HITS + 100;
    let report = grep_with_report(&scope, "n", &CancelToken::new(), |_| {});
    assert_eq!(
        report.state,
        ContentSearchState::Limited(ContentSearchLimit::Hits)
    );
    assert_eq!(report.hits.len(), MAX_CONTENT_HITS);
}
