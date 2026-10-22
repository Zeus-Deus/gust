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
    /// Whether this file supplies unambiguous literal endpoint evidence.
    /// This is NOT an effective OpenSSH configuration: callers must also
    /// assess system configuration before grouping hosts by this endpoint.
    /// False never prevents suggesting or connecting to the original alias.
    pub identity_certain: bool,
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
    let mut uncertain_defaults = false;
    parse_into(
        path,
        &mut out,
        &mut seen_files,
        0,
        &mut 0,
        &mut uncertain_defaults,
    );
    let mut aliases = HashSet::new();
    let mut repeated_aliases = HashSet::new();
    for host in &out {
        let alias = host.alias.to_ascii_lowercase();
        if !aliases.insert(alias.clone()) {
            repeated_aliases.insert(alias);
        }
    }
    for host in &mut out {
        if uncertain_defaults || repeated_aliases.contains(&host.alias.to_ascii_lowercase()) {
            host.identity_certain = false;
        }
    }
    out
}

/// Conservatively assess an additional (normally system-wide) SSH config.
/// Callers may use endpoint evidence only when `host.identity_certain` AND
/// this returns true for every additional configuration source in use.
///
/// Missing/unreadable files are unknown, not assumed empty. Includes, Match,
/// endpoint/auth-host overrides and unrecognized options return false without
/// expanding includes or executing commands. This intentionally sacrifices
/// grouping rather than attempting to evaluate OpenSSH precedence.
pub fn system_config_preserves_identity(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    for (i, line) in text.lines().enumerate() {
        if i >= MAX_LINES {
            return false;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = line
            .split(|c: char| c.is_whitespace() || c == '=')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if key != "host" && !identity_neutral_directive(&key) {
            return false;
        }
    }
    true
}

fn parse_into(
    path: &Path,
    out: &mut Vec<SshHost>,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
    lines_read: &mut usize,
    uncertain_defaults: &mut bool,
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
            *uncertain_defaults = true;
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
                    // Mixed/quoted patterns may match other aliases as well;
                    // retain their suggestions but never infer applicability.
                    if value.split_whitespace().count() != 1 || !literal_endpoint(first) {
                        *uncertain_defaults = true;
                    }
                    current = Some(SshHost {
                        alias: (*first).to_string(),
                        hostname: None,
                        user: None,
                        port: None,
                        identity_certain: value.split_whitespace().count() == 1,
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
                            identity_certain: false,
                            has_proxy: false,
                            remote_path: None,
                        });
                    }
                }
            }
            "match" => {
                // Never evaluated. Ends the current Host block. Match can
                // affect any alias, including one already emitted.
                *uncertain_defaults = true;
                in_match_block = true;
                if let Some(h) = current.take() {
                    out.push(h);
                }
            }
            "include" => {
                // Suggestions may still be collected, but this parser does
                // not implement Include's inherited scope/order/glob rules.
                // Missing files, cycles and bounded traversal also fail closed.
                *uncertain_defaults = true;
                let base = canon.parent().unwrap_or(Path::new("/"));
                for token in value.split_whitespace() {
                    let p = expand_include(token, base);
                    // Globs are expanded literally by directory scan, bounded.
                    if token.contains('*') {
                        if let Some(dir) = p.parent() {
                            if let Ok(rd) = std::fs::read_dir(dir) {
                                for e in rd.flatten().take(256) {
                                    parse_into(
                                        &e.path(),
                                        out,
                                        seen,
                                        depth + 1,
                                        lines_read,
                                        uncertain_defaults,
                                    );
                                }
                            }
                        }
                    } else {
                        parse_into(&p, out, seen, depth + 1, lines_read, uncertain_defaults);
                    }
                }
            }
            "hostname" if !in_match_block => {
                if let Some(h) = current.as_mut() {
                    // Suggestions retain their historical last-value parse;
                    // identity evidence cannot pretend this is OpenSSH's first.
                    h.identity_certain &= h.hostname.is_none() && literal_endpoint(value);
                    h.hostname = Some(value.to_string());
                } else {
                    *uncertain_defaults = true;
                }
            }
            "user" if !in_match_block => {
                if let Some(h) = current.as_mut() {
                    h.user = Some(value.to_string());
                }
            }
            "port" if !in_match_block => {
                if let Some(h) = current.as_mut() {
                    h.identity_certain &= h.port.is_none();
                    h.port = value.parse().ok();
                    h.identity_certain &= value.bytes().all(|c| c.is_ascii_digit())
                        && h.port.is_some_and(|port| port != 0);
                } else {
                    *uncertain_defaults = true;
                }
            }
            "proxycommand" | "proxyjump" | "localcommand" | "permitlocalcommand" => {
                if let Some(h) = current.as_mut() {
                    h.has_proxy = true;
                    h.identity_certain = false;
                } else {
                    *uncertain_defaults = true;
                }
            }
            _ if identity_neutral_directive(&key) => {}
            _ => {
                // Unknown directives are not assumed identity-neutral. This
                // includes host-key database overrides and canonicalization.
                if let Some(h) = current.as_mut() {
                    h.identity_certain = false;
                } else {
                    *uncertain_defaults = true;
                }
            }
        }
    }
    if let Some(h) = current.take() {
        out.push(h);
    }
}

