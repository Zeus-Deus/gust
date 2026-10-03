//! Creating new files: empty, or from the user's Templates folder.
//!
//! Both never replace anything. The file is created with `O_CREAT|O_EXCL`
//! (`create_new`), which also refuses a dangling symlink at the name, so a
//! race with another program creating the same name loses cleanly instead
//! of truncating its file.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A template is copied, never executed and never followed: bounded so a
/// huge or endless file (a FIFO planted in Templates) cannot hang Gust.
pub const TEMPLATE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Create an empty file `dir/name`. Errors are plain sentences.
pub fn new_empty_file(dir: &Path, name: &OsStr) -> Result<(), String> {
    create_with(dir, name, &[])
}

/// Create `dir/name` with the bytes of `template`. The template must be a
/// regular file (not a link, FIFO or device) no bigger than
/// [`TEMPLATE_MAX_BYTES`].
pub fn new_from_template(dir: &Path, name: &OsStr, template: &Path) -> Result<(), String> {
    let bytes = read_template(template)?;
    create_with(dir, name, &bytes)
}

fn read_template(template: &Path) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let md = template
        .symlink_metadata()
        .map_err(|_| "that template no longer exists".to_string())?;
    if !md.is_file() {
        return Err("that template is not an ordinary file".into());
    }
    if md.len() > TEMPLATE_MAX_BYTES {
        return Err("that template is too large to copy as a new file".into());
    }
    // O_NOFOLLOW|O_NONBLOCK: a link or FIFO swapped in after the check
    // above fails instead of being followed or blocking forever.
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc_flags::O_NOFOLLOW | libc_flags::O_NONBLOCK)
        .open(template)
        .map_err(|_| "that template could not be read".to_string())?;
    let held = f
        .metadata()
        .map_err(|_| "that template could not be read".to_string())?;
    if !held.is_file() {
        return Err("that template is not an ordinary file".into());
    }
    let mut out = Vec::new();
    Read::by_ref(&mut f)
        .take(TEMPLATE_MAX_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|_| "that template could not be read".to_string())?;
    if out.len() as u64 > TEMPLATE_MAX_BYTES {
        return Err("that template is too large to copy as a new file".into());
    }
    Ok(out)
}

fn create_with(dir: &Path, name: &OsStr, bytes: &[u8]) -> Result<(), String> {
    crate::recursive::validate_name(name).map_err(|e| format!("{e}"))?;
    let target = dir.join(name);
    let mut f = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err("a file with that name already exists".into())
        }
        Err(e) => return Err(crate::engine::plain_io_message(&e)),
    };
    if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
        // Our own half-written file, created a moment ago: never leave it.
        drop(f);
        let _ = std::fs::remove_file(&target);
        return Err(crate::engine::plain_io_message(&e));
    }
    Ok(())
}

/// The biggest image a paste will write (a screenshot of a 8K display as
/// PNG is ~60 MiB). Anything larger is refused rather than held in memory.
pub const PASTE_MAX_BYTES: usize = 128 * 1024 * 1024;

/// A file Gust created from pasted bytes, and what undo needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub ident: crate::attrs::Ident,
    pub len: u64,
}

/// "Pasted image 2026-10-03 16-24-05.png", then " (2)", " (3)"… in `dir`.
/// Never an existing name. `stamp` is local time, already formatted.
pub fn pasted_image_name(dir: &Path, stamp: &str, ext: &str) -> std::ffi::OsString {
    for i in 1..1000 {
        let cand = if i == 1 {
            format!("Pasted image {stamp}.{ext}")
        } else {
            format!("Pasted image {stamp} ({i}).{ext}")
        };
        if dir.join(&cand).symlink_metadata().is_err() {
            return cand.into();
        }
    }
    format!("Pasted image {stamp} ({}).{ext}", std::process::id()).into()
}

/// Create `dir/name` holding `bytes` (never replacing anything) and record
/// its identity so the undo can prove it is still that file.
pub fn create_from_bytes(dir: &Path, name: &OsStr, bytes: &[u8]) -> Result<Created, String> {
    if bytes.len() > PASTE_MAX_BYTES {
        return Err("that image is too large to paste as a file".into());
    }
    create_with(dir, name, bytes)?;
    let path = dir.join(name);
    let ident = crate::attrs::ident_of(&path).map_err(|e| crate::engine::plain_io_message(&e))?;
    Ok(Created {
        path,
        ident,
        len: bytes.len() as u64,
    })
}

/// Remove a file Gust created, only if it is still exactly that file
/// (same dev + inode + mtime and size). A file that was edited or replaced
/// since is the user's work and is left alone.
pub fn undo_create(c: &Created) -> Result<(), String> {
    let now =
        crate::attrs::ident_of(&c.path).map_err(|_| "the file is already gone".to_string())?;
    let len = c
        .path
        .symlink_metadata()
        .map(|m| m.len())
        .unwrap_or(u64::MAX);
    if now != c.ident || len != c.len {
        return Err("the file was changed since; left as it is".into());
    }
    std::fs::remove_file(&c.path).map_err(|e| crate::engine::plain_io_message(&e))
}

