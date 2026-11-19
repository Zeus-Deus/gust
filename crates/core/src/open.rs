//! Handing a file to the desktop's own opener.
//!
//! Gust never decides what a file *is* by running it, and never runs it
//! itself. It asks the desktop to open it, exactly as any other application
//! would, and it does so without a shell so a filename can never become a
//! command (DESIGN.md §7).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[path = "media.rs"]
pub mod media;
#[path = "openwith.rs"]
pub mod with;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Why an open was refused, in words a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRefusal {
    /// The path is not on the local filesystem.
    NotLocal,
    /// The entry is executable; running it must be deliberate.
    NeedsDeliberateRun,
    /// No handler is installed.
    NoHandler,
    /// The handler could not be started.
    Failed(String),
}

impl std::fmt::Display for OpenRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenRefusal::NotLocal => write!(f, "this location is not a local file"),
            OpenRefusal::NeedsDeliberateRun => {
                write!(f, "this file is executable — use Run to launch it")
            }
            OpenRefusal::NoHandler => {
                write!(f, "no application is set up to open this kind of file")
            }
            OpenRefusal::Failed(e) => write!(f, "could not open: {e}"),
        }
    }
}

/// The openers Gust will try, in order. All take a path as a single argument
/// and none interpret it as a shell word.
const OPENERS: [&str; 3] = ["xdg-open", "gio", "handlr"];

/// Does this look like a runnable program rather than a document?
///
/// Shared with the UI's activation guard and the Run action, so synthetic
/// executable bits on phone media cannot turn documents into programs.
pub fn is_runnable(path: &Path, mode: Option<u32>) -> bool {
    crate::mime::needs_trusted_launch(path.file_name().unwrap_or_default(), mode)
}

/// Build the command for a given opener without running it.
///
/// Separated so tests can assert the exact argv — the property that matters
/// is that the path is one argument and no shell is involved.
pub fn command_for(opener: &str, path: &Path) -> Command {
    let mut cmd = Command::new(opener);
    if matches!(opener, "gio" | "handlr") {
        cmd.arg("open");
    }
    // `--` so a filename beginning with '-' is never read as a flag.
    if opener == "xdg-open" {
        cmd.arg(path);
    } else {
        cmd.arg("--").arg(path);
    }
    cmd
}

/// How soon after its start an opener's failure still means "the open did
/// not happen". `xdg-open` can stay alive for as long as the application it
/// started, so a later failure says something about that application, not
/// about the open.
const OPEN_FAILED_WINDOW: Duration = Duration::from_secs(5);

/// What `on_refusal` is told when the opener itself reports failure.
pub const OPENER_REPORTED_FAILURE: &str = "the desktop launcher reported a failure";

/// gio and handlr exit after dispatch. xdg-open can stay alive with
/// the application, so only its early failures describe the open request.
fn refusal_from_exit(opener: &str, code: Option<i32>, elapsed: Duration) -> Option<OpenRefusal> {
    let reason = match (opener, code) {
        ("xdg-open", Some(3)) if elapsed <= OPEN_FAILED_WINDOW => {
            "a required desktop tool is missing"
        }
        ("xdg-open", Some(4)) if elapsed <= OPEN_FAILED_WINDOW => OPENER_REPORTED_FAILURE,
        ("gio", Some(2)) => OPENER_REPORTED_FAILURE,
        ("handlr", Some(code)) if code != 0 => OPENER_REPORTED_FAILURE,
        _ => return None,
    };
    Some(OpenRefusal::Failed(reason.into()))
}

/// Wait for a detached child on a worker thread, so the caller never blocks
/// and the child never lingers as a zombie. `done` runs there with the exit
/// code (`None` if a signal ended it) and how long the child lived.
fn wait_detached(mut child: Child, done: impl FnOnce(Option<i32>, Duration) + Send + 'static) {
    let started = Instant::now();
    let _ = std::thread::Builder::new()
        .name("gust-reap".into())
        .spawn(move || {
            if let Ok(status) = child.wait() {
                done(status.code(), started.elapsed());
            }
        });
}

