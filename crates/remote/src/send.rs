//! Explicit, cancellable sends. Never moves, deletes sources, or changes services.
use crate::helper::{self, CallContext, ErrorCode, RemoteError};
use gust_core::lister::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{File, Metadata};
use std::io::Write;
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendReceipt {
    pub accepted_files: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendFailure {
    pub error: RemoteError,
    /// Confirmed protocol completions before the failure, never assumed rollback.
    pub accepted_files: usize,
    pub delivery_uncertain: bool,
}
#[derive(Debug, Clone)]
pub struct LocalSendPeer {
    pub address: SocketAddr,
    /// Explicitly trusted SPKI pin, `sha256//` plus Base64 SHA-256.
    /// This is a public key pin, not LocalSend's certificate fingerprint.
    pub public_key_pin: String,
}

#[derive(Debug, Clone)]
pub struct LocalSendIdentity {
    pub peer: LocalSendPeer,
    /// Untrusted until the person verifies it with the receiver and accepts.
    pub certificate_fingerprint: String,
}

struct ClientIdentity {
    pem: File,
    fingerprint: String,
}
impl ClientIdentity {
    fn new(ctx: &CallContext<'_>) -> Result<Self, RemoteError> {
        let started = Instant::now();
        let mut command = Command::new("openssl");
        // Both PEM blocks go to a pipe, never a default private-key file.
        command.args([
            "req",
            "-config",
            "/dev/null",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-x509",
            "-days",
            "1",
            "-subj",
            "/CN=Gust",
            "-keyout",
            "/dev/stdout",
            "-out",
            "/dev/stdout",
            "-batch",
        ]);
        let mut output = helper::run(command, &[], ctx)?;
        if !output.status.success() {
            return Err(RemoteError::new(ErrorCode::Tls));
        }
        let fd = rustix::fs::memfd_create(
            "gust-send-identity",
            rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
        )
        .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
        let mut pem = File::from(fd);
        rustix::fs::fchmod(&pem, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)
            .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
        pem.write_all(&output.stdout)
            .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
        let mut command = Command::new("openssl");
        command.args(["x509", "-noout", "-fingerprint", "-sha256"]);
        let fingerprint = helper::run(command, &output.stdout, &remaining(ctx, started));
        output.stdout.fill(0);
        let fingerprint = fingerprint?;
        if !fingerprint.status.success() {
            return Err(RemoteError::new(ErrorCode::Tls));
        }
        let fingerprint = String::from_utf8(fingerprint.stdout)
            .map_err(|_| RemoteError::new(ErrorCode::Protocol))?;
        let fingerprint = fingerprint
            .trim()
            .split_once('=')
            .map(|(_, v)| v.replace(':', ""))
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
        rustix::fs::fcntl_add_seals(
            &pem,
            rustix::fs::SealFlags::SEAL
                | rustix::fs::SealFlags::SHRINK
                | rustix::fs::SealFlags::GROW
                | rustix::fs::SealFlags::WRITE,
        )
        .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
        Ok(Self { pem, fingerprint })
    }
    fn fd_path(&self) -> String {
        format!("/proc/{}/fd/{}", std::process::id(), self.pem.as_raw_fd())
    }
}

/// Explicit address inspection, never discovery or automatic trust. The UI
/// presents this fingerprint and obtains approval before calling `localsend`.
pub fn inspect_localsend(
    address: SocketAddr,
    ctx: &CallContext<'_>,
) -> Result<LocalSendIdentity, RemoteError> {
    let started = Instant::now();
    let mut command = Command::new("openssl");
    command.args(["s_client", "-connect", &address.to_string(), "-showcerts"]);
    let out = helper::run(command, &[], &remaining(ctx, started))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text
        .find("-----BEGIN CERTIFICATE-----")
        .ok_or_else(|| RemoteError::new(ErrorCode::Tls))?;
    let end = text[start..]
        .find("-----END CERTIFICATE-----")
        .map(|n| start + n + 25)
        .ok_or_else(|| RemoteError::new(ErrorCode::Tls))?;
    let cert = &text.as_bytes()[start..end];
    if cert.len() > 64 * 1024 {
        return Err(RemoteError::new(ErrorCode::Protocol));
    }
    let openssl = |args: &[&str], input: &[u8]| -> Result<Vec<u8>, RemoteError> {
        let mut command = Command::new("openssl");
        command.args(args);
        let out = helper::run(command, input, &remaining(ctx, started))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(RemoteError::new(ErrorCode::Tls))
        }
    };
    let fingerprint = openssl(&["x509", "-noout", "-fingerprint", "-sha256"], cert)?;
    let fingerprint =
        String::from_utf8(fingerprint).map_err(|_| RemoteError::new(ErrorCode::Protocol))?;
    let fingerprint = fingerprint
        .trim()
        .split_once('=')
        .map(|(_, s)| s)
        .filter(|s| s.len() == 95 && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':'))
        .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
    let public = openssl(&["x509", "-pubkey", "-noout"], cert)?;
    let der = openssl(&["pkey", "-pubin", "-outform", "DER"], &public)?;
    let digest = openssl(&["dgst", "-sha256", "-binary"], &der)?;
    if digest.len() != 32 {
        return Err(RemoteError::new(ErrorCode::Protocol));
    }
    Ok(LocalSendIdentity {
        peer: LocalSendPeer {
            address,
            public_key_pin: format!("sha256//{}", base64(&digest)),
        },
        certificate_fingerprint: fingerprint.into(),
    })
}
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
pub fn capability_explanation(service: &str) -> &'static str {
    match service {
        "taildrop" => "Needs Tailscale, an eligible receiving device and Taildrop permission. Sending copies regular local files; it does not mount a device.",
        "localsend" => "Needs a running LocalSend receiver, its address and an explicitly trusted TLS public key pin. The receiver must approve the files. Sending copies regular local files.",
        _ => "This send service is unsupported.",
    }
}
struct Source {
    file: File,
    identity: Metadata,
    name: String,
}
impl Source {
    fn open(path: &Path) -> Result<Self, RemoteError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            || gust_core::stall::fstype_for(path).is_none_or(|fs| gust_core::stall::can_stall(&fs))
        {
            return Err(RemoteError::new(ErrorCode::InvalidInput));
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.chars().any(char::is_control))
            .ok_or_else(|| RemoteError::new(ErrorCode::InvalidInput))?
            .to_string();
        let fd = rustix::fs::openat2(
            rustix::fs::CWD,
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::NO_SYMLINKS | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|_| RemoteError::new(ErrorCode::PermissionDenied))?;
        let file = File::from(fd);
        let identity = file
            .metadata()
            .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
        if !identity.is_file() {
            return Err(RemoteError::new(ErrorCode::InvalidInput));
        }
        Ok(Self {
            file,
            identity,
            name,
        })
    }
    fn fd_path(&self) -> String {
        format!("/proc/{}/fd/{}", std::process::id(), self.file.as_raw_fd())
    }
    fn unchanged(&self) -> Result<(), RemoteError> {
        let now = self
            .file
            .metadata()
            .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
        let stamp = |m: &Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        if stamp(&now) == stamp(&self.identity) {
            Ok(())
        } else {
            Err(RemoteError::new(ErrorCode::Failed))
        }
    }
}
fn sources(paths: &[PathBuf]) -> Result<Vec<Source>, RemoteError> {
    if paths.is_empty() || paths.len() > 128 {
        return Err(RemoteError::new(ErrorCode::InvalidInput));
    }
    paths.iter().map(|p| Source::open(p)).collect()
}
fn remaining<'a>(ctx: &CallContext<'a>, started: Instant) -> CallContext<'a> {
    CallContext {
        cancel: ctx.cancel,
        timeout: ctx.timeout.saturating_sub(started.elapsed()),
    }
}
fn failure(error: RemoteError, accepted_files: usize, delivery_uncertain: bool) -> SendFailure {
    SendFailure {
        error,
        accepted_files,
        delivery_uncertain,
    }
}

