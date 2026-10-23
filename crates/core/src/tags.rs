//! Colour tags, stored where Dolphin and Flea keep them (DESIGN §6.25).
//!
//! The extended attribute `user.xdg.tags` holds a comma-separated list of
//! names. Gust offers seven colours; a name it does not know (a Dolphin
//! word tag) is kept exactly as it is whenever Gust writes the list back.
//!
//! Reading is one `lgetxattr` per row, done with the rest of a row's
//! metadata on a worker: never on the UI thread, never following a link.

use std::path::Path;

/// The attribute Dolphin reads and writes.
pub const ATTR: &str = "user.xdg.tags";

/// The seven colours, in key order: Alt+1 is Red … Alt+7 is Gray.
pub const NAMES: [&str; 7] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"];

/// A stored value larger than this is not read (a hostile or broken file
/// cannot make Gust allocate without bound). Real tag lists are tiny.
pub const MAX_VALUE: usize = 4096;

/// Colours are interoperable across spelling case; word tags retain identity.
pub fn contains_tag(names: &[String], query: &str) -> bool {
    let query = query.trim();
    let colour = NAMES.iter().any(|name| name.eq_ignore_ascii_case(query));
    names.iter().any(|name| {
        if colour {
            name.trim().eq_ignore_ascii_case(query)
        } else {
            name.trim() == query
        }
    })
}

/// Split a stored value into names: trimmed, empties dropped.
pub fn parse(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == b',')
        .map(|p| String::from_utf8_lossy(p).trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The stored form of a list of names.
pub fn join(names: &[String]) -> Vec<u8> {
    names.join(",").into_bytes()
}

/// Which of the seven colours a list carries, as bits (bit 0 = Red).
pub fn mask_of(names: &[String]) -> u8 {
    let mut m = 0u8;
    for n in names {
        if let Some(i) = NAMES.iter().position(|k| k.eq_ignore_ascii_case(n.trim())) {
            m |= 1 << i;
        }
    }
    m
}

/// The colour keys of a mask, in order ("red", "blue"…), for the views.
pub fn keys_of(mask: u8) -> Vec<&'static str> {
    const KEYS: [&str; 7] = ["red", "orange", "yellow", "green", "blue", "purple", "gray"];
    (0..7)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| KEYS[i])
        .collect()
}

/// Add or remove colour `n` (1–7) in `names`, keeping every other name —
/// including ones Gust does not know — in its place.
pub fn with_colour(names: &[String], n: usize, on: bool) -> Vec<String> {
    let Some(colour) = NAMES.get(n.wrapping_sub(1)) else {
        return names.to_vec();
    };
    let mut out: Vec<String> = names
        .iter()
        .filter(|x| !x.trim().eq_ignore_ascii_case(colour))
        .cloned()
        .collect();
    if on {
        out.push((*colour).to_string());
    }
    out
}

/// Every colour removed; names Gust does not know are kept.
pub fn without_colours(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|x| !NAMES.iter().any(|k| k.eq_ignore_ascii_case(x.trim())))
        .cloned()
        .collect()
}

/// The raw stored value of `path` (not following a final link). `None`
/// when it has no tags or its filesystem cannot hold them.
pub fn read_raw(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0u8; MAX_VALUE];
    match rustix::fs::lgetxattr(path, ATTR, &mut buf[..]) {
        Ok(n) => {
            buf.truncate(n);
            Ok(Some(buf))
        }
        Err(e) if e == rustix::io::Errno::NODATA || e == rustix::io::Errno::NOTSUP => Ok(None),
        // ERANGE: larger than any real tag list. Treated as untagged for
        // display; writing refuses rather than clobber what it cannot read.
        Err(e) => Err(e.into()),
    }
}

/// The colour mask of `path`, for a row. Errors read as "no tags".
pub fn mask_for(path: &Path) -> u8 {
    match read_raw(path) {
        Ok(Some(raw)) => mask_of(&parse(&raw)),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dolphin_lists_parse() {
        assert_eq!(parse(b"Red, Custom ,,Blue"), v(&["Red", "Custom", "Blue"]));
        assert_eq!(parse(b""), Vec::<String>::new());
        assert_eq!(join(&v(&["Red", "work"])), b"Red,work".to_vec());
    }

    #[test]
    fn colours_are_matched_in_any_case() {
        assert_eq!(mask_of(&v(&["red", "BLUE", "work"])), 0b1_0001);
        assert_eq!(keys_of(0b1_0001), ["red", "blue"]);
        assert_eq!(keys_of(0), Vec::<&str>::new());
    }

    #[test]
    fn toggling_keeps_names_gust_does_not_know() {
        let start = v(&["work", "red"]);
        assert_eq!(with_colour(&start, 5, true), v(&["work", "red", "Blue"]));
        assert_eq!(with_colour(&start, 1, false), v(&["work"]));
        // Adding a colour already there does not duplicate it.
        assert_eq!(with_colour(&start, 1, true), v(&["work", "Red"]));
        assert_eq!(without_colours(&v(&["Red", "work", "Gray"])), v(&["work"]));
        // Out of range is no change.
        assert_eq!(with_colour(&start, 0, true), start);
        assert_eq!(with_colour(&start, 8, true), start);
    }

    #[test]
    fn a_file_without_tags_reads_as_none() {
        let d = std::env::temp_dir().join(format!("gust-tags-core-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("plain.txt");
        std::fs::write(&f, "x").unwrap();
        assert_eq!(read_raw(&f).unwrap(), None);
        assert_eq!(mask_for(&f), 0);
        // A missing file is "no tags" for a row, never a panic.
        assert_eq!(mask_for(&d.join("gone")), 0);
        std::fs::remove_dir_all(&d).ok();
    }
}
