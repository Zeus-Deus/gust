//! Identity capture before public-name readback.
use crate::anchor::{self, DirHandle, FileIdentity};
use rustix::fs::{AtFlags, Mode};
use std::{
    ffi::{OsStr, OsString},
    io,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) enum Kind<'a> {
    Folder,
    Link(&'a std::path::Path),
}

/// mkdirat/symlinkat return no owned descriptor. Capture their identity in
/// an exclusive private namespace before publishing the requested name.
pub(crate) fn publish(
    parent: &DirHandle,
    name: &OsStr,
    kind: Kind<'_>,
) -> io::Result<FileIdentity> {
    let mut stage = Private::new(parent)?;
    let item = OsStr::new("item");
    let identity = match kind {
        Kind::Folder => {
            rustix::fs::mkdirat(stage.dir.as_fd(), item, Mode::from_bits_truncate(0o777))?;
            let owned = stage.dir.open_child_dir(item)?;
            let identity = FileIdentity::from_stat(&owned.stat_self()?);
            rustix::fs::fsync(owned.as_fd())?;
            identity
        }
        Kind::Link(target) => {
            rustix::fs::symlinkat(target, stage.dir.as_fd(), item)?;
            FileIdentity::from_stat(&stage.dir.stat_child(item)?)
        }
    };
    stage.item = Some(identity);
    rustix::fs::fsync(stage.dir.as_fd())?;
    anchor::rename_noreplace(&stage.dir, item, parent, name)?;
    stage.item = None;
    #[cfg(test)]
    after_publication();
    let check = || -> io::Result<()> {
        let now = FileIdentity::from_stat(&parent.stat_child(name)?);
        if !identity.matches(&now) || identity.mode != now.mode {
            return Err(io::Error::other(
                "The new item changed after publication; current work was preserved.",
            ));
        }
        Ok(())
    };
    check()?;
    if identity.mode & 0o170000 == 0o040000 {
        let owned = parent.open_child_dir(name)?;
        if !identity.matches(&FileIdentity::from_stat(&owned.stat_self()?)) {
            return Err(io::Error::other(
                "The new folder changed; current work was preserved.",
            ));
        }
        for entry in rustix::fs::Dir::read_from(owned.as_fd())? {
            if !matches!(entry?.file_name().to_bytes(), b"." | b"..") {
                return Err(io::Error::other(
                    "The new folder contains new work; it was preserved.",
                ));
            }
        }
        check()?;
    }
    rustix::fs::fsync(parent.as_fd())?;
    Ok(identity)
}

struct Private<'a> {
    parent: &'a DirHandle,
    dir: DirHandle,
    name: OsString,
    identity: FileIdentity,
    item: Option<FileIdentity>,
}
impl<'a> Private<'a> {
    fn new(parent: &'a DirHandle) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..64 {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let name = OsString::from(format!(
                ".gust-create-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match rustix::fs::mkdirat(parent.as_fd(), &name, Mode::from_bits_truncate(0o700)) {
                Ok(()) => {
                    let first = FileIdentity::from_stat(&parent.stat_child(&name)?);
                    let dir = parent.open_child_dir(&name)?;
                    let identity = FileIdentity::from_stat(&dir.stat_self()?);
                    if !first.matches(&identity) || identity.mode & 0o777 != 0o700 {
                        return Err(io::Error::other(
                            "Private creation staging changed; inspect the staging directory.",
                        ));
                    }
                    return Ok(Self {
                        parent,
                        dir,
                        name,
                        identity,
                        item: None,
                    });
                }
                Err(rustix::io::Errno::EXIST) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(io::Error::other("Cannot reserve private creation staging."))
    }
    fn cleanup(&self) -> io::Result<()> {
        let root = FileIdentity::from_stat(&self.parent.stat_child(&self.name)?);
        if root.dev != self.identity.dev || root.ino != self.identity.ino {
            return Err(io::Error::other("Creation staging ownership changed."));
        }
        if let Some(expected) = self.item {
            let item = OsStr::new("item");
            let now = FileIdentity::from_stat(&self.dir.stat_child(item)?);
            if !expected.matches(&now) || expected.mode != now.mode {
                return Err(io::Error::other("Creation staging contains changed work."));
            }
            // Only an unchanged link or empty folder; never recurse into new work.
            rustix::fs::unlinkat(
                self.dir.as_fd(),
                item,
                if now.mode & 0o170000 == 0o040000 {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                },
            )?;
        }
        rustix::fs::unlinkat(self.parent.as_fd(), &self.name, AtFlags::REMOVEDIR)?;
        rustix::fs::fsync(self.parent.as_fd())?;
        Ok(())
    }
}
impl Drop for Private<'_> {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn after_publication() {
    AFTER_PUBLICATION.with(|cell| {
        if let Some(run) = cell.borrow_mut().take() {
            run();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, path::PathBuf};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gust-create-race-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
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
    fn a_folder_replaced_before_public_readback_is_never_adopted() {
        let f = Fixture::new();
        let path = f.0.join("folder");
        let saved = f.0.join("saved");
        AFTER_PUBLICATION.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, saved).unwrap();
                std::fs::create_dir(path).unwrap();
            }))
        });
        assert!(crate::create::new_folder_recorded(&f.0, OsStr::new("folder")).is_err());
        assert!(f.0.join("folder").is_dir());
        assert!(f.0.join("saved").is_dir());
    }
    #[test]
    fn a_same_target_replacement_link_is_never_adopted() {
        let f = Fixture::new();
        let path = f.0.join("link");
        let saved = f.0.join("saved");
        let target = f.0.join("target");
        let replaced_target = target.clone();
        AFTER_PUBLICATION.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, saved).unwrap();
                std::os::unix::fs::symlink(replaced_target, path).unwrap();
            }))
        });
        assert!(crate::attrs::create_link(&target, &f.0, OsStr::new("link")).is_err());
        assert_eq!(std::fs::read_link(f.0.join("link")).unwrap(), target);
        assert!(f.0.join("saved").symlink_metadata().is_ok());
    }
    #[test]
    fn a_same_size_replacement_file_is_never_adopted() {
        let f = Fixture::new();
        let path = f.0.join("file");
        let saved = f.0.join("saved");
        AFTER_PUBLICATION.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, saved).unwrap();
                std::fs::write(path, b"later!!!").unwrap();
            }))
        });
        assert!(crate::create::create_from_bytes(&f.0, OsStr::new("file"), b"original").is_err());
        assert_eq!(std::fs::read(f.0.join("file")).unwrap(), b"later!!!");
        assert_eq!(std::fs::read(f.0.join("saved")).unwrap(), b"original");
    }
}