/// Reap a detached child in the background. Closing Gust still leaves the
/// application running; only its exit status is collected.
pub(crate) fn reap(child: Child) {
    wait_detached(child, |_, _| {});
}

/// Reap an opener, and tell `on_refusal` if its exit says the open did not
/// happen.
fn watch_opener(
    opener: &'static str,
    child: Child,
    on_refusal: impl FnOnce(OpenRefusal) + Send + 'static,
) {
    wait_detached(child, move |code, elapsed| {
        if let Some(refusal) = refusal_from_exit(opener, code, elapsed) {
            on_refusal(refusal);
        }
    });
}

/// Ask the desktop to open `path`.
///
/// Returns the opener that accepted the request. The child is fully detached
/// from Gust's stdio so a chatty handler cannot spam the terminal, and Gust
/// does not wait for it. An opener that starts and then finds no application
/// is not reported here; [`open_with_desktop_reporting`] does that.
pub fn open_with_desktop(path: &Path, mode: Option<u32>) -> Result<String, OpenRefusal> {
    open_with_desktop_reporting(path, mode, |_| {})
}

/// [`open_with_desktop`], plus the answer that only arrives afterwards: an
/// opener can start and still fail to open the file. `on_refusal` is then
/// called once, on a worker thread, after this has returned `Ok`, with a
/// [`OpenRefusal::Failed`]: the opener does not say whether an application
/// was missing or merely failed to start.
pub fn open_with_desktop_reporting(
    path: &Path,
    mode: Option<u32>,
    on_refusal: impl FnOnce(OpenRefusal) + Send + 'static,
) -> Result<String, OpenRefusal> {
    if is_runnable(path, mode) {
        return Err(OpenRefusal::NeedsDeliberateRun);
    }
    if !path.exists() {
        return Err(OpenRefusal::Failed("the file no longer exists".into()));
    }

    let mut last_err: Option<String> = None;
    for opener in OPENERS {
        if which(opener).is_none() {
            continue;
        }
        let mut cmd = command_for(opener, path);
        crate::childenv::for_user_app(&mut cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.spawn() {
            Ok(child) => {
                watch_opener(opener, child, on_refusal);
                return Ok(opener.to_string());
            }
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    match last_err {
        Some(e) => Err(OpenRefusal::Failed(e)),
        None => Err(OpenRefusal::Failed("no desktop opener was found".into())),
    }
}

/// Find an executable on PATH. No shell, no `which(1)` dependency.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(program);
        match std::fs::metadata(&candidate) {
            Ok(m) if m.is_file() => {
                use std::os::unix::fs::PermissionsExt;
                if m.permissions().mode() & 0o111 != 0 {
                    Some(candidate)
                } else {
                    None
                }
            }
            _ => None,
        }
    })
}

/// Open a terminal in `dir`, for the "Open terminal here" action.
///
/// Honours `$TERMINAL` first, then the terminals an Omarchy install is
/// likely to have. Never falls back to a shell string.
/// The command that opens the user's own terminal in `dir`.
///
/// Omarchy (and any desktop following the xdg-terminal-exec spec) records
/// the chosen terminal there — its Super+Return uses it — so it comes
/// first. `--dir` is a separate argument: the path is never parsed by a
/// shell.
/// The argv that runs an executable file deliberately (the "Run…" action,
/// always behind a confirmation). It runs in the user's default terminal,
/// held open so its output can be read, with its own folder as the working
/// directory. Every part is a separate argv entry: no shell ever parses the
/// path, and the path is absolute, so it can never be read as an option.
///
/// Refused unless the file is a runnable regular file with an executable bit:
/// a script without +x is not "run" by guessing an interpreter, and content
/// with synthetic +x permissions still belongs in its desktop application.
pub fn run_argv(path: &Path, mode: u32, is_regular: bool) -> Result<Vec<OsString>, String> {
    if !path.is_absolute() {
        return Err("only a full path can be run".into());
    }
    if !is_regular {
        return Err("only an ordinary file can be run".into());
    }
    if mode & 0o111 == 0 {
        return Err("this file is not allowed to run — turn on Run in Permissions… first".into());
    }
    if !is_runnable(path, Some(mode)) {
        return Err("this file opens in an application — use Open instead of Run".into());
    }
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut working_dir = OsString::from("--dir=");
    working_dir.push(dir);
    Ok(vec![
        "xdg-terminal-exec".into(),
        "--hold".into(),
        working_dir,
        "--".into(),
        path.as_os_str().to_owned(),
    ])
}