fn literal_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b':'))
}

// Only options known not to change endpoint/host-key lookup are ignored for
// certainty. Extending OpenSSH's option set must fail closed here.
fn identity_neutral_directive(key: &str) -> bool {
    matches!(
        key,
        "user"
            | "identityfile"
            | "identitiesonly"
            | "identityagent"
            | "forwardagent"
            | "forwardx11"
            | "serveraliveinterval"
            | "serveralivecountmax"
            | "tcpkeepalive"
            | "connecttimeout"
            | "connectionattempts"
            | "compression"
            | "loglevel"
            | "batchmode"
    )
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
    fn direct_literal_endpoint_is_certain_in_the_parsed_user_config() {
        let h = parse_text("Host studio\n  HostName 192.0.2.10\n  Port 2222\n");
        assert!(h[0].identity_certain);
        assert_eq!(h[0].hostname.as_deref(), Some("192.0.2.10"));
        assert_eq!(h[0].port, Some(2222));
    }

    #[test]
    fn inherited_endpoint_rules_make_identity_uncertain_without_hiding_aliases() {
        for directive in [
            "ProxyJump bastion",
            "ProxyCommand nc %h %p",
            "HostName other.example",
            "Port 2200",
        ] {
            for (before, after) in [(true, false), (false, true)] {
                let defaults = format!("Host *\n  {directive}\n");
                let direct =
                    "Host studio\n  HostName 192.0.2.10\n  Port 2222\n  #gust-path /srv/data\n";
                let text = format!(
                    "{}{}{}",
                    if before { &defaults } else { "" },
                    direct,
                    if after { &defaults } else { "" }
                );
                let h = parse_text(&text);
                assert_eq!(h.len(), 1);
                assert_eq!(h[0].alias, "studio");
                assert_eq!(h[0].hostname.as_deref(), Some("192.0.2.10"));
                assert_eq!(h[0].port, Some(2222));
                assert_eq!(h[0].remote_path.as_deref(), Some("/srv/data"));
                assert!(!h[0].identity_certain, "inherited {directive}: {text}");
            }
        }
    }

    #[test]
    fn identity_overrides_are_not_evidence_for_an_ordinary_endpoint() {
        for directive in [
            "ProxyJump bastion",
            "ProxyCommand nc %h %p",
            "LocalCommand true",
            "PermitLocalCommand yes",
            "CanonicalizeHostname yes",
            "CanonicalDomains example.com",
            "HostKeyAlias other-host",
            "UserKnownHostsFile ~/.ssh/other_known_hosts",
            "GlobalKnownHostsFile /etc/ssh/other_known_hosts",
            "RevokedHostKeys /some/revoked",
            "KnownHostsCommand /some/command",
            "SomeFutureRoutingOption yes",
        ] {
            let text = format!("Host studio\n  HostName 192.0.2.10\n  {directive}\nHost plain\n  HostName 192.0.2.20\n");
            let h = parse_text(&text);
            assert_eq!(h.len(), 2);
            assert!(!h[0].identity_certain, "{directive} was trusted");
            assert!(
                h[1].identity_certain,
                "an unrelated simple block is still reliable"
            );
            assert_eq!(h[0].hostname.as_deref(), Some("192.0.2.10"));
        }
    }

    #[test]
    fn overlapping_rules_cannot_claim_last_value_as_effective_identity() {
        for text in [
            "Host studio\n HostName first.example\n HostName last.example\n",
            "Host studio\n HostName same.example\n Port 2200\n Port 2222\n",
            "Host studio\n HostName first.example\nHost studio\n HostName last.example\n",
            "Host STUDIO\n HostName first.example\nHost studio\n HostName last.example\n",
            "Host studio other\n HostName same.example\n",
            "Host studio !studio\n HostName same.example\n",
            "Host *.example studio\n HostName same.example\n",
        ] {
            let h = parse_text(text);
            assert!(!h.is_empty());
            assert!(
                h.iter().all(|h| !h.identity_certain),
                "ambiguous literal parse: {text}"
            );
        }
        // Preserve suggestion values, not silently change how a mount opens.
        let h = parse_text("Host studio\n HostName first.example\n HostName last.example\n");
        assert_eq!(h[0].hostname.as_deref(), Some("last.example"));
    }

    #[test]
    fn mixed_host_patterns_cannot_supply_defaults_to_other_certain_aliases() {
        for patterns in ["studio *", "studio !skip", "\"studio\"", "[studio]"] {
            let text = format!(
                "Host {patterns}\n HostName first.example\nHost plain\n HostName last.example\n"
            );
            let h = parse_text(&text);
            assert!(h.iter().any(|h| h.alias == "plain"));
            assert!(
                h.iter().all(|h| !h.identity_certain),
                "unparsed Host pattern {patterns}"
            );
        }
    }

    #[test]
    fn nonliteral_endpoint_values_are_uncertain_but_not_rewritten() {
        for directive in [
            "HostName %h.example",
            "HostName ${DESTINATION}",
            "HostName \"192.0.2.10\"",
            "HostName 192.0.2.10 # note",
            "HostName",
            "HostName *",
            "HostName user@host",
            "Port service",
            "Port 0",
            "Port 65536",
            "Port 22 # note",
            "Port",
        ] {
            let h = parse_text(&format!("Host studio\n {directive}\n"));
            assert_eq!(h[0].alias, "studio");
            assert!(
                !h[0].identity_certain,
                "not a resolved literal: {directive}"
            );
        }
        for text in [
            "Host studio\n HostName example.test\n",
            "Host studio\n HostName 2001:db8::1\n Port 22\n",
            "Host studio\n HostName 192.0.2.1\n User alex\n IdentityFile ~/.ssh/id_fixture\n",
            "Host studio\n HostName 192.0.2.1\nHost *\n ServerAliveInterval 60\n",
        ] {
            assert!(
                parse_text(text)[0].identity_certain,
                "simple preserved case: {text}"
            );
        }
    }

    #[test]
    fn reaching_the_line_budget_cannot_certify_an_unseen_tail() {
        let text = format!(
            "Host studio\n HostName 192.0.2.10\n{}Host *\n HostName other.example\n",
            "# padding\n".repeat(MAX_LINES)
        );
        let h = parse_text(&text);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].alias, "studio");
        assert!(
            !h[0].identity_certain,
            "unseen defaults could replace the endpoint"
        );
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
        assert!(!hosts[0].identity_certain, "Match routing is not evaluated");
        // The Match block's User must not be attributed to `safe`.
        assert_ne!(hosts[0].user.as_deref(), Some("attacker"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn include_context_is_uncertain_even_when_suggestions_can_be_read() {
        let d = tmp("include-certainty");
        let c = d.join("config");
        for include in ["missing", "missing/*.conf", "included"] {
            fs::write(d.join("included"), "Host included\n  HostName 192.0.2.30\n").unwrap();
            fs::write(
                &c,
                format!("Include {include}\nHost studio\n  HostName 192.0.2.10\n"),
            )
            .unwrap();
            let hosts = parse_ssh_config(&c);
            assert!(hosts.iter().any(|h| h.alias == "studio"));
            assert!(
                hosts.iter().all(|h| !h.identity_certain),
                "Include {include} cannot certify endpoints: {hosts:?}"
            );
            if include == "included" {
                assert!(hosts.iter().any(|h| h.alias == "included"));
            }
        }
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn system_config_assessment_requires_readable_identity_neutral_contents() {
        let d = tmp("system-certainty");
        let c = d.join("system-config");
        assert!(
            !system_config_preserves_identity(&c),
            "missing is unknown, not a default"
        );
        for text in [
            "",
            "# fixture only\nHost *\n ServerAliveInterval 30\n User alex\n",
        ] {
            fs::write(&c, text).unwrap();
            assert!(
                system_config_preserves_identity(&c),
                "neutral fixture: {text}"
            );
        }
        for directive in [
            "HostName replacement.example",
            "Port 2200",
            "ProxyJump bastion",
            "ProxyCommand /some/command",
            "CanonicalizeHostname yes",
            "HostKeyAlias another",
            "UserKnownHostsFile /some/keys",
            "GlobalKnownHostsFile /some/keys",
            "RevokedHostKeys /some/revoked",
            "KnownHostsCommand /some/command",
            "Match exec /some/command",
            "Include missing",
            "Include missing/*.conf",
            "SomeFutureIdentityOption yes",
        ] {
            fs::write(&c, format!("Host *\n {directive}\n")).unwrap();
            assert!(
                !system_config_preserves_identity(&c),
                "system {directive} is not a known default"
            );
        }
        fs::write(&c, [0xff]).unwrap();
        assert!(!system_config_preserves_identity(&c));
        fs::write(&c, "# many lines\n".repeat(MAX_LINES + 1)).unwrap();
        assert!(
            !system_config_preserves_identity(&c),
            "bounded scan cannot certify the unseen tail"
        );
        fs::remove_dir_all(d).unwrap();
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
