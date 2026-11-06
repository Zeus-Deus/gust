//! Real CXX-Qt object, signal observers and QPersistentModelIndex on Qt 6.
use super::*;
use cxx_qt_lib::QCoreApplication;
use std::os::unix::ffi::OsStringExt;
use std::time::{Duration, Instant};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gust-projection-qt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        for i in 0..1000 {
            std::fs::write(root.join("a").join(format!("file{i:04}")), b"data").unwrap();
        }
        std::fs::write(root.join("b/only-b"), b"b").unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn wait(
    app: &QCoreApplication,
    model: &qobject::DirModel,
    condition: impl Fn(&qobject::DirModel) -> bool,
) {
    let until = Instant::now() + Duration::from_secs(15);
    while !condition(model) {
        assert!(
            Instant::now() < until,
            "Qt model timed out: {}",
            model.state_json()
        );
        app.process_events();
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn actual_qt_navigation_layout_persistent_mapping_metadata_and_hold() {
    let app = QCoreApplication::new();
    let app = app.as_ref().unwrap();
    let fixture = Fixture::new();
    let raw = std::ffi::OsString::from_vec(vec![b'r', 0xff, b'\n']);
    std::fs::create_dir(fixture.0.join("a").join(&raw)).unwrap();
    let mut model = qobject::gust_test_new_model();
    qobject::gust_test_attach_probe(model.pin_mut());
    model.pin_mut().navigate_path(fixture.0.join("a"));
    wait(app, &model, |m| m.settled && m.count == 1001);
    model
        .pin_mut()
        .apply_sort_to(&QString::from("name"), true, false);
    wait(app, &model, |m| !m.sort_pending && m.frame.sort.ascending);
    let before_sort = probe(&model);
    let old_path = model.path_of_row(200).unwrap();
    qobject::gust_test_remember_index(model.pin_mut(), 200);
    model.pin_mut().focus_row(200);
    model
        .pin_mut()
        .apply_sort_to(&QString::from("name"), false, false);
    wait(app, &model, |m| !m.sort_pending && !m.frame.sort.ascending);
    let probe: serde_json::Value =
        serde_json::from_str(&qobject::gust_test_probe_state(&model).to_string()).unwrap();
    assert_eq!(probe["ok"], true, "{probe}");
    assert_eq!(
        probe["layouts"].as_u64().unwrap(),
        before_sort["layouts"].as_u64().unwrap() + 1
    );
    assert_eq!(model.path_of_row(model.cursor), Some(old_path));
    assert_eq!(probe["persistent_row"], model.cursor);
    assert_eq!(model.published_row(200), -1);
    // The old row still names A while B is requested and held.
    model.pin_mut().hold_live_updates(true);
    model.pin_mut().navigate_path(fixture.0.join("b"));
    assert_eq!(model.key, LocationKey::local(fixture.0.join("a")));
    assert!(model
        .path_of_row(0)
        .unwrap()
        .starts_with(fixture.0.join("a")));
    wait(app, &model, |m| m.runtime.slots()["completion"] == true);
    assert!(!model.settled);
    assert_eq!(model.key, LocationKey::local(fixture.0.join("a")));
    model.pin_mut().hold_live_updates(false);
    wait(app, &model, |m| m.settled && m.count == 1);
    assert_eq!(model.path_of_row(0), Some(fixture.0.join("b/only-b")));
    pending_selection_refuses_readonly_targets_and_recovers(app, &mut model, &fixture);
    let same_folder = model.location_token();
    assert_eq!(
        model
            .pin_mut()
            .copy_selection_to_token(&same_folder)
            .to_string(),
        "already there"
    );
    assert_eq!(
        model
            .pin_mut()
            .move_selection_to_token(&same_folder)
            .to_string(),
        "already there"
    );
    assert!(!model.job_running());
    let before_filter: serde_json::Value =
        serde_json::from_str(&qobject::gust_test_probe_state(&model).to_string()).unwrap();
    model.pin_mut().set_filter_text(&QString::from("only"));
    wait(app, &model, |m| {
        !m.filter_pending && m.shown_filter.to_string() == "only"
    });
    let after_filter: serde_json::Value =
        serde_json::from_str(&qobject::gust_test_probe_state(&model).to_string()).unwrap();
    assert_eq!(model.cursor, 0);
    assert!(after_filter["cursors"].as_u64().unwrap() > before_filter["cursors"].as_u64().unwrap());
    // A patch for the old listing/request cannot fill this same numerical row.
    let generation = model.frame.all.generation;
    let listing = model.frame.all.listing;
    let request = model.meta_request;
    model.pin_mut().apply_row_meta(
        generation,
        listing - 1,
        request,
        vec![(
            0,
            "only-b".into(),
            Some(gust_core::entry::Metadata {
                size: 987,
                ..Default::default()
            }),
        )],
        false,
    );
    assert_ne!(model.entry_at(0).unwrap().meta.map(|m| m.size), Some(987));
    // Raw directory activation survives invalid UTF-8 and display escaping.
    model.pin_mut().navigate_path(fixture.0.join("a"));
    wait(app, &model, |m| m.settled && m.count == 1001);
    let rawrow = model
        .frame
        .visible
        .iter()
        .position(|&i| model.frame.all[i].name == raw)
        .unwrap() as i32;
    assert_eq!(
        model.entry_token(rawrow).to_string(),
        LocationKey::local(fixture.0.join("a").join(&raw)).to_token()
    );
    assert_eq!(model.pin_mut().activate(rawrow).to_string(), "dir");
    wait(app, &model, |m| {
        m.settled && m.key.as_path() == Some(fixture.0.join("a").join(&raw).as_path())
    });
    assert_eq!(
        model.key.as_path(),
        Some(fixture.0.join("a").join(&raw).as_path())
    );
    let probe: serde_json::Value =
        serde_json::from_str(&qobject::gust_test_probe_state(&model).to_string()).unwrap();
    assert_eq!(probe["ok"], true, "{probe}");
    assert_eq!(probe["open"], false);
    assert!(probe["resets"].as_u64().unwrap() >= 4);
    // Distinct invalid byte names can have the same QString display. A
    // raw location transition still notifies session/history/view observers.
    let collision_a = fixture
        .0
        .join(std::ffi::OsString::from_vec(vec![b'b', 0xfe]));
    let collision_b = fixture
        .0
        .join(std::ffi::OsString::from_vec(vec![b'b', 0xff]));
    std::fs::create_dir(&collision_a).unwrap();
    std::fs::create_dir(&collision_b).unwrap();
    model.pin_mut().navigate_path(collision_a.clone());
    wait(app, &model, |m| {
        m.settled && m.key.as_path() == Some(collision_a.as_path())
    });
    let display = model.location.to_string();
    let before: serde_json::Value =
        serde_json::from_str(&qobject::gust_test_probe_state(&model).to_string()).unwrap();
    model
        .pin_mut()
        .navigate_token(&QString::from(&LocationKey::local(&collision_b).to_token()));
    assert_eq!(model.key.as_path(), Some(collision_a.as_path()));
    wait(app, &model, |m| {
        m.settled && m.key.as_path() == Some(collision_b.as_path())
    });
    assert_eq!(display, model.location.to_string());
    let after: serde_json::Value =
        serde_json::from_str(&qobject::gust_test_probe_state(&model).to_string()).unwrap();
    assert_eq!(
        after["locations"].as_u64().unwrap(),
        before["locations"].as_u64().unwrap() + 1
    );
    assert_eq!(after["ok"], true);
    assert_eq!(model.entry_token(-1).to_string(), "");
    for bad in [
        "/tmp/plain-text",
        "local:00",
        "local:72656c6174697665",
        "sftp:2f746d70",
    ] {
        assert!(model
            .pin_mut()
            .copy_selection_to_token(&QString::from(bad))
            .to_string()
            .contains("Invalid destination token"));
        assert!(model
            .pin_mut()
            .move_selection_to_token(&QString::from(bad))
            .to_string()
            .contains("Invalid destination token"));
        assert!(model
            .drag_preview_into_token(&QString::from(bad), 1)
            .to_string()
            .contains("Invalid destination token"));
        assert!(model
            .pin_mut()
            .drag_drop_into_token(&QString::from(bad), 1)
            .to_string()
            .contains("Invalid destination token"));
    }
    typed_destination_tokens_match_actual_navigation(app, &mut model, &fixture);
    newer_focus_survives_held_relist(app, &mut model, &fixture);
    equal_viewport_filter_measures_new_folders(app, &mut model, &fixture);
    publication_attribution_reenters_qt(app, &fixture);
    marks_do_not_reset_or_layout(app, &mut model, &fixture);
    search_output_uses_latest_presentation_and_rejects_replacement(app, &mut model, &fixture);
    search_survives_automatic_origin_relist_and_job_completion(app, &mut model, &fixture);
    bounded_cache_renewal_and_cancelled_publications(app, &mut model, &fixture);
    folder_size_job_and_undo_use_fresh_backing(app, &mut model, &fixture);
    model.pin_mut().stop_watch();
    drop(model);
    app.process_events();
}

// Keep one QCoreApplication for the real QObject tests in this process.
fn pending_selection_refuses_readonly_targets_and_recovers(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    const REFUSAL: &str = "Selection is still being applied; wait and try again.";
    let path = fixture.0.join("b/only-b");
    let destination = QString::from(fixture.0.join("a").to_string_lossy().as_ref());
    let destination_token = QString::from(&LocationKey::local(fixture.0.join("a")).to_token());
    model.pin_mut().drag_begin(0);
    assert_eq!(model.drag_count(), 1);
    let before_mime = model.drag_mime_json().to_string();
    let mime: serde_json::Value = serde_json::from_str(&before_mime).unwrap();
    assert!(!mime["uris"].as_str().unwrap().is_empty());
    assert!(!mime["gnome"].as_str().unwrap().is_empty());
    assert_eq!(mime["text"], path.to_string_lossy().as_ref());
    let before_preview = model.drag_preview_into(&destination, 1).to_string();
    assert_ne!(before_preview, REFUSAL);
    assert_eq!(
        model
            .drag_preview_into_token(&destination_token, 1)
            .to_string(),
        before_preview
    );
    assert_eq!(model.custom_targets().len(), 1);
    let before_custom: serde_json::Value =
        serde_json::from_str(&model.custom_actions_json().to_string()).unwrap();
    assert!(before_custom["actions"].is_array());
    assert!(before_custom["problems"].is_array());
    let enumerations = CUSTOM_ACTION_ENUMERATIONS.load(std::sync::atomic::Ordering::Relaxed);

    model.pin_mut().sel_only(0);
    assert!(model.selection_pending);
    // Even an invalid row must refuse before constructing its target/plan.
    assert_eq!(model.drag_preview(i32::MAX, 1).to_string(), REFUSAL);
    assert_eq!(
        model.drag_preview_into(&destination, 1).to_string(),
        REFUSAL
    );
    assert_eq!(
        model
            .drag_preview_into_token(&destination_token, 1)
            .to_string(),
        REFUSAL
    );
    assert_eq!(
        model.pin_mut().drag_drop_on_row(i32::MAX, 1).to_string(),
        REFUSAL
    );
    assert!(!model.job_running());
    assert_eq!(model.pin_mut().clip_copy(0).to_string(), REFUSAL);
    assert!(model.target_paths().is_empty());
    let pending_mime: serde_json::Value =
        serde_json::from_str(&model.drag_mime_json().to_string()).unwrap();
    for key in ["uris", "text", "gnome"] {
        assert_eq!(pending_mime[key], "", "pending drag exported {key}");
    }
    assert_eq!(pending_mime["refusal"], REFUSAL);
    assert!(model.custom_targets().is_empty());
    let pending_custom: serde_json::Value =
        serde_json::from_str(&model.custom_actions_json().to_string()).unwrap();
    assert_eq!(pending_custom["actions"], serde_json::json!([]));
    assert_eq!(pending_custom["problems"], serde_json::json!([REFUSAL]));
    assert_eq!(
        CUSTOM_ACTION_ENUMERATIONS.load(std::sync::atomic::Ordering::Relaxed),
        enumerations,
        "pending custom request entered action loading"
    );

    wait(app, model, |m| !m.selection_pending);
    assert_eq!(model.target_paths(), vec![path.clone()]);
    assert_eq!(model.drag_mime_json().to_string(), before_mime);
    assert_eq!(
        model.drag_preview_into(&destination, 1).to_string(),
        before_preview
    );
    assert_eq!(
        model
            .drag_preview_into_token(&destination_token, 1)
            .to_string(),
        before_preview
    );
    let targets = model.custom_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].path, path);
    assert!(!targets[0].is_dir);
    let recovered_custom: serde_json::Value =
        serde_json::from_str(&model.custom_actions_json().to_string()).unwrap();
    assert!(recovered_custom["actions"].is_array());
    assert!(recovered_custom["problems"].is_array());
    assert_eq!(
        CUSTOM_ACTION_ENUMERATIONS.load(std::sync::atomic::Ordering::Relaxed),
        enumerations + 1
    );
    model.pin_mut().drag_end();
    assert!(!model.drag_hold);
}

