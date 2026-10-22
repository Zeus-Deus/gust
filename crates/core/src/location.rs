//! Location identity and capability model.
//!
//! See `docs/DESIGN.md` §1 and §2. The rules that matter:
//! - location bytes are OS bytes, never `String` (non-UTF-8 names must survive)
//! - a row index / translated label / temp mount path is never identity
//! - `Unknown` capability is distinct from `No`

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// Which backend owns a location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProviderId {
    Local,
    Sftp,
    Gio,
    Rclone,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Local => "local",
            ProviderId::Sftp => "sftp",
            ProviderId::Gio => "gio",
            ProviderId::Rclone => "rclone",
        }
    }

    /// Parse a provider id. Named `parse_id` rather than `from_str` so it is
    /// not confused with the `FromStr` trait (this cannot fail into an error
    /// type; it returns `None`).
    pub fn parse_id(s: &str) -> Option<Self> {
        match s {
            "local" => Some(ProviderId::Local),
            "sftp" => Some(ProviderId::Sftp),
            "gio" => Some(ProviderId::Gio),
            "rclone" => Some(ProviderId::Rclone),
            _ => None,
        }
    }
}

/// Durable identity of a directory or file location.
///
/// Equality is byte equality of the location, never display text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocationKey {
    provider: ProviderId,
    /// Raw OS bytes. For `Local` this is the absolute path.
    location: OsString,
}

impl LocationKey {
    pub fn local(path: impl AsRef<Path>) -> Self {
        LocationKey {
            provider: ProviderId::Local,
            location: path.as_ref().as_os_str().to_os_string(),
        }
    }

    pub fn new(provider: ProviderId, location: OsString) -> Self {
        LocationKey { provider, location }
    }

    pub fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn location(&self) -> &OsStr {
        &self.location
    }

    /// Local filesystem path, if this is a local location.
    pub fn as_path(&self) -> Option<&Path> {
        match self.provider {
            ProviderId::Local => Some(Path::new(&self.location)),
            _ => None,
        }
    }

