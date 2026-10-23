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
    s.set_query(SearchQuery::filename("file"));
    let next = finish(&mut s);
    assert_eq!(next.state, SearchState::Complete);
    assert_eq!(
        next.stats.issue_count, 0,
        "old query errors are not new omissions"
    );
    assert_eq!(next.stats.visited, 6);
    assert_eq!(next.hits.len(), 6);
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

fn set_red(path: &std::path::Path) {
    rustix::fs::lsetxattr(
        path,
        gust_core::tags::ATTR,
        b"Red",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
}

#[test]
fn swapped_ancestor_between_advances_cannot_supply_outside_tags() {
    let root = Fixture::new("ancestor-live");
    let outside = Fixture::new("ancestor-live-outside");
    fs::create_dir(root.0.join("branch")).unwrap();
    fs::write(root.0.join("branch/subject"), b"inside").unwrap();
    fs::write(outside.0.join("subject"), b"outside").unwrap();
    set_red(&outside.0.join("subject"));
    let mut session = SearchSession::new(root.scope(), SearchOptions::default());
    session.set_query(SearchQuery::tagged("Red").unwrap());
    assert_eq!(
        session.advance(1, &CancelToken::new(), |_| {}).state,
        SearchState::Running
    );
    fs::rename(root.0.join("branch"), outside.0.join("original")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("branch")).unwrap();
    let report = finish(&mut session);
    assert!(
        report.hits.is_empty(),
        "outside xattrs must not satisfy an inside query"
    );
    assert_eq!(report.state, SearchState::Partial);
}

#[test]
fn cached_replay_rejects_an_ancestor_replaced_with_a_symlink() {
    let root = Fixture::new("ancestor-replay");
    let outside = Fixture::new("ancestor-replay-outside");
    fs::create_dir(root.0.join("branch")).unwrap();
    fs::write(root.0.join("branch/subject"), b"inside").unwrap();
    fs::write(outside.0.join("subject"), b"outside").unwrap();
    set_red(&root.0.join("branch/subject"));
    set_red(&outside.0.join("subject"));
    let mut session = SearchSession::new(root.scope(), SearchOptions::default());
    session.set_query(SearchQuery::tagged("Red").unwrap());
    let before = finish(&mut session);
    assert_eq!(before.hits.len(), 1);
    fs::rename(root.0.join("branch"), outside.0.join("original")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("branch")).unwrap();
    session.refresh();
    let after = finish(&mut session);
    assert!(
        after.hits.is_empty(),
        "replay must not read the replacement symlink target"
    );
    assert_eq!(after.state, SearchState::Partial);
    assert_eq!(after.stats.visited, before.stats.visited);
}

#[test]
fn outside_cache_marker_cannot_prune_a_held_original_directory() {
    let root = Fixture::new("ancestor-marker");
    let outside = Fixture::new("ancestor-marker-outside");
    fs::create_dir_all(root.0.join("branch/nested")).unwrap();
    fs::write(root.0.join("branch/nested/subject"), b"inside").unwrap();
    fs::create_dir(outside.0.join("nested")).unwrap();
    fs::write(
        outside.0.join("nested/CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    let mut session = SearchSession::new(root.scope(), SearchOptions::default());
    session.set_query(SearchQuery::filename("subject"));
    session.advance(1, &CancelToken::new(), |_| {});
    fs::rename(root.0.join("branch"), outside.0.join("original")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("branch")).unwrap();
    let report = finish(&mut session);
    assert_eq!(report.state, SearchState::Partial);
    assert_eq!(
        report.stats.generated_pruned, 0,
        "an outside marker has no authority"
    );
    assert!(report.hits.is_empty());
}

#[test]
fn cached_parent_identity_rejects_a_real_replacement_and_changed_root() {
    let root = Fixture::new("parent-identity");
    let outside = Fixture::new("parent-identity-outside");
    fs::create_dir_all(root.0.join("scope/branch")).unwrap();
    fs::write(root.0.join("scope/branch/subject"), b"inside").unwrap();
    let scope = SearchScope::recursive(LocationKey::local(root.0.join("scope")), 8);
    let mut session = SearchSession::new(scope, SearchOptions::default());
    session.set_query(SearchQuery::filename("subject"));
    assert_eq!(finish(&mut session).hits.len(), 1);
    fs::rename(root.0.join("scope/branch"), outside.0.join("original")).unwrap();
    fs::create_dir(root.0.join("scope/branch")).unwrap();
    fs::write(root.0.join("scope/branch/subject"), b"replacement").unwrap();
    session.refresh();
    let report = finish(&mut session);
    assert!(report.hits.is_empty());
    assert_eq!(report.state, SearchState::Partial);
    session.invalidate();
    assert_eq!(
        finish(&mut session).hits.len(),
        1,
        "explicit invalidation admits the new tree"
    );
    fs::rename(root.0.join("scope"), outside.0.join("old-scope")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("scope")).unwrap();
    session.refresh();
    let report = finish(&mut session);
    assert!(report.hits.is_empty());
    assert_eq!(report.state, SearchState::Partial);
}

#[test]
fn final_symlinks_and_fifos_are_named_without_reading_their_targets() {
    let root = Fixture::new("link-special");
    let outside = Fixture::new("link-special-outside");
    fs::write(outside.0.join("subject"), b"outside").unwrap();
    set_red(&outside.0.join("subject"));
    std::os::unix::fs::symlink(outside.0.join("subject"), root.0.join("subject-link")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        root.0.join("subject-pipe"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR,
        0,
    )
    .unwrap();
    let mut session = SearchSession::new(root.scope(), SearchOptions::default());
    session.set_query(SearchQuery::filename("subject"));
    let report = finish(&mut session);
    assert_eq!(report.state, SearchState::Complete);
    assert_eq!(report.hits.len(), 2);
    session.set_query(SearchQuery::tagged("Red").unwrap());
    let report = finish(&mut session);
    assert_eq!(report.state, SearchState::Complete);
    assert!(report.hits.is_empty());
}

#[test]
fn projected_rows_use_captured_metadata_after_the_path_is_redirected() {
    let root = Fixture::new("safe-projection");
    let outside = Fixture::new("safe-projection-outside");
    fs::create_dir(root.0.join("branch")).unwrap();
    fs::write(root.0.join("branch/subject"), b"inside").unwrap();
    fs::create_dir(outside.0.join("subject")).unwrap();
    set_red(&root.0.join("branch/subject"));
    let original = fs::symlink_metadata(root.0.join("branch/subject")).unwrap();
    let mut session = SearchSession::new(root.scope(), SearchOptions::default());
    session.set_query(SearchQuery::filename("subject"));
    let report = finish(&mut session);
    assert_eq!(report.hits.len(), 1);
    fs::rename(root.0.join("branch"), outside.0.join("original")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("branch")).unwrap();
    let projection = session.project_hits(&CancelToken::new());
    assert_eq!(projection.entries.len(), 1);
    let entry = &projection.entries[0];
    assert_eq!(entry.relative_path, PathBuf::from("branch/subject"));
    assert_eq!(entry.kind, gust_core::EntryKind::File);
    assert_eq!(entry.metadata.size, 6);
    assert_eq!(entry.metadata.tags, 1);
    assert_eq!(entry.metadata.link_ok, None);
    assert_eq!(entry.source.generation, report.generation);
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        (entry.source.dev, entry.source.ino),
        (original.dev(), original.ino())
    );
    assert_eq!(entry.source.mtime_ns, original.mtime_nsec());
    session.refresh();
    finish(&mut session);
    assert!(session.project_hits(&CancelToken::new()).entries.is_empty());
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        session.project_hits(&cancel).report.state,
        SearchState::Cancelled
    );
}

#[test]
fn projected_tag_errors_are_explicit_without_losing_filename_hits() {
    let root = Fixture::new("projection-tags");
    let path = root.0.join("subject");
    fs::write(&path, b"").unwrap();
    rustix::fs::lsetxattr(
        &path,
        gust_core::tags::ATTR,
        b"bad\xff",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    let mut session = SearchSession::new(root.scope(), SearchOptions::default());
    session.set_query(SearchQuery::filename("subject"));
    assert_eq!(finish(&mut session).state, SearchState::Complete);
    let projection = session.project_hits(&CancelToken::new());
    assert_eq!(projection.entries.len(), 1);
    assert_eq!(
        projection.entries[0].tag_error,
        Some(std::io::ErrorKind::InvalidData)
    );
    assert_eq!(projection.report.state, SearchState::Partial);
    assert_eq!(projection.report.stats.issue_count, 1);
    assert_eq!(
        session
            .project_hits(&CancelToken::new())
            .report
            .stats
            .issue_count,
        1,
        "projection does not accumulate errors or perform a new read"
    );
}

#[test]
fn unreadable_named_generated_directories_are_pruned_without_permission_issues() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().as_raw() == 0 {
        eprintln!("skipped: root bypasses the directory permissions under test");
        return;
    }
    for name in ["node_modules", "__pycache__"] {
        let fixture = Fixture::new(name);
        let generated = fixture.0.join(name);
        fs::create_dir(&generated).unwrap();
        fs::write(generated.join("unrelated-hit-inside"), b"hidden by policy").unwrap();
        fs::write(fixture.0.join("unrelated-hit"), b"visible").unwrap();
        fs::set_permissions(&generated, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = fs::read_dir(&generated).unwrap_err();
        let mut session = SearchSession::new(fixture.scope(), SearchOptions::default());
        session.set_query(SearchQuery::filename("unrelated-hit"));
        let report = finish(&mut session);
        // Restore traversal before assertions so fixture cleanup also works on failure.
        fs::set_permissions(&generated, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(report.state, SearchState::Complete, "{name}: {report:?}");
        assert_eq!(report.stats.generated_pruned, 1, "{name}");
        assert!(report.issues.is_empty(), "{name}: {:?}", report.issues);
        assert_eq!(report.stats.issue_count, 0);
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].path, fixture.0.join("unrelated-hit"));
    }
}
