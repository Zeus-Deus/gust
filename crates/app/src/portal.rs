//! `gust --portal`: the file-chooser backend for xdg-desktop-portal.
//!
//! When an app shows an Open or Save dialog through the portal, the portal
//! calls `org.freedesktop.impl.portal.FileChooser` here. Each call starts a
//! Gust window in chooser mode (GUST_CHOOSER=open|open-many|open-folder|save).
//! The service watches each child independently and collects its answer on
//! a worker. A chooser ends when its caller closes it or disconnects.

use std::collections::{hash_map::Entry, HashMap};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
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
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
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

struct Window {
    child: Child,
    exit_fd: Option<OwnedFd>,
    result: Option<PathBuf>,
}

impl Drop for Window {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        if let Some(result) = self.result.take() {
            // The service can also exit before this window has finished.
            let _ = std::thread::Builder::new().spawn(move || {
                let _ = std::fs::remove_file(result);
            });
        }
    }
}

type Reply = (u32, Vec<PathBuf>);

struct ResultWorker {
    ready: UnixStream,
    reply: Receiver<Reply>,
}

impl ResultWorker {
    fn start(result: &Path, code: u32) -> std::io::Result<Self> {
        let (ready, notify) = UnixStream::pair()?;
        let (send, reply) = mpsc::channel();
        let result = result.to_owned();
        std::thread::Builder::new()
            .name("gust-chooser-result".into())
            .spawn(move || {
                let (code, picked) = if code == 0 {
                    let picked = gust_core::chooser::parse_result(&result);
                    (u32::from(picked.is_empty()), picked)
                } else {
                    (code, Vec::new())
                };
                let _ = std::fs::remove_file(result);
                let _ = send.send((code, picked));
                drop(send);
                // Closing the peer wakes poll even when the reply was dropped.
                drop(notify);
            })?;
        Ok(Self { ready, reply })
    }
}

enum Progress {
    Window(Window),
    Result(ResultWorker),
}

struct Chooser {
    call: Option<Message>,
    progress: Progress,
}

impl Chooser {
    /// Keep a cancelled window registered until its child can be reaped.
    fn stop(&mut self) -> bool {
        self.call = None;
        if let Progress::Window(window) = &mut self.progress {
            let _ = window.child.kill();
            true
        } else {
            false
        }
    }

    fn finished(&mut self, bus: &Channel) -> bool {
        let (code, picked) = match &mut self.progress {
            Progress::Window(window) => {
                let code = match window.child.try_wait() {
                    Ok(None) => return false,
                    Ok(Some(status)) if status.success() && self.call.is_some() => 0,
                    _ => 2,
                };
                let result = window.result.as_deref().expect("window owns its result");
                if let Ok(worker) = ResultWorker::start(result, code) {
                    window.result = None;
                    self.progress = Progress::Result(worker);
                    return false;
                }
                (2, Vec::new())
            }
            Progress::Result(worker) => match worker.reply.try_recv() {
                Ok(reply) => reply,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => (2, Vec::new()),
            },
        };
        if let Some(call) = self.call.take() {
            answer(bus, &call, code, &picked);
        }
        true
    }

    fn poll_fd(&self) -> Option<PollFd<'_>> {
        match &self.progress {
            Progress::Window(window) => window
                .exit_fd
                .as_ref()
                .map(|fd| PollFd::new(fd, PollFlags::IN)),
            Progress::Result(worker) => Some(PollFd::new(&worker.ready, PollFlags::IN)),
        }
    }
}

fn watch_owners(conn: &Connection) -> Result<(), dbus::Error> {
    let rule = dbus::message::MatchRule::new_signal(DBUS, "NameOwnerChanged")
        .with_sender(DBUS)
        .with_path(DBUS_PATH);
    conn.add_match_no_cb(&rule.match_str())
}

fn disconnected_owner(msg: &Message) -> Option<&str> {
    if msg.msg_type() == MessageType::Signal
        && msg.sender().as_deref() == Some(DBUS)
        && msg.path().as_deref() == Some(DBUS_PATH)
        && msg.interface().as_deref() == Some(DBUS)
        && msg.member().as_deref() == Some("NameOwnerChanged")
    {
        let (name, old, new) = msg.read3::<&str, &str, &str>().ok()?;
        if name.starts_with(':') && old == name && new.is_empty() {
            return Some(name);
        }
    }
    None
}