fn probe(model: &qobject::DirModel) -> serde_json::Value {
    serde_json::from_str(&qobject::gust_test_probe_state(model).to_string()).unwrap()
}
fn marks_do_not_reset_or_layout(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    model.pin_mut().navigate_path(fixture.0.join("a"));
    wait(app, model, |m| {
        m.settled && m.count == 1001 && !m.sort_pending
    });
    let before = probe(model);
    let publications = model.callbacks.publications.json();
    let request = model.request_revision;
    assert!(!model.pin_mut().clip_copy(1).to_string().contains("nothing"));
    wait(app, model, |m| {
        m.frame.marks_revision == m.runtime.marks_revision() && m.frame.marked == 1
    });
    assert_eq!(model.request_revision, request);
    // An auxiliary change queued behind a selection event must not consume it.
    model.pin_mut().sel_only(3);
    model.pin_mut().clear_clip();
    wait(app, model, |m| {
        !m.selection_pending && m.frame.marks_revision == m.runtime.marks_revision()
    });
    assert_eq!(model.frame.marked, 0);
    assert_eq!(model.selection.len(), 1);
    assert_eq!(model.target_paths(), vec![model.path_of_row(3).unwrap()]);
    let after = probe(model);
    for key in ["resets", "layouts"] {
        assert_eq!(after[key], before[key]);
        assert_eq!(model.callbacks.publications.json()[key], publications[key]);
    }
    assert!(
        model.callbacks.publications.json()["marks"]
            .as_u64()
            .unwrap()
            >= 2
    );
    wait(app, model, |m| m.runtime.slots()["retiring"] == false);
    model.pin_mut().band_begin(false);
    assert!(model.band_active);
    let revision = model.selection_revision;
    model.pin_mut().band_update(0, 1, 0, 0, 1);
    for _ in 0..350 {
        model.pin_mut().band_update(1, 0, 0, 0, 1);
    }
    assert_eq!(
        model.selection_revision,
        revision + 1,
        "identical normalized rectangles admitted duplicate intent"
    );
    for end in 2..=320 {
        model.pin_mut().band_update(0, end, 0, 0, 1);
    }
    assert!(model.runtime.slots()["selection_intents"].as_u64().unwrap() <= 2);
    wait(app, model, |m| !m.selection_pending);
    assert_eq!(model.selection.len(), 321);
    for row in 0..model.count {
        assert_eq!(
            model.selection.contains(&model.entry_at(row).unwrap().name),
            row <= 320
        );
    }
}
#[derive(Default)]
struct SearchGate {
    entered: std::sync::atomic::AtomicBool,
    open: std::sync::Mutex<bool>,
    ready: std::sync::Condvar,
}
impl SearchGate {
    fn block(&self) {
        self.entered
            .store(true, std::sync::atomic::Ordering::Release);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.ready.wait(open).unwrap();
        }
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.ready.notify_all();
    }
}
fn search_output_uses_latest_presentation_and_rejects_replacement(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    let search_root = fixture.0.join("search");
    std::fs::create_dir(&search_root).unwrap();
    for i in 0..10 {
        std::fs::write(search_root.join(format!("hit{i:02}")), b"data").unwrap();
    }
    for grep in [false, true] {
        model.pin_mut().navigate_path(search_root.clone());
        if model.finding {
            model.pin_mut().find_clear();
        }
        model.pin_mut().set_filter_text(&QString::default());
        wait(app, model, |m| {
            m.settled && !m.filter_pending && !m.sort_pending && !m.frame.all.ranked
        });
        let gate = std::sync::Arc::new(SearchGate::default());
        let blocked = gate.clone();
        model
            .runtime
            .set_search_hook(Some(std::sync::Arc::new(move || blocked.block())));
        if grep {
            model.pin_mut().grep_start(QString::from("data"));
        } else {
            model.pin_mut().find_start(QString::from("hit"));
        }
        wait(app, model, |_| {
            gate.entered.load(std::sync::atomic::Ordering::Acquire)
        });
        model
            .pin_mut()
            .apply_sort_to(&QString::from("size"), false, false);
        model.pin_mut().set_filter_text(&QString::from("hit00"));
        model.pin_mut().set_show_hidden(true);
        model.pin_mut().queue_projection(2);
        model
            .pin_mut()
            .set_clip(vec![search_root.join("hit00")], ClipMode::Copy);
        let latest = model.request_revision;
        gate.release();
        wait(app, model, |m| {
            m.frame.all.ranked && m.frame.request == latest && !m.filter_pending && !m.sort_pending
        });
        assert!(model.finding);
        assert_eq!(model.count, 1);
        assert!(model.selection.is_empty());
        assert_eq!(model.frame.marked, 1);
        assert!(
            model
                .runtime
                .metrics()
                .search_metadata_entries
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 10
        );
        model.runtime.set_search_hook(None);
        // A new search supersedes a blocked output from the same directory.
        let gate = std::sync::Arc::new(SearchGate::default());
        let blocked = gate.clone();
        model
            .runtime
            .set_search_hook(Some(std::sync::Arc::new(move || blocked.block())));
        if grep {
            model.pin_mut().grep_start(QString::from("data"));
        } else {
            model.pin_mut().find_start(QString::from("hit"));
        }
        wait(app, model, |_| {
            gate.entered.load(std::sync::atomic::Ordering::Acquire)
        });
        model.runtime.set_search_hook(None);
        if grep {
            model.pin_mut().grep_start(QString::from("absent"));
        } else {
            model.pin_mut().find_start(QString::from("absent"));
        }
        gate.release();
        wait(app, model, |m| {
            m.frame.all.ranked && m.count == 0 && !m.filter_pending
        });
        let gate = std::sync::Arc::new(SearchGate::default());
        let blocked = gate.clone();
        model
            .runtime
            .set_search_hook(Some(std::sync::Arc::new(move || blocked.block())));
        if grep {
            model.pin_mut().grep_start(QString::from("data"));
        } else {
            model.pin_mut().find_start(QString::from("hit"));
        }
        wait(app, model, |_| {
            gate.entered.load(std::sync::atomic::Ordering::Acquire)
        });
        model.pin_mut().navigate_path(fixture.0.join("b"));
        gate.release();
        model.runtime.set_search_hook(None);
        wait(app, model, |m| {
            m.settled && m.key.as_path() == Some(fixture.0.join("b").as_path())
        });
        assert!(!model.frame.all.ranked);
        assert_eq!(model.count, 1);
    }
}

