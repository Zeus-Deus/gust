//! Sandboxed thumbnailing + output validation.
//!
//! Untrusted image bytes are never decoded in the UI process. A helper runs
//! under `bwrap` (no network, read-only single input, private output dir) with
//! `prlimit` budgets, and the parent then validates the output through a held
//! descriptor: regular file only, size bounded, real PNG, no symlink/FIFO.

use rustix::fs::{Mode, OFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub input: PathBuf,
    pub output_dir: PathBuf,
    pub cpu_seconds: u32,
    pub address_space_bytes: u64,
    pub wall_timeout_seconds: u32,
    pub max_output_bytes: u64,
}

impl SandboxSpec {
    pub fn new(input: impl Into<PathBuf>, output_dir: impl Into<PathBuf>) -> Self {
        SandboxSpec {
            input: input.into(),
            output_dir: output_dir.into(),
            cpu_seconds: 10,
            address_space_bytes: 512 * 1024 * 1024,
            wall_timeout_seconds: 20,
            max_output_bytes: 8 * 1024 * 1024,
        }
    }

    /// Build the bwrap+prlimit argv for a thumbnailer.
    ///
    /// No secret ever goes in argv (AGENTS.md). Network is unshared. The
    /// input is bound read-only at a fixed in-sandbox path.
    pub fn bwrap_argv(&self, helper: &str, helper_args: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = vec![
            "bwrap".into(),
            "--unshare-all".into(),
            "--die-with-parent".into(),
            "--new-session".into(),
            "--clearenv".into(),
            "--setenv".into(),
            "PATH".into(),
            "/usr/bin".into(),
            // Read-only system.
            "--ro-bind".into(),
            "/usr".into(),
            "/usr".into(),
            "--symlink".into(),
            "usr/lib".into(),
            "/lib".into(),
            "--symlink".into(),
            "usr/lib64".into(),
            "/lib64".into(),
            "--symlink".into(),
            "usr/bin".into(),
            "/bin".into(),
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--tmpfs".into(),
            "/tmp".into(),
            // The single untrusted input, read-only.
            "--ro-bind".into(),
            self.input.to_string_lossy().into_owned(),
            "/in/input".into(),
            // Private output directory.
            "--bind".into(),
            self.output_dir.to_string_lossy().into_owned(),
            "/out".into(),
            "--chdir".into(),
            "/".into(),
            // Resource budget.
            "prlimit".into(),
            format!("--cpu={}", self.cpu_seconds),
            format!("--as={}", self.address_space_bytes),
            format!("--fsize={}", self.max_output_bytes),
            "--nproc=64".into(),
            "timeout".into(),
            format!("{}", self.wall_timeout_seconds),
            helper.to_string(),
        ];
        v.extend(helper_args.iter().map(|s| s.to_string()));
        v
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    NotRegularFile,
    TooLarge(u64),
    Empty,
    BadMagic,
    OpenFailed(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::NotRegularFile => write!(f, "helper output is not a regular file"),
            ValidationError::TooLarge(n) => write!(f, "helper output too large ({n} bytes)"),
            ValidationError::Empty => write!(f, "helper produced no output"),
            ValidationError::BadMagic => write!(f, "helper output is not a PNG"),
            ValidationError::OpenFailed(e) => write!(f, "cannot open helper output: {e}"),
        }
    }
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Validate a helper's output through a HELD descriptor.
///
/// Opening with `O_NOFOLLOW` and then `fstat`ing that same descriptor closes
/// the window where a symlink/FIFO is swapped in between check and use.
///
/// `O_NONBLOCK` is essential and not an optimisation: opening a FIFO for
/// reading *blocks until a writer appears*, so a malicious helper that leaves
/// a FIFO where its PNG should be would hang the caller forever. With
/// `O_NONBLOCK` the open returns immediately and `fstat` then rejects it.
pub fn validate_output(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ValidationError> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| ValidationError::OpenFailed(e.to_string()))?;

    let st = rustix::fs::fstat(&fd).map_err(|e| ValidationError::OpenFailed(e.to_string()))?;

    // Regular file only — a FIFO would block us forever, a device is worse.
    if (st.st_mode as u32 & 0o170000) != 0o100000 {
        return Err(ValidationError::NotRegularFile);
    }
    let size = st.st_size as u64;
    if size == 0 {
        return Err(ValidationError::Empty);
    }
    if size > max_bytes {
        return Err(ValidationError::TooLarge(size));
    }

