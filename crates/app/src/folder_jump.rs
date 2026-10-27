//! Contextual folder suggestions from known paths, without remote I/O.
use gust_core::{
    entry::{Entry, EntryKind},
    jump::{Candidate, Source},
    mount_table::MountInfo,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_KNOWN: usize = 2048;

fn known() -> &'static Mutex<Vec<PathBuf>> {
    static KNOWN: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(Vec::new()))
}

/// Listing data is already in memory; remembering it must do no I/O.
pub fn remember(root: &Path, entries: &[Entry]) {
    let mut paths = known().lock().unwrap_or_else(|e| e.into_inner());
    paths.push(root.to_owned());
    paths.extend(
        entries
            .iter()
            .filter(|e| e.kind == EntryKind::Dir && !e.is_hidden())
            .take(256)
            .map(|e| root.join(&e.name)),
    );
    let mut seen = HashSet::new();
    paths.reverse();
    paths.retain(|p| seen.insert(p.clone()));
    paths.truncate(MAX_KNOWN);
    paths.reverse();
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    root: PathBuf,
    label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    places: Vec<Place>,
    active: Option<PathBuf>,
}

impl Context {
    /// Worker only. Mount topology is local kernel data, not a remote probe.
    pub fn load(current: &Path, places_json: &str) -> Self {
        use std::io::Read;
        let mut bytes = Vec::new();
        if let Ok(file) = std::fs::File::open("/proc/self/mountinfo") {
            let _ = file
                .take(gust_core::mount_table::MAX_MOUNTINFO_BYTES as u64 + 1)
                .read_to_end(&mut bytes);
        }
        Self::from_mounts(current, places_json, &bytes)
    }

    fn from_mounts(current: &Path, places_json: &str, bytes: &[u8]) -> Self {
        let mut context = Self::default();
        if let Ok(table) = MountInfo::parse(bytes) {
            if let Ok(mounts) = table.visible_mounts() {
                for mount in mounts {
                    if gust_core::stall::can_stall(&mount.fs_type) {
                        context.places.push(Place {
                            root: mount.mount_point.clone(),
                            label: mount
                                .mount_point
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "Remote".into()),
                        });
                    }
                }
            }
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(places_json) {
            for kind in ["ssh", "tailnet", "cloud"] {
                for place in value[kind].as_array().into_iter().flatten() {
                    if place["connected"].as_bool() != Some(true) {
                        continue;
                    }
                    let (Some(path), Some(label)) =
                        (place["path"].as_str(), place["name"].as_str())
                    else {
                        continue;
                    };
                    if let Some(mounted) = context
                        .places
                        .iter_mut()
                        .find(|p| p.root == Path::new(path))
                    {
                        mounted.label = label.to_owned();
                    }
                }
            }
        }
        context.active = context.place(current).map(|p| p.root.clone());
        context
    }

    fn place(&self, path: &Path) -> Option<&Place> {
        self.places
            .iter()
            .filter(|p| path.starts_with(&p.root))
            .max_by_key(|p| p.root.components().count())
    }

    fn is_active(&self, path: &Path) -> bool {
        self.place(path).map(|p| &p.root) == self.active.as_ref()
    }

    pub fn admit(&self, path: &Path) -> bool {
        // Known mounted folders are suggestions, not a promise of reachability.
        // Normal navigation probes whichever suggestion the user chooses.
        path.to_str().is_some()
            && (self.place(path).is_some()
                || (!gust_core::stall::needs_probe(path) && path.is_dir()))
    }

    pub fn augment(&self, candidates: &mut Vec<Candidate>) {
        let paths = known().lock().unwrap_or_else(|e| e.into_inner()).clone();
        self.add_paths(
            candidates,
            paths
                .into_iter()
                .chain(self.places.iter().map(|p| p.root.clone())),
        );
    }

    fn add_paths(&self, candidates: &mut Vec<Candidate>, paths: impl IntoIterator<Item = PathBuf>) {
        let mut seen: HashSet<PathBuf> = candidates.iter().map(|c| c.path.clone()).collect();
        for path in paths {
            if seen.contains(&path) || !self.admit(&path) {
                continue;
            }
            seen.insert(path.clone());
            candidates.push(Candidate {
                path,
                source: Source::Recent,
                rank_in_source: candidates.len(),
            });
        }
    }

