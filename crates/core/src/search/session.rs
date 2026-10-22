//! Resumable, bounded, worker-owned filename/tag retrieval.
use super::{match_score, SearchHit, SearchScope};
use crate::{
    entry::{EntryKind, Metadata},
    lister::CancelToken,
    tags,
};
use std::fs::{self, ReadDir};
use std::os::fd::AsRawFd;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    pub name: String,
    pub tags: Vec<String>,
}

impl SearchQuery {
    pub fn tagged(tag: &str) -> Result<Self, &'static str> {
        let tag = tag.trim();
        if tag.is_empty() || tag.contains(',') || tag.chars().any(char::is_control) {
            return Err("A tag query needs one nonempty tag name");
        }
        Ok(Self {
            name: String::new(),
            tags: vec![tag.to_owned()],
        })
    }
    pub fn filename(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tags: Vec::new(),
        }
    }

    /// Quotes allow spaces in a tag; backslash escapes a quote/backslash.
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quoted = false;
        let mut escaped = false;
        for c in input.chars() {
            if escaped {
                word.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = !quoted;
            } else if c.is_whitespace() && !quoted {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                word.push(c);
            }
        }
        if quoted || escaped {
            return Err("Unclosed quote or escape in query");
        }
        if !word.is_empty() {
            words.push(word);
        }
        let mut out = Self::default();
        let mut names = Vec::new();
        for word in words {
            if let Some(tag) = word.strip_prefix("tag:") {
                let tag = tag.trim();
                if tag.is_empty() || tag.contains(',') || tag.chars().any(char::is_control) {
                    return Err("A tag query needs one nonempty tag name");
                }
                out.tags.push(tag.to_owned());
            } else {
                names.push(word);
            }
        }
        out.name = names.join(" ");
        Ok(out)
    }
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub max_visited: usize,
    pub max_candidates: usize,
    pub max_path_bytes: usize,
    pub max_issues: usize,
    pub skip_generated: bool,
    /// Explicit admission for this root after the caller's remote stall guard.
    pub allow_remote: bool,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_visited: 200_000,
            max_candidates: 100_000,
            max_path_bytes: 32 * 1024 * 1024,
            max_issues: 32,
            skip_generated: true,
            allow_remote: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchLimit {
    Results,
    Depth,
    VisitedEntries,
    CachedCandidates,
    PathBytes,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchState {
    Running,
    Complete,
    Partial,
    Cancelled,
    Limited(SearchLimit),
    Failed(String),
}
#[derive(Debug, Clone)]
pub struct SearchIssue {
    pub path: PathBuf,
    pub operation: &'static str,
    pub kind: std::io::ErrorKind,
}
#[derive(Debug, Clone, Default)]
pub struct SearchStats {
    pub visited: usize,
    pub cached: usize,
    pub path_bytes: usize,
    pub hidden: usize,
    pub depth_pruned: usize,
    pub device_pruned: usize,
    pub generated_pruned: usize,
    pub issue_count: usize,
}
#[derive(Debug, Clone)]
pub struct SearchReport {
    pub generation: u64,
    pub hits: Vec<SearchHit>,
    pub state: SearchState,
    pub stats: SearchStats,
    pub issues: Vec<SearchIssue>,
}
/// Identity and revision of the held object used to produce a search row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSourceStamp {
    pub generation: u64,
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime: i64,
    pub mtime_ns: i64,
    pub ctime: i64,
    pub ctime_ns: i64,
}
impl SearchSourceStamp {
    fn of(meta: &fs::Metadata, generation: u64) -> Self {
        Self {
            generation,
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime(),
            mtime_ns: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_ns: meta.ctime_nsec(),
        }
    }
}
/// Plain row data captured from a held object. It is not operation authority.
#[derive(Debug, Clone)]
pub struct SearchEntry {
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub kind: EntryKind,
    pub metadata: Metadata,
    pub source: SearchSourceStamp,
    /// A zero tag mask is unknown when an error is present here.
    pub tag_error: Option<std::io::ErrorKind>,
}
#[derive(Debug, Clone)]
pub struct SearchProjection {
    pub report: SearchReport,
    pub entries: Vec<SearchEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DirectoryId {
    dev: u64,
    ino: u64,
}
impl DirectoryId {
    fn of(file: &fs::File) -> std::io::Result<Self> {
        let meta = file.metadata()?;
        Ok(Self {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
}
struct Candidate {
    path: PathBuf,
    depth: usize,
    parent_id: DirectoryId,
}
struct Directory {
    path: PathBuf,
    depth: usize,
    entries: ReadDir,
    handle: fs::File,
    id: DirectoryId,
}

impl Directory {
    fn open(
        path: PathBuf,
        depth: usize,
        parent: Option<&fs::File>,
        device: Option<u64>,
    ) -> std::io::Result<Self> {
        use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let handle = match parent {
            Some(parent) => open_beneath(
                parent,
                Path::new(path.file_name().unwrap_or_default()),
                flags,
                device.is_some(),
            )?,
            None => fs::File::from(openat2(
                rustix::fs::CWD,
                &path,
                flags,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )?),
        };
        let id = DirectoryId::of(&handle)?;
        if device.is_some_and(|dev| dev != id.dev) {
            return Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
        }
        // Enumerate the held object, so replacing a child with a symlink
        // cannot redirect the traversal between validation and readdir.
        let entries = fs::read_dir(format!("/proc/self/fd/{}", handle.as_raw_fd()))?;
        Ok(Self {
            path,
            depth,
            entries,
            handle,
            id,
        })
    }
}

/// Every user-controlled component is resolved beneath a held directory.
fn open_beneath(
    parent: &fs::File,
    relative: &Path,
    flags: rustix::fs::OFlags,
    stay_on_device: bool,
) -> std::io::Result<fs::File> {
    use rustix::fs::{openat2, Mode, ResolveFlags};
    let mut resolve =
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
    if stay_on_device {
        resolve |= ResolveFlags::NO_XDEV;
    }
    Ok(fs::File::from(openat2(
        parent,
        relative,
        flags,
        Mode::empty(),
        resolve,
    )?))
}

fn read_held_tags(object: &fs::File) -> std::io::Result<Option<Vec<u8>>> {
    use std::os::unix::fs::OpenOptionsExt;
    let meta = object.metadata()?;
    // Linux user attributes cannot be attached to symlinks or special files.
    if !meta.is_file() && !meta.is_dir() {
        return Ok(None);
    }
    // O_PATH supports safe identity reads but not fgetxattr. Reopen the held
    // regular file/directory via procfs, never via its mutable user pathname.
    let reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(format!("/proc/self/fd/{}", object.as_raw_fd()))?;
    let mut raw = vec![0u8; tags::MAX_VALUE];
    match rustix::fs::fgetxattr(&reader, tags::ATTR, &mut raw) {
        Ok(size) => {
            raw.truncate(size);
            Ok(Some(raw))
        }
        Err(e) if e == rustix::io::Errno::NODATA || e == rustix::io::Errno::NOTSUP => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Own on a worker. A query change replays candidates without another walk.
pub struct SearchSession {
    scope: SearchScope,
    options: SearchOptions,
    query: SearchQuery,
    generation: u64,
    candidates: Vec<Candidate>,
    replay: usize,
    stack: Vec<Directory>,
    root_dev: Option<u64>,
    root_handle: Option<fs::File>,
    walk_state: SearchState,
    started: bool,
    result_limited: bool,
    cancelled: bool,
    hits: Vec<(SearchHit, usize, SearchEntry)>,
    stats: SearchStats,
    issues: Vec<(SearchIssue, bool)>,
    walk_issue_count: usize,
}
impl SearchSession {
    pub fn new(scope: SearchScope, options: SearchOptions) -> Self {
        Self {
            scope,
            options,
            query: SearchQuery::default(),
            generation: 0,
            candidates: Vec::new(),
            replay: 0,
            stack: Vec::new(),
            root_dev: None,
            root_handle: None,
            walk_state: SearchState::Running,
            started: false,
            result_limited: false,
            cancelled: false,
            hits: Vec::new(),
            stats: SearchStats::default(),
            issues: Vec::new(),
            walk_issue_count: 0,
        }
    }
    pub fn set_query(&mut self, query: SearchQuery) {
        self.query = query;
        self.refresh();
    }
    /// Replay cached paths with current metadata and tags, without a fresh walk.
    pub fn refresh(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.replay = 0;
        self.hits.clear();
        self.result_limited = false;
        self.cancelled = false;
        self.issues.retain(|(_, from_walk)| *from_walk);
        self.stats.issue_count = self.walk_issue_count;
    }
    /// Call for directory-watch changes; no filesystem work occurs here.
    pub fn invalidate(&mut self) {
        let query = self.query.clone();
        let generation = self.generation;
        *self = Self::new(self.scope.clone(), self.options.clone());
        self.generation = generation;
        self.set_query(query);
    }
    fn issue(&mut self, path: PathBuf, operation: &'static str, error: std::io::Error) {
        self.record_issue(path, operation, error, true);
    }
    fn query_issue(&mut self, path: PathBuf, operation: &'static str, error: std::io::Error) {
        self.record_issue(path, operation, error, false);
    }
    fn record_issue(
        &mut self,
        path: PathBuf,
        operation: &'static str,
        error: std::io::Error,
        from_walk: bool,
    ) {
        if from_walk {
            self.walk_issue_count = self.walk_issue_count.saturating_add(1);
        }
        self.stats.issue_count = self.stats.issue_count.saturating_add(1);
        if self.issues.len() < self.options.max_issues {
            self.issues.push((
                SearchIssue {
                    path,
                    operation,
                    kind: error.kind(),
                },
                from_walk,
            ));
        }
    }
    fn start(&mut self) {
        self.started = true;
        let Some(root) = self.scope.root.as_path().map(Path::to_path_buf) else {
            self.walk_state = SearchState::Failed("Local search requires a local location".into());
            return;
        };
        if !self.options.allow_remote && crate::stall::needs_probe(&root) {
            self.walk_state = SearchState::Failed(
                "Remote search requires explicit opt-in for this location".into(),
            );
            return;
        }
        if self.scope.max_depth == 0 {
            self.stats.depth_pruned = 1;
            self.walk_state = SearchState::Complete;
            return;
        }
        match Directory::open(root.clone(), 0, None, None) {
            Ok(dir) => {
                self.root_dev = Some(dir.id.dev);
                match dir.handle.try_clone() {
                    Ok(handle) => self.root_handle = Some(handle),
                    Err(e) => {
                        self.issue(root, "retain_root", e);
                        self.walk_state = SearchState::Failed("Cannot retain search root".into());
                        return;
                    }
                }
                self.stack.push(dir);
            }
            Err(e) => {
                self.issue(root, "read_dir", e);
                self.walk_state = SearchState::Failed("Cannot read search root".into());
            }
        }
    }
    fn restrict_device(&self) -> bool {
        self.scope.stay_on_device || !self.options.allow_remote
    }
    fn validated_parent(&self, path: &Path, expected: DirectoryId) -> std::io::Result<fs::File> {
        use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};
        let invalid = || std::io::Error::other("Search directory changed; invalidate the session");
        let root = self.root_handle.as_ref().ok_or_else(invalid)?;
        let root_path = self.scope.root.as_path().ok_or_else(invalid)?;
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let current_root = fs::File::from(openat2(
            rustix::fs::CWD,
            root_path,
            flags,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )?);
        if DirectoryId::of(&current_root)? != DirectoryId::of(root)? {
            return Err(invalid());
        }
        let relative = path
            .parent()
            .and_then(|p| p.strip_prefix(root_path).ok())
            .ok_or_else(invalid)?;
        let relative = if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        };
        let parent = open_beneath(
            root,
            relative,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            self.restrict_device(),
        )?;
        if DirectoryId::of(&parent)? != expected {
            return Err(invalid());
        }
        Ok(parent)
    }
    fn resolve_candidate(&self, path: &Path, parent_id: DirectoryId) -> std::io::Result<fs::File> {
        let parent = self.validated_parent(path, parent_id)?;
        open_beneath(
            &parent,
            Path::new(path.file_name().unwrap_or_default()),
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            self.restrict_device(),
        )
    }
    fn evaluate(&mut self, index: usize, on_hit: &mut impl FnMut(&SearchHit)) {
        let c = &self.candidates[index];
        let Some(score) = match_score(&self.query.name, c.path.file_name().unwrap_or_default())
        else {
            return;
        };
        let path = c.path.clone();
        let depth = c.depth;
        let parent_id = c.parent_id;
        let object = match self.resolve_candidate(&path, parent_id) {
            Ok(object) => object,
            Err(e) => {
                self.query_issue(path, "resolve_candidate", e);
                return;
            }
        };
        let meta = match object.metadata() {
            Ok(meta) => meta,
            Err(e) => {
                self.query_issue(path, "metadata", e);
                return;
            }
        };
        let source = SearchSourceStamp::of(&meta, self.generation);
        let mut tag_error = None;
        let raw = match read_held_tags(&object) {
            Ok(raw) => raw.unwrap_or_default(),
            Err(e) => {
                tag_error = Some(e.kind());
                if !self.query.tags.is_empty() {
                    self.query_issue(path.clone(), "tags", e);
                    return;
                }
                Vec::new()
            }
        };
        if std::str::from_utf8(&raw).is_err() {
            tag_error = Some(std::io::ErrorKind::InvalidData);
            if !self.query.tags.is_empty() {
                self.query_issue(
                    path.clone(),
                    "tags",
                    std::io::Error::from(std::io::ErrorKind::InvalidData),
                );
                return;
            }
        }
        let names = tags::parse(&raw);
        if !self
            .query
            .tags
            .iter()
            .all(|tag| tags::contains_tag(&names, tag))
        {
            return;
        }
        let current = self.resolve_candidate(&path, parent_id);
        match current
            .and_then(|current| Ok(DirectoryId::of(&current)? == DirectoryId::of(&object)?))
        {
            Ok(true) => {}
            Ok(false) => {
                self.query_issue(
                    path,
                    "changed_candidate",
                    std::io::Error::other("Search candidate changed"),
                );
                return;
            }
            Err(e) => {
                self.query_issue(path, "resolve_candidate", e);
                return;
            }
        }
        match object.metadata() {
            Ok(current) if SearchSourceStamp::of(&current, self.generation) == source => {}
            Ok(_) => {
                self.query_issue(
                    path,
                    "changed_candidate",
                    std::io::Error::other("Search candidate changed"),
                );
                return;
            }
            Err(e) => {
                self.query_issue(path, "metadata", e);
                return;
            }
        }
        if self.hits.len() >= self.scope.max_results {
            self.result_limited = true;
            return;
        }
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else if meta.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Special
        };
        let mount_root = if kind == EntryKind::Dir {
            use rustix::fs::{statx, AtFlags, StatxAttributes, StatxFlags};
            statx(
                &object,
                "",
                AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
                StatxFlags::empty(),
            )
            .is_ok_and(|stat| {
                stat.stx_attributes_mask
                    .contains(StatxAttributes::MOUNT_ROOT)
                    && stat.stx_attributes.contains(StatxAttributes::MOUNT_ROOT)
            })
        } else {
            false
        };
        let metadata = Metadata {
            size: meta.size(),
            mtime: meta.mtime(),
            mode: meta.mode(),
            uid: meta.uid(),
            gid: meta.gid(),
            dev: meta.dev(),
            ino: meta.ino(),
            link_ok: None,
            link_dir: false,
            mount_root,
            tags: if tag_error.is_none() {
                tags::mask_of(&names)
            } else {
                0
            },
        };
        let Some(relative_path) = self
            .scope
            .root
            .as_path()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(Path::to_path_buf)
        else {
            self.query_issue(
                path,
                "relative_path",
                std::io::Error::from(std::io::ErrorKind::InvalidInput),
            );
            return;
        };
        let entry = SearchEntry {
            path: path.clone(),
            relative_path,
            kind,
            metadata,
            source,
            tag_error,
        };
        let hit = SearchHit { path, score };
        on_hit(&hit);
        self.hits.push((hit, depth, entry));
    }
    /// At most `entry_budget` directory entries or cached candidates are examined.
    /// A callback is a provisional hit for this generation, not sorted output.
    pub fn advance(
        &mut self,
        entry_budget: usize,
        cancel: &CancelToken,
        mut on_hit: impl FnMut(&SearchHit),
    ) -> SearchReport {
        if cancel.is_cancelled() {
            self.cancelled = true;
            return self.snapshot();
        }
        if self.cancelled || self.result_limited {
            return self.snapshot();
        }
        if !self.started {
            self.start();
        }
        for _ in 0..entry_budget {
            if cancel.is_cancelled() {
                self.cancelled = true;
                break;
            }
            if self.replay < self.candidates.len() {
                let i = self.replay;
                self.replay += 1;
                self.evaluate(i, &mut on_hit);
            } else {
                if self.walk_state != SearchState::Running {
                    break;
                }
                let Some(dir) = self.stack.last_mut() else {
                    self.walk_state = SearchState::Complete;
                    break;
                };
                let depth = dir.depth;
                let parent_id = dir.id;
                let parent = dir.path.clone();
                let next = dir.entries.next();
                let Some(next) = next else {
                    self.stack.pop();
                    continue;
                };
                if self.stats.visited >= self.options.max_visited {
                    self.walk_state = SearchState::Limited(SearchLimit::VisitedEntries);
                    break;
                }
                self.stats.visited += 1;
                let entry = match next {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.issue(parent, "read_dir_entry", e);
                        continue;
                    }
                };
                let name = entry.file_name();
                if !self.scope.include_hidden && name.as_bytes().first() == Some(&b'.') {
                    self.stats.hidden += 1;
                    continue;
                }
                let path = parent.join(&name);
                let bytes = path.as_os_str().as_bytes().len();
                let limit = if self.candidates.len() >= self.options.max_candidates {
                    Some(SearchLimit::CachedCandidates)
                } else if bytes
                    > self
                        .options
                        .max_path_bytes
                        .saturating_sub(self.stats.path_bytes)
                {
                    Some(SearchLimit::PathBytes)
                } else {
                    None
                };
                if let Some(limit) = limit {
                    self.walk_state = SearchState::Limited(limit);
                    break;
                }
                self.stats.path_bytes += bytes;
                self.candidates.push(Candidate {
                    path: path.clone(),
                    depth,
                    parent_id,
                });
                self.stats.cached = self.candidates.len();
                self.replay += 1;
                self.evaluate(self.candidates.len() - 1, &mut on_hit);
                match entry.file_type() {
                    Ok(ft) if ft.is_dir() && self.scope.recursive => {
                        if depth + 1 >= self.scope.max_depth {
                            self.stats.depth_pruned += 1;
                        } else {
                            let device = if self.restrict_device() {
                                self.root_dev
                            } else {
                                None
                            };
                            let child =
                                self.validated_parent(&path, parent_id).and_then(|parent| {
                                    Directory::open(path.clone(), depth + 1, Some(&parent), device)
                                });
                            match child {
                                Ok(dir)
                                    if self.options.skip_generated
                                        && generated_tree(&dir.handle, &name) =>
                                {
                                    self.stats.generated_pruned += 1;
                                }
                                Ok(dir) => self.stack.push(dir),
                                Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
                                    self.stats.device_pruned += 1
                                }
                                Err(e) => self.issue(path, "read_dir", e),
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => self.issue(path, "file_type", e),
                }
            }
            if self.result_limited {
                break;
            }
        }
        self.snapshot()
    }
    fn ranked_hits(&self) -> Vec<&(SearchHit, usize, SearchEntry)> {
        let mut hits: Vec<_> = self.hits.iter().collect();
        hits.sort_by(|(a, da, _), (b, db, _)| {
            a.score
                .cmp(&b.score)
                .then(da.cmp(db))
                .then(a.path.cmp(&b.path))
        });
        hits
    }
    /// Pure projection of data captured during safe evaluation: never reopens paths.
    /// Consumers build rows from relative_path/kind/metadata, without fill_metadata.
    pub fn project_hits(&self, cancel: &CancelToken) -> SearchProjection {
        let mut report = self.snapshot();
        if cancel.is_cancelled() || self.cancelled {
            report.state = SearchState::Cancelled;
            report.hits.clear();
            return SearchProjection {
                report,
                entries: Vec::new(),
            };
        }
        let mut entries = Vec::new();
        for (_, _, entry) in self.ranked_hits() {
            if cancel.is_cancelled() {
                report.state = SearchState::Cancelled;
                report.hits.clear();
                return SearchProjection {
                    report,
                    entries: Vec::new(),
                };
            }
            if let Some(kind) = entry.tag_error {
                report.stats.issue_count = report.stats.issue_count.saturating_add(1);
                if report.issues.len() < self.options.max_issues {
                    report.issues.push(SearchIssue {
                        path: entry.path.clone(),
                        operation: "tags",
                        kind,
                    });
                }
                if report.state == SearchState::Complete {
                    report.state = SearchState::Partial;
                }
            }
            entries.push(entry.clone());
        }
        SearchProjection { report, entries }
    }
    pub fn snapshot(&self) -> SearchReport {
        let hits = self.ranked_hits();
        let state = if self.cancelled {
            SearchState::Cancelled
        } else if self.result_limited {
            SearchState::Limited(SearchLimit::Results)
        } else if self.replay < self.candidates.len() {
            SearchState::Running
        } else if self.walk_state == SearchState::Complete && self.stats.issue_count > 0 {
            SearchState::Partial
        } else if self.walk_state == SearchState::Complete && self.stats.depth_pruned > 0 {
            SearchState::Limited(SearchLimit::Depth)
        } else {
            self.walk_state.clone()
        };
        SearchReport {
            generation: self.generation,
            hits: hits.into_iter().map(|(h, _, _)| h.clone()).collect(),
            state,
            stats: self.stats.clone(),
            issues: self.issues.iter().map(|(issue, _)| issue.clone()).collect(),
        }
    }
}

fn generated_tree(dir: &fs::File, name: &std::ffi::OsStr) -> bool {
    if name == "node_modules" || name == "__pycache__" {
        return true;
    }
    use std::io::Read;
    let flags = rustix::fs::OFlags::RDONLY
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::NONBLOCK
        | rustix::fs::OFlags::CLOEXEC;
    let Ok(mut file) = open_beneath(dir, Path::new("CACHEDIR.TAG"), flags, true) else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut signature = [0u8; 43];
    file.read_exact(&mut signature).is_ok()
        && &signature == b"Signature: 8a477f597d28d172789f06886806bc55"
}
