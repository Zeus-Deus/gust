//! The environment of the programs Gust starts for the user.
//!
//! Gust changes its own environment before Qt starts: it picks a renderer
//! (`QT_QUICK_BACKEND`) and turns on Qt's logging. Those choices are about
//! Gust's window. An application opened from Gust must start as if the user
//! had launched it themselves — otherwise a Qt Quick application opened on a
//! machine where Gust draws in software would be forced into software too.
//!
//! So every variable Gust sets for itself goes through [`set_for_gust`],
//! which remembers what the user had, and every spawn of an application,
//! opener or terminal goes through [`for_user_app`], which puts that back.
//! Gust's own helpers and re-execs do not: they need those variables.

use std::ffi::{OsStr, OsString};
use std::process::Command;
use std::sync::Mutex;

/// A variable Gust changed for itself, with the value the user had (`None`
/// when it was not set).
type Changed = (OsString, Option<OsString>);

static CHANGED: Mutex<Vec<Changed>> = Mutex::new(Vec::new());

/// Instructions from one Gust process to the Gust window it starts: what to
/// select, which handoff to take, that it is a file dialog or an
/// administrator session. They address that one window and mean nothing to
/// another program — and a terminal that inherited `GUST_CHOOSER` would turn
/// the next `gust` typed in it into a file dialog.
const WINDOW_ONLY: [&str; 9] = [
    "GUST_SELECT",
    "GUST_HANDOFF",
    "GUST_READY_FILE",
    "GUST_CHOOSER",
    "GUST_CHOOSER_RESULT",
    "GUST_CHOOSER_TITLE",
    "GUST_CHOOSER_NAME",
    "GUST_ADMIN",
    "GUST_ADMIN_FOLDER",
];

/// Remember the user's value of `key` the first time Gust changes it. A
/// second change must not mistake Gust's own first value for the user's.
fn remember(changed: &mut Vec<Changed>, key: &OsStr, users: Option<OsString>) {
    if !changed.iter().any(|(k, _)| k == key) {
        changed.push((key.to_owned(), users));
    }
}

/// Set `key` for this Gust process, and only for it: programs started
/// through [`for_user_app`] get the user's own value back.
///
/// Call before any thread exists, as with every change to the environment.
pub fn set_for_gust(key: &str, value: impl AsRef<OsStr>) {
    let mut changed = CHANGED.lock().unwrap_or_else(|e| e.into_inner());
    remember(&mut changed, OsStr::new(key), std::env::var_os(key));
    std::env::set_var(key, value);
}

/// Give `cmd` the environment the user started Gust with: every variable
/// Gust set for itself is restored to the user's value or removed, and
/// window-only instructions are dropped. Nothing else is touched.
pub fn for_user_app(cmd: &mut Command) -> &mut Command {
    let changed = CHANGED.lock().unwrap_or_else(|e| e.into_inner());
    restore(cmd, &changed)
}