    /// Stable string form for session state: `provider:hex-of-bytes`.
    ///
    /// Hex rather than lossy UTF-8 so non-UTF-8 names round-trip exactly.
    pub fn to_token(&self) -> String {
        let mut s = String::with_capacity(8 + self.location.as_bytes().len() * 2);
        s.push_str(self.provider.as_str());
        s.push(':');
        for b in self.location.as_bytes() {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn from_token(token: &str) -> Option<Self> {
        let (p, hex) = token.split_once(':')?;
        let provider = ProviderId::parse_id(p)?;
        if hex.len() % 2 != 0 {
            return None;
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let h = hex.as_bytes();
        for pair in h.chunks(2) {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            bytes.push((hi * 16 + lo) as u8);
        }
        Some(LocationKey {
            provider,
            location: OsString::from_vec(bytes),
        })
    }

    /// Parent location, or `None` at the root.
    pub fn parent(&self) -> Option<LocationKey> {
        let p = Path::new(&self.location).parent()?;
        if p.as_os_str().is_empty() {
            return None;
        }
        Some(LocationKey {
            provider: self.provider,
            location: p.as_os_str().to_os_string(),
        })
    }

    pub fn join(&self, name: &OsStr) -> LocationKey {
        let mut pb = PathBuf::from(&self.location);
        pb.push(name);
        LocationKey {
            provider: self.provider,
            location: pb.into_os_string(),
        }
    }

    /// Final path component as display text.
    pub fn display_name(&self) -> String {
        let p = Path::new(&self.location);
        match p.file_name() {
            Some(n) => escape_for_display(n),
            None => escape_for_display(p.as_os_str()),
        }
    }

    /// Full location as display text (lossy + control chars escaped).
    pub fn display_path(&self) -> String {
        escape_for_display(&self.location)
    }
}

/// Is `target` the folder `current` already shows? Trailing and doubled
/// slashes do not make a different place. Used so clicking the folder on
/// screen (sidebar, breadcrumb) does not reload it (Flea 0.3.7).
pub fn same_place(current: &LocationKey, target: &Path) -> bool {
    let norm = |p: &Path| p.components().collect::<std::path::PathBuf>();
    current.as_path().is_some_and(|c| norm(c) == norm(target))
}

#[cfg(test)]
mod same_place_tests {
    use super::*;

    #[test]
    fn the_same_folder_is_recognised_however_it_is_written() {
        let k = LocationKey::local("/home/u/Documents");
        assert!(same_place(&k, Path::new("/home/u/Documents")));
        assert!(same_place(&k, Path::new("/home/u/Documents/")));
        assert!(same_place(&k, Path::new("/home/u//Documents")));
        assert!(!same_place(&k, Path::new("/home/u/Documents2")));
        assert!(!same_place(&k, Path::new("/home/u")));
    }
}

/// Render OS bytes safely for a UI label.
///
/// Escapes C0/C1 controls and Unicode bidi overrides (a filename containing
/// U+202E can otherwise make `evil.txt.exe` render as `evil.exe.txt`).
/// This changes only the *display* string — never the filesystem bytes.
pub fn escape_for_display(s: &OsStr) -> String {
    let lossy = String::from_utf8_lossy(s.as_bytes());
    let mut out = String::with_capacity(lossy.len());
    for c in lossy.chars() {
        push_escaped_display_char(&mut out, c);
    }
    out
}

pub(crate) fn push_escaped_display_char(out: &mut String, c: char) {
    let dangerous = matches!(c,
        '\u{202A}'..='\u{202E}'   // LRE RLE PDF LRO RLO
        | '\u{2066}'..='\u{2069}' // LRI RLI FSI PDI
        | '\u{200E}' | '\u{200F}' // LRM RLM
        | '\u{061C}'              // ALM
    );
    if c.is_control() || dangerous {
        out.push_str(&format!("<U+{:04X}>", c as u32));
    } else {
        out.push(c);
    }
}

/// The place a path lives in, when it is somewhere Gust or the desktop
/// mounted something: the mount's root folder and the name a person knows it
/// by. `/run/user/1000/gust/tailnet/nas/Shared` → (`…/tailnet/nas`, "nas").
///
/// The path bar starts there instead of at `/`, and the window title, the
/// tab and a stalled-place message use the name. Pure: it reads no file.
pub fn place_root(path: &Path) -> Option<(PathBuf, String)> {
    let comps: Vec<&OsStr> = path.components().map(|c| c.as_os_str()).collect();
    let at = |i: usize| comps.get(i).map(|c| c.to_string_lossy().to_string());
    let root_of = |n: usize| -> PathBuf { comps[..n].iter().collect() };
    // /run/user/<uid>/gust/{places,tailnet,cloud}/<name>
    // /run/user/<uid>/gvfs/<share>
    if at(1).as_deref() == Some("run") && at(2).as_deref() == Some("user") {
        if at(4).as_deref() == Some("gust") {
            if let (Some(kind), Some(name)) = (at(5), at(6)) {
                if matches!(kind.as_str(), "places" | "tailnet" | "cloud") {
                    return Some((root_of(7), escape_for_display(OsStr::new(&name))));
                }
            }
        }
        if at(4).as_deref() == Some("gvfs") {
            if let Some(share) = at(5) {
                return Some((root_of(6), gvfs_label(&share)));
            }
        }
    }
    // /run/media/<user>/<label>: a drive udisks mounted.
    if at(1).as_deref() == Some("run") && at(2).as_deref() == Some("media") {
        if let Some(label) = at(4) {
            return Some((root_of(5), escape_for_display(OsStr::new(&label))));
        }
    }
    None
}

/// A GVfs mount directory's name as a person would say it:
/// `smb-share:server=nas,share=photos` → "photos on nas".
fn gvfs_label(dir: &str) -> String {
    let Some((scheme, params)) = dir.split_once(':') else {
        return escape_for_display(OsStr::new(dir));
    };
    // A phone's mount names it by serial; say what it is instead (the app
    // replaces this with the phone's own name once it has opened it).
    match scheme {
        "afc" => return "iPhone or iPad".into(),
        "mtp" => return "Phone".into(),
        "gphoto2" => return "Camera".into(),
        _ => {}
    }
    let mut server = None;
    let mut share = None;
    for kv in params.split(',') {
        match kv.split_once('=') {
            Some(("server" | "host", v)) => server = Some(v.to_string()),
            Some(("share" | "prefix", v)) => share = Some(v.trim_matches('/').replace("%2F", "/")),
            _ => {}
        }
    }
    let label = match (share, server) {
        (Some(sh), Some(sv)) if !sh.is_empty() => format!("{sh} on {sv}"),
        (_, Some(sv)) => sv,
        _ => dir.to_string(),
    };
    escape_for_display(OsStr::new(&label))
}

#[cfg(test)]
mod place_root_tests {
    use super::*;

    #[test]
    fn gusts_own_mounts_are_named_by_the_place() {
        for (p, root, label) in [
            (
                "/run/user/1000/gust/tailnet/nas/Shared/a",
                "/run/user/1000/gust/tailnet/nas",
                "nas",
            ),
            (
                "/run/user/1000/gust/places/box",
                "/run/user/1000/gust/places/box",
                "box",
            ),
            (
                "/run/user/1000/gust/cloud/drive/x",
                "/run/user/1000/gust/cloud/drive",
                "drive",
            ),
            (
                "/run/media/me/USB STICK/dcim",
                "/run/media/me/USB STICK",
                "USB STICK",
            ),
            (
                "/run/user/1000/gvfs/afc:host=00008030-000A1B2C3D4E5F60/DCIM",
                "/run/user/1000/gvfs/afc:host=00008030-000A1B2C3D4E5F60",
                "iPhone or iPad",
            ),
            (
                "/run/user/1000/gvfs/smb-share:server=nas.local,share=photos/2024",
                "/run/user/1000/gvfs/smb-share:server=nas.local,share=photos",
                "photos on nas.local",
            ),
        ] {
            let (r, l) = place_root(Path::new(p)).unwrap_or_else(|| panic!("{p}"));
            assert_eq!(r, PathBuf::from(root), "{p}");
            assert_eq!(l, label, "{p}");
        }
    }

    #[test]
    fn ordinary_folders_and_half_paths_have_no_place() {
        for p in [
            "/home/me/Documents",
            "/run/user/1000/gust/tailnet",
            "/run/user/1000/gust/other/x",
            "/run/media/me",
            "/",
        ] {
            assert_eq!(place_root(Path::new(p)), None, "{p}");
        }
    }

    #[test]
    fn a_hostile_device_name_is_shown_escaped() {
        let (_, l) = place_root(Path::new("/run/media/me/a\u{202E}gpj.exe")).unwrap();
        assert!(l.contains("<U+202E>"), "{l}");
    }
}

/// Tri-state capability. `Unknown` must not be collapsed into `No`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Yes,
    No,
    Unknown,
}

impl Cap {
    /// True only for a definite yes.
    pub fn is_yes(self) -> bool {
        self == Cap::Yes
    }
    /// True for a definite no — the UI disables the action.
    pub fn is_no(self) -> bool {
        self == Cap::No
    }
    /// Unknown means: attempt it, and report honestly what happened.
    pub fn is_unknown(self) -> bool {
        self == Cap::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub read: Cap,
    pub write: Cap,
    pub atomic_rename: Cap,
    pub trash: Cap,
    pub unix_permissions: Cap,
    pub server_side_copy: Cap,
    pub hardlinks: Cap,
    pub symlinks: Cap,
    pub case_sensitive: Cap,
    pub max_name_len: Option<usize>,
}

impl Capabilities {
    /// Everything unknown: the honest default for a backend not yet probed.
    pub fn unknown() -> Self {
        Capabilities {
            read: Cap::Unknown,
            write: Cap::Unknown,
            atomic_rename: Cap::Unknown,
            trash: Cap::Unknown,
            unix_permissions: Cap::Unknown,
            server_side_copy: Cap::Unknown,
            hardlinks: Cap::Unknown,
            symlinks: Cap::Unknown,
            case_sensitive: Cap::Unknown,
            max_name_len: None,
        }
    }

    /// A local POSIX filesystem. Trash is `Unknown` until the trash directory
    /// is actually resolved for the volume holding the file.
    pub fn local_posix() -> Self {
        Capabilities {
            read: Cap::Yes,
            write: Cap::Yes,
            atomic_rename: Cap::Yes,
            trash: Cap::Unknown,
            unix_permissions: Cap::Yes,
            server_side_copy: Cap::No,
            hardlinks: Cap::Yes,
            symlinks: Cap::Yes,
            case_sensitive: Cap::Yes,
            max_name_len: Some(255),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_utf8_names_round_trip_through_token() {
        // 0xFF is not valid UTF-8 — it must survive identity round trips.
        let raw = OsString::from_vec(b"/tmp/caf\xff\xfe-name".to_vec());
        let key = LocationKey::new(ProviderId::Local, raw.clone());
        let token = key.to_token();
        let back = LocationKey::from_token(&token).expect("token parses");
        assert_eq!(back.location(), raw.as_os_str());
        assert_eq!(back, key);
    }

    #[test]
    fn display_escapes_bidi_override() {
        // The classic filename spoof: RLO makes "gpj.exe" look like "exe.jpg".
        let name = OsString::from("photo\u{202E}gpj.exe");
        let shown = escape_for_display(&name);
        assert!(
            !shown.contains('\u{202E}'),
            "bidi override must not reach UI"
        );
        assert!(shown.contains("<U+202E>"));
    }

    #[test]
    fn display_escapes_newline_in_filename() {
        let name = OsString::from("two\nlines");
        assert_eq!(escape_for_display(&name), "two<U+000A>lines");
    }

    #[test]
    fn unknown_is_not_no() {
        assert!(!Cap::Unknown.is_no());
        assert!(!Cap::Unknown.is_yes());
        assert!(Cap::Unknown.is_unknown());
        // A freshly seen backend claims nothing.
        assert!(Capabilities::unknown().trash.is_unknown());
    }

    #[test]
    fn local_trash_is_unknown_until_probed() {
        // Trash support depends on the volume, not on "it is local".
        assert!(Capabilities::local_posix().trash.is_unknown());
    }

    #[test]
    fn parent_and_join_preserve_provider() {
        let k = LocationKey::local("/home/user/projects");
        assert_eq!(
            k.parent().unwrap().as_path().unwrap(),
            Path::new("/home/user")
        );
        let c = k.join(OsStr::new("gust"));
        assert_eq!(c.as_path().unwrap(), Path::new("/home/user/projects/gust"));
        assert_eq!(c.provider(), ProviderId::Local);
    }

    #[test]
    fn root_has_no_parent() {
        assert!(LocationKey::local("/").parent().is_none());
    }
}