fn search_survives_automatic_origin_relist_and_job_completion(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    for grep in [false, true] {
        let root = fixture
            .0
            .join(if grep { "relist-grep" } else { "relist-find" });
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("hit00"), b"needle").unwrap();
        std::fs::write(root.join("other"), b"other").unwrap();
        let source = fixture.0.join(if grep {
            "copy-source-grep"
        } else {
            "copy-source-find"
        });
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("new-result.txt"), b"job payload").unwrap();
        if model.finding {
            model.pin_mut().find_clear();
            wait(app, model, |m| m.settled && !m.finding);
        }
        model.pin_mut().navigate_path(root.clone());
        model.pin_mut().set_filter_text(&QString::default());
        wait(app, model, |m| {
            m.settled
                && !m.filter_pending
                && !m.sort_pending
                && m.key.as_path() == Some(root.as_path())
        });
        // Real engine copy, isolated in-memory journal; completion is polled
        // through the production bridge while the search result is gated.
        model.pin_mut().rust_mut().job = Some(gust_ops::jobs::start(
            "qt-search-relist",
            gust_ops::jobs::Request::Copy {
                sources: vec![source.join("new-result.txt")],
                dest_dir: root.clone(),
            },
            gust_ops::journal::ConflictPolicy::Ask,
        ));
        let gate = std::sync::Arc::new(SearchGate::default());
        let blocked = gate.clone();
        model
            .runtime
            .set_search_hook(Some(std::sync::Arc::new(move || blocked.block())));
        if grep {
            model.pin_mut().grep_start(QString::from("needle"));
        } else {
            model.pin_mut().find_start(QString::from("hit"));
        }
        wait(app, model, |_| {
            gate.entered.load(std::sync::atomic::Ordering::Acquire)
        });
        let load = model.runtime.slots()["load_revision"].clone();
        let generation = model.generation;
        model.pin_mut().relist_focusing("new-result.txt".into());
        model.pin_mut().start_live_relist();
        assert_eq!(model.runtime.slots()["load_revision"], load);
        assert!(model.runtime.has_current_search(generation));
        assert!(!model.find_cancel.is_cancelled());
        assert_eq!(
            model.find_return.as_deref(),
            Some(std::ffi::OsStr::new("new-result.txt"))
        );
        wait(app, model, |m| m.job.as_ref().unwrap().is_finished());
        let completed: serde_json::Value =
            serde_json::from_str(&model.pin_mut().poll_job().to_string()).unwrap();
        assert_eq!(completed["state"], "Confirmed");
        assert!(!model.undo_label().to_string().is_empty());
        assert_eq!(
            std::fs::read(root.join("new-result.txt")).unwrap(),
            b"job payload"
        );
        assert_eq!(model.runtime.slots()["load_revision"], load);
        assert!(model.runtime.has_current_search(generation));
        assert!(model.live_dirty);
        gate.release();
        model.runtime.set_search_hook(None);
        wait(app, model, |m| {
            m.frame.all.ranked && m.count == 1 && !m.filter_pending && !m.sort_pending
        });
        assert!(model.finding);
        assert_eq!(model.entry_at(0).unwrap().name, "hit00");
        let listing = model.frame.all.listing;
        let token = CancelToken::new();
        for version in 1..=3 {
            qobject::DirModel::publish_thumbnails(
                model.pin_mut(),
                generation,
                listing,
                &token,
                vec![(
                    "hit00".into(),
                    format!("file:///search-cache-{version}.png"),
                )],
            );
        }
        assert!(model.thumbs.get(std::ffi::OsStr::new("hit00")).is_none());
        assert!(model.runtime.has_current_search(generation));
        assert!(model.live_dirty);
        assert_eq!(model.runtime.slots()["load_revision"], load);
        assert_eq!(
            model.find_return.as_deref(),
            Some(std::ffi::OsStr::new("new-result.txt"))
        );
        model.pin_mut().find_clear();
        wait(app, model, |m| {
            m.settled && !m.frame.all.ranked && m.count == 3 && !m.sort_pending
        });
        assert!(!model.runtime.has_current_search(model.generation));
        assert_eq!(model.entry_at(model.cursor).unwrap().name, "new-result.txt");
        assert!(!model.live_dirty);
    }
}

