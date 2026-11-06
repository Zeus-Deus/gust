use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Fixture {
    files: BTreeMap<Document, String>,
    writes: Vec<(Document, String)>,
    blocked: bool,
    entered: bool,
    fail: bool,
    reading_blocked: bool,
    blocked_read: Option<Document>,
    failed_reads: std::collections::BTreeSet<Document>,
}
#[derive(Default)]
struct Control {
    state: Mutex<Fixture>,
    gate: Condvar,
}
struct Fake(Arc<Control>);
impl Backend for Fake {
    fn read(&mut self, document: Document) -> Result<Option<String>, String> {
        let mut state = lock(&self.0.state);
        while state.reading_blocked || state.blocked_read == Some(document) {
            state.entered = true;
            self.0.gate.notify_all();
            state = self.0.gate.wait(state).unwrap();
        }
        if state.failed_reads.contains(&document) {
            return Err("injected document read failure".into());
        }
        Ok(state.files.get(&document).cloned())
    }
    fn publish(
        &mut self,
        document: Document,
        previous: Option<&str>,
        text: &str,
    ) -> Result<(), String> {
        let mut state = lock(&self.0.state);
        state.entered = true;
        self.0.gate.notify_all();
        while state.blocked {
            state = self.0.gate.wait(state).unwrap();
        }
        if state.fail {
            return Err("injected sync failure".into());
        }
        if state.files.get(&document).map(String::as_str) != previous {
            return Err("preferences conflict: changed during write".into());
        }
        state.files.insert(document, text.to_owned());
        state.writes.push((document, text.to_owned()));
        Ok(())
    }
}
fn fixture() -> (Preferences, Arc<Control>) {
    let control = Arc::new(Control::default());
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    wait(&owner, |s| s.ready);
    (owner, control)
}
fn wait(owner: &Preferences, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut state = lock(&owner.shared.state);
    while !predicate(&state.snapshot) {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("preference state deadline");
        state = owner
            .shared
            .changed
            .wait_timeout(state, remaining)
            .unwrap()
            .0;
    }
    state.snapshot.clone()
}
fn entered(control: &Control) {
    let mut state = lock(&control.state);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !state.entered {
        state = control
            .gate
            .wait_timeout(
                state,
                deadline
                    .checked_duration_since(Instant::now())
                    .expect("backend deadline"),
            )
            .unwrap()
            .0;
    }
}
fn release(control: &Control) {
    {
        let mut state = lock(&control.state);
        state.blocked = false;
        state.reading_blocked = false;
        state.blocked_read = None;
    }
    control.gate.notify_all();
}
fn setting(key: &str, value: &str) -> Mutation {
    Mutation::Setting(key.into(), value.into())
}
fn key(path: &str) -> LocationKey {
    LocationKey::local(Path::new(path))
}

