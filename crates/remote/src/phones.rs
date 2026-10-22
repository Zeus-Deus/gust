//! Phones and cameras plugged in over USB (DESIGN §6.13.1).
//!
//! An iPhone, iPad or Android phone is not a disk: `lsblk` never lists it.
//! The kernel and udev already know it is there, so Gust reads what they
//! wrote (sysfs and the udev database) and contacts nothing to find it. The
//! phone is reached only when the user clicks it, through GVfs (`gio mount`),
//! the same layer Nautilus uses, so one program owns the USB device.
//!
//! iPhone/iPad: usbmuxd (pulled in by gvfs-afc) is started by udev on
//! plug-in and makes the phone ask "Trust This Computer?" by itself. Until
//! the user unlocks it and taps Trust, `gio mount` asks a question on its
//! stdin ("Try again" / "Cancel"); Gust answers it quietly and reports the
//! state in words. Photos are at the AFC root (`afc://<udid>/DCIM`), files
//! shared by apps at `afc://<udid>:3/`.
//!
//! Android: the user must pick "File transfer" on the phone; until then it
//! offers no MTP interface and does not appear. Locked phones list nothing.
//!
//! Names from a phone are untrusted text: shown escaped, and only ever passed
//! to a program as one argv element.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What kind of thing is plugged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneKind {
    /// iPhone, iPad, iPod touch (AFC over usbmuxd).
    Apple,
    /// An Android phone or media player in File transfer mode (MTP).
    Android,
    /// A camera, or a phone set to PTP (gphoto2).
    Camera,
}

impl PhoneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PhoneKind::Apple => "apple",
            PhoneKind::Android => "android",
            PhoneKind::Camera => "camera",
        }
    }
    /// The GVfs backend this kind needs, as its `.mount` file name.
    fn backend(self) -> &'static str {
        match self {
            PhoneKind::Apple => "afc",
            PhoneKind::Android => "mtp",
            PhoneKind::Camera => "gphoto2",
        }
    }
    /// The Arch package that provides that backend.
    pub fn package(self) -> &'static str {
        match self {
            PhoneKind::Apple => "gvfs-afc",
            PhoneKind::Android => "gvfs-mtp",
            PhoneKind::Camera => "gvfs-gphoto2",
        }
    }
}

/// One phone or camera, as the system describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phone {
    pub kind: PhoneKind,
    /// The model the device reports over USB ("iPhone", "Pixel 8").
    pub model: String,
    /// The device's serial as USB reports it (the UDID on Apple devices).
    pub serial: String,
    /// What GVfs mounts: `afc://<udid>/`, `mtp://<ID_SERIAL>/`, …
    pub uri: String,
    /// Apple only: the files apps share (`afc://<udid>:3/`).
    pub apps_uri: Option<String>,
}

impl Phone {
    /// A stable key for this device in the UI and in QA.
    pub fn id(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.serial)
    }
}

/// Apple's USB vendor id.
const APPLE: &str = "05ac";

/// Read every plugged-in phone and camera from sysfs and the udev database.
/// Local file reads only: nothing is sent to any device.
pub fn scan() -> Vec<Phone> {
    let sys = std::env::var_os("GUST_SYSFS_USB").map(PathBuf::from);
    let udev = std::env::var_os("GUST_UDEV_DATA").map(PathBuf::from);
    scan_in(
        sys.as_deref().unwrap_or(Path::new("/sys/bus/usb/devices")),
        udev.as_deref().unwrap_or(Path::new("/run/udev/data")),
    )
}

/// The folder whose changes mean a USB device came or went: watch it to know
/// when to [`scan`] again (a test may point it elsewhere).
pub fn watch_dir() -> PathBuf {
    std::env::var_os("GUST_USB_WATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/dev/bus/usb"))
}

