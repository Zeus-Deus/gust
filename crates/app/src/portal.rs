//! `gust --portal`: the file-chooser backend for xdg-desktop-portal.
//!
//! When an app shows an Open or Save dialog through the portal, the portal
//! calls `org.freedesktop.impl.portal.FileChooser` here. Each call starts a
//! Gust window in chooser mode (GUST_CHOOSER=open|open-many|open-folder|save)
//! and waits for it on its own thread, so several dialogs can be open at
//! once. The window writes the pick to a private result file; nothing
//! written means the user cancelled. `Request.Close` on the call's handle
//! ends that window (DESIGN §6.27).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::channel::{MatchingReceiver, Sender};
use dbus::Message;

const NAME: &str = "org.freedesktop.impl.portal.desktop.gust";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const REQUEST_IFACE: &str = "org.freedesktop.impl.portal.Request";

/// Open calls by their request handle, with the bus name that made each one:
/// `Close` reaches the right window, and only from that caller.
type Calls = Arc<Mutex<HashMap<String, (String, Arc<AtomicBool>)>>>;

// A longer title or label would not fit one environment string, and nobody
// reads that much in a dialog.
const MAX_TITLE: usize = 4096;
const MAX_LABEL: usize = 256;

fn bounded(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// The options the chooser uses, read with typed reads straight from the
/// message. (Reading them through `PropMap`/`RefArg` mangles an `ay` inside
/// a variant in dbus 0.9: the length survives but the bytes do not, so the
/// requested start folder came out as garbage.)
#[derive(Default)]
struct Options {
    directory: bool,
    multiple: bool,
    current_folder: Option<Vec<u8>>,
    current_name: Option<String>,
    current_file: Option<Vec<u8>>,
    accept_label: Option<String>,
    files: Vec<Vec<u8>>,
    filters: Vec<gust_core::chooser::RawFilter>,
    current_filter: Option<gust_core::chooser::RawFilter>,
}

fn read_options(it: &mut dbus::arg::Iter) -> Option<Options> {
    use dbus::arg::ArgType;
    let mut o = Options::default();
    let mut dict = it.recurse(ArgType::Array)?;
    while dict.arg_type() == ArgType::DictEntry {
        if let Some(mut entry) = dict.recurse(ArgType::DictEntry) {
            let key: String = entry.read().ok()?;
            if let Some(mut var) = entry.recurse(ArgType::Variant) {
                match key.as_str() {
                    "directory" => o.directory = var.read::<bool>().unwrap_or(false),
                    "multiple" => o.multiple = var.read::<bool>().unwrap_or(false),
                    "current_folder" => o.current_folder = var.read::<Vec<u8>>().ok(),
                    "current_name" => o.current_name = var.read::<String>().ok(),
                    "current_file" => o.current_file = var.read::<Vec<u8>>().ok(),
                    "accept_label" => o.accept_label = var.read::<String>().ok(),
                    "files" => o.files = var.read::<Vec<Vec<u8>>>().unwrap_or_default(),
                    "filters" => o.filters = var.read().unwrap_or_default(),
                    "current_filter" => o.current_filter = var.read().ok(),
                    _ => {}
                }
            }
        }
        if !dict.next() {
            break;
        }
    }
    Some(o)
}

/// `shared-mime-info`'s glob lists, read here so the window never has to.
fn read_globs2() -> String {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => dirs.push(d.into()),
        None => dirs.extend(
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")),
        ),
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    dirs.extend(system.split(':').map(std::path::PathBuf::from));
    dirs.iter()
        .filter_map(|d| std::fs::read_to_string(d.join("mime/globs2")).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run one chooser window to completion; returns the picked paths, or None
/// when no window could start. That must not read as the user cancelling.
fn choose(
    req: gust_core::chooser::Request,
    title: &str,
    closed: &AtomicBool,
) -> Option<Vec<std::path::PathBuf>> {
    let exe = std::env::current_exe().ok()?;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("gust");
    std::fs::create_dir_all(&dir).ok()?;
    let result = dir.join(format!(
        "chooser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&result);
    let mut cmd = std::process::Command::new(exe);
    if let Some(f) = &req.folder {
        cmd.arg(f);
    }
    cmd.env("GUST_CHOOSER", req.mode.as_str())
        .env("GUST_CHOOSER_RESULT", &result)
        .env("GUST_CHOOSER_TITLE", bounded(title, MAX_TITLE))
        .env_remove("GUST_SELECT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match &req.name {
        Some(n) => cmd.env("GUST_CHOOSER_NAME", n),
        None => cmd.env_remove("GUST_CHOOSER_NAME"),
    };
    match &req.accept {
        Some(label) => cmd.env("GUST_CHOOSER_ACCEPT", bounded(label, MAX_LABEL)),
        None => cmd.env_remove("GUST_CHOOSER_ACCEPT"),
    };
    if req.filters.items.is_empty() {
        cmd.env_remove("GUST_CHOOSER_FILTERS");
    } else {
        cmd.env("GUST_CHOOSER_FILTERS", req.filters.to_json());
    }
    if req.files.is_empty() {
        cmd.env_remove("GUST_CHOOSER_FILES");
    } else {
        cmd.env(
            "GUST_CHOOSER_FILES",
            gust_core::chooser::files_to_env(&req.files),
        );
    }
    let mut child = cmd.spawn().ok()?;
    let status = loop {
        if closed.load(Ordering::SeqCst) {
            // The app gave up on the dialog: the window goes with it, and
            // anything it picked in that instant no longer has anywhere to go.
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&result);
            return Some(Vec::new());
        }
        match child.try_wait() {
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Ok(Some(status)) => break Some(status),
            Err(_) => break None,
        }
    };
    let picked = gust_core::chooser::parse_result(&result);
    let _ = std::fs::remove_file(&result);
    // Cancel is a clean exit with nothing picked. A window that failed to
    // start exits with an error, and that must not read as the user's choice.
    let finished = status.is_some_and(|s| s.success());
    (finished || !picked.is_empty()).then_some(picked)
}

fn handle(msg: &Message, kind: &str, closed: &AtomicBool) -> (u32, PropMap) {
    // (o handle, s app_id, s parent_window, s title, a{sv} options)
    let mut it = msg.iter_init();
    let (Some(_), Some(_app), Some(_parent), Some(title)) = (
        it.read::<dbus::Path>().ok(),
        it.read::<String>().ok(),
        it.read::<String>().ok(),
        it.read::<String>().ok(),
    ) else {
        return (2, PropMap::new());
    };
    let Some(options) = read_options(&mut it) else {
        return (2, PropMap::new());
    };
    if std::env::var_os("GUST_PORTAL_DEBUG").is_some() {
        eprintln!(
            "gust portal: dir={} multiple={} folder={:?} name={:?}",
            options.directory,
            options.multiple,
            options
                .current_folder
                .as_deref()
                .map(String::from_utf8_lossy),
            options.current_name
        );
    }
    let mut req = gust_core::chooser::Request::new(
        kind,
        options.directory,
        options.multiple,
        options.current_folder,
        options.current_name,
    )
    .with_current_file(options.current_file);
    if kind == "save-many" {
        let Some(with_files) = req.with_files(options.files) else {
            return (2, PropMap::new());
        };
        req = with_files;
    }
    req.accept = options
        .accept_label
        .as_deref()
        .map(gust_core::chooser::plain_label)
        .filter(|label| !label.is_empty());
    let wants_mime = options
        .filters
        .iter()
        .chain(options.current_filter.iter())
        .flat_map(|(_, rules)| rules)
        .any(|(kind, _)| *kind == 1);
    let globs2 = if wants_mime {
        read_globs2()
    } else {
        String::new()
    };
    req.filters =
        gust_core::chooser::Filters::expand(options.filters, options.current_filter, &globs2);
    let Some(picked) = choose(req, &title, closed) else {
        return (2, PropMap::new());
    };
    let mut results = PropMap::new();
    if closed.load(Ordering::SeqCst) {
        // 2 = ended some other way: the app itself closed the request.
        return (2, results);
    }
    if picked.is_empty() {
        // 1 = cancelled by the user.
        return (1, results);
    }
    let uris = gust_core::chooser::answer_uris(&picked);
    results.insert("uris".into(), Variant(Box::new(uris) as Box<dyn RefArg>));
    (0, results)
}

pub fn run() -> i32 {
    let Ok(conn) = Connection::new_session() else {
        eprintln!("gust: no session bus");
        return 1;
    };
    if conn.request_name(NAME, false, true, true).is_err() {
        eprintln!("gust: {NAME} is already owned");
        return 1;
    }
    // Replies are sent from worker threads through this channel, because a
    // chooser window can stay open for minutes and must not block others.
    let (tx, rx) = mpsc::channel::<Message>();
    let pending: Arc<std::sync::atomic::AtomicUsize> = Default::default();
    let calls: Calls = Default::default();
    {
        let pending = pending.clone();
        conn.start_receive(
            dbus::message::MatchRule::new_method_call(),
            Box::new(move |msg, conn| {
                let member = msg.member().map(|m| m.to_string()).unwrap_or_default();
                if member == "Close" && msg.interface().is_some_and(|i| &*i == REQUEST_IFACE) {
                    let path = msg.path().map(|p| p.to_string()).unwrap_or_default();
                    let sender = msg.sender().map(|s| s.to_string());
                    if let Some((owner, flag)) =
                        calls.lock().ok().and_then(|c| c.get(&path).cloned())
                    {
                        if sender.as_deref() == Some(owner.as_str()) {
                            flag.store(true, Ordering::SeqCst);
                        }
                    }
                    if !msg.get_no_reply() {
                        let _ = conn.send(msg.method_return());
                    }
                    return true;
                }
                let right = msg.interface().is_some_and(|i| &*i == IFACE)
                    && msg.path().is_some_and(|p| &*p == PATH);
                let kind = match member.as_str() {
                    "OpenFile" if right => "open",
                    "SaveFile" if right => "save",
                    "SaveFiles" if right => "save-many",
                    _ => {
                        if msg.get_no_reply() {
                            return true;
                        }
                        let _ = conn.send(msg.error(
                            &"org.freedesktop.DBus.Error.UnknownMethod".into(),
                            &std::ffi::CString::new("not supported").unwrap(),
                        ));
                        return true;
                    }
                };
                let tx = tx.clone();
                let pending = pending.clone();
                let calls = calls.clone();
                let closed = Arc::new(AtomicBool::new(false));
                let handle_path = msg
                    .iter_init()
                    .read::<dbus::Path>()
                    .ok()
                    .map(|p| p.to_string());
                let sender = msg.sender().map(|s| s.to_string()).unwrap_or_default();
                if let (Some(path), Ok(mut open)) = (&handle_path, calls.lock()) {
                    // A handle names one request. A second call with it would
                    // leave the first window impossible to close.
                    if open.contains_key(path) {
                        let _ = conn.send(msg.method_return().append2(2u32, PropMap::new()));
                        return true;
                    }
                    open.insert(path.clone(), (sender, closed.clone()));
                }
                pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    let (code, results) = handle(&msg, kind, &closed);
                    if let (Some(path), Ok(mut open)) = (&handle_path, calls.lock()) {
                        open.remove(path);
                    }
                    let reply = msg.method_return().append2(code, results);
                    let _ = tx.send(reply);
                    pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
                true
            }),
        );
    }
    // Exit after a quiet minute with nothing open; the portal restarts us.
    let mut idle = 0u32;
    loop {
        let _ = conn.process(Duration::from_millis(250));
        while let Ok(reply) = rx.try_recv() {
            let _ = conn.send(reply);
            idle = 0;
        }
        if pending.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            idle += 1;
            if idle >= 240 {
                return 0;
            }
        } else {
            idle = 0;
        }
    }
}
