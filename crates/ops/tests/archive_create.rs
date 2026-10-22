use gust_core::lister::CancelToken;
use gust_ops::{
    archive::{
        io::{self, ArchiveError, ArchiveFormat, ArchivePassword},
        jobs::{start_journalled, ArchivePhase, ArchiveRequest},
        ExtractPolicy,
    },
    journal::{JobKind, JobState, Journal},
};
use std::{ffi::OsString, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gust-archive-create-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("source/folder")).unwrap();
        std::fs::write(path.join("source/folder/file"), b"contents\n").unwrap();
        std::fs::write(path.join("source/-dash"), b"flag safe").unwrap();
        std::fs::write(path.join("source/line\nbreak"), b"byte safe").unwrap();
        Self(path)
    }
    fn names(&self) -> Vec<OsString> {
        vec!["folder".into(), "-dash".into(), "line\nbreak".into()]
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn all_formats_round_trip_complete_byte_names_and_journal_undo() {
    let f = Fixture::new();
    for (format, extension) in [
        (ArchiveFormat::TarZstd, "tar.zst"),
        (ArchiveFormat::Zip, "zip"),
        (ArchiveFormat::SevenZip, "7z"),
    ] {
        let destination = f.0.join(format!("made.{extension}"));
        let journal = Journal::new(f.0.join(format!("journal-{extension}")));
        let mut handle = start_journalled(
            extension,
            ArchiveRequest::Create {
                directory: f.0.join("source"),
                names: f.names(),
                destination: destination.clone(),
                format,
                password: None,
            },
            Journal::new(journal.path()),
        )
        .unwrap();
        let progress = handle.wait();
        assert_eq!(progress.phase, ArchivePhase::Confirmed, "{progress:?}");
        let result = progress.result.unwrap().unwrap();
        assert!(result.extracted.is_none());
        assert_eq!(result.record.kind, JobKind::Compress);
        assert_eq!(result.record.state, JobState::Confirmed);
        assert_eq!(result.record.entries.len(), 1);
        assert_eq!(journal.load().last().unwrap().kind, JobKind::Compress);
        let out = f.0.join(format!("out-{extension}"));
        std::fs::create_dir(&out).unwrap();
        io::extract(&destination, &out, &ExtractPolicy::default()).unwrap();
        assert_eq!(
            std::fs::read(out.join("folder/file")).unwrap(),
            b"contents\n"
        );
        assert_eq!(std::fs::read(out.join("-dash")).unwrap(), b"flag safe");
        assert_eq!(
            std::fs::read(out.join("line\nbreak")).unwrap(),
            b"byte safe"
        );
        assert!(
            gust_ops::undo::apply_undo(&gust_ops::undo::plan_undo(&result.record)).is_complete()
        );
        assert!(!destination.exists());
        assert!(f.0.join("source/folder/file").exists());
    }
}
#[test]
fn encrypted_zip_password_is_typed_and_debug_redacted() {
    let f = Fixture::new();
    let destination = f.0.join("locked.zip");
    let password = ArchivePassword::new(b"synthetic test secret".to_vec()).unwrap();
    assert!(!format!("{password:?}").contains("synthetic"));
    io::create_with(
        &f.0.join("source"),
        &f.names(),
        &destination,
        ArchiveFormat::Zip,
        Some(&password),
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    assert_eq!(
        io::extract(&destination, &out, &ExtractPolicy::default()),
        Err(ArchiveError::PasswordRequired)
    );
    assert_eq!(
        io::extract_with(
            &destination,
            &out,
            &ExtractPolicy::default(),
            Some(&ArchivePassword::new(b"wrong".to_vec()).unwrap()),
            &CancelToken::new(),
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::IncorrectPassword)
    );
    io::extract_with(
        &destination,
        &out,
        &ExtractPolicy::default(),
        Some(&password),
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(
        std::fs::read(out.join("folder/file")).unwrap(),
        b"contents\n"
    );
}
#[test]
fn occupied_canceled_changed_and_recursive_outputs_never_publish() {
    let f = Fixture::new();
    let source = f.0.join("source");
    let destination = f.0.join("archive.zip");
    std::os::unix::fs::symlink("absent", &destination).unwrap();
    assert_eq!(
        io::create_with(
            &source,
            &f.names(),
            &destination,
            ArchiveFormat::Zip,
            None,
            &CancelToken::new(),
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::DestinationExists)
    );
    assert_eq!(
        std::fs::read_link(&destination).unwrap(),
        PathBuf::from("absent")
    );
    std::fs::remove_file(&destination).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        io::create_with(
            &source,
            &f.names(),
            &destination,
            ArchiveFormat::Zip,
            None,
            &cancel,
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::Cancelled)
    );
    assert!(!destination.exists());
    assert!(io::create_with(
        &source,
        &f.names(),
        &source.join("folder/recursive.zip"),
        ArchiveFormat::Zip,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {}
    )
    .is_err());
    let mut changed = false;
    assert!(io::create_with(
        &source,
        &f.names(),
        &destination,
        ArchiveFormat::Zip,
        None,
        &CancelToken::new(),
        &mut |phase, _, _| {
            if phase == ArchivePhase::Creating && !changed {
                changed = true;
                std::fs::write(source.join("folder/new-work"), b"preserve").unwrap();
            }
        }
    )
    .is_err());
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read(source.join("folder/new-work")).unwrap(),
        b"preserve"
    );
    assert!(!std::fs::read_dir(&f.0).unwrap().flatten().any(|e| e
        .file_name()
        .to_string_lossy()
        .starts_with(".gust-extract-")));
}
#[test]
fn links_are_stored_without_reading_targets_and_unsupported_encryption_has_no_effects() {
    let f = Fixture::new();
    let source = f.0.join("source");
    std::os::unix::fs::symlink("folder/file", source.join("link")).unwrap();
    let names = vec![OsString::from("folder"), OsString::from("link")];
    let destination = f.0.join("link.tar.zst");
    io::create_with(
        &source,
        &names,
        &destination,
        ArchiveFormat::TarZstd,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    io::extract(&destination, &out, &ExtractPolicy::default()).unwrap();
    assert_eq!(
        std::fs::read_link(out.join("link")).unwrap(),
        PathBuf::from("folder/file")
    );
    let password = ArchivePassword::new(b"synthetic".to_vec()).unwrap();
    assert!(matches!(
        io::create_with(
            &source,
            &names,
            &f.0.join("unsupported.7z"),
            ArchiveFormat::SevenZip,
            Some(&password),
            &CancelToken::new(),
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::Unsupported(_))
    ));
    assert!(!f.0.join("unsupported.7z").exists());
}

#[test]
fn cancellation_at_publication_preserves_source_and_absent_destination() {
    let f = Fixture::new();
    let destination = f.0.join("cancel-at-publish.zip");
    let cancel = CancelToken::new();
    let mut publishing = false;
    let result = io::create_with(
        &f.0.join("source"),
        &f.names(),
        &destination,
        ArchiveFormat::Zip,
        None,
        &cancel,
        &mut |phase, _, _| {
            if phase == ArchivePhase::Publishing {
                assert!(!destination.exists());
                publishing = true;
                cancel.cancel();
            }
        },
    );
    assert!(publishing);
    assert_eq!(result, Err(ArchiveError::Cancelled));
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read(f.0.join("source/folder/file")).unwrap(),
        b"contents\n"
    );
}

/// An independent native metadata reader inside its own read-only sandbox:
/// assertions do not trust the writer's frames or Gust's member conversion.
fn independent_mtimes(path: &std::path::Path) -> Vec<(String, i64, i64)> {
    use std::{
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Command, Stdio},
    };
    let inherited = std::fs::File::open(path).unwrap();
    let mut command = Command::new("/usr/bin/bwrap");
    command.args(["--unshare-all","--die-with-parent","--clearenv","--setenv","LC_ALL","C.UTF-8","--ro-bind","/usr","/usr","--symlink","usr/lib","/lib","--symlink","usr/lib","/lib64","--proc","/proc","--dev","/dev","--ro-bind-fd"]).arg(inherited.as_raw_fd().to_string()).arg("/input").args(["/usr/bin/prlimit","--as=536870912","--cpu=10","--nofile=64","--fsize=0","--","/usr/bin/python3","-B","-c",r#"
import ctypes as c,json
lib=c.CDLL('libarchive.so');p=c.c_void_p
lib.archive_read_new.restype=p
for name in ['archive_read_support_filter_all','archive_read_support_format_all','archive_read_data_skip','archive_read_free']:
 getattr(lib,name).argtypes=[p]
lib.archive_read_open_filename.argtypes=[p,c.c_char_p,c.c_size_t]
lib.archive_read_next_header.argtypes=[p,c.POINTER(p)]
lib.archive_entry_pathname.argtypes=[p];lib.archive_entry_pathname.restype=c.c_char_p
lib.archive_entry_mtime.argtypes=[p];lib.archive_entry_mtime.restype=c.c_int64
lib.archive_entry_mtime_nsec.argtypes=[p];lib.archive_entry_mtime_nsec.restype=c.c_long
lib.archive_entry_mtime_is_set.argtypes=[p]
a=lib.archive_read_new();assert a
assert lib.archive_read_support_filter_all(a)==0
assert lib.archive_read_support_format_all(a)==0
assert lib.archive_read_open_filename(a,b'/input',65536)==0
out=[]
while True:
 e=p();status=lib.archive_read_next_header(a,c.byref(e))
 if status==1:break
 assert status==0 and e.value
 assert lib.archive_entry_mtime_is_set(e)>0
 out.append((lib.archive_entry_pathname(e).rstrip(b'/').hex(),lib.archive_entry_mtime(e),lib.archive_entry_mtime_nsec(e)))
 assert lib.archive_read_data_skip(a)==0
lib.archive_read_free(a)
print(json.dumps(out))
"#]).stdin(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only async-signal-safe fcntl on this child-owned input fd.
    unsafe {
        command.pre_exec(move || {
            rustix::io::fcntl_setfd(&inherited, rustix::io::FdFlags::empty())
                .map_err(std::io::Error::from)
        });
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "independent metadata reader failed"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn all_formats_keep_admitted_file_directory_and_link_timestamps() {
    let f = Fixture::new();
    let source = f.0.join("source");
    std::os::unix::fs::symlink("folder/file", source.join("stamp-link")).unwrap();
    let mut names = f.names();
    names.push("stamp-link".into());
    let stamp = rustix::fs::Timespec {
        tv_sec: 1_700_000_123,
        tv_nsec: 456_789_123,
    };
    let times = rustix::fs::Timestamps {
        last_access: stamp,
        last_modification: stamp,
    };
    for path in [
        "folder/file",
        "-dash",
        "line\nbreak",
        "stamp-link",
        "folder",
    ] {
        let (parent, name) = gust_ops::resolve_parent(&source.join(path)).unwrap();
        rustix::fs::utimensat(
            parent.as_fd(),
            name,
            &times,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
    }
    for (format, extension, nsec) in [
        (ArchiveFormat::TarZstd, "tar.zst", 456_789_123),
        (ArchiveFormat::Zip, "zip", 0),
        (ArchiveFormat::SevenZip, "7z", 456_789_100),
    ] {
        let destination = f.0.join(format!("timestamps.{extension}"));
        io::create_with(
            &source,
            &names,
            &destination,
            format,
            None,
            &CancelToken::new(),
            &mut |_, _, _| {},
        )
        .unwrap();
        let rows = independent_mtimes(&destination);
        assert_eq!(rows.len(), 5);
        for (_, seconds, nanoseconds) in rows {
            assert_eq!(seconds, stamp.tv_sec, "{extension}");
            assert_eq!(nanoseconds, nsec, "{extension}");
        }
    }
}

fn unsupported_date_refuses_before_any_payload(format: ArchiveFormat, seconds: i64) {
    let f = Fixture::new();
    let source = f.0.join("source");
    std::fs::write(source.join("a-first"), b"payload must not stream").unwrap();
    let invalid = source.join("z-date");
    std::fs::write(&invalid, b"keep source").unwrap();
    let stamp = rustix::fs::Timespec {
        tv_sec: seconds,
        tv_nsec: 123_456_789,
    };
    let times = rustix::fs::Timestamps {
        last_access: stamp,
        last_modification: stamp,
    };
    let (parent, name) = gust_ops::resolve_parent(&invalid).unwrap();
    rustix::fs::utimensat(
        parent.as_fd(),
        name,
        &times,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
    assert_eq!(
        gust_ops::FileIdentity::of_path(&invalid).unwrap().mtime,
        seconds
    );
    let destination = f.0.join("unused.archive");
    let mut callbacks = Vec::new();
    let result = io::create_with(
        &source,
        &["a-first".into(), "z-date".into()],
        &destination,
        format,
        None,
        &CancelToken::new(),
        &mut |phase, _, bytes| callbacks.push((phase, bytes)),
    );
    assert_eq!(
        result,
        Err(ArchiveError::UnsupportedTimestamp {
            format,
            member: "z-date".into(),
            seconds,
            nanoseconds: 123_456_789
        })
    );
    assert!(callbacks.is_empty(), "payload started: {callbacks:?}");
    assert!(!destination.exists());
    assert_eq!(std::fs::read(invalid).unwrap(), b"keep source");
    assert_eq!(
        std::fs::read_dir(&f.0).unwrap().count(),
        1,
        "no staging was admitted"
    );
}
#[test]
fn negative_7z_date_is_typed_before_payload_streaming() {
    unsupported_date_refuses_before_any_payload(ArchiveFormat::SevenZip, -1);
}
#[test]
fn wide_zip_date_is_typed_before_payload_streaming() {
    unsupported_date_refuses_before_any_payload(ArchiveFormat::Zip, 4_294_967_296);
}

fn name_round_trip(format: ArchiveFormat, bytes: &[u8]) {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let source = f.0.join("source");
    let name = OsString::from_vec(bytes.to_vec());
    std::fs::write(source.join(&name), b"exact bytes").unwrap();
    std::os::unix::fs::symlink(std::path::Path::new(&name), source.join("link")).unwrap();
    let destination = f.0.join("names.archive");
    io::create_with(
        &source,
        &[name.clone(), "link".into()],
        &destination,
        format,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    io::extract(&destination, &out, &Default::default()).unwrap();
    assert_eq!(std::fs::read(out.join(&name)).unwrap(), b"exact bytes");
    assert_eq!(
        std::fs::read_link(out.join("link")).unwrap().as_os_str(),
        &name
    );
}
#[test]
fn tar_unicode_name_and_link_target_round_trip() {
    name_round_trip(ArchiveFormat::TarZstd, "é.txt".as_bytes());
}
#[test]
fn zip_unicode_name_and_link_target_round_trip() {
    name_round_trip(ArchiveFormat::Zip, "é.txt".as_bytes());
}
#[test]
fn sevenzip_unicode_name_and_link_target_round_trip() {
    name_round_trip(ArchiveFormat::SevenZip, "é.txt".as_bytes());
}
#[test]
fn tar_raw_name_and_link_target_round_trip() {
    name_round_trip(ArchiveFormat::TarZstd, b"raw-\xff");
}
#[test]
fn zip_raw_name_and_link_target_round_trip() {
    name_round_trip(ArchiveFormat::Zip, b"raw-\xff");
}
#[test]
fn sevenzip_raw_name_refuses_before_payload() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let source = f.0.join("source");
    let name = OsString::from_vec(b"raw-\xff".to_vec());
    std::fs::write(source.join(&name), b"preserve").unwrap();
    let destination = f.0.join("raw.7z");
    let mut callbacks = 0;
    let result = io::create_with(
        &source,
        std::slice::from_ref(&name),
        &destination,
        ArchiveFormat::SevenZip,
        None,
        &CancelToken::new(),
        &mut |_, _, _| callbacks += 1,
    );
    assert_eq!(
        result,
        Err(ArchiveError::UnsupportedEncoding {
            format: ArchiveFormat::SevenZip,
            member: name.clone().into()
        })
    );
    assert_eq!(callbacks, 0);
    assert!(!destination.exists());
    assert_eq!(std::fs::read(source.join(name)).unwrap(), b"preserve");
}

#[test]
fn sevenzip_raw_link_target_refuses_before_payload() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let source = f.0.join("source");
    let target = OsString::from_vec(b"raw-\xff".to_vec());
    std::os::unix::fs::symlink(&target, source.join("z-link")).unwrap();
    let destination = f.0.join("raw-target.7z");
    let mut callbacks = 0;
    let result = io::create_with(
        &source,
        &["-dash".into(), "z-link".into()],
        &destination,
        ArchiveFormat::SevenZip,
        None,
        &CancelToken::new(),
        &mut |_, _, _| callbacks += 1,
    );
    assert_eq!(
        result,
        Err(ArchiveError::UnsupportedEncoding {
            format: ArchiveFormat::SevenZip,
            member: "z-link".into()
        })
    );
    assert_eq!(callbacks, 0);
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read_link(source.join("z-link"))
            .unwrap()
            .as_os_str(),
        &target
    );
}
#[test]
fn zip_mixed_unicode_and_opaque_members_keep_exact_names_and_link_targets() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let source = f.0.join("source");
    let names = [
        OsString::from_vec(b"a-raw-\xff".to_vec()),
        OsString::from("é.txt"),
    ];
    let mut selection = Vec::new();
    for (index, name) in names.iter().enumerate() {
        std::fs::write(source.join(name), b"exact").unwrap();
        let link = format!("link-{index}");
        std::os::unix::fs::symlink(name, source.join(&link)).unwrap();
        selection.push(name.clone());
        selection.push(link.into());
    }
    let destination = f.0.join("mixed.zip");
    io::create_with(
        &source,
        &selection,
        &destination,
        ArchiveFormat::Zip,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    io::extract(&destination, &out, &Default::default()).unwrap();
    for (index, name) in names.iter().enumerate() {
        assert_eq!(std::fs::read(out.join(name)).unwrap(), b"exact");
        assert_eq!(
            std::fs::read_link(out.join(format!("link-{index}")))
                .unwrap()
                .as_os_str(),
            name
        );
    }
}
