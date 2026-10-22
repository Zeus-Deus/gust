use gust_core::search::*;
use gust_core::{CancelToken, LocationKey};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("gust-session-{}-{test}", std::process::id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn scope(&self) -> SearchScope {
        SearchScope::recursive(LocationKey::local(&self.0), 8)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn finish(s: &mut SearchSession) -> SearchReport {
    for _ in 0..1000 {
        let r = s.advance(3, &CancelToken::new(), |_| {});
        if r.state != SearchState::Running {
            return r;
        }
    }
    panic!("small fixture never finished")
}
#[test]
fn streams_before_completion_and_reuses_candidates_for_new_queries() {
    let f = Fixture::new("reuse");
    for name in ["alpha", "beta", "alphabet", "gamma"] {
        fs::write(f.0.join(name), b"").unwrap();
    }
    let mut s = SearchSession::new(f.scope(), SearchOptions::default());
    s.set_query(SearchQuery::filename("a"));
    let mut streamed = 0;
    let early = s.advance(1, &CancelToken::new(), |_| streamed += 1);
    assert_eq!(early.state, SearchState::Running);
    assert_eq!(streamed, 1);
    let first = finish(&mut s);
    assert_eq!(first.state, SearchState::Complete);
    assert_eq!(first.hits.len(), 4);
    s.set_query(SearchQuery::filename("alpha"));
    let next = finish(&mut s);
    assert_eq!(next.hits.len(), 2);
    assert_eq!(next.stats.visited, first.stats.visited, "no second walk");
    assert!(next.generation > first.generation);
    fs::remove_file(f.0.join("alpha")).unwrap();
    s.refresh();
    let changed = finish(&mut s);
    assert_eq!(changed.hits.len(), 1);
    assert_eq!(changed.state, SearchState::Partial);
    assert_eq!(changed.issues[0].kind, std::io::ErrorKind::NotFound);
}
#[test]
fn result_limit_is_explicit_and_narrowing_resumes_the_walk() {
    let f = Fixture::new("results");
    for i in 0..8 {
        fs::write(f.0.join(format!("item-{i}")), b"").unwrap();
    }
    let mut scope = f.scope();
    scope.max_results = 2;
    let mut s = SearchSession::new(scope, SearchOptions::default());
    let r = finish(&mut s);
    assert_eq!(r.state, SearchState::Limited(SearchLimit::Results));
    assert_eq!(r.hits.len(), 2);
    s.set_query(SearchQuery::filename("item-7"));
    let r = finish(&mut s);
    assert_eq!(r.state, SearchState::Complete);
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.stats.visited, 8);
}
#[test]
fn exact_result_limit_is_not_false_truncation() {
    let f = Fixture::new("exact");
    fs::write(f.0.join("a"), b"").unwrap();
    let mut scope = f.scope();
    scope.max_results = 1;
    let mut s = SearchSession::new(scope, SearchOptions::default());
    assert_eq!(finish(&mut s).state, SearchState::Complete);
}
#[test]
fn all_resource_limits_are_reported_independently() {
    let f = Fixture::new("bounds");
    fs::write(f.0.join("match"), b"").unwrap();
    for (options, expected) in [
        (
            SearchOptions {
                max_visited: 0,
                ..Default::default()
            },
            SearchLimit::VisitedEntries,
        ),
        (
            SearchOptions {
                max_candidates: 0,
                ..Default::default()
            },
            SearchLimit::CachedCandidates,
        ),
        (
            SearchOptions {
                max_path_bytes: 0,
                ..Default::default()
            },
            SearchLimit::PathBytes,
        ),
    ] {
        let mut s = SearchSession::new(f.scope(), options);
        s.set_query(SearchQuery::filename("no-matches"));
        assert_eq!(finish(&mut s).state, SearchState::Limited(expected));
    }
}
#[test]
fn hidden_depth_scope_symlink_and_generated_exclusions_are_explicit() {
    let f = Fixture::new("scope");
    fs::create_dir_all(f.0.join("visible/deeper")).unwrap();
    fs::create_dir_all(f.0.join(".hidden")).unwrap();
    fs::create_dir_all(f.0.join("node_modules")).unwrap();
    fs::write(f.0.join("visible/match"), b"").unwrap();
    fs::write(f.0.join(".hidden/match"), b"").unwrap();
    fs::write(f.0.join("node_modules/match"), b"").unwrap();
    std::os::unix::fs::symlink(&f.0, f.0.join("loop")).unwrap();
    let mut scope = f.scope();
    scope.max_depth = 2;
    let mut s = SearchSession::new(scope.clone(), SearchOptions::default());
    s.set_query(SearchQuery::filename("match"));
    let r = finish(&mut s);
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.state, SearchState::Limited(SearchLimit::Depth));
    assert_eq!(r.stats.hidden, 1);
    assert_eq!(r.stats.generated_pruned, 1);
    assert_eq!(r.stats.depth_pruned, 1);
    scope.include_hidden = true;
    let mut s = SearchSession::new(
        scope,
        SearchOptions {
            skip_generated: false,
            ..Default::default()
        },
    );
    s.set_query(SearchQuery::filename("match"));
    assert_eq!(finish(&mut s).hits.len(), 3);
    let mut scope = f.scope();
    scope.max_depth = 0;
    let mut s = SearchSession::new(scope, SearchOptions::default());
    let r = finish(&mut s);
    assert!(r.hits.is_empty());
    assert_eq!(r.state, SearchState::Limited(SearchLimit::Depth));
}
#[test]
fn cancellation_and_bad_roots_do_not_pretend_completion() {
    let f = Fixture::new("cancel");
    fs::write(f.0.join("a"), b"").unwrap();
    let mut s = SearchSession::new(f.scope(), SearchOptions::default());
    let cancel = CancelToken::new();
    cancel.cancel();
    let r = s.advance(100, &cancel, |_| panic!("cancelled callback"));
    assert_eq!(r.state, SearchState::Cancelled);
    assert_eq!(r.stats.visited, 0);
    s.refresh();
    assert_eq!(finish(&mut s).hits.len(), 1);
    let mut s = SearchSession::new(
        SearchScope::current_dir(LocationKey::local(f.0.join("gone"))),
        SearchOptions::default(),
    );
    assert!(matches!(finish(&mut s).state, SearchState::Failed(_)));
    let root = LocationKey::new(gust_core::ProviderId::Sftp, "/remote".into());
    let mut s = SearchSession::new(SearchScope::recursive(root, 3), SearchOptions::default());
    assert!(matches!(finish(&mut s).state, SearchState::Failed(_)));
}
#[test]
fn tag_queries_keep_seven_colours_and_unknown_words_and_refresh_external_changes() {
    let f = Fixture::new("tags");
    let p = f.0.join("report.txt");
    fs::write(&p, b"").unwrap();
    let initial = b"red,Orange,Yellow,Green,Blue,Purple,Gray,work item,Custom";
    rustix::fs::lsetxattr(
        &p,
        gust_core::tags::ATTR,
        initial,
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    let mut s = SearchSession::new(f.scope(), SearchOptions::default());
    s.set_query(SearchQuery::parse("report tag:RED tag:\"work item\" tag:Custom").unwrap());
    assert_eq!(finish(&mut s).hits.len(), 1);
    for colour in gust_core::tags::NAMES {
        s.set_query(SearchQuery::parse(&format!("tag:{colour}")).unwrap());
        assert_eq!(finish(&mut s).hits.len(), 1);
    }
    rustix::fs::lsetxattr(
        &p,
        gust_core::tags::ATTR,
        b"Custom",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    s.refresh();
    assert!(finish(&mut s).hits.is_empty());
    s.set_query(SearchQuery::parse("tag:Custom").unwrap());
    assert_eq!(finish(&mut s).hits.len(), 1);
    s.set_query(SearchQuery::parse("tag:custom").unwrap());
    assert!(
        finish(&mut s).hits.is_empty(),
        "word tags retain case identity"
    );
    assert_eq!(gust_core::tags::read_raw(&p).unwrap().unwrap(), b"Custom");
    for query in ["tag:", "tag:\"\"", "tag:a,b", "tag:\"unclosed"] {
        assert!(SearchQuery::parse(query).is_err());
    }
}

#[test]
fn a_directory_replaced_by_a_link_during_streaming_cannot_redirect_descent() {
    let f = Fixture::new("swapped");
    let outside = Fixture::new("outside");
    fs::create_dir(f.0.join("child")).unwrap();
    fs::write(outside.0.join("outside-only"), b"").unwrap();
    let mut s = SearchSession::new(f.scope(), SearchOptions::default());
    let r = s.advance(100, &CancelToken::new(), |hit| {
        if hit.path == f.0.join("child") {
            fs::rename(f.0.join("child"), f.0.join("retained")).unwrap();
            std::os::unix::fs::symlink(&outside.0, f.0.join("child")).unwrap();
        }
    });
    assert!(!r.hits.iter().any(|hit| hit.path.ends_with("outside-only")));
    assert_eq!(r.state, SearchState::Partial);
}

#[test]
fn invalid_tag_bytes_are_reported_and_issue_details_are_bounded() {
    let f = Fixture::new("tag-errors");
    for i in 0..6 {
        let p = f.0.join(format!("file-{i}"));
        fs::write(&p, b"").unwrap();
        rustix::fs::lsetxattr(
            &p,
            gust_core::tags::ATTR,
            b"bad\xff",
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
    }
    let mut s = SearchSession::new(
        f.scope(),
        SearchOptions {
            max_issues: 2,
            ..Default::default()
        },
    );
    s.set_query(SearchQuery::tagged("bad\u{fffd}").unwrap());
    let r = finish(&mut s);
    assert_eq!(r.state, SearchState::Partial);
    assert_eq!(r.stats.issue_count, 6);
    assert_eq!(r.issues.len(), 2);
    assert!(r.hits.is_empty());
}

#[test]
fn bogus_cache_marker_does_not_hide_user_files_and_invalid_names_stay_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new("marker");
    fs::create_dir(f.0.join("data")).unwrap();
    fs::write(f.0.join("data/CACHEDIR.TAG"), b"ordinary notes").unwrap();
    let raw = std::ffi::OsString::from_vec(b"match\xff".to_vec());
    fs::write(f.0.join("data").join(&raw), b"").unwrap();
    let report = find_with_report(
        &f.scope(),
        SearchQuery::filename("match"),
        SearchOptions::default(),
        &CancelToken::new(),
        |_| {},
    );
    assert_eq!(report.state, SearchState::Complete);
    assert_eq!(report.hits[0].path.file_name().unwrap(), raw);
    assert!(!gust_core::search::fuzzy_matches("\u{fffd}", &raw));
    assert!(gust_core::search::match_score("\u{fffd}", &raw).is_none());
}
