//! `gust --dbus-service`: answers `org.freedesktop.FileManager1` on the
//! session bus, so "Show in folder" in a browser, a download notification
//! or an editor opens Gust with the file selected.
//!
//! A separate small process, started on demand by D-Bus activation (see
//! packaging/org.freedesktop.FileManager1.service), so no Gust window has
//! to be running and none holds the name. Each call opens new Gust windows
//! as ordinary child processes; paths go through the environment and argv
//! as single arguments, never through a shell.

use std::time::Duration;

const NAME: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";

fn launch(opens: Vec<gust_core::filemanager1::Open>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    for o in opens {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(&o.folder)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match &o.select {
            Some(name) => cmd.env("GUST_SELECT", name),
            None => cmd.env_remove("GUST_SELECT"),
        };
        if let Ok(mut child) = cmd.spawn() {
            // Reaped in the background so no zombie outlives the window.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

pub fn run() -> i32 {
    use dbus::blocking::Connection;
    use dbus::channel::MatchingReceiver;
    use dbus_crossroads::Crossroads;

    let Ok(conn) = Connection::new_session() else {
        eprintln!("gust: no session bus");
        return 1;
    };
    // Do not replace another file manager that holds the name.
    if conn.request_name(NAME, false, false, true).is_err() {
        eprintln!("gust: {NAME} is already owned");
        return 1;
    }
    let mut cr = Crossroads::new();
    let iface = cr.register(NAME, |b| {
        b.method(
            "ShowFolders",
            ("URIs", "StartupId"),
            (),
            |_, _, (uris, _startup): (Vec<String>, String)| {
                launch(gust_core::filemanager1::plan(&uris, false));
                Ok(())
            },
        );
        b.method(
            "ShowItems",
            ("URIs", "StartupId"),
            (),
            |_, _, (uris, _startup): (Vec<String>, String)| {
                launch(gust_core::filemanager1::plan(&uris, true));
                Ok(())
            },
        );
        // Gust has no separate properties window; the details view is
        // Quick Look, so show the item selected with its details reachable.
        b.method(
            "ShowItemProperties",
            ("URIs", "StartupId"),
            (),
            |_, _, (uris, _startup): (Vec<String>, String)| {
                launch(gust_core::filemanager1::plan(&uris, true));
                Ok(())
            },
        );
    });
    cr.insert(PATH, &[iface], ());
    conn.start_receive(
        dbus::message::MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            let _ = cr.handle_message(msg, conn);
            true
        }),
    );
    // Stay a while for follow-up calls, then exit: D-Bus starts us again
    // on the next request. An idle service holds nothing.
    let mut idle = 0u32;
    loop {
        match conn.process(Duration::from_secs(1)) {
            Ok(true) => idle = 0,
            Ok(false) => {
                idle += 1;
                if idle >= 30 {
                    return 0;
                }
            }
            Err(_) => return 1,
        }
    }
}