#[test]
fn actual_qt_search_operations_defer_refresh_until_clear() {
    const TEST: &str =
        "bridge::projection_qt_tests::actual_qt_search_operations_defer_refresh_until_clear";
    const ROOT_ENV: &str = "GUST_QT_SEARCH_OPERATIONS_ROOT";
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        let root = PathBuf::from(root);
        // The parent sets every path before this process starts. Never mutate
        // process-global XDG variables alongside parallel Rust tests.
        let trash = gust_ops::trash::TrashDir::at(root.join("data/Trash"));
        assert_eq!(gust_ops::trash::TrashDir::home(), Some(trash.clone()));
        let app = QCoreApplication::new();
        let app = app.as_ref().unwrap();
        for grep in [false, true] {
            for rename in [false, true] {
                search_operation_defers_refresh(app, &root, &trash, grep, rename);
            }
        }
        return;
    }

    use std::os::unix::fs::DirBuilderExt;
    let root = std::env::temp_dir().join(format!(
        "gust-search-operations-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // Exclusive creation: cleanup can only remove a directory this test owns.
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let fixture = Fixture(root);
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST, "--nocapture"])
        .env(ROOT_ENV, &fixture.0)
        .env("QT_QPA_PLATFORM", "offscreen");
    for (key, name) in [
        ("HOME", "home"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
    ] {
        let path = fixture.0.join(name);
        std::fs::create_dir(&path).unwrap();
        command.env(key, path);
    }
    let output = command.output().unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "isolated Qt search operation test failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn search_operation_defers_refresh(
    app: &QCoreApplication,
    sandbox: &Path,
    trash: &gust_ops::trash::TrashDir,
    grep: bool,
    rename: bool,
) {
    let case = format!(
        "{}-{}",
        if grep { "grep" } else { "find" },
        if rename { "rename" } else { "trash" }
    );
    let root = sandbox.join(&case);
    std::fs::create_dir(&root).unwrap();
    let raw = std::ffi::OsString::from_vec(b"hit-\xff\n.txt".to_vec());
    let original = root.join(&raw);
    let renamed = root.join("renamed.txt");
    let untouched = root.join("keep.txt");
    let bytes = b"needle: owned operation payload\n";
    std::fs::write(&original, bytes).unwrap();
    std::fs::write(&untouched, b"untouched bytes").unwrap();
    let identity = gust_ops::FileIdentity::of_path(&original).unwrap();
    let mut model = qobject::gust_test_new_model();
    qobject::gust_test_attach_probe(model.pin_mut());
    model.pin_mut().navigate_path(root.clone());
    wait(app, &model, |m| {
        m.settled && m.count == 2 && !m.sort_pending && !m.filter_pending
    });
    // Exercise operation-triggered refresh directly, independently of watch
    // timing. The production operation and Qt publication paths stay intact.
    model.pin_mut().stop_watch();
    if grep {
        model.pin_mut().grep_start(QString::from("needle"));
    } else {
        model.pin_mut().find_start(QString::from("hit"));
    }
    wait(app, &model, |m| {
        m.frame.all.ranked && m.count == 1 && !m.sort_pending && !m.filter_pending
    });
    let generation = model.generation;
    let slots = model.runtime.slots();
    let listing = model.frame.all.listing;
    let assert_deferred = |m: &qobject::DirModel| {
        assert!(m.finding && m.frame.all.ranked && m.live_dirty);
        assert!(!m.find_cancel.is_cancelled());
        assert!(m.runtime.has_current_search(generation));
        assert_eq!(m.generation, generation);
        assert_eq!(m.runtime.slots()["load_revision"], slots["load_revision"]);
        assert_eq!(
            m.runtime.slots()["search_revision"],
            slots["search_revision"]
        );
        assert_eq!(m.frame.all.listing, listing);
        assert_eq!(m.key, LocationKey::local(&root));
        assert_eq!(m.requested_key, m.key);
        assert_eq!(m.count, 1);
        // Labels escape these bytes; actions must keep resolving the old raw
        // target until clearing search publishes a fresh origin listing.
        assert_eq!(m.entry_at(0).unwrap().name, raw);
        assert_eq!(m.path_of_row(0), Some(original.clone()));
        assert_eq!(
            m.entry_token(0).to_string(),
            LocationKey::local(&original).to_token()
        );
        assert_eq!(std::fs::read(&untouched).unwrap(), b"untouched bytes");
        let signals = probe(m);
        assert_eq!(signals["ok"], true, "{signals}");
        assert_eq!(signals["open"], false);
    };
    let operate = |m: &mut cxx::UniquePtr<qobject::DirModel>| {
        if rename {
            m.pin_mut().rename_row(0, &QString::from("renamed.txt"))
        } else {
            m.pin_mut().trash_row(0)
        }
        .to_string()
    };
    assert_eq!(operate(&mut model), "");
    wait(app, &model, |m| !m.selection_pending);
    assert_deferred(&model);
    assert!(!original.exists());
    let moved = if rename {
        assert_eq!(model.find_return.as_deref(), renamed.file_name());
        renamed.clone()
    } else {
        assert_eq!(model.last_trashed_batch.len(), 1);
        let item = &model.last_trashed_batch[0].0;
        assert_eq!(item.original, original);
        assert!(item.info_file.starts_with(&trash.info));
        trash.files.join(&item.trashed_name)
    };
    assert_owned_operation_file(&moved, identity, bytes);
    let undo = model.undo_label().to_string();
    assert!(!undo.is_empty());
    let refusal = if rename {
        model
            .pin_mut()
            .rename_row(0, &QString::from("must-not-exist.txt"))
            .to_string()
    } else {
        model.pin_mut().trash_row(0).to_string()
    };
    if rename {
        assert_eq!(refusal, "That item is no longer there.");
    } else {
        assert!(
            refusal.contains(&std::io::Error::from_raw_os_error(2).to_string()),
            "missing target must be reported: {refusal}"
        );
        assert_eq!(model.last_trashed_batch.len(), 1);
    }
    wait(app, &model, |m| !m.selection_pending);
    assert_deferred(&model);
    assert!(!root.join("must-not-exist.txt").exists());
    assert_eq!(model.undo_label().to_string(), undo);
    assert_owned_operation_file(&moved, identity, bytes);

    // Undo during the same search also defers refresh and preserves identity.
    assert_eq!(model.pin_mut().undo_last().to_string(), "");
    assert_deferred(&model);
    assert_owned_operation_file(&original, identity, bytes);
    assert!(!moved.exists());
    if rename {
        assert_eq!(model.find_return.as_deref(), Some(raw.as_os_str()));
    }
    assert_eq!(model.undo_label().to_string(), "");

    // Operate once more so clear must replace the stale row with current
    // folder contents, including the latest deferred rename focus.
    assert_eq!(operate(&mut model), "");
    wait(app, &model, |m| !m.selection_pending);
    assert_deferred(&model);
    model.pin_mut().find_clear();
    wait(app, &model, |m| {
        m.settled
            && !m.finding
            && !m.frame.all.ranked
            && !m.sort_pending
            && !m.filter_pending
            && m.count == if rename { 2 } else { 1 }
    });
    assert_eq!(model.key, LocationKey::local(&root));
    assert_ne!(
        model.runtime.slots()["load_revision"],
        slots["load_revision"]
    );
    assert!(!model.runtime.has_current_search(model.generation));
    assert!(!model.live_dirty);
    assert!(!model.frame.all.iter().any(|entry| entry.name == raw));
    if rename {
        assert_eq!(model.path_of_row(model.cursor), Some(renamed.clone()));
        assert_owned_operation_file(&renamed, identity, bytes);
    }
    // Leave every operation undone; private Trash contains no payloads.
    assert_eq!(model.pin_mut().undo_last().to_string(), "");
    wait(app, &model, |m| {
        m.settled
            && !m.live_busy
            && m.count == 2
            && m.frame.all.iter().any(|entry| entry.name == raw)
    });
    assert_owned_operation_file(&original, identity, bytes);
    assert_eq!(std::fs::read(&untouched).unwrap(), b"untouched bytes");
    assert_eq!(model.undo_label().to_string(), "");
    if rename {
        assert_eq!(model.path_of_row(model.cursor), Some(original));
    } else {
        assert_eq!(std::fs::read_dir(&trash.files).unwrap().count(), 0);
    }
    let signals = probe(&model);
    assert_eq!(signals["ok"], true, "{signals}");
    assert_eq!(signals["open"], false);
    println!("{case}: raw target, unchanged search/load ticket, stale row/refusal, deferred Undo, clear/focus and restored identity/bytes passed");
    model.pin_mut().stop_watch();
    drop(model);
    app.process_events();
}

