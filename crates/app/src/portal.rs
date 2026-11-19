//! `gust --portal`: the file-chooser backend for xdg-desktop-portal.
//!
//! When an app shows an Open or Save dialog through the portal, the portal
//! calls `org.freedesktop.impl.portal.FileChooser` here. Each call starts a
//! Gust window in chooser mode (GUST_CHOOSER=open|open-many|open-folder|save|save-many).
//! The service watches each child independently and collects its answer on
//! a worker. A chooser ends when its caller closes it or disconnects.

use std::collections::{hash_map::Entry, HashMap};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
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

use gust_core::chooser::{self, RawFilter};
use tempfile::TempDir;

const NAME: &str = "org.freedesktop.impl.portal.desktop.gust";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.impl.portal.Request";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const FALLBACK_INTERVAL: Duration = Duration::from_millis(20);

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
    filters: Vec<RawFilter>,
    current_filter: Option<RawFilter>,
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

// Spawn only after preparation finishes and the dispatcher knows the caller
// still wants the window. Tests substitute a child while keeping the transport.
type StartChooser = dyn Fn(&mut Prepared) -> std::io::Result<Child>;

struct Prepared {
    command: Command,
    directory: TempDir,
    filters: Vec<RawFilter>,
}

fn prepare(kind: &str, options: Options, title: &str, root: &Path) -> std::io::Result<Prepared> {
    let mut req = gust_core::chooser::Request::new(
        kind,
        options.directory,
        options.multiple,
        options.current_folder,
        options.current_name,
    )
    .with_current_file(options.current_file);
    if kind == "save-many" {
        req = req.with_files(options.files).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid SaveFiles names")
        })?;
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
    // The answer names the filter the way the app wrote it, so keep that.
    let mut as_sent = options.filters.clone();
    as_sent.extend(options.current_filter.clone());
    req.filters =
        gust_core::chooser::Filters::expand(options.filters, options.current_filter, &globs2);
    let filters = req
        .filters
        .items
        .iter()
        .map(|f| as_sent[f.origin].clone())
        .collect();

    let exe = std::env::current_exe()?;
    let directory = tempfile::Builder::new()
        .prefix("gust-chooser-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root)?;
    let result = directory.path().join("result");
    let mut cmd = Command::new(exe);
    if let Some(f) = &req.folder {
        cmd.arg(f);
    }
    cmd.env("GUST_CHOOSER", req.mode.as_str())
        .env("GUST_CHOOSER_RESULT", &result)
        .env("GUST_CHOOSER_TITLE", bounded(title, MAX_TITLE))
        .env_remove("GUST_SELECT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
    let names = result.with_extension("names");
    if req.files.is_empty() {
        cmd.env_remove("GUST_CHOOSER_FILES");
    } else {
        gust_core::chooser::write_names(&names, &req.files)?;
        cmd.env("GUST_CHOOSER_FILES", &names);
    }
    Ok(Prepared {
        command: cmd,
        directory,
        filters,
    })
}

type RequestKey = (dbus::strings::BusName<'static>, dbus::Path<'static>);

fn request_key(msg: &Message, path: dbus::Path<'_>) -> Option<RequestKey> {
    Some((msg.sender()?.into_static(), path.into_static()))
}

fn read_request(msg: &Message) -> Option<(RequestKey, Options, String)> {
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
    Some((key, options, title))
}

fn answer(bus: &Channel, msg: &Message, code: u32, picked: &[PathBuf], filter: Option<RawFilter>) {
    if msg.get_no_reply() {
        return;
    }
    let mut results = PropMap::new();
    if code == 0 {
        let uris = gust_core::chooser::answer_uris(picked);
        results.insert("uris".into(), Variant(Box::new(uris) as Box<dyn RefArg>));
        if let Some(filter) = filter {
            results.insert("current_filter".into(), Variant(Box::new(filter)));
        }
    }
    let _ = bus.send(msg.method_return().append2(code, results));
}

struct Window {
    child: Child,
    exit_fd: Option<OwnedFd>,
    directory: Option<TempDir>,
    filters: Vec<RawFilter>,
}

impl Prepared {
    fn launch(mut self, start: &StartChooser) -> std::io::Result<Window> {
        let child = start(&mut self)?;
        let exit_fd = Pid::from_raw(child.id() as i32)
            .and_then(|pid| pidfd_open(pid, PidfdFlags::empty()).ok());
        Ok(Window {
            child,
            exit_fd,
            directory: Some(self.directory),
            filters: self.filters,
        })
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// The peer closing after send wakes the same poll as a child exit or D-Bus
// message. A cancelled request drops its receiver, including any late value.
struct Worker<T> {
    ready: UnixStream,
    reply: Receiver<T>,
}

impl<T: Send + 'static> Worker<T> {
    fn start(name: &str, work: impl FnOnce() -> T + Send + 'static) -> std::io::Result<Self> {
        let (ready, notify) = UnixStream::pair()?;
        let (send, reply) = mpsc::channel();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = send.send(work());
                drop(notify);
            })?;
        Ok(Self { ready, reply })
    }
}

