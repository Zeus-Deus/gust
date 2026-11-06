use gust_core::entry::{Entry, EntryKind};
use gust_core::search::{
    filter_entries, fuzzy_matches, match_score, matching_indices, PreparedQuery,
};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

// Frozen pre-optimization implementations: the differential must not use the
// compatibility wrappers as its oracle, since those share the prepared matcher.
fn old_fuzzy(needle: &str, haystack: &OsStr) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle: Vec<_> = needle.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let mut i = 0;
    for b in haystack.as_bytes() {
        if b.to_ascii_lowercase() == needle[i] {
            i += 1;
            if i == needle.len() {
                return true;
            }
        }
    }
    false
}

fn old_score(needle: &str, haystack: &OsStr) -> Option<u32> {
    if needle.is_empty() {
        return Some(0);
    }
    if let Some(p) = haystack
        .as_bytes()
        .windows(needle.len())
        .position(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
    {
        Some(if p == 0 {
            0
        } else {
            1 + u32::try_from(p).unwrap_or(u32::MAX - 1)
        })
    } else {
        old_fuzzy(needle, haystack).then_some(1000)
    }
}

#[test]
fn prepared_and_compatibility_matchers_equal_the_old_byte_semantics() {
    let mut names: Vec<OsString> = [
        "",
        "Downloads",
        "my-report.TXT",
        "a-b-c",
        "Éclair",
        "éclair",
        "a\u{202e}b\nc\t",
        "a<U+202E>b<U+000A>c<U+0009>",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    names.push(OsString::from_vec(b"raw-\xffA\x80b.txt".to_vec()));
    names.push(OsString::from("a".repeat(4096)));
    // Deterministic arbitrary bytes cover invalid UTF-8 and repeated case
    // boundaries without a property-test dependency or external fixture.
    let mut state = 0x68b1_12f8_u64;
    for len in 0..128 {
        names.push(OsString::from_vec(
            (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect(),
        ));
    }
    let mut queries: Vec<String> = [
        "", "dwn", "DWN", "report", "TXT", "abc", "zzz", "raw", "ab", "É", "é", "�", "\u{202e}",
        "\n", "\t", "<U+202E>",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    queries.extend(["a".repeat(4096), "A".repeat(4097)]);
    queries.extend((0..128).map(|b| char::from(b).to_string()));
    for needle in queries {
        let query = PreparedQuery::new(&needle);
        for name in &names {
            let expected_match = old_fuzzy(&needle, name);
            let expected_score = old_score(&needle, name);
            assert_eq!(
                query.fuzzy_matches(name),
                expected_match,
                "{needle:?} {name:?}"
            );
            assert_eq!(
                fuzzy_matches(&needle, name),
                expected_match,
                "{needle:?} {name:?}"
            );
            assert_eq!(
                query.match_score(name),
                expected_score,
                "{needle:?} {name:?}"
            );
            assert_eq!(
                match_score(&needle, name),
                expected_score,
                "{needle:?} {name:?}"
            );
        }
    }
}

fn entry(name: &str) -> Entry {
    Entry::new(name.into(), EntryKind::File)
}

#[test]
fn direct_membership_preserves_listing_order_and_hidden_policy() {
    let mut entries = vec![
        entry("my-report.txt"),
        entry(".report"),
        entry("report.txt"),
        entry("r-e-p-o-r-t"),
        entry("elsewhere"),
    ];
    entries.push(Entry::new(
        OsString::from_vec(b"report-\xff".to_vec()),
        EntryKind::File,
    ));
    assert_eq!(matching_indices(&entries, "REPORT", false), [0, 2, 3, 5]);
    assert_eq!(matching_indices(&entries, "report", true), [0, 1, 2, 3, 5]);
    assert_eq!(matching_indices(&entries, "", false), [0, 2, 3, 4, 5]);
    assert_eq!(matching_indices(&entries, "", true), [0, 1, 2, 3, 4, 5]);
    assert!(matching_indices(&entries, "absent", true).is_empty());
    assert!(matching_indices(&[], "", true).is_empty());
    // Ranked Find/compatibility filtering retains its distinct score order.
    assert_eq!(filter_entries(&entries, "report")[0].name, "report.txt");
}

#[test]
fn membership_uses_unmodified_bytes_instead_of_display_labels_or_ids() {
    let entries = vec![
        entry("a\nb\u{202e}"),
        entry("a<U+000A>b<U+202E>"),
        Entry::new(OsString::from_vec(b"raw-\xff".to_vec()), EntryKind::File),
        entry("raw-�"),
    ];
    let original: Vec<_> = entries.iter().map(|e| (e.name.clone(), e.id)).collect();
    assert_eq!(matching_indices(&entries, "\n", true), [0]);
    assert_eq!(matching_indices(&entries, "\u{202e}", true), [0]);
    assert_eq!(matching_indices(&entries, "<U+000A>", true), [1]);
    assert_eq!(matching_indices(&entries, "�", true), [3]);
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.name.clone(), e.id))
            .collect::<Vec<_>>(),
        original
    );
}
