use super::matcher::Matcher;
use super::{
    looks_binary, ContentHit, ContentMatchError, ContentQueryError, ContentSnippet, SearchScope,
    MAX_FILE_BYTES, SNIFF_BYTES,
};
use crate::CancelToken;
use serde::Serialize;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub const MAX_SEARCH_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_SCAN_ENTRIES: usize = 100_000;
pub const MAX_CONTENT_HITS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSearchLimit {
    Hits,
    Entries,
    Bytes,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ContentSearchState {
    Partial,
    Limited(ContentSearchLimit),
    Cancelled,
    InvalidQuery(ContentQueryError),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailedContentHit {
    pub hit: ContentHit,
    pub snippet: ContentSnippet,
}
#[derive(Debug, Clone, Serialize)]
pub struct ContentSearchStats {
    pub entries: usize,
    pub read_bytes: u64,
    pub read_errors: u64,
    pub entry_errors: u64,
    pub depth_pruned: u64,
    pub device_pruned: u64,
    pub hidden_pruned: u64,
    pub symlink_pruned: u64,
    pub binary_files: u64,
    pub ascii_fallback_files: u64,
    pub file_byte_limits: u64,
    pub scope_unverified: bool,
}
#[derive(Debug, Clone)]
pub struct ContentSearchReport {
    pub hits: Vec<DetailedContentHit>,
    pub state: ContentSearchState,
    pub stats: ContentSearchStats,
}

#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: MAX_SCAN_ENTRIES,
            bytes: MAX_SEARCH_BYTES,
        }
    }
}

/// Retains the existing iterative walk. This report never claims anchored coverage.
pub fn grep_with_report(
    scope: &SearchScope,
    needle: &str,
    cancel: &CancelToken,
    on_hit: impl FnMut(&DetailedContentHit),
) -> ContentSearchReport {
    walk(scope, needle, cancel, on_hit, Limits::default())
}
fn walk(
    scope: &SearchScope,
    needle: &str,
    cancel: &CancelToken,
    mut on_hit: impl FnMut(&DetailedContentHit),
    limits: Limits,
) -> ContentSearchReport {
    let mut report = ContentSearchReport {
        hits: Vec::new(),
        state: ContentSearchState::Partial,
        stats: ContentSearchStats {
            entries: 0,
            read_bytes: 0,
            read_errors: 0,
            entry_errors: 0,
            depth_pruned: 0,
            device_pruned: 0,
            hidden_pruned: 0,
            symlink_pruned: 0,
            binary_files: 0,
            ascii_fallback_files: 0,
            file_byte_limits: 0,
            scope_unverified: true,
        },
    };
    if cancel.is_cancelled() {
        report.state = ContentSearchState::Cancelled;
        return report;
    }
    let mut matcher = match Matcher::new(needle) {
        Ok(m) => m,
        Err(e) => {
            report.state = ContentSearchState::InvalidQuery(e);
            return report;
        }
    };
    let Some(root) = scope.root.as_path() else {
        report.stats.read_errors += 1;
        return report;
    };
    let root_dev = match std::fs::metadata(root) {
        Ok(m) => Some(m.dev()),
        Err(_) => {
            report.stats.read_errors += 1;
            None
        }
    };
    let mut stack = vec![(root.to_path_buf(), 0)];
    let hit_limit = scope.max_results.min(MAX_CONTENT_HITS);
    'walk: while let Some((dir, depth)) = stack.pop() {
        if cancel.is_cancelled() {
            report.state = ContentSearchState::Cancelled;
            break;
        }
        if report.hits.len() >= hit_limit {
            report.state = ContentSearchState::Limited(ContentSearchLimit::Hits);
            break;
        }
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => {
                report.stats.read_errors += 1;
                continue;
            }
        };
        for entry in rd {
            if cancel.is_cancelled() {
                report.state = ContentSearchState::Cancelled;
                break 'walk;
            }
            if report.hits.len() >= hit_limit {
                report.state = ContentSearchState::Limited(ContentSearchLimit::Hits);
                break 'walk;
            }
            if report.stats.entries >= limits.entries {
                report.state = ContentSearchState::Limited(ContentSearchLimit::Entries);
                break 'walk;
            }
            report.stats.entries += 1;
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    report.stats.entry_errors += 1;
                    continue;
                }
            };
            use std::os::unix::ffi::OsStrExt;
            if !scope.include_hidden && entry.file_name().as_bytes().starts_with(b".") {
                report.stats.hidden_pruned += 1;
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    report.stats.read_errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            if meta.is_symlink() {
                report.stats.symlink_pruned += 1;
                continue;
            }
            if scope.stay_on_device && root_dev.is_some_and(|d| meta.dev() != d) {
                report.stats.device_pruned += 1;
                continue;
            }
            if meta.is_dir() {
                if !scope.recursive || depth + 1 >= scope.max_depth {
                    report.stats.depth_pruned += 1;
                    continue;
                }
                stack.push((path, depth + 1));
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let remaining = limits.bytes.saturating_sub(report.stats.read_bytes);
            if remaining == 0 {
                report.state = ContentSearchState::Limited(ContentSearchLimit::Bytes);
                break 'walk;
            }
            let input = match read_bounded(&path, remaining, cancel, &mut report.stats.read_bytes) {
                Ok(input) => input,
                Err(ReadError::Cancelled) => {
                    report.state = ContentSearchState::Cancelled;
                    break 'walk;
                }
                Err(ReadError::Io) => {
                    report.stats.read_errors += 1;
                    if report.stats.read_bytes >= limits.bytes {
                        report.state = ContentSearchState::Limited(ContentSearchLimit::Bytes);
                        break 'walk;
                    }
                    continue;
                }
            };
            if input.binary {
                report.stats.binary_files += 1;
                if report.stats.read_bytes >= limits.bytes {
                    report.state = ContentSearchState::Limited(ContentSearchLimit::Bytes);
                    break 'walk;
                }
                continue;
            }
            if input.bytes.len() > MAX_FILE_BYTES as usize {
                report.stats.file_byte_limits += 1;
            }
            let matched = match matcher.buffer(&input.bytes, input.truncated, cancel) {
                Ok(m) => m,
                Err(ContentMatchError::Cancelled) => {
                    report.state = ContentSearchState::Cancelled;
                    break 'walk;
                }
                Err(ContentMatchError::InvalidQuery(e)) => {
                    report.state = ContentSearchState::InvalidQuery(e);
                    break 'walk;
                }
            };
            if matched.ascii_fallback {
                report.stats.ascii_fallback_files += 1;
            }
            if let Some(snippet) = matched.snippet {
                let hit = DetailedContentHit {
                    hit: ContentHit {
                        path,
                        lines: matched.lines,
                        first_line: matched.first_line,
                    },
                    snippet,
                };
                on_hit(&hit);
                report.hits.push(hit);
            }
            if report.stats.read_bytes >= limits.bytes {
                report.state = ContentSearchState::Limited(ContentSearchLimit::Bytes);
                break 'walk;
            }
        }
    }
    if cancel.is_cancelled() {
        report.state = ContentSearchState::Cancelled;
    }
    report
}

