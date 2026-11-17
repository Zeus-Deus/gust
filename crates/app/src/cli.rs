//! Byte-preserving command-line folder admission.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The arguments that may be options: everything before a `--` terminator.
pub(crate) fn flags(args: &[OsString]) -> &[OsString] {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    &args[..end]
}

/// The first ordinary launch argument, before filesystem admission.
///
/// Arguments starting with `-` are options until a `--`, after which the
/// next one is taken as it is, so a folder named `-x` can be opened.
///
/// `Err` carries an argument that names nothing on this computer
/// (`file://server/…`), so the window can say so instead of silently
/// opening Home.
pub(crate) fn startup_argument(
    args: impl IntoIterator<Item = OsString>,
) -> Option<Result<PathBuf, OsString>> {
    let mut args = args.into_iter();
    let arg = loop {
        let arg = args.next()?;
        if arg == "--" {
            break args.next()?;
        }
        if !arg.as_encoded_bytes().starts_with(b"-") {
            break arg;
        }
    };
    if arg.as_encoded_bytes().starts_with(b"file://") {
        return Some(
            arg.to_str()
                .and_then(gust_core::clipboard::uri_to_path)
                .ok_or(arg),
        );
    }
    Some(Ok(PathBuf::from(arg)))
}

/// Where a window starts: the folder, the entry to put the cursor on, and
/// one line to show when the argument could not be honoured as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Startup {
    pub folder: PathBuf,
    pub focus: Option<OsString>,
    pub notice: Option<String>,
}

impl Startup {
    pub(crate) fn folder(folder: PathBuf) -> Self {
        Startup {
            folder,
            focus: None,
            notice: None,
        }
    }
}

/// Admit an absolute launch argument against the filesystem.
pub(crate) fn resolve_startup(abs: &Path, home: &Path) -> Startup {
    let why = match std::fs::metadata(abs) {
        Ok(m) if m.is_dir() => return Startup::folder(abs.to_path_buf()),
        // Pointed at a file: open its folder with the cursor on it, which
        // is what the user means.
        Ok(_) => return beside(abs, home),
        // A link to nothing is still an entry in its folder.
        Err(_) if std::fs::symlink_metadata(abs).is_ok() => return beside(abs, home),
        Err(e) => e.kind(),
    };
    let folder = abs
        .ancestors()
        .skip(1)
        .find(|a| a.is_dir())
        .unwrap_or(home)
        .to_path_buf();
    let verb = if why == std::io::ErrorKind::NotFound {
        "find"
    } else {
        "open"
    };
    let name = abs.file_name().unwrap_or(abs.as_os_str());
    let notice = instead(verb, name, &folder, home);
    Startup {
        folder,
        focus: None,
        notice: Some(notice),
    }
}

/// An argument that is not a path on this computer at all: Home, and say so.
pub(crate) fn unusable_startup(arg: &OsStr, home: &Path) -> Startup {
    Startup {
        folder: home.to_path_buf(),
        focus: None,
        notice: Some(instead("open", arg, home, home)),
    }
}

fn beside(entry: &Path, home: &Path) -> Startup {
    match (entry.parent(), entry.file_name()) {
        (Some(folder), Some(name)) => Startup {
            folder: folder.to_path_buf(),
            focus: Some(name.to_os_string()),
            notice: None,
        },
        _ => Startup::folder(home.to_path_buf()),
    }
}

