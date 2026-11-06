use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

fn entries(names: &[OsString], key: &str, listing: u64) -> Arc<Entries> {
    let mut snapshot = DirectorySnapshot::empty(LocationKey::local(key), 1);
    snapshot.complete = Completeness::Complete;
    snapshot.settled = false;
    snapshot.entries = names
        .iter()
        .map(|name| Entry::new(name.clone(), gust_core::entry::EntryKind::File))
        .collect();
    let entries = Arc::new(Entries::new(snapshot, listing));
    let mut retired = Retirement::default();
    entries.ready(&mut retired, &|| false).unwrap()
}
fn frame(count: usize) -> Arc<Frame> {
    let names: Vec<_> = (0..count)
        .map(|i| OsString::from(format!("file{i:07}")))
        .collect();
    let entries = entries(&names, "/old", 1);
    let base = Frame::empty(LocationKey::local("/old"));
    let mut request = request(1, "");
    request.generation = 1;
    let mut retired = Retirement::default();
    Arc::new(
        prepare(&base, entries, &request, None, None, &mut retired, &|| {
            false
        })
        .unwrap(),
    )
}
fn request(revision: u64, query: &str) -> Request {
    Request {
        generation: 1,
        request: revision,
        selection_revision: 0,
        query: query.into(),
        sort: SortSpec::default(),
        sort_revision: 0,
        hidden: false,
        cursor: 0,
        focus: None,
        focus_revision: 0,
        keep: 1,
        submitted: Instant::now(),
        clipboard: Arc::default(),
        marks_revision: 0,
    }
}
fn receive(runtime: &Runtime) -> Arc<Frame> {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(frame) = runtime.take(false) {
            return frame;
        }
        assert!(
            Instant::now() < until,
            "projection did not finish: {}",
            runtime.slots()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn install(runtime: &Runtime, current: &mut Arc<Frame>, next: Arc<Frame>) {
    let old = std::mem::replace(current, next.clone());
    runtime.installed(next, old);
}
fn wait_retired(runtime: &Runtime) {
    let until = Instant::now() + Duration::from_secs(10);
    while runtime.slots()["retiring"] == true {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn requested_navigation_never_changes_displayed_bytes_before_publication() {
    use std::os::unix::ffi::OsStringExt;
    let names = [
        OsString::from_vec(vec![0xff, b'\n']),
        OsString::from("<U+000A>"),
    ];
    let base = Frame::empty(LocationKey::local("/old"));
    let mut retired = Retirement::default();
    let old = Arc::new(
        prepare(
            &base,
            entries(&names, "/old", 1),
            &request(1, ""),
            None,
            None,
            &mut retired,
            &|| false,
        )
        .unwrap(),
    );
    let runtime = Runtime::new(old.clone());
    let mut next_request = request(2, "");
    next_request.generation = 2;
    {
        let mut state = runtime.pane.state.lock().unwrap();
        state.desired = next_request.clone();
        let mut snapshot = DirectorySnapshot::empty(LocationKey::local("/new"), 2);
        snapshot.complete = Completeness::Complete;
        snapshot.entries = vec![Entry::new(
            "other".into(),
            gust_core::entry::EntryKind::File,
        )];
        state.candidate = Some(Arc::new(Entries::new(snapshot, 2)));
    }
    runtime.pane.schedule();
    assert_eq!(old.all.key, LocationKey::local("/old"));
    assert_eq!(old.all[old.visible[0]].name, names[1]);
    let until = Instant::now() + Duration::from_secs(10);
    while runtime.slots()["completion"] != true {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(runtime.take(true).is_none());
    assert_eq!(old.all.key, LocationKey::local("/old"));
    let new = receive(&runtime);
    assert_eq!(new.all.key, LocationKey::local("/new"));
    assert_eq!(new.all[0].name, "other");
    runtime.installed(new, old);
}
#[test]
fn stale_and_out_of_order_completions_fail_every_revision_component() {
    let base = frame(20);
    let runtime = Runtime::new(base.clone());
    let mut retired = Retirement::default();
    let candidate = prepare(
        &base,
        base.all.clone(),
        &request(4, "1"),
        None,
        None,
        &mut retired,
        &|| false,
    )
    .unwrap();
    let mut state = runtime.pane.state.lock().unwrap();
    state.desired = request(4, "1");
    assert!(state.valid(&candidate));
    state.desired.request += 1;
    assert!(!state.valid(&candidate));
    state.desired.request -= 1;
    state.desired.generation += 1;
    assert!(!state.valid(&candidate));
    state.desired.generation -= 1;
    state.desired.selection_revision += 1;
    assert!(!state.valid(&candidate));
    state.desired.selection_revision -= 1;
    let mut candidate = candidate;
    candidate.source_display_revision += 1;
    assert!(!state.valid(&candidate));
}
#[test]
fn rapid_replacement_is_one_latest_intent_and_one_completion() {
    let mut current = frame(10_000);
    let runtime = Runtime::new(current.clone());
    for revision in 2..202 {
        runtime.request(request(revision, &format!("{}", revision % 97)));
        let slots = runtime.slots();
        assert!(slots["selection_intents"].as_u64().unwrap() <= 256);
    }
    let next = receive(&runtime);
    assert_eq!(next.request, 201);
    assert_eq!(next.query, format!("{}", 201 % 97));
    assert_eq!(next.all.listing, current.all.listing);
    install(&runtime, &mut current, next);
    wait_retired(&runtime);
    assert_eq!(runtime.slots()["completion"], false);
}
#[test]
fn accepted_selection_events_keep_order_and_raw_targets_during_projection() {
    use std::os::unix::ffi::OsStringExt;
    let names = [
        OsString::from_vec(vec![0xff]),
        "file1".into(),
        "file2".into(),
    ];
    let base = Frame::empty(LocationKey::local("/old"));
    let mut retirement = Retirement::default();
    let mut current = Arc::new(
        prepare(
            &base,
            entries(&names, "/old", 1),
            &request(1, ""),
            None,
            None,
            &mut retirement,
            &|| false,
        )
        .unwrap(),
    );
    let runtime = Runtime::new(current.clone());
    runtime.request(request(2, "file"));
    for (revision, gesture) in [
        (1, Gesture::Toggle(names[0].clone())),
        (2, Gesture::Toggle(names[0].clone())),
        (3, Gesture::Only(names[0].clone())),
    ] {
        let mut r = request(2, "file");
        r.selection_revision = revision;
        runtime.gesture(gesture, r).unwrap();
    }
    for expected in 1..=3 {
        let next = receive(&runtime);
        assert!(!next.row_change);
        assert_eq!(next.selection_revision, expected);
        assert_eq!(next.all.key, LocationKey::local("/old"));
        install(&runtime, &mut current, next);
    }
    assert!(current.selection.contains(&names[0]));
    assert_eq!(current.selection.len(), 1);
    let filtered = receive(&runtime);
    assert!(filtered.row_change);
    assert_eq!(filtered.selection_revision, 3);
    assert_eq!(filtered.query, "file");
    assert!(filtered.selection.is_empty());
    install(&runtime, &mut current, filtered);
    assert!(!runtime.pending_selection());
}
#[test]
fn selection_capacity_refuses_without_losing_events() {
    let runtime = Runtime::new(frame(3));
    // Reserve the executor slot deterministically while exercising admission.
    runtime.pane.state.lock().unwrap().cpu_running = true;
    for revision in 1..=MAX_GESTURES {
        let mut r = request(2, "");
        r.selection_revision = revision as u64;
        runtime
            .gesture(Gesture::Toggle("file0000001".into()), r)
            .unwrap();
    }
    let mut r = request(2, "");
    r.selection_revision = 257;
    assert!(runtime.gesture(Gesture::Clear, r).is_err());
    assert_eq!(runtime.slots()["selection_intents"], 256);
    let mut state = runtime.pane.state.lock().unwrap();
    assert_eq!(state.gestures.back().unwrap().revision, 256);
    state.cpu_running = false;
}
#[test]
fn metadata_sidecar_survives_reorder_and_refuses_wrong_raw_name() {
    let base = frame(100);
    let backing = base.all.clone();
    let mut r = request(2, "");
    r.sort.ascending = false;
    let mut retirement = Retirement::default();
    let sorted = prepare(
        &base,
        backing.clone(),
        &r,
        None,
        None,
        &mut retirement,
        &|| false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&sorted.all, &backing));
    let metadata = Metadata {
        size: 91,
        ..Metadata::default()
    };
    assert!(!backing.patch(7, std::ffi::OsStr::new("wrong"), Some(metadata)));
    assert!(backing.patch(7, &backing[7].name, Some(metadata)));
    assert_eq!(sorted.all[7].meta, Some(metadata));
    assert_eq!(sorted.row_of[7], 92);
    assert_eq!(sorted.all.meta_known.load(Ordering::Relaxed), 1);
    assert!(!backing.patch(
        7,
        &backing[7].name,
        Some(Metadata {
            size: 12,
            ..metadata
        })
    ));
    let relisted = entries(&[backing[7].name.clone()], "/old", 2);
    assert!(relisted[0].meta.is_none());
}
#[test]
fn cancellable_selection_hands_partial_storage_to_retirement() {
    let base = frame(100_000);
    let mut all = Selection::new();
    all.select_all_by_name(
        &(0..100_000)
            .map(|i| OsString::from(format!("file{i:07}")))
            .collect::<Vec<_>>(),
    );
    let mut selected = Frame {
        selection: Arc::new(all),
        ..clone_frame_for_test(&base)
    };
    selected.display_revision = 2;
    let checkpoints = AtomicUsize::new(0);
    let cancelled = || checkpoints.fetch_add(1, Ordering::Relaxed) > 2200;
    let mut retirement = Retirement::default();
    let result = prepare(
        &selected,
        selected.all.clone(),
        &request(2, ""),
        None,
        None,
        &mut retirement,
        &cancelled,
    );
    assert!(matches!(
        result,
        Err(projection::ProjectionError::Cancelled)
    ));
    assert!(!retirement.is_empty());
    // The ownership recipient can hold the partial allocation after return.
    assert!(retirement.scratch.tracked_names() > 0 || retirement.selection.is_some());
}
fn clone_frame_for_test(frame: &Frame) -> Frame {
    Frame {
        all: frame.all.clone(),
        thumbs: frame.thumbs.clone(),
        dir_sizes: frame.dir_sizes.clone(),
        row_of: frame.row_of.clone(),
        clipboard: frame.clipboard.clone(),
        marked: frame.marked,
        jump_paths: frame.jump_paths.clone(),
        order: frame.order.clone(),
        visible: frame.visible.clone(),
        selection: frame.selection.clone(),
        request: frame.request,
        selection_revision: frame.selection_revision,
        display_revision: frame.display_revision,
        source_display_revision: frame.source_display_revision,
        query: frame.query.clone(),
        sort: frame.sort,
        sort_revision: frame.sort_revision,
        hidden: frame.hidden,
        cursor: frame.cursor,
        focus_revision: frame.focus_revision,
        summary: frame.summary.clone(),
        mapping: frame.mapping.clone(),
        keep: frame.keep,
        row_change: frame.row_change,
        publication: frame.publication,
        marks_revision: frame.marks_revision,
        validated_thumbnail_count: frame.validated_thumbnail_count,
    }
}
#[test]
fn independent_panes_receive_fair_bounded_cpu_admission() {
    let runtimes: Vec<_> = (0..8).map(|_| Runtime::new(frame(2_000))).collect();
    for runtime in &runtimes {
        runtime.request(request(2, "5"));
    }
    for runtime in &runtimes {
        let next = receive(runtime);
        assert_eq!(next.query, "5");
        let old = runtime.pane.state.lock().unwrap().base.clone();
        runtime.installed(next, old);
    }
}

#[derive(Default)]
struct Gate {
    entered: AtomicBool,
    open: Mutex<bool>,
    ready: Condvar,
}
impl Gate {
    fn block(&self) {
        self.entered.store(true, Ordering::Release);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.ready.wait(open).unwrap();
        }
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.ready.notify_all();
    }
    fn wait_entered(&self) {
        let until = Instant::now() + Duration::from_secs(30);
        while !self.entered.load(Ordering::Acquire) {
            assert!(Instant::now() < until, "worker did not enter gate");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
#[test]
fn million_name_cancellation_has_one_retirement_slot_and_measured_newest_start() {
    let base = frame(1_000_000);
    let mut all = Selection::new();
    all.select_all_by_name(
        &(0..1_000_000)
            .map(|i| OsString::from(format!("file{i:07}")))
            .collect::<Vec<_>>(),
    );
    let selected = Arc::new(Frame {
        selection: Arc::new(all),
        ..clone_frame_for_test(&base)
    });
    drop(base);
    let runtime = Runtime::new(selected.clone());
    let checkpoint = Arc::new(Gate::default());
    let cleanup = Arc::new(Gate::default());
    let gate = checkpoint.clone();
    *runtime.pane.hooks.checkpoint.lock().unwrap() = Some(Arc::new(move |n| {
        if n == 22500 {
            gate.block()
        }
    }));
    let gate = cleanup.clone();
    *runtime.pane.hooks.retire.lock().unwrap() = Some(Arc::new(move || gate.block()));
    runtime.request(request(2, ""));
    checkpoint.wait_entered();
    runtime.request(request(3, "absent"));
    checkpoint.release();
    cleanup.wait_entered();
    assert_eq!(runtime.slots()["retiring"], true);
    assert_eq!(runtime.slots()["running"], false);
    for revision in 4..54 {
        runtime.request(request(revision, "absent"));
    }
    let started = runtime.metrics().started.load(Ordering::Relaxed);
    assert_eq!(started, 1);
    assert!(
        runtime
            .metrics()
            .retired_selection_names_max
            .load(Ordering::Relaxed)
            > 0
    );
    // Newest light input waits visibly for the sole recipient. No new CPU
    // clone or second retirement envelope can start while it is occupied.
    assert!(runtime.take(false).is_none());
    cleanup.release();
    let next = receive(&runtime);
    assert_eq!(next.request, 53);
    assert!(next.visible.is_empty());
    assert!(runtime.metrics().cancel_observed.load(Ordering::Relaxed) >= 1);
    assert!(
        runtime
            .metrics()
            .latest_start_us_max
            .load(Ordering::Relaxed)
            > 0
    );
    eprintln!("million-selection-scheduler {}", runtime.metrics().json());
    runtime.installed(next, selected);
}
#[test]
fn object_teardown_final_backing_drop_is_on_retirement_worker() {
    let current = frame(100_000);
    let notice = Arc::new(Mutex::new(Vec::new()));
    *current.all.drop_notice.lock().unwrap() = Some(notice.clone());
    let weak = Arc::downgrade(&current.all);
    let runtime = Runtime::new(current.clone());
    drop(current);
    drop(runtime);
    let until = Instant::now() + Duration::from_secs(15);
    while weak.upgrade().is_some() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(notice.lock().unwrap().as_slice(), ["gust-retire-0"]);
}

#[test]
fn continuous_filter_replacement_does_not_starve_exact_metadata_slots() {
    let mut current = frame(2_000);
    let runtime = Runtime::new(current.clone());
    for revision in 2..202 {
        runtime.request(request(revision, "file"));
        let index = revision as usize;
        let entry = &current.all[index];
        assert!(current.all.patch(
            index,
            &entry.name,
            Some(Metadata {
                size: revision,
                ..Metadata::default()
            })
        ));
    }
    let next = receive(&runtime);
    assert_eq!(next.request, 201);
    assert!(Arc::ptr_eq(&current.all, &next.all));
    assert_eq!(next.all.meta_known.load(Ordering::Relaxed), 200);
    assert_eq!(next.all[201].meta.unwrap().size, 201);
    install(&runtime, &mut current, next);
}
#[test]
fn an_old_risky_reader_cannot_occupy_fresh_local_admission_for_the_same_pane() {
    let path = std::env::temp_dir().join(format!(
        "gust-projection-local-admission-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("fresh"), b"x").unwrap();
    let base = frame(5);
    let runtime = Runtime::new(base.clone());
    runtime.pane.state.lock().unwrap().risky_running = true; // controlled stalled backend
    let mut next = request(2, "");
    next.generation = 2;
    runtime.request(next);
    runtime.load(Load {
        key: LocationKey::local(&path),
        generation: 2,
        sort: SortSpec::default(),
        reuse: false,
        live: false,
        thumbnail_names: Vec::new(),
    });
    let next = receive(&runtime);
    assert_eq!(next.all.key, LocationKey::local(&path));
    assert!(!next.all.risky);
    assert_eq!(next.all[next.visible[0]].name, "fresh");
    runtime.pane.state.lock().unwrap().risky_running = false;
    runtime.installed(next, base);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn worker_failure_is_visible_and_a_later_request_can_retry() {
    let base = frame(50);
    let runtime = Runtime::new(base.clone());
    let first = AtomicBool::new(true);
    *runtime.pane.hooks.checkpoint.lock().unwrap() = Some(Arc::new(move |_| {
        if first.swap(false, Ordering::AcqRel) {
            panic!("controlled backend failure")
        }
    }));
    runtime.request(request(2, "x"));
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(error) = runtime.take_error() {
            assert!(error.contains("stopped unexpectedly"));
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    *runtime
        .pane
        .hooks
        .checkpoint
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    runtime.request(request(3, "file"));
    let next = receive(&runtime);
    assert_eq!(next.request, 3);
    assert_eq!(next.visible.len(), 50);
    runtime.installed(next, base);
}

#[test]
fn ranked_find_keeps_hit_order_and_clears_prior_directory_selection() {
    let root = std::env::temp_dir().join(format!("gust-projection-find-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let z = root.join("z");
    let a = root.join("a");
    std::fs::write(&z, b"z").unwrap();
    std::fs::write(&a, b"a").unwrap();
    let key = root.to_str().unwrap();
    let empty = Frame::empty(LocationKey::local(&root));
    let mut retirement = Retirement::default();
    let original = prepare(
        &empty,
        entries(&["z".into(), "a".into()], key, 1),
        &request(1, ""),
        None,
        None,
        &mut retirement,
        &|| false,
    )
    .unwrap();
    let mut prior = Selection::new();
    prior.select_only(std::ffi::OsStr::new("z"));
    let original = Arc::new(Frame {
        selection: Arc::new(prior),
        ..original
    });
    let runtime = Runtime::new(original.clone());
    runtime.request(request(2, ""));
    let cancel = CancelToken::new();
    let search = runtime.begin_search(1, cancel.clone());
    assert!(runtime.producer().find(
        vec![z.clone(), a.clone()],
        Some(root.clone()),
        search,
        &cancel
    ));
    let next = receive(&runtime);
    assert!(next.all.ranked);
    assert_eq!(
        next.visible
            .iter()
            .map(|&i| next.all[i].name.clone())
            .collect::<Vec<_>>(),
        [OsString::from("z"), OsString::from("a")]
    );
    assert!(next.selection.is_empty());
    runtime.installed(next, original);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relisting_revalidates_thumbnail_versions_even_when_listing_metadata_matches() {
    let names = [OsString::from("photo.png")];
    let old = entries(&names, "/old", 1);
    let metadata = Metadata {
        size: 12,
        mtime: 4,
        ..Metadata::default()
    };
    assert!(old.patch(0, &names[0], Some(metadata)));
    old.thumbs
        .insert(names[0].clone(), "file:///old-cache-version".into());
    let empty = Frame::empty(LocationKey::local("/old"));
    let mut retired = Retirement::default();
    let before = prepare(
        &empty,
        old,
        &request(1, ""),
        None,
        None,
        &mut retired,
        &|| false,
    )
    .unwrap();
    let fresh = entries(&names, "/old", 2);
    assert!(fresh.patch(0, &names[0], Some(metadata)));
    let mut retired = Retirement::default();
    let after = prepare(
        &before,
        fresh,
        &request(2, ""),
        None,
        None,
        &mut retired,
        &|| false,
    )
    .unwrap();
    assert!(after.thumbs.get(&names[0]).is_none());
    assert!(before.thumbs.get(&names[0]).is_some());
}

#[test]
fn same_generation_load_retires_candidate_and_completion_before_replacement_io() {
    for retained_completion in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "gust-load-fence-{}-{retained_completion}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("replacement"), b"new").unwrap();
        let mut current = frame(20);
        let runtime = Runtime::new(current.clone());
        let candidate = entries(&["obsolete".into()], "/old", 8);
        let weak = Arc::downgrade(&candidate);
        let notice = Arc::new(Mutex::new(Vec::new()));
        *candidate.drop_notice.lock().unwrap() = Some(notice.clone());
        let mut retired = Retirement::default();
        let old_completion = retained_completion.then(|| {
            Arc::new(
                prepare(
                    &current,
                    candidate.clone(),
                    &request(2, ""),
                    None,
                    None,
                    &mut retired,
                    &|| false,
                )
                .unwrap(),
            )
        });
        drop(retired);
        {
            let mut s = runtime.pane.state.lock().unwrap();
            s.desired = request(2, "");
            s.candidate = Some(candidate);
            s.completion = old_completion;
        }
        let cleanup = Arc::new(Gate::default());
        let gate = cleanup.clone();
        *runtime.pane.hooks.retire.lock().unwrap() = Some(Arc::new(move || gate.block()));
        let io_seen = Arc::new(AtomicBool::new(false));
        let seen = io_seen.clone();
        *runtime.pane.hooks.io_start.lock().unwrap() = Some(Arc::new(move || {
            assert!(
                weak.upgrade().is_none(),
                "replacement I/O preceded old-load retirement"
            );
            seen.store(true, Ordering::Release);
        }));
        runtime.load(Load {
            key: LocationKey::local(&root),
            generation: 1,
            sort: SortSpec::default(),
            reuse: false,
            live: true,
            thumbnail_names: Vec::new(),
        });
        assert!(runtime.take(false).is_none());
        cleanup.wait_entered();
        assert_eq!(runtime.slots()["candidate"], false);
        assert_eq!(runtime.slots()["completion"], false);
        assert!(!io_seen.load(Ordering::Acquire));
        cleanup.release();
        let next = receive(&runtime);
        assert_eq!(next.all.load_revision, 1);
        assert_eq!(next.all.generation, current.all.generation);
        assert_eq!(next.all[next.visible[0]].name, "replacement");
        assert!(io_seen.load(Ordering::Acquire));
        assert_eq!(notice.lock().unwrap().as_slice(), ["gust-retire-0"]);
        install(&runtime, &mut current, next);
        wait_retired(&runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn same_generation_load_cancels_running_projection_and_recovers() {
    let root = std::env::temp_dir().join(format!("gust-load-running-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("new"), b"x").unwrap();
    let mut current = frame(2000);
    let runtime = Runtime::new(current.clone());
    let checkpoint = Arc::new(Gate::default());
    let gate = checkpoint.clone();
    *runtime.pane.hooks.checkpoint.lock().unwrap() = Some(Arc::new(move |n| {
        if n == 2 {
            gate.block()
        }
    }));
    runtime.request(request(2, ""));
    checkpoint.wait_entered();
    runtime.load(Load {
        key: LocationKey::local(&root),
        generation: 1,
        sort: SortSpec::default(),
        reuse: false,
        live: true,
        thumbnail_names: Vec::new(),
    });
    checkpoint.release();
    let next = receive(&runtime);
    assert_eq!(next.all.load_revision, 1);
    assert_eq!(next.all[next.visible[0]].name, "new");
    assert!(runtime.metrics().cancel_observed.load(Ordering::Relaxed) > 0);
    install(&runtime, &mut current, next);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn search_ticket_survives_presentation_but_not_search_or_navigation_replacement() {
    let root = std::env::temp_dir().join(format!("gust-search-ticket-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for name in ["z", "a", ".hidden"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    for explicit_sort in [false, true] {
        let mut current = frame(10);
        let runtime = Runtime::new(current.clone());
        runtime.request(request(2, ""));
        let cancel = CancelToken::new();
        let ticket = runtime.begin_search(1, cancel.clone());
        let mut latest = request(3, "");
        latest.hidden = true;
        latest.clipboard = Arc::new([root.join("a")].into_iter().collect());
        if explicit_sort {
            latest.sort_revision = 1;
        }
        runtime.request(latest);
        assert!(runtime.producer().find(
            vec![root.join("z"), root.join("a"), root.join(".hidden")],
            Some(root.clone()),
            ticket,
            &cancel
        ));
        // Presentation may already have finished before the search candidate.
        loop {
            let next = receive(&runtime);
            let ranked = next.all.ranked;
            install(&runtime, &mut current, next);
            if ranked {
                break;
            }
        }
        let names: Vec<_> = current
            .visible
            .iter()
            .map(|&i| current.all[i].name.to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            if explicit_sort {
                vec![".hidden", "a", "z"]
            } else {
                vec!["z", "a", ".hidden"]
            }
        );
        assert_eq!(current.request, 3);
        assert_eq!(current.marked, 1);
        assert!(current.selection.is_empty());
        let replacement = runtime.begin_search(1, CancelToken::new());
        assert!(!runtime.producer().find(
            vec![root.join("z")],
            Some(root.clone()),
            ticket,
            &cancel
        ));
        assert_ne!(ticket, replacement);
        let mut nav = request(4, "");
        nav.generation = 2;
        runtime.request(nav);
        assert!(!runtime.producer().find(
            vec![],
            Some(root.clone()),
            replacement,
            &CancelToken::new()
        ));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn trailing_band_replacements_preserve_running_events_and_toggle_barriers() {
    let original = frame(1000);
    let mut selected = Selection::new();
    selected.toggle(std::ffi::OsStr::new("file0000900"));
    let mut current = Arc::new(Frame {
        selection: Arc::new(selected),
        ..clone_frame_for_test(&original)
    });
    let runtime = Runtime::new(current.clone());
    let base = Arc::new(BandBase {
        selection: Some(current.selection.clone()),
    });
    let gate = Arc::new(Gate::default());
    let blocked = gate.clone();
    *runtime.pane.hooks.checkpoint.lock().unwrap() = Some(Arc::new(move |n| {
        if n == 0 {
            blocked.block()
        }
    }));
    let band = |hi| Gesture::Band {
        base: base.clone(),
        all: original.all.clone(),
        visible: original.visible.clone(),
        rectangle: BandRect {
            lo: 0,
            hi,
            left: 0,
            right: 0,
            cols: 1,
        },
    };
    let r = |revision| {
        let mut r = request(1, "");
        r.selection_revision = revision;
        r
    };
    runtime.gesture(band(0), r(1)).unwrap();
    gate.wait_entered();
    for revision in 2..=350 {
        runtime
            .gesture(band(revision as usize % 100), r(revision))
            .unwrap();
    }
    assert_eq!(runtime.slots()["selection_intents"], 2);
    runtime
        .gesture(Gesture::Toggle("file0000800".into()), r(351))
        .unwrap();
    for revision in 352..=701 {
        runtime
            .gesture(band(revision as usize % 100), r(revision))
            .unwrap();
    }
    assert_eq!(runtime.slots()["selection_intents"], 4);
    gate.release();
    for revision in [1, 350, 351, 701] {
        let next = receive(&runtime);
        assert_eq!(next.selection_revision, revision);
        assert!(Arc::ptr_eq(&next.row_of, &original.row_of));
        if revision == 351 {
            assert!(next.selection.contains(std::ffi::OsStr::new("file0000800")));
        }
        install(&runtime, &mut current, next);
    }
    assert!(!runtime.pending_selection());
    assert_eq!(current.selection.len(), 3);
    for name in ["file0000000", "file0000001", "file0000900"] {
        assert!(current.selection.contains(std::ffi::OsStr::new(name)));
    }
    assert!(!current
        .selection
        .contains(std::ffi::OsStr::new("file0000800")));
    // The following range has exactly the core band's pinned/anchor semantics.
    let mut oracle = Selection::new();
    oracle.set_band(
        &["file0000900".into()],
        &["file0000000".into(), "file0000001".into()],
    );
    let names: Vec<_> = current
        .visible
        .iter()
        .map(|&i| current.all[i].name.clone())
        .collect();
    oracle.extend_by_name(&names, 2, 3, RangeMode::Replace);
    runtime
        .gesture(Gesture::Range { from: 2, to: 3 }, r(702))
        .unwrap();
    let next = receive(&runtime);
    for name in &names {
        assert_eq!(next.selection.contains(name), oracle.contains(name));
    }
    install(&runtime, &mut current, next);
}

#[test]
fn mark_publication_has_its_own_identity_and_never_acknowledges_presentation_or_selection() {
    let mut current = frame(1000);
    let runtime = Runtime::new(current.clone());
    let mut r = request(2, "file9");
    r.focus = Some("file0000900".into());
    r.keep = 1;
    r.selection_revision = 1;
    runtime
        .gesture(Gesture::Only("file0000007".into()), r)
        .unwrap();
    runtime.marks(Arc::new(
        [std::path::PathBuf::from("/old/file0000007")]
            .into_iter()
            .collect(),
    ));
    let next = receive(&runtime);
    assert_eq!(next.publication, Publication::Selection);
    assert_eq!(next.request, 1);
    assert_eq!(next.query, "");
    assert_eq!(next.marks_revision, 0);
    install(&runtime, &mut current, next);
    let next = receive(&runtime);
    assert_eq!(next.publication, Publication::Marks);
    assert_eq!(next.selection_revision, 1);
    assert_eq!(next.request, 1);
    assert_eq!(next.query, "");
    assert_eq!(next.marked, 1);
    assert!(Arc::ptr_eq(&next.row_of, &current.row_of));
    assert!(Arc::ptr_eq(&next.selection, &current.selection));
    install(&runtime, &mut current, next);
    assert!(!runtime.pending_selection());
    let rows = receive(&runtime);
    assert_eq!(rows.publication, Publication::Rows);
    assert_eq!(rows.request, 2);
    assert_eq!(rows.query, "file9");
    install(&runtime, &mut current, rows);
    runtime.marks(Arc::default());
    let cleared = receive(&runtime);
    assert_eq!(cleared.publication, Publication::Marks);
    assert_eq!(cleared.marked, 0);
    install(&runtime, &mut current, cleared);
}

#[test]
#[ignore = "isolated release scheduler acceptance; run --release --exact with no parallel probes"]
fn release_scheduler_supersession_trials() {
    use gust_core::entry::{sort_entries, EntryKind, SortField};
    use std::os::unix::ffi::OsStrExt;
    fn checksum(names: impl Iterator<Item = OsString>) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for name in names {
            for &b in name.as_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
            }
            h = (h ^ 0xff).wrapping_mul(0x100000001b3)
        }
        h
    }
    let size = std::env::var("GUST_APP_TRIAL_ROWS")
        .ok()
        .map(|s| s.parse::<usize>().unwrap());
    // Exercise the prepared scalar path independently of the historical
    // high-cardinality Name-to-Size and selection-retirement controls.
    let scalar = std::env::var_os("GUST_APP_TRIAL_SCALAR").is_some();
    for n in [100_000usize, 1_000_000]
        .into_iter()
        .filter(|n| size.is_none_or(|size| size == *n))
    {
        let mut snapshot = DirectorySnapshot::empty(LocationKey::local("/controlled"), 1);
        snapshot.complete = Completeness::Complete;
        snapshot.entries = (0..n)
            .map(|i| {
                let number = i * 524_287 % n;
                let mut entry = Entry::new(
                    format!("file{number:08}").into(),
                    if i % 7 == 0 {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                );
                entry.meta = Some(Metadata {
                    size: if scalar {
                        (i % 17) as u64
                    } else {
                        ((i * 1_000_003) % n) as u64
                    },
                    mtime: if scalar {
                        (i % 19) as i64 - 9
                    } else {
                        (i * 17 % n) as i64
                    },
                    ..Metadata::default()
                });
                entry
            })
            .collect();
        let raw = Arc::new(Entries::new(snapshot, 1));
        let mut retired = Retirement::default();
        let backing = raw.ready(&mut retired, &|| false).unwrap();
        drop(raw);
        drop(retired);
        let initial = Frame::empty(LocationKey::local("/controlled"));
        let mut initial_request = request(1, "");
        initial_request.hidden = true;
        let mut retired = Retirement::default();
        let original = Arc::new(
            prepare(
                &initial,
                backing.clone(),
                &initial_request,
                None,
                None,
                &mut retired,
                &|| false,
            )
            .unwrap(),
        );
        drop(retired);
        let mut oracle: Vec<_> = backing.iter().cloned().collect();
        let final_sort = SortSpec {
            field: SortField::Size,
            ascending: false,
            dirs_first: true,
        };
        sort_entries(&mut oracle, final_sort);
        let expected: Vec<_> = oracle.iter().map(|e| e.name.clone()).collect();
        let expected_checksum = checksum(expected.iter().cloned());
        drop(oracle);
        for scenario in ["arbitrary-sort", "selection-prune"] {
            if scalar && scenario != "arbitrary-sort" {
                continue;
            }
            for repetition in 0..3 {
                let mut current = if scenario == "selection-prune" {
                    let names: Vec<_> = original
                        .visible
                        .iter()
                        .map(|&i| original.all[i].name.clone())
                        .collect();
                    let mut selection = Selection::new();
                    selection.select_all_by_name(&names);
                    Arc::new(Frame {
                        selection: Arc::new(selection),
                        ..clone_frame_for_test(&original)
                    })
                } else {
                    original.clone()
                };
                let input_checksum =
                    checksum(current.visible.iter().map(|&i| current.all[i].name.clone()));
                let runtime = Runtime::new(current.clone());
                let gate = Arc::new(Gate::default());
                let blocked = gate.clone();
                let checkpoints = AtomicUsize::new(0);
                let phase = if scenario == "selection-prune" {
                    "selection"
                } else {
                    "sort"
                };
                let at = if scenario == "selection-prune" {
                    (n * 3 / 4).div_ceil(256)
                } else if scalar {
                    // Past preparation and bounded runs for this fixture.
                    // The receipt records the actual sort-phase checkpoint;
                    // core probes separately identify exact algorithm phases.
                    2 * n.div_ceil(256) + 64
                } else {
                    64
                };
                *runtime.pane.hooks.checkpoint.lock().unwrap() = Some(Arc::new(move |_| {
                    if JOB_PHASE.with(|current| current.get()) == phase
                        && checkpoints.fetch_add(1, Ordering::Relaxed) == at
                    {
                        blocked.block()
                    }
                }));
                let mut first = initial_request.clone();
                first.request = 2;
                if scenario == "arbitrary-sort" {
                    if scalar {
                        first.sort.field = SortField::Modified;
                    }
                    first.sort.ascending = false;
                    first.sort_revision = 1;
                }
                first.submitted = Instant::now();
                runtime.request(first);
                gate.wait_entered();
                let mut latest = initial_request.clone();
                latest.request = 3;
                latest.submitted = Instant::now();
                if scenario == "arbitrary-sort" {
                    latest.sort = final_sort;
                    latest.sort_revision = 2;
                } else {
                    latest.query = "absent".into();
                }
                let submitted = latest.submitted;
                runtime.request(latest);
                gate.release();
                let next = receive(&runtime);
                let completed_us = submitted.elapsed().as_micros();
                assert_eq!(next.request, 3);
                let actual: Vec<_> = next
                    .visible
                    .iter()
                    .map(|&i| next.all[i].name.clone())
                    .collect();
                let order_checksum = checksum(next.order.iter().map(|&i| next.all[i].name.clone()));
                let result_checksum = checksum(actual.iter().cloned());
                if scenario == "arbitrary-sort" {
                    assert_eq!(actual, expected);
                    assert_eq!(result_checksum, expected_checksum);
                } else {
                    assert!(actual.is_empty());
                    assert!(next.selection.is_empty());
                    assert_eq!(next.order.as_ref(), original.order.as_ref());
                    assert_eq!(order_checksum, input_checksum);
                    assert!(
                        runtime
                            .metrics()
                            .retired_selection_names_max
                            .load(Ordering::Relaxed)
                            > n as u64 / 2
                    );
                }
                assert_eq!(runtime.metrics().cancel_observed.load(Ordering::Relaxed), 1);
                eprintln!(
                    "APP_SCHEDULER_TRIAL {}",
                    serde_json::json!({"rows":n,"scenario":scenario,"scalar_prepared_fixture":scalar,"repetition":repetition,"setup":"immutable shuffled raw entries with complete metadata, oracle and selection built before timing; process pools production2CPU/1retire","trigger":format!("deterministic checkpoint phase={phase} index={at}; no retirement/cleanup gate; hook cached once/job"),"input_checksum":input_checksum,"order_checksum":order_checksum,"result_checksum":result_checksum,"full_oracle_equality":true,"completion_us":completed_us,"worker":runtime.metrics().json()})
                );
                install(&runtime, &mut current, next);
                wait_retired(&runtime);
            }
        }
    }
}

#[test]
fn live_preflight_validates_full_source_stamp_before_frame_publication() {
    use std::os::unix::fs::MetadataExt;
    fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        // The cache stamp scanner reads bounded metadata, independently of a
        // decoder; this fixture intentionally contains only the required tags.
        png.extend_from_slice(&0u32.to_be_bytes());
    }
    let root = std::env::temp_dir().join(format!("gust-preflight-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("photo.png");
    std::fs::write(&source, b"original").unwrap();
    let md = source.symlink_metadata().unwrap();
    let modified = md.modified().unwrap();
    let stamp = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        md.dev(),
        md.ino(),
        md.len(),
        md.mtime(),
        md.mtime_nsec(),
        md.ctime(),
        md.ctime_nsec()
    );
    let target =
        gust_thumbs::cache::thumbnail_path(&source, gust_thumbs::ThumbSize::Normal).unwrap();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0; 13]);
    chunk(
        &mut png,
        b"tEXt",
        format!("Thumb::MTime\0{}", md.mtime()).as_bytes(),
    );
    chunk(
        &mut png,
        b"tEXt",
        format!("Gust::SourceVersion\0{stamp}").as_bytes(),
    );
    std::fs::write(&target, png).unwrap();
    let url = gust_thumbs::render::cached_thumbnail(&source, gust_thumbs::ThumbSize::Normal)
        .unwrap()
        .url();
    let mut current = frame(3);
    let runtime = Runtime::new(current.clone());
    runtime.request(request(2, ""));
    let live = || Load {
        key: LocationKey::local(&root),
        generation: 1,
        sort: SortSpec::default(),
        reuse: false,
        live: true,
        thumbnail_names: vec!["photo.png".into(), "removed.png".into()],
    };
    runtime.load(live());
    let next = receive(&runtime);
    assert_eq!(next.validated_thumbnail_count, 1);
    assert_eq!(next.thumbs.len(), 1);
    assert_eq!(
        next.thumbs.get(std::ffi::OsStr::new("photo.png")),
        Some(&url)
    );
    install(&runtime, &mut current, next);
    wait_retired(&runtime);
    let previous = gust_core::lister::stat_entry(
        &root,
        std::ffi::OsStr::new("photo.png"),
        gust_core::EntryKind::File,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1));
    std::fs::write(&source, b"modified").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let now = gust_core::lister::stat_entry(
        &root,
        std::ffi::OsStr::new("photo.png"),
        gust_core::EntryKind::File,
    )
    .unwrap();
    assert_eq!(
        now, previous,
        "fixture must retain every listing Metadata fact"
    );
    runtime.load(live());
    let changed = receive(&runtime);
    assert_eq!(changed.validated_thumbnail_count, 0);
    assert!(changed.thumbs.is_empty());
    assert!(changed
        .thumbs
        .get(std::ffi::OsStr::new("photo.png"))
        .is_none());
    install(&runtime, &mut current, changed);
    wait_retired(&runtime);
    // Only the two cache paths derived from this unique fixture are owned.
    let immutable =
        std::path::PathBuf::from(url.split('?').next().unwrap().trim_start_matches("file://"));
    let _ = std::fs::remove_file(immutable);
    let _ = std::fs::remove_file(target);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn marks_start_metric_uses_own_admission_not_aged_row_request() {
    let runtime = Runtime::new(frame(3));
    let row_submitted = Instant::now() - Duration::from_secs(3600);
    {
        let mut state = runtime.pane.state.lock().unwrap();
        // Reserve worker admission while testing timestamps, without a sleep.
        state.cpu_running = true;
        state.desired = request(1, "row-query");
        state.desired.submitted = row_submitted;
    }
    let before = Instant::now();
    runtime.marks(Arc::default());
    let after = Instant::now();
    {
        let state = runtime.pane.state.lock().unwrap();
        assert!((before..=after).contains(&state.marks_submitted));
        assert_eq!(state.desired.submitted, row_submitted);
        assert_eq!(state.desired.request, 1);
        assert_eq!(state.desired.query, "row-query");
        assert_eq!(state.admission_time(None, false), row_submitted);
        let admitted = state.admission_time(None, true);
        assert_eq!(admitted, state.marks_submitted);
        // Synthetic worker start makes this an exact metric assertion,
        // independent of executor scheduling or the host's elapsed time.
        let metrics = Metrics::default();
        metrics.record_start(admitted, admitted + Duration::from_micros(37));
        assert_eq!(metrics.json()["latest_start_us_max"], 37);
        assert_eq!(metrics.json()["started"], 1);
    }
    // A replacement mark has its own latest admission without re-aging rows.
    runtime.pane.state.lock().unwrap().marks_submitted = row_submitted;
    let before = Instant::now();
    runtime.marks(Arc::default());
    let mut state = runtime.pane.state.lock().unwrap();
    assert!(state.marks_submitted >= before);
    assert_eq!(state.desired.marks_revision, 2);
    assert_eq!(state.desired.submitted, row_submitted);
    state.cpu_running = false;
}

#[test]
fn queued_selection_events_keep_their_admission_across_later_intents() {
    let runtime = Runtime::new(frame(3));
    let row_submitted = Instant::now() - Duration::from_secs(3600);
    {
        let mut state = runtime.pane.state.lock().unwrap();
        state.cpu_running = true;
        state.desired = request(1, "");
        state.desired.submitted = row_submitted;
    }
    let mut first = request(1, "");
    first.selection_revision = 1;
    let before = Instant::now();
    runtime
        .gesture(Gesture::Toggle("file0000000".into()), first)
        .unwrap();
    let after = Instant::now();
    {
        let mut state = runtime.pane.state.lock().unwrap();
        assert_eq!(state.desired.submitted, row_submitted);
        let event = state.gestures.front_mut().unwrap();
        assert!((before..=after).contains(&event.submitted));
        // Deterministically model time already spent in the ordered queue.
        event.submitted -= Duration::from_secs(60);
    }
    let first_submitted = runtime
        .pane
        .state
        .lock()
        .unwrap()
        .gestures
        .front()
        .unwrap()
        .submitted;
    let mut second = request(1, "");
    second.selection_revision = 2;
    runtime
        .gesture(Gesture::Toggle("file0000001".into()), second)
        .unwrap();
    assert_eq!(
        runtime.pane.state.lock().unwrap().desired.submitted,
        row_submitted
    );
    runtime.marks(Arc::default());
    let mut latest = request(2, "later-row-query");
    latest.selection_revision = 2;
    let latest_submitted = latest.submitted;
    runtime.request(latest);
    let mut state = runtime.pane.state.lock().unwrap();
    assert_eq!(state.gestures.len(), 2);
    assert_eq!(state.gestures.front().unwrap().submitted, first_submitted);
    assert_eq!(state.desired.submitted, latest_submitted);
    let first = state.gestures.front().unwrap();
    assert_eq!(state.admission_time(Some(first), false), first_submitted);
    // Even with a mark waiting, selection retains its ordered priority/time.
    assert_eq!(state.admission_time(Some(first), true), first_submitted);
    let metrics = Metrics::default();
    metrics.record_start(
        state.admission_time(Some(first), false),
        first_submitted + Duration::from_secs(61),
    );
    assert_eq!(metrics.json()["latest_start_us_max"], 61_000_000);
    let second = state.gestures.back().unwrap();
    assert!(second.submitted > first_submitted);
    let metrics = Metrics::default();
    metrics.record_start(
        state.admission_time(Some(second), false),
        second.submitted + Duration::from_micros(11),
    );
    assert_eq!(metrics.json()["latest_start_us_max"], 11);
    state.cpu_running = false;
}

#[test]
fn explicit_same_origin_load_still_supersedes_search_ticket() {
    let root =
        std::env::temp_dir().join(format!("gust-explicit-search-load-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("current"), b"current").unwrap();
    let mut current = frame(3);
    let runtime = Runtime::new(current.clone());
    runtime.request(request(2, ""));
    let cancel = CancelToken::new();
    let ticket = runtime.begin_search(1, cancel.clone());
    assert!(runtime.has_current_search(1));
    assert!(!runtime.has_current_search(2));
    runtime.load(Load {
        key: LocationKey::local(&root),
        generation: 1,
        sort: SortSpec::default(),
        reuse: false,
        live: false,
        thumbnail_names: Vec::new(),
    });
    assert!(!runtime.has_current_search(1));
    assert!(cancel.is_cancelled());
    assert!(!runtime.search_current(ticket));
    let next = receive(&runtime);
    assert!(!next.all.ranked);
    assert_eq!(next.all[next.visible[0]].name, "current");
    install(&runtime, &mut current, next);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn size_snapshot_copy_checks_full_metadata_and_exact_names() {
    let base = frame(130);
    let value = |i: usize| gust_core::dirsize::Sized {
        bytes: i as u64 * 119,
        files: i as u64 ^ 0x9876,
        complete: !i.is_multiple_of(3),
    };
    for i in 0..base.all.len() {
        assert!(base.all.patch(
            i,
            &base.all[i].name,
            Some(Metadata {
                size: i as u64,
                ..Metadata::default()
            })
        ));
        base.dir_sizes.insert(base.all[i].name.clone(), value(i));
    }
    let retained_copy = base.dir_sizes.get(&base.all[7].name).unwrap();
    let mut snapshot = DirectorySnapshot::empty(base.all.key.clone(), 1);
    snapshot.complete = Completeness::Complete;
    snapshot.entries = base.all.iter().cloned().collect();
    snapshot.entries.reverse();
    snapshot.entries[0].meta.as_mut().unwrap().size += 1;
    snapshot.entries[1].name = "new-raw-name".into();
    let mut retired = Retirement::default();
    let copied = prepare(
        &base,
        Arc::new(Entries::new(snapshot, 2)),
        &request(2, ""),
        None,
        None,
        &mut retired,
        &|| false,
    )
    .unwrap();
    assert_eq!(copied.dir_sizes.len(), 128);
    for i in 0..128 {
        assert_eq!(copied.dir_sizes.get(&base.all[i].name), Some(value(i)));
    }
    assert_eq!(copied.dir_sizes.get(&base.all[129].name), None);
    assert_eq!(
        copied.dir_sizes.get(std::ffi::OsStr::new("new-raw-name")),
        None
    );
    base.dir_sizes.insert(base.all[7].name.clone(), value(999));
    assert_eq!(retained_copy, value(7));
    assert_eq!(copied.dir_sizes.get(&base.all[7].name), Some(value(7)));
}

#[test]
fn cancelled_size_preparation_transfers_partial_pages_to_existing_retirement() {
    let n: usize = 100_000;
    let mut snapshot = DirectorySnapshot::empty(LocationKey::local("/size-cancel"), 1);
    snapshot.entries = (0..n)
        .map(|i| Entry::new(format!("n{i}").into(), gust_core::EntryKind::File))
        .collect();
    let initial = Arc::new(Entries::new(snapshot, 1));
    let weak_raw = Arc::downgrade(&initial.raw);
    let checkpoints = AtomicUsize::new(0);
    // NameIndex, metadata pages, and both thumbnail page indices precede sizes.
    let size_start = n.div_ceil(128) + 3 * n.div_ceil(64).div_ceil(128);
    let mut retired = Retirement::default();
    assert!(initial
        .ready(&mut retired, &|| checkpoints
            .fetch_add(1, Ordering::Relaxed)
            > size_start + 2)
        .is_err());
    assert!(retired.objects[0].is_some());
    assert!(retired.objects[1].is_some());
    assert!(retired.objects[2].is_some());
    drop(initial);
    assert!(weak_raw.upgrade().is_some());
    std::thread::Builder::new()
        .name("cancelled-size-preparation-recipient".into())
        .spawn(move || {
            drop(retired);
            assert!(weak_raw.upgrade().is_none());
        })
        .unwrap()
        .join()
        .unwrap();
}
