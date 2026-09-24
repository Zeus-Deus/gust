//! Properties: the facts about one path, as label/value pairs.
//!
//! Gathered with `lstat` only — a symlink is described, never followed —
//! and every string that came from the filesystem is passed through the
//! same sanitiser as Quick Look, so a hostile link target is shown, not
//! obeyed. Owner and group names are read from `/etc/passwd` and
//! `/etc/group` directly; no NSS call, no helper process.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub label: &'static str,
    pub value: String,
}

fn fact(label: &'static str, value: impl Into<String>) -> Fact {
    Fact { label, value: value.into() }
}

/// A filesystem string made safe to display: valid text is kept as
/// written (any language), invalid bytes become U+FFFD, and control or
/// bidirectional-override characters are written as `<U+XXXX>` so a name
/// can neither paint escape codes nor visually reverse itself.
fn shown(s: &std::ffi::OsStr) -> String {
    String::from_utf8_lossy(s.as_encoded_bytes())
        .chars()
        .map(|c| {
            let bidi = matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}');
            if c.is_control() || bidi {
                format!("<U+{:04X}>", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Everything the Properties view lists, in display order.
pub fn describe(path: &Path) -> Vec<Fact> {
    let mut out = Vec::new();
    if let Some(parent) = path.parent() {
        out.push(fact("Location", shown(parent.as_os_str())));
    }
    let Ok(md) = std::fs::symlink_metadata(path) else {
        out.push(fact("Status", "This item no longer exists"));
        return out;
    };
    let ft = md.file_type();
    if ft.is_symlink() {
        if let Ok(t) = std::fs::read_link(path) {
            out.push(fact("Link to", shown(t.as_os_str())));
        }
    }
    let offset = local_offset_seconds();
    out.push(fact("Modified", full_time(md.mtime(), offset)));
    if let Ok(created) = md.created() {
        if let Ok(d) = created.duration_since(std::time::UNIX_EPOCH) {
            out.push(fact("Created", full_time(d.as_secs() as i64, offset)));
        }
    }
    out.push(fact("Accessed", full_time(md.atime(), offset)));
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    out.push(fact("Owner", owner_names(md.uid(), md.gid(), &passwd, &group)));
    let mode = md.permissions().mode();
    out.push(fact(
        "Permissions",
        format!(
            "{}  ({:o})",
            crate::peek::mode_line(mode, ft.is_dir(), ft.is_symlink()),
            mode & 0o7777
        ),
    ));
    if md.nlink() > 1 && !ft.is_dir() {
        out.push(fact("Hard links", md.nlink().to_string()));
    }
    out
}

/// `YYYY-MM-DD HH:MM`, for a Unix time shifted by `offset` seconds.
pub fn full_time(unix: i64, offset: i64) -> String {
    let t = unix + offset;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// This machine's current UTC offset, from the `date` of `localtime`.
///
/// Read from `/etc/localtime` via the C library's own conversion, which
/// honours `TZ`. No process is spawned.
pub fn local_offset_seconds() -> i64 {
    // SAFETY: localtime_r writes only into the struct passed to it.
    unsafe {
        let now = libc_time();
        let mut tm: Tm = std::mem::zeroed();
        if localtime_r(&now, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff
    }
}

#[repr(C)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const std::ffi::c_char,
}

extern "C" {
    fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    fn tzset();
}

fn libc_time() -> i64 {
    // tzset first so TZ changes and /etc/localtime are honoured.
    unsafe { tzset() };
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `user : group` by name, falling back to the numbers.
pub fn owner_names(uid: u32, gid: u32, passwd: &str, group: &str) -> String {
    let lookup = |db: &str, id: u32| {
        db.lines()
            .filter_map(|l| {
                let mut f = l.split(':');
                let name = f.next()?;
                let _pw = f.next()?;
                let n: u32 = f.next()?.parse().ok()?;
                (n == id).then(|| name.to_string())
            })
            .next()
            .unwrap_or_else(|| id.to_string())
    };
    format!("{} : {}", lookup(passwd, uid), lookup(group, gid))
}

/// The list's Modified column: "today 16:49", "yesterday 23:50", or the date.
/// Local time, local calendar days — never UTC.
pub fn relative_time(mtime: i64, now: i64, offset: i64) -> String {
    let day = |t: i64| (t + offset).div_euclid(86_400);
    let hm = |t: i64| {
        let s = (t + offset).rem_euclid(86_400);
        format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
    };
    match day(now) - day(mtime) {
        d if d < 0 => full_time(mtime, offset),
        0 => format!("today {}", hm(mtime)),
        1 => format!("yesterday {}", hm(mtime)),
        _ => full_time(mtime, offset)[..10].to_string(),
    }
}
