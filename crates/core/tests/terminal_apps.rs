//! Terminal applications (`Terminal=true`, e.g. nvim) must be started inside
//! the user's terminal, however many files were selected. Started bare they
//! have nothing to draw on: the launch "succeeds" and nothing appears.
//!
//! An isolated PATH contains stand-in launchers and a target that records
//! its arguments and exits.

use gust_core::open::with::{launch, launch_many, Handler, MultiOpen};
use gust_core::open::{open_terminal_here, run_in_terminal, OpenRefusal};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The launcher environment is process-wide; one test at a time may own them.
static ENV: Mutex<()> = Mutex::new(());

const DONE: &str = "gust-test-end-of-argv";

// Each test registry has at most one entry. Match the reference launcher's
// missing-entry refusal before it inspects any command-line options.
const SELECT_ENTRY: &str = r#"
for entry in "$XDG_DATA_HOME"/applications/*.desktop; do
    [ -f "$entry" ] || exit 1
    break
done"#;
const PRINT_ID: &str = r#"
if [ "$1" = --print-id ]; then
    [ "$#" -eq 1 ] || exit 1
    echo "${entry##*/}"
    exit 0
fi"#;

/// A directory of stand-in programs, each of which records its own argv, one
/// argument per line, in `<name>.called`.
struct Stage {
    dir: PathBuf,
    old_data_home: Option<std::ffi::OsString>,
    old_path: Option<std::ffi::OsString>,
    old_terminal: Option<std::ffi::OsString>,
    _env: std::sync::MutexGuard<'static, ()>,
}

