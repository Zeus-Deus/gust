//! Authoritative confirmed Trash scope; retained evidence is never swept.
use super::*;
use crate::{
    history::{History, HistoryError},
    permanent::{self, DeleteFailure, Removed},
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;
type Entries = BTreeMap<OsString, FileIdentity>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyTrashSummary {
    pub locations: usize,
    pub items: usize,
    pub data_entries: usize,
    /// Logical regular-file sizes, not promised disk space reclaimed.
    pub data_bytes: u64,
    /// Untouched root recovery objects plus files/info directory entries.
    pub retained_entries: usize,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyTrashOutcome {
    pub items_completed: usize,
    pub removed: Removed,
    pub retained_entries: usize,
    /// Exact Info records retained after claiming, including incomplete items.
    pub information_retained: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyTrashPhase {
    Deleting,
    Finished,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyTrashProgress {
    pub phase: EmptyTrashPhase,
    pub total_items: usize,
    pub total_data_entries: usize,
    pub outcome: EmptyTrashOutcome,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmptyTrashFailure {
    Cancelled,
    Changed,
    Limit,
    Unsupported,
    Io(io::ErrorKind),
    Barrier(Box<HistoryError>),
    Delete(DeleteFailure),
}
#[derive(Debug)]
pub struct EmptyTrashError {
    pub reason: EmptyTrashFailure,
    pub outcome: EmptyTrashOutcome,
    /// Last-known uncertain location hints, never cleanup authority.
    pub retained: Vec<PathBuf>,
    pub detail: String,
}
impl std::fmt::Display for EmptyTrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Empty Trash stopped: {} ({} items completed; {} files and {} folders removed)",
            self.detail,
            self.outcome.items_completed,
            self.outcome.removed.files,
            self.outcome.removed.dirs
        )?;
        if !self.retained.is_empty() {
            write!(
                f,
                "; last-known location hints (may have moved): {:?}",
                self.retained
            )?;
        }
        Ok(())
    }
}
impl std::error::Error for EmptyTrashError {}
#[derive(Debug)]
struct EnumerationLimit;
impl std::fmt::Display for EnumerationLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Complete Trash enumeration exceeds resource limits")
    }
}
impl std::error::Error for EnumerationLimit {}
fn fail(error: io::Error) -> EmptyTrashError {
    if let Some(failure) = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<InventoryFailure>())
    {
        return admission_failure(failure.0.clone());
    }
    if error.get_ref().is_some_and(|e| e.is::<EnumerationLimit>()) {
        return limit();
    }
    let reason = match error.kind() {
        io::ErrorKind::Interrupted => EmptyTrashFailure::Cancelled,
        io::ErrorKind::InvalidData => EmptyTrashFailure::Changed,
        io::ErrorKind::Unsupported => EmptyTrashFailure::Unsupported,
        kind => EmptyTrashFailure::Io(kind),
    };
    EmptyTrashError {
        reason,
        outcome: EmptyTrashOutcome::default(),
        retained: error
            .get_ref()
            .and_then(|e| e.downcast_ref::<TrashPartial>())
            .map_or_else(Vec::new, |p| p.locations.clone()),
        detail: error.to_string(),
    }
}
fn limit() -> EmptyTrashError {
    EmptyTrashError {
        reason: EmptyTrashFailure::Limit,
        outcome: EmptyTrashOutcome::default(),
        retained: Vec::new(),
        detail: "Complete Trash scope exceeds its metadata or entry limit.".into(),
    }
}
fn admission_failure(reason: DeleteFailure) -> EmptyTrashError {
    if reason == DeleteFailure::Limit {
        limit()
    } else {
        fail(delete_failure(reason))
    }
}
fn checkpoint_failure(error: io::Error) -> DeleteFailure {
    if let Some(failure) = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<InventoryFailure>())
    {
        return failure.0.clone();
    }
    match error.kind() {
        io::ErrorKind::Interrupted => DeleteFailure::Cancelled,
        io::ErrorKind::InvalidData => DeleteFailure::Changed,
        io::ErrorKind::Unsupported => DeleteFailure::Unsupported,
        kind => DeleteFailure::Io(kind),
    }
}
fn delete_failure(reason: DeleteFailure) -> io::Error {
    let kind = match reason {
        DeleteFailure::Cancelled => io::ErrorKind::Interrupted,
        DeleteFailure::Unsupported => io::ErrorKind::Unsupported,
        DeleteFailure::Io(kind) => kind,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(
        kind,
        "Trash data cannot be completely admitted for permanent deletion",
    )
}
fn dir_key(id: FileIdentity) -> (u64, u64, u32) {
    (id.dev, id.ino, id.mode)
}
fn entries(dir: &DirHandle, cancel: &CancelToken, mount: u64) -> io::Result<Entries> {
    check_mount_at(dir, OsStr::new(""), Some(mount))?;
    let mut out = Entries::new();
    let mut bytes = 0usize;
    for entry in rustix::fs::Dir::read_from(dir.as_fd())? {
        check(cancel)?;
        let entry = entry?;
        let raw = entry.file_name().to_bytes();
        if matches!(raw, b"." | b"..") {
            continue;
        }
        bytes = bytes.saturating_add(raw.len() + 128);
        if out.len() >= MAX_ENTRIES || bytes > MAX_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, EnumerationLimit));
        }
        let name = OsStr::from_bytes(raw);
        check_mount_at(dir, name, Some(mount))?;
        out.insert(
            name.to_owned(),
            FileIdentity::from_stat(&dir.stat_child(name)?),
        );
    }
    Ok(out)
}
#[derive(Debug)]
struct Scope {
    location: TrashLocation,
    root: FileIdentity,
    files_id: FileIdentity,
    info_id: FileIdentity,
    mount: u64,
    files: Entries,
    info: Entries,
    receipts: Vec<TrashReceipt>,
    evidence: Entries,
    run: Option<(OsString, FileIdentity)>,
}
impl Scope {
    fn handles(&self) -> io::Result<(DirHandle, DirHandle)> {
        let root = history_tree::open_dir(self.location.trash.root())?;
        let (files, info) = self.location.handles(false)?;
        for (dir, id) in [
            (&root, self.root),
            (&files, self.files_id),
            (&info, self.info_id),
        ] {
            if dir_key(FileIdentity::from_stat(&dir.stat_self()?)) != dir_key(id)
                || permanent::mount_id(dir, OsStr::new("")).map_err(delete_failure)? != self.mount
            {
                return Err(invalid("Trash directory binding or mount changed"));
            }
        }
        Ok((files, info))
    }
    fn verify_names(&self, cancel: &CancelToken) -> io::Result<()> {
        let (files, info) = self.handles()?;
        if entries(&files, cancel, self.mount)? != self.files
            || entries(&info, cancel, self.mount)? != self.info
        {
            return Err(invalid(
                "Trash scope changed after admission; new work was preserved",
            ));
        }
        let root = history_tree::open_dir(self.location.trash.root())?;
        let mut evidence = entries(&root, cancel, self.mount)?;
        evidence.remove(OsStr::new("files"));
        evidence.remove(OsStr::new("info"));
        if let Some((name, id)) = &self.run {
            if evidence.remove(name).map(dir_key) != Some(dir_key(*id)) {
                return Err(invalid("Empty Trash run binding changed"));
            }
        }
        if evidence != self.evidence {
            return Err(invalid("Trash recovery namespace changed"));
        }
        Ok(())
    }
}
/// Opaque full scope. Keep on a worker; only projections below are memory-only.
#[derive(Debug)]
pub struct EmptyTrashPlan {
    scopes: Vec<Scope>,
    summary: EmptyTrashSummary,
    charge: usize,
}
/// Consumes the exact admitted plan after explicit irreversible confirmation.
#[derive(Debug)]
pub struct ConfirmedEmptyTrash {
    plan: EmptyTrashPlan,
}
impl EmptyTrashPlan {
    pub fn summary(&self) -> EmptyTrashSummary {
        self.summary
    }
    pub fn retained_bytes(&self) -> usize {
        self.charge
    }
    pub fn confirm_irreversible(self) -> ConfirmedEmptyTrash {
        ConfirmedEmptyTrash { plan: self }
    }
}
fn protected(receipt: &TrashReceipt) -> bool {
    receipt.name.as_bytes().starts_with(b".gust-")
        || receipt.tree.keys().any(|path| {
            path.strip_prefix(receipt.data_path())
                .is_ok_and(|relative| {
                    relative
                        .components()
                        .any(|c| c.as_os_str().as_bytes().starts_with(b".gust-"))
                })
        })
}
fn relative_tree(receipt: &TrashReceipt) -> io::Result<BTreeMap<PathBuf, FileIdentity>> {
    receipt
        .tree
        .iter()
        .map(|(path, id)| {
            Ok((
                path.strip_prefix(receipt.data_path())
                    .map_err(|_| invalid("Invalid Trash inventory root"))?
                    .to_path_buf(),
                *id,
            ))
        })
        .collect()
}
/// Caller supplies the complete checked-discovery location scope, never cached
/// row projections. Unpaired/recovery-shaped entries are retained and counted.
pub fn prepare_empty(
    locations: &[TrashLocation],
    cancel: &CancelToken,
) -> Result<EmptyTrashPlan, EmptyTrashError> {
    let mut plan = EmptyTrashPlan {
        scopes: Vec::new(),
        summary: EmptyTrashSummary::default(),
        charge: 0,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut scanned = 0usize;
    for (index, location) in locations.iter().enumerate() {
        if locations[..index].iter().any(|other| {
            other.trash.root().starts_with(location.trash.root())
                || location.trash.root().starts_with(other.trash.root())
        }) {
            return Err(fail(invalid(
                "Overlapping Trash location scopes are not admitted",
            )));
        }
        check(cancel).map_err(fail)?;
        let (files, info) = location.handles(false).map_err(fail)?;
        let root = history_tree::open_dir(location.trash.root()).map_err(fail)?;
        let root_id = FileIdentity::from_stat(&root.stat_self().map_err(fail)?);
        if !seen.insert((root_id.dev, root_id.ino)) {
            return Err(fail(invalid("Duplicate Trash location in confirmed scope")));
        }
        let mount = permanent::mount_id(&root, OsStr::new("")).map_err(admission_failure)?;
        let mut evidence = entries(&root, cancel, mount).map_err(fail)?;
        evidence.remove(OsStr::new("files"));
        evidence.remove(OsStr::new("info"));
        let mut scope = Scope {
            location: location.clone(),
            root: root_id,
            files_id: FileIdentity::from_stat(&files.stat_self().map_err(fail)?),
            info_id: FileIdentity::from_stat(&info.stat_self().map_err(fail)?),
            mount,
            files: entries(&files, cancel, mount).map_err(fail)?,
            info: entries(&info, cancel, mount).map_err(fail)?,
            receipts: Vec::new(),
            evidence,
            run: None,
        };
        scope.handles().map_err(fail)?;
        scanned =
            scanned.saturating_add(scope.files.len() + scope.info.len() + scope.evidence.len());
        plan.summary.retained_entries = plan
            .summary
            .retained_entries
            .saturating_add(scope.files.len() + scope.info.len() + scope.evidence.len());
        plan.charge = scope
            .files
            .keys()
            .chain(scope.info.keys())
            .chain(scope.evidence.keys())
            .fold(plan.charge, |sum, name| {
                sum.saturating_add(name.len() + 256)
            });
        plan.charge = plan
            .charge
            .saturating_add(location.trash.root().as_os_str().len() * 4 + 512);
        if plan.charge > MAX_BYTES || scanned > MAX_ENTRIES {
            return Err(limit());
        }
        for name in scope.files.keys() {
            check(cancel).map_err(fail)?;
            if name.as_bytes().starts_with(b".gust-") {
                continue;
            }
            if !scope.info.contains_key(&info_name(name).map_err(fail)?) {
                continue;
            }
            let receipt = location
                .receipt_named_inner(name, cancel, false, Some(scope.mount))
                .map_err(fail)?;
            if protected(&receipt) {
                continue;
            }
            let admitted =
                permanent::admit_tree(&receipt.data_path(), cancel).map_err(admission_failure)?;
            if admitted != relative_tree(&receipt).map_err(fail)? {
                return Err(fail(invalid("Trash data changed during admission")));
            }
            plan.charge = plan.charge.saturating_add(receipt.retained_bytes());
            plan.summary.data_entries =
                plan.summary.data_entries.saturating_add(receipt.tree.len());
            if plan.charge > MAX_BYTES
                || scanned.saturating_add(plan.summary.data_entries) > MAX_ENTRIES
            {
                return Err(limit());
            }
            for id in receipt
                .tree
                .values()
                .filter(|id| id.mode & 0o170000 == 0o100000)
            {
                plan.summary.data_bytes = plan
                    .summary
                    .data_bytes
                    .checked_add(id.size)
                    .ok_or_else(limit)?;
            }
            plan.summary.items += 1;
            plan.summary.retained_entries -= 2;
            scope.receipts.push(receipt);
        }
        scope.verify_names(cancel).map_err(fail)?;
        for receipt in &scope.receipts {
            receipt.verify(cancel).map_err(fail)?;
        }
        plan.summary.locations += 1;
        plan.scopes.push(scope);
    }
    Ok(plan)
}
struct Run {
    root: DirHandle,
    dir: DirHandle,
    path: PathBuf,
    identity: FileIdentity,
    root_id: FileIdentity,
    mount: u64,
}
impl Run {
    fn verify(&self) -> io::Result<()> {
        let current = history_tree::open_dir(
            self.path
                .parent()
                .ok_or_else(|| invalid("Missing Trash run parent"))?,
        )?;
        let named = self.root.stat_child(
            self.path
                .file_name()
                .ok_or_else(|| invalid("Missing Trash run name"))?,
        )?;
        if dir_key(FileIdentity::from_stat(&current.stat_self()?)) != dir_key(self.root_id)
            || dir_key(FileIdentity::from_stat(&named)) != dir_key(self.identity)
            || dir_key(FileIdentity::from_stat(&self.dir.stat_self()?)) != dir_key(self.identity)
        {
            return Err(invalid("Empty Trash private namespace moved or changed"));
        }
        for dir in [&current, &self.root, &self.dir] {
            private(dir)?;
            if permanent::mount_id(dir, OsStr::new("")).map_err(delete_failure)? != self.mount {
                return Err(invalid("Empty Trash private mount changed"));
            }
        }
        Ok(())
    }
    fn verify_pair(&self, pair: &DirHandle, name: &str, expected: FileIdentity) -> io::Result<()> {
        self.verify()?;
        private(pair)?;
        check_mount_at(&self.dir, OsStr::new(name), Some(self.mount))?;
        let current = history_tree::open_dir(&self.path.join(name))?;
        for dir in [pair, &current] {
            if dir_key(FileIdentity::from_stat(&dir.stat_self()?)) != dir_key(expected)
                || permanent::mount_id(dir, OsStr::new("")).map_err(delete_failure)? != self.mount
            {
                return Err(invalid("Empty Trash item namespace changed"));
            }
        }
        if dir_key(FileIdentity::from_stat(
            &self.dir.stat_child(OsStr::new(name))?,
        )) != dir_key(expected)
        {
            return Err(invalid("Empty Trash item name changed"));
        }
        Ok(())
    }
}
fn write_run_intent(dir: &DirHandle, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::from(rustix::fs::openat(
        dir.as_fd(),
        "intent.json",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    file.write_all(bytes)?;
    file.sync_all()?;
    rustix::fs::fsync(dir.as_fd())?;
    Ok(())
}
fn run_for(scope: &Scope, retained: &mut Vec<PathBuf>) -> io::Result<Run> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    scope.handles()?;
    let root = history_tree::open_dir(scope.location.trash.root())?;
    if dir_key(FileIdentity::from_stat(&root.stat_self()?)) != dir_key(scope.root)
        || permanent::mount_id(&root, OsStr::new("")).map_err(delete_failure)? != scope.mount
    {
        return Err(invalid("Empty Trash root changed before staging"));
    }
    private(&root)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let name = format!(
        ".gust-empty-{}-{stamp}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    rustix::fs::mkdirat(root.as_fd(), name.as_str(), Mode::from_bits_truncate(0o700))?;
    let path = scope.location.trash.root().join(&name);
    retained.push(path.clone());
    let dir = root.open_child_dir(OsStr::new(&name))?;
    private(&dir)?;
    rustix::fs::fsync(root.as_fd())?;
    let identity = FileIdentity::from_stat(&dir.stat_self()?);
    let bytes = serde_json::to_vec(
        &serde_json::json!({"version":1,"irreversible":true,"items":scope.receipts.len(),"location":scope.location.trash.root().as_os_str().as_bytes()}),
    )?;
    write_run_intent(&dir, &bytes)?;
    Ok(Run {
        root,
        dir,
        path,
        identity,
        root_id: scope.root,
        mount: scope.mount,
    })
}
impl ConfirmedEmptyTrash {
    /// Worker-only; callback must be nonpanicking and publish memory values.
    /// Caller retains its forward marker and owns History through this call.
    pub fn execute(
        self,
        history: &mut History,
        cancel: &CancelToken,
        mut progress: impl FnMut(EmptyTrashProgress),
    ) -> Result<EmptyTrashOutcome, EmptyTrashError> {
        let mut plan = self.plan;
        let summary = plan.summary;
        let mut outcome = EmptyTrashOutcome {
            retained_entries: summary.retained_entries,
            ..EmptyTrashOutcome::default()
        };
        let mut retained = Vec::new();
        let result = (|| -> Result<(), EmptyTrashError> {
            check(cancel).map_err(fail)?;
            for scope in &plan.scopes {
                scope.verify_names(cancel).map_err(fail)?;
                for receipt in &scope.receipts {
                    receipt.verify(cancel).map_err(fail)?;
                }
            }
            if summary.items == 0 {
                return Ok(());
            }
            history
                .record_barrier("Empty Trash (irreversible)")
                .map_err(|error| EmptyTrashError {
                    detail: error.to_string(),
                    reason: EmptyTrashFailure::Barrier(Box::new(error)),
                    outcome,
                    retained: Vec::new(),
                })?;
            for scope in &mut plan.scopes {
                if scope.receipts.is_empty() {
                    continue;
                }
                scope.verify_names(cancel).map_err(fail)?;
                let run = run_for(scope, &mut retained).map_err(fail)?;
                scope.run = Some((
                    run.path.file_name().expect("generated run name").into(),
                    run.identity,
                ));
                for index in 0..scope.receipts.len() {
                    check(cancel).map_err(fail)?;
                    run.verify().map_err(fail)?;
                    scope.verify_names(cancel).map_err(fail)?;
                    let receipt = &scope.receipts[index];
                    receipt.verify(cancel).map_err(fail)?;
                    let (files, info) = scope.handles().map_err(fail)?;
                    let pair_name = format!("item-{index:06}");
                    rustix::fs::mkdirat(
                        run.dir.as_fd(),
                        pair_name.as_str(),
                        Mode::from_bits_truncate(0o700),
                    )
                    .map_err(|e| fail(e.into()))?;
                    let pair = run
                        .dir
                        .open_child_dir(OsStr::new(&pair_name))
                        .map_err(fail)?;
                    private(&pair).map_err(fail)?;
                    let pair_path = run.path.join(&pair_name);
                    let pair_id = FileIdentity::from_stat(&pair.stat_self().map_err(fail)?);
                    run.verify_pair(&pair, &pair_name, pair_id).map_err(fail)?;
                    let bytes = serde_json::to_vec(&serde_json::json!({
                        "version": 1,
                        "slot": receipt.name.as_bytes(),
                        "data": crate::journal::RecordedIdentity::from(receipt.data),
                        "info": crate::journal::RecordedIdentity::from(receipt.info),
                        "irreversible": true
                    }))
                    .map_err(|e| fail(e.into()))?;
                    write_run_intent(&pair, &bytes).map_err(fail)?;
                    rustix::fs::fsync(run.dir.as_fd()).map_err(|e| fail(e.into()))?;
                    let iname = info_name(&receipt.name).map_err(fail)?;
                    let info_path = receipt.info_path();
                    let info_tree = BTreeMap::from([(info_path.clone(), receipt.info)]);
                    run.verify_pair(&pair, &pair_name, pair_id).map_err(fail)?;
                    move_checked_on_mount(
                        &info,
                        &iname,
                        &info_path,
                        &pair,
                        OsStr::new("info"),
                        &pair_path.join("info"),
                        &info_tree,
                        cancel,
                        Some(scope.mount),
                    )
                    .map_err(fail)?;
                    let (_, id, bytes) =
                        open_info_with_mount(&pair, OsStr::new("info"), Some(scope.mount))
                            .map_err(fail)?;
                    if id != receipt.info || bytes != receipt.info_bytes {
                        return Err(fail(invalid("Claimed Trash information changed")));
                    }
                    outcome.information_retained += 1;
                    scope.info.remove(&iname);
                    scope.verify_names(cancel).map_err(fail)?;
                    run.verify_pair(&pair, &pair_name, pair_id).map_err(fail)?;
                    move_checked_on_mount(
                        &files,
                        &receipt.name,
                        &receipt.data_path(),
                        &pair,
                        OsStr::new("data"),
                        &pair_path.join("data"),
                        &receipt.tree,
                        cancel,
                        Some(scope.mount),
                    )
                    .map_err(fail)?;
                    scope.files.remove(&receipt.name);
                    let admitted = relative_tree(receipt).map_err(fail)?;
                    #[cfg(test)]
                    BEFORE_DELETE.with(|h| {
                        let run = h.borrow_mut().take();
                        if let Some(run) = run {
                            run();
                        }
                    });
                    scope.verify_names(cancel).map_err(fail)?;
                    let (_, id, bytes) =
                        open_info_with_mount(&pair, OsStr::new("info"), Some(scope.mount))
                            .map_err(fail)?;
                    if id != receipt.info || bytes != receipt.info_bytes {
                        return Err(fail(invalid(
                            "Claimed Trash information changed before deletion",
                        )));
                    }
                    run.verify_pair(&pair, &pair_name, pair_id).map_err(fail)?;
                    let previous = outcome;
                    let result = permanent::delete_admitted(
                        &pair_path,
                        &pair_path.join("data"),
                        &admitted,
                        (pair_id, scope.mount),
                        cancel,
                        &|| {
                            check(cancel)
                                .and_then(|_| scope.handles().map(|_| ()))
                                .and_then(|_| run.verify_pair(&pair, &pair_name, pair_id))
                                .map_err(checkpoint_failure)
                        },
                        |p| {
                            let current = EmptyTrashOutcome {
                                removed: Removed {
                                    files: previous.removed.files + p.removed.files,
                                    dirs: previous.removed.dirs + p.removed.dirs,
                                },
                                ..previous
                            };
                            progress(EmptyTrashProgress {
                                phase: EmptyTrashPhase::Deleting,
                                total_items: summary.items,
                                total_data_entries: summary.data_entries,
                                outcome: current,
                            });
                        },
                    );
                    let removed = match result {
                        Ok(removed) => removed,
                        Err(error) => {
                            outcome.removed.files += error.removed.files;
                            outcome.removed.dirs += error.removed.dirs;
                            retained.extend(error.retained);
                            return Err(EmptyTrashError {
                                reason: EmptyTrashFailure::Delete(error.reason),
                                outcome,
                                retained: Vec::new(),
                                detail: "Confirmed Trash data deletion stopped; private claims and information were retained.".into(),
                            });
                        }
                    };
                    outcome.removed.files += removed.files;
                    outcome.removed.dirs += removed.dirs;
                    run.verify_pair(&pair, &pair_name, pair_id).map_err(fail)?;
                    scope.verify_names(cancel).map_err(fail)?;
                    let (_, id, bytes) =
                        open_info_with_mount(&pair, OsStr::new("info"), Some(scope.mount))
                            .map_err(fail)?;
                    if id != receipt.info || bytes != receipt.info_bytes {
                        return Err(fail(invalid("Retained Trash information changed")));
                    }
                    outcome.items_completed += 1;
                }
                run.verify().map_err(fail)?;
            }
            for scope in &plan.scopes {
                scope.verify_names(cancel).map_err(fail)?;
            }
            Ok(())
        })();
        if let Err(mut error) = result {
            error.outcome = outcome;
            error.retained.extend(retained);
            return Err(error);
        }
        progress(EmptyTrashProgress {
            phase: EmptyTrashPhase::Finished,
            total_items: summary.items,
            total_data_entries: summary.data_entries,
            outcome,
        });
        Ok(outcome)
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_DELETE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
#[path = "trash_empty_tests.rs"]
mod tests;
