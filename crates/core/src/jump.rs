//! Folder jump for Ctrl+L: type a few letters, land in the folder.
//!
//! Candidates come from three places the user already built up: bookmarks,
//! the folders of recently used files, and zoxide's database when zoxide is
//! installed. Gathering them touches the filesystem and runs a program, so
//! [`gather`] belongs on a worker; [`rank`] is pure and cheap, so it can run
//! on every keystroke.
//!
//! Tab completion ([`complete`]) finishes a typed path from the real folder
//! listing, the way a shell does.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Bookmark,
    Zoxide,
    Recent,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Bookmark => "bookmark",
            Source::Zoxide => "zoxide",
            Source::Recent => "recent",
        }
    }
    fn weight(self) -> i64 {
        match self {
            Source::Bookmark => 30,
            Source::Zoxide => 20,
            Source::Recent => 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub source: Source,
    /// Position in its own source's order (0 = most used / most recent).
    pub rank_in_source: usize,
}

/// Parse `zoxide query --list` output: one absolute path per line, most
/// frecent first.
pub fn parse_zoxide(out: &str) -> Vec<PathBuf> {
    out.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}

/// Merge the three sources: one entry per folder, keeping its best source.
/// `exists` decides which folders are still there (injected for tests).
pub fn merge(
    bookmarks: &[PathBuf],
    zoxide: &[PathBuf],
    recent_files: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |p: &Path, source: Source, i: usize| {
        if out.iter().any(|c| c.path == p) || !exists(p) {
            return;
        }
        out.push(Candidate {
            path: p.to_path_buf(),
            source,
            rank_in_source: i,
        });
    };
    for (i, p) in bookmarks.iter().enumerate() {
        push(p, Source::Bookmark, i);
    }
    for (i, p) in zoxide.iter().take(500).enumerate() {
        push(p, Source::Zoxide, i);
    }
    let mut folders: Vec<&Path> = Vec::new();
    for f in recent_files {
        if let Some(d) = f.parent() {
            if !folders.contains(&d) {
                folders.push(d);
            }
        }
    }
    for (i, d) in folders.into_iter().take(200).enumerate() {
        push(d, Source::Recent, i);
    }
    out
}

/// Score one path against a query, or `None` when it does not match.
///
/// Every query character must appear in order in the path (case-insensitive
/// subsequence). Matches in the LAST component count most: "dl" should find
/// ~/Downloads before /usr/lib/dl-something. Space-separated words must each
/// match, in order, like zoxide's own query.
fn score(path: &Path, query: &str) -> Option<i64> {
    let full = path.to_string_lossy().to_lowercase();
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Some(0);
    }
    let mut s: i64 = 0;
    let mut from = 0usize;
    for (wi, w) in words.iter().enumerate() {
        let last = wi == words.len() - 1;
        if last {
            // The last word is about the folder's own name.
            if base == *w {
                s += 400;
            } else if base.starts_with(w.as_str()) {
                s += 250;
            } else if base.contains(w.as_str()) {
                s += 150;
            } else if subseq(&base, w) {
                s += 60;
            } else if full[from..].contains(w.as_str()) {
                s += 20;
            } else if subseq(&full[from..], w) {
                s += 5;
            } else {
                return None;
            }
        } else {
            let at = full[from..].find(w.as_str())?;
            from += at + w.len();
            s += 40;
        }
    }
    // Shorter paths are nearer the top of the tree: slightly preferred.
    s -= (full.len() as i64) / 8;
    Some(s)
}

fn subseq(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// The best `limit` candidates for `query`, best first.
pub fn rank<'a>(query: &str, cands: &'a [Candidate], limit: usize) -> Vec<&'a Candidate> {
    let mut scored: Vec<(i64, &Candidate)> = cands
        .iter()
        .filter_map(|c| {
            let s = score(&c.path, query)?;
            // Source and position break ties and nudge, never override a
            // clearly better name match.
            let nudge = c.source.weight() - (c.rank_in_source as i64).min(50) / 2;
            Some((s + nudge, c))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored.into_iter().take(limit).map(|(_, c)| c).collect()
}

/// Expand a leading `~` to `home`.
pub fn expand(typed: &str, home: &Path) -> PathBuf {
    if typed == "~" {
        return home.to_path_buf();
    }
    match typed.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(typed),
    }
}

/// Tab completion. `typed` is what is in the box ("~/Doc", "/usr/sh");
/// `children` the folder names inside its parent (read on a worker).
/// Returns the new text, or `None` when nothing can be added. A unique
/// match gets a trailing `/`, so the next Tab descends.
pub fn complete(typed: &str, children: &[String]) -> Option<String> {
    let i = typed.rfind('/')?;
    let (dir_part, partial) = (&typed[..=i], &typed[i + 1..]);
    let lower = partial.to_lowercase();
    let mut hits: Vec<&String> = children.iter().filter(|c| c.starts_with(partial)).collect();
    if hits.is_empty() {
        // Case-insensitive only when nothing matches exactly.
        hits = children
            .iter()
            .filter(|c| c.to_lowercase().starts_with(&lower))
            .collect();
    }
    if !partial.starts_with('.') {
        hits.retain(|c| !c.starts_with('.'));
    }
    match hits.len() {
        0 => None,
        1 => Some(format!("{dir_part}{}/", hits[0])),
        _ => {
            let first = hits[0];
            let mut n = first.len();
            for h in &hits[1..] {
                n = n.min(
                    first
                        .char_indices()
                        .zip(h.chars())
                        .take_while(|((_, a), b)| a == b)
                        .count(),
                );
            }
            let common: String = first.chars().take(n).collect();
            if common.chars().count() > partial.chars().count() {
                Some(format!("{dir_part}{common}"))
            } else {
                None
            }
        }
    }
}

