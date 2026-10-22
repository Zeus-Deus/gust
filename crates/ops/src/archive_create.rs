//! Bounded parent-owned input streaming into an isolated archive writer.
use super::{
    io::{ArchiveError, ArchiveFormat, ArchivePassword, Extracted, PublicationObserver},
    jobs::ArchivePhase,
    protocol::{self, Member, WriteFrame, WriteReply, WriteRequest},
    stage,
};
use crate::anchor::{DirHandle, FileIdentity};
use gust_core::lister::CancelToken;
use rustix::fs::{Mode, OFlags};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};
const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
#[derive(Clone, PartialEq, Eq)]
struct Input {
    identity: FileIdentity,
    link: Option<Vec<u8>>,
}
type Inventory = BTreeMap<PathBuf, Input>;
pub(crate) struct CreateInput<'a> {
    pub directory: &'a Path,
    pub names: &'a [OsString],
    pub destination: &'a Path,
    pub format: ArchiveFormat,
    pub password: Option<&'a ArchivePassword>,
}
fn failed(message: &str) -> ArchiveError {
    ArchiveError::Failed(message.into())
}
fn check(cancel: &CancelToken, started: Instant) -> Result<(), ArchiveError> {
    if cancel.is_cancelled() {
        return Err(ArchiveError::Cancelled);
    }
    if started.elapsed() > Duration::from_secs(120) {
        return Err(ArchiveError::Limit(
            "Archive creation exceeded its two-minute deadline.".into(),
        ));
    }
    Ok(())
}
fn inventory(
    root: &DirHandle,
    names: &[OsString],
    cancel: &CancelToken,
    started: Instant,
) -> Result<Inventory, ArchiveError> {
    #[allow(clippy::too_many_arguments)] // One bounded descriptor walk, not a public API.
    fn visit(
        parent: &DirHandle,
        name: &OsStr,
        path: PathBuf,
        depth: usize,
        out: &mut Inventory,
        budget: &mut (u64, usize),
        cancel: &CancelToken,
        started: Instant,
    ) -> Result<(), ArchiveError> {
        check(cancel, started)?;
        if depth > 64
            || path.as_os_str().as_bytes().len() > protocol::MAX_PATH
            || out.len() >= protocol::MAX_HEADERS
        {
            return Err(ArchiveError::Limit(
                "Archive input exceeds member/path limits.".into(),
            ));
        }
        let identity = FileIdentity::from_stat(&parent.stat_child(name)?);
        let kind = identity.mode & 0o170000;
        if !matches!(kind, 0o100000 | 0o040000 | 0o120000) {
            return Err(failed("Special files cannot be archived."));
        }
        let link = if kind == 0o120000 {
            Some(
                rustix::fs::readlinkat(parent.as_fd(), name, Vec::new())?
                    .to_bytes()
                    .to_vec(),
            )
        } else {
            None
        };
        if link.as_ref().is_some_and(|s| s.len() > protocol::MAX_PATH) {
            return Err(ArchiveError::Limit(
                "Archive link target exceeds path limit.".into(),
            ));
        }
        budget.0 = budget
            .0
            .checked_add(if kind == 0o100000 { identity.size } else { 0 })
            .ok_or_else(|| failed("Archive input size overflow."))?;
        budget.1 = budget.1.saturating_add(
            path.as_os_str().as_bytes().len() + link.as_ref().map_or(0, Vec::len) + 128,
        );
        if budget.0 > MAX_BYTES || budget.1 > protocol::MAX_METADATA {
            return Err(ArchiveError::Limit(
                "Archive input exceeds byte/metadata limits.".into(),
            ));
        }
        if out.insert(path.clone(), Input { identity, link }).is_some() {
            return Err(failed("Archive selection contains duplicate names."));
        }
        if kind == 0o040000 {
            let dir = parent.open_child_dir(name)?;
            if !identity.matches(&FileIdentity::from_stat(&dir.stat_self()?)) {
                return Err(failed("Archive input directory changed."));
            }
            for entry in rustix::fs::Dir::read_from(dir.as_fd())? {
                let entry = entry?;
                let bytes = entry.file_name().to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                let child = OsStr::from_bytes(bytes);
                visit(
                    &dir,
                    child,
                    path.join(child),
                    depth + 1,
                    out,
                    budget,
                    cancel,
                    started,
                )?;
            }
            if !identity.matches(&FileIdentity::from_stat(&dir.stat_self()?)) {
                return Err(failed(
                    "Archive input directory changed during enumeration.",
                ));
            }
        }
        if !identity.matches(&FileIdentity::from_stat(&parent.stat_child(name)?)) {
            return Err(failed("Archive input changed during enumeration."));
        }
        Ok(())
    }
    let mut out = Inventory::new();
    let mut budget = (0, 0);
    for name in names {
        crate::recursive::validate_name(name).map_err(|e| ArchiveError::Failed(e.to_string()))?;
        visit(
            root,
            name,
            PathBuf::from(name),
            1,
            &mut out,
            &mut budget,
            cancel,
            started,
        )?;
    }
    Ok(out)
}

