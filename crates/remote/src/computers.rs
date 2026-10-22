//! Local-only computer grouping; display labels never establish identity.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteKind {
    Ssh,
    #[serde(rename = "tailnet")]
    Tailscale,
}

/// Exact existing-action descriptor; grouping never rewrites or deduplicates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerRoute {
    pub kind: RouteKind,
    pub name: String,
    pub target: String,
    pub detail: String,
    pub user: Option<String>,
    pub path: Option<String>,
    pub port: Option<u16>,
    /// Remote starting folder; `path` is the local mount used by Open.
    pub remote_path: Option<String>,
    pub connected: bool,
    pub lost: bool,
    pub online: bool,
    pub available: bool,
    /// Recovery hint only: never permission to disable a configured SSH action.
    #[serde(default)]
    pub requires_tailnet: bool,
    /// Canonical endpoint equivalences, not display names or SSH aliases.
    #[serde(default)]
    pub endpoint_ids: Vec<String>,
}

/// An explicitly known literal SSH endpoint, not an alias or display label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEndpoint {
    pub host: String,
    pub port: u16,
}

/// Pure recovery metadata. `online = true` includes unknown reachability; it is
/// not a probe or a connection guarantee. CGNAT/name hints are conservative and
/// MUST NOT block an explicit SSH action. Proxy-dependent routes remain unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePolicy {
    pub requires_tailnet: bool,
    pub online: bool,
    pub endpoint_ids: Vec<String>,
}

/// Evaluate explicit endpoints against local daemon metadata, without I/O.
/// Only certain, non-proxy endpoints supply identity. A short peer name can
/// hint at dependency/liveness but never expands into the peer's endpoint IDs.
/// IDs use `[normalized-host]:port` for both IP families and DNS; peer metadata
/// expands exact DNS/IP matches at the input port, never across SSH ports.
pub fn route_policy(
    endpoints: &[HostEndpoint],
    peers: &[crate::tailscale::TailnetPeer],
    tailnet_available: bool,
    identity_certain: bool,
    has_proxy: bool,
) -> RoutePolicy {
    let mut policy = RoutePolicy {
        requires_tailnet: false,
        online: true,
        endpoint_ids: Vec::new(),
    };
    if has_proxy || endpoints.is_empty() {
        return policy;
    }
    let mut all_tailnet = true;
    let mut all_offline = true;
    for endpoint in endpoints {
        let Some((host, port)) = endpoint_identity(endpoint) else {
            all_tailnet = false;
            all_offline = false;
            continue;
        };
        if identity_certain {
            policy.endpoint_ids.push(format!("[{host}]:{port}"));
        }
        let matching: Vec<_> = peers
            .iter()
            .filter_map(|peer| {
                let exact = peer
                    .ips
                    .iter()
                    .chain(peer.dns_name.iter())
                    .chain(std::iter::once(&peer.target))
                    .filter_map(|value| {
                        endpoint_identity(&HostEndpoint {
                            host: value.clone(),
                            port,
                        })
                    })
                    .any(|(value, _)| {
                        (value.contains('.') || value.contains(':')) && value == host
                    });
                let short = !host.contains('.')
                    && !host.contains(':')
                    && peer.name.eq_ignore_ascii_case(&host);
                (exact || short).then_some((peer, exact))
            })
            .collect();
        if matching.is_empty() {
            let tailnet_hint = match host.parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(ip)) => u32::from(ip) & 0xffc0_0000 == 0x6440_0000,
                Ok(std::net::IpAddr::V6(ip)) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
                Err(_) => host.ends_with(".ts.net"),
            };
            all_tailnet &= tailnet_hint;
            all_offline = false;
        }
        for (peer, exact) in matching {
            all_offline &= !peer.online;
            if identity_certain && exact {
                policy.endpoint_ids.extend(
                    peer.ips
                        .iter()
                        .chain(peer.dns_name.iter())
                        .chain(std::iter::once(&peer.target))
                        .filter_map(|value| {
                            endpoint_identity(&HostEndpoint {
                                host: value.clone(),
                                port,
                            })
                        })
                        .filter(|(value, _)| value.contains('.') || value.contains(':'))
                        .map(|(value, port)| format!("[{value}]:{port}")),
                );
            }
        }
    }
    policy.requires_tailnet = all_tailnet;
    policy.online = !(all_offline || (all_tailnet && !tailnet_available));
    policy.endpoint_ids.sort();
    policy.endpoint_ids.dedup();
    policy
}

/// Previously trusted host-key evidence only. Never populate this from a scan.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrustedHostKey {
    pub key_type: String,
    pub key: String,
}

/// Evidence is supplied separately so unresolved aliases cannot become identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifiedRoute {
    pub route: ComputerRoute,
    pub endpoints: Vec<HostEndpoint>,
    pub trusted_host_keys: Vec<TrustedHostKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computer {
    pub name: String,
    pub routes: Vec<ComputerRoute>,
}

/// Pure presentation grouping. Does not authorize connecting, fallback, or replay.
pub fn group_computers(routes: Vec<IdentifiedRoute>) -> Vec<Computer> {
    let mut parents: Vec<_> = (0..routes.len()).collect();
    let mut endpoints = std::collections::HashMap::new();
    let mut keys = std::collections::HashMap::new();
    for (index, input) in routes.iter().enumerate() {
        for endpoint in &input.endpoints {
            if let Some(identity) = endpoint_identity(endpoint) {
                if let Some(previous) = endpoints.insert(identity, index) {
                    let a = root(&mut parents, previous);
                    let b = root(&mut parents, index);
                    parents[a.max(b)] = a.min(b);
                }
            }
        }
        for key in input.trusted_host_keys.iter().filter(|key| valid_key(key)) {
            if let Some(previous) = keys.insert(key.clone(), index) {
                let a = root(&mut parents, previous);
                let b = root(&mut parents, index);
                parents[a.max(b)] = a.min(b);
            }
        }
    }
    let mut positions = std::collections::HashMap::new();
    let mut groups: Vec<Computer> = Vec::new();
    for (index, input) in routes.into_iter().enumerate() {
        let owner = root(&mut parents, index);
        let position = *positions.entry(owner).or_insert_with(|| {
            groups.push(Computer {
                name: input.route.name.clone(),
                routes: Vec::new(),
            });
            groups.len() - 1
        });
        groups[position].routes.push(input.route);
    }
    groups
}