    pub fn query(
        &self,
        query: &str,
        candidates: &[Candidate],
        limit: usize,
        home: Option<&Path>,
    ) -> String {
        let mut ranked = gust_core::jump::rank(query, candidates, candidates.len());
        // Stable partition: context wins first; name/source rank still decides
        // between folders on the same computer or connection.
        ranked.sort_by_key(|c| !self.is_active(&c.path));
        let rows: Vec<_> = ranked
            .into_iter()
            .take(limit)
            .filter_map(|c| {
                let path = c.path.to_str()?;
                let (shown, label) = if let Some(place) = self.place(&c.path) {
                    let relative = c.path.strip_prefix(&place.root).ok()?;
                    (
                        format!("{}:/{}", place.label, relative.to_string_lossy()),
                        place.label.as_str(),
                    )
                } else {
                    let shown = match home.and_then(|h| c.path.strip_prefix(h).ok()) {
                        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
                        Some(rest) => format!("~/{}", rest.to_string_lossy()),
                        None => path.into(),
                    };
                    (shown, "This device")
                };
                Some(serde_json::json!({"path": path,
                "shown": gust_core::location::escape_for_display(std::ffi::OsStr::new(&shown)),
                "source": gust_core::location::escape_for_display(std::ffi::OsStr::new(label))}))
            })
            .collect();
        serde_json::Value::Array(rows).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(current: &str) -> Context {
        Context::from_mounts(Path::new(current), r#"{"ssh":[{"name":"server-a","path":"/mnt/a","connected":true},{"name":"server-b","path":"/mnt/b","connected":true}]}"#,
            b"1 0 8:1 / / rw - ext4 /dev/test rw\n2 1 0:1 / /mnt/a rw - fuse.sshfs fixture rw\n3 1 0:2 / /mnt/b rw - fuse.sshfs fixture rw\n")
    }
    fn candidates() -> Vec<Candidate> {
        [
            "/home/test/Documents",
            "/mnt/b/Documents",
            "/mnt/a/Documents",
        ]
        .iter()
        .map(|p| Candidate {
            path: p.into(),
            source: Source::Bookmark,
            rank_in_source: 0,
        })
        .collect()
    }
    fn query(current: &str) -> Vec<serde_json::Value> {
        serde_json::from_str(&context(current).query(
            "Documents",
            &candidates(),
            3,
            Some(Path::new("/home/test")),
        ))
        .unwrap()
    }
    #[test]
    fn active_connection_wins_and_other_places_remain_labelled() {
        let rows = query("/mnt/a/work");
        assert_eq!(rows[0]["path"], "/mnt/a/Documents");
        assert_eq!(rows[0]["shown"], "server-a:/Documents");
        assert_eq!(rows[0]["source"], "server-a");
        assert!(rows.iter().any(|r| r["source"] == "This device"));
        assert!(rows.iter().any(|r| r["source"] == "server-b"));
        assert_eq!(query("/mnt/b")[0]["path"], "/mnt/b/Documents");
    }
    #[test]
    fn local_device_wins_when_browsing_locally() {
        assert_eq!(query("/home/test")[0]["path"], "/home/test/Documents");
        assert!(!context("/mnt/a").is_active(Path::new("/mnt/ab/Documents")));
    }
    #[test]
    fn mounted_candidates_are_admitted_without_stat_or_connect() {
        let context = context("/mnt/a");
        let mut candidates = Vec::new();
        context.add_paths(
            &mut candidates,
            [
                PathBuf::from("/mnt/a/not-a-real-folder"),
                PathBuf::from("/mnt/a/not-a-real-folder"),
            ],
        );
        assert_eq!(candidates.len(), 1);
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&context.query("real", &candidates, 1, None)).unwrap();
        assert_eq!(rows[0]["path"], "/mnt/a/not-a-real-folder");
    }

    #[test]
    fn listed_folders_are_suggested_without_crawling_or_exposing_hidden_children() {
        let root = Path::new("/mnt/a/cached-listing");
        remember(
            root,
            &[
                Entry::new("visible-child".into(), EntryKind::Dir),
                Entry::new(".hidden-child".into(), EntryKind::Dir),
                Entry::new("ordinary-file".into(), EntryKind::File),
                Entry::new("linked-folder".into(), EntryKind::Symlink),
            ],
        );
        let context = context("/mnt/a");
        let mut candidates = Vec::new();
        context.augment(&mut candidates);
        assert!(candidates
            .iter()
            .any(|c| c.path == root.join("visible-child")));
        for excluded in [".hidden-child", "ordinary-file", "linked-folder"] {
            assert!(!candidates.iter().any(|c| c.path == root.join(excluded)));
        }
    }
}
