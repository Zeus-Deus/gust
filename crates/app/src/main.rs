//! Gust — a fast, calm file manager for Omarchy.

pub mod bridge;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Claim or release the "default file manager" association.
///
/// Deliberately a plain CLI path with no Qt involved: it must work over SSH
/// and from a package post-install hook, where there is no display.
fn run_default(mode: &str) -> i32 {
    use gust_core::defaults::*;

    // Never as root: writing another user's associations as root leaves
    // files they cannot fix.
    if is_root() {
        eprintln!("gust: refusing to change defaults as root.");
        eprintln!("gust: run this as the user whose defaults should change.");
        return 1;
    }

    let (Some(mime_path), Some(state)) = (mimeapps_path(), state_path()) else {
        eprintln!("gust: cannot find your config directory (is HOME set?)");
        return 1;
    };
    let current = MimeApps::parse(&std::fs::read_to_string(&mime_path).unwrap_or_default());

    match mode {
        "on" => {
            // Claiming twice must not record gust.desktop as the "previous"
            // value, which would make rollback restore Gust over itself.
            if state.exists() && current.get("inode/directory").as_deref() == Some(GUST_DESKTOP) {
                println!("gust is already the default file manager");
                return 0;
            }
            let (next, prev) = plan_claim(&current);
            if let Err(e) = write_atomic(&state, &prev.to_text()) {
                eprintln!("gust: cannot record the previous default: {e}");
                return 1;
            }
            if let Err(e) = write_atomic(&mime_path, &next.to_text()) {
                eprintln!("gust: cannot update {}: {e}", mime_path.display());
                return 1;
            }
            let was = prev
                .entries
                .get("inode/directory")
                .and_then(|v| v.clone())
                .unwrap_or_else(|| "nothing".into());
            println!("gust is now the default file manager (was: {was})");
            println!("run `gust --default off` to put it back");
            0
        }
        "off" => {
            let Ok(text) = std::fs::read_to_string(&state) else {
                println!("gust was not set as the default; nothing to undo");
                return 0;
            };
            let prev = PreviousDefaults::from_text(&text);
            let (next, skipped) = plan_release(&current, &prev);
            if let Err(e) = write_atomic(&mime_path, &next.to_text()) {
                eprintln!("gust: cannot update {}: {e}", mime_path.display());
                return 1;
            }
            let _ = std::fs::remove_file(&state);
            for s in &skipped {
                // A later choice by something else is reported, not undone.
                println!("left alone: {s}");
            }
            println!("the previous default file manager has been restored");
            0
        }
        other => {
            eprintln!("gust: unknown option --default {other} (expected on or off)");
            1
        }
    }
}

fn is_root() -> bool {
    // Reading /proc/self/status avoids pulling in a C binding for one call.
    // Fails safe: if the UID cannot be determined we do NOT claim root, so a
    // legitimate user is never blocked from setting their own default.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|v| v.split_whitespace().next().map(str::to_string))
        })
        .map(|uid| uid == "0")
        .unwrap_or(false)
}

/// The QML entry point, compiled into the binary's resources.
const MAIN_QML: &str = "qrc:/qt/qml/com/gust/app/ui/Main.qml";

fn main() {
    // `--default [on|off]` is a CLI action, not a window: handle it before
    // any Qt object exists so it works over SSH and in a package hook.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(pos) = args.iter().position(|a| a == "--default") {
        let mode = args.get(pos + 1).map(String::as_str).unwrap_or("on");
        std::process::exit(run_default(mode));
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("gust [PATH]              open a folder");
        println!("gust --default [on|off]  set or restore the default file manager");
        std::process::exit(0);
    }

    // Qt suppresses qDebug/qWarning output when stderr is not a terminal,
    // which hides QML errors from any automated run. Gust always wants them:
    // a silent QML failure is how a broken build looks healthy. Set only if
    // the user has not expressed their own preference.
    if std::env::var_os("QT_FORCE_STDERR_LOGGING").is_none() {
        std::env::set_var("QT_FORCE_STDERR_LOGGING", "1");
    }
    if std::env::var_os("QT_ASSUME_STDERR_HAS_CONSOLE").is_none() {
        std::env::set_var("QT_ASSUME_STDERR_HAS_CONSOLE", "1");
    }

    // Respect the user's existing render settings: Gust sets no global QT_*
    // rendering variables and changes nothing about the compositor (plan §9).
    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    // A failed QML load must be LOUD and must exit non-zero. Silently running
    // an event loop with no window is the worst failure mode: the process
    // looks healthy while the user sees nothing. `objectCreationFailed` is
    // Qt's own signal for this, so we do not have to guess.
    //
    // Atomics rather than Cell: the connection closures must be Send.
    let failed = Arc::new(AtomicBool::new(false));
    let created = Arc::new(AtomicBool::new(false));

    let _fail_guard = {
        let failed = failed.clone();
        engine.as_mut().map(|e| {
            e.on_object_creation_failed(move |_engine, url| {
                eprintln!("gust: QML creation FAILED for {}", url.to_string());
                failed.store(true, Ordering::SeqCst);
            })
        })
    };

    let _created_guard = {
        let created = created.clone();
        engine.as_mut().map(|e| unsafe {
            e.on_object_created(move |_engine, obj, url| {
                if obj.is_null() {
                    eprintln!("gust: QML root object is null for {}", url.to_string());
                } else {
                    created.store(true, Ordering::SeqCst);
                }
            })
        })
    };

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from(&QString::from(MAIN_QML)));
    }

    if failed.load(Ordering::SeqCst) || !created.load(Ordering::SeqCst) {
        eprintln!("gust: failed to load {MAIN_QML} — no root object was created.");
        eprintln!("gust: check the QML errors above, or whether the resource");
        eprintln!("gust: path matches the alias built into the qrc.");
        std::process::exit(2);
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