fn handle(
    bus: &Channel,
    active: &mut HashMap<RequestKey, Chooser>,
    start: &StartChooser,
    msg: Message,
) {
    if let Some(owner) = disconnected_owner(&msg) {
        active.retain(|(sender, _), chooser| &**sender != owner || chooser.stop());
        return;
    }
    if msg.msg_type() != MessageType::MethodCall {
        return;
    }
    let iface = msg.interface();
    let member = msg.member().expect("D-Bus method calls have a member");
    if iface.as_deref() == Some(REQUEST) && &*member == "Close" {
        if let Some(key) = msg.path().and_then(|path| request_key(&msg, path)) {
            if let Entry::Occupied(mut entry) = active.entry(key) {
                if !msg.get_no_reply() {
                    let _ = bus.send(msg.method_return());
                }
                if let Some(call) = entry.get_mut().call.take() {
                    answer(bus, &call, 1, &[]);
                }
                if !entry.get_mut().stop() {
                    entry.remove();
                }
                return;
            }
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
        let Entry::Vacant(slot) = active.entry(key) else {
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
            progress: Progress::Window(Window {
                child,
                exit_fd,
                result: Some(result),
            }),
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
        let count = active.len();
        while let Some(msg) = bus.pop_message() {
            if msg.msg_type() == MessageType::MethodCall {
                last_activity = Instant::now();
            }
            handle(bus, &mut active, start, msg);
        }
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
        } else if active.values().any(|chooser| chooser.poll_fd().is_none()) {
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
        fds.extend(active.values().filter_map(Chooser::poll_fd));
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
    if let Err(error) = watch_owners(&conn) {
        eprintln!("gust: could not watch chooser owners: {error}");
        return 1;
    }
    match conn.request_name(NAME, false, false, true) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        Ok(_) => {
            eprintln!("gust: {NAME} is already owned");
            return 1;
        }
        Err(error) => {
            eprintln!("gust: could not acquire {NAME}: {error}");
            return 1;
        }
    }
    i32::from(serve(conn.channel(), &start_chooser).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbus::blocking::BlockingSender;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::sync::mpsc;

    const TIMEOUT: Duration = Duration::from_secs(10);

    struct Bus(Child);

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn private_bus() -> (Bus, String) {
        let mut daemon = Bus(Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon is required for the portal integration test"));
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        (daemon, address.trim().to_owned())
    }

    fn service(
        address: &str,
        start: impl Fn(gust_core::chooser::Request, &str) -> std::io::Result<(Child, PathBuf)>
            + Send
            + 'static,
    ) -> std::thread::JoinHandle<()> {
        let mut bus = Channel::open_private(address).unwrap();
        bus.register().unwrap();
        bus.set_watch_enabled(true);
        let conn = Connection::from(bus);
        watch_owners(&conn).unwrap();
        assert_eq!(
            conn.request_name(NAME, false, false, true).unwrap(),
            RequestNameReply::PrimaryOwner
        );
        std::thread::spawn(move || assert_eq!(serve(conn.channel(), &start), Err(())))
    }

    fn open(caller: &Connection, path: &str, method: &str) -> u32 {
        let mut msg = Message::new_method_call(NAME, PATH, IFACE, method).unwrap();
        msg.append_all((
            dbus::Path::new(path).unwrap(),
            "test",
            "",
            "Fixture",
            PropMap::new(),
        ));
        caller.channel().send(msg).unwrap()
    }

    fn close(caller: &Connection, path: &str) -> Result<(), dbus::Error> {
        let msg = Message::new_method_call(NAME, path, REQUEST, "Close").unwrap();
        caller.send_with_reply_and_block(msg, TIMEOUT).map(|_| ())
    }

    fn assert_cancelled(caller: &Connection, serial: u32) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "no cancellation reply");
            if let Some(reply) = caller.channel().blocking_pop_message(remaining).unwrap() {
                if let Some(actual_serial) = reply.get_reply_serial() {
                    assert_eq!(actual_serial, serial, "unexpected chooser reply");
                    let (status, results): (u32, PropMap) = reply.read2().unwrap();
                    assert_eq!(status, 1);
                    assert!(results.is_empty());
                    return;
                }
            }
        }
    }

    fn wait_for<T>(mut ready: impl FnMut() -> Option<T>, complaint: &str) -> T {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(value) = ready() {
                return value;
            }
            assert!(Instant::now() < deadline, "{complaint}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_reaped(pid: u32) {
        let proc = PathBuf::from(format!("/proc/{pid}"));
        wait_for(
            || (!proc.exists()).then_some(()),
            "chooser child was not reaped",
        );
    }

    #[test]
    fn request_close_only_ends_the_callers_chooser() {
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let other = Connection::new_address(&address).unwrap();
        let (started, children) = mpsc::channel();
        let service = service(&address, move |_, _| {
            // The bus and child lifecycles are real. A sleeping child stands in
            // for an open window so this runs on a display-free CI worker.
            let child = Command::new("/usr/bin/sleep").arg("60").spawn()?;
            started.send(child.id()).unwrap();
            Ok((child, PathBuf::new()))
        });

        let first = "/org/freedesktop/portal/desktop/request/test/open";
        let second = "/org/freedesktop/portal/desktop/request/test/save";
        let open_serial = open(&caller, first, "OpenFile");
        let first_child = children.recv_timeout(TIMEOUT).unwrap();
        let save_serial = open(&caller, second, "SaveFile");
        let save_child = children.recv_timeout(TIMEOUT).unwrap();

        assert!(close(&other, first).is_err());
        assert!(fs::exists(format!("/proc/{first_child}")).unwrap());
        close(&caller, first).unwrap();
        assert_cancelled(&caller, open_serial);
        wait_reaped(first_child);
        assert!(fs::exists(format!("/proc/{save_child}")).unwrap());
        close(&caller, second).unwrap();
        assert_cancelled(&caller, save_serial);
        wait_reaped(save_child);

        open(&caller, first, "OpenFile");
        let departed_child = children.recv_timeout(TIMEOUT).unwrap();
        let other_serial = open(&other, first, "OpenFile");
        let other_child = children.recv_timeout(TIMEOUT).unwrap();
        drop(caller);
        wait_reaped(departed_child);
        assert!(fs::exists(format!("/proc/{other_child}")).unwrap());
        close(&other, first).unwrap();
        assert_cancelled(&other, other_serial);
        wait_reaped(other_child);

        drop(daemon);
        service.join().unwrap();
    }

    #[test]
    fn a_stalled_result_does_not_block_close_or_replace_a_new_request() {
        use rustix::fs::{mkfifoat, Mode, OFlags, CWD};
        use std::time::{SystemTime, UNIX_EPOCH};

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("gust-portal-{}-{stamp}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&workspace)
            .unwrap();
        let pipe = workspace.join("result");
        mkfifoat(CWD, &pipe, Mode::RUSR | Mode::WUSR).unwrap();

        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let other = Connection::new_address(&address).unwrap();
        let (started, children) = mpsc::channel();
        let first_launch = std::cell::Cell::new(true);
        let result = pipe.clone();
        let service = service(&address, move |_, _| {
            let (child, path) = if first_launch.replace(false) {
                (Command::new("/usr/bin/true").spawn()?, result.clone())
            } else {
                (
                    Command::new("/usr/bin/sleep").arg("60").spawn()?,
                    PathBuf::new(),
                )
            };
            started.send(child.id()).unwrap();
            Ok((child, path))
        });

        let first = "/org/freedesktop/portal/desktop/request/test/open";
        let second = "/org/freedesktop/portal/desktop/request/test/save";
        let first_serial = open(&caller, first, "OpenFile");
        let first_child = children.recv_timeout(TIMEOUT).unwrap();
        wait_reaped(first_child);
        // The nonblocking writer opens only once the result read has
        // reached the pipe. Keeping it open holds the worker in that read.
        let mut writer = wait_for(
            || match fs::OpenOptions::new()
                .write(true)
                .custom_flags(OFlags::NONBLOCK.bits() as i32)
                .open(&pipe)
            {
                Ok(writer) => Some(writer),
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::NXIO.raw_os_error()) =>
                {
                    None
                }
                Err(error) => panic!("could not open the result pipe: {error}"),
            },
            "chooser did not start reading its result",
        );

        let second_serial = open(&other, second, "SaveFile");
        let second_child = children.recv_timeout(TIMEOUT).unwrap();
        close(&other, second).unwrap();
        assert_cancelled(&other, second_serial);
        wait_reaped(second_child);

        close(&caller, first).unwrap();
        assert_cancelled(&caller, first_serial);
        let retry_serial = open(&caller, first, "OpenFile");
        let retry_child = children.recv_timeout(TIMEOUT).unwrap();
        writer.write_all(b"/late-result\0").unwrap();
        drop(writer);
        wait_for(
            || (!pipe.exists()).then_some(()),
            "late result was not cleaned up",
        );
        assert!(fs::exists(format!("/proc/{retry_child}")).unwrap());
        close(&caller, first).unwrap();
        assert_cancelled(&caller, retry_serial);
        wait_reaped(retry_child);
        fs::remove_dir(workspace).unwrap();

        drop(daemon);
        service.join().unwrap();
    }
}
