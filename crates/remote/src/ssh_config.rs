//! Non-executing SSH config parser.
//!
//! `ssh -G` would evaluate `Match exec` — running commands from a config file
//! as a side effect of *browsing*. Gust never does that. This is a bounded,
//! literal parse that suggests alias names only (plan §9).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// True if the block contains ProxyCommand/ProxyJump or similar. Such
    /// hosts still connect, but only on a deliberate user action.
    pub has_proxy: bool,
    /// An explicit starting folder, from a `#gust-path` comment in the
    /// Host block. Without one a connection lands in the login directory,
    /// which is whatever that account's home happens to be.
    pub remote_path: Option<String>,
}

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_LINES: usize = 20_000;

/// Parse literal `Host` aliases. Wildcard patterns (`*`, `?`) are NOT hosts —
/// they are defaults, and inventing a host named `*` would be a bug.
pub fn parse_ssh_config(path: &Path) -> Vec<SshHost> {
    let mut seen_files = HashSet::new();
    let mut out = Vec::new();
    parse_into(path, &mut out, &mut seen_files, 0, &mut 0);
    out
}

fn parse_into(
    path: &Path,
    out: &mut Vec<SshHost>,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
    lines_read: &mut usize,
) {
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
    let Ok(canon) = path.canonicalize() else {
        return;
    };
    // Include cycles terminate here.
    if !seen.insert(canon.clone()) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&canon) else {
        return;
    };

    let mut current: Option<SshHost> = None;
    // Inside a `Match` block, keys do not belong to the last Host alias.
    let mut in_match_block = false;

    for line in text.lines() {
        *lines_read += 1;
        if *lines_read > MAX_LINES {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            // `#gust-path /srv/data` names the folder a place opens in.
            // A comment is inert to ssh itself, so this adds no behaviour
            // to the user's configuration — it only tells Gust where to
            // start instead of defaulting to the login directory.
            if let Some(v) = c.trim().strip_prefix("gust-path") {
                let v = v.trim();
                if !v.is_empty() && !in_match_block {
                    if let Some(h) = current.as_mut() {
                        h.remote_path = Some(v.to_string());
                    }
                }
            }
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k.trim().to_ascii_lowercase(),
                v.trim().trim_start_matches('=').trim(),
            ),
            None => (line.to_ascii_lowercase(), ""),
        };

        match key.as_str() {
            "host" => {
                in_match_block = false;
                if let Some(h) = current.take() {
                    out.push(h);
                }
                // A Host line can carry several patterns; keep literal ones.
                let literal: Vec<&str> = value
                    .split_whitespace()
                    .filter(|p| !p.contains('*') && !p.contains('?') && !p.starts_with('!'))
                    .collect();
                if let Some(first) = literal.first() {
                    current = Some(SshHost {
                        alias: (*first).to_string(),
                        hostname: None,
                        user: None,
                        port: None,
                        has_proxy: false,
                        remote_path: None,
                    });
                    // Additional literal aliases become their own suggestions.
                    for extra in literal.iter().skip(1) {
                        out.push(SshHost {
                            alias: (*extra).to_string(),
                            hostname: None,
                            user: None,
                            port: None,
                            has_proxy: false,
                            remote_path: None,
                        });
                    }
                }
            }
            "match" => {
                // Never evaluated. Ends the current Host block.
                in_match_block = true;
                if let Some(h) = current.take() {
                    out.push(h);
                }
            }
            "include" => {
                let base = canon.parent().unwrap_or(Path::new("/"));
                for token in value.split_whitespace() {
                    let p = expand_include(token, base);
                    // Globs are expanded literally by directory scan, bounded.
                    if token.contains('*') {
                        if let Some(dir) = p.parent() {
                            if let Ok(rd) = std::fs::read_dir(dir) {
                                for e in rd.flatten().take(256) {
                                    parse_into(&e.path(), out, seen, depth + 1, lines_read);
                                }
                            }
                        }
                    } else {
                        parse_into(&p, out, seen, depth + 1, lines_read);
                    }
                }
            }
            "hostname" if !in_match_block => {
                if let Some(h) = current.as_mut() {
                    h.hostname = Some(value.to_string());
                }
            }
            "user" if !in_match_block => {
                if let Some(h) = current.as_mut() {
                    h.user = Some(value.to_string());
                }
            }
            "port" if !in_match_block => {
                if let Some(h) = current.as_mut() {
                    h.port = value.parse().ok();
                }
            }
            "proxycommand" | "proxyjump" | "localcommand" | "permitlocalcommand" => {
                if let Some(h) = current.as_mut() {
                    h.has_proxy = true;
                }
            }
            _ => {}
        }
    }
    if let Some(h) = current.take() {
        out.push(h);
    }
}

