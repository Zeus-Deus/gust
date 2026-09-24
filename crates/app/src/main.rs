//! Gust — a fast, calm file manager for Omarchy.

pub mod bridge;
mod fm1;

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
                claim_show_in_folder();
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
            claim_show_in_folder();
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
            if let Some(dir) = gust_core::filemanager1::user_service_dir() {
                let _ = gust_core::filemanager1::release_service(&dir);
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

/// Add or remove Gust's marked block in `~/.config/hypr/bindings.lua`, the
/// file Omarchy loads after its defaults for exactly this purpose, then ask
/// Hyprland to reload. Nothing else in the file is touched.
fn run_bind_keys(mode: &str) -> i32 {
    use gust_core::omarchy_keys::*;
    let Some(path) = bindings_path() else {
        eprintln!("gust: cannot find your config directory (is HOME set?)");
        return 1;
    };
    if !path.exists() {
        eprintln!("gust: {} does not exist — is this Omarchy?", path.display());
        return 1;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("gust: cannot read {}", path.display());
        return 1;
    };
    let next = match mode {
        "on" => {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned());
            add_block(&text, exe.as_deref().unwrap_or("gust"))
        }
        "off" => remove_block(&text),
        other => {
            eprintln!("gust: unknown option --bind-keys {other} (expected on or off)");
            return 1;
        }
    };
    if next != text {
        if let Err(e) = write_atomic(&path, &next) {
            eprintln!("gust: cannot update {}: {e}", path.display());
            return 1;
        }
    }
    // Hyprland picks up the change on reload; harmless if not running.
    let _ = std::process::Command::new("hyprctl")
        .arg("reload")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if mode == "on" {
        println!("Super+Shift+F now opens gust (Super+Alt+Shift+F: in the terminal's folder)");
    } else {
        println!("Super+Shift+F is back to Omarchy's default");
    }
    0
}

/// "Show in folder" from browsers and other apps goes over D-Bus, not
/// mimeapps.list. Being the default means answering that too: a per-user
/// activation file, which never touches another package's files and is
/// removed again by `--default off`.
fn claim_show_in_folder() {
    let (Some(dir), Ok(exe)) = (gust_core::filemanager1::user_service_dir(), std::env::current_exe()) else {
        return;
    };
    match gust_core::filemanager1::claim_service(&dir, &exe) {
        Ok(()) => println!("\"Show in folder\" from other apps now opens gust"),
        Err(e) => println!("left alone: {e}"),
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
    // Omarchy: make Super+Shift+F open Gust (opt-in, reversible).
    if let Some(pos) = args.iter().position(|a| a == "--bind-keys") {
        let mode = args.get(pos + 1).map(String::as_str).unwrap_or("on");
        std::process::exit(run_bind_keys(mode));
    }
    // D-Bus activated: answer "Show in folder" from other apps.
    if args.first().map(String::as_str) == Some("--dbus-service") {
        std::process::exit(fm1::run());
    }
    // Internal: hold a clipboard offer after the window that made it.
    if args.first().map(String::as_str) == Some("--clip-serve") {
        std::process::exit(gust_core::clipboard::serve_from_stdin());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("gust [PATH]              open a folder");
        println!("gust --default [on|off]  set or restore the default file manager");
        println!("gust --bind-keys [on|off] make Super+Shift+F open gust (Omarchy)");
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

    // No GPU (a VM, a server, a machine whose driver is missing): Qt would
    // emulate OpenGL on the CPU through llvmpipe, which measured 2.5x the
    // memory and 1.5x the first-window time of Qt's own raster renderer.
    // Decided from sysfs before Qt starts; only THIS process is affected,
    // and a user's own QT_QUICK_BACKEND / GUST_RENDERER always wins (plan §9:
    // nothing global, nothing about the compositor).
    // The Settings choice applies unless the user already chose in the
    // environment (GUST_RENDERER / QT_QUICK_BACKEND always win).
    let setting = gust_core::settings::Settings::default_path()
        .map(|p| gust_core::settings::Settings::load(&p).renderer)
        .unwrap_or(gust_core::settings::RendererChoice::Auto);
    let env_decides = ["GUST_RENDERER", "QT_QUICK_BACKEND", "QSG_RHI_BACKEND"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()));
    let software = match setting {
        _ if env_decides => {
            gust_core::renderer::choose_for_this_machine() == gust_core::renderer::Renderer::Software
        }
        gust_core::settings::RendererChoice::Software => true,
        gust_core::settings::RendererChoice::Gpu => false,
        gust_core::settings::RendererChoice::Auto => {
            gust_core::renderer::choose_for_this_machine() == gust_core::renderer::Renderer::Software
        }
    };
    if software {
        std::env::set_var("QT_QUICK_BACKEND", "software");
    }
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