/// [`scan`] against given roots, so it is tested on a fake tree.
pub fn scan_in(sys_usb: &Path, udev_data: &Path) -> Vec<Phone> {
    let Ok(rd) = std::fs::read_dir(sys_usb) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut names: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    names.sort();
    for dev in names {
        // Interfaces ("1-1:1.0") have no idVendor; only devices do.
        let read = |f: &str| {
            std::fs::read_to_string(dev.join(f))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let Some(vendor) = read("idVendor") else {
            continue;
        };
        let (Some(bus), Some(num)) = (read("busnum"), read("devnum")) else {
            continue;
        };
        let (Ok(bus), Ok(num)) = (bus.parse::<u32>(), num.parse::<u32>()) else {
            continue;
        };
        // udev keys a USB device by its char device number: major 189,
        // minor (bus-1)*128 + (dev-1).
        let minor = (bus.saturating_sub(1)) * 128 + num.saturating_sub(1);
        let props = std::fs::read_to_string(udev_data.join(format!("c189:{minor}")))
            .map(|s| parse_udev_props(&s))
            .unwrap_or_default();
        let prop = |k: &str| {
            props
                .iter()
                .find(|(a, _)| a == k)
                .map(|(_, v)| v.clone())
                .filter(|v| !v.is_empty())
        };
        let serial = read("serial")
            .or_else(|| prop("ID_SERIAL_SHORT"))
            .unwrap_or_default();
        let model = read("product")
            .or_else(|| prop("ID_MODEL").map(|m| m.replace('_', " ")))
            .unwrap_or_else(|| "Phone".into());
        if let Some(p) = classify(&vendor, &serial, &model, &prop) {
            out.push(p);
        }
    }
    out
}

fn classify(
    vendor: &str,
    serial: &str,
    model: &str,
    prop: &dyn Fn(&str) -> Option<String>,
) -> Option<Phone> {
    let model = gust_core::location::escape_for_display(std::ffi::OsStr::new(model));
    if vendor.eq_ignore_ascii_case(APPLE)
        && (prop("USBMUX_SUPPORTED").as_deref() == Some("1") || looks_like_ios(&model))
    {
        let udid = udid_from_serial(serial)?;
        return Some(Phone {
            kind: PhoneKind::Apple,
            model,
            serial: udid.clone(),
            uri: format!("afc://{udid}/"),
            apps_uri: Some(format!("afc://{udid}:3/")),
        });
    }
    if prop("ID_MTP_DEVICE").as_deref() == Some("1") {
        let id = prop("ID_SERIAL").filter(|s| safe_host(s))?;
        return Some(Phone {
            kind: PhoneKind::Android,
            model,
            serial: id.clone(),
            uri: format!("mtp://{id}/"),
            apps_uri: None,
        });
    }
    if prop("ID_GPHOTO2").is_some() {
        let id = prop("ID_SERIAL").filter(|s| safe_host(s))?;
        return Some(Phone {
            kind: PhoneKind::Camera,
            model,
            serial: id.clone(),
            uri: format!("gphoto2://{id}/"),
            apps_uri: None,
        });
    }
    None
}

fn looks_like_ios(model: &str) -> bool {
    ["iPhone", "iPad", "iPod"]
        .iter()
        .any(|m| model.starts_with(m))
}

/// An Apple USB serial is the UDID, but newer devices hyphenate it in the
/// UDID (`00008030-000A1B2C3D4E5F60`) and not in USB (`00008030000A1B2C3D4E5F60`).
/// Anything that is not plain hex is refused: it goes into a URI.
pub fn udid_from_serial(serial: &str) -> Option<String> {
    let s = serial.trim();
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let plain: String = s.chars().filter(|c| *c != '-').collect();
    match plain.len() {
        24 => Some(format!("{}-{}", &plain[..8], &plain[8..]).to_uppercase()),
        40 => Some(plain.to_lowercase()),
        _ => Some(s.to_string()),
    }
}

/// A host part of a GVfs URI: letters, digits and `_ - .` only.
fn safe_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `E:KEY=value` lines of a udev database file.
pub fn parse_udev_props(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.strip_prefix("E:"))
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Is the GVfs backend for this kind installed?
pub fn backend_installed(kind: PhoneKind) -> bool {
    let dir = std::env::var_os("GUST_GVFS_MOUNTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/gvfs/mounts"));
    dir.join(format!("{}.mount", kind.backend())).exists()
}

