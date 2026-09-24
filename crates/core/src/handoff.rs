//! Tab detach handoff: the state a tab carries into its new window, and
//! the readiness handshake that decides when the old tab may close.
//!
//! The handoff is a private file (0600) in the runtime directory, passed by
//! path. Binary-safe length-prefixed records, so any file name survives.
//! A damaged file is refused whole — the new window then opens at the
//! location alone rather than with half a state.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub location: PathBuf,
    pub view: String,
    pub cursor_name: Option<OsString>,
    pub selected: Vec<OsString>,
    /// Back history, oldest first.
    pub back: Vec<PathBuf>,
    pub show_hidden: bool,
}

const MAGIC: &[u8] = b"GUSTTAB1";

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self) -> Option<&'a [u8]> {
        if self.0.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes(self.0[..4].try_into().ok()?) as usize;
        if self.0.len() < 4 + n {
            return None;
        }
        let v = &self.0[4..4 + n];
        self.0 = &self.0[4 + n..];
        Some(v)
    }
    fn count(&mut self) -> Option<usize> {
        let b = self.take()?;
        Some(u32::from_le_bytes(b.try_into().ok()?) as usize)
    }
}

fn os(b: &[u8]) -> OsString {
    OsString::from_vec(b.to_vec())
}

impl Handoff {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        put(&mut out, self.location.as_os_str().as_bytes());
        put(&mut out, self.view.as_bytes());
        put(&mut out, self.cursor_name.as_deref().map(|c| c.as_bytes()).unwrap_or(b""));
        put(&mut out, &[self.cursor_name.is_some() as u8]);
        put(&mut out, &[self.show_hidden as u8]);
        put(&mut out, &(self.selected.len() as u32).to_le_bytes());
        for s in &self.selected {
            put(&mut out, s.as_bytes());
        }
        put(&mut out, &(self.back.len() as u32).to_le_bytes());
        for b in &self.back {
            put(&mut out, b.as_os_str().as_bytes());
        }
        put(&mut out, b"end");
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Handoff> {
        let rest = bytes.strip_prefix(MAGIC)?;
        let mut r = Reader(rest);
        let location = PathBuf::from(os(r.take()?));
        let view = String::from_utf8(r.take()?.to_vec()).ok()?;
        let cursor = r.take()?;
        let has_cursor = r.take()? == [1];
        let show_hidden = r.take()? == [1];
        let n = r.count()?;
        let mut selected = Vec::with_capacity(n.min(100_000));
        for _ in 0..n {
            selected.push(os(r.take()?));
        }
        let n = r.count()?;
        let mut back = Vec::with_capacity(n.min(10_000));
        for _ in 0..n {
            back.push(PathBuf::from(os(r.take()?)));
        }
        if r.take()? != b"end" || !r.0.is_empty() || !location.is_absolute() {
            return None;
        }
        Some(Handoff {
            location,
            view,
            cursor_name: has_cursor.then(|| os(cursor)),
            selected,
            back,
            show_hidden,
        })
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(&self.encode())
    }

    /// Read and delete: a handoff is used once.
    pub fn take(path: &Path) -> Option<Handoff> {
        let bytes = std::fs::read(path).ok()?;
        let _ = std::fs::remove_file(path);
        Handoff::decode(&bytes)
    }
}

/// The parent's side of the handshake.
pub struct ReadyWait {
    ready: PathBuf,
    deadline: Instant,
}

impl ReadyWait {
    pub fn new(ready: PathBuf, timeout: Duration) -> ReadyWait {
        ReadyWait { ready, deadline: Instant::now() + timeout }
    }

    /// Ok(true): the new window is up. Ok(false): not yet, ask again.
    /// Err: it died or never appeared — keep the tab.
    pub fn poll(&self, child: &mut std::process::Child) -> Result<bool, String> {
        if self.ready.exists() {
            let _ = std::fs::remove_file(&self.ready);
            return Ok(true);
        }
        match child.try_wait() {
            Ok(Some(status)) => Err(format!("the new window closed before it opened ({status})")),
            Err(e) => Err(e.to_string()),
            Ok(None) if Instant::now() >= self.deadline => {
                Err("the new window did not open in time".into())
            }
            Ok(None) => Ok(false),
        }
    }
}