pub(crate) fn create_recorded(
    input: CreateInput<'_>,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(ArchivePhase, usize, u64),
    observer: &mut dyn PublicationObserver,
) -> Result<Extracted, ArchiveError> {
    let started = Instant::now();
    check(cancel, started)?;
    if input.names.is_empty() {
        return Err(failed("Choose at least one item to archive."));
    }
    if input.password.is_some() && input.format != ArchiveFormat::Zip {
        return Err(ArchiveError::Unsupported(
            "password creation for this format".into(),
        ));
    }
    let root = stage::open_dir(input.directory)?;
    for name in input.names {
        crate::recursive::validate_name(name).map_err(|e| ArchiveError::Failed(e.to_string()))?;
        if input.destination.starts_with(input.directory.join(name)) {
            return Err(failed(
                "Archive destination cannot be inside a selected input.",
            ));
        }
    }
    let sources = inventory(&root, input.names, cancel, started)?;
    for (path, source) in &sources {
        if input.format == ArchiveFormat::SevenZip
            && (std::str::from_utf8(path.as_os_str().as_bytes()).is_err()
                || source
                    .link
                    .as_ref()
                    .is_some_and(|target| std::str::from_utf8(target).is_err()))
        {
            return Err(ArchiveError::UnsupportedEncoding {
                format: input.format,
                member: path.clone(),
            });
        }
        let seconds = source.identity.mtime;
        let nanoseconds = u32::try_from(source.identity.mtime_nsec).unwrap_or(u32::MAX);
        if !input
            .format
            .writer_format()
            .supports_timestamp(seconds, nanoseconds)
        {
            return Err(ArchiveError::UnsupportedTimestamp {
                format: input.format,
                member: path.clone(),
                seconds,
                nanoseconds,
            });
        }
    }
    let (parent, destination_name) = crate::anchor::resolve_parent(input.destination)?;
    match parent.stat_child(&destination_name) {
        Ok(_) => return Err(ArchiveError::DestinationExists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let parent_path = input
        .destination
        .parent()
        .ok_or_else(|| failed("Archive destination needs a parent."))?;
    let staging = stage::Stage::create(parent_path)?;
    let expected_parent = FileIdentity::from_stat(&parent.stat_self()?);
    let stage_parent = FileIdentity::from_stat(&staging.parent.stat_self()?);
    if expected_parent.dev != stage_parent.dev || expected_parent.ino != stage_parent.ino {
        return Err(failed("Archive destination folder changed."));
    }
    let output_name = OsStr::new("archive");
    let output = std::fs::File::from(rustix::fs::openat(
        staging.dir.as_fd(),
        output_name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    let mut writer = Writer::open(&output, input.format, input.password, cancel, started)?;
    let mut bytes_done = 0u64;
    for (index, (path, source)) in sources.iter().enumerate() {
        check(cancel, started)?;
        let (parent_source, _) =
            stage::parents(&root, path.parent().unwrap_or(Path::new("")), false)?;
        let name = path
            .file_name()
            .ok_or_else(|| failed("Invalid archive input name."))?;
        let current = FileIdentity::from_stat(&parent_source.stat_child(name)?);
        if !source.identity.matches(&current) {
            return Err(failed("Archive input changed; nothing was published."));
        }
        let regular = source.identity.mode & 0o170000 == 0o100000;
        writer.send(
            &WriteFrame::Header(Member {
                name: path.as_os_str().as_bytes().to_vec(),
                mode: source.identity.mode,
                size: if regular { source.identity.size } else { 0 },
                link: source.link.clone(),
                hardlink: false,
                mtime: Some((source.identity.mtime, source.identity.mtime_nsec as u32)),
            }),
            cancel,
        )?;
        if regular {
            let fd = rustix::fs::openat(
                parent_source.as_fd(),
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            let mut file = std::fs::File::from(fd);
            if !source
                .identity
                .matches(&FileIdentity::from_stat(&rustix::fs::fstat(&file)?))
            {
                return Err(failed("Archive input identity changed."));
            }
            let mut buffer = [0; protocol::DATA_CHUNK];
            let mut read = 0u64;
            loop {
                check(cancel, started)?;
                let count = match file.read(&mut buffer) {
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    other => other?,
                };
                if count == 0 {
                    break;
                }
                read = read
                    .checked_add(count as u64)
                    .ok_or_else(|| failed("Archive source length overflow."))?;
                if read > source.identity.size {
                    return Err(failed("Archive input grew while it was read."));
                }
                writer.send(&WriteFrame::Data(buffer[..count].to_vec()), cancel)?;
                bytes_done += count as u64;
                progress(ArchivePhase::Creating, index + 1, bytes_done);
            }
            if read != source.identity.size
                || !source
                    .identity
                    .matches(&FileIdentity::from_stat(&rustix::fs::fstat(&file)?))
            {
                return Err(failed(
                    "Archive input was incomplete or changed; nothing was published.",
                ));
            }
        }
        writer.send(&WriteFrame::EndMember, cancel)?;
        progress(ArchivePhase::Creating, index + 1, bytes_done);
    }
    writer.finish(cancel)?;
    if sources != inventory(&root, input.names, cancel, started)? {
        return Err(failed("Archive selection changed; nothing was published."));
    }
    output.sync_all()?;
    let staged_path = parent_path.join(&staging.name).join(output_name);
    let actual = super::io::enumerate(
        &staged_path,
        &super::ExtractPolicy {
            max_entries: protocol::MAX_HEADERS,
            max_total_bytes: MAX_BYTES,
            ..Default::default()
        },
        cancel,
    )?;
    if actual.len() != sources.len() {
        return Err(failed("Created archive inventory is incomplete."));
    }
    let mut seen = std::collections::BTreeSet::new();
    for member in actual {
        let path = super::io::member_path(&member.name)
            .ok_or_else(|| failed("Created archive has an invalid member."))?;
        let expected = sources
            .get(&path)
            .ok_or_else(|| failed("Created archive has an unexpected member."))?;
        let kind = expected.identity.mode & 0o170000;
        let expected_nsec = match input.format {
            ArchiveFormat::TarZstd => expected.identity.mtime_nsec as u32,
            ArchiveFormat::Zip => 0,
            ArchiveFormat::SevenZip => (expected.identity.mtime_nsec as u32 / 100) * 100,
        };
        if member.mtime != Some((expected.identity.mtime, expected_nsec)) {
            return Err(failed(
                "Created archive did not preserve supported timestamp precision.",
            ));
        }
        if !seen.insert(path)
            || member.mode & 0o170000 != kind
            || (kind == 0o100000 && member.size != expected.identity.size)
            || member.link != expected.link
        {
            return Err(failed(
                "Created archive inventory does not match its inputs.",
            ));
        }
    }
    check(cancel, started)?;
    let current_parent = FileIdentity::from_stat(&stage::open_dir(parent_path)?.stat_self()?);
    if expected_parent.dev != current_parent.dev || expected_parent.ino != current_parent.ino {
        return Err(failed("Archive destination folder changed."));
    }
    let expected = FileIdentity::from_stat(&rustix::fs::fstat(&output)?);
    let ticket = observer.intent(&staged_path, input.destination, Some(expected))?;
    progress(ArchivePhase::Publishing, sources.len(), bytes_done);
    check(cancel, started)?;
    crate::anchor::rename_noreplace(&staging.dir, output_name, &parent, &destination_name)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                ArchiveError::DestinationExists
            } else {
                e.into()
            }
        })?;
    let receipt = Extracted {
        files: 1,
        published: vec![(input.destination.to_path_buf(), expected.into())],
        ..Default::default()
    };
    let committed = (|| -> Result<(), ArchiveError> {
        if !expected.matches(&FileIdentity::from_stat(
            &parent.stat_child(&destination_name)?,
        )) {
            return Err(failed(
                "Created archive changed during publication; all work was preserved.",
            ));
        }
        observer.committed(ticket, expected)?;
        rustix::fs::fsync(parent.as_fd())?;
        staging.cleanup()?;
        Ok(())
    })();
    if let Err(cause) = committed {
        return Err(ArchiveError::Partial {
            receipt,
            cause: Box::new(cause),
        });
    }
    Ok(receipt)
}
struct Writer {
    child: Option<Child>,
    input: ChildStdin,
    output: ChildStdout,
    started: Instant,
}
impl Writer {
    fn open(
        output: &std::fs::File,
        format: ArchiveFormat,
        password: Option<&ArchivePassword>,
        cancel: &CancelToken,
        started: Instant,
    ) -> Result<Self, ArchiveError> {
        let helper = super::io::helper_path().ok_or(ArchiveError::NoTool)?;
        if !Path::new("/usr/bin/bwrap").is_file() || !Path::new("/usr/bin/prlimit").is_file() {
            return Err(ArchiveError::SandboxUnavailable);
        }
        let inherited = output.try_clone()?;
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .args([
                "--unshare-all",
                "--die-with-parent",
                "--new-session",
                "--clearenv",
                "--setenv",
                "PATH",
                "/usr/bin",
                "--setenv",
                "LC_ALL",
                "C.UTF-8",
                "--ro-bind",
                "/usr",
                "/usr",
                "--symlink",
                "usr/lib",
                "/lib",
                "--symlink",
                "usr/lib",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--size",
                "268435456",
                "--tmpfs",
                "/tmp",
                "--bind-fd",
            ])
            .arg(inherited.as_raw_fd().to_string())
            .arg("/output")
            .arg("--ro-bind")
            .arg(helper)
            .arg("/helper")
            .args([
                "--chdir",
                "/",
                "/usr/bin/prlimit",
                "--cpu=60",
                "--as=536870912",
                "--nproc=16",
                "--nofile=64",
                "--fsize=18253611008",
                "--",
                "/helper",
                "--create",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        use std::os::unix::process::CommandExt;
        // SAFETY: the post-fork closure only calls async-signal-safe fcntl on
        // its independently owned fd. No parent descriptor flags change.
        unsafe {
            command.pre_exec(move || {
                rustix::io::fcntl_setfd(&inherited, rustix::io::FdFlags::empty())
                    .map_err(std::io::Error::from)
            });
        }
        let mut child = command
            .spawn()
            .map_err(|_| ArchiveError::SandboxUnavailable)?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| failed("Archive writer stdin unavailable."))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| failed("Archive writer output unavailable."))?;
        rustix::fs::fcntl_setfl(&input, rustix::fs::fcntl_getfl(&input)? | OFlags::NONBLOCK)?;
        rustix::fs::fcntl_setfl(
            &output,
            rustix::fs::fcntl_getfl(&output)? | OFlags::NONBLOCK,
        )?;
        let mut writer = Self {
            child: Some(child),
            input,
            output,
            started,
        };
        let mut request = WriteRequest {
            format: format.writer_format(),
            password: password.map(|s| s.bytes().to_vec()).unwrap_or_default(),
        };
        let mut bytes = serde_json::to_vec(&request).map_err(std::io::Error::from)?;
        protocol::wipe(&mut request.password);
        let result = writer.write_message(&bytes, cancel);
        protocol::wipe(&mut bytes);
        result?;
        if !matches!(writer.reply(cancel)?, WriteReply::Ready) {
            return Err(failed("Archive writer could not start with these options."));
        }
        Ok(writer)
    }
    fn write_all(&mut self, mut bytes: &[u8], cancel: &CancelToken) -> Result<(), ArchiveError> {
        while !bytes.is_empty() {
            check(cancel, self.started)?;
            match self.input.write(bytes) {
                Ok(0) => return Err(failed("Archive writer closed its input.")),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_micros(250))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn write_message(&mut self, bytes: &[u8], cancel: &CancelToken) -> Result<(), ArchiveError> {
        self.write_all(&(bytes.len() as u32).to_le_bytes(), cancel)?;
        self.write_all(bytes, cancel)
    }
    fn read_all(&mut self, mut bytes: &mut [u8], cancel: &CancelToken) -> Result<(), ArchiveError> {
        while !bytes.is_empty() {
            check(cancel, self.started)?;
            match self.output.read(bytes) {
                Ok(0) => return Err(failed("Archive writer stopped before completion.")),
                Ok(n) => bytes = &mut bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_micros(250))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn reply(&mut self, cancel: &CancelToken) -> Result<WriteReply, ArchiveError> {
        let mut length = [0; 4];
        self.read_all(&mut length, cancel)?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 || length > protocol::MAX_FRAME {
            return Err(failed("Invalid archive writer reply."));
        }
        let mut bytes = vec![0; length];
        self.read_all(&mut bytes, cancel)?;
        serde_json::from_slice(&bytes).map_err(|_| failed("Invalid archive writer reply."))
    }
    fn send(&mut self, frame: &WriteFrame, cancel: &CancelToken) -> Result<(), ArchiveError> {
        let bytes = serde_json::to_vec(frame).map_err(std::io::Error::from)?;
        if bytes.len() > protocol::MAX_FRAME {
            return Err(ArchiveError::Limit(
                "Archive writer frame exceeds limit.".into(),
            ));
        }
        self.write_message(&bytes, cancel)?;
        if !matches!(self.reply(cancel)?, WriteReply::Ready) {
            return Err(failed("Archive writer refused an input member."));
        }
        Ok(())
    }
    fn finish(&mut self, cancel: &CancelToken) -> Result<(), ArchiveError> {
        let bytes = serde_json::to_vec(&WriteFrame::Finish).map_err(std::io::Error::from)?;
        self.write_message(&bytes, cancel)?;
        if !matches!(self.reply(cancel)?, WriteReply::Done) {
            return Err(failed("Archive writer could not finish."));
        }
        loop {
            check(cancel, self.started)?;
            if let Some(status) = self
                .child
                .as_mut()
                .ok_or_else(|| failed("Archive writer missing."))?
                .try_wait()?
            {
                return if status.success() {
                    Ok(())
                } else {
                    Err(failed("Archive writer failed."))
                };
            }
            std::thread::sleep(Duration::from_micros(250));
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = std::thread::Builder::new()
                    .name("gust-archive-writer-reap".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Job, JobKind, JobState, Journal, JournalUpdate};
    struct Observer<'a>(&'a Journal);
    impl PublicationObserver for Observer<'_> {
        fn intent(
            &mut self,
            source: &Path,
            destination: &Path,
            expected: Option<FileIdentity>,
        ) -> Result<usize, ArchiveError> {
            self.0.record_update(&JournalUpdate::ArchiveIntent {
                id: "cancel-publication".into(),
                index: 0,
                source: source.as_os_str().as_bytes().to_vec(),
                destination: destination.as_os_str().as_bytes().to_vec(),
                expected: expected.map(Into::into),
            })?;
            Ok(0)
        }
        fn committed(&mut self, _: usize, _: FileIdentity) -> Result<(), ArchiveError> {
            panic!("Cancelled publication must not commit")
        }
    }
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn native_writer_bounds_and_sub_tick_precision_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "gust-date-bounds-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let fixture = Fixture(root);
        let cancel = CancelToken::new();
        for (index, format) in [
            ArchiveFormat::TarZstd,
            ArchiveFormat::Zip,
            ArchiveFormat::SevenZip,
        ]
        .into_iter()
        .enumerate()
        {
            let (minimum, maximum) = format.creation_timestamp_range();
            let wire = format.writer_format();
            assert!(!wire.supports_timestamp(0, 1_000_000_000));
            assert!(!wire.supports_timestamp(minimum.0 - 1, 0));
            if maximum.1 < 999_999_999 {
                assert!(!wire.supports_timestamp(maximum.0, maximum.1 + 1));
            }
            if maximum.0 != i64::MAX {
                assert!(!wire.supports_timestamp(maximum.0 + 1, 0));
            }
            for (case, (seconds, nanoseconds)) in
                [minimum, maximum, (1, 123)].into_iter().enumerate()
            {
                let path = fixture.0.join(format!("{index}-{case}.archive"));
                let output = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .unwrap();
                let mut writer =
                    Writer::open(&output, format, None, &cancel, Instant::now()).unwrap();
                writer
                    .send(
                        &WriteFrame::Header(Member {
                            name: b"member".to_vec(),
                            mode: 0o100600,
                            size: 0,
                            link: None,
                            hardlink: false,
                            mtime: Some((seconds, nanoseconds)),
                        }),
                        &cancel,
                    )
                    .unwrap();
                writer.send(&WriteFrame::EndMember, &cancel).unwrap();
                writer.finish(&cancel).unwrap();
                let read =
                    super::super::io::enumerate(&path, &Default::default(), &cancel).unwrap();
                let expected_nsec = match format {
                    ArchiveFormat::TarZstd => nanoseconds,
                    ArchiveFormat::Zip => 0,
                    ArchiveFormat::SevenZip => nanoseconds / 100 * 100,
                };
                assert_eq!(read.len(), 1);
                assert_eq!(read[0].mtime, Some((seconds, expected_nsec)), "{format:?}");
            }
            let path = fixture.0.join(format!("{index}-invalid.archive"));
            let output = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap();
            let mut writer = Writer::open(&output, format, None, &cancel, Instant::now()).unwrap();
            assert!(
                writer
                    .send(
                        &WriteFrame::Header(Member {
                            name: b"member".to_vec(),
                            mode: 0o100600,
                            size: 0,
                            link: None,
                            hardlink: false,
                            mtime: Some((minimum.0 - 1, 0))
                        }),
                        &cancel
                    )
                    .is_err(),
                "helper must reject unsupported metadata before accepting payload"
            );
        }
    }
    #[test]
    fn cancellation_after_a_durable_intent_leaves_no_public_output() {
        let root = std::env::temp_dir().join(format!(
            "gust-create-intent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("source")).unwrap();
        let fixture = Fixture(root);
        std::fs::write(fixture.0.join("source/file"), b"preserve").unwrap();
        let destination = fixture.0.join("new.zip");
        let journal = Journal::new(fixture.0.join("journal"));
        journal
            .record(&Job::new("cancel-publication", JobKind::Compress))
            .unwrap();
        let cancel = CancelToken::new();
        let mut seen = false;
        let result = create_recorded(
            CreateInput {
                directory: &fixture.0.join("source"),
                names: &["file".into()],
                destination: &destination,
                format: ArchiveFormat::Zip,
                password: None,
            },
            &cancel,
            &mut |phase, _, _| {
                if phase == ArchivePhase::Publishing {
                    let latest = journal.load();
                    let pending = latest.last().unwrap();
                    assert_eq!(pending.state, JobState::Committing);
                    assert_eq!(pending.entries.len(), 1);
                    assert!(!pending.entries[0].done);
                    assert!(!destination.exists());
                    seen = true;
                    cancel.cancel();
                }
            },
            &mut Observer(&journal),
        );
        assert!(seen);
        assert_eq!(result, Err(ArchiveError::Cancelled));
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read(fixture.0.join("source/file")).unwrap(),
            b"preserve"
        );
        assert!(!journal.load().last().unwrap().entries[0].done);
        assert!(!std::fs::read_dir(&fixture.0).unwrap().flatten().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with(".gust-extract-")));
    }
}