/// Read previously trusted keys from one explicit local known_hosts file.
///
/// This runs only `ssh-keygen -F`, never SSH/config evaluation, DNS, a scan,
/// private-key parsing, or a trust write. Run on a worker, not the UI thread.
/// Missing files/matches yield no evidence; invalid inputs and failed/bounded
/// lookups return an error. The timeout is capped at five seconds and stdout
/// at one MiB. Certificate-authority/certificate/revoked keys are not identity.
pub fn trusted_host_keys(
    known_hosts: &std::path::Path,
    endpoint: &HostEndpoint,
    timeout: std::time::Duration,
) -> std::io::Result<Vec<TrustedHostKey>> {
    use std::io::{Error, ErrorKind};
    endpoint_identity(endpoint)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Invalid SSH host endpoint"))?;
    // Comparison may canonicalize an IP, but a hashed known_hosts record is
    // indexed by the exact spelling SSH was given. Validate, then preserve it.
    let host = &endpoint.host;
    let port = endpoint.port;
    let start = std::time::Instant::now();
    let Some(identity) = known_hosts_identity(known_hosts)? else {
        return Ok(Vec::new());
    };
    let text = read_known_hosts(known_hosts, &identity)?;
    let revoked = revoked_keys(&text);
    let query = if port == 22 {
        host.clone()
    } else {
        format!("[{host}]:{port}")
    };
    let mut command = std::process::Command::new("ssh-keygen");
    command.arg("-F").arg(query).arg("-f").arg(known_hosts);
    let output = lookup_output(
        &mut command,
        timeout
            .min(std::time::Duration::from_secs(5))
            .saturating_sub(start.elapsed()),
    )?;
    if known_hosts_identity(known_hosts)? != Some(identity) {
        return Err(Error::other("known_hosts changed during lookup"));
    }
    let text = std::str::from_utf8(&output)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "Invalid known_hosts output"))?;
    Ok(parse_trusted_keys(text)
        .into_iter()
        .filter(|key| !revoked.contains(key))
        .collect())
}

/// Process-retained, bounded known_hosts evidence. Keep one instance on the
/// polling worker (or behind its Mutex); this type performs local I/O only.
/// Changing file identity or any stat/read error invalidates old evidence.
#[derive(Default)]
pub struct TrustedKeyCache {
    file: Option<(std::path::PathBuf, KnownHostsIdentity)>,
    entries: std::collections::HashMap<(String, u16), Vec<TrustedHostKey>>,
    order: std::collections::VecDeque<(String, u16)>,
    bytes: usize,
}

impl TrustedKeyCache {
    /// One result per endpoint, in input order, including duplicates. Batch ALL
    /// routes before mount probes: only this call owns the lookup budget. Fresh
    /// work shares min(budget, 750ms), each subprocess at most 200ms. Cached keys
    /// from this same file are returned even after the budget expires. Endpoint
    /// spelling is preserved because hashed known_hosts indexes the raw input.
    pub fn lookup_batch(
        &mut self,
        known_hosts: &std::path::Path,
        endpoints: &[HostEndpoint],
        budget: std::time::Duration,
    ) -> Vec<std::io::Result<Vec<TrustedHostKey>>> {
        self.lookup_batch_with(known_hosts, endpoints, budget, trusted_host_keys)
    }

