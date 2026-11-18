use super::*;
use dbus::blocking::BlockingSender;
use std::fs;
use std::io::{BufRead, BufReader};
use std::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(10);

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
            if reply.get_reply_serial() == Some(serial) {
                let (status, results): (u32, PropMap) = reply.read2().unwrap();
                assert_eq!(status, 1);
                assert!(results.is_empty());
                return;
            }
        }
    }
}

fn wait_reaped(pid: u32) {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let deadline = Instant::now() + TIMEOUT;
    while proc.exists() {
        assert!(Instant::now() < deadline, "chooser child was not reaped");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn request_close_only_ends_the_callers_chooser() {
    let mut daemon = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon is required for the portal integration test"));
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    let caller = Connection::new_address(address).unwrap();
    let other = Connection::new_address(address).unwrap();
    let mut bus = Channel::open_private(address).unwrap();
    bus.register().unwrap();
    bus.set_watch_enabled(true);
    let conn = Connection::from(bus);
    assert_eq!(
        conn.request_name(NAME, false, false, true).unwrap(),
        RequestNameReply::PrimaryOwner
    );

    let (started, children) = mpsc::channel();
    let service = std::thread::spawn(move || {
        // The bus and child lifecycles are real. A sleeping child stands in
        // for an open window so this runs on a display-free CI worker.
        let start = move |_: gust_core::chooser::Request, _: &str| {
            let child = Command::new("/usr/bin/sleep").arg("60").spawn()?;
            started.send(child.id()).unwrap();
            Ok((child, PathBuf::new()))
        };
        assert_eq!(serve(conn.channel(), &start), Err(()));
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

    drop(daemon);
    service.join().unwrap();
}