impl Stage {
    fn new(tag: &str, programs: &[&str]) -> Self {
        let env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("gust-term-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("applications")).unwrap();
        std::fs::write(
            dir.join("applications/fixture.desktop"),
            format!("Exec={}/selected-terminal\n", dir.display()),
        )
        .unwrap();
        for name in programs {
            let probe = if *name == "xdg-terminal-exec" {
                format!("{SELECT_ENTRY}\n{PRINT_ID}")
            } else {
                String::new()
            };
            write_script(
                &dir.join(name),
                &format!("{probe}\nprintf '%s\\n' \"$@\" {DONE} > \"$0.called\""),
            );
        }
        let stage = Stage {
            old_data_home: std::env::var_os("XDG_DATA_HOME"),
            old_path: std::env::var_os("PATH"),
            old_terminal: std::env::var_os("TERMINAL"),
            dir,
            _env: env,
        };
        std::env::set_var("XDG_DATA_HOME", &stage.dir);
        std::env::set_var("PATH", &stage.dir);
        std::env::remove_var("TERMINAL");
        stage
    }

    /// The first stand-in to be called, and the arguments it was given.
    fn first_called(&self, programs: &[&str]) -> (String, Vec<String>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for name in programs {
                let text = std::fs::read_to_string(self.dir.join(format!("{name}.called")));
                let mut args: Vec<String> = text
                    .iter()
                    .flat_map(|t| t.lines())
                    .map(String::from)
                    .collect();
                if args.last().map(String::as_str) == Some(DONE) {
                    args.pop();
                    return (name.to_string(), args);
                }
            }
            assert!(Instant::now() < deadline, "nothing was started");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn was_called(&self, name: &str) -> bool {
        self.dir.join(format!("{name}.called")).exists()
    }
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Drop for Stage {
    fn drop(&mut self) {
        match self.old_data_home.take() {
            Some(p) => std::env::set_var("XDG_DATA_HOME", p),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match self.old_path.take() {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        match self.old_terminal.take() {
            Some(t) => std::env::set_var("TERMINAL", t),
            None => std::env::remove_var("TERMINAL"),
        }
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn nvim() -> Handler {
    Handler {
        id: "nvim.desktop".into(),
        name: "Neovim".into(),
        exec: "nvim %F".into(),
        terminal: true,
        is_default: true,
    }
}

#[test]
fn several_files_for_a_terminal_app_open_inside_a_terminal() {
    let programs = ["xdg-terminal-exec", "nvim"];
    let stage = Stage::new("many", &programs);
    let paths = vec![
        PathBuf::from("/tmp/a b.txt"),
        PathBuf::from("/tmp/; rm -rf ~"),
    ];

    let outcome = launch_many(&nvim(), &paths);
    assert_eq!(
        outcome,
        MultiOpen::Together {
            app: "Neovim".into(),
            count: 2
        }
    );

    let (who, args) = stage.first_called(&programs);
    assert_eq!(
        who, "xdg-terminal-exec",
        "nvim was started without a terminal"
    );
    // One launch, argv form: every path is still exactly one argument.
    assert_eq!(args, vec!["--", "nvim", "/tmp/a b.txt", "/tmp/; rm -rf ~"]);
    assert!(!stage.was_called("nvim"));
}

#[test]
fn a_terminal_app_with_no_terminal_to_run_in_is_refused_not_faked() {
    let stage = Stage::new("none", &["xdg-terminal-exec", "nvim"]);
    std::fs::remove_file(stage.dir.join("applications/fixture.desktop")).unwrap();
    assert_eq!(open_terminal_here(&stage.dir), Err(OpenRefusal::NoHandler));
    assert_eq!(
        run_in_terminal("nvim", &["one"]),
        Err(OpenRefusal::NoHandler)
    );
    let outcome = launch_many(
        &nvim(),
        &[PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b")],
    );
    assert!(matches!(outcome, MultiOpen::Refused(_)), "got {outcome:?}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!stage.was_called("nvim"), "started bare, with no window");
}

#[test]
fn the_desktops_chosen_terminal_wins_over_whatever_else_is_installed() {
    // A Ghostty user who also has Alacritty installed: xdg-terminal-exec is
    // where Omarchy records the choice, so it is asked before any guess.
    let programs = ["xdg-terminal-exec", "alacritty", "ghostty"];
    let stage = Stage::new("pref", &[&programs[..], &["rclone"]].concat());

    let term = gust_core::open::run_in_terminal("rclone", &["config"]).expect("starts");
    assert_eq!(term, "xdg-terminal-exec");
    let (who, args) = stage.first_called(&programs);
    assert_eq!(who, "xdg-terminal-exec");
    assert_eq!(args, vec!["--", "rclone", "config"]);
}

#[test]
fn without_xdg_terminal_exec_a_known_terminal_is_still_found() {
    let programs = ["alacritty"];
    let stage = Stage::new("fallback", &[&programs[..], &["rclone"]].concat());

    let term = gust_core::open::run_in_terminal("rclone", &["config"]).expect("starts");
    assert_eq!(term, "alacritty");
    let (_, args) = stage.first_called(&programs);
    assert_eq!(args, vec!["-e", "rclone", "config"]);
}

#[test]
fn unresolved_desktop_entry_uses_terminal_env_then_named_terminal() {
    let stage = Stage::new(
        "unresolved",
        &["xdg-terminal-exec", "my-term", "alacritty", "nvim"],
    );
    std::fs::remove_file(stage.dir.join("applications/fixture.desktop")).unwrap();
    std::env::set_var("TERMINAL", "my-term");
    assert_eq!(open_terminal_here(&stage.dir).unwrap(), "my-term");
    let (_, args) = stage.first_called(&["my-term"]);
    assert_eq!(args, ["--working-directory", stage.dir.to_str().unwrap()]);

    // An explicit path to the same unresolved launcher must not bypass the lookup.
    std::env::set_var("TERMINAL", stage.dir.join("xdg-terminal-exec"));
    assert_eq!(run_in_terminal("nvim", &["one"]).unwrap(), "alacritty");
    assert_eq!(stage.first_called(&["alacritty"]).1, ["-e", "nvim", "one"]);
    assert!(!stage.was_called("xdg-terminal-exec"));
}

#[test]
fn legacy_launcher_discarding_unknown_options_never_opens_a_probe_terminal() {
    let stage = Stage::new(
        "legacy",
        &["xdg-terminal-exec", "selected-terminal", "foot", "rclone"],
    );
    // v0.12.4 discards --print-id, then execs the selected entry even if
    // no arguments remain. Record that launch if it reaches our real entry.
    write_script(
        &stage.dir.join("xdg-terminal-exec"),
        &format!(
            r#"{SELECT_ENTRY}
test_mode=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --test) test_mode=true; shift ;;
        --) shift; break ;;
        -*) shift ;;
        *) break ;;
    esac
done
while IFS= read -r line; do
    case "$line" in Exec=*)
        program=${{line#Exec=}}
        if "$test_mode"; then
            printf 'Command and arguments:\n  >%s<\n' "$program"
            exit 0
        fi
        exec "$program" "$@" ;;
    esac
done < "$entry"
exit 1"#
        ),
    );
    assert_eq!(run_in_terminal("rclone", &["config"]).unwrap(), "foot");
    assert_eq!(stage.first_called(&["foot"]).1, ["-e", "rclone", "config"]);
    std::fs::remove_file(stage.dir.join("foot")).unwrap();
    assert_eq!(open_terminal_here(&stage.dir), Err(OpenRefusal::NoHandler));
    assert!(!stage.was_called("selected-terminal"));
}

#[test]
fn a_failing_target_runs_once_with_its_original_file_arguments() {
    let stage = Stage::new("dispatch", &["xdg-terminal-exec", "foot", "nvim"]);
    // The launcher execs the target. Its eventual nonzero status is therefore
    // indistinguishable from a desktop-entry lookup failure after dispatch.
    write_script(
        &stage.dir.join("xdg-terminal-exec"),
        &format!(
            r#"{SELECT_ENTRY}
{PRINT_ID}
[ "$1" = -- ] || exit 2
shift
exec "$@""#
        ),
    );
    write_script(
        &stage.dir.join("nvim"),
        r#"
printf '%s\0' "$@" >> "$0.argv"
echo "$$" >> "$0.pids"
exit 1
"#,
    );
    let a = PathBuf::from(OsStr::from_bytes(b"/tmp/a\xff \n"));
    let b = PathBuf::from("/tmp/; $HOME");
    assert_eq!(launch(&nvim(), &a).unwrap(), "Neovim");
    let wait_reaped = |count| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pids = std::fs::read_to_string(stage.dir.join("nvim.pids")).unwrap_or_default();
            if pids.lines().count() == count
                && pids
                    .lines()
                    .all(|pid| !Path::new("/proc").join(pid).exists())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "target did not finish and get reaped: {pids}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait_reaped(1);
    assert_eq!(
        launch_many(&nvim(), &[a.clone(), b.clone()]),
        MultiOpen::Together {
            app: "Neovim".into(),
            count: 2
        }
    );
    wait_reaped(2);
    let expected: Vec<_> = [&a, &a, &b]
        .into_iter()
        .flat_map(|p| p.as_os_str().as_bytes().iter().copied().chain([0]))
        .collect();
    assert_eq!(
        std::fs::read(stage.dir.join("nvim.argv")).unwrap(),
        expected
    );
    assert!(!stage.was_called("foot"), "retried after the target ran");
}
