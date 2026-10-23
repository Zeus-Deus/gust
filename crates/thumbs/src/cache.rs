//! Freedesktop thumbnail cache paths.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Normal, // 128px
    Large,  // 256px
    /// 1024px, the freedesktop spec's `xx-large`: what Quick Look shows.
    /// A 256px render stretched across the panel was visibly blurry.
    XxLarge,
}

impl ThumbSize {
    pub fn dir_name(self) -> &'static str {
        match self {
            ThumbSize::Normal => "normal",
            ThumbSize::Large => "large",
            ThumbSize::XxLarge => "xx-large",
        }
    }
    pub fn pixels(self) -> u32 {
        match self {
            ThumbSize::Normal => 128,
            ThumbSize::Large => 256,
            ThumbSize::XxLarge => 1024,
        }
    }
}

pub(crate) fn md5_hex(data: &[u8]) -> String {
    // Minimal MD5 — the freedesktop spec mandates it for cache file names.
    // Used ONLY for cache addressing, never for security.
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());

    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);

    for chunk in msg.chunks(64) {
        let m: [u32; 16] = std::array::from_fn(|i| {
            let at = i * 4;
            u32::from_le_bytes([chunk[at], chunk[at + 1], chunk[at + 2], chunk[at + 3]])
        });
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f2.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(32);
    for v in [a0, b0, c0, d0] {
        for byte in v.to_le_bytes() {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 15) as usize] as char);
        }
    }
    out
}

/// Cache base: `$XDG_CACHE_HOME/thumbnails`.
pub fn cache_base() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("thumbnails"))
}

/// URI used by the spec for a local file.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let bytes = path.as_os_str().as_bytes();
    let mut s = String::with_capacity(7 + bytes.len());
    s.push_str("file://");
    for &b in bytes {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/');
        if unreserved {
            s.push(b as char);
        } else {
            s.push('%');
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 15) as usize] as char);
        }
    }
    s
}

/// Where the thumbnail for `path` lives.
pub fn thumbnail_path(path: &Path, size: ThumbSize) -> Option<PathBuf> {
    let base = cache_base()?;
    Some(thumbnail_path_in(&base, path, size))
}

pub(crate) fn thumbnail_path_in(base: &Path, path: &Path, size: ThumbSize) -> PathBuf {
    let hash = md5_hex(file_uri(path).as_bytes());
    base.join(size.dir_name()).join(format!("{hash}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_known_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        assert_eq!(
            md5_hex(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
            "d174ab98d277d9f5a5611c2c9f419d9f"
        );
        assert_eq!(
            md5_hex(&[b'a'; 1_000_000]),
            "7707d6ae4e027c70eea2a935c2296f21"
        );
    }

    #[test]
    fn uri_percent_encodes_spaces() {
        assert_eq!(file_uri(Path::new("/a b/c")), "file:///a%20b/c");
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            file_uri(Path::new(std::ffi::OsStr::from_bytes(b"/a%#\xff"))),
            "file:///a%25%23%FF"
        );
    }

    #[test]
    fn thumbnail_path_is_stable_and_size_scoped() {
        let base = Path::new("/tmp/gust-cache-test");
        let p = Path::new("/home/u/pic.png");
        let a = thumbnail_path_in(base, p, ThumbSize::Normal);
        let b = thumbnail_path_in(base, p, ThumbSize::Normal);
        let c = thumbnail_path_in(base, p, ThumbSize::Large);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.to_string_lossy().contains("/normal/"));
        assert!(c.to_string_lossy().contains("/large/"));
    }
}