fn instead(verb: &str, name: &OsStr, folder: &Path, home: &Path) -> String {
    let shown = if folder == home {
        "Home".to_string()
    } else {
        folder.display().to_string()
    };
    format!(
        "Could not {verb} \"{}\" — showing {shown} instead",
        name.to_string_lossy()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    /// A private folder under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gust-cli-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // The temp dir itself may be reached through a link.
            Scratch(dir.canonicalize().unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_desktop_file_uri_decodes_spaces_and_literal_fragment_bytes() {
        let got = startup_argument([OsString::from(
            "file:///synthetic/folder%20with%20space%23notes",
        )]);
        assert_eq!(
            got,
            Some(Ok(PathBuf::from("/synthetic/folder with space#notes")))
        );
    }

    #[test]
    fn an_encoded_non_utf8_uri_keeps_the_filename_bytes() {
        let got = startup_argument([OsString::from("file:///synthetic/nonutf8-%FF")])
            .unwrap()
            .unwrap();
        assert_eq!(got.as_os_str().as_bytes(), b"/synthetic/nonutf8-\xff");
    }

    #[test]
    fn a_plain_non_utf8_path_keeps_its_bytes() {
        let raw = OsString::from_vec(b"/synthetic/plain-\xff".to_vec());
        let got = startup_argument([raw.clone()]).unwrap().unwrap();
        assert_eq!(got.into_os_string(), raw);
    }

    #[test]
    fn a_relative_plain_path_is_not_percent_decoded() {
        assert_eq!(
            startup_argument([OsString::from("folder%20literal")]),
            Some(Ok(PathBuf::from("folder%20literal")))
        );
    }

    #[test]
    fn a_nonlocal_file_uri_is_not_a_literal_filesystem_path() {
        assert_eq!(
            startup_argument([OsString::from("file://server/share")]),
            Some(Err(OsString::from("file://server/share")))
        );
    }

    #[test]
    fn a_localhost_file_uri_is_a_local_path() {
        assert_eq!(
            startup_argument([OsString::from("file://localhost/synthetic/a.pdf")]),
            Some(Ok(PathBuf::from("/synthetic/a.pdf")))
        );
    }

    #[test]
    fn a_terminator_lets_a_folder_named_like_an_option_be_opened() {
        assert_eq!(
            startup_argument([OsString::from("--"), OsString::from("-x")]),
            Some(Ok(PathBuf::from("-x")))
        );
        assert_eq!(
            startup_argument(["--some-option", "--", "--default", "later"].map(OsString::from)),
            Some(Ok(PathBuf::from("--default")))
        );
        // Without the terminator a leading dash is still an option.
        assert_eq!(startup_argument([OsString::from("-x")]), None);
        assert_eq!(startup_argument([OsString::from("--")]), None);
    }

    #[test]
    fn options_are_only_looked_for_before_the_terminator() {
        let args = ["--help", "--", "--default"].map(OsString::from);
        assert_eq!(flags(&args), &args[..1]);
        let plain = ["a", "--default"].map(OsString::from);
        assert_eq!(flags(&plain), &plain[..]);
    }

    #[test]
    fn a_file_opens_its_folder_with_the_cursor_on_the_file() {
        let dir = Scratch::new("file");
        let file = dir.0.join("a.pdf");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            resolve_startup(&file, Path::new("/synthetic-home")),
            Startup {
                folder: dir.0.clone(),
                focus: Some(OsString::from("a.pdf")),
                notice: None,
            }
        );
    }

    #[test]
    fn a_folder_opens_as_itself_without_a_cursor_request() {
        let dir = Scratch::new("folder");
        assert_eq!(
            resolve_startup(&dir.0, Path::new("/synthetic-home")),
            Startup::folder(dir.0.clone())
        );
    }

    #[test]
    fn a_dangling_link_is_still_an_entry_to_put_the_cursor_on() {
        let dir = Scratch::new("dangling");
        let link = dir.0.join("gone-link");
        std::os::unix::fs::symlink(dir.0.join("nowhere"), &link).unwrap();
        assert_eq!(
            resolve_startup(&link, Path::new("/synthetic-home")),
            Startup {
                folder: dir.0.clone(),
                focus: Some(OsString::from("gone-link")),
                notice: None,
            }
        );
    }

    #[test]
    fn a_missing_path_opens_the_nearest_folder_that_exists_and_says_so() {
        let dir = Scratch::new("missing");
        let missing = dir.0.join("gone").join("deeper").join("a.pdf");
        assert_eq!(
            resolve_startup(&missing, Path::new("/synthetic-home")),
            Startup {
                folder: dir.0.clone(),
                focus: None,
                notice: Some(format!(
                    "Could not find \"a.pdf\" — showing {} instead",
                    dir.0.display()
                )),
            }
        );
    }

    #[test]
    fn a_missing_path_directly_under_home_names_home_plainly() {
        let home = Scratch::new("home");
        let got = resolve_startup(&home.0.join("nope"), &home.0);
        assert_eq!(got.folder, home.0);
        assert_eq!(
            got.notice.as_deref(),
            Some("Could not find \"nope\" — showing Home instead")
        );
    }

    #[test]
    fn a_file_uri_on_another_computer_opens_home_and_says_so() {
        let got = unusable_startup(
            OsStr::new("file://server/share"),
            Path::new("/synthetic-home"),
        );
        assert_eq!(got.folder, Path::new("/synthetic-home"));
        assert_eq!(got.focus, None);
        assert_eq!(
            got.notice.as_deref(),
            Some("Could not open \"file://server/share\" — showing Home instead")
        );
    }
}