fn expand_include(token: &str, base: &Path) -> PathBuf {
    let t = token.trim_matches('"');
    if let Some(rest) = t.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Default user SSH config path.
pub fn user_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ssh/config"))
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    fn parse_text(t: &str) -> Vec<SshHost> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        // Unique per CALL: tests share a process, so process::id() alone
        // means four parallel tests delete each other's file mid-parse.
        let p = std::env::temp_dir().join(format!(
            "gust-cfg-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&p, t).unwrap();
        let h = parse_ssh_config(&p);
        let _ = std::fs::remove_file(&p);
        h
    }

    #[test]
    fn a_gust_path_comment_names_the_starting_folder() {
        let h = parse_text("Host studio\n  HostName studio.example\n  #gust-path /srv/data\n");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].remote_path.as_deref(), Some("/srv/data"));
    }

    #[test]
    fn without_a_gust_path_there_is_no_starting_folder() {
        // Absence must stay absence: inventing "/" or a home path would
        // silently open somebody's entire account.
        let h = parse_text("Host studio\n  HostName studio.example\n");
        assert_eq!(h[0].remote_path, None);
    }

    #[test]
    fn an_ordinary_comment_is_still_ignored() {
        let h = parse_text("Host studio\n  # just a note\n  HostName s.example\n");
        assert_eq!(h[0].remote_path, None);
        assert_eq!(h[0].hostname.as_deref(), Some("s.example"));
    }

    #[test]
    fn a_gust_path_inside_a_match_block_is_not_attributed_to_a_host() {
        let h = parse_text(
            "Host studio\n  HostName s.example\nMatch host other\n  #gust-path /wrong\n",
        );
        assert_eq!(
            h[0].remote_path, None,
            "a Match block leaked a path into an unrelated host"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gust-ssh-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn parses_literal_hosts_only() {
        let d = tmp("literal");
        let c = d.join("config");
        fs::write(
            &c,
            "Host studio\n  HostName 10.0.0.5\n  User alex\n  Port 2222\n\nHost *\n  ServerAliveInterval 60\n",
        )
        .unwrap();
        let hosts = parse_ssh_config(&c);
        assert_eq!(hosts.len(), 1, "wildcard is not a host: {hosts:?}");
        assert_eq!(hosts[0].alias, "studio");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[0].user.as_deref(), Some("alex"));
        assert_eq!(hosts[0].port, Some(2222));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn match_exec_is_never_executed_and_does_not_leak_keys() {
        // If this ran `ssh -G`, the Match exec line would execute a command.
        let d = tmp("matchexec");
        let c = d.join("config");
        let canary = d.join("CANARY-EXECUTED");
        fs::write(
            &c,
            format!(
                "Host safe\n  HostName real.example\n\nMatch exec \"touch {}\"\n  User attacker\n",
                canary.display()
            ),
        )
        .unwrap();
        let hosts = parse_ssh_config(&c);
        assert!(
            !canary.exists(),
            "config parsing must never execute anything"
        );
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "safe");
        // The Match block's User must not be attributed to `safe`.
        assert_ne!(hosts[0].user.as_deref(), Some("attacker"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn include_cycle_terminates() {
        let d = tmp("cycle");
        let a = d.join("a");
        let b = d.join("b");
        fs::write(&a, format!("Include {}\nHost alpha\n", b.display())).unwrap();
        fs::write(&b, format!("Include {}\nHost beta\n", a.display())).unwrap();
        let hosts = parse_ssh_config(&a);
        let names: Vec<_> = hosts.iter().map(|h| h.alias.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn multiple_aliases_on_one_host_line() {
        let d = tmp("multi");
        let c = d.join("config");
        fs::write(&c, "Host web1 web2\n  HostName x\n").unwrap();
        let hosts = parse_ssh_config(&c);
        let names: Vec<_> = hosts.iter().map(|h| h.alias.clone()).collect();
        assert!(names.contains(&"web1".into()));
        assert!(names.contains(&"web2".into()));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn negated_and_wildcard_patterns_are_not_hosts() {
        let d = tmp("neg");
        let c = d.join("config");
        fs::write(&c, "Host !bad *.example.com\n  User x\n").unwrap();
        assert!(parse_ssh_config(&c).is_empty());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn proxy_is_flagged_but_host_still_listed() {
        let d = tmp("proxy");
        let c = d.join("config");
        fs::write(&c, "Host jump\n  ProxyJump bastion\n").unwrap();
        let hosts = parse_ssh_config(&c);
        assert_eq!(hosts.len(), 1);
        assert!(hosts[0].has_proxy);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_config_yields_no_hosts() {
        assert!(parse_ssh_config(Path::new("/gust/nope/ssh_config")).is_empty());
    }

    #[test]
    fn equals_syntax_is_supported() {
        let d = tmp("equals");
        let c = d.join("config");
        fs::write(&c, "Host=eq\nHostName=1.2.3.4\n").unwrap();
        let hosts = parse_ssh_config(&c);
        assert_eq!(hosts[0].alias, "eq");
        assert_eq!(hosts[0].hostname.as_deref(), Some("1.2.3.4"));
        fs::remove_dir_all(&d).ok();
    }
}