/// The device's own name ("Alex's iPhone"), from GVfs's list of volumes.
/// `gio mount -li` reads the monitors' state: it does not talk to the phone.
pub fn friendly_names() -> Vec<(String, String)> {
    let Ok(mut child) = gio()
        .args(["mount", "-li"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let mut text = String::new();
    if let Some(mut o) = child.stdout.take() {
        // Bounded: a wedged monitor must not hold a worker for ever.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = o.read_to_string(&mut s);
            let _ = tx.send(s);
        });
        text = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    }
    let _ = child.kill();
    gust_core::stall::reap_in_background(child);
    parse_volume_names(&text)
}

/// `(serial without hyphens, upper-case; name)` for each phone volume in
/// `gio mount -li` output. AFC names its volume "Documents on <name>".
pub fn parse_volume_names(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut label: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Volume(") {
            label = rest.split_once("): ").map(|(_, l)| l.trim().to_string());
            continue;
        }
        if !line.starts_with(' ') {
            label = None;
            continue;
        }
        let t = line.trim();
        let Some(l) = label.as_ref() else { continue };
        let root = t
            .strip_prefix("activation_root=")
            .or_else(|| t.strip_prefix("uuid="));
        if let Some(r) = root {
            let host = r
                .trim_start_matches("afc://")
                .trim_start_matches("mtp://")
                .trim_start_matches("gphoto2://");
            let host = host.split([':', '/']).next().unwrap_or("");
            let key: String = host
                .chars()
                .filter(|c| *c != '-')
                .collect::<String>()
                .to_uppercase();
            let name = l.strip_prefix("Documents on ").unwrap_or(l).to_string();
            if !key.is_empty() && !out.iter().any(|(k, _): &(String, String)| *k == key) {
                out.push((key, name));
            }
        }
    }
    out
}

/// The name to show for a phone: its own name when GVfs knows it.
pub fn display_name(p: &Phone, names: &[(String, String)]) -> String {
    let key: String = p
        .serial
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_uppercase();
    // An exact serial first: an iPhone shows up twice, once as a camera
    // (gphoto2, named just "iPhone") and once over AFC with its own name,
    // and only the AFC volume's key is exactly the UDID.
    names
        .iter()
        .find(|(k, _)| *k == key)
        .or_else(|| {
            names
                .iter()
                .find(|(k, _)| k.contains(&key) || (!k.is_empty() && key.contains(k.as_str())))
        })
        .map(|(_, n)| gust_core::location::escape_for_display(std::ffi::OsStr::new(n)))
        .unwrap_or_else(|| p.model.clone())
}

/// Where a phone stands, said in words a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneState {
    /// Plugged in, not opened yet.
    Ready,
    /// The GVfs backend for it is not installed.
    NeedsPackage(&'static str),
    /// Connecting.
    Opening,
    /// The iPhone has not been trusted yet, or is locked: Gust keeps asking.
    WaitingForTrust,
    WaitingForUnlock,
    /// The user tapped "Don't Trust".
    Refused,
    /// Open at this folder.
    Open(PathBuf),
    /// It did not open; why.
    Failed(String),
}