fn assert_owned_operation_file(path: &Path, identity: gust_ops::FileIdentity, bytes: &[u8]) {
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(gust_ops::FileIdentity::of_path(path).unwrap(), identity);
}

fn bounded_cache_renewal_and_cancelled_publications(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    let root = fixture.0.join("cache-renewal");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("photo.png"), b"source").unwrap();
    model.pin_mut().request_thumbnails(false);
    model.pin_mut().navigate_path(root.clone());
    wait(app, model, |m| {
        m.settled && !m.sort_pending && m.count == 1 && m.key.as_path() == Some(root.as_path())
    });
    model.pin_mut().stop_watch();
    let gen = model.generation;
    let listing = model.frame.all.listing;
    let live = CancelToken::new();
    let name = std::ffi::OsString::from("photo.png");
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        gen,
        listing,
        &live,
        vec![(name.clone(), String::new())],
    );
    assert!(model.thumbs.is_empty());
    let primary = String::from("file:///validated-primary.png?gust=one");
    let renewal = String::from("file:///validated-renewal.png?gust=two");
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        gen,
        listing,
        &live,
        vec![(name.clone(), primary.clone())],
    );
    for _ in 0..300 {
        qobject::DirModel::publish_thumbnails(
            model.pin_mut(),
            gen,
            listing,
            &live,
            vec![(name.clone(), primary.clone())],
        );
    }
    let old_thumbs = model.thumbs.clone();
    let first = old_thumbs.get(&name).unwrap();
    let old_sizes = model.dir_sizes.clone();
    let sized = |bytes| gust_core::dirsize::Sized {
        bytes,
        files: 1,
        complete: true,
    };
    qobject::DirModel::publish_dir_size(
        model.pin_mut(),
        gen,
        listing,
        &live,
        name.clone(),
        sized(11),
    );
    let stale = CancelToken::new();
    let queued = stale.clone();
    let complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = complete.clone();
    let n = name.clone();
    model
        .qt_thread_handle()
        .unwrap()
        .queue(move |mut model| {
            qobject::DirModel::publish_thumbnails(
                model.as_mut(),
                gen,
                listing,
                &queued,
                vec![(n.clone(), String::from("file:///stale.png"))],
            );
            qobject::DirModel::publish_dir_size(
                model.as_mut(),
                gen,
                listing,
                &queued,
                n,
                sized(99),
            );
            done.store(true, std::sync::atomic::Ordering::Release);
        })
        .unwrap();
    stale.cancel();
    wait(app, model, |_| {
        complete.load(std::sync::atomic::Ordering::Acquire)
    });
    assert_eq!(model.thumbs.get(&name), Some(&primary));
    assert_eq!(model.dir_sizes.get(&name).unwrap().bytes, 11);
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        gen,
        listing,
        &live,
        vec![(name.clone(), String::new())],
    );
    assert!(model.thumbs.get(&name).is_none());
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        gen,
        listing,
        &live,
        vec![(name.clone(), renewal.clone())],
    );
    for _ in 0..300 {
        qobject::DirModel::publish_thumbnails(
            model.pin_mut(),
            gen,
            listing,
            &live,
            vec![(name.clone(), renewal.clone())],
        );
    }
    old_sizes.remove(&name);
    qobject::DirModel::publish_dir_size(
        model.pin_mut(),
        gen,
        listing,
        &live,
        name.clone(),
        sized(22),
    );
    assert_eq!(model.frame.all.listing, listing);
    assert_eq!(model.thumbs.get(&name), Some(&renewal));
    assert_eq!(model.dir_sizes.get(&name).unwrap().bytes, 22);
    let second = old_thumbs.get(&name).unwrap();
    assert_eq!(first, &primary);
    // Synthetic callback publication proves scalar replacement, not production
    // reachability of same-backing size renewal after a filesystem mutation.
    let retained_copy = old_sizes.get(&name).unwrap();
    model.pin_mut().hold_live_updates(true);
    let dirty_before = model.live_dirty;
    for bytes in [33, 44, 55, 55, 66] {
        qobject::DirModel::publish_dir_size(
            model.pin_mut(),
            gen,
            listing,
            &live,
            name.clone(),
            sized(bytes),
        );
        assert_eq!(model.dir_sizes.get(&name), Some(sized(bytes)));
        assert_eq!(model.frame.all.listing, listing);
        assert_eq!(model.live_dirty, dirty_before);
    }
    assert_eq!(retained_copy, sized(22));
    // Both independent identity fences reject queued callbacks even with a
    // live token; the existing cancelled queued callback above covers token.
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished = done.clone();
    let n = name.clone();
    model
        .qt_thread_handle()
        .unwrap()
        .queue(move |mut model| {
            let token = CancelToken::new();
            for (g, l) in [
                (gen.wrapping_add(1), listing),
                (gen, listing.wrapping_add(1)),
            ] {
                qobject::DirModel::publish_dir_size(
                    model.as_mut(),
                    g,
                    l,
                    &token,
                    n.clone(),
                    sized(999),
                );
            }
            qobject::DirModel::publish_dir_size(
                model.as_mut(),
                gen,
                listing,
                &token,
                "unknown".into(),
                sized(999),
            );
            finished.store(true, std::sync::atomic::Ordering::Release);
        })
        .unwrap();
    wait(app, model, |_| {
        done.load(std::sync::atomic::Ordering::Acquire)
    });
    assert_eq!(model.dir_sizes.get(&name), Some(sized(66)));
    assert_eq!(model.dir_sizes.len(), 1);
    assert_eq!(model.live_dirty, dirty_before);
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        gen,
        listing,
        &live,
        vec![(name.clone(), String::from("file:///third.png?gust=three"))],
    );
    qobject::DirModel::publish_dir_size(
        model.pin_mut(),
        gen,
        listing,
        &live,
        name.clone(),
        sized(33),
    );
    assert!(model.thumbs.get(&name).is_none());
    assert_eq!(model.dir_sizes.get(&name), Some(sized(33)));
    assert!(std::sync::Arc::ptr_eq(&old_sizes, &model.dir_sizes));
    assert!(model.live_dirty);
    assert_eq!(model.frame.all.listing, listing);
    assert_eq!(first, &primary);
    assert_eq!(second, &renewal);
    model.pin_mut().hold_live_updates(false);
    wait(app, model, |m| {
        m.settled && m.frame.all.listing != listing && !m.live_busy
    });
    assert!(!std::sync::Arc::ptr_eq(&old_thumbs, &model.thumbs));
    assert!(!std::sync::Arc::ptr_eq(&old_sizes, &model.dir_sizes));
    let fresh_gen = model.generation;
    let fresh_listing = model.frame.all.listing;
    let third = String::from("file:///third.png?gust=three");
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        fresh_gen,
        fresh_listing,
        &live,
        vec![(name.clone(), third.clone())],
    );
    qobject::DirModel::publish_dir_size(
        model.pin_mut(),
        fresh_gen,
        fresh_listing,
        &live,
        name.clone(),
        sized(33),
    );
    assert_eq!(model.thumbs.get(&name), Some(&third));
    assert_eq!(model.dir_sizes.get(&name).unwrap().bytes, 33);
    qobject::DirModel::publish_thumbnails(
        model.pin_mut(),
        gen,
        listing,
        &live,
        vec![(name.clone(), String::from("file:///obsolete-listing.png"))],
    );
    qobject::DirModel::publish_dir_size(
        model.pin_mut(),
        gen,
        listing,
        &live,
        name.clone(),
        sized(99),
    );
    assert_eq!(model.thumbs.get(&name), Some(&third));
    assert_eq!(model.dir_sizes.get(&name).unwrap().bytes, 33);
    assert_eq!(first, &primary);
    assert_eq!(second, &renewal);
}
fn folder_size_job_and_undo_use_fresh_backing(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    let root = fixture.0.join("size-job");
    std::fs::create_dir_all(root.join("Backups")).unwrap();
    std::fs::write(root.join("Backups/old.txt"), b"old").unwrap();
    let source = fixture.0.join("size-source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("added.txt"), b"freshdata").unwrap();
    model.pin_mut().navigate_path(root.clone());
    wait(app, model, |m| {
        m.settled
            && m.key.as_path() == Some(root.as_path())
            && m.dir_sizes
                .get(std::ffi::OsStr::new("Backups"))
                .is_some_and(|s| s.bytes == 3)
    });
    model.pin_mut().stop_watch();
    let first = model.frame.all.listing;
    let old = model.dir_sizes.clone();
    model.pin_mut().rust_mut().job = Some(gust_ops::jobs::start(
        "qt-size-reload",
        gust_ops::jobs::Request::Copy {
            sources: vec![source.join("added.txt")],
            dest_dir: root.join("Backups"),
        },
        gust_ops::journal::ConflictPolicy::Ask,
    ));
    wait(app, model, |m| m.job.as_ref().unwrap().is_finished());
    let completed: serde_json::Value =
        serde_json::from_str(&model.pin_mut().poll_job().to_string()).unwrap();
    assert_eq!(completed["state"], "Confirmed");
    assert!(old.get(std::ffi::OsStr::new("Backups")).is_none());
    wait(app, model, |m| {
        m.settled
            && m.frame.all.listing != first
            && m.dir_sizes
                .get(std::ffi::OsStr::new("Backups"))
                .is_some_and(|s| s.bytes == 12)
    });
    assert!(!std::sync::Arc::ptr_eq(&old, &model.dir_sizes));
    assert_eq!(
        std::fs::read(root.join("Backups/added.txt")).unwrap(),
        b"freshdata"
    );
    let second = model.frame.all.listing;
    let copied = model.dir_sizes.clone();
    assert_eq!(model.pin_mut().undo_last().to_string(), "");
    assert!(copied.get(std::ffi::OsStr::new("Backups")).is_none());
    wait(app, model, |m| {
        m.settled
            && m.frame.all.listing != second
            && m.dir_sizes
                .get(std::ffi::OsStr::new("Backups"))
                .is_some_and(|s| s.bytes == 3)
    });
    assert!(!root.join("Backups/added.txt").exists());
    assert_eq!(std::fs::read(root.join("Backups/old.txt")).unwrap(), b"old");
    assert_eq!(
        std::fs::read(source.join("added.txt")).unwrap(),
        b"freshdata"
    );
}

// Runs under the existing test's one QCoreApplication, never another app on a
// parallel Rust test thread. Recorder injection does not mutate process env.
fn publication_attribution_reenters_qt(app: &QCoreApplication, fixture: &Fixture) {
    let mut recorder_ids = std::collections::HashSet::new();
    for enabled in [false, true, true] {
        let mut model = qobject::gust_test_new_model();
        model.pin_mut().rust_mut().callbacks = std::sync::Arc::new(
            crate::callback_metrics::Callbacks::with_publication_probe(enabled),
        );
        qobject::gust_test_attach_probe(model.pin_mut());
        // Every synchronous end observer performs nine calls, of which seven
        // are name-role reads (four name + three Qt DisplayRole).
        qobject::gust_test_publication_reads(model.pin_mut(), 4, 3, 2);
        model.pin_mut().navigate_path(fixture.0.join("a"));
        wait(app, &model, |m| {
            m.settled && m.count == 1001 && !m.sort_pending && !m.filter_pending
        });
        model.pin_mut().stop_watch();
        let before = probe(&model);
        let view_bounds = model.view_rows;
        model
            .pin_mut()
            .apply_sort_to(&QString::from("name"), false, false);
        wait(app, &model, |m| !m.sort_pending && !m.frame.sort.ascending);
        let after = probe(&model);
        assert_eq!(after["ok"], true, "{after}");
        assert_eq!(
            after["layouts"].as_u64().unwrap(),
            before["layouts"].as_u64().unwrap() + 1
        );
        let callbacks = model.callbacks.json();
        let events = &callbacks["publication_events"];
        let publications = model.callbacks.publications.json();
        let moving =
            publications["resets"].as_u64().unwrap() + publications["layouts"].as_u64().unwrap();
        assert_eq!(callbacks["data"]["count"], after["read_calls"]);
        assert_eq!(
            after["read_calls"].as_u64().unwrap(),
            after["read_rounds"].as_u64().unwrap() * 9
        );
        assert_eq!(after["read_rounds"], moving);
        assert_eq!(callbacks["publish_phases"]["end"]["count"], moving);
        assert_eq!(callbacks["publish_structural"]["count"], moving);
        assert_eq!(events["enabled"], enabled);
        assert_eq!(after["reentrant_enabled"], enabled);
        if enabled {
            let id = events["recorder_id"].as_u64().unwrap();
            assert!(id > 0 && recorder_ids.insert(id));
            assert_eq!(events["resets"]["count"], publications["resets"]);
            assert_eq!(events["layouts"]["count"], publications["layouts"]);
            // Diagnostics inside end_* sees only prior completed records. The
            // current correlated record is inserted after the callback work.
            assert_eq!(after["reentrant_events"].as_u64().unwrap() + 1, moving);
            for key in ["worst_end", "worst_total"] {
                let samples = events[key].as_array().unwrap();
                assert!(!samples.is_empty() && samples.len() <= 8);
                for sample in samples {
                    assert_eq!(sample["end_data_calls"], 9);
                    assert_eq!(sample["end_name_calls"], 7);
                    let data = sample["end_data_ns"].as_u64().unwrap();
                    let end = sample["end_ns"].as_u64().unwrap();
                    let total = sample["total_ns"].as_u64().unwrap();
                    assert!(data <= end && end <= total, "{sample}");
                    assert_eq!(sample["new_rows"], 1001);
                }
                let last = samples
                    .iter()
                    .find(|sample| sample["sequence"] == moving)
                    .unwrap();
                assert_eq!(last["kind"], "layout");
                assert_eq!(last["keep"], 2);
                assert_eq!(last["old_rows"], 1001);
                assert_eq!(last["shown_request_revision"], model.frame.request);
                assert_eq!(last["listing_revision"], model.frame.all.listing);
                assert_eq!(last["view_first"], view_bounds.0);
                assert_eq!(last["view_count"], view_bounds.1);
            }
        } else {
            assert!(model.callbacks.publication_events.is_none());
            assert_eq!(events["worst_end"], serde_json::json!([]));
            assert_eq!(events["worst_total"], serde_json::json!([]));
            assert_eq!(after["reentrant_events"], 0);
        }
        // A dry/held drain and selection-only publication have no end signal
        // and cannot fabricate a new correlated structural event.
        let prior_events = model.callbacks.json()["publication_events"].clone();
        model.pin_mut().hold_live_updates(true);
        model.pin_mut().drain_projection();
        assert_eq!(model.callbacks.json()["publication_events"], prior_events);
        model.pin_mut().hold_live_updates(false);
        model.pin_mut().focus_row(17);
        wait(app, &model, |m| !m.selection_pending);
        assert_eq!(model.callbacks.json()["publication_events"], prior_events);
        let marks_before = model.callbacks.publications.json()["marks"]
            .as_u64()
            .unwrap();
        assert!(!model.pin_mut().clip_copy(1).to_string().contains("nothing"));
        wait(app, &model, |m| {
            m.frame.marks_revision == m.runtime.marks_revision() && m.frame.marked == 1
        });
        model.pin_mut().clear_clip();
        wait(app, &model, |m| {
            m.frame.marks_revision == m.runtime.marks_revision() && m.frame.marked == 0
        });
        assert!(
            model.callbacks.publications.json()["marks"]
                .as_u64()
                .unwrap()
                >= marks_before + 2
        );
        assert_eq!(model.callbacks.json()["publication_events"], prior_events);
        println!("publication attribution enabled={enabled}: {}", callbacks);
        model.pin_mut().stop_watch();
        drop(model);
        app.process_events();
    }
}

fn newer_focus_survives_held_relist(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    let root = fixture.0.join("focus-intents");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("original"), b"original").unwrap();
    model.pin_mut().navigate_path(root.clone());
    wait(app, model, |m| m.settled && m.count == 1);
    model.pin_mut().stop_watch();
    std::fs::create_dir(root.join("A")).unwrap();
    model.pin_mut().relist_focusing("A".into());
    let older_revision = model.focus_revision;
    model.pin_mut().hold_live_updates(true);
    // Hold the old completion at the actual publication boundary.
    wait(app, model, |m| m.runtime.slots()["completion"] == true);
    std::fs::create_dir(root.join("B")).unwrap();
    model.pin_mut().relist_focusing("B".into());
    assert!(model.focus_revision > older_revision);
    assert!(model.live_busy && model.live_dirty);
    assert_eq!(model.focus_name.as_deref(), Some(std::ffi::OsStr::new("B")));
    model.pin_mut().hold_live_updates(false);
    wait(app, model, |m| {
        !m.live_busy
            && !m.live_dirty
            && m.count == 3
            && m.focus_name.is_none()
            && m.path_of_row(m.cursor) == Some(root.join("B"))
    });
    assert_eq!(model.entry_name(model.cursor).to_string(), "B");
    model.pin_mut().stop_watch();
}