    let mut buf = vec![0u8; size as usize];
    let mut off = 0;
    while off < buf.len() {
        let n = rustix::io::read(&fd, &mut buf[off..])
            .map_err(|e| ValidationError::OpenFailed(e.to_string()))?;
        if n == 0 {
            break;
        }
        off += n;
    }
    buf.truncate(off);

    if buf.len() < 8 || buf[..8] != PNG_MAGIC {
        return Err(ValidationError::BadMagic);
    }
    Ok(buf)
}

/// Is the sandbox available on this system? If not, thumbnailing of untrusted
/// input is simply unavailable — never "run it unsandboxed instead".
pub fn sandbox_available() -> bool {
    which("bwrap") && which("prlimit")
}

/// Is `bin` on PATH?
pub fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|d| {
                let c = d.join(bin);
                c.is_file()
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gust-sbx-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn png_bytes() -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(b"rest of a png");
        v
    }

    #[test]
    fn argv_unshares_network_and_binds_input_readonly() {
        let s = SandboxSpec::new("/home/u/secret.jpg", "/tmp/out");
        let argv = s.bwrap_argv("gust-thumbnailer", &["/in/input", "/out/thumb.png"]);
        let joined = argv.join(" ");
        assert!(joined.contains("--unshare-all"), "{joined}");
        assert!(joined.contains("--die-with-parent"));
        assert!(joined.contains("--clearenv"));
        assert!(joined.contains("--ro-bind /home/u/secret.jpg /in/input"));
        assert!(joined.contains("--cpu=10"));
        assert!(joined.contains("--as=536870912"));
        assert!(joined.contains("timeout 20"));
    }

    #[test]
    fn argv_contains_no_secrets_or_home_paths_beyond_the_input() {
        let s = SandboxSpec::new("/in.jpg", "/tmp/o");
        let argv = s.bwrap_argv("h", &[]);
        // No environment inheritance at all.
        assert!(argv.contains(&"--clearenv".to_string()));
        assert!(!argv
            .iter()
            .any(|a| a.contains("TOKEN") || a.contains("PASSWORD")));
    }

    #[test]
    fn accepts_a_real_png() {
        let d = tmp("ok");
        let f = d.join("t.png");
        fs::write(&f, png_bytes()).unwrap();
        let out = validate_output(&f, 1_000_000).unwrap();
        assert_eq!(&out[..8], &PNG_MAGIC);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rejects_a_symlinked_output() {
        // Helper tries to make us read /etc/passwd via a symlink.
        let d = tmp("symlink");
        fs::write(d.join("real.png"), png_bytes()).unwrap();
        let link = d.join("out.png");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        let err = validate_output(&link, 1_000_000).unwrap_err();
        assert!(
            matches!(err, ValidationError::OpenFailed(_)),
            "symlinked output must be refused, got {err:?}"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rejects_a_fifo_output_without_hanging() {
        // REGRESSION: a plain O_RDONLY open of a FIFO blocks until a writer
        // appears — a hostile helper could hang the thumbnail pipeline
        // forever. The validator must return promptly AND reject.
        // The test runs the call on a worker thread and fails if it does not
        // finish quickly, so a reintroduced blocking open fails the suite
        // instead of hanging it.
        let d = tmp("fifo");
        let f = d.join("out.png");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &f,
            rustix::fs::FileType::Fifo,
            Mode::from_bits_truncate(0o600),
            0,
        )
        .unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let path = f.clone();
        std::thread::spawn(move || {
            let _ = tx.send(validate_output(&path, 1_000_000));
        });
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("validate_output must not block on a FIFO");
        assert_eq!(got.unwrap_err(), ValidationError::NotRegularFile);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rejects_oversize_output() {
        let d = tmp("big");
        let f = d.join("t.png");
        let mut v = png_bytes();
        v.resize(5000, 0);
        fs::write(&f, v).unwrap();
        assert!(matches!(
            validate_output(&f, 1000).unwrap_err(),
            ValidationError::TooLarge(_)
        ));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rejects_non_png_content() {
        let d = tmp("magic");
        let f = d.join("t.png");
        fs::write(&f, b"<html>not an image</html>").unwrap();
        assert_eq!(
            validate_output(&f, 1_000_000).unwrap_err(),
            ValidationError::BadMagic
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rejects_empty_output() {
        let d = tmp("empty");
        let f = d.join("t.png");
        fs::write(&f, b"").unwrap();
        assert_eq!(
            validate_output(&f, 1000).unwrap_err(),
            ValidationError::Empty
        );
        fs::remove_dir_all(&d).ok();
    }
}