struct Input {
    bytes: Vec<u8>,
    binary: bool,
    truncated: bool,
}
enum ReadError {
    Io,
    Cancelled,
}
fn read_bounded(
    path: &Path,
    remaining: u64,
    cancel: &CancelToken,
    read_bytes: &mut u64,
) -> Result<Input, ReadError> {
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)
        .map_err(|_| ReadError::Io)?;
    if !file.metadata().map_err(|_| ReadError::Io)?.is_file() {
        return Err(ReadError::Io);
    }
    let cap = (MAX_FILE_BYTES + 1).min(remaining) as usize;
    let mut input = Input {
        bytes: Vec::new(),
        binary: false,
        truncated: false,
    };
    let mut buf = [0u8; 8192];
    while input.bytes.len() < cap {
        if cancel.is_cancelled() {
            return Err(ReadError::Cancelled);
        }
        let want = (cap - input.bytes.len()).min(buf.len());
        let n = file.read(&mut buf[..want]).map_err(|_| ReadError::Io)?;
        if n == 0 {
            return Ok(input);
        }
        *read_bytes += n as u64;
        input.bytes.extend_from_slice(&buf[..n]);
        if looks_binary(&input.bytes[..input.bytes.len().min(SNIFF_BYTES)]) {
            input.binary = true;
            return Ok(input);
        }
    }
    input.truncated = true;
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entry_and_total_byte_caps_are_explicit_and_keep_captured_hits() {
        let dir = std::env::temp_dir().join(format!("gust-content-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), b"needle\n").unwrap();
        std::fs::write(dir.join("b"), b"needle\n").unwrap();
        let scope = SearchScope::current_dir(crate::LocationKey::local(&dir));
        let entries = walk(
            &scope,
            "needle",
            &CancelToken::new(),
            |_| {},
            Limits {
                entries: 1,
                bytes: 100,
            },
        );
        assert_eq!(
            entries.state,
            ContentSearchState::Limited(ContentSearchLimit::Entries)
        );
        assert_eq!(entries.hits.len(), 1);
        let bytes = walk(
            &scope,
            "needle",
            &CancelToken::new(),
            |_| {},
            Limits {
                entries: 10,
                bytes: 6,
            },
        );
        assert_eq!(
            bytes.state,
            ContentSearchState::Limited(ContentSearchLimit::Bytes)
        );
        assert_eq!(bytes.stats.read_bytes, 6);
        assert_eq!(bytes.hits.len(), 1);
        assert!(bytes.hits[0].snippet.truncated);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