#[test]
fn blocked_load_exposes_readiness_and_rejects_early_mutations() {
    let control = Arc::new(Control::default());
    lock(&control.state).reading_blocked = true;
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    entered(&control);
    assert!(!owner.snapshot().ready);
    assert!(owner
        .admit(setting("sidebar", "false"))
        .unwrap_err()
        .contains("loading"));
    release(&control);
    wait(&owner, |s| s.ready);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn blocked_io_keeps_admission_cached_reads_and_flush_nonblocking_and_coalesces() {
    let (owner, control) = fixture();
    lock(&control.state).blocked = true;
    owner.admit(setting("sidebar", "false")).unwrap();
    entered(&control);
    let start = Instant::now();
    for i in 0..1000 {
        owner
            .admit(setting(
                "show_hidden",
                if i % 2 == 0 { "false" } else { "true" },
            ))
            .unwrap();
        owner.flush();
    }
    owner.admit(setting("density", "roomy")).unwrap();
    let pending = owner.snapshot();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(pending.pending);
    assert!(pending.settings.show_hidden);
    assert!(!pending.settings.sidebar);
    assert_eq!(pending.saved_revision, 0);
    assert_eq!(lock(&owner.shared.state).intents.len(), 3);
    release(&control);
    let done = wait(&owner, |s| s.saved_revision == pending.revision);
    assert!(!done.pending);
    assert!(done.error.is_empty());
    let state = lock(&control.state);
    assert!(
        state.writes.len() <= 2,
        "one in-flight document and one coalesced document"
    );
    let stored = Settings::from_text(&state.files[&Document::Settings]);
    assert_eq!(stored, pending.settings);
    drop(state);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn failing_write_keeps_old_bytes_optimistic_intents_and_ordered_ack_until_retry() {
    let (owner, control) = fixture();
    let old = Settings::default().to_text();
    {
        let mut s = lock(&control.state);
        s.files.insert(Document::Settings, old.clone());
        s.fail = true;
    }
    let first = owner.admit(setting("show_hidden", "true")).unwrap();
    let failed = wait(&owner, |s| !s.error.is_empty());
    assert_eq!(failed.saved_revision, 0);
    assert!(failed.pending && failed.settings.show_hidden);
    assert_eq!(lock(&control.state).files[&Document::Settings], old);
    lock(&control.state).fail = false;
    let second = owner.admit(setting("density", "compact")).unwrap();
    assert!(second > first);
    owner.flush();
    let saved = wait(&owner, |s| s.saved_revision == second);
    assert!(saved.error.is_empty());
    assert!(saved.settings.show_hidden);
    assert_eq!(saved.settings.density_name(), "compact");
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn external_edits_merge_unrelated_fields_locations_and_unknown_keys() {
    let (owner, control) = fixture();
    lock(&control.state).blocked = true;
    owner.admit(setting("show_hidden", "true")).unwrap();
    entered(&control);
    // The in-flight source changes after its read: publication must conflict.
    {
        let mut s = lock(&control.state);
        s.files.insert(
            Document::Settings,
            "sidebar = false\ndensity = \"roomy\"\nfuture = \"retained\"\n".into(),
        );
    }
    release(&control);
    wait(&owner, |s| s.error.contains("conflict"));
    owner.flush();
    wait(&owner, |s| !s.pending);
    let snapshot = owner.snapshot();
    assert!(snapshot.settings.show_hidden);
    assert!(!snapshot.settings.sidebar);
    assert_eq!(snapshot.settings.density_name(), "roomy");
    assert!(lock(&control.state).files[&Document::Settings].contains("future = \"retained\""));
    let external = key("/external");
    let mine = key("/mine");
    let mut sorts = SortMemory::new();
    sorts.remember(&external, SortField::Size, false);
    let mut views = ViewMemory::new();
    views.remember(&external, ViewMode::Columns);
    {
        let mut s = lock(&control.state);
        s.files.insert(Document::Sorts, sorts.to_text());
        s.files.insert(Document::Views, views.to_text());
    }
    owner
        .admit(Mutation::Sort(mine.clone(), SortField::Kind, true))
        .unwrap();
    let revision = owner
        .admit(Mutation::View(mine.clone(), ViewMode::Grid))
        .unwrap();
    wait(&owner, |s| s.saved_revision >= revision);
    let snapshot = owner.snapshot();
    assert_eq!(
        snapshot.sorts.get(&external),
        Some((SortField::Size, false))
    );
    assert_eq!(snapshot.sorts.get(&mine), Some((SortField::Kind, true)));
    assert_eq!(snapshot.views.get(&external), Some(ViewMode::Columns));
    assert_eq!(snapshot.views.get(&mine), Some(ViewMode::Grid));
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn tabs_share_state_and_lost_receivers_do_not_lose_acknowledgements() {
    let (owner, control) = fixture();
    let updates = Arc::new(AtomicUsize::new(0));
    let count = updates.clone();
    let subscriber = owner
        .subscribe(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            true
        }))
        .unwrap();
    let dead = owner.subscribe(Arc::new(|| false)).unwrap();
    let revision = owner
        .admit(Mutation::View(key("/shared"), ViewMode::Columns))
        .unwrap();
    wait(&owner, |s| s.saved_revision == revision);
    assert!(updates.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        owner.snapshot().views.get(&key("/shared")),
        Some(ViewMode::Columns)
    );
    assert_eq!(lock(&owner.shared.state).subscribers.len(), 1);
    drop(dead);
    drop(subscriber);
    assert!(lock(&owner.shared.state).subscribers.is_empty());
    assert!(lock(&control.state).files.contains_key(&Document::Views));
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn revision_order_survives_a_newer_same_field_value_during_inflight_write() {
    let (owner, control) = fixture();
    lock(&control.state).blocked = true;
    let first = owner.admit(setting("sidebar", "false")).unwrap();
    entered(&control);
    let second = owner.admit(setting("sidebar", "true")).unwrap();
    let third = owner.admit(setting("show_hidden", "true")).unwrap();
    assert!(first < second && second < third);
    release(&control);
    wait(&owner, |s| s.saved_revision == third);
    let s = lock(&control.state);
    assert!(!Settings::from_text(&s.writes[0].1).sidebar);
    assert!(Settings::from_text(&s.writes.last().unwrap().1).sidebar);
    assert!(Settings::from_text(&s.writes.last().unwrap().1).show_hidden);
    drop(s);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn session_is_serialized_after_restore_setting_and_disabled_sessions_are_refused() {
    let (owner, control) = fixture();
    let session = Session::from_parts(&[(key("/tabs").to_token(), "grid".into())], 0);
    assert!(owner.admit(Mutation::Session(session.clone())).is_err());
    lock(&control.state).blocked = true;
    owner.admit(setting("restore_tabs", "true")).unwrap();
    entered(&control);
    let target = owner.admit(Mutation::Session(session.clone())).unwrap();
    release(&control);
    wait(&owner, |s| s.saved_revision == target);
    let s = lock(&control.state);
    let session_at = s
        .writes
        .iter()
        .position(|(d, _)| *d == Document::Session)
        .unwrap();
    assert!(s.writes[..session_at]
        .iter()
        .any(|(d, t)| *d == Document::Settings && Settings::from_text(t).restore_tabs));
    assert_eq!(Session::from_text(&s.files[&Document::Session]), session);
    drop(s);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn shutdown_deadline_does_not_join_blocked_worker_or_claim_saved() {
    let (owner, control) = fixture();
    lock(&control.state).blocked = true;
    owner.admit(setting("sidebar", "false")).unwrap();
    entered(&control);
    let start = Instant::now();
    assert!(!owner.shutdown(Duration::from_millis(20)));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(owner.snapshot().pending);
    assert!(owner.admit(setting("density", "compact")).is_err());
    release(&control);
}

#[test]
fn idle_refresh_observes_external_changes_and_admission_is_bounded() {
    let (owner, control) = fixture();
    lock(&control.state)
        .files
        .insert(Document::Settings, "sidebar = false\n".into());
    owner.refresh();
    wait(&owner, |s| !s.settings.sidebar);
    lock(&control.state).blocked = true;
    owner.admit(setting("show_hidden", "true")).unwrap();
    entered(&control);
    for i in 1..MAX_PENDING {
        owner
            .admit(Mutation::View(key(&format!("/folder-{i}")), ViewMode::Grid))
            .unwrap();
    }
    let before = owner.snapshot().revision;
    assert!(owner
        .admit(Mutation::View(key("/overflow"), ViewMode::Grid))
        .unwrap_err()
        .contains("full"));
    assert_eq!(owner.snapshot().revision, before);
    // Even saturated, a compatible key replacement remains admissible.
    owner.admit(setting("show_hidden", "false")).unwrap();
    assert_eq!(lock(&owner.shared.state).intents.len(), MAX_PENDING);
    release(&control);
    assert!(owner.shutdown(Duration::from_secs(5)));
}

fn disk_fixture() -> (PathBuf, Disk) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gust-private-preferences-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).unwrap();
    let disk = Disk {
        paths: Some([(Document::Settings, dir.join("settings.toml"))].into()),
        batch: None,
    };
    (dir, disk)
}

// Another test can fork while this test holds a flock descriptor. CLOEXEC
// closes the child's inherited copy at exec, not at fork, so dropping our copy
// need not make immediate reacquisition succeed. Retry only the backend's exact
// nonblocking contention refusal; never turn unsafe paths or other errors into
// an eventual success. This is a test driver, not a worker retry policy.
fn retry_busy<T>(mut attempt: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match attempt() {
            Err(error)
                if error == "preferences busy in another Gust process; retry flush"
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => return result,
        }
    }
}

fn flush_after_busy(owner: &Preferences, target: u64) {
    retry_busy(|| {
        let attempt = owner.snapshot().completed_attempt;
        owner.flush();
        let completed = wait(owner, |s| s.completed_attempt > attempt);
        if completed.saved_revision >= target {
            Ok(())
        } else {
            Err(completed.error)
        }
    })
    .unwrap();
}

#[test]
fn busy_retry_does_not_retry_security_or_other_failures() {
    for error in [
        "unsafe preferences lock file ownership, type or permissions",
        "preferences busy with an unrelated error",
        "injected sync failure",
    ] {
        let mut attempts = 0;
        let result: Result<(), String> = retry_busy(|| {
            attempts += 1;
            Err(error.into())
        });
        assert_eq!(result.unwrap_err(), error);
        assert_eq!(attempts, 1);
    }
}

#[test]
fn disk_atomic_publication_refuses_changed_bytes_and_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mut disk) = disk_fixture();
    let path = dir.join("settings.toml");
    retry_busy(|| disk.begin_write(&[Document::Settings])).unwrap();
    assert!(
        rustix::io::fcntl_getfd(&disk.batch.as_ref().unwrap()._locks[0])
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    disk.publish(Document::Settings, None, "sidebar = false\n")
        .unwrap();
    let old = disk.read(Document::Settings).unwrap().unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::write(&path, "sidebar = true\n").unwrap();
    assert!(disk
        .publish(Document::Settings, Some(&old), "show_hidden = true\n")
        .unwrap_err()
        .contains("conflict"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "sidebar = true\n");
    let link = dir.join("link.toml");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    disk.end_write();
    disk.paths
        .as_mut()
        .unwrap()
        .insert(Document::Settings, link);
    assert!(disk.read(Document::Settings).is_err());
    retry_busy(|| disk.begin_write(&[Document::Settings])).unwrap();
    assert!(disk.publish(Document::Settings, None, "new").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "sidebar = true\n");
    assert!(std::fs::read_dir(&dir).unwrap().all(|f| !f
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn disk_rejects_symlink_ancestors_and_nonregular_files() {
    let (dir, mut disk) = disk_fixture();
    let nested = dir.join("real");
    std::fs::create_dir(&nested).unwrap();
    std::os::unix::fs::symlink(&nested, dir.join("redirect")).unwrap();
    disk.paths
        .as_mut()
        .unwrap()
        .insert(Document::Settings, dir.join("redirect/settings.toml"));
    assert!(disk.read(Document::Settings).is_err());
    assert!(disk.publish(Document::Settings, None, "private").is_err());
    assert!(!nested.join("settings.toml").exists());
    disk.paths
        .as_mut()
        .unwrap()
        .insert(Document::Settings, nested);
    assert!(disk.read(Document::Settings).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn actual_staging_sync_failure_retains_old_bytes_and_removes_only_owned_stage() {
    let (dir, mut disk) = disk_fixture();
    retry_busy(|| disk.begin_write(&[Document::Settings])).unwrap();
    disk.publish(Document::Settings, None, "sidebar = false\n")
        .unwrap();
    let target = dir.join("settings.toml");
    let old = std::fs::read_to_string(&target).unwrap();
    let sentinel = dir.join(".gust-prefs-unrelated.tmp");
    std::fs::write(&sentinel, "untouched").unwrap();
    let result = publish_file(
        &target,
        Document::Settings,
        Some(&old),
        "show_hidden = true\n",
        |_file| Err(io::Error::other("injected staged-file sync failure")),
    );
    assert!(result.unwrap_err().contains("sync failure"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), old);
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "untouched");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn subscription_bound_and_drop_release_admission_without_leaking_receivers() {
    let (owner, _) = fixture();
    let subscriptions: Vec<_> = (0..MAX_SUBSCRIBERS)
        .map(|_| owner.subscribe(Arc::new(|| true)).unwrap())
        .collect();
    assert!(owner.subscribe(Arc::new(|| true)).is_err());
    drop(subscriptions);
    assert!(lock(&owner.shared.state).subscribers.is_empty());
    let subscription = owner.subscribe(Arc::new(|| true)).unwrap();
    drop(subscription);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn future_location_values_are_preserved_while_known_cache_entries_stay_bounded() {
    let future = key("/future").to_token();
    let mut texts: BTreeMap<_, _> = [(
        Document::Views,
        Some(format!("{future} = \"future-mode\"\n")),
    )]
    .into();
    let mut intents = BTreeMap::new();
    for i in 0..505 {
        let key = key(&format!("/known-{i}"));
        let mutation = Mutation::View(key, ViewMode::Rows);
        intents.insert(mutation.key(), (i, mutation));
    }
    let snapshot = reconcile(&mut texts, &intents).unwrap();
    assert_eq!(snapshot.views.len(), 500);
    let text = texts[&Document::Views].as_ref().unwrap();
    assert!(text.contains(&format!("{future} = \"future-mode\"")));
    assert_eq!(ViewMemory::from_text(text).len(), 500);
}

#[test]
fn pre_qt_renderer_wait_does_not_wait_for_blocked_other_preferences() {
    let control = Arc::new(Control::default());
    {
        let mut state = lock(&control.state);
        state
            .files
            .insert(Document::Settings, "renderer = \"software\"\n".into());
        state.blocked_read = Some(Document::Sorts);
    }
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    entered(&control);
    let start = Instant::now();
    assert_eq!(owner.renderer_with_env(|_| None), RendererChoice::Software);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!owner.snapshot().ready);
    release(&control);
    wait(&owner, |s| s.ready);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn blocked_settings_read_times_out_and_late_renderer_does_not_replace_the_choice() {
    let control = Arc::new(Control::default());
    {
        let mut state = lock(&control.state);
        state
            .files
            .insert(Document::Settings, "renderer = \"software\"\n".into());
        state.blocked_read = Some(Document::Settings);
    }
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    entered(&control);
    let start = Instant::now();
    assert_eq!(owner.renderer_with_env(|_| None), RendererChoice::Auto);
    assert!(start.elapsed() >= RENDERER_WAIT);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!owner.snapshot().ready);
    release(&control);
    wait(&owner, |s| s.ready);
    assert_eq!(owner.snapshot().settings.renderer, RendererChoice::Software);
    assert_eq!(owner.renderer_with_env(|_| None), RendererChoice::Auto);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn explicit_renderer_overrides_skip_even_the_bounded_settings_wait() {
    for (key, value) in [
        ("GUST_RENDERER", "software"),
        ("GUST_RENDERER", "gpu"),
        ("GUST_RENDERER", "hardware"),
        ("QT_QUICK_BACKEND", "software"),
        ("QSG_RHI_BACKEND", "vulkan"),
    ] {
        let control = Arc::new(Control::default());
        lock(&control.state).blocked_read = Some(Document::Settings);
        let owner = Preferences::start(Box::new(Fake(control.clone())));
        entered(&control);
        let start = Instant::now();
        assert_eq!(
            owner.renderer_with_env(|k| (k == key).then(|| value.into())),
            RendererChoice::Auto
        );
        assert!(
            start.elapsed() < RENDERER_WAIT,
            "explicit override waited: {key}"
        );
        assert!(!owner.snapshot().ready);
        release(&control);
        wait(&owner, |s| s.ready);
        assert!(owner.shutdown(Duration::from_secs(1)));
    }
}

#[test]
fn rejected_choices_survive_ack_refresh_and_new_subscriber_until_reselected() {
    let (owner, control) = fixture();
    lock(&control.state).blocked = true;
    owner.admit(setting("show_hidden", "true")).unwrap();
    entered(&control);
    for i in 1..MAX_PENDING {
        owner
            .admit(Mutation::View(key(&format!("/held-{i}")), ViewMode::Grid))
            .unwrap();
    }
    let rejected = Mutation::View(key("/rejected"), ViewMode::Columns);
    let revision = owner.snapshot().revision;
    assert!(owner.admit(rejected.clone()).is_err());
    let failed = owner.snapshot();
    assert_eq!(failed.revision, revision, "refusal has no saved revision");
    assert!(failed.admission_error.contains("reselect"));
    assert_eq!(failed.views.get(&key("/rejected")), None);
    release(&control);
    wait(&owner, |s| !s.pending);
    owner.refresh();
    let subscription = owner.subscribe(Arc::new(|| true)).unwrap();
    let switched_tab = owner.snapshot();
    assert!(
        !switched_tab.pending,
        "rejected choices are not retained intents"
    );
    assert!(switched_tab.error.contains("Choice not accepted"));
    owner.admit(setting("sidebar", "false")).unwrap();
    wait(&owner, |s| !s.pending);
    assert!(
        !owner.snapshot().admission_error.is_empty(),
        "unrelated success cannot clear a rejection"
    );
    let target = owner.admit(rejected).unwrap();
    wait(&owner, |s| s.saved_revision >= target);
    assert!(owner.snapshot().error.is_empty());
    assert!(owner.snapshot().admission_error.is_empty());
    assert_eq!(
        owner.snapshot().views.get(&key("/rejected")),
        Some(ViewMode::Columns)
    );
    drop(subscription);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn rejected_setting_validation_and_subscription_recover_only_on_explicit_retry() {
    let (owner, _) = fixture();
    assert!(owner.admit(setting("density", "invalid")).is_err());
    let target = owner.admit(setting("sidebar", "false")).unwrap();
    wait(&owner, |s| s.saved_revision >= target);
    assert!(owner.snapshot().error.contains("Choice not accepted"));
    owner.admit(setting("density", "roomy")).unwrap();
    wait(&owner, |s| !s.pending);
    assert!(owner.snapshot().error.is_empty());
    let subscriptions: Vec<_> = (0..MAX_SUBSCRIBERS)
        .map(|_| owner.subscribe(Arc::new(|| true)).unwrap())
        .collect();
    assert!(owner.subscribe(Arc::new(|| true)).is_err());
    owner.refresh();
    assert!(owner.snapshot().admission_error.contains("receivers"));
    drop(subscriptions);
    assert!(owner.snapshot().admission_error.contains("receivers"));
    let retried = owner.subscribe(Arc::new(|| true)).unwrap();
    assert!(owner.snapshot().admission_error.is_empty());
    drop(retried);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

// Real disk backend with no integration/restore probes of host paths.
struct SyntheticDisk(Disk);
impl Backend for SyntheticDisk {
    fn begin_write(&mut self, docs: &[Document]) -> Result<(), String> {
        self.0.begin_write(docs)
    }
    fn end_write(&mut self) {
        self.0.end_write();
    }
    fn read(&mut self, doc: Document) -> Result<Option<String>, String> {
        self.0.read(doc)
    }
    fn publish(&mut self, doc: Document, old: Option<&str>, text: &str) -> Result<(), String> {
        self.0.publish(doc, old, text)
    }
}

#[test]
fn read_only_load_and_refresh_never_create_parents_or_lock_files() {
    let (dir, mut disk) = disk_fixture();
    let missing = dir.join("missing/settings.toml");
    disk.paths
        .as_mut()
        .unwrap()
        .insert(Document::Settings, missing);
    let owner = Preferences::start(Box::new(SyntheticDisk(disk)));
    wait(&owner, |s| s.ready);
    owner.refresh();
    assert!(owner.shutdown(Duration::from_secs(1)));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn disk_lock_refuses_symlink_hardlink_and_public_lock_files() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mut disk) = disk_fixture();
    let sentinel = dir.join("sentinel");
    std::fs::write(&sentinel, "untouched").unwrap();
    std::os::unix::fs::symlink(&sentinel, dir.join(LOCK_NAME)).unwrap();
    assert!(disk.begin_write(&[Document::Settings]).is_err());
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "untouched");
    std::fs::remove_file(dir.join(LOCK_NAME)).unwrap();
    std::fs::hard_link(&sentinel, dir.join(LOCK_NAME)).unwrap();
    assert!(disk.begin_write(&[Document::Settings]).is_err());
    std::fs::remove_file(dir.join(LOCK_NAME)).unwrap();
    std::fs::write(dir.join(LOCK_NAME), "").unwrap();
    std::fs::set_permissions(dir.join(LOCK_NAME), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(disk.begin_write(&[Document::Settings]).is_err());
    std::fs::set_permissions(dir.join(LOCK_NAME), std::fs::Permissions::from_mode(0o600)).unwrap();
    retry_busy(|| disk.begin_write(&[Document::Settings])).unwrap();
    disk.end_write();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn independent_owners_merge_different_fields_after_real_lock_contention() {
    let (dir, mut first) = disk_fixture();
    retry_busy(|| first.begin_write(&[Document::Settings])).unwrap();
    let second = Disk {
        paths: first.paths.clone(),
        batch: None,
    };
    let owner = Preferences::start(Box::new(SyntheticDisk(second)));
    wait(&owner, |s| s.ready);
    let target = owner.admit(setting("sidebar", "false")).unwrap();
    let refused = wait(&owner, |s| s.error.contains("busy"));
    assert!(owner.snapshot().pending);
    assert_eq!(owner.snapshot().saved_revision, 0);
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(
        owner.snapshot().completed_attempt,
        refused.completed_attempt,
        "contention has no timed retry loop"
    );
    first
        .publish(Document::Settings, None, "show_hidden = true\nfuture = 7\n")
        .unwrap();
    first.end_write();
    // Refusal is stable while idle: there is no background retry loop.
    assert!(owner.snapshot().pending);
    flush_after_busy(&owner, target);
    let text = std::fs::read_to_string(dir.join("settings.toml")).unwrap();
    let settings = Settings::from_text(&text);
    assert!(!settings.sidebar && settings.show_hidden);
    assert!(text.contains("future = 7"));
    assert!(owner.snapshot().error.is_empty());
    assert!(owner.shutdown(Duration::from_secs(1)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cooperating_process_lock_refuses_then_retries_without_losing_fields() {
    use std::process::{Command, Stdio};
    const ENV: &str = "GUST_SYNTHETIC_PREFERENCES_CHILD";
    if let Some(dir) = std::env::var_os(ENV).map(PathBuf::from) {
        let mut disk = Disk {
            paths: Some([(Document::Settings, dir.join("settings.toml"))].into()),
            batch: None,
        };
        retry_busy(|| disk.begin_write(&[Document::Settings])).unwrap();
        let old = disk.read(Document::Settings).unwrap();
        std::fs::write(dir.join("locked"), "ready").unwrap();
        let mut release = String::new();
        io::stdin().read_line(&mut release).unwrap();
        assert_eq!(release.trim(), "release");
        let mut texts = [(Document::Settings, old.clone())].into();
        let mutation = setting("show_hidden", "true");
        reconcile(&mut texts, &[(mutation.key(), (1, mutation))].into()).unwrap();
        disk.publish(
            Document::Settings,
            old.as_deref(),
            texts[&Document::Settings].as_deref().unwrap(),
        )
        .unwrap();
        disk.end_write();
        return;
    }
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (dir, disk) = disk_fixture();
    let mut child = Child(Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "preferences::tests::cooperating_process_lock_refuses_then_retries_without_losing_fields", "--nocapture"])
        .env(ENV, &dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dir.join("locked").exists() {
        assert!(Instant::now() < deadline, "child must acquire lock");
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before lock acquisition"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let owner = Preferences::start(Box::new(SyntheticDisk(disk)));
    wait(&owner, |s| s.ready);
    let target = owner.admit(setting("sidebar", "false")).unwrap();
    wait(&owner, |s| s.error.contains("busy"));
    assert!(owner.snapshot().pending);
    child
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child must finish its publication"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    flush_after_busy(&owner, target);
    let settings =
        Settings::from_text(&std::fs::read_to_string(dir.join("settings.toml")).unwrap());
    assert!(settings.show_hidden && !settings.sidebar);
    assert!(owner.shutdown(Duration::from_secs(1)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retry_completion_counter_distinguishes_identical_failures() {
    let (owner, control) = fixture();
    lock(&control.state).fail = true;
    owner.admit(setting("sidebar", "false")).unwrap();
    let failed = wait(&owner, |s| !s.error.is_empty());
    owner.flush();
    let retried = wait(&owner, |s| s.completed_attempt > failed.completed_attempt);
    assert_eq!(retried.error, failed.error);
    assert_eq!(retried.saved_revision, failed.saved_revision);
    assert!(retried.pending);
    lock(&control.state).fail = false;
    owner.flush();
    wait(&owner, |s| !s.pending);
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn held_parent_is_used_for_reconcile_and_publication_after_path_swap() {
    let (dir, mut disk) = disk_fixture();
    let original = dir.join("original");
    std::fs::create_dir(&original).unwrap();
    disk.paths
        .as_mut()
        .unwrap()
        .insert(Document::Settings, original.join("settings.toml"));
    std::fs::write(original.join("settings.toml"), "sidebar = false\n").unwrap();
    retry_busy(|| disk.begin_write(&[Document::Settings])).unwrap();
    let held = dir.join("held");
    std::fs::rename(&original, &held).unwrap();
    std::fs::create_dir(&original).unwrap();
    std::fs::write(original.join("settings.toml"), "sentinel").unwrap();
    let previous = disk.read(Document::Settings).unwrap();
    assert_eq!(previous.as_deref(), Some("sidebar = false\n"));
    disk.publish(
        Document::Settings,
        previous.as_deref(),
        "sidebar = false\nshow_hidden = true\n",
    )
    .unwrap();
    disk.end_write();
    assert_eq!(
        std::fs::read_to_string(original.join("settings.toml")).unwrap(),
        "sentinel"
    );
    assert!(
        Settings::from_text(&std::fs::read_to_string(held.join("settings.toml")).unwrap())
            .show_hidden
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sigkill_after_staged_sync_never_publishes_incomplete_preferences() {
    use std::os::unix::{fs::PermissionsExt, process::ExitStatusExt};
    use std::process::{Command, Stdio};
    const PATH_ENV: &str = "GUST_SYNTHETIC_PREFERENCES_CRASH_PATH";
    const MODE_ENV: &str = "GUST_SYNTHETIC_PREFERENCES_CRASH_MODE";
    const TEST: &str =
        "preferences::tests::sigkill_after_staged_sync_never_publishes_incomplete_preferences";
    let old = Settings::default().to_text();
    let new = Settings::from_text("sidebar = false\ndensity = \"roomy\"\n").to_text();

    if let Some(dir) = std::env::var_os(PATH_ENV).map(PathBuf::from) {
        let mode = std::env::var(MODE_ENV).unwrap();
        assert!(matches!(mode.as_str(), "replace" | "create"));
        let target = dir.join("settings.toml");
        let (parent, name) = parent(&target, false).unwrap();
        publish_at(
            &parent,
            &name,
            Document::Settings,
            (mode == "replace").then_some(old.as_str()),
            &new,
            |stage| {
                stage.sync_all()?;
                // This checkpoint is reached only after all staged bytes were
                // synced, and blocks before publish_at can compare or rename.
                std::fs::write(dir.join("stage-synced"), b"ready")?;
                io::stdin().read_exact(&mut [0u8])?;
                Err(io::Error::other("crash fixture unexpectedly released"))
            },
        )
        .unwrap();
        panic!("crash fixture must be killed before publication");
    }

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let (dir, _) = disk_fixture();
    let fixture = Fixture(dir);
    std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    for mode in ["replace", "create"] {
        let dir = fixture.0.join(mode);
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("settings.toml");
        if mode == "replace" {
            std::fs::write(&target, &old).unwrap();
        }
        let mut child = Child(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(PATH_ENV, &dir)
                .env(MODE_ENV, mode)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dir.join("stage-synced").exists() {
            assert!(
                Instant::now() < deadline,
                "child must sync its stage: {mode}"
            );
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child exited before sync: {mode}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let prefix = format!(".gust-prefs-{}-", child.0.id());
        let stages: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .collect();
        assert_eq!(stages.len(), 1, "checkpoint must have one owned stage");
        assert_eq!(std::fs::read(&stages[0]).unwrap(), new.as_bytes());
        // Child::kill sends SIGKILL on Linux; the child cannot unwind, run the
        // error cleanup, or proceed to rename after this held sync checkpoint.
        child.0.kill().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "SIGKILL child must be reaped: {mode}"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.signal(), Some(9), "must observe actual SIGKILL");
        if mode == "replace" {
            assert_eq!(
                std::fs::read(&target).unwrap(),
                old.as_bytes(),
                "old complete document survives"
            );
        } else {
            assert!(
                !target.try_exists().unwrap(),
                "no new document was published"
            );
        }
        assert_eq!(
            std::fs::read(&stages[0]).unwrap(),
            new.as_bytes(),
            "synced bytes remain only in the unpublished stage"
        );
    }
    // This checks a process-crash boundary, not directory power-loss durability.
}

#[test]
fn repeated_session_replacement_heals_known_headers_and_stays_byte_idempotent() {
    let parts: Vec<_> = (0..gust_core::session::MAX_TABS)
        .map(|n| (key(&format!("/session/tab-{n}")).to_token(), "grid".into()))
        .collect();
    let session = Session::from_parts(&parts, parts.len() - 1);
    let canonical = session.to_text();
    let headers: String = canonical
        .lines()
        .take_while(|line| line.starts_with('#'))
        .map(|line| format!("{line}\n"))
        .collect();
    for copies in [0, 200] {
        let control = Arc::new(Control::default());
        let original = format!(
            "{}{}# unknown comment retained\nfuture = \"retained\"\n",
            headers.repeat(copies),
            canonical
        );
        assert!((original.len() as u64) < Document::Session.limit());
        if copies > 0 {
            assert!(Session::from_text(&original).tabs.is_empty());
        }
        {
            let mut files = lock(&control.state);
            files
                .files
                .insert(Document::Settings, "restore_tabs = true\n".into());
            files.files.insert(Document::Session, original);
        }
        let owner = Preferences::start(Box::new(Fake(control.clone())));
        assert_eq!(
            wait(&owner, |s| s.ready).session,
            session,
            "heal before bounded parsing"
        );
        let mut first_saved = None;
        for _ in 0..220 {
            let target = owner.admit(Mutation::Session(session.clone())).unwrap();
            let saved = wait(&owner, |s| s.saved_revision >= target);
            assert!(saved.error.is_empty());
            assert_eq!(saved.session, session);
            let text = lock(&control.state).files[&Document::Session].clone();
            assert_eq!(Session::from_text(&text), session);
            assert!(text.contains("future = \"retained\""));
            assert!(text.contains("# unknown comment retained"));
            for header in headers.lines() {
                assert_eq!(text.lines().filter(|line| *line == header).count(), 1);
            }
            match &first_saved {
                Some(first) => {
                    assert_eq!(&text, first, "every save after the first is byte-identical")
                }
                None => first_saved = Some(text),
            }
        }
        let target = owner.admit(setting("density", "compact")).unwrap();
        let saved = wait(&owner, |s| s.saved_revision >= target);
        assert!(saved.error.is_empty());
        assert_eq!(saved.settings.density_name(), "compact");
        assert_eq!(saved.session, session);
        assert!(owner.shutdown(Duration::from_secs(1)));
    }
}

fn loaded_documents_fixture() -> (Preferences, Arc<Control>) {
    let control = Arc::new(Control::default());
    let mut sorts = SortMemory::new();
    sorts.remember(&key("/last-good"), SortField::Modified, false);
    let mut views = ViewMemory::new();
    views.remember(&key("/last-good"), ViewMode::Columns);
    let session = Session::from_parts(&[(key("/last-good-tab").to_token(), "grid".into())], 0);
    lock(&control.state).files = [
        (
            Document::Settings,
            "restore_tabs = true\nsidebar = false\ndensity = \"roomy\"\n".into(),
        ),
        (Document::Sorts, sorts.to_text()),
        (Document::Views, views.to_text()),
        (Document::Session, session.to_text()),
    ]
    .into();
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    wait(&owner, |s| s.ready);
    (owner, control)
}
fn assert_same_choices(actual: &Snapshot, expected: &Snapshot) {
    assert_eq!(actual.settings, expected.settings);
    assert_eq!(actual.sorts.to_text(), expected.sorts.to_text());
    assert_eq!(actual.views.to_text(), expected.views.to_text());
    assert_eq!(actual.session, expected.session);
}

#[test]
fn every_failed_document_read_retains_last_good_and_pending_choices_until_recovery() {
    for document in [
        Document::Settings,
        Document::Sorts,
        Document::Views,
        Document::Session,
    ] {
        let (owner, control) = loaded_documents_fixture();
        let previous = owner.snapshot();
        lock(&control.state).failed_reads.insert(document);
        owner.refresh();
        let failed = wait(&owner, |s| s.completed_attempt > previous.completed_attempt);
        assert!(failed.error.contains("read failure"), "{document:?}");
        assert_eq!(failed.saved_revision, previous.saved_revision);
        assert!(!failed.pending);
        assert_same_choices(&failed, &previous);
        let mutation = match document {
            Document::Settings => setting("density", "compact"),
            Document::Sorts => Mutation::Sort(key("/pending"), SortField::Size, false),
            Document::Views => Mutation::View(key("/pending"), ViewMode::Rows),
            Document::Session => Mutation::Session(Session::from_parts(
                &[
                    (key("/pending-one").to_token(), "columns".into()),
                    (key("/pending-two").to_token(), "grid".into()),
                ],
                1,
            )),
        };
        owner.admit(mutation.clone()).unwrap();
        let target = owner.admit(setting("show_hidden", "true")).unwrap();
        let failed = wait(&owner, |s| s.completed_attempt > failed.completed_attempt);
        let mut optimistic = previous.clone();
        mutation.apply(&mut optimistic).unwrap();
        setting("show_hidden", "true")
            .apply(&mut optimistic)
            .unwrap();
        assert_same_choices(&failed, &optimistic);
        assert_eq!(failed.saved_revision, previous.saved_revision);
        assert!(failed.pending);
        assert!(
            lock(&control.state).writes.is_empty(),
            "no document is published from a batch with a failed read"
        );
        // An unrelated external choice in the unreadable source is observed
        // only when a fresh successful read can safely reconcile it.
        {
            let mut files = lock(&control.state);
            let text = files.files.get_mut(&document).unwrap();
            text.push_str("future = \"external-retained\"\n");
            match document {
                Document::Settings => text.push_str("key_preset = \"vim\"\n"),
                Document::Sorts => text.push_str(&format!(
                    "{} = \"kind-desc\"\n",
                    key("/external").to_token()
                )),
                Document::Views => {
                    text.push_str(&format!("{} = \"grid\"\n", key("/external").to_token()))
                }
                Document::Session => {}
            }
            files.failed_reads.clear();
        }
        owner.flush();
        let recovered = wait(&owner, |s| s.saved_revision >= target);
        assert!(recovered.error.is_empty(), "{document:?}");
        assert!(!recovered.pending);
        assert!(recovered.settings.show_hidden);
        match document {
            Document::Settings => {
                assert_eq!(recovered.settings.density_name(), "compact");
                assert_eq!(recovered.settings.key_preset_name(), "vim");
            }
            Document::Sorts => {
                assert_eq!(
                    recovered.sorts.get(&key("/pending")),
                    Some((SortField::Size, false))
                );
                assert_eq!(
                    recovered.sorts.get(&key("/external")),
                    Some((SortField::Kind, false))
                );
            }
            Document::Views => {
                assert_eq!(recovered.views.get(&key("/pending")), Some(ViewMode::Rows));
                assert_eq!(recovered.views.get(&key("/external")), Some(ViewMode::Grid));
            }
            Document::Session => assert_eq!(recovered.session, optimistic.session),
        }
        assert!(lock(&control.state).files[&document].contains("future = \"external-retained\""));
        assert!(owner.shutdown(Duration::from_secs(1)));
    }
}

#[test]
fn successful_document_deletion_replaces_last_good_instead_of_retaining_it_as_an_error() {
    for document in [
        Document::Settings,
        Document::Sorts,
        Document::Views,
        Document::Session,
    ] {
        let (owner, control) = loaded_documents_fixture();
        let previous = owner.snapshot();
        lock(&control.state).files.remove(&document);
        owner.refresh();
        let refreshed = wait(&owner, |s| s.completed_attempt > previous.completed_attempt);
        assert!(refreshed.error.is_empty());
        match document {
            Document::Settings => assert_eq!(refreshed.settings, Settings::default()),
            Document::Sorts => assert!(refreshed
                .sorts
                .to_text()
                .lines()
                .all(|line| !line.contains('='))),
            Document::Views => assert_eq!(refreshed.views.len(), 0),
            Document::Session => assert_eq!(refreshed.session, Session::default()),
        }
        assert!(owner.shutdown(Duration::from_secs(1)));
    }
}

#[test]
fn session_header_healing_does_not_bypass_size_or_symlink_admission() {
    let (dir, mut disk) = disk_fixture();
    let path = dir.join("tabs.toml");
    let header = Session::default().to_text();
    std::fs::write(&path, header.repeat(1024)).unwrap();
    disk.paths
        .as_mut()
        .unwrap()
        .insert(Document::Session, path.clone());
    assert!(disk
        .read(Document::Session)
        .unwrap_err()
        .contains("oversized"));
    let link = dir.join("linked-tabs.toml");
    std::os::unix::fs::symlink(path, &link).unwrap();
    disk.paths.as_mut().unwrap().insert(Document::Session, link);
    assert!(disk.read(Document::Session).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejected_early_sort_reaches_reload_without_becoming_another_locations_preference() {
    use crate::sort_state::{needs_reload, SortState};
    use gust_core::entry::{Entry, EntryKind, SortSpec};
    let control = Arc::new(Control::default());
    lock(&control.state).blocked_read = Some(Document::Settings);
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    entered(&control);
    let here = key("/early-sort");
    let requested = SortSpec {
        field: SortField::Size,
        ascending: false,
        dirs_first: true,
    };
    let admission = owner.admit(Mutation::Sort(
        here.clone(),
        requested.field,
        requested.ascending,
    ));
    assert!(admission.as_ref().unwrap_err().contains("loading"));
    let mut runtime = SortState::default();
    runtime.requested(&here, requested, admission.is_ok());
    let entries = [Entry::new("pending-metadata".into(), EntryKind::File)];
    assert!(needs_reload(requested, true, &entries));
    assert_eq!(
        runtime.for_reload(&here, &owner.snapshot().sorts),
        requested
    );
    release(&control);
    wait(&owner, |s| s.ready);
    assert_eq!(
        runtime.for_reload(&here, &owner.snapshot().sorts),
        requested
    );
    assert_eq!(
        owner.snapshot().sorts.get(&here),
        None,
        "refused intent is not silently persisted"
    );
    let elsewhere = key("/elsewhere");
    assert_eq!(
        runtime.for_reload(&elsewhere, &owner.snapshot().sorts),
        SortSpec::default()
    );
    assert_eq!(
        runtime.for_reload(&here, &owner.snapshot().sorts),
        SortSpec::default(),
        "leaving retires the transient choice"
    );
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn canonical_session_records_precede_preserved_unknown_lines_across_save_refresh_and_restart() {
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let (dir, disk) = disk_fixture();
    let fixture = Fixture(dir);
    std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let parts: Vec<_> = (0..gust_core::session::MAX_TABS)
        .map(|number| {
            let name = if number == 17 {
                std::ffi::OsString::from_vec(b"byte-tab-\xff".to_vec())
            } else {
                format!("tab-{number}").into()
            };
            let folder = fixture.0.join(name);
            std::fs::create_dir(&folder).unwrap();
            (LocationKey::local(folder).to_token(), "grid".to_string())
        })
        .collect();
    let session = Session::from_parts(&parts, 39);
    assert_eq!(
        session.still_there(),
        session,
        "all forty fixture folders exist"
    );
    let canonical = session.to_text();
    let unknown: String = (0..160)
        .map(|number| {
            if number % 2 == 0 {
                format!("# preserved trailing comment {number}\n")
            } else {
                format!("future_{number} = \"preserved value {number}\"\n")
            }
        })
        .collect();
    assert_eq!(unknown.lines().count(), 160);

    // Unknown leading content still consumes the unchanged bounded parser
    // prefix. Header healing does not scan arbitrary content past that bound.
    let out_of_prefix = format!("{unknown}{canonical}");
    assert_eq!(Session::from_text(&out_of_prefix), Session::default());
    assert_eq!(
        decode(&[(Document::Session, Some(out_of_prefix))].into()).session,
        Session::default()
    );

    for (number, leading) in ["", "# preserved leading comment\n"]
        .into_iter()
        .enumerate()
    {
        let path = fixture.0.join(format!("tabs-{number}.toml"));
        let initial = format!("{leading}{canonical}{unknown}");
        let expected = format!("{canonical}{leading}{unknown}");
        assert!((initial.len() as u64) < Document::Session.limit());
        assert_eq!(
            Session::from_text(&initial),
            session,
            "valid initial bounded core decode"
        );
        assert_eq!(
            decode(&[(Document::Session, Some(initial.clone()))].into()).session,
            session
        );
        std::fs::write(&path, &initial).unwrap();
        std::fs::write(fixture.0.join("settings.toml"), "restore_tabs = true\n").unwrap();
        let mut paths = disk.paths.clone().unwrap();
        paths.insert(Document::Session, path.clone());
        let owner = Preferences::start(Box::new(SyntheticDisk(Disk {
            paths: Some(paths.clone()),
            batch: None,
        })));
        assert_eq!(
            wait(&owner, |s| s.ready).session,
            session,
            "initial service decode"
        );
        let mut first_saved = None;
        for _ in 0..4 {
            let target = owner.admit(Mutation::Session(session.clone())).unwrap();
            retry_busy(|| {
                let before = owner.snapshot().completed_attempt;
                owner.flush();
                // An older read-only attempt can finish before this admitted
                // revision. Wait for its acknowledgement or a completed error,
                // rather than mistaking any completion for the save result.
                let saved = wait(&owner, |s| {
                    s.saved_revision >= target
                        || (s.completed_attempt > before && !s.error.is_empty())
                });
                if saved.saved_revision >= target {
                    Ok(())
                } else {
                    Err(saved.error)
                }
            })
            .unwrap();
            let saved = owner.snapshot();
            assert!(saved.error.is_empty());
            assert_eq!(saved.session, session);
            let text = std::fs::read_to_string(&path).unwrap();
            // Direct core decoding must succeed without service normalization.
            assert_eq!(Session::from_text(&text), session);
            assert!(text.starts_with(&canonical));
            assert_eq!(
                text, expected,
                "every unknown line retained after canonical records"
            );
            assert!((text.len() as u64) < Document::Session.limit());
            if let Some(first) = &first_saved {
                assert_eq!(&text, first, "byte-idempotent repeated save");
            } else {
                first_saved = Some(text);
            }
        }
        let before_refresh = owner.snapshot().completed_attempt;
        owner.refresh();
        let refreshed = wait(&owner, |s| s.completed_attempt > before_refresh);
        assert!(refreshed.error.is_empty());
        assert_eq!(refreshed.session, session);
        assert!(owner.shutdown(Duration::from_secs(1)));
        let restarted = Preferences::start(Box::new(SyntheticDisk(Disk {
            paths: Some(paths),
            batch: None,
        })));
        let loaded = wait(&restarted, |s| s.ready);
        assert!(loaded.error.is_empty());
        assert_eq!(
            loaded.session, session,
            "restart-style service decode preserves tabs, byte names and active=39"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        assert!(restarted.shutdown(Duration::from_secs(1)));
    }
}

fn location_canonical(document: Document, snapshot: &Snapshot) -> String {
    match document {
        Document::Sorts => snapshot.sorts.to_text(),
        Document::Views => snapshot.views.to_text(),
        _ => panic!("expected a bounded location cache"),
    }
}
fn location_direct(document: Document, text: &str) -> String {
    match document {
        Document::Sorts => SortMemory::from_text(text).to_text(),
        Document::Views => ViewMemory::from_text(text).to_text(),
        _ => panic!("expected a bounded location cache"),
    }
}
fn location_choice(document: Document, location: LocationKey) -> Mutation {
    match document {
        Document::Sorts => Mutation::Sort(location, SortField::Kind, false),
        Document::Views => Mutation::View(location, ViewMode::Grid),
        _ => panic!("expected a bounded location cache"),
    }
}
fn admit_saved(owner: &Preferences, mutation: Mutation) -> Snapshot {
    let target = owner.admit(mutation).unwrap();
    retry_busy(|| {
        let before = owner.snapshot().completed_attempt;
        owner.flush();
        let saved = wait(owner, |s| {
            s.saved_revision >= target || (s.completed_attempt > before && !s.error.is_empty())
        });
        if saved.saved_revision >= target {
            Ok(())
        } else {
            Err(saved.error)
        }
    })
    .unwrap();
    let saved = owner.snapshot();
    assert!(saved.error.is_empty());
    saved
}
fn owner_with_document(document: Document, text: String) -> (Preferences, Arc<Control>) {
    let control = Arc::new(Control::default());
    lock(&control.state).files.insert(document, text);
    let owner = Preferences::start(Box::new(Fake(control.clone())));
    wait(&owner, |s| s.ready);
    (owner, control)
}

#[test]
fn sort_and_view_records_precede_two_thousand_comment_lines_on_save_refresh_and_restart() {
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let touched = LocationKey::local(Path::new(std::ffi::OsStr::from_bytes(b"/updated-\xfe")));
    let tail: String = (0..2000)
        .map(|n| format!("# preserved trailing comment {n}\n"))
        .collect();
    assert_eq!(tail.lines().count(), 2000);
    for document in [Document::Sorts, Document::Views] {
        for unrelated in [0, 40] {
            let (dir, mut disk) = disk_fixture();
            let fixture = Fixture(dir);
            std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o700)).unwrap();
            let path = fixture.0.join("locations.toml");
            disk.paths.as_mut().unwrap().insert(document, path.clone());
            let value = if document == Document::Sorts {
                "size-asc"
            } else {
                "rows"
            };
            // The zero-unrelated case is exactly one valid line followed by
            // 2,000 comments. The other case retains forty live byte keys too.
            let mut initial = format!("{} = \"{value}\"\n", touched.to_token());
            for number in 0..unrelated {
                let location = if number == 17 {
                    LocationKey::local(Path::new(std::ffi::OsStr::from_bytes(b"/unrelated-\xff")))
                } else {
                    key(&format!("/unrelated-{number}"))
                };
                initial.push_str(&format!("{} = \"{value}\"\n", location.to_token()));
            }
            initial.push_str(&tail);
            assert!((initial.len() as u64) < document.limit());
            std::fs::write(&path, &initial).unwrap();
            let paths = disk.paths.clone();
            let owner = Preferences::start(Box::new(SyntheticDisk(disk)));
            let loaded = wait(&owner, |s| s.ready);
            assert_eq!(
                location_canonical(document, &loaded),
                location_direct(document, &initial)
            );
            let count = if document == Document::Sorts {
                loaded.sorts.len()
            } else {
                loaded.views.len()
            };
            assert_eq!(count, unrelated + 1);
            let mutation = location_choice(document, touched.clone());
            let mut expected = loaded;
            mutation.apply(&mut expected).unwrap();
            let canonical = location_canonical(document, &expected);
            let mut first = None;
            for _ in 0..4 {
                let saved = admit_saved(&owner, mutation.clone());
                let text = std::fs::read_to_string(&path).unwrap();
                assert_eq!(location_canonical(document, &saved), canonical);
                assert_eq!(
                    location_direct(document, &text),
                    canonical,
                    "direct bounded core decode"
                );
                assert_eq!(
                    text,
                    format!("{canonical}{tail}"),
                    "all unknown comments retained after records"
                );
                assert!((text.len() as u64) < document.limit());
                if let Some(first) = &first {
                    assert_eq!(&text, first, "byte-idempotent acknowledged save");
                } else {
                    first = Some(text);
                }
            }
            let before = owner.snapshot().completed_attempt;
            owner.refresh();
            let refreshed = wait(&owner, |s| s.completed_attempt > before);
            assert!(refreshed.error.is_empty());
            assert_eq!(location_canonical(document, &refreshed), canonical);
            assert!(owner.shutdown(Duration::from_secs(1)));
            let restarted =
                Preferences::start(Box::new(SyntheticDisk(Disk { paths, batch: None })));
            let restored = wait(&restarted, |s| s.ready);
            assert!(restored.error.is_empty());
            assert_eq!(
                location_canonical(document, &restored),
                canonical,
                "fresh owner must recover every choice"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), first.unwrap());
            assert!(restarted.shutdown(Duration::from_secs(1)));
        }
    }
}

#[test]
fn bounded_location_caches_keep_eviction_defaults_and_headers_idempotent() {
    for document in [Document::Sorts, Document::Views] {
        let value = if document == Document::Sorts {
            "modified-desc"
        } else {
            "columns"
        };
        let headers = location_canonical(document, &Snapshot::default());
        let fresh = key("/fresh-choice");
        let future = key("/untouched-future").to_token();
        let unknown = format!("# unrelated comment\n{future} = \"future-value\"\n");
        let mut source = headers.repeat(4);
        for number in 0..500 {
            source.push_str(&format!(
                "{} = \"{value}\"\n",
                key(&format!("/cache-{number:03}")).to_token()
            ));
        }
        source.push_str(&format!("{} = \"future-value\"\n", fresh.to_token()));
        // The original parser stopped at capacity; this recognized overflow
        // must be removed rather than resurfacing behind the canonical block.
        let ignored = key("/recognized-overflow");
        source.push_str(&format!("{} = \"{value}\"\n{unknown}", ignored.to_token()));
        assert!((source.len() as u64) < document.limit());
        let (owner, control) = owner_with_document(document, source);
        let mutation = location_choice(document, fresh.clone());
        let mut first = None;
        for _ in 0..4 {
            let saved = admit_saved(&owner, mutation.clone());
            let canonical = location_canonical(document, &saved);
            let text = lock(&control.state).files[&document].clone();
            assert_eq!(text, format!("{canonical}{unknown}"));
            assert_eq!(location_direct(document, &text), canonical);
            assert!(
                !text.contains(&key("/cache-000").to_token()),
                "oldest entry was evicted"
            );
            assert!(
                !text.contains(&ignored.to_token()),
                "out-of-cache recognized record stays removed"
            );
            assert!(
                !text.contains(&format!("{} = \"future-value\"", fresh.to_token())),
                "explicit choice overwrites unknown value at touched key"
            );
            for header in headers.lines() {
                assert_eq!(text.lines().filter(|line| *line == header).count(), 1);
            }
            if document == Document::Sorts {
                assert_eq!(saved.sorts.len(), 500);
            } else {
                assert_eq!(saved.views.len(), 500);
            }
            if let Some(first) = &first {
                assert_eq!(&text, first);
            } else {
                first = Some(text);
            }
        }
        let mutation = if document == Document::Sorts {
            Mutation::Sort(fresh.clone(), SortField::Name, true)
        } else {
            Mutation::View(fresh.clone(), ViewMode::Rows)
        };
        let saved = admit_saved(&owner, mutation);
        let text = lock(&control.state).files[&document].clone();
        assert_eq!(
            location_direct(document, &text),
            location_canonical(document, &saved)
        );
        if document == Document::Sorts {
            assert_eq!(saved.sorts.len(), 499);
            assert_eq!(
                saved.sorts.get(&fresh),
                None,
                "explicit default forgets the sort override"
            );
            assert!(!text.contains(&fresh.to_token()));
        } else {
            assert_eq!(saved.views.len(), 500);
            assert_eq!(
                saved.views.get(&fresh),
                Some(ViewMode::Rows),
                "Rows remains an explicit view choice"
            );
        }
        assert!(text.ends_with(&unknown));
        assert!(owner.shutdown(Duration::from_secs(1)));
    }
}

#[test]
fn location_record_classification_matches_core_without_granting_path_authority() {
    for document in [Document::Sorts, Document::Views] {
        let value = if document == Document::Sorts {
            "size-desc"
        } else {
            "columns"
        };
        let headers = location_canonical(document, &Snapshot::default());
        let touched = key("/explicitly-overwritten");
        let future = key("/future-key").to_token();
        let inline = key("/inline-unknown").to_token();
        let unknown = format!("# unrelated = comment\nlocal:zz = \"{value}\"\nfuture-provider:2f78 = \"{value}\"\n{future} = \"future-value\"\nlocal:00 = \"future-value\"\n{inline} = \"{value}\" # ignored by core\n\n");
        let legacy = ["local:", "local:2e2e2f72656c", "local:2f00"];
        let mut source = headers.repeat(3);
        for token in legacy {
            assert!(LocationKey::from_token(token).is_some());
            assert!(gust_core::location::PathToken::parse(token).is_none());
            source.push_str(&format!("{token} = {value}\n"));
        }
        // An untouched legacy default is retained exactly as core loads it;
        // only an explicit default mutation forgets a sort record.
        let legacy_default = key("/legacy-default").to_token();
        source.push_str(&format!(
            "{legacy_default} = \"{}\"\n",
            if document == Document::Sorts {
                "name-asc"
            } else {
                "rows"
            }
        ));
        source.push_str(&format!(
            "{unknown}{} = \"future-value\"\n",
            touched.to_token()
        ));
        let (owner, control) = owner_with_document(document, source);
        let mutation = location_choice(document, touched.clone());
        let mut first = None;
        for _ in 0..4 {
            let saved = admit_saved(&owner, mutation.clone());
            let canonical = location_canonical(document, &saved);
            let text = lock(&control.state).files[&document].clone();
            assert_eq!(text, format!("{canonical}{unknown}"));
            assert_eq!(location_direct(document, &text), canonical);
            for token in legacy {
                assert!(canonical.contains(&format!("{token} = \"{value}\"")));
            }
            assert!(canonical.contains(&legacy_default));
            for header in headers.lines() {
                assert_eq!(text.lines().filter(|line| *line == header).count(), 1);
            }
            if let Some(first) = &first {
                assert_eq!(&text, first);
            } else {
                first = Some(text);
            }
        }
        assert!(owner.shutdown(Duration::from_secs(1)));
    }
}

#[test]
fn serialized_roundtrip_guard_refuses_each_known_state_mismatch() {
    let (owner, control) = loaded_documents_fixture();
    let expected = owner.snapshot();
    for document in [
        Document::Settings,
        Document::Sorts,
        Document::Views,
        Document::Session,
    ] {
        let valid = lock(&control.state).files[&document].clone();
        assert!(validate_serialized(document, &valid, &expected).is_ok());
        let ignored = match document {
            Document::Settings => Settings::default().to_text(),
            Document::Sorts | Document::Views => {
                format!("{}{}", "# padded prefix\n".repeat(2000), valid)
            }
            Document::Session => format!("{}{}", "# padded prefix\n".repeat(160), valid),
        };
        assert!((ignored.len() as u64) < document.limit());
        assert!(validate_serialized(document, &ignored, &expected)
            .unwrap_err()
            .contains("serialization does not preserve known choices"));
    }
    assert!(owner.shutdown(Duration::from_secs(1)));
}

#[test]
fn settings_keep_field_patch_semantics_beyond_two_thousand_lines() {
    let unknown = format!(
        "{}future = \"retained\"\n",
        "# preserved settings comment\n".repeat(2000)
    );
    let source = format!("sidebar = false\n{unknown}");
    assert!((source.len() as u64) < Document::Settings.limit());
    let mutation = setting("density", "roomy");
    let mut texts = [(Document::Settings, Some(source))].into();
    let snapshot = reconcile(&mut texts, &[(mutation.key(), (1, mutation))].into()).unwrap();
    let text = texts[&Document::Settings].as_deref().unwrap();
    assert_eq!(
        text,
        format!("sidebar = false\n{unknown}density = \"roomy\"\n")
    );
    assert!(!snapshot.settings.sidebar);
    assert_eq!(snapshot.settings.density_name(), "roomy");
    assert_eq!(Settings::from_text(text), snapshot.settings);
}

#[test]
fn roundtrip_mismatch_blocks_worker_publication_and_ack_until_valid_replacement() {
    let (owner, control) = loaded_documents_fixture();
    let original = owner.snapshot();
    let mut invalid = original.session.clone();
    // Fault injection at the Rust service boundary: normal QML session input
    // is cleaned by Session::from_parts, but this known state cannot survive
    // direct core decoding. Exercise the full worker refusal path, not just
    // the comparison helper, with another admitted document in the batch.
    invalid.tabs[0].view = "future-unrepresentable-view".into();
    owner.admit(Mutation::Session(invalid.clone())).unwrap();
    let first = wait(&owner, |s| {
        s.error
            .contains("serialization does not preserve known choices")
    });
    let target = owner.admit(setting("density", "compact")).unwrap();
    let failed = wait(&owner, |s| s.completed_attempt > first.completed_attempt);
    assert!(failed
        .error
        .contains("serialization does not preserve known choices"));
    assert_eq!(failed.revision, target);
    assert_eq!(failed.saved_revision, original.saved_revision);
    assert!(failed.pending);
    assert_eq!(
        failed.settings.density_name(),
        "compact",
        "optimistic setting stays visible"
    );
    assert_eq!(failed.session, invalid, "admitted intent stays retained");
    assert!(
        lock(&control.state).writes.is_empty(),
        "no touched document publishes from an inconsistent batch"
    );
    assert_eq!(
        lock(&control.state).files[&Document::Session],
        original.session.to_text()
    );
    let saved = admit_saved(&owner, Mutation::Session(original.session.clone()));
    assert!(!saved.pending);
    assert!(saved.saved_revision > target);
    assert_eq!(saved.session, original.session);
    assert_eq!(saved.settings.density_name(), "compact");
    let files = lock(&control.state);
    assert_eq!(
        Session::from_text(&files.files[&Document::Session]),
        original.session
    );
    assert_eq!(
        Settings::from_text(&files.files[&Document::Settings]).density_name(),
        "compact"
    );
    drop(files);
    assert!(owner.shutdown(Duration::from_secs(1)));
}
