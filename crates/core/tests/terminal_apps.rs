//! Terminal applications (`Terminal=true`, e.g. nvim) must be started inside
//! the user's terminal, however many files were selected. Started bare they
//! have nothing to draw on: the launch "succeeds" and nothing appears.
//!
//! No real application runs here. PATH is pointed at a temp directory of
//! stand-in scripts that only write down how they were called.

use gust_core::open::with::{launch_many, Handler, MultiOpen};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// PATH and TERMINAL are process-wide; one test at a time may own them.
static ENV: Mutex<()> = Mutex::new(());

const DONE: &str = "gust-test-end-of-argv";

/// A directory of stand-in programs, each of which records its own argv, one
/// argument per line, in `<name>.called`.
struct Stage {
    dir: PathBuf,
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
        for name in programs {
            let script = dir.join(name);
            let log = dir.join(format!("{name}.called"));
            // Shell builtins only (PATH holds nothing else); the last line
            // marks the record complete.
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$@\" {DONE} > '{}'\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let stage = Stage {
            old_path: std::env::var_os("PATH"),
            old_terminal: std::env::var_os("TERMINAL"),
            dir,
            _env: env,
        };
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

impl Drop for Stage {
    fn drop(&mut self) {
        match self.old_path.take() {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        if let Some(t) = self.old_terminal.take() {
            std::env::set_var("TERMINAL", t);
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
    let stage = Stage::new("none", &["nvim"]);
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
