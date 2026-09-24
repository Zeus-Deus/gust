//! `gust --portal`: the file-chooser backend for xdg-desktop-portal.
//!
//! When an app shows an Open or Save dialog through the portal, the portal
//! calls `org.freedesktop.impl.portal.FileChooser` here. Each call starts a
//! Gust window in chooser mode (GUST_CHOOSER=open|open-many|open-folder|save)
//! and waits for it on its own thread, so several dialogs can be open at
//! once. The window writes the pick to a private result file; nothing
//! written means the user cancelled.

use std::sync::mpsc;
use std::time::Duration;

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::channel::{MatchingReceiver, Sender};
use dbus::Message;

const NAME: &str = "org.freedesktop.impl.portal.desktop.gust";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";

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

/// Run one chooser window to completion; returns the picked paths.
fn choose(req: gust_core::chooser::Request, title: &str) -> Vec<std::path::PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("gust");
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
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
        .env("GUST_CHOOSER_TITLE", title)
        .env_remove("GUST_SELECT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match &req.name {
        Some(n) => cmd.env("GUST_CHOOSER_NAME", n),
        None => cmd.env_remove("GUST_CHOOSER_NAME"),
    };
    let Ok(mut child) = cmd.spawn() else {
        return Vec::new();
    };
    let _ = child.wait();
    let picked = gust_core::chooser::parse_result(&result);
    let _ = std::fs::remove_file(&result);
    picked
}

fn handle(msg: &Message, kind: &str) -> (u32, PropMap) {
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
            options.current_folder.as_deref().map(String::from_utf8_lossy),
            options.current_name
        );
    }
    let req = gust_core::chooser::Request::new(
        kind,
        options.directory,
        options.multiple,
        options.current_folder,
        options.current_name,
    );
    let picked = choose(req, &title);
    let mut results = PropMap::new();
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
    let pending: std::sync::Arc<std::sync::atomic::AtomicUsize> = Default::default();
    {
        let pending = pending.clone();
        conn.start_receive(
            dbus::message::MatchRule::new_method_call(),
            Box::new(move |msg, conn| {
                let member = msg.member().map(|m| m.to_string()).unwrap_or_default();
                let right = msg.interface().is_some_and(|i| &*i == IFACE)
                    && msg.path().is_some_and(|p| &*p == PATH);
                let kind = match member.as_str() {
                    "OpenFile" if right => "open",
                    "SaveFile" if right => "save",
                    _ => {
                        if msg.get_no_reply() {
                            return true;
                        }
                        let _ = conn.send(
                            msg.error(
                                &"org.freedesktop.DBus.Error.UnknownMethod".into(),
                                &std::ffi::CString::new("not supported").unwrap(),
                            ),
                        );
                        return true;
                    }
                };
                let tx = tx.clone();
                let pending = pending.clone();
                pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    let (code, results) = handle(&msg, kind);
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