    fn clear(&mut self) {
        self.file = None;
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    fn lookup_batch_with(
        &mut self,
        known_hosts: &std::path::Path,
        endpoints: &[HostEndpoint],
        budget: std::time::Duration,
        mut lookup: impl FnMut(
            &std::path::Path,
            &HostEndpoint,
            std::time::Duration,
        ) -> std::io::Result<Vec<TrustedHostKey>>,
    ) -> Vec<std::io::Result<Vec<TrustedHostKey>>> {
        use std::io::{Error, ErrorKind};
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let budget = budget.min(Duration::from_millis(750));
        let identity = match known_hosts_identity(known_hosts) {
            Ok(identity) => identity,
            Err(error) => {
                self.clear();
                return endpoints
                    .iter()
                    .map(|_| {
                        Err(Error::new(
                            error.kind(),
                            "Cannot stat bounded known_hosts file",
                        ))
                    })
                    .collect();
            }
        };
        let file = identity.map(|identity| (known_hosts.to_owned(), identity));
        if self.file != file {
            self.clear();
            self.file = file.clone();
        }
        let mut results = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            if endpoint_identity(endpoint).is_none() {
                results.push(Err(Error::new(
                    ErrorKind::InvalidInput,
                    "Invalid SSH host endpoint",
                )));
                continue;
            }
            let key = (endpoint.host.clone(), endpoint.port);
            if let Some(keys) = self.entries.get(&key) {
                results.push(Ok(keys.clone()));
                continue;
            }
            if file.is_none() {
                results.push(Ok(Vec::new()));
                continue;
            }
            let remaining = budget.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                results.push(Err(Error::new(
                    ErrorKind::TimedOut,
                    "SSH host-key lookup budget exhausted",
                )));
                continue;
            }
            let result = lookup(
                known_hosts,
                endpoint,
                remaining.min(Duration::from_millis(200)),
            );
            if let Ok(keys) = &result {
                let bytes = key.0.len()
                    + keys
                        .iter()
                        .map(|key| key.key_type.len() + key.key.len())
                        .sum::<usize>();
                const MAX_BYTES: usize = 8 * 1024 * 1024;
                if bytes <= MAX_BYTES {
                    while self.entries.len() >= 1024 || self.bytes + bytes > MAX_BYTES {
                        let Some(old) = self.order.pop_front() else {
                            break;
                        };
                        if let Some(keys) = self.entries.remove(&old) {
                            self.bytes -= old.0.len()
                                + keys
                                    .iter()
                                    .map(|key| key.key_type.len() + key.key.len())
                                    .sum::<usize>();
                        }
                    }
                    self.bytes += bytes;
                    self.order.push_back(key.clone());
                    self.entries.insert(key, keys.clone());
                }
            }
            results.push(result);
        }
        // No old or partially refreshed evidence escapes a racing replacement,
        // removal, revocation append, chmod, unreadable file, or truncation.
        match known_hosts_identity(known_hosts) {
            Ok(after) if after.as_ref() == file.as_ref().map(|(_, identity)| identity) => results,
            _ => {
                self.clear();
                endpoints
                    .iter()
                    .map(|_| Err(Error::other("known_hosts changed during batch")))
                    .collect()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownHostsIdentity {
    dev: u64,
    ino: u64,
    mtime: i64,
    mtime_ns: i64,
    size: u64,
    // Also invalidate on chmod or an edit that restored mtime.
    ctime: i64,
    ctime_ns: i64,
}

impl KnownHostsIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Not a bounded known_hosts file",
            ));
        }
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            mtime: metadata.mtime(),
            mtime_ns: metadata.mtime_nsec(),
            size: metadata.len(),
            ctime: metadata.ctime(),
            ctime_ns: metadata.ctime_nsec(),
        })
    }
}

fn known_hosts_identity(path: &std::path::Path) -> std::io::Result<Option<KnownHostsIdentity>> {
    match std::fs::metadata(path) {
        Ok(metadata) => KnownHostsIdentity::from_metadata(&metadata).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_known_hosts(
    path: &std::path::Path,
    identity: &KnownHostsIdentity,
) -> std::io::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    if KnownHostsIdentity::from_metadata(&file.metadata()?)? != *identity {
        return Err(std::io::Error::other("known_hosts changed during lookup"));
    }
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(std::io::Error::other("known_hosts exceeded limit"));
    }
    String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid known_hosts file")
    })
}

fn revoked_keys(text: &str) -> std::collections::HashSet<TrustedHostKey> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()? != "@revoked" {
                return None;
            }
            fields.next()?;
            let key = TrustedHostKey {
                key_type: fields.next()?.into(),
                key: fields.next()?.into(),
            };
            valid_key(&key).then_some(key)
        })
        .collect()
}

fn lookup_output(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let timeout = timeout.min(Duration::from_secs(5));
    if timeout.is_zero() {
        return Err(Error::new(
            ErrorKind::TimedOut,
            "SSH host-key lookup timed out",
        ));
    }
    // A nonblocking local socket avoids a pipe-reader thread whose join could
    // hang forever if an unexpected descendant retains the output descriptor.
    let (mut reader, writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::null());
    let start = Instant::now();
    let mut child = command.spawn()?;
    // Do not retain Command's extra stdout descriptor in the parent.
    command.stdout(Stdio::null());
    let result = (|| {
        let mut output = Vec::new();
        let mut status = None;
        let mut eof = false;
        loop {
            if start.elapsed() >= timeout {
                return Err(Error::new(
                    ErrorKind::TimedOut,
                    "SSH host-key lookup timed out",
                ));
            }
            let mut buffer = [0; 8192];
            match reader.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(count) => {
                    if output.len() + count > 1024 * 1024 {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            "SSH host-key output exceeded limit",
                        ));
                    }
                    output.extend_from_slice(&buffer[..count]);
                    // Drain promptly but recheck the deadline on every read.
                    continue;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if let Some(status) = status.filter(|_| eof) {
                return if status.success() || (status.code() == Some(1) && output.is_empty()) {
                    Ok(output)
                } else {
                    Err(Error::other("SSH host-key lookup failed"))
                };
            }
            std::thread::sleep(
                Duration::from_millis(5).min(timeout.saturating_sub(start.elapsed())),
            );
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        gust_core::stall::reap_in_background(child);
    }
    result
}

fn parse_trusted_keys(text: &str) -> Vec<TrustedHostKey> {
    let mut keys = Vec::new();
    let mut revoked = std::collections::HashSet::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(first) = fields.next().filter(|field| !field.starts_with('#')) else {
            continue;
        };
        let marker = first.starts_with('@').then_some(first);
        let hosts = if marker.is_some() {
            let Some(hosts) = fields.next() else { continue };
            hosts
        } else {
            first
        };
        let (Some(key_type), Some(key)) = (fields.next(), fields.next()) else {
            continue;
        };
        let key = TrustedHostKey {
            key_type: key_type.into(),
            key: key.into(),
        };
        if !valid_key(&key) {
            continue;
        }
        match marker {
            Some("@revoked") => {
                revoked.insert(key);
            }
            None if !hosts.contains(['*', '?', '!']) && !keys.contains(&key) => {
                keys.push(key);
            }
            _ => {}
        }
    }
    keys.retain(|key| !revoked.contains(key));
    keys
}

