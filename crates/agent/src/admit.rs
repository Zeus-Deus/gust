//! What an agent may send, and where on a device it may put it.
//!
//! The agent already has a shell, so these rules are a guardrail against a
//! prompt-injected request, not a security boundary: they keep keys, config
//! and dotfiles from being handed to a phone by mistake.

use std::ffi::OsStr;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// At most this many files per call.
pub const MAX_PATHS: usize = 64;

/// A source file that passed admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Why a source was refused, in words the agent can repeat to a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub path: String,
    pub reason: &'static str,
}

/// The folders sources may come from: the home folder, removable drives
/// mounted for this user, and the temporary folder.
pub fn source_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if home.is_absolute() && home != Path::new("/") {
            roots.push(home);
        }
    }
    if let Some(user) = std::env::var_os("USER") {
        roots.push(Path::new("/run/media").join(user));
    }
    let tmp = std::env::temp_dir();
    if tmp.is_absolute() && tmp != Path::new("/") {
        roots.push(tmp);
    }
    roots
}

/// Check one requested path against `roots`. Reads only the path itself.
pub fn source(raw: &str, roots: &[PathBuf]) -> Result<Admitted, Refusal> {
    let refuse = |reason| Refusal {
        path: raw.to_string(),
        reason,
    };
    let path = Path::new(raw);
    if !path.is_absolute() || raw.contains('\0') {
        return Err(refuse(
            "Give an absolute path, such as /home/you/Pictures/photo.jpg.",
        ));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(refuse("The path may not contain \".\" or \"..\"."));
    }
    let Some(rest) = roots.iter().find_map(|r| path.strip_prefix(r).ok()) else {
        return Err(refuse(
            "Only files in the home folder, on a removable drive or in the temporary folder can be sent.",
        ));
    };
    if rest
        .components()
        .any(|c| matches!(c, Component::Normal(n) if n.as_bytes().starts_with(b".")))
    {
        return Err(refuse(
            "Hidden files and files in hidden folders (such as ~/.ssh or ~/.config) are never sent.",
        ));
    }
    if gust_remote::send::check_source_location(path).is_err() {
        return Err(refuse(
            "Only files on this computer's own disks can be sent, not files on a network or phone mount.",
        ));
    }
    let file = open_no_symlinks(path).map_err(|_| {
        refuse("The file does not exist, cannot be read, or its path goes through a symbolic link.")
    })?;
    let meta = file
        .metadata()
        .map_err(|_| refuse("The file cannot be read."))?;
    if !meta.is_file() {
        return Err(refuse(
            "Only regular files can be sent, not folders or devices.",
        ));
    }
    Ok(Admitted {
        path: path.to_path_buf(),
        bytes: meta.len(),
    })
}

fn open_no_symlinks(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let fd = rustix::fs::openat2(
        rustix::fs::CWD,
        path,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )?;
    Ok(File::from(fd))
}

/// A folder on a device, relative to the place the device is open at.
/// Returns the absolute folder once it is known to be a real folder beneath
/// `root`, reached without following any symbolic link.
pub fn device_folder(root: &Path, raw: &str) -> Result<PathBuf, &'static str> {
    let relative = Path::new(raw.trim_matches('/'));
    if raw.contains('\0') || raw.len() > 4096 {
        return Err("That folder name is not valid.");
    }
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err("Give the folder as a path inside the device, without \".\" or \"..\".");
    }
    if relative.as_os_str().is_empty() {
        return Ok(root.to_path_buf());
    }
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let dir = rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "The device's folder could not be opened.")?;
    rustix::fs::openat2(
        &dir,
        relative,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|_| "That folder does not exist on the device. List its folders first.")?;
    Ok(root.join(relative))
}

/// The first component of a relative device folder, if any.
pub fn first_component(raw: &str) -> Option<&OsStr> {
    Path::new(raw.trim_matches('/'))
        .components()
        .next()
        .map(|c| c.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("gust-agent-admit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_ordinary_file_in_an_allowed_folder_is_admitted_with_its_size() {
        let root = scratch("ok");
        std::fs::create_dir(root.join("Pictures")).unwrap();
        std::fs::write(root.join("Pictures/photo.jpg"), b"12345").unwrap();
        let got = source(
            root.join("Pictures/photo.jpg").to_str().unwrap(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert_eq!(got.bytes, 5);
    }

    #[test]
    fn hidden_paths_relative_paths_and_outside_files_are_refused() {
        let root = scratch("refuse");
        std::fs::create_dir(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/id_ed25519"), b"k").unwrap();
        std::fs::write(root.join(".env"), b"k").unwrap();
        let roots = std::slice::from_ref(&root);
        for p in [
            root.join(".ssh/id_ed25519"),
            root.join(".env"),
            root.join("a/../.env"),
            PathBuf::from("relative.txt"),
            PathBuf::from("/etc/hostname"),
        ] {
            assert!(
                source(p.to_str().unwrap(), roots).is_err(),
                "{p:?} admitted"
            );
        }
    }

    #[test]
    fn folders_and_symlinked_paths_are_refused() {
        let root = scratch("links");
        std::fs::create_dir(root.join("real")).unwrap();
        std::fs::write(root.join("real/f.txt"), b"x").unwrap();
        symlink(root.join("real"), root.join("linkdir")).unwrap();
        symlink(root.join("real/f.txt"), root.join("link.txt")).unwrap();
        let roots = std::slice::from_ref(&root);
        assert!(source(root.join("real").to_str().unwrap(), roots).is_err());
        assert!(source(root.join("link.txt").to_str().unwrap(), roots).is_err());
        assert!(source(root.join("linkdir/f.txt").to_str().unwrap(), roots).is_err());
    }

    #[test]
    fn a_device_folder_must_exist_beneath_the_device_without_links() {
        let root = scratch("folder");
        std::fs::create_dir_all(root.join("VLC/Movies")).unwrap();
        let outside = scratch("folder-outside");
        symlink(&outside, root.join("escape")).unwrap();
        assert_eq!(
            device_folder(&root, "VLC/Movies").unwrap(),
            root.join("VLC/Movies")
        );
        assert_eq!(device_folder(&root, "/VLC/").unwrap(), root.join("VLC"));
        assert_eq!(device_folder(&root, "").unwrap(), root);
        assert!(device_folder(&root, "missing").is_err());
        assert!(device_folder(&root, "../x").is_err());
        assert!(device_folder(&root, "VLC/../VLC").is_err());
        assert!(device_folder(&root, "escape").is_err());
        assert_eq!(first_component("/VLC/Movies"), Some(OsStr::new("VLC")));
        assert_eq!(first_component(""), None);
    }
}
