//! Diagnostic receipts, not frames. One process worker, one newest pending
//! document, no GUI filesystem access or shutdown join. Admission is not save.
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
const MAX_BYTES: usize = 2 * 1024 * 1024;
struct Document {
    path: PathBuf,
    contents: String,
    revision: u64,
}
#[derive(Default)]
struct State {
    pending: Option<Document>,
    next: u64,
    written: u64,
    failed: u64,
}
pub struct Writer {
    state: Arc<(Mutex<State>, Condvar)>,
}
impl Writer {
    fn new() -> Self {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = state.clone();
        std::thread::Builder::new()
            .name("gust-qa-state".into())
            .spawn(move || loop {
                let document = {
                    let mut state = worker.0.lock().unwrap();
                    while state.pending.is_none() {
                        state = worker.1.wait(state).unwrap();
                    }
                    state.pending.take().unwrap()
                };
                let result = write_atomic(&document);
                let mut state = worker.0.lock().unwrap();
                if result.is_ok() {
                    state.written = document.revision;
                } else {
                    state.failed += 1;
                }
                worker.1.notify_all();
            })
            .expect("QA receipt executor creation");
        Self { state }
    }
    pub fn admit(&self, path: PathBuf, contents: String) -> bool {
        if !allowed(&path) || contents.len() > MAX_BYTES {
            return false;
        }
        let mut state = self.state.0.lock().unwrap();
        state.next = state.next.wrapping_add(1);
        let revision = state.next;
        state.pending = Some(Document {
            path,
            contents,
            revision,
        });
        self.state.1.notify_one();
        true
    }
}
pub fn process() -> &'static Writer {
    static WRITER: OnceLock<Writer> = OnceLock::new();
    WRITER.get_or_init(Writer::new)
}
fn allowed(path: &Path) -> bool {
    path.file_name().is_some()
        && (path.starts_with("/tmp/") || path.starts_with("/var/tmp/"))
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
}
fn write_atomic(document: &Document) -> std::io::Result<()> {
    // Resolve the parent on this worker. A symlinked temp directory must not
    // turn diagnostic publication into a write under the user's home.
    let parent = document.path.parent().unwrap().canonicalize()?;
    if !allowed(&parent.join("receipt")) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "QA receipt parent is outside temporary storage",
        ));
    }
    let target = parent.join(document.path.file_name().unwrap());
    let staging = target.with_extension(format!(
        "partial.{}.{}",
        std::process::id(),
        document.revision
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    let result = (|| {
        file.write_all(document.contents.as_bytes())?;
        drop(file);
        std::fs::rename(&staging, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn admission_is_bounded_temp_only_and_latest_receipt_is_atomic() {
        let dir = std::env::temp_dir().join(format!("gust-qa-writer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let writer = Writer::new();
        let path = dir.join("state.json");
        assert!(!writer.admit(PathBuf::from("/home/user/state"), "x".into()));
        assert!(!writer.admit(PathBuf::from("/tmp/../home/state"), "x".into()));
        assert!(!writer.admit(path.clone(), "x".repeat(MAX_BYTES + 1)));
        for revision in 0..500 {
            assert!(writer.admit(path.clone(), format!("{{\"revision\":{revision}}}")));
        }
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let done = {
                let state = writer.state.0.lock().unwrap();
                state.written == state.next
            };
            if done {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"revision\":499}"
        );
        assert_eq!(writer.state.0.lock().unwrap().failed, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_symlinked_temp_parent_cannot_write_elsewhere() {
        let path = std::env::temp_dir().join(format!("gust-qa-writer-link-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        std::os::unix::fs::symlink("/dev", path.join("outside")).unwrap();
        let document = Document {
            path: path.join("outside/null"),
            contents: "no".into(),
            revision: 1,
        };
        assert!(allowed(&document.path));
        assert!(write_atomic(&document).is_err());
        std::fs::remove_dir_all(path).unwrap();
    }
}