pub fn taildrop(
    target: &str,
    paths: &[PathBuf],
    ctx: &CallContext<'_>,
) -> Result<SendReceipt, SendFailure> {
    taildrop_with(target, paths, ctx, || Command::new("tailscale"))
}
fn taildrop_with(
    target: &str,
    paths: &[PathBuf],
    ctx: &CallContext<'_>,
    mut make_command: impl FnMut() -> Command,
) -> Result<SendReceipt, SendFailure> {
    ctx.check().map_err(|e| failure(e, 0, false))?;
    if !crate::tailscale::safe_target(target) {
        return Err(failure(RemoteError::new(ErrorCode::InvalidInput), 0, false));
    }
    let started = Instant::now();
    let sources = sources(paths).map_err(|e| failure(e, 0, false))?;
    let target = if target.contains(':') {
        format!("[{target}]:")
    } else {
        format!("{target}:")
    };
    let mut accepted = 0;
    for source in sources {
        source
            .unchanged()
            .map_err(|e| failure(e, accepted, false))?;
        let mut command = make_command();
        command
            .args(["file", "cp", "--name"])
            .arg(&source.name)
            .arg("--")
            .arg(source.fd_path())
            .arg(&target);
        let out = helper::run(command, &[], &remaining(ctx, started))
            .map_err(|e| failure(e, accepted, true))?;
        if !out.status.success() {
            return Err(failure(RemoteError {code:ErrorCode::Failed,message:"Taildrop refused the send. Check that the receiver is online, eligible, and allowed by tailnet policy.".into()},accepted,true));
        }
        accepted += 1;
        source.unchanged().map_err(|e| failure(e, accepted, true))?;
    }
    Ok(SendReceipt {
        accepted_files: accepted,
    })
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn config_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}
enum Body<'a> {
    Json(&'a str),
    File(&'a Source),
    Empty,
}
#[cfg(test)]
fn request(
    peer: &LocalSendPeer,
    path: &str,
    body: Body<'_>,
    ctx: &CallContext<'_>,
) -> Result<(u16, Vec<u8>), RemoteError> {
    request_at(peer, path, body, ctx, false, None)
}
fn request_at(
    peer: &LocalSendPeer,
    path: &str,
    body: Body<'_>,
    ctx: &CallContext<'_>,
    fixture_http: bool,
    identity: Option<&ClientIdentity>,
) -> Result<(u16, Vec<u8>), RemoteError> {
    ctx.check()?;
    let pin = peer
        .public_key_pin
        .strip_prefix("sha256//")
        .ok_or_else(|| RemoteError::new(ErrorCode::Tls))?;
    if pin.len() != 44
        || !pin
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return Err(RemoteError::new(ErrorCode::Tls));
    }
    let scheme = if fixture_http && peer.address.ip().is_loopback() {
        "http"
    } else {
        "https"
    };
    let mut config = format!(
        "url = {}\nrequest = \"POST\"\n",
        config_quote(&format!(
            "{scheme}://{}/api/localsend/v2/{path}",
            peer.address
        ))
    );
    match body {
        Body::Json(json) => {
            config.push_str("header = \"Content-Type: application/json\"\n");
            config.push_str(&format!("data-binary = {}\n", config_quote(json)));
        }
        Body::File(source) => config.push_str(&format!(
            "header = \"Content-Type: application/octet-stream\"\nupload-file = {}\n",
            config_quote(&source.fd_path())
        )),
        Body::Empty => config.push_str("data-binary = \"\"\n"),
    }
    let mut command = Command::new("curl");
    command.env_remove("SSLKEYLOGFILE");
    command.args([
        "--disable",
        "--config",
        "-",
        "--silent",
        "--show-error",
        "--noproxy",
        "*",
        "--write-out",
        "\n%{http_code}",
        "--proto",
        if scheme == "https" { "=https" } else { "=http" },
    ]);
    if scheme == "https" {
        // LocalSend uses self-signed TLS. The explicit trusted key replaces
        // public CA/hostname authentication; mismatched keys still fail closed.
        command.args(["--insecure", "--pinnedpubkey", &peer.public_key_pin]);
        if let Some(identity) = identity {
            let pem = identity.fd_path();
            command.args(["--cert", &pem, "--key", &pem]);
        }
    }
    let result = helper::run(command, config.as_bytes(), ctx);
    config.clear();
    let out = result?;
    if !out.status.success() {
        return Err(RemoteError::new(
            if matches!(out.status.code(), Some(35 | 51 | 58 | 60 | 83 | 90 | 91)) {
                ErrorCode::Tls
            } else {
                ErrorCode::Failed
            },
        ));
    }
    let split = out
        .stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
    let code = std::str::from_utf8(&out.stdout[split + 1..])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
    Ok((code, out.stdout[..split].to_vec()))
}