fn valid_key(key: &TrustedHostKey) -> bool {
    matches!(
        key.key_type.as_str(),
        "ssh-ed25519"
            | "ssh-rsa"
            | "ecdsa-sha2-nistp256"
            | "ecdsa-sha2-nistp384"
            | "ecdsa-sha2-nistp521"
            | "sk-ssh-ed25519@openssh.com"
            | "sk-ecdsa-sha2-nistp256@openssh.com"
    ) && key.key.len() >= 16
        && key.key.len() <= 32 * 1024
        && key.key.len().is_multiple_of(4)
        && key.key.len() - key.key.trim_end_matches('=').len() <= 2
        && key
            .key
            .trim_end_matches('=')
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn endpoint_identity(endpoint: &HostEndpoint) -> Option<(String, u16)> {
    let host = &endpoint.host;
    if endpoint.port == 0 || host.is_empty() || host.len() > 253 {
        return None;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Some((ip.to_string(), endpoint.port));
    }
    // One terminal DNS dot and case are spelling, not a different route.
    let labels = host.strip_suffix('.').unwrap_or(host);
    if !labels.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            && label
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    }) {
        return None;
    }
    Some((labels.to_ascii_lowercase(), endpoint.port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str, host: &str) -> IdentifiedRoute {
        IdentifiedRoute {
            route: ComputerRoute {
                kind: RouteKind::Ssh,
                name: name.into(),
                target: name.into(),
                detail: host.into(),
                user: None,
                path: None,
                port: None,
                remote_path: None,
                connected: false,
                lost: false,
                online: true,
                available: true,
                requires_tailnet: false,
                endpoint_ids: Vec::new(),
            },
            endpoints: vec![HostEndpoint {
                host: host.into(),
                port: 22,
            }],
            trusted_host_keys: Vec::new(),
        }
    }

    fn peer(online: bool) -> crate::tailscale::TailnetPeer {
        crate::tailscale::TailnetPeer {
            name: "device".into(),
            target: "device.tail.ts.net".into(),
            ips: vec!["100.64.0.1".into(), "fd7a:115c:a1e0::1".into()],
            dns_name: Some("device.tail.ts.net".into()),
            os: "linux".into(),
            online,
        }
    }

    #[test]
    fn policy_normalizes_peer_endpoints_but_short_names_only_hint_dependency() {
        let peers = [peer(false)];
        let full = route_policy(
            &[HostEndpoint {
                host: "DEVICE.tail.ts.net.".into(),
                port: 2222,
            }],
            &peers,
            true,
            true,
            false,
        );
        assert!(full.requires_tailnet);
        assert!(!full.online);
        assert!(full.endpoint_ids.contains(&"[100.64.0.1]:2222".into()));
        let short = route_policy(
            &[HostEndpoint {
                host: "device".into(),
                port: 22,
            }],
            &peers,
            true,
            false,
            false,
        );
        assert!(short.requires_tailnet);
        assert!(!short.online);
        assert!(short.endpoint_ids.is_empty());
    }

    #[test]
    fn uncached_tailnet_prefixes_are_recovery_hints_with_correct_boundaries() {
        for host in [
            "100.64.0.0",
            "100.127.255.255",
            "fd7a:115c:a1e0::1",
            "FD7A:115C:A1E0:ffff::1",
            "DEVICE.TAIL.TS.NET.",
        ] {
            let policy = route_policy(
                &[HostEndpoint {
                    host: host.into(),
                    port: 22,
                }],
                &[],
                false,
                true,
                false,
            );
            assert!(policy.requires_tailnet, "{host}");
            assert!(!policy.online, "{host}");
        }
        for host in [
            "100.63.255.255",
            "100.128.0.0",
            "fd7a:115c:a1df::1",
            "fd7a:115c:a1e1::1",
            "ts.net",
            "fake-ts.net",
            "host.ts.net.example",
        ] {
            let policy = route_policy(
                &[HostEndpoint {
                    host: host.into(),
                    port: 22,
                }],
                &[],
                false,
                true,
                false,
            );
            assert!(!policy.requires_tailnet, "{host}");
            assert!(policy.online, "{host}");
        }
    }

    #[test]
    fn policy_proxy_uncertainty_never_claims_local_vpn_dependency() {
        let endpoints = [HostEndpoint {
            host: "device.tail.ts.net".into(),
            port: 22,
        }];
        let policy = route_policy(&endpoints, &[peer(false)], false, true, true);
        assert!(!policy.requires_tailnet);
        assert!(policy.online);
        assert!(policy.endpoint_ids.is_empty());
        let uncertain = route_policy(&endpoints, &[peer(true)], true, false, false);
        assert!(uncertain.requires_tailnet);
        assert!(uncertain.online);
        assert!(uncertain.endpoint_ids.is_empty());
    }

    #[test]
    fn policy_endpoint_ids_exclude_equivalent_recovery_routes_without_changing_ports() {
        let policies: Vec<_> = [
            "2001:db8::1",
            "2001:0db8:0:0:0:0:0:1",
            "DEVICE.Example.",
            "device.example",
        ]
        .into_iter()
        .map(|host| {
            route_policy(
                &[HostEndpoint {
                    host: host.into(),
                    port: 22,
                }],
                &[],
                true,
                true,
                false,
            )
        })
        .collect();
        assert_eq!(policies[0].endpoint_ids, policies[1].endpoint_ids);
        assert_eq!(policies[2].endpoint_ids, policies[3].endpoint_ids);
        let other_port = route_policy(
            &[HostEndpoint {
                host: "device.example".into(),
                port: 2222,
            }],
            &[],
            true,
            true,
            false,
        );
        assert_ne!(policies[2].endpoint_ids, other_port.endpoint_ids);
        let a = route_policy(
            &[HostEndpoint {
                host: "100.64.0.1".into(),
                port: 22,
            }],
            &[],
            true,
            true,
            false,
        );
        let b = route_policy(
            &[HostEndpoint {
                host: "different.tail.ts.net".into(),
                port: 22,
            }],
            &[],
            true,
            true,
            false,
        );
        assert!(a.requires_tailnet && b.requires_tailnet);
        assert!(!a.endpoint_ids.iter().any(|id| b.endpoint_ids.contains(id)));
    }

    #[test]
    fn policy_direct_unknown_routes_remain_online_and_bad_endpoints_never_supply_ids() {
        let endpoints = [HostEndpoint {
            host: "192.0.2.1".into(),
            port: 22,
        }];
        let policy = route_policy(&endpoints, &[peer(false)], false, true, false);
        assert!(!policy.requires_tailnet);
        assert!(policy.online);
        for host in ["", "-oProxyCommand=x", "a..b"] {
            let policy = route_policy(
                &[HostEndpoint {
                    host: host.into(),
                    port: 22,
                }],
                &[],
                false,
                true,
                false,
            );
            assert!(!policy.requires_tailnet);
            assert!(policy.online);
            assert!(policy.endpoint_ids.is_empty());
        }
        let empty = route_policy(&[], &[], false, true, false);
        assert!(!empty.requires_tailnet && empty.online && empty.endpoint_ids.is_empty());
        let mut mixed = endpoints.to_vec();
        mixed.push(HostEndpoint {
            host: "100.64.0.1".into(),
            port: 22,
        });
        assert!(!route_policy(&mixed, &[peer(false)], false, true, false).requires_tailnet);
    }

    #[test]
    fn absolute_dns_endpoint_spelling_has_one_identity() {
        assert_eq!(
            endpoint_identity(&HostEndpoint {
                host: "Device.Example.".into(),
                port: 22
            }),
            endpoint_identity(&HostEndpoint {
                host: "device.example".into(),
                port: 22
            }),
        );
    }

    #[test]
    fn exact_endpoints_group_without_losing_aliases_accounts_paths_or_duplicates() {
        let first = route("studio", "192.0.2.1");
        let mut second = route("studio-vpn", "192.0.2.1");
        second.route.kind = RouteKind::Tailscale;
        second.route.user = Some("other".into());
        second.route.path = Some("/different".into());
        second.route.remote_path = Some("/srv/photos".into());
        second.route.available = false;
        let expected = vec![
            first.route.clone(),
            second.route.clone(),
            first.route.clone(),
        ];
        let groups = group_computers(vec![first.clone(), second, first]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].routes, expected);
    }

    #[test]
    fn same_label_or_target_without_endpoint_evidence_never_groups() {
        let a = route("studio", "192.0.2.1");
        let b = route("studio", "192.0.2.2");
        assert_eq!(group_computers(vec![a.clone(), b]).len(), 2);
        let mut unresolved = a.clone();
        unresolved.endpoints.clear();
        assert_eq!(
            group_computers(vec![unresolved.clone(), unresolved]).len(),
            2
        );
    }

    #[test]
    fn ports_and_invalid_endpoints_cannot_create_identity_edges() {
        let a = route("studio", "192.0.2.1");
        let mut b = a.clone();
        b.endpoints[0].port = 2222;
        assert_eq!(group_computers(vec![a, b]).len(), 2);
        for bad in [
            "-oProxyCommand=x",
            "example;touch CANARY",
            "user@host",
            "host/path",
            "a b",
            "a\nb",
            "",
            "::invalid",
            "a..b",
        ] {
            let a = route("same", bad);
            assert_eq!(group_computers(vec![a.clone(), a]).len(), 2, "{bad:?}");
        }
        let mut zero = route("zero", "192.0.2.1");
        zero.endpoints[0].port = 0;
        assert_eq!(group_computers(vec![zero.clone(), zero]).len(), 2);
    }

    fn key(kind: &str, value: &str) -> TrustedHostKey {
        TrustedHostKey {
            key_type: kind.into(),
            key: value.into(),
        }
    }

    #[test]
    fn shared_trusted_key_groups_different_endpoints() {
        let mut a = route("studio", "192.0.2.1");
        let mut b = route("vpn", "100.64.0.1");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        a.trusted_host_keys.push(shared.clone());
        b.trusted_host_keys.push(shared);
        assert_eq!(group_computers(vec![a, b]).len(), 1);
    }

    #[test]
    fn distinct_keys_and_algorithm_mismatches_do_not_group() {
        let mut a = route("studio", "192.0.2.1");
        let mut b = route("studio", "192.0.2.2");
        a.trusted_host_keys.push(key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ));
        b.trusted_host_keys.push(key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        ));
        assert_eq!(group_computers(vec![a.clone(), b.clone()]).len(), 2);
        b.trusted_host_keys[0] = a.trusted_host_keys[0].clone();
        b.trusted_host_keys[0].key_type = "ssh-rsa".into();
        assert_eq!(group_computers(vec![a, b]).len(), 2);
    }

    #[test]
    fn malformed_or_certificate_key_evidence_does_not_group() {
        for bad in [
            key("ssh-ed25519", ""),
            key("ssh-ed25519", "invalid;command"),
            key("@cert-authority", "AAAA"),
            key(
                "ssh-ed25519-cert-v01@openssh.com",
                "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ),
        ] {
            let mut a = route("studio", "192.0.2.1");
            let mut b = route("vpn", "192.0.2.2");
            a.trusted_host_keys.push(bad.clone());
            b.trusted_host_keys.push(bad);
            assert_eq!(group_computers(vec![a, b]).len(), 2);
        }
    }

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "gust-computers-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cached_keys_survive_zero_lookup_budget_but_not_a_file_change() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        std::fs::write(
            &path,
            format!("example.test {} {}\n", shared.key_type, shared.key),
        )
        .unwrap();
        let endpoints = [HostEndpoint {
            host: "example.test".into(),
            port: 22,
        }];
        let mut cache = TrustedKeyCache::default();
        assert_eq!(
            cache
                .lookup_batch(&path, &endpoints, Duration::from_millis(750))
                .remove(0)
                .unwrap(),
            vec![shared.clone()]
        );
        assert_eq!(
            cache
                .lookup_batch(&path, &endpoints, Duration::ZERO)
                .remove(0)
                .unwrap(),
            vec![shared.clone()]
        );
        std::fs::write(&path, "# pin removed\n").unwrap();
        assert!(cache
            .lookup_batch(&path, &endpoints, Duration::ZERO)
            .remove(0)
            .is_err());
        assert!(cache
            .lookup_batch(&path, &endpoints, Duration::from_millis(750))
            .remove(0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cache_exhausted_batch_still_returns_later_cached_keys() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        std::fs::write(&path, "# fixture\n").unwrap();
        let cached = HostEndpoint {
            host: "cached.test".into(),
            port: 22,
        };
        let keys = vec![key("ssh-ed25519", "AAAAAAAAAAAAAAAA")];
        let mut cache = TrustedKeyCache::default();
        assert_eq!(
            cache
                .lookup_batch_with(
                    &path,
                    std::slice::from_ref(&cached),
                    Duration::from_millis(750),
                    |_, _, _| Ok(keys.clone())
                )
                .remove(0)
                .unwrap(),
            keys
        );
        let endpoints = [
            HostEndpoint {
                host: "slow.test".into(),
                port: 22,
            },
            HostEndpoint {
                host: "unlooked.test".into(),
                port: 22,
            },
            cached,
        ];
        let mut calls = 0;
        let results = cache.lookup_batch_with(
            &path,
            &endpoints,
            Duration::from_millis(20),
            |_, _, timeout| {
                calls += 1;
                assert!(timeout <= Duration::from_millis(20));
                std::thread::sleep(Duration::from_millis(30));
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "fixture deadline",
                ))
            },
        );
        assert_eq!(calls, 1);
        assert!(results[0].is_err() && results[1].is_err());
        assert_eq!(results[2].as_ref().unwrap(), &keys);
    }

    #[test]
    fn cache_total_lookup_budget_is_capped_at_750ms() {
        use std::time::{Duration, Instant};
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        std::fs::write(&path, "# fixture\n").unwrap();
        let endpoints: Vec<_> = (0..10)
            .map(|n| HostEndpoint {
                host: format!("slow{n}.test"),
                port: 22,
            })
            .collect();
        let mut cache = TrustedKeyCache::default();
        let mut calls = 0;
        let start = Instant::now();
        let results = cache.lookup_batch_with(
            &path,
            &endpoints,
            Duration::from_secs(20),
            |_, _, timeout| {
                calls += 1;
                assert!(timeout <= Duration::from_millis(200));
                std::thread::sleep(timeout + Duration::from_millis(1));
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "fixture deadline",
                ))
            },
        );
        assert!(calls > 0 && calls <= 4, "{calls} fresh lookups");
        assert_eq!(results.len(), endpoints.len());
        assert!(results.iter().all(Result::is_err));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cache_caps_entries_and_returns_one_result_per_input() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        std::fs::write(&path, "# fixture\n").unwrap();
        let endpoints: Vec<_> = (0..1100)
            .map(|n| HostEndpoint {
                host: format!("host{n}.test"),
                port: 22,
            })
            .collect();
        let mut cache = TrustedKeyCache::default();
        let results = cache.lookup_batch_with(
            &path,
            &endpoints,
            Duration::from_secs(20),
            |_, _, timeout| {
                assert!(timeout <= Duration::from_millis(200));
                Ok(Vec::new())
            },
        );
        assert_eq!(results.len(), endpoints.len());
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(cache.entries.len(), 1024);
        let results = cache.lookup_batch(
            &path,
            &[endpoints[0].clone(), endpoints[1099].clone()],
            Duration::ZERO,
        );
        assert!(results[0].is_err());
        assert!(results[1].as_ref().unwrap().is_empty());
    }

    #[test]
    fn cache_replacement_deletion_and_stat_errors_never_reuse_pins() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        std::fs::write(&path, "# fixture\n").unwrap();
        let endpoint = HostEndpoint {
            host: "host.test".into(),
            port: 22,
        };
        let endpoints = [endpoint];
        let mut cache = TrustedKeyCache::default();
        cache
            .lookup_batch_with(&path, &endpoints, Duration::from_millis(750), |_, _, _| {
                Ok(vec![key("ssh-ed25519", "AAAAAAAAAAAAAAAA")])
            })
            .remove(0)
            .unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let replacement = fixture.0.join("replacement");
        std::fs::write(&replacement, "# fixture\n").unwrap();
        std::fs::File::open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(cache
            .lookup_batch(&path, &endpoints, Duration::ZERO)
            .remove(0)
            .is_err());
        assert!(cache.entries.is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(cache
            .lookup_batch(&path, &endpoints, Duration::ZERO)
            .remove(0)
            .unwrap()
            .is_empty());
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            cache
                .lookup_batch(&path, &endpoints, Duration::ZERO)
                .remove(0)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn cache_racing_file_change_invalidates_every_result_including_cached() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        std::fs::write(&path, "# fixture\n").unwrap();
        let cached = HostEndpoint {
            host: "cached.test".into(),
            port: 22,
        };
        let mut cache = TrustedKeyCache::default();
        cache
            .lookup_batch_with(
                &path,
                std::slice::from_ref(&cached),
                Duration::from_millis(750),
                |_, _, _| Ok(vec![key("ssh-ed25519", "AAAAAAAAAAAAAAAA")]),
            )
            .remove(0)
            .unwrap();
        let results = cache.lookup_batch_with(
            &path,
            &[
                cached,
                HostEndpoint {
                    host: "new.test".into(),
                    port: 22,
                },
            ],
            Duration::from_millis(750),
            |path, _, _| {
                std::fs::write(path, "# revoked or replaced during batch\n").unwrap();
                Ok(Vec::new())
            },
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(Result::is_err));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn cache_failures_are_retryable_and_raw_host_spellings_are_not_interchangeable() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        std::fs::write(&path, "# fixture\n").unwrap();
        let endpoint = HostEndpoint {
            host: "DEVICE.test".into(),
            port: 22,
        };
        let endpoints = [endpoint.clone()];
        let mut cache = TrustedKeyCache::default();
        assert!(cache
            .lookup_batch_with(
                &path,
                &endpoints,
                Duration::from_millis(750),
                |_, _, _| Err(std::io::Error::other("fixture failure"))
            )
            .remove(0)
            .is_err());
        assert!(cache.entries.is_empty());
        assert!(cache
            .lookup_batch_with(&path, &endpoints, Duration::from_millis(750), |_, _, _| Ok(
                Vec::new()
            ))
            .remove(0)
            .unwrap()
            .is_empty());
        let normalized = HostEndpoint {
            host: "device.test".into(),
            port: 22,
        };
        assert!(cache
            .lookup_batch(&path, &[normalized], Duration::ZERO)
            .remove(0)
            .is_err());
        assert!(cache
            .lookup_batch(&path, &[endpoint], Duration::ZERO)
            .remove(0)
            .is_ok());
    }

    #[test]
    fn local_known_hosts_lookup_reads_only_explicit_endpoint() {
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        let text = format!(
            "# untouched\nexample.test {} {}\nother.test ssh-rsa AAAA\n",
            shared.key_type, shared.key
        );
        std::fs::write(&path, &text).unwrap();
        let endpoint = HostEndpoint {
            host: "example.test".into(),
            port: 22,
        };
        assert_eq!(
            trusted_host_keys(&path, &endpoint, std::time::Duration::from_secs(1)).unwrap(),
            vec![shared]
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let absent = HostEndpoint {
            host: "missing.test".into(),
            port: 22,
        };
        assert!(
            trusted_host_keys(&path, &absent, std::time::Duration::from_secs(1))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tailnet_dns_and_ip_evidence_links_routes_transitively_in_input_order() {
        let first = route("lan", "192.0.2.1");
        let other = route("separate", "192.0.2.9");
        let vpn = route("vpn-ip", "100.64.0.1");
        let mut peer = route("device", "device.tail.test");
        peer.route.kind = RouteKind::Tailscale;
        peer.endpoints.extend(first.endpoints.clone());
        peer.endpoints.extend(vpn.endpoints.clone());
        let expected = vec![first.route.clone(), vpn.route.clone(), peer.route.clone()];
        let groups = group_computers(vec![first, other.clone(), vpn, peer]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "lan");
        assert_eq!(groups[0].routes, expected);
        assert_eq!(groups[1].routes, vec![other.route]);
    }

    #[test]
    fn endpoint_comparison_normalizes_ip_case_and_dot_but_not_dns_suffixes() {
        assert_eq!(
            group_computers(vec![
                route("a", "2001:db8::1"),
                route("b", "2001:0db8:0:0:0:0:0:1")
            ])
            .len(),
            1
        );
        assert_eq!(
            group_computers(vec![
                route("a", "DEVICE.example"),
                route("b", "device.example")
            ])
            .len(),
            1
        );
        assert_eq!(
            group_computers(vec![route("a", "device"), route("b", "device.example")]).len(),
            2
        );
        assert_eq!(
            group_computers(vec![route("a", "device"), route("b", "device.")]).len(),
            1
        );
        assert!(group_computers(Vec::new()).is_empty());
    }

    #[test]
    fn hashed_known_hosts_respects_nondefault_ports_including_ipv6() {
        use std::process::{Command, Stdio};
        use std::time::Duration;
        let fixture = Fixture::new();
        // A hostile-looking file name is still just one explicit -f argument.
        let path = fixture.0.join("known hosts; touch CANARY");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        std::fs::write(
            &path,
            format!(
                "[example.test]:2222 {} {}\n[2001:db8::1]:2200 {} {}\n",
                shared.key_type, shared.key, shared.key_type, shared.key
            ),
        )
        .unwrap();
        let status = Command::new("ssh-keygen")
            .arg("-H")
            .arg("-f")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let before = std::fs::read(&path).unwrap();
        assert!(std::str::from_utf8(&before).unwrap().starts_with("|1|"));
        for (host, port) in [("example.test", 2222), ("2001:db8::1", 2200)] {
            let endpoint = HostEndpoint {
                host: host.into(),
                port,
            };
            assert_eq!(
                trusted_host_keys(&path, &endpoint, Duration::from_secs(1)).unwrap(),
                vec![shared.clone()]
            );
            let default = HostEndpoint {
                host: host.into(),
                port: 22,
            };
            assert!(trusted_host_keys(&path, &default, Duration::from_secs(1))
                .unwrap()
                .is_empty());
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!fixture.0.join("CANARY").exists());
    }

    #[test]
    fn hashed_lookup_preserves_the_explicit_host_spelling() {
        use std::process::{Command, Stdio};
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        let host = "2001:0db8:0:0:0:0:0:1";
        std::fs::write(
            &path,
            format!("[{host}]:2223 {} {}\n", shared.key_type, shared.key),
        )
        .unwrap();
        assert!(Command::new("ssh-keygen")
            .arg("-H")
            .arg("-f")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        let endpoint = HostEndpoint {
            host: host.into(),
            port: 2223,
        };
        assert_eq!(
            trusted_host_keys(&path, &endpoint, Duration::from_secs(1)).unwrap(),
            vec![shared]
        );
    }

    #[test]
    fn revoked_and_certificate_authority_entries_never_supply_identity() {
        use std::time::Duration;
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        for text in [
            format!(
                "@cert-authority example.test {} {}\n",
                shared.key_type, shared.key
            ),
            format!(
                "example.test {} {}\n@revoked example.test {} {}\n",
                shared.key_type, shared.key, shared.key_type, shared.key
            ),
            format!(
                "@revoked example.test {} {}\nexample.test {} {}\n",
                shared.key_type, shared.key, shared.key_type, shared.key
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            let endpoint = HostEndpoint {
                host: "example.test".into(),
                port: 22,
            };
            assert!(trusted_host_keys(&path, &endpoint, Duration::from_secs(1))
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn wildcard_and_globally_revoked_keys_are_not_identity() {
        let fixture = Fixture::new();
        let path = fixture.0.join("known_hosts");
        let shared = key(
            "ssh-ed25519",
            "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        let endpoint = HostEndpoint {
            host: "example.test".into(),
            port: 22,
        };
        for text in [
            format!("*.test {} {}\n", shared.key_type, shared.key),
            format!(
                "example.test {} {}\n@revoked other.test {} {}\n",
                shared.key_type, shared.key, shared.key_type, shared.key
            ),
        ] {
            std::fs::write(&path, &text).unwrap();
            assert!(
                trusted_host_keys(&path, &endpoint, std::time::Duration::from_secs(1))
                    .unwrap()
                    .is_empty(),
                "{text}"
            );
        }
    }

    #[test]
    fn local_lookup_rejects_unsafe_endpoints_nonfiles_and_oversized_files() {
        use std::io::ErrorKind;
        use std::time::Duration;
        let fixture = Fixture::new();
        let missing = fixture.0.join("missing");
        let endpoint = HostEndpoint {
            host: "example.test".into(),
            port: 22,
        };
        assert!(
            trusted_host_keys(&missing, &endpoint, Duration::from_secs(1))
                .unwrap()
                .is_empty()
        );
        for host in [
            "-f",
            "x;touch CANARY",
            "$(touch CANARY)",
            "x\ny",
            "user@host",
            "host/path",
        ] {
            let bad = HostEndpoint {
                host: host.into(),
                port: 22,
            };
            assert_eq!(
                trusted_host_keys(&missing, &bad, Duration::from_secs(1))
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidInput
            );
        }
        assert_eq!(
            trusted_host_keys(&fixture.0, &endpoint, Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
        let large = fixture.0.join("large");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(8 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            trusted_host_keys(&large, &endpoint, Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn malicious_route_labels_and_targets_remain_inert_display_data() {
        let mut input = route("$(touch CANARY)", "192.0.2.1");
        input.route.target = "-oProxyCommand=touch CANARY".into();
        input.route.detail = "Match exec touch CANARY".into();
        input.route.path = Some("/folder;touch CANARY".into());
        let expected = input.route.clone();
        let groups = group_computers(vec![input]);
        assert_eq!(groups[0].routes, vec![expected]);
    }

    #[test]
    fn lookup_deadline_and_output_budget_fail_closed() {
        use std::io::ErrorKind;
        use std::process::Command;
        use std::time::{Duration, Instant};
        let fixture = Fixture::new();
        let canary = fixture.0.join("CANARY");
        let mut zero = Command::new("touch");
        zero.arg(&canary);
        assert_eq!(
            lookup_output(&mut zero, Duration::ZERO).unwrap_err().kind(),
            ErrorKind::TimedOut
        );
        assert!(!canary.exists());
        let mut slow = Command::new("sleep");
        slow.arg("2");
        let start = Instant::now();
        assert_eq!(
            lookup_output(&mut slow, Duration::from_millis(30))
                .unwrap_err()
                .kind(),
            ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        let big = fixture.0.join("output");
        std::fs::write(&big, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        let mut flood = Command::new("cat");
        flood.arg(&big);
        assert_eq!(
            lookup_output(&mut flood, Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidData
        );
        let mut failure = Command::new("sh");
        failure.args(["-c", "printf 'partial output'; exit 1"]);
        assert!(lookup_output(&mut failure, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn tailscale_route_kind_preserves_existing_tailnet_action_token() {
        assert_eq!(
            serde_json::to_value(RouteKind::Tailscale).unwrap(),
            "tailnet"
        );
        assert_eq!(
            serde_json::from_value::<RouteKind>(serde_json::json!("tailnet")).unwrap(),
            RouteKind::Tailscale
        );
    }

    #[test]
    fn singleton_preserves_route_and_serializes_only_display_fields() {
        let input = route("studio", "192.0.2.1");
        let expected = input.route.clone();
        let groups = group_computers(vec![input]);
        assert_eq!(
            groups,
            vec![Computer {
                name: "studio".into(),
                routes: vec![expected]
            }]
        );
        let value = serde_json::to_value(&groups[0]).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert_eq!(value["routes"][0]["kind"], "ssh");
        assert_eq!(
            serde_json::from_value::<Computer>(value).unwrap(),
            groups[0]
        );
    }
}
