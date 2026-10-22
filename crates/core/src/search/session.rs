//! Resumable, bounded, worker-owned filename/tag retrieval.
use super::{match_score, SearchHit, SearchScope};
use crate::{lister::CancelToken, tags};
use std::fs::{self, ReadDir};
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
struct Candidate {
    path: PathBuf,
    depth: usize,
}
struct Directory {
    path: PathBuf,
    depth: usize,
    entries: ReadDir,
    handle: fs::File,
}

impl Directory {
    fn open(
        path: PathBuf,
        depth: usize,
        parent: Option<&fs::File>,
        device: Option<u64>,
    ) -> std::io::Result<Self> {
        use rustix::fs::{open, openat, Mode, OFlags};
        use std::os::fd::AsRawFd;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = match parent {
            Some(parent) => openat(
                parent,
                path.file_name().unwrap_or_default(),
                flags,
                Mode::empty(),
            ),
            None => open(&path, flags, Mode::empty()),
        }?;
        let handle = fs::File::from(fd);
        let actual_device = handle.metadata()?.dev();
        if device.is_some_and(|dev| dev != actual_device) {
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
        })
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
    walk_state: SearchState,
    started: bool,
    result_limited: bool,
    cancelled: bool,
    hits: Vec<(SearchHit, usize)>,
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
        match fs::symlink_metadata(&root) {
            Ok(meta) if meta.is_dir() => self.root_dev = Some(meta.dev()),
            Ok(_) => {
                self.walk_state =
                    SearchState::Failed("Search root must be a real directory".into());
                return;
            }
            Err(e) => {
                self.issue(root, "metadata", e);
                self.walk_state = SearchState::Failed("Cannot inspect search root".into());
                return;
            }
        }
        match Directory::open(root.clone(), 0, None, None) {
            Ok(dir) => {
                self.root_dev = dir.handle.metadata().ok().map(|m| m.dev());
                self.stack.push(dir);
            }
            Err(e) => {
                self.issue(root, "read_dir", e);
                self.walk_state = SearchState::Failed("Cannot read search root".into());
            }
        }
    }
    fn evaluate(&mut self, index: usize, on_hit: &mut impl FnMut(&SearchHit)) {
        let c = &self.candidates[index];
        let Some(score) = match_score(&self.query.name, c.path.file_name().unwrap_or_default())
        else {
            return;
        };
        let path = c.path.clone();
        let depth = c.depth;
        if let Err(e) = fs::symlink_metadata(&path) {
            self.query_issue(path, "metadata", e);
            return;
        }
        if !self.query.tags.is_empty() {
            match tags::read_raw(&path) {
                Ok(raw) => {
                    let raw = raw.as_deref().unwrap_or_default();
                    if std::str::from_utf8(raw).is_err() {
                        self.query_issue(
                            path,
                            "tags",
                            std::io::Error::from(std::io::ErrorKind::InvalidData),
                        );
                        return;
                    }
                    let names = tags::parse(raw);
                    if !self
                        .query
                        .tags
                        .iter()
                        .all(|tag| tags::contains_tag(&names, tag))
                    {
                        return;
                    }
                }
                Err(e) => {
                    self.query_issue(path, "tags", e);
                    return;
                }
            }
        }
        if self.hits.len() >= self.scope.max_results {
            self.result_limited = true;
            return;
        }
        let hit = SearchHit { path, score };
        on_hit(&hit);
        self.hits.push((hit, depth));
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
                });
                self.stats.cached = self.candidates.len();
                self.replay += 1;
                self.evaluate(self.candidates.len() - 1, &mut on_hit);
                match entry.file_type() {
                    Ok(ft) if ft.is_dir() && self.scope.recursive => {
                        if depth + 1 >= self.scope.max_depth {
                            self.stats.depth_pruned += 1;
                        } else if self.options.skip_generated && generated_tree(&path, &name) {
                            self.stats.generated_pruned += 1;
                        } else {
                            let parent = self.stack.last().map(|dir| &dir.handle);
                            let device = if self.scope.stay_on_device {
                                self.root_dev
                            } else {
                                None
                            };
                            match Directory::open(path.clone(), depth + 1, parent, device) {
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
    pub fn snapshot(&self) -> SearchReport {
        let mut hits = self.hits.clone();
        hits.sort_by(|(a, da), (b, db)| {
            a.score
                .cmp(&b.score)
                .then(da.cmp(db))
                .then(a.path.cmp(&b.path))
        });
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
            hits: hits.into_iter().map(|(h, _)| h).collect(),
            state,
            stats: self.stats.clone(),
            issues: self.issues.iter().map(|(issue, _)| issue.clone()).collect(),
        }
    }
}

fn generated_tree(path: &Path, name: &std::ffi::OsStr) -> bool {
    if name == "node_modules" || name == "__pycache__" {
        return true;
    }
    // A mere file with this name must not hide ordinary user data.
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let Ok(mut file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path.join("CACHEDIR.TAG"))
    else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return false;
    }
    let mut signature = [0u8; 43];
    file.read_exact(&mut signature).is_ok()
        && &signature == b"Signature: 8a477f597d28d172789f06886806bc55"
}
