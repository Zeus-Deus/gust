//! Byte-preserving command-line folder admission.

use std::ffi::OsString;
use std::path::PathBuf;

/// The first ordinary launch argument, before filesystem admission.
pub(crate) fn startup_argument(args: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let arg = args
        .into_iter()
        .find(|arg| !arg.as_encoded_bytes().starts_with(b"-"))?;
    if arg.as_encoded_bytes().starts_with(b"file://") {
        return arg.to_str().and_then(gust_core::clipboard::uri_to_path);
    }
    Some(PathBuf::from(arg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    #[test]
    fn a_desktop_file_uri_decodes_spaces_and_literal_fragment_bytes() {
        let got = startup_argument([OsString::from(
            "file:///synthetic/folder%20with%20space%23notes",
        )]);
        assert_eq!(
            got,
            Some(PathBuf::from("/synthetic/folder with space#notes"))
        );
    }

    #[test]
    fn an_encoded_non_utf8_uri_keeps_the_filename_bytes() {
        let got = startup_argument([OsString::from("file:///synthetic/nonutf8-%FF")]).unwrap();
        assert_eq!(got.as_os_str().as_bytes(), b"/synthetic/nonutf8-\xff");
    }

    #[test]
    fn a_plain_non_utf8_path_keeps_its_bytes() {
        let raw = OsString::from_vec(b"/synthetic/plain-\xff".to_vec());
        let got = startup_argument([raw.clone()]).unwrap();
        assert_eq!(got.into_os_string(), raw);
    }

    #[test]
    fn a_relative_plain_path_is_not_percent_decoded() {
        assert_eq!(
            startup_argument([OsString::from("folder%20literal")]),
            Some(PathBuf::from("folder%20literal"))
        );
    }

    #[test]
    fn a_nonlocal_file_uri_is_not_a_literal_filesystem_path() {
        assert_eq!(
            startup_argument([OsString::from("file://server/share")]),
            None
        );
    }
}
