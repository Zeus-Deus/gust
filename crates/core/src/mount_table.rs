//! Pure parsing shared by proc mount-table consumers. Candidate paths stay bytes.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

/// Decode the four kernel mount-field escapes, once; retain all other bytes.
pub fn decode_field(field: &[u8]) -> OsString {
    let mut out = Vec::with_capacity(field.len());
    let mut remaining = field;
    while let Some((&byte, tail)) = remaining.split_first() {
        let decoded = match remaining.get(..4) {
            Some(b"\\040") => Some(b' '),
            Some(b"\\011") => Some(b'\t'),
            Some(b"\\012") => Some(b'\n'),
            Some(b"\\134") => Some(b'\\'),
            _ => None,
        };
        if let Some(decoded) = decoded {
            out.push(decoded);
            remaining = &remaining[4..];
        } else {
            out.push(byte);
            remaining = tail;
        }
    }
    OsString::from_vec(out)
}

pub(crate) fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    // Other Unicode/ASCII whitespace can be literal path bytes in proc tables.
    line.split(|b| matches!(b, b' ' | b'\t'))
        .filter(|field| !field.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn kernel_escapes_decode_once_without_lossy_byte_conversion() {
        assert_eq!(
            decode_field(b"/a\\040b\\011c\\012d\\134e\xff").as_bytes(),
            b"/a b\tc\nd\\e\xff"
        );
        for suffix in [b"040", b"011", b"012", b"134"] {
            let encoded = [b"/a\\134".as_slice(), suffix, b"b"].concat();
            let expected = [b"/a\\".as_slice(), suffix, b"b"].concat();
            assert_eq!(decode_field(&encoded).as_bytes(), expected);
        }
    }

    #[test]
    fn incomplete_unknown_and_adjacent_escapes_remain_unambiguous() {
        for raw in [
            b"/a\\".as_slice(),
            b"/a\\04",
            b"/a\\041",
            b"/a\\777",
            b"/a\\xyz",
        ] {
            assert_eq!(decode_field(raw).as_bytes(), raw);
        }
        assert_eq!(decode_field(b"/a\\134\\040b").as_bytes(), b"/a\\ b");
        assert_eq!(decode_field(b"/a\\134\\134040b").as_bytes(), b"/a\\\\040b");
    }

    #[test]
    fn field_separators_do_not_split_other_path_whitespace() {
        assert_eq!(
            fields(b"dev\t /a\x0b\x0c\r\xc2\xa0b  ext4 rw").collect::<Vec<_>>(),
            [b"dev".as_slice(), b"/a\x0b\x0c\r\xc2\xa0b", b"ext4", b"rw"]
        );
    }
}