pub fn localsend(
    peer: &LocalSendPeer,
    paths: &[PathBuf],
    ctx: &CallContext<'_>,
) -> Result<SendReceipt, SendFailure> {
    localsend_with_pin(peer, paths, None, ctx)
}
/// Optional receiver PIN travels only in the private curl configuration pipe.
pub fn localsend_with_pin(
    peer: &LocalSendPeer,
    paths: &[PathBuf],
    pin: Option<String>,
    ctx: &CallContext<'_>,
) -> Result<SendReceipt, SendFailure> {
    if pin
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 64 || s.chars().any(char::is_control))
    {
        return Err(failure(RemoteError::new(ErrorCode::InvalidInput), 0, false));
    }
    let started = Instant::now();
    let identity = ClientIdentity::new(ctx).map_err(|e| failure(e, 0, false))?;
    localsend_with_fingerprint(
        peer,
        paths,
        &remaining(ctx, started),
        &identity.fingerprint,
        |path, body, ctx| {
            let route = if path == "prepare-upload" {
                pin.as_ref()
                    .map(|pin| format!("{path}?pin={}", encode(pin)))
            } else {
                None
            };
            request_at(
                peer,
                route.as_deref().unwrap_or(path),
                body,
                ctx,
                false,
                Some(&identity),
            )
        },
    )
}
#[cfg(test)]
fn localsend_with(
    _peer: &LocalSendPeer,
    paths: &[PathBuf],
    ctx: &CallContext<'_>,
    transport: impl FnMut(&str, Body<'_>, &CallContext<'_>) -> Result<(u16, Vec<u8>), RemoteError>,
) -> Result<SendReceipt, SendFailure> {
    localsend_with_fingerprint(_peer, paths, ctx, "synthetic-fixture", transport)
}
fn localsend_with_fingerprint(
    _peer: &LocalSendPeer,
    paths: &[PathBuf],
    ctx: &CallContext<'_>,
    fingerprint: &str,
    mut transport: impl FnMut(&str, Body<'_>, &CallContext<'_>) -> Result<(u16, Vec<u8>), RemoteError>,
) -> Result<SendReceipt, SendFailure> {
    ctx.check().map_err(|e| failure(e, 0, false))?;
    let started = Instant::now();
    let sources = sources(paths).map_err(|e| failure(e, 0, false))?;
    let files:serde_json::Map<String,serde_json::Value>=sources.iter().enumerate().map(|(i,s)| {
        let id=i.to_string(); (id.clone(),json!({"id":id,"fileName":s.name,"size":s.identity.len(),"fileType":"application/octet-stream"}))
    }).collect();
    let prepare=json!({"info":{"alias":"Gust","version":"2.2","deviceType":"desktop","fingerprint":fingerprint,"port":0,"protocol":"https","download":false},"files":files}).to_string();
    let (status, body) = transport(
        "prepare-upload",
        Body::Json(&prepare),
        &remaining(ctx, started),
    )
    .map_err(|e| failure(e, 0, true))?;
    if status == 204 {
        return Ok(SendReceipt { accepted_files: 0 });
    }
    if status != 200 {
        return Err(failure(http_error(status), 0, false));
    }
    let reply: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| failure(RemoteError::new(ErrorCode::Protocol), 0, true))?;
    let session = reply["sessionId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(|| failure(RemoteError::new(ErrorCode::Protocol), 0, true))?;
    let tokens = reply["files"]
        .as_object()
        .ok_or_else(|| failure(RemoteError::new(ErrorCode::Protocol), 0, true))?;
    let mut accepted = 0;
    let result = (|| {
        if tokens
            .keys()
            .any(|id| id.parse::<usize>().ok().is_none_or(|i| i >= sources.len()))
        {
            return Err(RemoteError::new(ErrorCode::Protocol));
        }
        for (i, source) in sources.iter().enumerate() {
            let Some(token) = tokens.get(&i.to_string()) else {
                continue;
            };
            let token = token
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 4096)
                .ok_or_else(|| RemoteError::new(ErrorCode::Protocol))?;
            source.unchanged()?;
            let path = format!(
                "upload?sessionId={}&fileId={i}&token={}",
                encode(session),
                encode(token)
            );
            let (status, _) = transport(&path, Body::File(source), &remaining(ctx, started))?;
            if status != 200 {
                return Err(http_error(status));
            }
            accepted += 1;
            source.unchanged()?;
        }
        Ok(SendReceipt {
            accepted_files: accepted,
        })
    })();
    if result.is_err() {
        // A cancelled token cannot also cancel the cleanup. Charge cleanup to
        // the remaining call budget and cap it at one second; never retry data.
        let cleanup = CancelToken::new();
        let budget = ctx
            .timeout
            .saturating_sub(started.elapsed())
            .min(Duration::from_secs(1));
        if !budget.is_zero() {
            let _ = transport(
                &format!("cancel?sessionId={}", encode(session)),
                Body::Empty,
                &CallContext {
                    cancel: &cleanup,
                    timeout: budget,
                },
            );
        }
    }
    result.map_err(|e| failure(e, accepted, true))
}
fn http_error(status: u16) -> RemoteError {
    RemoteError::new(match status {
        401 => ErrorCode::Authentication,
        403 => ErrorCode::PermissionDenied,
        409 | 429 => ErrorCode::Busy,
        404 => ErrorCode::Unsupported,
        _ => ErrorCode::Failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    fn fixture(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("gust-send-{}-{label}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    fn peer() -> LocalSendPeer {
        LocalSendPeer {
            address: "127.0.0.1:1".parse().unwrap(),
            public_key_pin: format!("sha256//{}=", "A".repeat(43)),
        }
    }
    #[test]
    fn taildrop_uses_held_source_and_keeps_it_after_success_or_cancel() {
        let root = fixture("taildrop");
        let source = root.join("sample.txt");
        let received = root.join("received");
        std::fs::write(&source, b"synthetic Taildrop payload").unwrap();
        let ctx = CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(3),
        };
        let receipt=taildrop_with("peer.example",std::slice::from_ref(&source),&ctx,||{
            let mut command=Command::new("sh");command.args(["-c",r#"destination="$1"; shift; test "$1" = file && test "$2" = cp && test "$3" = --name && test "$4" = sample.txt && test "$5" = -- && test "$7" = peer.example: && cat "$6" > "$destination""#,"fixture"]).arg(&received);command
        }).unwrap();
        assert_eq!(receipt.accepted_files, 1);
        assert_eq!(
            std::fs::read(&received).unwrap(),
            std::fs::read(&source).unwrap()
        );
        let token = CancelToken::new();
        let cancel = token.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        let error = taildrop_with(
            "peer.example",
            std::slice::from_ref(&source),
            &CallContext {
                cancel: &token,
                timeout: Duration::from_secs(2),
            },
            || {
                let mut command = Command::new("sh");
                command.args(["-c", "sleep 10", "fixture"]);
                command
            },
        )
        .unwrap_err();
        canceller.join().unwrap();
        assert_eq!(error.error.code, ErrorCode::Cancelled);
        assert!(error.delivery_uncertain);
        assert!(source.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn local_protocol_transfers_selected_files_and_keeps_sources() {
        let root = fixture("protocol");
        let path = root.join("sample.txt");
        std::fs::write(&path, b"synthetic payload").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = peer();
        peer.address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for n in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut received = Vec::new();
                let mut byte = [0];
                while !received.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    received.push(byte[0]);
                }
                let header = String::from_utf8(received).unwrap();
                let length = header
                    .lines()
                    .find_map(|s| {
                        s.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.parse::<usize>().ok())
                    })
                    .unwrap();
                if header.to_ascii_lowercase().contains("expect: 100-continue") {
                    stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
                }
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let response = if n == 0 {
                    assert!(header.starts_with("POST /api/localsend/v2/prepare-upload "));
                    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(value["files"]["0"]["fileName"], "sample.txt");
                    r#"{"sessionId":"synthetic session","files":{"0":"synthetic token"}}"#
                } else {
                    assert!(header.contains(
                        "sessionId=synthetic%20session&fileId=0&token=synthetic%20token"
                    ));
                    assert_eq!(body, b"synthetic payload");
                    ""
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                )
                .unwrap();
            }
        });
        let token = CancelToken::new();
        let ctx = CallContext {
            cancel: &token,
            timeout: Duration::from_secs(8),
        };
        let receipt = localsend_with(
            &peer,
            std::slice::from_ref(&path),
            &ctx,
            |route, body, ctx| request_at(&peer, route, body, ctx, true, None),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(receipt.accepted_files, 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic payload");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn partial_delivery_and_cancel_are_explicit_and_tokens_stay_private() {
        let root = fixture("cancel");
        let a = root.join("a");
        let b = root.join("b");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let token = CancelToken::new();
        let ctx = CallContext {
            cancel: &token,
            timeout: Duration::from_secs(5),
        };
        let mut calls = 0;
        let mut cancelled = false;
        let failure=localsend_with(&peer(),&[a.clone(),b.clone()],&ctx,|path,_,_|{
            calls+=1;
            if path=="prepare-upload" {return Ok((200,br#"{"sessionId":"private-session","files":{"0":"private-token","1":"private-token"}}"#.to_vec()));}
            if path.starts_with("cancel?") {cancelled=true;return Ok((200,Vec::new()));}
            if calls==2 {Ok((200,Vec::new()))}else{Err(RemoteError::new(ErrorCode::Cancelled))}
        }).unwrap_err();
        assert_eq!(failure.accepted_files, 1);
        assert!(failure.delivery_uncertain && cancelled);
        assert!(!format!("{failure:?}").contains("private-"));
        assert!(a.exists() && b.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn special_sources_and_untrusted_tls_pins_are_refused_before_send() {
        let root = fixture("sources");
        let path = root.join("source");
        std::fs::write(&path, b"data").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Source::open(&link).is_err());
        assert!(Source::open(&root).is_err());
        let mut invalid = peer();
        invalid.public_key_pin = "untrusted".into();
        let ctx = CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(1),
        };
        assert_eq!(
            request(&invalid, "prepare-upload", Body::Empty, &ctx)
                .unwrap_err()
                .code,
            ErrorCode::Tls
        );
        assert_eq!(config_quote("a\"\nurl=x"), "\"a\\\"\\nurl=x\"");
        assert!(std::fs::read(&path).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tls_inspection_pin_refusal_and_mutual_tls_transfer_use_only_memory_keys() {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let root = fixture("tls");
        let path = root.join("sample.txt");
        std::fs::write(&path, b"synthetic TLS payload").unwrap();
        let ctx = CallContext {
            cancel: &CancelToken::new(),
            timeout: Duration::from_secs(15),
        };
        let server_identity = ClientIdentity::new(&ctx).unwrap();
        let client_identity = ClientIdentity::new(&ctx).unwrap();
        struct Server(std::process::Child);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let script = r#"
import http.server,json,ssl,sys
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args): pass
 def do_POST(self):
  assert self.connection.getpeercert(binary_form=True), 'missing client certificate'
  data=self.rfile.read(int(self.headers['Content-Length']))
  if self.path.endswith('/prepare-upload'):
   value=json.loads(data); assert len(value['info']['fingerprint'])==64
   body=b'{"sessionId":"synthetic-session","files":{"0":"synthetic-token"}}'
  else:
   assert data==b'synthetic TLS payload';body=b''
  self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=http.server.HTTPServer(('127.0.0.1',0),Handler)
context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(sys.argv[1]);context.load_verify_locations(sys.argv[2]);context.verify_mode=ssl.CERT_OPTIONAL
server.socket=context.wrap_socket(server.socket,server_side=True)
with open(sys.argv[3],'w') as output: output.write(str(server.server_port))
server.serve_forever()
"#;
        let port_file = root.join("port");
        let child = Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(server_identity.fd_path())
            .arg(client_identity.fd_path())
            .arg(&port_file)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Server(child);
        let started = Instant::now();
        while !port_file.exists() && started.elapsed() < Duration::from_secs(3) {
            assert!(server.0.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(10));
        }
        let port: u16 = std::fs::read_to_string(&port_file)
            .unwrap()
            .parse()
            .unwrap();
        let identity = inspect_localsend(([127, 0, 0, 1], port).into(), &ctx).unwrap();
        assert_eq!(
            identity.certificate_fingerprint.replace(':', ""),
            server_identity.fingerprint
        );
        let mut wrong = identity.peer.clone();
        wrong.public_key_pin = peer().public_key_pin;
        let error = request_at(
            &wrong,
            "prepare-upload",
            Body::Empty,
            &ctx,
            false,
            Some(&client_identity),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Tls);
        let receipt = localsend_with_fingerprint(
            &identity.peer,
            std::slice::from_ref(&path),
            &ctx,
            &client_identity.fingerprint,
            |route, body, ctx| {
                request_at(
                    &identity.peer,
                    route,
                    body,
                    ctx,
                    false,
                    Some(&client_identity),
                )
            },
        )
        .unwrap();
        assert_eq!(receipt.accepted_files, 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic TLS payload");
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            2,
            "keys must never be persisted in fixture files"
        );
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }
}