impl PhoneState {
    pub fn key(&self) -> &'static str {
        match self {
            PhoneState::Ready => "ready",
            PhoneState::NeedsPackage(_) => "needs-package",
            PhoneState::Opening => "opening",
            PhoneState::WaitingForTrust => "trust",
            PhoneState::WaitingForUnlock => "unlock",
            PhoneState::Refused => "refused",
            PhoneState::Open(_) => "open",
            PhoneState::Failed(_) => "failed",
        }
    }
    /// One calm line for the sidebar.
    pub fn words(&self, kind: PhoneKind) -> String {
        let what = match kind {
            PhoneKind::Apple => "Apple device",
            PhoneKind::Android => "phone",
            PhoneKind::Camera => "camera",
        };
        match self {
            PhoneState::Ready => "Click to open".into(),
            PhoneState::NeedsPackage(_) => "Set up device".into(),
            PhoneState::Opening => "Opening…".into(),
            PhoneState::WaitingForTrust => format!("Unlock your {what} and tap Trust"),
            PhoneState::WaitingForUnlock => format!("Unlock your {what} to continue"),
            PhoneState::Refused => {
                format!("This {what} said no. Unplug it and plug it in to be asked again")
            }
            PhoneState::Open(_) => "Open · right-click to eject".into(),
            PhoneState::Failed(w) => w.clone(),
        }
    }
}

fn gio() -> Command {
    // A test points this at a stand-in that speaks gio's protocol.
    let prog = std::env::var_os("GUST_GIO").unwrap_or_else(|| "gio".into());
    let mut c = Command::new(prog);
    // gio's own text is read for a few fixed English phrases; C keeps them
    // fixed whatever the user's language (the phone's name is never parsed).
    c.env("LC_ALL", "C").env("LANGUAGE", "C");
    c
}