type Reply = (u32, Vec<PathBuf>, Option<RawFilter>);

enum Progress {
    Preparing(Worker<std::io::Result<Prepared>>),
    Window(Window),
    Result(Worker<Reply>),
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

    fn finished(&mut self, bus: &Channel, start: &StartChooser) -> bool {
        let (code, picked, filter) = match &mut self.progress {
            Progress::Preparing(worker) => match worker.reply.try_recv() {
                Err(TryRecvError::Empty) => return false,
                Ok(Ok(prepared)) => match prepared.launch(start) {
                    Ok(window) => {
                        self.progress = Progress::Window(window);
                        return false;
                    }
                    Err(_) => (2, Vec::new(), None),
                },
                _ => (2, Vec::new(), None),
            },
            Progress::Window(window) => {
                let status = match window.child.try_wait() {
                    Ok(None) => return false,
                    status => status.ok().flatten(),
                };
                let directory = window.directory.take().expect("window owns its result");
                let mut filters = std::mem::take(&mut window.filters);
                let closed = self.call.is_none();
                let worker = Worker::start("gust-chooser-result", move || {
                    if closed {
                        // Retire the private directory off the dispatcher without reading a cancelled answer.
                        return (2, Vec::new(), None);
                    }
                    match chooser::try_read_answer(&directory.path().join("result")) {
                        Ok((picked, None)) if filters.is_empty() => (0, picked, None),
                        Ok((picked, Some(i))) if i < filters.len() => {
                            (0, picked, Some(filters.swap_remove(i)))
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::NotFound
                                && status.is_some_and(|s| s.success()) =>
                        {
                            (1, Vec::new(), None)
                        }
                        _ => (2, Vec::new(), None),
                    }
                });
                if closed {
                    // The child has been reaped. Its handle is available even during cleanup.
                    return true;
                }
                if let Ok(worker) = worker {
                    self.progress = Progress::Result(worker);
                    return false;
                }
                (2, Vec::new(), None)
            }
            Progress::Result(worker) => match worker.reply.try_recv() {
                Ok(reply) => reply,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => (2, Vec::new(), None),
            },
        };
        if let Some(call) = self.call.take() {
            answer(bus, &call, code, &picked, filter);
        }
        true
    }

    fn poll_fd(&self) -> Option<PollFd<'_>> {
        match &self.progress {
            Progress::Preparing(worker) => Some(PollFd::new(&worker.ready, PollFlags::IN)),
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

fn handle(bus: &Channel, active: &mut HashMap<RequestKey, Chooser>, root: &Path, msg: Message) {
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
                    answer(bus, &call, 2, &[], None);
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
        "SaveFiles" => Some("save-many"),
        _ => None,
    };
    if let Some(kind) =
        kind.filter(|_| iface.as_deref() == Some(IFACE) && msg.path().is_some_and(|p| &*p == PATH))
    {
        let Some((key, request, title)) = read_request(&msg) else {
            answer(bus, &msg, 2, &[], None);
            return;
        };
        let Entry::Vacant(slot) = active.entry(key) else {
            answer(bus, &msg, 2, &[], None);
            return;
        };
        let root = root.to_owned();
        let Ok(worker) = Worker::start("gust-chooser-prepare", move || {
            prepare(kind, request, &title, &root)
        }) else {
            answer(bus, &msg, 2, &[], None);
            return;
        };
        slot.insert(Chooser {
            call: Some(msg),
            progress: Progress::Preparing(worker),
        });
    } else if !msg.get_no_reply() {
        if let Some(reply) = channel::default_reply(&msg) {
            let _ = bus.send(reply);
        }
    }
}

fn serve(bus: &Channel, root: &Path, start: &StartChooser) -> Result<(), ()> {
    let mut active = HashMap::new();
    let mut last_activity = Instant::now();
    loop {
        bus.read_write(Some(Duration::ZERO))?;
        let count = active.len();
        while let Some(msg) = bus.pop_message() {
            if msg.msg_type() == MessageType::MethodCall {
                last_activity = Instant::now();
            }
            handle(bus, &mut active, root, msg);
        }
        active.retain(|_, chooser| !chooser.finished(bus, start));
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
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    i32::from(serve(conn.channel(), &root, &|prepared| prepared.command.spawn()).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbus::blocking::BlockingSender;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::OpenOptionsExt;
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
        start: impl Fn(&mut Prepared) -> std::io::Result<Child> + Send + 'static,
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
        std::thread::spawn(move || {
            assert_eq!(
                serve(conn.channel(), &std::env::temp_dir(), &start),
                Err(())
            )
        })
    }

    fn open(caller: &Connection, path: &str, method: &str) -> u32 {
        caller
            .channel()
            .send(call(path, method, PropMap::new()))
            .unwrap()
    }

    fn call(path: &str, method: &str, options: PropMap) -> Message {
        let mut msg = Message::new_method_call(NAME, PATH, IFACE, method).unwrap();
        msg.append_all((
            dbus::Path::new(path).unwrap(),
            "test",
            "",
            "Fixture",
            options,
        ));
        msg
    }

    fn close(caller: &Connection, path: &str) -> Result<(), dbus::Error> {
        let msg = Message::new_method_call(NAME, path, REQUEST, "Close").unwrap();
        caller.send_with_reply_and_block(msg, TIMEOUT).map(|_| ())
    }

    fn assert_closed(caller: &Connection, serial: u32) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "no cancellation reply");
            if let Some(reply) = caller.channel().blocking_pop_message(remaining).unwrap() {
                if let Some(actual_serial) = reply.get_reply_serial() {
                    assert_eq!(actual_serial, serial, "unexpected chooser reply");
                    let (status, results): (u32, PropMap) = reply.read2().unwrap();
                    assert_eq!(status, 2);
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
    fn save_files_returns_raw_names_and_the_selected_original_filter() {
        use std::os::unix::ffi::OsStrExt;
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let (started, dirs) = mpsc::channel();
        let service = service(&address, move |prepared| {
            let env: HashMap<_, _> = prepared.command.get_envs().collect();
            let value = |name: &str| env[std::ffi::OsStr::new(name)].unwrap();
            assert_eq!(value("GUST_CHOOSER"), "save-many");
            assert_eq!(value("GUST_CHOOSER_ACCEPT"), "Export");
            let files = chooser::read_names(Path::new(value("GUST_CHOOSER_FILES")));
            assert_eq!(files.len(), 1025);
            assert_eq!(files[0].as_bytes(), b"raw-\xff");
            let filters =
                chooser::Filters::from_json(value("GUST_CHOOSER_FILTERS").to_str().unwrap());
            assert_eq!(filters.current, 0);
            assert_eq!(filters.items[1].globs, ["*"]);
            let picked = files
                .iter()
                .map(|name| Path::new("/dest").join(name))
                .collect::<Vec<_>>();
            chooser::write_answer(Path::new(value("GUST_CHOOSER_RESULT")), &picked, Some(1))?;
            started.send(prepared.directory.path().to_owned()).unwrap();
            // A completed pick survives a later nonzero child exit.
            Command::new("/usr/bin/false").spawn()
        });

        let text: RawFilter = ("Text".into(), vec![(0, "*.txt".into())]);
        let all: RawFilter = ("All".into(), vec![(1, "*/*".into())]);
        let mut files = vec![b"raw-\xff\0".to_vec()];
        files.extend((0..1024).map(|i| format!("{i:0200}\0").into_bytes()));
        let mut options = PropMap::new();
        options.insert("files".into(), Variant(Box::new(files)));
        options.insert(
            "filters".into(),
            Variant(Box::new(vec![text.clone(), all.clone()])),
        );
        options.insert("current_filter".into(), Variant(Box::new(text)));
        options.insert(
            "accept_label".into(),
            Variant(Box::new("_Export".to_string())),
        );
        let path = "/org/freedesktop/portal/desktop/request/test/batch";
        let response = caller
            .send_with_reply_and_block(call(path, "SaveFiles", options), TIMEOUT)
            .unwrap();
        let mut it = response.iter_init();
        assert_eq!(it.read::<u32>().unwrap(), 0);
        assert_eq!(read_options(&mut it).unwrap().current_filter, Some(all));
        let (_, results): (u32, PropMap) = response.read2().unwrap();
        let uris = dbus::arg::prop_cast::<Vec<String>>(&results, "uris").unwrap();
        assert_eq!(uris.len(), 1025);
        assert_eq!(uris[0], "file:///dest/raw-%FF");
        assert_eq!(uris[1024], format!("file:///dest/{:0200}", 1023));
        assert!(!dirs.recv_timeout(TIMEOUT).unwrap().exists());
        drop(daemon);
        service.join().unwrap();
    }

    #[test]
    fn accepting_the_default_returns_the_exact_preferred_rules() {
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let service = service(&address, |prepared| {
            let env: HashMap<_, _> = prepared.command.get_envs().collect();
            let json = env[std::ffi::OsStr::new("GUST_CHOOSER_FILTERS")].unwrap();
            let filters = chooser::Filters::from_json(json.to_str().unwrap());
            assert_eq!(filters.items.len(), 2);
            chooser::write_answer(
                &prepared.directory.path().join("result"),
                &[PathBuf::from("/picked")],
                Some(filters.current),
            )?;
            Command::new("/usr/bin/true").spawn()
        });
        let raw = |kind, rule: &str| ("Images".to_owned(), vec![(kind, rule.to_owned())]);
        let mime = raw(1, "*/*");
        let glob = raw(0, "*");
        for (offered, preferred) in [
            (vec![raw(0, "*.png")], raw(0, "*.jpg")),
            (vec![glob.clone(), mime.clone()], mime.clone()),
            (vec![glob], mime),
        ] {
            let mut options = PropMap::new();
            options.insert("filters".into(), Variant(Box::new(offered)));
            options.insert(
                "current_filter".into(),
                Variant(Box::new(preferred.clone())),
            );
            let path = "/org/freedesktop/portal/desktop/request/test/default";
            let reply = caller
                .send_with_reply_and_block(call(path, "OpenFile", options), TIMEOUT)
                .unwrap();
            let mut it = reply.iter_init();
            assert_eq!(it.read::<u32>().unwrap(), 0);
            assert_eq!(
                read_options(&mut it).unwrap().current_filter,
                Some(preferred)
            );
        }
        drop(daemon);
        service.join().unwrap();
    }

    #[test]
    fn malformed_answers_fail_without_returning_uris_or_filters() {
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let cases = [
            (b"/first\0/incomplete".as_slice(), "/usr/bin/false"),
            (b"/first\0garbage\0", "/usr/bin/true"),
            (b"/first\0filter:2\0", "/usr/bin/true"),
            (b"/first\0", "/usr/bin/false"),
        ]
        .into_iter();
        let cases = std::cell::RefCell::new(cases);
        let count = cases.borrow().len();
        let service = service(&address, move |prepared| {
            let (bytes, exit) = cases.borrow_mut().next().unwrap();
            fs::write(prepared.directory.path().join("result"), bytes)?;
            Command::new(exit).spawn()
        });
        for _ in 0..count {
            let mut options = PropMap::new();
            let filter: RawFilter = ("All".into(), vec![(0, "*".into())]);
            options.insert("filters".into(), Variant(Box::new(vec![filter])));
            let path = "/org/freedesktop/portal/desktop/request/test/malformed";
            let reply = caller
                .send_with_reply_and_block(call(path, "OpenFile", options), TIMEOUT)
                .unwrap();
            let (code, results): (u32, PropMap) = reply.read2().unwrap();
            assert_eq!(code, 2);
            assert!(results.is_empty());
        }
        drop(daemon);
        service.join().unwrap();
    }

    #[test]
    fn empty_answers_distinguish_user_cancel_from_window_or_request_failure() {
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let launches = std::cell::Cell::new(0);
        let service = service(&address, move |_| {
            launches.set(launches.get() + 1);
            match launches.get() {
                1 => Command::new("/usr/bin/true").spawn(),
                2 => Command::new("/usr/bin/false").spawn(),
                3 => Err(std::io::Error::other("fixture spawn failure")),
                _ => panic!("unexpected chooser for invalid batch"),
            }
        });
        let path = "/org/freedesktop/portal/desktop/request/test/empty";
        for (method, expected) in [
            ("OpenFile", 1),
            ("SaveFile", 2),
            ("OpenFile", 2),
            ("SaveFiles", 2),
        ] {
            let reply = caller
                .send_with_reply_and_block(call(path, method, PropMap::new()), TIMEOUT)
                .unwrap();
            let (code, results): (u32, PropMap) = reply.read2().unwrap();
            assert_eq!(code, expected);
            assert!(results.is_empty());
        }
        drop(daemon);
        service.join().unwrap();
    }

    #[test]
    fn request_close_only_ends_the_callers_chooser() {
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let other = Connection::new_address(&address).unwrap();
        let (started, children) = mpsc::channel();
        let service = service(&address, move |prepared| {
            // The bus and child lifecycles are real. A sleeping child stands in
            // for an open window so this runs on a display-free CI worker.
            let child = Command::new("/usr/bin/sleep").arg("60").spawn()?;
            started
                .send((child.id(), prepared.directory.path().to_owned()))
                .unwrap();
            Ok(child)
        });

        let first = "/org/freedesktop/portal/desktop/request/test/open";
        let second = "/org/freedesktop/portal/desktop/request/test/save";
        let open_serial = open(&caller, first, "OpenFile");
        let (first_child, _) = children.recv_timeout(TIMEOUT).unwrap();
        let batch = || {
            let mut options = PropMap::new();
            options.insert(
                "files".into(),
                Variant(Box::new(vec![b"raw-\xff".to_vec()])),
            );
            options
        };
        let save_serial = caller
            .channel()
            .send(call(second, "SaveFiles", batch()))
            .unwrap();
        let (save_child, saved_dir) = children.recv_timeout(TIMEOUT).unwrap();

        assert!(saved_dir.join("result.names").is_file());
        let duplicate = caller
            .send_with_reply_and_block(call(second, "SaveFiles", batch()), TIMEOUT)
            .unwrap();
        assert_eq!(duplicate.read1::<u32>().unwrap(), 2);
        assert!(close(&other, first).is_err());
        assert!(fs::exists(format!("/proc/{first_child}")).unwrap());
        close(&caller, first).unwrap();
        assert_closed(&caller, open_serial);
        wait_reaped(first_child);
        assert!(fs::exists(format!("/proc/{save_child}")).unwrap());
        close(&caller, second).unwrap();
        assert_closed(&caller, save_serial);
        wait_reaped(save_child);

        wait_for(
            || (!saved_dir.exists()).then_some(()),
            "closed SaveFiles names remained",
        );
        caller
            .channel()
            .send(call(first, "SaveFiles", batch()))
            .unwrap();
        let (departed_child, departed_dir) = children.recv_timeout(TIMEOUT).unwrap();
        let other_serial = open(&other, first, "OpenFile");
        let (other_child, _) = children.recv_timeout(TIMEOUT).unwrap();
        drop(caller);
        wait_reaped(departed_child);
        wait_for(
            || (!departed_dir.exists()).then_some(()),
            "disconnected SaveFiles names remained",
        );
        assert!(fs::exists(format!("/proc/{other_child}")).unwrap());
        close(&other, first).unwrap();
        assert_closed(&other, other_serial);
        wait_reaped(other_child);

        drop(daemon);
        service.join().unwrap();
    }

    #[test]
    fn a_stalled_result_does_not_block_close_or_replace_a_new_request() {
        use rustix::fs::{mkfifoat, Mode, OFlags, CWD};
        let (daemon, address) = private_bus();
        let caller = Connection::new_address(&address).unwrap();
        let other = Connection::new_address(&address).unwrap();
        let (started, children) = mpsc::channel();
        let first_launch = std::cell::Cell::new(true);
        let service = service(&address, move |prepared| {
            let result = prepared.directory.path().join("result");
            let child = if first_launch.replace(false) {
                mkfifoat(CWD, &result, Mode::RUSR | Mode::WUSR).unwrap();
                Command::new("/usr/bin/true").spawn()?
            } else {
                Command::new("/usr/bin/sleep").arg("60").spawn()?
            };
            started.send((child.id(), result.to_owned())).unwrap();
            Ok(child)
        });

        let first = "/org/freedesktop/portal/desktop/request/test/open";
        let second = "/org/freedesktop/portal/desktop/request/test/save";
        let first_serial = open(&caller, first, "OpenFile");
        let (first_child, pipe) = children.recv_timeout(TIMEOUT).unwrap();
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
        let (second_child, _) = children.recv_timeout(TIMEOUT).unwrap();
        close(&other, second).unwrap();
        assert_closed(&other, second_serial);
        wait_reaped(second_child);

        close(&caller, first).unwrap();
        assert_closed(&caller, first_serial);
        let retry_serial = open(&caller, first, "OpenFile");
        let (retry_child, _) = children.recv_timeout(TIMEOUT).unwrap();
        writer.write_all(b"/late-result\0filter:1\0").unwrap();
        drop(writer);
        wait_for(
            || (!pipe.exists()).then_some(()),
            "late result was not cleaned up",
        );
        assert!(fs::exists(format!("/proc/{retry_child}")).unwrap());
        close(&caller, first).unwrap();
        assert_closed(&caller, retry_serial);
        wait_reaped(retry_child);

        drop(daemon);
        service.join().unwrap();
    }
}
