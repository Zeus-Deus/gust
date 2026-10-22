//! Owned, bounded helper execution shared by rich previews and media probes.

use gust_core::lister::CancelToken;
use rustix::fs::{Mode, OFlags};
use std::ffi::OsString;
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt},
    process::CommandExt,
};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Error {
    Cancelled,
    Limit,
    Failed,
    Io,
}

pub(crate) fn check(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Only this request's directory is ever removed. No remove-then-create races.
pub(crate) struct Scratch(pub PathBuf);
impl Scratch {
    pub fn new() -> Result<Self, Error> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..100 {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("gust-preview-{}-{n}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(Error::Io),
            }
        }
        Err(Error::Io)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Revision(u64, u64, u64, i64, i64, i64, i64);
fn revision(m: &Metadata) -> Revision {
    Revision(
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

pub(crate) struct Input {
    file: File,
    revision: Revision,
}
impl Input {
    pub fn try_clone(&self) -> Result<Self, Error> {
        Ok(Self {
            file: self.file.try_clone().map_err(|_| Error::Io)?,
            revision: self.revision.clone(),
        })
    }
    pub fn open(path: &Path, max: u64) -> Result<Self, Error> {
        let file: File = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| Error::Io)?
        .into();
        let meta = file.metadata().map_err(|_| Error::Io)?;
        if !meta.is_file() {
            return Err(Error::Io);
        }
        if meta.len() > max {
            return Err(Error::Limit);
        }
        Ok(Self {
            file,
            revision: revision(&meta),
        })
    }
    pub fn unchanged(&self) -> bool {
        self.file
            .metadata()
            .map(|m| revision(&m) == self.revision)
            .unwrap_or(false)
    }
}

/// Lossless adapter to the existing sandbox argv contract.
pub(crate) fn argv(
    spec: &crate::sandbox::SandboxSpec,
    binary: &str,
    args: &[&str],
) -> Vec<OsString> {
    pinned_argv(spec.bwrap_argv(binary, args), &spec.output_dir)
}

/// The FD placeholder is replaced by spawn_input with a request-owned clone.
/// bwrap closes a bound FD; never use stdin, which helpers expect to be open.
pub(crate) fn pinned_argv(args: Vec<String>, output_dir: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = args.into_iter().map(OsString::from).collect();
    v[0] = "/usr/bin/bwrap".into();
    let input = v
        .windows(3)
        .position(|a| a[0] == "--ro-bind" && a[2] == "/in/input")
        .expect("sandbox input contract");
    v[input] = "--ro-bind-fd".into();
    v[input + 1] = "0".into();
    let output = v
        .iter()
        .position(|a| a == "/out")
        .expect("sandbox output contract");
    v[output - 1] = output_dir.as_os_str().to_owned();
    // Bound anonymous helper scratch as well as output files on the host.
    let tmpfs = v
        .iter()
        .position(|a| a == "--tmpfs")
        .expect("sandbox tmpfs contract");
    v.splice(tmpfs..tmpfs, ["--size".into(), "67108864".into()]);
    let command = v
        .iter()
        .position(|a| a == "prlimit")
        .expect("sandbox budget contract");
    let mut settings: Vec<OsString> = [
        "--setenv",
        "HOME",
        "/tmp",
        "--setenv",
        "XDG_CACHE_HOME",
        "/tmp/cache",
        "--setenv",
        "LC_ALL",
        "C",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if Path::new("/etc/fonts").is_dir() {
        settings.extend(
            ["--ro-bind", "/etc/fonts", "/etc/fonts"]
                .into_iter()
                .map(OsString::from),
        );
    }
    v.splice(command..command, settings);
    let command = v
        .iter()
        .position(|a| a == "prlimit")
        .expect("sandbox budget contract");
    v.insert(command + 1, "--core=0".into());
    v
}

pub(crate) struct OwnedChild {
    child: Option<Child>,
}
impl OwnedChild {
    pub fn spawn(argv: &[OsString], stdin: Stdio, stdout: Stdio) -> Result<Self, Error> {
        Self::spawn_inner(argv, stdin, stdout, None)
    }
    pub fn spawn_input(argv: &[OsString], input: &Input, stdout: Stdio) -> Result<Self, Error> {
        // Keep the inherited clone away from standard streams even when the
        // desktop launcher started Gust with one of those streams closed.
        let file: File = rustix::io::fcntl_dupfd_cloexec(&input.file, 3)
            .map_err(|_| Error::Io)?
            .into();
        let mut args = argv.to_owned();
        let index = args
            .windows(3)
            .position(|a| a[0] == "--ro-bind-fd" && a[2] == "/in/input")
            .ok_or(Error::Failed)?;
        args[index + 1] = file.as_raw_fd().to_string().into();
        Self::spawn_inner(&args, Stdio::null(), stdout, Some(file))
    }
    /// A stdout relay does not monitor host output trees. Remove that write
    /// surface entirely, rather than treating a per-file limit as a quota.
    pub fn spawn_stream_input(argv: &[OsString], input: &Input) -> Result<Self, Error> {
        let mut args = argv.to_owned();
        let output = args
            .windows(3)
            .position(|a| a[0] == "--bind" && a[2] == "/out")
            .ok_or(Error::Failed)?;
        args.splice(
            output..output + 3,
            [
                "--size",
                "1048576",
                "--tmpfs",
                "/out",
                "--remount-ro",
                "/out",
            ]
            .map(OsString::from),
        );
        Self::spawn_input(&args, input, Stdio::piped())
    }
    fn spawn_inner(
        argv: &[OsString],
        stdin: Stdio,
        stdout: Stdio,
        input: Option<File>,
    ) -> Result<Self, Error> {
        let (binary, args) = argv.split_first().ok_or(Error::Failed)?;
        let binary = if binary == "bwrap" {
            std::ffi::OsStr::new("/usr/bin/bwrap")
        } else {
            binary.as_os_str()
        };
        let mut command = Command::new(binary);
        command.args(args).process_group(0);
        if let Some(input) = input {
            // SAFETY: the child closure performs only an async-signal-safe
            // fcntl syscall on its owned descriptor. No locks, allocation or
            // parent descriptor flag changes occur. The captured File keeps
            // the FD alive until exec; bwrap consumes it after binding input.
            unsafe {
                command.pre_exec(move || {
                    rustix::io::fcntl_setfd(&input, rustix::io::FdFlags::empty())
                        .map_err(std::io::Error::from)
                });
            }
        }
        let child = command
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Failed)?;
        Ok(Self { child: Some(child) })
    }
    pub fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned child")
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Do not signal a PID after reaping it: it might already be reused.
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = child.kill();
        // Normal helpers die promptly; reap before their scratch is removed.
        // Keep the rare uninterruptible-mount case off the cancellation path.
        for _ in 0..10 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // A mount syscall can be in uninterruptible sleep. Cancellation must
        // still return; a reaper retains ownership until the kernel releases it.
        let _ = std::thread::Builder::new()
            .name("gust-preview-reap".into())
            .spawn(move || {
                let _ = child.wait();
            });
    }
}

pub(crate) fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), Error> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|_| Error::Io)?;
    rustix::fs::fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|_| Error::Io)
}

