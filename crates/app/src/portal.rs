//! `gust --portal`: the file-chooser backend for xdg-desktop-portal.
//!
//! When an app shows an Open or Save dialog through the portal, the portal
//! calls `org.freedesktop.impl.portal.FileChooser` here. Each call starts a
//! Gust window in chooser mode (GUST_CHOOSER=open|open-many|open-folder|save).
//! The service watches each child independently and answers when it exits.
//! The caller can close its own chooser through its Request object.

use std::collections::HashMap;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::{stdintf::org_freedesktop_dbus::RequestNameReply, Connection};
use dbus::channel::{self, BusType, Channel};
use dbus::{Message, MessageType};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::process::{pidfd_open, Pid, PidfdFlags};

const NAME: &str = "org.freedesktop.impl.portal.desktop.gust";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.impl.portal.Request";
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const FALLBACK_INTERVAL: Duration = Duration::from_millis(20);

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

type StartChooser = dyn Fn(gust_core::chooser::Request, &str) -> std::io::Result<(Child, PathBuf)>;

fn start_chooser(
    req: gust_core::chooser::Request,
    title: &str,
) -> std::io::Result<(Child, PathBuf)> {
    let exe = std::env::current_exe()?;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("gust");
    std::fs::create_dir_all(&dir)?;
    let result = dir.join(format!(
        "chooser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&result);
    let mut cmd = Command::new(exe);
    if let Some(f) = &req.folder {
        cmd.arg(f);
    }
    cmd.env("GUST_CHOOSER", req.mode.as_str())
        .env("GUST_CHOOSER_RESULT", &result)
        .env("GUST_CHOOSER_TITLE", title)
        .env_remove("GUST_SELECT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match &req.name {
        Some(n) => cmd.env("GUST_CHOOSER_NAME", n),
        None => cmd.env_remove("GUST_CHOOSER_NAME"),
    };
    Ok((cmd.spawn()?, result))
}

type RequestKey = (dbus::strings::BusName<'static>, dbus::Path<'static>);

fn request_key(msg: &Message, path: dbus::Path<'_>) -> Option<RequestKey> {
    Some((msg.sender()?.into_static(), path.into_static()))
}

fn read_request(
    msg: &Message,
    kind: &str,
) -> Option<(RequestKey, gust_core::chooser::Request, String)> {
    // (o handle, s app_id, s parent_window, s title, a{sv} options)
    let mut it = msg.iter_init();
    let key = request_key(msg, it.read::<dbus::Path>().ok()?)?;
    let _app = it.read::<String>().ok()?;
    let _parent = it.read::<String>().ok()?;
    let title = it.read::<String>().ok()?;
    let options = read_options(&mut it)?;
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
    let req = gust_core::chooser::Request::new(
        kind,
        options.directory,
        options.multiple,
        options.current_folder,
        options.current_name,
    );
    Some((key, req, title))
}

fn answer(bus: &Channel, msg: &Message, code: u32, picked: &[PathBuf]) {
    if msg.get_no_reply() {
        return;
    }
    let mut results = PropMap::new();
    if code == 0 {
        let uris = gust_core::chooser::answer_uris(picked);
        results.insert("uris".into(), Variant(Box::new(uris) as Box<dyn RefArg>));
    }
    let _ = bus.send(msg.method_return().append2(code, results));
}

struct Chooser {
    call: Option<Message>,
    child: Child,
    exit_fd: Option<OwnedFd>,
    result: PathBuf,
}

impl Chooser {
    fn cancel(&mut self, bus: &Channel) {
        if let Some(call) = self.call.take() {
            let _ = self.child.kill();
            answer(bus, &call, 1, &[]);
        }
    }

    fn finished(&mut self, bus: &Channel) -> bool {
        let status = self.child.try_wait();
        if matches!(status, Ok(None)) {
            return false;
        }
        if let Some(call) = self.call.take() {
            if matches!(status, Ok(Some(status)) if status.success()) {
                let picked = gust_core::chooser::parse_result(&self.result);
                answer(bus, &call, u32::from(picked.is_empty()), &picked);
            } else {
                answer(bus, &call, 2, &[]);
            }
        }
        true
    }
}

impl Drop for Chooser {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = std::fs::remove_file(&self.result);
    }
}

fn handle(
    bus: &Channel,
    active: &mut HashMap<RequestKey, Chooser>,
    start: &StartChooser,
    msg: Message,
) {
    if msg.msg_type() != MessageType::MethodCall {
        return;
    }
    let iface = msg.interface();
    let member = msg.member().expect("D-Bus method calls have a member");
    if iface.as_deref() == Some(REQUEST) && &*member == "Close" {
        if let Some(chooser) = msg
            .path()
            .and_then(|path| request_key(&msg, path))
            .and_then(|key| active.get_mut(&key))
        {
            if !msg.get_no_reply() {
                let _ = bus.send(msg.method_return());
            }
            chooser.cancel(bus);
            return;
        }
    }
    let kind = match &*member {
        "OpenFile" => Some("open"),
        "SaveFile" => Some("save"),
        _ => None,
    };
    if let Some(kind) =
        kind.filter(|_| iface.as_deref() == Some(IFACE) && msg.path().is_some_and(|p| &*p == PATH))
    {
        let Some((key, request, title)) = read_request(&msg, kind) else {
            answer(bus, &msg, 2, &[]);
            return;
        };
        let std::collections::hash_map::Entry::Vacant(slot) = active.entry(key) else {
            answer(bus, &msg, 2, &[]);
            return;
        };
        let Ok((child, result)) = start(request, &title) else {
            answer(bus, &msg, 2, &[]);
            return;
        };
        let exit_fd = Pid::from_raw(child.id() as i32)
            .and_then(|pid| pidfd_open(pid, PidfdFlags::empty()).ok());
        slot.insert(Chooser {
            call: Some(msg),
            child,
            exit_fd,
            result,
        });
    } else if !msg.get_no_reply() {
        if let Some(reply) = channel::default_reply(&msg) {
            let _ = bus.send(reply);
        }
    }
}

fn serve(bus: &Channel, start: &StartChooser) -> Result<(), ()> {
    let mut active = HashMap::new();
    let mut last_activity = Instant::now();
    loop {
        bus.read_write(Some(Duration::ZERO))?;
        while let Some(msg) = bus.pop_message() {
            handle(bus, &mut active, start, msg);
            last_activity = Instant::now();
        }
        let count = active.len();
        active.retain(|_, chooser| !chooser.finished(bus));
        if count != active.len() {
            last_activity = Instant::now();
        }
        let timeout = if active.is_empty() {
            let remaining = IDLE_TIMEOUT.saturating_sub(last_activity.elapsed());
            if remaining.is_zero() {
                return Ok(());
            }
            Some(remaining)
        } else if active.values().any(|chooser| chooser.exit_fd.is_none()) {
            Some(FALLBACK_INTERVAL)
        } else {
            None
        };

        let watch = bus.watch();
        // The private D-Bus connection owns this socket for the entire poll,
        // and this loop is its sole reader.
        let socket = unsafe { BorrowedFd::borrow_raw(watch.fd) };
        let mut events = PollFlags::IN;
        if watch.write || bus.has_messages_to_send() {
            events |= PollFlags::OUT;
        }
        let mut fds = Vec::with_capacity(1 + active.len());
        fds.push(PollFd::from_borrowed_fd(socket, events));
        fds.extend(active.values().filter_map(|chooser| {
            chooser
                .exit_fd
                .as_ref()
                .map(|fd| PollFd::new(fd, PollFlags::IN))
        }));
        let timeout = timeout.map(|d| Timespec {
            tv_sec: d.as_secs() as i64,
            tv_nsec: d.subsec_nanos().into(),
        });
        if let Err(error) = poll(&mut fds, timeout.as_ref()) {
            if error != rustix::io::Errno::INTR {
                return Err(());
            }
        }
    }
}

pub fn run() -> i32 {
    let Ok(mut bus) = Channel::get_private(BusType::Session) else {
        eprintln!("gust: no session bus");
        return 1;
    };
    bus.set_watch_enabled(true);
    let conn = Connection::from(bus);
    if !matches!(
        conn.request_name(NAME, false, false, true),
        Ok(RequestNameReply::PrimaryOwner)
    ) {
        eprintln!("gust: could not own {NAME}");
        return 1;
    }
    i32::from(serve(conn.channel(), &start_chooser).is_err())
}

#[cfg(test)]
mod tests;