/// The file named by a Run prompt, captured from the listing without I/O.
/// Mode is deliberately excluded: a permission worker may finish while the
/// prompt is open. Call `metadata` on the worker before chmod or launch.
#[derive(Debug, Clone)]
pub struct RunTarget {
    pub path: PathBuf,
    snapshot: crate::entry::Metadata,
}

impl RunTarget {
    pub fn from_snapshot(path: PathBuf, snapshot: crate::entry::Metadata) -> Self {
        Self { path, snapshot }
    }

    pub fn metadata(&self) -> Result<std::fs::Metadata, String> {
        use std::os::unix::fs::MetadataExt;
        let md = self
            .path
            .symlink_metadata()
            .map_err(|_| "that file is gone")?;
        let before = &self.snapshot;
        if !md.is_file()
            || (md.dev(), md.ino(), md.len(), md.mtime())
                != (before.dev, before.ino, before.size, before.mtime)
        {
            return Err("that file changed; choose Run again".into());
        }
        Ok(md)
    }

    pub fn argv(&self) -> Result<Vec<OsString>, String> {
        use std::os::unix::fs::MetadataExt;
        let md = self.metadata()?;
        run_argv(&self.path, md.mode(), md.is_file())
    }
}

/// Where a file stands as a program, for the activation prompt and the
/// Properties toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramState {
    /// Content, a folder, or not a local path: it opens, it is never run.
    NotAProgram,
    /// A program with an executable bit: Run… can start it.
    Runnable,
    /// A program that is not allowed to run yet — a downloaded AppImage or
    /// `install.sh` arrives without the x bit. Allowing it adds `u+x`.
    NeedsPermission,
}

/// Classify a file for Run. Only something that already looks like a
/// program is offered the permission; a document is never promoted to one.
pub fn program_state(path: &Path, mode: u32, is_regular: bool) -> ProgramState {
    if run_argv(path, mode, is_regular).is_ok() {
        return ProgramState::Runnable;
    }
    if is_regular && path.is_absolute() && mode & 0o111 == 0 && is_runnable(path, Some(mode)) {
        return ProgramState::NeedsPermission;
    }
    ProgramState::NotAProgram
}

pub fn terminal_command(dir: &Path, have: impl Fn(&str) -> bool) -> Option<Command> {
    if have("xdg-terminal-exec") {
        let mut c = Command::new("xdg-terminal-exec");
        c.arg(format!("--dir={}", dir.display()));
        return Some(c);
    }
    None
}

