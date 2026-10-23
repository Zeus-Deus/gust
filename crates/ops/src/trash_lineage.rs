//! Native Info coherence for revisions authorized by complete typed history.
use super::*;
use crate::journal::RecordedIdentity;
use serde::{Deserialize, Serialize};

const DIRECTORY: &str = ".gust-lineage";
const LIMIT: usize = 32 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    files: Vec<u8>,
    files_dev: u64,
    files_ino: u64,
    info: Vec<u8>,
    info_dev: u64,
    info_ino: u64,
    name: Vec<u8>,
    info_identity: RecordedIdentity,
    info_mode: u32,
    info_bytes: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Node {
    relative: Vec<u8>,
    identity: RecordedIdentity,
    mode: u32,
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Certificate {
    version: u32,
    binding: Binding,
    tree: Vec<Node>,
}
fn nodes(root: &Path, tree: &BTreeMap<PathBuf, FileIdentity>) -> io::Result<Vec<Node>> {
    tree.iter()
        .map(|(path, identity)| {
            Ok(Node {
                relative: path
                    .strip_prefix(root)
                    .map_err(|_| invalid("Invalid Trash lineage path"))?
                    .as_os_str()
                    .as_bytes()
                    .to_vec(),
                identity: (*identity).into(),
                mode: identity.mode,
            })
        })
        .collect()
}
fn key(info: RecordedIdentity, data: FileIdentity) -> String {
    format!(
        "{}-{}-{}-{}-{}-{}-{}",
        info.dev, info.ino, data.dev, data.ino, data.mtime, data.mtime_nsec, data.size
    )
}
fn read(dir: &DirHandle, name: &OsStr, allowance: usize) -> io::Result<Vec<u8>> {
    let mut file = File::from(rustix::fs::openat(
        dir.as_fd(),
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let before = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
    let stat = rustix::fs::fstat(&file)?;
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o170077 != 0o100000 {
        return Err(invalid(
            "Trash lineage must be an owned private regular record",
        ));
    }
    if before.size > allowance as u64 {
        return Err(invalid("Trash lineage metadata limit"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(allowance as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > allowance
        || before != FileIdentity::from_stat(&rustix::fs::fstat(&file)?)
        || before != FileIdentity::from_stat(&dir.stat_child(name)?)
    {
        return Err(invalid(
            "Trash lineage changed or exceeds its metadata limit",
        ));
    }
    Ok(bytes)
}
impl Binding {
    pub(crate) fn new(
        location: &TrashLocation,
        name: &OsStr,
        files: FileIdentity,
        info: FileIdentity,
        info_identity: FileIdentity,
        info_bytes: Vec<u8>,
    ) -> Self {
        Self {
            files: location.trash.files.as_os_str().as_bytes().to_vec(),
            files_dev: files.dev,
            files_ino: files.ino,
            info: location.trash.info.as_os_str().as_bytes().to_vec(),
            info_dev: info.dev,
            info_ino: info.ino,
            name: name.as_bytes().to_vec(),
            info_identity: info_identity.into(),
            info_mode: info_identity.mode,
            info_bytes,
        }
    }
    pub(crate) fn data_path(&self) -> PathBuf {
        Path::new(OsStr::from_bytes(&self.files)).join(OsStr::from_bytes(&self.name))
    }
    fn info_handle(&self) -> io::Result<DirHandle> {
        let info = history_tree::open_dir(Path::new(OsStr::from_bytes(&self.info)))?;
        let files = history_tree::open_dir(Path::new(OsStr::from_bytes(&self.files)))?;
        private(&info)?;
        private(&files)?;
        let ist = info.stat_self()?;
        let fst = files.stat_self()?;
        if (ist.st_dev, ist.st_ino) != (self.info_dev, self.info_ino)
            || (fst.st_dev, fst.st_ino) != (self.files_dev, self.files_ino)
        {
            return Err(invalid("Trash lineage directory was replaced"));
        }
        Ok(info)
    }
    pub(crate) fn publish(
        &self,
        tree: &BTreeMap<PathBuf, FileIdentity>,
        cancel: &CancelToken,
    ) -> io::Result<()> {
        check(cancel)?;
        let root = self.data_path();
        let data = *tree
            .get(&root)
            .ok_or_else(|| invalid("Missing Trash lineage root"))?;
        let location = TrashLocation {
            trash: TrashDir::at(
                Path::new(OsStr::from_bytes(&self.info))
                    .parent()
                    .ok_or_else(|| invalid("Missing Trash root"))?,
            ),
            topdir: None,
        };
        // Only the identity line matters here; original byte paths remain bound
        // in the unchanged Info bytes and by the typed receipt admission.
        let recorded = self
            .info_bytes
            .split(|b| *b == b'\n')
            .find_map(|line| line.strip_prefix(b"X-Gust-Identity="))
            .map(serde_json::from_slice::<RecordedIdentity>)
            .transpose()?;
        let Some(recorded) = recorded else {
            return Ok(());
        };
        if recorded.matches(&data) {
            return Ok(());
        }
        if data.mode & 0o170000 != 0o040000 || (data.dev, data.ino) != (recorded.dev, recorded.ino)
        {
            return Err(invalid("Trash lineage root is not the admitted directory"));
        }
        // Check the selected layout before any certificate creation as well.
        local_location(&location, &mounts()?)?;
        let info = self.info_handle()?;
        let (_, identity, bytes) = open_info(&info, &info_name(OsStr::from_bytes(&self.name))?)?;
        if !self.info_identity.matches(&identity)
            || identity.mode != self.info_mode
            || bytes != self.info_bytes
        {
            return Err(invalid("Trash lineage Info changed"));
        }
        match rustix::fs::mkdirat(info.as_fd(), DIRECTORY, Mode::from_bits_truncate(0o700)) {
            Ok(()) => rustix::fs::fsync(info.as_fd())?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let dir = info.open_child_dir(OsStr::new(DIRECTORY))?;
        private(&dir)?;
        let certificate = Certificate {
            version: 1,
            binding: self.clone(),
            tree: nodes(&root, tree)?,
        };
        let bytes = serde_json::to_vec(&certificate)?;
        if bytes.len() > LIMIT {
            return Err(invalid("Trash lineage metadata limit"));
        }
        let name = key(self.info_identity, data);
        match read(&dir, OsStr::new(&name), LIMIT) {
            Ok(existing) if existing == bytes => return Ok(()),
            Ok(_) => return Err(invalid("Trash lineage certificate is occupied or changed")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let (mut count, mut retained) = (0usize, 0u64);
        for entry in rustix::fs::Dir::read_from(dir.as_fd())? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            count += 1;
            retained = retained
                .saturating_add(dir.stat_child(OsStr::from_bytes(name))?.st_size.max(0) as u64);
            if count >= 4096 || retained.saturating_add(bytes.len() as u64) > LIMIT as u64 {
                return Err(invalid("Retained Trash lineage metadata limit"));
            }
        }
        check(cancel)?;
        // Exclusive creation never replaces or cleans an uncertain record. A
        // partial certificate is retained and cannot pass exact validation.
        let mut file = File::from(rustix::fs::openat(
            dir.as_fd(),
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        file.write_all(&bytes)?;
        file.sync_all()?;
        let held = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
        if held != FileIdentity::from_stat(&dir.stat_child(OsStr::new(&name))?)
            || read(&dir, OsStr::new(&name), LIMIT)? != bytes
        {
            return Err(invalid("Trash lineage changed during publication"));
        }
        rustix::fs::fsync(dir.as_fd())?;
        Ok(())
    }
    pub(super) fn matches(
        &self,
        info: &DirHandle,
        tree: &BTreeMap<PathBuf, FileIdentity>,
        metadata: &mut usize,
    ) -> io::Result<bool> {
        let root = self.data_path();
        let data = *tree
            .get(&root)
            .ok_or_else(|| invalid("Missing Trash lineage root"))?;
        let dir = match info.open_child_dir(OsStr::new(DIRECTORY)) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        private(&dir)?;
        let bytes = match read(
            &dir,
            OsStr::new(&key(self.info_identity, data)),
            LIMIT.saturating_sub(*metadata),
        ) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        *metadata = metadata.saturating_add(bytes.len());
        let certificate: Certificate = serde_json::from_slice(&bytes)?;
        Ok(certificate.version == 1
            && certificate.binding == *self
            && certificate.tree == nodes(&root, tree)?)
    }
}