fn equal_viewport_filter_measures_new_folders(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    let root = fixture.0.join("filtered-sizes");
    std::fs::create_dir(&root).unwrap();
    for group in ["alpha", "beta"] {
        for i in 0..100 {
            let dir = root.join(format!("{group}-{i:03}"));
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(
                dir.join("payload"),
                vec![0; if group == "alpha" { 13 } else { 29 }],
            )
            .unwrap();
        }
    }
    model.pin_mut().navigate_path(root.clone());
    wait(app, model, |m| m.settled && m.count == 200);
    model.pin_mut().stop_watch();
    model.pin_mut().set_filter_text(&QString::from("alpha"));
    model.pin_mut().want_rows(0, 2);
    wait(app, model, |m| {
        !m.filter_pending
            && m.count == 100
            && m.dir_sizes.get(std::ffi::OsStr::new("alpha-002")).is_some()
    });
    let backing = model.all.clone();
    let bounds = gust_core::dirsize::rows_in_view(0, 2, model.visible.len());
    assert!(model
        .dir_sizes
        .get(std::ffi::OsStr::new("beta-000"))
        .is_none());
    // Drain the completed pass, then repeat exactly the numeric bounds.
    wait(app, model, |m| !m.size_work.is_active());
    model.pin_mut().set_filter_text(&QString::from("beta"));
    wait(app, model, |m| {
        !m.filter_pending
            && m.count == 100
            && m.dir_sizes.get(std::ffi::OsStr::new("beta-002")).is_some()
    });
    assert!(std::sync::Arc::ptr_eq(&backing, &model.all));
    assert_eq!(
        gust_core::dirsize::rows_in_view(0, 2, model.visible.len()),
        bounds
    );
    for i in 0..3 {
        assert_eq!(
            model
                .dir_sizes
                .get(std::ffi::OsStr::new(&format!("beta-{i:03}")))
                .unwrap()
                .bytes,
            29
        );
    }
    model.pin_mut().set_filter_text(&QString::from(""));
    wait(app, model, |m| !m.filter_pending);
}