pub fn open_terminal_here(dir: &Path) -> Result<String, OpenRefusal> {
    if !dir.is_dir() {
        return Err(OpenRefusal::Failed("not a folder".into()));
    }
    if let Some(mut cmd) = terminal_command(dir, |p| which(p).is_some()) {
        crate::childenv::for_user_app(&mut cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(child) = cmd.spawn() {
            reap(child);
            return Ok("xdg-terminal-exec".into());
        }
    }
    let mut candidates: Vec<String> = Vec::new();
    if let Some(t) = std::env::var_os("TERMINAL") {
        candidates.push(t.to_string_lossy().into_owned());
    }
    for t in ["alacritty", "kitty", "foot", "ghostty", "wezterm"] {
        candidates.push(t.to_string());
    }

    for term in candidates {
        if which(&term).is_none() {
            continue;
        }
        let mut cmd = Command::new(&term);
        // Every one of these accepts a working directory flag; none of them
        // needs a shell to receive it.
        match term.as_str() {
            "kitty" | "ghostty" => {
                cmd.arg(format!("--working-directory={}", dir.display()));
            }
            _ => {
                cmd.arg("--working-directory").arg(dir);
            }
        }
        crate::childenv::for_user_app(&mut cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(child) = cmd.spawn() {
            reap(child);
            return Ok(term);
        }
    }
    Err(OpenRefusal::NoHandler)
}

/// Run a fixed, known command in a terminal.
///
/// Used only for `rclone config`, whose whole job is to ask the user for
/// credentials that Gust must never see or touch. The command is a compiled-in
/// constant and its arguments are passed as separate argv entries: no shell is
/// involved, so nothing here can be turned into command injection, and no
/// secret ever passes through Gust.
pub fn run_in_terminal(program: &str, args: &[impl AsRef<OsStr>]) -> Result<String, OpenRefusal> {
    if which(program).is_none() {
        return Err(OpenRefusal::NoHandler);
    }
    let terminal = std::env::var_os("TERMINAL").map(|t| t.to_string_lossy().into_owned());
    for (term, lead) in terminal_candidates(terminal.as_deref(), |p| which(p).is_some()) {
        let mut cmd = Command::new(&term);
        crate::childenv::for_user_app(&mut cmd)
            .arg(lead)
            .arg(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(child) = cmd.spawn() {
            reap(child);
            return Ok(term);
        }
    }
    Err(OpenRefusal::NoHandler)
}

/// The terminals to try for running a command, best first, each with the
/// argument that introduces the command.
///
/// `xdg-terminal-exec` leads, as in [`terminal_command`]: it is where the
/// desktop records the user's choice, so someone on Ghostty who also has
/// Alacritty installed gets Ghostty. Then `$TERMINAL`, then the terminals an
/// Omarchy install is likely to have. Every one of them takes the program
/// and its arguments as separate argv entries after that lead; none of them
/// needs a shell.
pub fn terminal_candidates(
    terminal_env: Option<&str>,
    have: impl Fn(&str) -> bool,
) -> Vec<(String, &'static str)> {
    let mut candidates = vec![("xdg-terminal-exec".to_string(), "--")];
    if let Some(t) = terminal_env.filter(|t| !t.is_empty()) {
        candidates.push((t.to_string(), "-e"));
    }
    for t in ["alacritty", "kitty", "foot", "ghostty", "wezterm"] {
        candidates.push((t.to_string(), "-e"));
    }
    candidates.retain(|(term, _)| have(term));
    candidates
}

/// Reveal `path` by opening its parent folder — used by "show in folder".
pub fn parent_of(path: &Path) -> Option<&Path> {
    path.parent().filter(|p| !p.as_os_str().is_empty())
}

/// Is this name safe to pass to a handler as a single argument?
///
/// Always true by construction — Gust passes real `Path`s, not strings — but
/// the check exists so a regression that starts building command lines is
/// caught by a test rather than by a user.
pub fn is_single_argument(path: &OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    !path.as_bytes().contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_runs_in_the_desktops_own_terminal_before_any_guess() {
        let all = terminal_candidates(Some("footclient"), |_| true);
        assert_eq!(all[0], ("xdg-terminal-exec".to_string(), "--"));
        assert_eq!(all[1], ("footclient".to_string(), "-e"));
        assert_eq!(all[2], ("alacritty".to_string(), "-e"));

        // Only what is installed is offered, in the same order.
        let some = terminal_candidates(None, |p| p == "ghostty" || p == "alacritty");
        let names: Vec<&str> = some.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, vec!["alacritty", "ghostty"]);
        assert!(terminal_candidates(None, |_| false).is_empty());
    }

    #[test]
    fn the_desktops_own_terminal_is_used_first() {
        let d = std::path::Path::new("/home/u/My Projects");
        let c = terminal_command(d, |p| p == "xdg-terminal-exec").expect("uses xdg-terminal-exec");
        assert_eq!(c.get_program(), "xdg-terminal-exec");
        let args: Vec<_> = c.get_args().collect();
        assert_eq!(
            args,
            vec![std::ffi::OsStr::new("--dir=/home/u/My Projects")]
        );
        assert!(
            terminal_command(d, |_| false).is_none(),
            "falls back when absent"
        );
    }

    #[test]
    fn a_terminal_command_never_goes_through_a_shell() {
        // The guard is structural: run_in_terminal takes a program and a
        // slice of arguments, so there is no string for a shell to reparse.
        // This test fails to compile if that signature is ever loosened to
        // take one command line.
        fn takes_argv<'a>(f: fn(&str, &[&'a str]) -> Result<String, OpenRefusal>) -> bool {
            let _ = f;
            true
        }
        assert!(takes_argv(run_in_terminal));
    }

    #[test]
    fn executables_are_never_opened_by_activation() {
        let p = Path::new("/tmp/whatever");
        assert!(is_runnable(p, Some(0o755)));
        assert!(is_runnable(p, Some(0o700)));
        assert!(!is_runnable(p, Some(0o644)));
    }

    #[test]
    fn executable_bits_on_content_do_not_require_run() {
        for name in [
            "IMG_0001.JPG",
            "clip.MOV",
            "memo.M4A",
            "report.PDF",
            "notes.txt",
        ] {
            let path = Path::new("/phone/DCIM").join(name);
            assert!(!is_runnable(&path, Some(0o100700)), "{name}");
            assert!(run_argv(&path, 0o100700, true)
                .unwrap_err()
                .contains("use Open instead of Run"));
        }
    }

    #[test]
    fn executable_js_and_ts_scripts_are_guarded_and_runnable() {
        for name in ["build.js", "tool.ts"] {
            let path = Path::new("/work/bin").join(name);
            assert!(is_runnable(&path, Some(0o100755)), "{name}");
            let argv = run_argv(&path, 0o100755, true).expect(name);
            assert_eq!(argv.last().map(OsString::as_os_str), Some(path.as_os_str()));
        }
        // Phone media with the same synthetic bits still opens normally.
        let photo = Path::new("/phone/DCIM/IMG_0001.JPG");
        assert!(!is_runnable(photo, Some(0o100755)));
    }

    #[test]
    fn script_extensions_count_as_runnable_even_without_the_bit() {
        // A script that arrived over the network may not be +x yet; opening
        // it must still not hand it to an interpreter.
        for name in [
            "install.sh",
            "setup.PY",
            "thing.AppImage",
            "run.bash",
            "x.fish",
        ] {
            assert!(
                is_runnable(Path::new(name), Some(0o644)),
                "{name} should need a deliberate Run"
            );
        }
        for name in ["notes.txt", "photo.png", "report.pdf", "data.csv"] {
            assert!(
                !is_runnable(Path::new(name), Some(0o644)),
                "{name} is a document"
            );
        }
    }

    #[test]
    fn open_refuses_an_executable_with_a_clear_reason() {
        let d = std::env::temp_dir().join(format!("gust-open-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("thing");
        std::fs::write(&exe, b"#!/bin/sh\necho hi\n").unwrap();
        let err = open_with_desktop(&exe, Some(0o755)).unwrap_err();
        assert_eq!(err, OpenRefusal::NeedsDeliberateRun);
        assert!(err.to_string().contains("Run"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_path_is_one_argument_and_no_shell_is_used() {
        // A filename containing shell metacharacters must survive as data.
        let nasty = Path::new("/tmp/a b; rm -rf ~/$(whoami)'\"`.txt");
        let cmd = command_for("xdg-open", nasty);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args.len(), 1, "exactly one argument");
        assert_eq!(args[0], nasty.as_os_str(), "passed verbatim, not expanded");
        assert_eq!(cmd.get_program(), "xdg-open");
    }

    #[test]
    fn gio_and_handlr_get_the_open_subcommand_and_one_path() {
        let path = Path::new("-two words.txt");
        for opener in ["gio", "handlr"] {
            let cmd = command_for(opener, path);
            assert_eq!(cmd.get_program(), opener);
            assert_eq!(
                cmd.get_args().collect::<Vec<_>>(),
                [OsStr::new("open"), OsStr::new("--"), path.as_os_str()]
            );
        }
    }

    /// A fake opener: a script in a temp dir, run by `sh` so the test never
    /// executes a file it has just written (no ETXTBSY) and never starts a
    /// real application.
    fn fake_opener(tag: &str, body: &str) -> (PathBuf, Child) {
        let d = std::env::temp_dir().join(format!("gust-opener-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let script = d.join("xdg-open");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        let child = Command::new("sh")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        (d, child)
    }

    #[test]
    fn an_opener_that_reports_failure_is_a_failure_not_a_missing_app() {
        for (code, reason) in [
            (3, "a required desktop tool is missing"),
            (4, OPENER_REPORTED_FAILURE),
        ] {
            let (d, child) = fake_opener(&format!("none-{code}"), &format!("exit {code}"));
            let (tx, rx) = std::sync::mpsc::channel();
            watch_opener("xdg-open", child, move |refusal| {
                let _ = tx.send(refusal);
            });
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(10)).ok(),
                Some(OpenRefusal::Failed(reason.into())),
                "xdg-open exit {code}"
            );
            std::fs::remove_dir_all(&d).ok();
        }
    }

    #[test]
    fn an_opener_that_succeeds_reports_nothing() {
        let (d, child) = fake_opener("fine", "exit 0");
        let (tx, rx) = std::sync::mpsc::channel::<OpenRefusal>();
        watch_opener("xdg-open", child, move |refusal| {
            let _ = tx.send(refusal);
        });
        // The sender is dropped unused once the opener has been reaped.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn only_a_prompt_xdg_open_failure_is_reported() {
        let soon = Duration::from_millis(200);
        for code in [Some(0), Some(1), Some(2), None] {
            assert_eq!(refusal_from_exit("xdg-open", code, soon), None, "{code:?}");
        }
        // xdg-open may live as long as the application: a failure an hour in
        // is that application ending badly, not a failed open.
        assert_eq!(
            refusal_from_exit("xdg-open", Some(4), Duration::from_secs(3600)),
            None
        );
    }

    #[test]
    fn gio_reports_a_dispatch_failure_even_if_the_desktop_took_time_to_reply() {
        assert_eq!(
            refusal_from_exit("gio", Some(2), Duration::from_secs(60)),
            Some(OpenRefusal::Failed(OPENER_REPORTED_FAILURE.into())),
        );
        for code in [Some(0), Some(1), Some(4), None] {
            assert_eq!(refusal_from_exit("gio", code, Duration::ZERO), None);
        }
    }

    #[test]
    fn handlr_reports_unsuccessful_dispatch() {
        let elapsed = Duration::from_secs(60);
        assert_eq!(refusal_from_exit("handlr", Some(0), elapsed), None);
        assert_eq!(
            refusal_from_exit("handlr", Some(1), elapsed),
            Some(OpenRefusal::Failed(OPENER_REPORTED_FAILURE.into())),
        );
    }

    #[test]
    fn a_detached_child_is_reaped_not_left_defunct() {
        let (d, child) = fake_opener("reap", "exit 0");
        let proc_entry = PathBuf::from(format!("/proc/{}", child.id()));
        reap(child);
        // A zombie keeps its /proc entry until its parent waits for it.
        let deadline = Instant::now() + Duration::from_secs(10);
        while proc_entry.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!proc_entry.exists(), "the child was never waited for");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_file_is_reported_not_opened() {
        let err =
            open_with_desktop(Path::new("/tmp/definitely-not-here-9f3a"), Some(0o644)).unwrap_err();
        assert!(matches!(err, OpenRefusal::Failed(_)));
    }

    #[test]
    fn which_finds_a_real_program_and_rejects_a_fake_one() {
        assert!(which("sh").is_some(), "sh must exist on PATH");
        assert!(which("definitely-not-a-program-zz9").is_none());
    }

    #[test]
    fn which_ignores_a_non_executable_file() {
        let d = std::env::temp_dir().join(format!("gust-which-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("notaprogram");
        std::fs::write(&f, b"data").unwrap();
        let old = std::env::var_os("PATH");
        std::env::set_var("PATH", &d);
        let found = which("notaprogram");
        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        assert!(found.is_none(), "a non-executable file is not a program");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn parent_of_a_root_path_is_handled() {
        assert_eq!(parent_of(Path::new("/tmp/a.txt")), Some(Path::new("/tmp")));
        assert_eq!(parent_of(Path::new("/")), None);
    }

    #[test]
    fn a_null_byte_is_not_a_valid_argument() {
        use std::os::unix::ffi::OsStrExt;
        assert!(is_single_argument(OsStr::new("fine.txt")));
        assert!(!is_single_argument(OsStr::from_bytes(b"bad\0name")));
    }

    #[test]
    fn refusal_messages_are_human_readable() {
        for r in [
            OpenRefusal::NotLocal,
            OpenRefusal::NeedsDeliberateRun,
            OpenRefusal::NoHandler,
            OpenRefusal::Failed("disk on fire".into()),
        ] {
            let s = r.to_string();
            assert!(!s.is_empty());
            assert!(
                s.chars().next().unwrap().is_lowercase() || s.starts_with("no"),
                "message reads as a sentence fragment: {s}"
            );
            // No errno codes or type names leak to the user.
            assert!(!s.contains("Errno"), "{s}");
            assert!(!s.contains("OpenRefusal"), "{s}");
        }
    }

    #[test]
    fn run_argv_is_argv_only_and_absolute() {
        let p = Path::new("/tmp/a $(touch PWNED); b.sh");
        let v = run_argv(p, 0o755, true).unwrap();
        assert_eq!(v[0], "xdg-terminal-exec");
        assert_eq!(v[3], "--", "everything after -- is the program");
        assert_eq!(
            v[4], "/tmp/a $(touch PWNED); b.sh",
            "the path is ONE argument"
        );
        assert_eq!(v[2], "--dir=/tmp");
    }

    #[test]
    fn run_argv_preserves_non_utf8_directory_and_filename_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let path = PathBuf::from(OsString::from_vec(b"/tmp/folder\xff/tool\xfe.sh".to_vec()));
        let argv = run_argv(&path, 0o755, true).unwrap();
        assert_eq!(argv[2].as_bytes(), b"--dir=/tmp/folder\xff");
        assert_eq!(argv[4].as_bytes(), path.as_os_str().as_bytes());
    }

    #[test]
    fn run_argv_refuses_what_is_not_an_executable_file() {
        assert!(run_argv(Path::new("/tmp/x.sh"), 0o644, true)
            .unwrap_err()
            .contains("turn on Run in Permissions"));
        assert!(run_argv(Path::new("/tmp/d"), 0o755, false).is_err());
        assert!(run_argv(Path::new("rel.sh"), 0o755, true).is_err());
    }

    #[test]
    fn a_program_without_the_bit_is_offered_the_permission() {
        let s = |name: &str, mode: u32, regular: bool| {
            program_state(&Path::new("/home/u/Downloads").join(name), mode, regular)
        };
        // Downloaded programs arrive 644: they need permission, not a guess.
        assert_eq!(
            s("Synara-1.0.9-x86_64.AppImage", 0o100644, true),
            ProgramState::NeedsPermission
        );
        assert_eq!(
            s("install.sh", 0o100644, true),
            ProgramState::NeedsPermission
        );
        assert_eq!(s("install.sh", 0o100755, true), ProgramState::Runnable);
        // An extensionless binary with the bit runs; without it, it is
        // indistinguishable from data and is not promoted.
        assert_eq!(s("tool", 0o100755, true), ProgramState::Runnable);
        assert_eq!(s("tool", 0o100644, true), ProgramState::NotAProgram);
        // Content stays content, with or without bits.
        for name in ["report.pdf", "notes.txt", "photo.png", "build.js"] {
            assert_eq!(s(name, 0o100644, true), ProgramState::NotAProgram, "{name}");
        }
        assert_eq!(s("IMG_0001.JPG", 0o100700, true), ProgramState::NotAProgram);
        // Only regular files at absolute paths.
        assert_eq!(
            s("app.AppImage", 0o040755, false),
            ProgramState::NotAProgram
        );
        assert_eq!(
            program_state(Path::new("rel.AppImage"), 0o100644, true),
            ProgramState::NotAProgram
        );
    }
}
