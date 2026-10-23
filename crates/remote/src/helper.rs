//! Owned, bounded subprocess calls. No command text or diagnostics are logged.
use gust_core::lister::CancelToken;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    Cancelled,
    Timeout,
    Busy,
    MissingHelper,
    Authentication,
    PermissionDenied,
    Tls,
    Unsupported,
    InvalidInput,
    Protocol,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteError {
    pub code: ErrorCode,
    pub message: String,
}
impl RemoteError {
    pub fn new(code: ErrorCode) -> Self {
        let message = match code {
            ErrorCode::Cancelled => "Cancelled. Some remote work may already have been accepted; check before retrying.",
            ErrorCode::Timeout => "The remote did not answer in time. Check the connection before retrying.",
            ErrorCode::Busy => "Remote helpers are still busy. Wait for them to finish before retrying.",
            ErrorCode::MissingHelper => "The required helper is unavailable. Check connection support before retrying.",
            ErrorCode::Authentication => "The server refused the login. Check the account, domain and credentials.",
            ErrorCode::PermissionDenied => "This account is not allowed to perform that operation in this folder.",
            ErrorCode::Tls => "The server certificate could not be verified. Check its identity; certificate checks remain enabled.",
            ErrorCode::Unsupported => "This service does not support the requested operation.",
            ErrorCode::InvalidInput => "The address or file selection is invalid. Check it before retrying.",
            ErrorCode::Protocol => "The service returned an incomplete or invalid response. No success was assumed.",
            ErrorCode::Failed => "The remote operation failed. Check the service and connection before retrying.",
        };
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RemoteError {}

pub struct CallContext<'a> {
    pub cancel: &'a CancelToken,
    pub timeout: Duration,
}
impl CallContext<'_> {
    pub fn check(&self) -> Result<(), RemoteError> {
        if self.cancel.is_cancelled() {
            Err(RemoteError::new(ErrorCode::Cancelled))
        } else if self.timeout.is_zero() {
            Err(RemoteError::new(ErrorCode::Timeout))
        } else {
            Ok(())
        }
    }
}

const MAX_HELPERS: usize = 8;
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static REAP: OnceLock<Mutex<Vec<OwnedChild>>> = OnceLock::new();

struct OwnedChild {
    child: Child,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
fn kill_group(child: &mut Child) {
    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
}
fn retire(mut child: OwnedChild) {
    kill_group(&mut child.child);
    child.child.stdin.take();
    child.child.stdout.take();
    child.child.stderr.take();
    if matches!(child.child.try_wait(), Ok(Some(_))) {
        return;
    }
    // Retain the admission slot until the kernel has reaped even a stuck child.
    let pending = REAP.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("gust-remote-reaper".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(pending) = REAP.get() {
                    let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
                    pending.retain_mut(|owned| !matches!(owned.child.try_wait(), Ok(Some(_))));
                }
            });
        Mutex::new(Vec::with_capacity(MAX_HELPERS))
    });
    pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(child);
}

pub(crate) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), RemoteError> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|_| RemoteError::new(ErrorCode::Failed))?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| RemoteError::new(ErrorCode::Failed))
}
fn drain(reader: &mut impl Read, output: &mut Vec<u8>) -> Result<bool, RemoteError> {
    let mut buf = [0; 16384];
    // Bounded work per turn, even for a continuously writing child.
    for _ in 0..16 {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(n) if output.len() + n <= OUTPUT_LIMIT => output.extend_from_slice(&buf[..n]),
            Ok(_) => return Err(RemoteError::new(ErrorCode::Protocol)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(RemoteError::new(ErrorCode::Failed)),
        }
    }
    Ok(false)
}