fn restore<'a>(cmd: &'a mut Command, changed: &[Changed]) -> &'a mut Command {
    for key in WINDOW_ONLY {
        cmd.env_remove(key);
    }
    for (key, users) in changed {
        match users {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn changed(pairs: &[(&str, Option<&str>)]) -> Vec<Changed> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), v.map(OsString::from)))
            .collect()
    }

    /// What `cmd` would do to `key`: `None` = inherit, `Some(None)` = remove,
    /// `Some(Some(v))` = set.
    fn planned(cmd: &Command, key: &str) -> Option<Option<OsString>> {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .map(|(_, v)| v.map(OsStr::to_owned))
    }

    #[test]
    fn what_gust_added_is_removed_for_the_application() {
        let mut cmd = Command::new("anything");
        restore(
            &mut cmd,
            &changed(&[
                ("QT_QUICK_BACKEND", None),
                ("QT_FORCE_STDERR_LOGGING", None),
            ]),
        );
        assert_eq!(planned(&cmd, "QT_QUICK_BACKEND"), Some(None));
        assert_eq!(planned(&cmd, "QT_FORCE_STDERR_LOGGING"), Some(None));
    }

    #[test]
    fn what_gust_overwrote_goes_back_to_the_users_value() {
        let mut cmd = Command::new("anything");
        restore(&mut cmd, &changed(&[("QT_QUICK_BACKEND", Some("opengl"))]));
        assert_eq!(
            planned(&cmd, "QT_QUICK_BACKEND"),
            Some(Some(OsString::from("opengl")))
        );
    }

    #[test]
    fn what_gust_never_changed_is_inherited_untouched() {
        let mut cmd = Command::new("anything");
        restore(&mut cmd, &changed(&[("QT_QUICK_BACKEND", None)]));
        // The user's own choices, and Gust settings a user or a test
        // harness exports, are not Gust's to remove.
        for key in [
            "QT_ASSUME_STDERR_HAS_CONSOLE",
            "QSG_RHI_BACKEND",
            "QT_QPA_PLATFORM",
            "GUST_RENDERER",
            "GUST_QA_STATE",
            "GUST_OMARCHY_STATE",
            "PATH",
            "HOME",
        ] {
            assert_eq!(planned(&cmd, key), None, "{key}");
        }
    }

    #[test]
    fn window_instructions_never_reach_another_program() {
        let mut cmd = Command::new("anything");
        restore(&mut cmd, &[]);
        for key in WINDOW_ONLY {
            assert_eq!(planned(&cmd, key), Some(None), "{key}");
        }
    }

    #[test]
    fn the_users_value_is_the_one_from_before_gusts_first_change() {
        let mut list = Vec::new();
        let key = OsStr::new("QT_QUICK_BACKEND");
        remember(&mut list, key, Some("opengl".into()));
        // Gust's own value, seen on a second change, is not the user's.
        remember(&mut list, key, Some("software".into()));
        assert_eq!(list, changed(&[("QT_QUICK_BACKEND", Some("opengl"))]));

        let mut unset = Vec::new();
        remember(&mut unset, key, None);
        remember(&mut unset, key, Some("software".into()));
        assert_eq!(unset, changed(&[("QT_QUICK_BACKEND", None)]));
    }

    /// The real thing, end to end: a fake application in a temp dir reports
    /// the environment it was actually started with. The variables are put
    /// on the command rather than in this process, so no other test sees
    /// them; `restore` must undo them exactly as it undoes inherited ones.
    #[test]
    fn a_started_application_sees_the_users_environment() {
        let d = std::env::temp_dir().join(format!("gust-childenv-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let app = d.join("fake-app");
        std::fs::write(
            &app,
            "#!/bin/sh\nprintf '%s|%s|%s|%s' \"${QT_QUICK_BACKEND-unset}\" \
             \"${QT_FORCE_STDERR_LOGGING-unset}\" \"${GUST_CHOOSER-unset}\" \"${KEPT-unset}\"\n",
        )
        .unwrap();
        let run = |changed: &[Changed]| {
            let mut cmd = Command::new("sh");
            cmd.arg(&app)
                .env("QT_QUICK_BACKEND", "software")
                .env("QT_FORCE_STDERR_LOGGING", "1")
                .env("GUST_CHOOSER", "open")
                .env("KEPT", "yes")
                .stdin(Stdio::null());
            restore(&mut cmd, changed);
            String::from_utf8(cmd.output().unwrap().stdout).unwrap()
        };
        assert_eq!(
            run(&changed(&[
                ("QT_QUICK_BACKEND", None),
                ("QT_FORCE_STDERR_LOGGING", None)
            ])),
            "unset|unset|unset|yes"
        );
        assert_eq!(
            run(&changed(&[
                ("QT_QUICK_BACKEND", Some("opengl")),
                ("QT_FORCE_STDERR_LOGGING", Some("0"))
            ])),
            "opengl|0|unset|yes"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