/// Mount a phone through GVfs and return the folder to browse.
///
/// Blocking, bounded by `timeout`; call on a worker. `progress` is told when
/// the phone is waiting for the user (Trust, unlock), and `cancelled` is
/// polled so the user can give up. The trust question is answered "Try again"
/// every few seconds until it works, the phone says no, or time runs out.
pub fn mount(
    phone: &Phone,
    timeout: Duration,
    progress: &mut dyn FnMut(PhoneState),
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, PhoneState> {
    if !backend_installed(phone.kind) {
        return Err(PhoneState::NeedsPackage(phone.kind.package()));
    }
    if let Some(p) = local_path(&phone.uri) {
        return Ok(p);
    }
    let mut child = gio()
        .args(["mount", "--"])
        .arg(&phone.uri)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PhoneState::Failed(format!("Could not start gio: {e}")))?;
    let mut stdin = child.stdin.take();
    // Read stdout on a thread: questions arrive without a final newline
    // ("Choice: "), so lines are split on the choice list instead.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Some(o) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut r = BufReader::new(o);
            let mut line = String::new();
            while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                let _ = tx.send(std::mem::take(&mut line));
            }
        });
    }
    let err_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(mut e) = child.stderr.take() {
        let et = err_text.clone();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            if let Ok(mut g) = et.lock() {
                *g = s;
            }
        });
    }
    let started = Instant::now();
    let mut said = String::new();
    // When the user stops waiting, gio is told Cancel and given a moment to
    // pass it on and exit; it is killed only if it does not.
    let mut give_up_at: Option<Instant> = None;
    loop {
        while let Ok(l) = rx.try_recv() {
            said.push_str(&l);
            // "[1] Try again" ends the question block: answer it.
            if l.trim_start().starts_with("[1]") {
                let waiting = if said.contains("is locked") {
                    PhoneState::WaitingForUnlock
                } else {
                    PhoneState::WaitingForTrust
                };
                progress(waiting);
                said.clear();
                // Give the person time to reach the phone before asking again.
                let until = Instant::now() + Duration::from_secs(3);
                while Instant::now() < until && !cancelled() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let answer = if cancelled() { "2\n" } else { "1\n" };
                if let Some(s) = stdin.as_mut() {
                    let _ = s.write_all(answer.as_bytes());
                    let _ = s.flush();
                }
                if cancelled() && give_up_at.is_none() {
                    give_up_at = Some(Instant::now() + Duration::from_secs(2));
                }
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                std::thread::sleep(Duration::from_millis(50));
                let err = err_text.lock().map(|g| g.clone()).unwrap_or_default();
                if status.success() {
                    return local_path(&phone.uri).ok_or_else(|| {
                        PhoneState::Failed("It opened, but Gust could not find its folder.".into())
                    });
                }
                if cancelled() {
                    return Err(PhoneState::Ready);
                }
                return Err(classify_failure(&err, phone.kind));
            }
            Ok(None) => {}
            Err(e) => return Err(PhoneState::Failed(e.to_string())),
        }
        if cancelled() && give_up_at.is_none() {
            // Not mid-question: closing stdin makes gio's next question
            // read end-of-file, which it takes as Cancel.
            stdin = None;
            give_up_at = Some(Instant::now() + Duration::from_secs(2));
        }
        let gave_up = give_up_at.is_some_and(|t| Instant::now() >= t);
        if started.elapsed() >= timeout || gave_up {
            let _ = child.kill();
            gust_core::stall::reap_in_background(child);
            return Err(if cancelled() {
                PhoneState::Ready
            } else {
                PhoneState::Failed(
                    "The phone did not answer in time. Unlock it and try again.".into(),
                )
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// gio's error text, in the C locale, to a state the user can act on.
pub fn classify_failure(err: &str, kind: PhoneKind) -> PhoneState {
    let e = err.to_lowercase();
    if e.contains("refused to trust") {
        return PhoneState::Refused;
    }
    if e.contains("not trusted") {
        return PhoneState::WaitingForTrust;
    }
    if e.contains("password protected") || e.contains("is locked") {
        return PhoneState::WaitingForUnlock;
    }
    if kind == PhoneKind::Android
        && (e.contains("no mtp devices")
            || e.contains("unable to open mtp")
            || e.contains("no storage"))
    {
        return PhoneState::Failed(
            "Unlock your phone, then choose File transfer in its USB notification".into(),
        );
    }
    let last = err
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("It could not be opened.")
        .trim();
    let cleaned = last
        .strip_prefix("gio: ")
        .map(|s| s.split_once(": ").map(|(_, b)| b).unwrap_or(s))
        .unwrap_or(last);
    PhoneState::Failed(cleaned.to_string())
}

/// The local folder GVfs exposes for a mounted URI, or None if it is not
/// mounted. `gio info` answers from the GVfs daemon.
pub fn local_path(uri: &str) -> Option<PathBuf> {
    let out = gio()
        .args(["info", "-a", "standard::name", "--"])
        .arg(uri)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_local_path(&String::from_utf8_lossy(&out.stdout))
}

/// The `local path:` line of `gio info` output.
pub fn parse_local_path(text: &str) -> Option<PathBuf> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("local path:"))
        .map(|p| PathBuf::from(p.trim()))
        .filter(|p| p.is_absolute())
}

/// Every place a phone can be open at: its photos, and an iPhone's app files.
pub fn uris(phone: &Phone) -> Vec<String> {
    let mut v = vec![phone.uri.clone()];
    if let Some(a) = &phone.apps_uri {
        v.push(a.clone());
    }
    v
}

/// Unmount a phone. Never forced: a phone is what people copy photos from,
/// and a forced unmount can cut a copy short.
pub fn unmount(phone: &Phone) -> Result<(), String> {
    let out = gio()
        .args(["mount", "-u", "--"])
        .arg(&phone.uri)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(
            match classify_failure(&String::from_utf8_lossy(&out.stderr), phone.kind) {
                PhoneState::Failed(w) => format!("Could not eject: {w}"),
                other => other.words(phone.kind),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_support_points_to_device_setup_without_a_terminal_command() {
        for kind in [PhoneKind::Apple, PhoneKind::Android, PhoneKind::Camera] {
            assert_eq!(
                PhoneState::NeedsPackage(kind.package()).words(kind),
                "Set up device"
            );
        }
    }

    #[test]
    fn apple_trust_unlock_and_refusal_cover_all_apple_devices() {
        for state in [
            PhoneState::WaitingForTrust,
            PhoneState::WaitingForUnlock,
            PhoneState::Refused,
        ] {
            let words = state.words(PhoneKind::Apple);
            assert!(words.contains("Apple device"), "{words}");
            assert!(!words.contains("iPhone"), "{words}");
        }
        assert_eq!(
            PhoneState::WaitingForUnlock.words(PhoneKind::Android),
            "Unlock your phone to continue"
        );
        assert_eq!(
            PhoneState::WaitingForUnlock.words(PhoneKind::Camera),
            "Unlock your camera to continue"
        );
    }

    fn fake_device(
        sys: &Path,
        udev: &Path,
        name: &str,
        files: &[(&str, &str)],
        bus: u32,
        dev: u32,
        props: &str,
    ) {
        let d = sys.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            std::fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
        std::fs::write(d.join("busnum"), format!("{bus}\n")).unwrap();
        std::fs::write(d.join("devnum"), format!("{dev}\n")).unwrap();
        std::fs::create_dir_all(udev).unwrap();
        let minor = (bus - 1) * 128 + dev - 1;
        std::fs::write(udev.join(format!("c189:{minor}")), props).unwrap();
    }

    fn tree(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("gust-phones-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (root.clone(), root.join("sys"), root.join("udev"))
    }

    #[test]
    fn an_iphone_an_android_phone_and_a_camera_are_found_and_nothing_else() {
        let (root, sys, udev) = tree("scan");
        fake_device(
            &sys,
            &udev,
            "1-1",
            &[
                ("idVendor", "05ac"),
                ("idProduct", "12a8"),
                ("product", "iPhone"),
                ("serial", "00008030000A1B2C3D4E5F60"),
            ],
            1,
            5,
            "E:USBMUX_SUPPORTED=1\nE:ID_SERIAL=Apple_Inc._iPhone_00008030000A1B2C3D4E5F60\n",
        );
        fake_device(&sys, &udev, "1-2", &[("idVendor", "04e8"), ("product", "SAMSUNG_Android"),
            ("serial", "TESTSERIAL01")], 1, 7,
            "E:ID_MTP_DEVICE=1\nE:ID_MEDIA_PLAYER=1\nE:ID_SERIAL=SAMSUNG_SAMSUNG_Android_TESTSERIAL01\n");
        fake_device(
            &sys,
            &udev,
            "2-1",
            &[("idVendor", "04a9"), ("product", "Canon EOS")],
            2,
            3,
            "E:ID_GPHOTO2=1\nE:ID_SERIAL=Canon_Inc._Canon_EOS_123\n",
        );
        // A keyboard and a USB stick are not phones.
        fake_device(
            &sys,
            &udev,
            "2-2",
            &[("idVendor", "046d"), ("product", "Keyboard")],
            2,
            4,
            "E:ID_INPUT=1\n",
        );
        fake_device(
            &sys,
            &udev,
            "2-3",
            &[("idVendor", "0781"), ("product", "Cruzer")],
            2,
            5,
            "E:ID_USB_INTERFACES=:080650:\n",
        );
        // An interface directory has no idVendor.
        std::fs::create_dir_all(sys.join("1-1:1.0")).unwrap();

        let got = scan_in(&sys, &udev);
        assert_eq!(got.len(), 3, "{got:?}");
        let apple = &got[0];
        assert_eq!(apple.kind, PhoneKind::Apple);
        assert_eq!(apple.uri, "afc://00008030-000A1B2C3D4E5F60/");
        assert_eq!(
            apple.apps_uri.as_deref(),
            Some("afc://00008030-000A1B2C3D4E5F60:3/")
        );
        assert_eq!(got[1].kind, PhoneKind::Android);
        assert_eq!(got[1].uri, "mtp://SAMSUNG_SAMSUNG_Android_TESTSERIAL01/");
        assert_eq!(got[2].kind, PhoneKind::Camera);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hostile_serial_never_reaches_a_uri() {
        let (root, sys, udev) = tree("hostile");
        fake_device(
            &sys,
            &udev,
            "1-1",
            &[
                ("idVendor", "05ac"),
                ("product", "iPhone"),
                ("serial", "abc/../../x;rm"),
            ],
            1,
            2,
            "E:USBMUX_SUPPORTED=1\n",
        );
        fake_device(
            &sys,
            &udev,
            "1-2",
            &[("idVendor", "04e8"), ("product", "x")],
            1,
            3,
            "E:ID_MTP_DEVICE=1\nE:ID_SERIAL=a/b c\n",
        );
        assert!(scan_in(&sys, &udev).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_and_new_udids_are_spelled_the_way_afc_spells_them() {
        assert_eq!(
            udid_from_serial("00008030000a1b2c3d4e5f60").unwrap(),
            "00008030-000A1B2C3D4E5F60"
        );
        let old = "A1B2C3D4E5F60718293A4B5C6D7E8F9012345678";
        assert_eq!(udid_from_serial(old).unwrap(), old.to_lowercase());
        assert!(udid_from_serial("").is_none());
    }

    #[test]
    fn the_phones_own_name_comes_from_the_volume_list() {
        // Captured on a real machine (Flea's notes, iOS 26.6.2 and Samsung).
        let text = "Volume(0): iPhone\n  Type: GProxyVolume (GProxyVolumeMonitorGPhoto2)\n  activation_root=gphoto2://Apple_Inc._iPhone_00008030000A1B2C3D4E5F60/\n\
Volume(1): Documents on Alex's iPhone\n  Type: GProxyVolume (GProxyVolumeMonitorAfc)\n  uuid=00008030-000A1B2C3D4E5F60\n  activation_root=afc://00008030-000A1B2C3D4E5F60:3/\n\
Mount(2): Alex's iPhone -> afc://00008030-000A1B2C3D4E5F60/\n\
Volume(0): SAMSUNG Android\n  Type: GProxyVolume (GProxyVolumeMonitorMTP)\n  activation_root=mtp://SAMSUNG_SAMSUNG_Android_TESTSERIAL01/\n";
        let names = parse_volume_names(text);
        let iphone = Phone {
            kind: PhoneKind::Apple,
            model: "iPhone".into(),
            serial: "00008030-000A1B2C3D4E5F60".into(),
            uri: String::new(),
            apps_uri: None,
        };
        assert_eq!(display_name(&iphone, &names), "Alex's iPhone");
        let android = Phone {
            kind: PhoneKind::Android,
            model: "SAMSUNG Android".into(),
            serial: "SAMSUNG_SAMSUNG_Android_TESTSERIAL01".into(),
            uri: String::new(),
            apps_uri: None,
        };
        assert_eq!(display_name(&android, &names), "SAMSUNG Android");
        // Unknown device: the USB model.
        let other = Phone {
            serial: "FFFF".into(),
            ..iphone
        };
        assert_eq!(display_name(&other, &names), "iPhone");
    }

    #[test]
    fn gio_failures_become_states_a_person_can_act_on() {
        let k = PhoneKind::Apple;
        assert_eq!(
            classify_failure("gio: afc://x/: User refused to trust this computer\n", k),
            PhoneState::Refused
        );
        assert_eq!(
            classify_failure("gio: afc://x/: The user has not trusted this computer\n", k),
            PhoneState::WaitingForTrust
        );
        assert_eq!(
            classify_failure("gio: afc://x/: The device is password protected\n", k),
            PhoneState::WaitingForUnlock
        );
        match classify_failure("gio: mtp://x/: No MTP devices found\n", PhoneKind::Android) {
            PhoneState::Failed(w) => assert!(w.contains("File transfer"), "{w}"),
            o => panic!("{o:?}"),
        }
        match classify_failure("gio: afc://x/: Something else broke\n", k) {
            PhoneState::Failed(w) => assert_eq!(w, "Something else broke"),
            o => panic!("{o:?}"),
        }
        assert!(PhoneState::WaitingForTrust.words(k).contains("tap Trust"));
    }

    /// The whole conversation with a stand-in gio that asks the iPhone's
    /// Trust question until a file says it was tapped.
    #[test]
    fn mount_answers_trust_until_tapped_and_cancel_says_cancel() {
        let root = std::env::temp_dir().join(format!("gust-phones-mount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mounts = root.join("mounts");
        std::fs::create_dir_all(&mounts).unwrap();
        std::fs::write(mounts.join("afc.mount"), "").unwrap();
        let log = root.join("log");
        let mnt = root.join("mnt");
        let gio = root.join("gio");
        std::fs::write(
            &gio,
            format!(
                r#"#!/usr/bin/env bash
T="{root}"
case "$1 $2" in
  "info -a") [[ -e "$T/mounted" ]] || exit 1; printf 'local path: %s\n' "{mnt}"; exit 0 ;;
  "mount --")
    echo "mount $3" >> "$T/log"
    while [[ ! -e "$T/trusted" ]]; do
      printf 'Untrusted Device\nThe device is not trusted yet.\n\nSelect Trust and click Try again.\n[1] Try again\n[2] Cancel\nChoice: '
      read -r ans || exit 1
      echo "answer $ans" >> "$T/log"
      [[ "$ans" == 2 ]] && {{ echo "gio: x: Operation was cancelled" >&2; exit 1; }}
    done
    touch "$T/mounted"; exit 0 ;;
esac
exit 3
"#,
                root = root.display(),
                mnt = mnt.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gio, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("GUST_GIO", &gio);
        std::env::set_var("GUST_GVFS_MOUNTS", &mounts);
        let phone = Phone {
            kind: PhoneKind::Apple,
            model: "iPhone".into(),
            serial: "X".into(),
            uri: "afc://X/".into(),
            apps_uri: None,
        };

        // Trust is tapped while Gust keeps answering "Try again".
        let r2 = root.clone();
        let tapper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(4500));
            std::fs::write(r2.join("trusted"), "").unwrap();
        });
        let mut states = Vec::new();
        let got = mount(
            &phone,
            Duration::from_secs(30),
            &mut |s| states.push(s),
            &|| false,
        );
        tapper.join().unwrap();
        assert_eq!(got.unwrap(), mnt);
        assert_eq!(
            states.first(),
            Some(&PhoneState::WaitingForTrust),
            "{states:?}"
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("answer 1"), "{text}");

        // Waiting is stopped: gio is told Cancel, not killed mid-question.
        std::fs::remove_file(root.join("trusted")).unwrap();
        std::fs::remove_file(root.join("mounted")).unwrap();
        std::fs::write(&log, "").unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        let got = mount(
            &phone,
            Duration::from_secs(30),
            &mut |_| s2.store(true, std::sync::atomic::Ordering::SeqCst),
            &|| stop.load(std::sync::atomic::Ordering::SeqCst),
        );
        assert_eq!(got.unwrap_err(), PhoneState::Ready);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("answer 2"), "{text}");

        std::env::remove_var("GUST_GIO");
        std::env::remove_var("GUST_GVFS_MOUNTS");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gio_info_names_the_local_folder() {
        let t = "display name: Alex's iPhone\nname: /\nlocal path: /run/user/1000/gvfs/afc:host=00008030-000A1B2C3D4E5F60\nuri: afc://x/\n";
        assert_eq!(
            parse_local_path(t).unwrap(),
            PathBuf::from("/run/user/1000/gvfs/afc:host=00008030-000A1B2C3D4E5F60")
        );
        assert!(parse_local_path("name: x\n").is_none());
    }
}