pub(crate) fn run(
    mut command: Command,
    input: &[u8],
    ctx: &CallContext<'_>,
) -> Result<Output, RemoteError> {
    run_borrowed(&mut command, input, ctx)
}
pub(crate) fn run_borrowed(
    command: &mut Command,
    input: &[u8],
    ctx: &CallContext<'_>,
) -> Result<Output, RemoteError> {
    run_inner(
        command,
        input,
        ctx.timeout,
        &|| ctx.cancel.is_cancelled(),
        false,
        &mut |_| None,
        Completion::Terminate,
    )
}

/// SSHFS's successful starter hands off both a daemon in a new session and an
/// SSH transport in the starter's group. Neither may be killed on acceptance.
pub(crate) fn run_daemon_startup(
    mut command: Command,
    ctx: &CallContext<'_>,
) -> Result<Output, RemoteError> {
    run_inner(
        &mut command,
        &[],
        ctx.timeout,
        &|| ctx.cancel.is_cancelled(),
        false,
        &mut |_| None,
        Completion::DaemonStartup,
    )
}

#[derive(Clone, Copy)]
enum Completion {
    Terminate,
    DaemonStartup,
}

/// Interactive adapter for the existing phone question protocol. The callback
/// sees only newly arrived stdout and supplies bounded stdin responses.
pub(crate) fn run_interactive(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
    respond: &mut dyn FnMut(&[u8]) -> Option<Vec<u8>>,
) -> Result<Output, RemoteError> {
    run_inner(
        command,
        input,
        timeout,
        cancelled,
        true,
        respond,
        Completion::Terminate,
    )
}
fn run_inner(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
    interactive: bool,
    respond: &mut dyn FnMut(&[u8]) -> Option<Vec<u8>>,
    completion: Completion,
) -> Result<Output, RemoteError> {
    let started = Instant::now();
    if cancelled() {
        return Err(RemoteError::new(ErrorCode::Cancelled));
    }
    if timeout.is_zero() {
        return Err(RemoteError::new(ErrorCode::Timeout));
    }
    if input.len() > 1024 * 1024 {
        return Err(RemoteError::new(ErrorCode::InvalidInput));
    }
    if let Some(pending) = REAP.get() {
        pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain_mut(|owned| !matches!(owned.child.try_wait(), Ok(Some(_))));
    }
    loop {
        #[allow(deprecated)] // fetch_update supports the workspace Rust 1.90 MSRV.
        let admitted = ACTIVE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_HELPERS).then_some(n + 1)
            })
            .is_ok();
        if admitted {
            break;
        }
        if cancelled() {
            return Err(RemoteError::new(ErrorCode::Cancelled));
        }
        if started.elapsed() >= timeout {
            return Err(RemoteError::new(ErrorCode::Busy));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    command.env_remove("SSLKEYLOGFILE");
    let child = command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            ACTIVE.fetch_sub(1, Ordering::AcqRel);
            return Err(RemoteError::new(
                if e.kind() == std::io::ErrorKind::NotFound {
                    ErrorCode::MissingHelper
                } else {
                    ErrorCode::Failed
                },
            ));
        }
    };
    let mut owned = OwnedChild { child };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let stdin = owned
            .child
            .stdin
            .take()
            .ok_or_else(|| RemoteError::new(ErrorCode::Failed))?;
        let mut stdout = owned
            .child
            .stdout
            .take()
            .ok_or_else(|| RemoteError::new(ErrorCode::Failed))?;
        let mut stderr = owned
            .child
            .stderr
            .take()
            .ok_or_else(|| RemoteError::new(ErrorCode::Failed))?;
        nonblocking(&stdin)?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut stdin = Some(stdin);
        let mut pending = input.to_vec();
        let mut written = 0;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut cancellation_started: Option<Instant> = None;
        loop {
            if cancelled()
                && (!interactive
                    || cancellation_started
                        .get_or_insert_with(Instant::now)
                        .elapsed()
                        >= Duration::from_millis(100))
            {
                return Err(RemoteError::new(ErrorCode::Cancelled));
            }
            if started.elapsed() >= timeout {
                return Err(RemoteError::new(ErrorCode::Timeout));
            }
            if written < pending.len() {
                match stdin
                    .as_mut()
                    .ok_or_else(|| RemoteError::new(ErrorCode::Failed))?
                    .write(&pending[written..])
                {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => written = pending.len(),
                    Err(_) => return Err(RemoteError::new(ErrorCode::Failed)),
                }
            }
            if written == pending.len() && (!interactive || cancellation_started.is_some()) {
                stdin.take();
            }
            let previous = out.len();
            let out_done = drain(&mut stdout, &mut out)?;
            let err_done = drain(&mut stderr, &mut err)?;
            if out.len() != previous {
                if let Some(answer) = respond(&out[previous..]) {
                    if pending.len() + answer.len() > 1024 * 1024 {
                        return Err(RemoteError::new(ErrorCode::Protocol));
                    }
                    pending.extend_from_slice(&answer);
                }
            }
            let status = owned
                .child
                .try_wait()
                .map_err(|_| RemoteError::new(ErrorCode::Failed))?;
            if let Some(status) = status {
                if out_done && err_done {
                    if cancelled() {
                        return Err(RemoteError::new(ErrorCode::Cancelled));
                    }
                    return Ok(Output {
                        status,
                        stdout: out,
                        stderr: err,
                    });
                }
            }
            let mut fds = Vec::new();
            if !out_done {
                fds.push(rustix::event::PollFd::new(
                    &stdout,
                    rustix::event::PollFlags::IN,
                ));
            }
            if !err_done {
                fds.push(rustix::event::PollFd::new(
                    &stderr,
                    rustix::event::PollFlags::IN,
                ));
            }
            if written < pending.len() {
                if let Some(stdin) = &stdin {
                    fds.push(rustix::event::PollFd::new(
                        stdin,
                        rustix::event::PollFlags::OUT,
                    ));
                }
            }
            let wait = if fds.is_empty() {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(20)
            };
            let wait = wait.min(timeout.saturating_sub(started.elapsed()));
            let timespec = rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: wait.as_nanos() as _,
            };
            let _ = rustix::event::poll(&mut fds, Some(&timespec));
        }
    }));
    if matches!(completion, Completion::DaemonStartup)
        && matches!(&result, Ok(Ok(output)) if output.status.success())
    {
        // try_wait already reaped the starter; release its admission slot only.
        drop(owned);
    } else {
        retire(owned);
    }
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    // A daemon in a new session plus a transport in the starter's group, as in
    // SSHFS. Only fixture-owned Unix sockets are used; no mount or SSH service.
    const STARTER: &str = r#"