/// The folder whose children [`complete`] needs for `typed`.
pub fn completion_dir(typed: &str, home: &Path) -> Option<PathBuf> {
    let i = typed.rfind('/')?;
    let dir = &typed[..=i];
    Some(expand(dir.trim_end_matches('/'), home)).map(|p| {
        if p.as_os_str().is_empty() {
            PathBuf::from("/")
        } else {
            p
        }
    })
}

/// Gather candidates for real: bookmarks file, recent files, zoxide.
/// Runs zoxide with a 2 s limit; a missing or slow zoxide just contributes
/// nothing. Worker thread only.
pub fn gather(bookmarks: &[PathBuf]) -> Vec<Candidate> {
    let recent: Vec<PathBuf> = crate::recent::load(300)
        .into_iter()
        .map(|r| r.path)
        .collect();
    let zoxide = run_zoxide().unwrap_or_default();
    merge(bookmarks, &zoxide, &recent, &|p| p.is_dir())
}

fn run_zoxide() -> Option<Vec<PathBuf>> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new("zoxide")
        .args(["query", "--list"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut s = String::new();
    child.stdout.take()?.read_to_string(&mut s).ok()?;
    Some(parse_zoxide(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands(paths: &[(&str, Source)]) -> Vec<Candidate> {
        paths
            .iter()
            .enumerate()
            .map(|(i, (p, s))| Candidate {
                path: PathBuf::from(p),
                source: *s,
                rank_in_source: i,
            })
            .collect()
    }

    #[test]
    fn the_folder_name_beats_a_match_buried_in_the_path() {
        let c = cands(&[
            ("/usr/lib/dlopen-things", Source::Zoxide),
            ("/home/u/Downloads", Source::Recent),
        ]);
        let r = rank("down", &c, 5);
        assert_eq!(r[0].path, PathBuf::from("/home/u/Downloads"));
    }

    #[test]
    fn subsequence_matches_like_a_fuzzy_finder() {
        let c = cands(&[("/home/u/Projects/gust", Source::Zoxide)]);
        assert_eq!(rank("gst", &c, 5).len(), 1);
        assert!(rank("xyz", &c, 5).is_empty());
    }

    #[test]
    fn words_match_in_order_like_zoxide() {
        let c = cands(&[
            ("/home/u/Projects/gust/docs", Source::Zoxide),
            ("/home/u/work/docs", Source::Zoxide),
        ]);
        let r = rank("gust docs", &c, 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].path, PathBuf::from("/home/u/Projects/gust/docs"));
    }

    #[test]
    fn a_bookmark_wins_a_tie() {
        let c = cands(&[("/a/music", Source::Recent), ("/b/music", Source::Bookmark)]);
        assert_eq!(rank("music", &c, 5)[0].path, PathBuf::from("/b/music"));
    }

    #[test]
    fn merge_dedupes_keeps_the_best_source_and_drops_gone_folders() {
        let b = vec![PathBuf::from("/x/Music")];
        let z = vec![PathBuf::from("/x/Music"), PathBuf::from("/x/gone")];
        let r = vec![
            PathBuf::from("/x/Music/a.mp3"),
            PathBuf::from("/x/Docs/b.pdf"),
        ];
        let m = merge(&b, &z, &r, &|p| p != Path::new("/x/gone"));
        let paths: Vec<_> = m
            .iter()
            .map(|c| (c.path.to_str().unwrap(), c.source))
            .collect();
        assert_eq!(
            paths,
            vec![("/x/Music", Source::Bookmark), ("/x/Docs", Source::Recent)]
        );
    }

    #[test]
    fn zoxide_output_is_parsed_one_path_per_line() {
        let z = parse_zoxide("/home/u/a\n  /home/u/b  \nnot a path\n\n");
        assert_eq!(
            z,
            vec![PathBuf::from("/home/u/a"), PathBuf::from("/home/u/b")]
        );
    }

    #[test]
    fn tab_completes_a_unique_folder_and_adds_a_slash() {
        let kids = vec![
            "Documents".to_string(),
            "Downloads".to_string(),
            "Music".to_string(),
        ];
        assert_eq!(complete("~/Mu", &kids).as_deref(), Some("~/Music/"));
        assert_eq!(
            complete("~/mu", &kids).as_deref(),
            Some("~/Music/"),
            "case-insensitive fallback"
        );
    }

    #[test]
    fn tab_extends_to_the_common_prefix_when_ambiguous() {
        let kids = vec!["Documents".to_string(), "Downloads".to_string()];
        assert_eq!(complete("~/D", &kids).as_deref(), Some("~/Do"));
        assert_eq!(complete("~/Do", &kids), None, "nothing more in common");
    }

    #[test]
    fn hidden_folders_complete_only_when_asked_for() {
        let kids = vec![".config".to_string(), "code".to_string()];
        assert_eq!(complete("~/c", &kids).as_deref(), Some("~/code/"));
        assert_eq!(complete("~/.c", &kids).as_deref(), Some("~/.config/"));
    }

    #[test]
    fn the_completion_dir_is_the_typed_parent() {
        let home = Path::new("/home/u");
        assert_eq!(
            completion_dir("~/Doc", home),
            Some(PathBuf::from("/home/u"))
        );
        assert_eq!(completion_dir("/usr/sh", home), Some(PathBuf::from("/usr")));
        assert_eq!(completion_dir("/us", home), Some(PathBuf::from("/")));
        assert_eq!(
            completion_dir("~/a/b/c", home),
            Some(PathBuf::from("/home/u/a/b"))
        );
    }
}