/// The user's Templates folder (XDG user-dirs), when it exists.
pub fn templates_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    let from_dirs = std::fs::read_to_string(cfg.join("user-dirs.dirs"))
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                let v = l.trim().strip_prefix("XDG_TEMPLATES_DIR=")?;
                let v = v.trim().trim_matches('"');
                Some(match v.strip_prefix("$HOME/") {
                    Some(tail) => PathBuf::from(&home).join(tail),
                    None => PathBuf::from(v),
                })
            })
        });
    let dir = from_dirs.unwrap_or_else(|| PathBuf::from(&home).join("Templates"));
    // $HOME itself is what an unset Templates entry often points at; that
    // is "no templates", not "every file in your home is a template".
    if dir == Path::new(&home) {
        return None;
    }
    dir.is_dir().then_some(dir)
}

/// Templates offered in "New file from template": regular, visible files
/// directly inside the folder, sorted, at most 50.
pub fn list_templates(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    out.sort();
    out.truncate(50);
    out
}

mod libc_flags {
    // Linux values; Gust is Linux-only (Omarchy).
    pub const O_NOFOLLOW: i32 = 0o400000;
    pub const O_NONBLOCK: i32 = 0o4000;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-create-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_empty_file_is_created() {
        let d = tmp("empty");
        new_empty_file(&d, OsStr::new("notes.txt")).unwrap();
        assert_eq!(fs::read(d.join("notes.txt")).unwrap(), b"");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_existing_file_is_never_replaced() {
        let d = tmp("clobber");
        fs::write(d.join("keep.txt"), b"precious").unwrap();
        let e = new_empty_file(&d, OsStr::new("keep.txt")).unwrap_err();
        assert!(e.contains("already exists"), "{e}");
        assert_eq!(fs::read(d.join("keep.txt")).unwrap(), b"precious");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_dangling_symlink_at_the_name_is_not_followed() {
        let d = tmp("link");
        let outside = d.join("outside-target");
        std::os::unix::fs::symlink(&outside, d.join("trap")).unwrap();
        assert!(new_empty_file(&d, OsStr::new("trap")).is_err());
        assert!(
            !outside.exists(),
            "creating through a link would write elsewhere"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_bad_name_is_refused() {
        let d = tmp("names");
        assert!(new_empty_file(&d, OsStr::new("a/b")).is_err());
        assert!(new_empty_file(&d, OsStr::new("..")).is_err());
        assert!(new_empty_file(&d, OsStr::new("")).is_err());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_template_is_copied_byte_for_byte() {
        let d = tmp("tpl");
        let t = d.join("Letter.odt");
        fs::write(&t, b"PK\x03\x04 template bytes").unwrap();
        new_from_template(&d, OsStr::new("Mine.odt"), &t).unwrap();
        assert_eq!(fs::read(d.join("Mine.odt")).unwrap(), fs::read(&t).unwrap());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_fifo_template_is_refused_without_hanging() {
        let d = tmp("fifo");
        let t = d.join("pipe");
        std::process::Command::new("mkfifo")
            .arg(&t)
            .status()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let dd = d.clone();
        std::thread::spawn(move || {
            let _ = tx.send(new_from_template(&dd, OsStr::new("x"), &t));
        });
        let r = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a FIFO template hung the copy");
        assert!(r.is_err());
        assert!(
            !d.join("x").exists(),
            "a refused template must leave no file"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlinked_template_is_refused() {
        let d = tmp("tpllink");
        fs::write(d.join("real"), b"secret").unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("link")).unwrap();
        assert!(new_from_template(&d, OsStr::new("x"), &d.join("link")).is_err());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn templates_lists_only_visible_regular_files() {
        let d = tmp("list");
        fs::write(d.join("b.md"), b"").unwrap();
        fs::write(d.join("a.txt"), b"").unwrap();
        fs::write(d.join(".hidden"), b"").unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        let names: Vec<String> = list_templates(&d)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.md"]);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_pasted_image_gets_a_fresh_name_and_its_bytes() {
        let d = tmp("pasteimg");
        let png = b"\x89PNG\r\n\x1a\nnot really decoded";
        let n1 = pasted_image_name(&d, "2026-10-03 16-24-05", "png");
        assert_eq!(n1, "Pasted image 2026-10-03 16-24-05.png");
        let c = create_from_bytes(&d, &n1, png).unwrap();
        assert_eq!(fs::read(&c.path).unwrap(), png);
        let n2 = pasted_image_name(&d, "2026-10-03 16-24-05", "png");
        assert_eq!(n2, "Pasted image 2026-10-03 16-24-05 (2).png");
        undo_create(&c).unwrap();
        assert!(!c.path.exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_of_a_paste_leaves_an_edited_file_alone() {
        let d = tmp("pasteedit");
        let c = create_from_bytes(&d, OsStr::new("p.png"), b"one").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&c.path, b"the user drew on it").unwrap();
        let e = undo_create(&c).unwrap_err();
        assert!(e.contains("left as it is"), "{e}");
        assert_eq!(fs::read(&c.path).unwrap(), b"the user drew on it");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_paste_never_replaces_an_existing_file() {
        let d = tmp("pasteclash");
        fs::write(d.join("p.png"), b"keep").unwrap();
        assert!(create_from_bytes(&d, OsStr::new("p.png"), b"new").is_err());
        assert_eq!(fs::read(d.join("p.png")).unwrap(), b"keep");
        fs::remove_dir_all(&d).ok();
    }
}