fn typed_destination_tokens_match_actual_navigation(
    app: &QCoreApplication,
    model: &mut cxx::UniquePtr<qobject::DirModel>,
    fixture: &Fixture,
) {
    let destination = fixture.0.join("Documents");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("file"), b"synthetic destination").unwrap();
    let expected = LocationKey::local(&destination).to_token();
    let aliases = [
        format!("{}/Documents/", fixture.0.display()),
        format!("{}//Documents//", fixture.0.display()),
        format!("{}/./Documents/./", fixture.0.display()),
    ];
    // Parse the real GTK representation; its path retains the trailing slash.
    let bookmark_text = format!(
        "{}/ Documents\n",
        gust_core::clipboard::path_to_uri(&destination)
    );
    let bookmarks = gust_core::bookmarks::parse(&bookmark_text);
    assert_eq!(bookmarks.len(), 1);
    assert!(bookmarks[0]
        .path
        .as_os_str()
        .as_encoded_bytes()
        .ends_with(b"/"));
    for text in std::iter::once(bookmarks[0].path.to_string_lossy().into_owned()).chain(aliases) {
        let text = QString::from(&text);
        assert_eq!(model.token_for(&text).to_string(), expected);
        model.pin_mut().navigate(&text);
        assert_eq!(model.requested_key.to_token(), expected);
        wait(app, model, |m| {
            m.settled && m.location_token().to_string() == expected
        });
        assert_eq!(model.path_of_row(0), Some(destination.join("file")));
    }
    // Parent components retain their meaning even when a component is a symlink.
    let parent_text = format!("{}/link/../Documents/", fixture.0.display());
    assert_eq!(
        model.token_for(&QString::from(&parent_text)).to_string(),
        LocationKey::local(fixture.0.join("link/../Documents")).to_token()
    );
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let expected_home: PathBuf = Path::new(&home).components().collect();
        assert_eq!(
            model.token_for(&QString::from("~/./")).to_string(),
            LocationKey::local(expected_home).to_token()
        );
    }
    model.pin_mut().stop_watch();
}