import os, subprocess, sys, time
transport = '''
import os, socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(4)
s.connect(sys.argv[1])
s.sendall(sys.argv[2].encode())
os.write(int(sys.argv[3]), b'R')
os.close(int(sys.argv[3]))
try:
    while True:
        data = s.recv(1)
        if data != b'p': break
        s.sendall(b'p')
finally:
    s.close()
'''
mode, address = sys.argv[1:]
for role in (['t', 'd'] if mode == 'success' else ['t']):
    r, w = os.pipe()
    subprocess.Popen([sys.executable, '-c', transport, address, role, str(w)],
        pass_fds=(w,), start_new_session=(role == 'd'),
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(w)
    assert os.read(r, 1) == b'R'
    os.close(r)
if mode in ('timeout', 'cancel'): time.sleep(30)
sys.exit(7 if mode == 'failure' else 0)
"#;

    struct StarterFixture {
        root: std::path::PathBuf,
        listener: UnixListener,
    }
    impl StarterFixture {
        fn new(mode: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("gust-starter-{}-{mode}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            let listener = UnixListener::bind(root.join("socket")).unwrap();
            listener.set_nonblocking(true).unwrap();
            Self { root, listener }
        }
        fn command(&self, mode: &str) -> Command {
            let mut command = Command::new("python3");
            command
                .args(["-c", STARTER, mode])
                .arg(self.root.join("socket"));
            command
        }
        fn accept(&self) -> UnixStream {
            let started = Instant::now();
            loop {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        return stream;
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(3) =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("fixture connection: {e}"),
                }
            }
        }
    }
    impl Drop for StarterFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn responds(stream: &mut UnixStream) -> bool {
        let mut role = [0];
        let mut reply = [0];
        let alive = stream.read_exact(&mut role).is_ok()
            && stream.write_all(b"p").is_ok()
            && stream.read_exact(&mut reply).is_ok()
            && reply == *b"p";
        // Both children exit on this owned protocol; even a failed assertion
        // cannot leave a detached fixture daemon running indefinitely.
        let _ = stream.write_all(b"q");
        alive
    }
    #[test]
    fn successful_daemon_startup_preserves_daemon_and_original_group_transport() {
        let fixture = StarterFixture::new("success");
        let output = run_daemon_startup(
            fixture.command("success"),
            &CallContext {
                cancel: &CancelToken::new(),
                timeout: Duration::from_secs(3),
            },
        );
        let alive = [
            responds(&mut fixture.accept()),
            responds(&mut fixture.accept()),
        ];
        assert!(output.unwrap().status.success());
        assert_eq!(alive, [true, true]);
    }
    #[test]
    fn unsuccessful_or_interrupted_daemon_startup_terminates_transport() {
        for mode in ["failure", "timeout", "cancel"] {
            let fixture = StarterFixture::new(mode);
            let command = fixture.command(mode);
            let cancel = CancelToken::new();
            let worker_cancel = cancel.clone();
            let worker = std::thread::spawn(move || {
                run_daemon_startup(
                    command,
                    &CallContext {
                        cancel: &worker_cancel,
                        timeout: if mode == "timeout" {
                            Duration::from_millis(250)
                        } else {
                            Duration::from_secs(3)
                        },
                    },
                )
            });
            let mut stream = fixture.accept();
            if mode == "cancel" {
                cancel.cancel();
            }
            let output = worker.join().unwrap();
            assert!(!responds(&mut stream), "{mode} left transport alive");
            match mode {
                "failure" => assert_eq!(output.unwrap().status.code(), Some(7)),
                _ => assert_eq!(
                    output.err().unwrap().code,
                    if mode == "cancel" {
                        ErrorCode::Cancelled
                    } else {
                        ErrorCode::Timeout
                    }
                ),
            }
        }
    }
    #[test]
    fn drains_both_pipes_and_bounds_descendant_held_pipe() {
        let token = CancelToken::new();
        let mut command = Command::new("sh");
        command.args(["-c", "dd if=/dev/zero bs=65536 count=3 2>/dev/null; dd if=/dev/zero bs=65536 count=3 1>&2 2>/dev/null"]);
        let out = run(
            command,
            &[],
            &CallContext {
                cancel: &token,
                timeout: Duration::from_secs(3),
            },
        )
        .unwrap();
        assert_eq!(out.stdout.len(), 196608);
        assert_eq!(out.stderr.len(), 196608);
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 20 & exit 0"]);
        let started = Instant::now();
        let error = run(
            command,
            &[],
            &CallContext {
                cancel: &token,
                timeout: Duration::from_millis(120),
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::Timeout);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn cancellation_and_oversized_output_never_report_success() {
        let token = CancelToken::new();
        token.cancel();
        let mut command = Command::new("true");
        assert_eq!(
            run(
                command,
                &[],
                &CallContext {
                    cancel: &token,
                    timeout: Duration::from_secs(1)
                }
            )
            .err()
            .unwrap()
            .code,
            ErrorCode::Cancelled
        );
        command = Command::new("sh");
        command.args(["-c", "yes x"]);
        assert_eq!(
            run(
                command,
                &[],
                &CallContext {
                    cancel: &CancelToken::new(),
                    timeout: Duration::from_secs(5)
                }
            )
            .err()
            .unwrap()
            .code,
            ErrorCode::Protocol
        );
    }
}