/// Bounded stdout even when a helper floods or holds its pipe open. No stderr
/// from an untrusted file is copied into errors, logs or display strings.
#[cfg(test)]
pub(crate) fn capture(
    argv: &[OsString],
    cancel: &CancelToken,
    wall: Duration,
    max: usize,
) -> Result<Vec<u8>, Error> {
    capture_in(argv, cancel, wall, max, None, None)
}

pub(crate) fn capture_in(
    argv: &[OsString],
    cancel: &CancelToken,
    wall: Duration,
    max: usize,
    input: Option<&Input>,
    output_dir: Option<&Path>,
) -> Result<Vec<u8>, Error> {
    check(cancel)?;
    let mut child = match input {
        Some(input) => OwnedChild::spawn_input(argv, input, Stdio::piped())?,
        None => OwnedChild::spawn(argv, Stdio::null(), Stdio::piped())?,
    };
    let mut pipe = child.child().stdout.take().ok_or(Error::Failed)?;
    nonblocking(&pipe)?;
    let deadline = Instant::now() + wall;
    let mut output = Vec::new();
    let mut eof = false;
    let mut last_disk_check = Instant::now() - Duration::from_secs(1);
    loop {
        check(cancel)?;
        if Instant::now() >= deadline {
            return Err(Error::Limit);
        }
        if last_disk_check.elapsed() >= Duration::from_millis(50) {
            if let Some(output) = output_dir {
                check_output_tree(output)?;
            }
            last_disk_check = Instant::now();
        }
        let mut buf = [0u8; 8192];
        if !eof {
            match pipe.read(&mut buf) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if output.len().saturating_add(n) > max {
                        return Err(Error::Limit);
                    }
                    output.extend_from_slice(&buf[..n]);
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Io),
            }
        }
        if eof {
            if let Some(status) = child.child().try_wait().map_err(|_| Error::Failed)? {
                return if status.success() {
                    Ok(output)
                } else {
                    Err(Error::Failed)
                };
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn check_output_tree(root: &Path) -> Result<(), Error> {
    use rustix::fs::{openat, statat, AtFlags, Dir};
    let root = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| Error::Io)?;
    let mut pending = vec![(root, 0)];
    let mut entries = 0usize;
    let mut bytes = 0u64;
    while let Some((fd, depth)) = pending.pop() {
        for entry in Dir::read_from(&fd).map_err(|_| Error::Io)? {
            let entry = entry.map_err(|_| Error::Io)?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            entries += 1;
            if entries > 4096 || depth > 12 {
                return Err(Error::Limit);
            }
            let m = match statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(m) => m,
                Err(rustix::io::Errno::NOENT) => continue,
                Err(_) => return Err(Error::Io),
            };
            bytes = bytes.saturating_add(m.st_size.max(0) as u64);
            if bytes > 128 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            if m.st_mode & 0o170000 == 0o040000 {
                match openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(child) => pending.push((child, depth + 1)),
                    Err(rustix::io::Errno::NOENT) => {}
                    Err(_) => return Err(Error::Io),
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn read_file(path: &Path, max: usize, cancel: &CancelToken) -> Result<Vec<u8>, Error> {
    check(cancel)?;
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| Error::Io)?;
    let st = rustix::fs::fstat(&fd).map_err(|_| Error::Io)?;
    if st.st_mode & 0o170000 != 0o100000 {
        return Err(Error::Io);
    }
    if st.st_size < 0 || st.st_size as u64 > max as u64 {
        return Err(Error::Limit);
    }
    let mut out = Vec::new();
    loop {
        check(cancel)?;
        let mut buf = [0; 8192];
        let n = rustix::io::read(&fd, &mut buf).map_err(|_| Error::Io)?;
        if n == 0 {
            return Ok(out);
        }
        if out.len().saturating_add(n) > max {
            return Err(Error::Limit);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str) -> Vec<OsString> {
        vec!["sh".into(), "-c".into(), script.into()]
    }
    #[test]
    fn streaming_spawn_has_no_host_output_write_surface() {
        gust_testkit::skip_unless!(
            crate::sandbox::sandbox_available(),
            "PREVIEWS",
            "sandbox unavailable"
        );
        let d = Scratch::new().unwrap();
        let path = d.0.join("input");
        std::fs::write(&path, b"synthetic").unwrap();
        let input = Input::open(&path, 100).unwrap();
        let mut spec = crate::sandbox::SandboxSpec::new("/in/input", &d.0);
        spec.wall_timeout_seconds = 5;
        let args = argv(&spec, "sh", &["-c", "truncate -s 1 /tmp/canary || exit 1; i=0; written=0; while [ \"$i\" -lt 18 ]; do if truncate -s 8388608 \"/out/f$i\" 2>/dev/null; then written=$((written+1)); fi; i=$((i+1)); done; printf '%s' \"$written\""]);
        // Exercise the exact constructor used by both direct decoder relays,
        // outside capture_in and its output-tree monitor.
        let mut child = OwnedChild::spawn_stream_input(&args, &input).unwrap();
        let mut output = Vec::new();
        child
            .child()
            .stdout
            .take()
            .unwrap()
            .take(16)
            .read_to_end(&mut output)
            .unwrap();
        assert!(child.child().wait().unwrap().success());
        assert_eq!(output, b"0", "none of the 18 sparse writes may succeed");
        assert_eq!(
            std::fs::read_dir(&d.0).unwrap().count(),
            1,
            "only the source fixture is on the host"
        );
    }
    #[test]
    fn held_input_binding_survives_namespaces_and_placeholder_paths() {
        gust_testkit::skip_unless!(
            crate::sandbox::sandbox_available(),
            "PREVIEWS",
            "sandbox unavailable"
        );
        let d = Scratch::new().unwrap();
        let path = d.0.join("input");
        std::fs::write(&path, b"synthetic").unwrap();
        let input = Input::open(&path, 100).unwrap();
        let spec = crate::sandbox::SandboxSpec::new("/in/input", &d.0);
        let args = argv(
            &spec,
            "sh",
            &["-c", "test -e /proc/self/fd/0 && cat /in/input"],
        );
        assert_eq!(
            capture_in(
                &args,
                &CancelToken::new(),
                Duration::from_secs(5),
                100,
                Some(&input),
                Some(&d.0)
            )
            .unwrap(),
            b"synthetic"
        );
    }
    #[test]
    fn startup_read_and_flood_are_cancellable_and_bounded() {
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            c.cancel();
        });
        let now = Instant::now();
        assert_eq!(
            capture(&shell("sleep 30"), &cancel, Duration::from_secs(10), 1024),
            Err(Error::Cancelled)
        );
        assert!(now.elapsed() < Duration::from_secs(2));
        thread.join().unwrap();
        assert_eq!(
            capture(
                &shell("yes"),
                &CancelToken::new(),
                Duration::from_secs(2),
                1024
            ),
            Err(Error::Limit)
        );
        assert_eq!(
            capture(
                &shell("sleep 30"),
                &CancelToken::new(),
                Duration::from_millis(50),
                1024
            ),
            Err(Error::Limit)
        );
    }
    #[test]
    fn cancellation_kills_the_owned_helper_group() {
        let d = Scratch::new().unwrap();
        let args = vec![
            OsString::from("sh"),
            "-c".into(),
            "sleep 30 & printf '%s\\n' \"$!\" > \"$1/pid\"; wait".into(),
            "gust-test".into(),
            d.0.as_os_str().to_owned(),
        ];
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let worker =
            std::thread::spawn(move || capture(&args, &token, Duration::from_secs(10), 100));
        let until = Instant::now() + Duration::from_secs(2);
        let pid_file = d.0.join("pid");
        while !pid_file.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        let pid: u32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        cancel.cancel();
        assert_eq!(worker.join().unwrap(), Err(Error::Cancelled));
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            let stopped = match stat {
                Err(_) => true,
                Ok(s) => s
                    .split_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z')),
            };
            if stopped {
                break;
            }
            assert!(
                Instant::now() < until,
                "cancelled helper descendant is still running"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn output_budget_never_follows_helper_symlinks() {
        let d = Scratch::new().unwrap();
        let outside = Scratch::new().unwrap();
        std::fs::File::create(outside.0.join("large"))
            .unwrap()
            .set_len(200 * 1024 * 1024)
            .unwrap();
        std::os::unix::fs::symlink(&outside.0, d.0.join("link")).unwrap();
        assert_eq!(check_output_tree(&d.0), Ok(()));
        std::fs::File::create(d.0.join("large"))
            .unwrap()
            .set_len(200 * 1024 * 1024)
            .unwrap();
        assert_eq!(check_output_tree(&d.0), Err(Error::Limit));
    }

    #[test]
    fn output_and_input_reject_special_files_and_change() {
        let d = Scratch::new().unwrap();
        let f = d.0.join("input");
        std::fs::write(&f, b"safe").unwrap();
        let input = Input::open(&f, 10).unwrap();
        assert!(input.unchanged());
        std::fs::write(&f, b"changed").unwrap();
        assert!(!input.unchanged());
        assert!(matches!(Input::open(&f, 2), Err(Error::Limit)));
        let link = d.0.join("link");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        assert_eq!(read_file(&link, 100, &CancelToken::new()), Err(Error::Io));
        let fifo = d.0.join("fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .unwrap();
        assert_eq!(read_file(&fifo, 100, &CancelToken::new()), Err(Error::Io));
    }
}
