//! Local capability setup. Call blocking functions only on a worker.
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupRequirement {
    pub key: String,
    /// Only packages whose local helpers are missing; empty means ready.
    pub packages: Vec<String>,
    pub installable: bool,
}

trait Probe {
    fn executable(&self, path: &str) -> bool;
    fn file(&self, path: &str) -> bool;
}

const KEYS: [&str; 7] = ["ssh", "cloud", "apple", "android", "camera", "smb", "nfs"];

fn missing(probe: &impl Probe, key: &str) -> Vec<&'static str> {
    let mut packages = Vec::new();
    let mut require = |ready: bool, package| {
        if !ready {
            packages.push(package);
        }
    };
    match key {
        "ssh" => {
            require(probe.executable("/usr/bin/sshfs"), "sshfs");
            require(
                [
                    "/usr/bin/ssh",
                    "/usr/bin/ssh-keyscan",
                    "/usr/bin/ssh-keygen",
                ]
                .iter()
                .all(|path| probe.executable(path)),
                "openssh",
            );
        }
        "cloud" => require(probe.executable("/usr/bin/rclone"), "rclone"),
        _ => {
            let (backend, package) = match key {
                "apple" => ("afc", "gvfs-afc"),
                "android" => ("mtp", "gvfs-mtp"),
                "camera" => ("gphoto2", "gvfs-gphoto2"),
                "smb" => ("smb", "gvfs-smb"),
                "nfs" => ("nfs", "gvfs-nfs"),
                _ => return packages,
            };
            require(
                probe.file(&format!("/usr/share/gvfs/mounts/{backend}.mount")),
                package,
            );
            require(probe.executable("/usr/bin/gio"), "glib2");
            require(probe.executable("/usr/lib/gvfsd-fuse"), "gvfs");
        }
    }
    if matches!(key, "ssh" | "cloud") {
        require(
            probe.executable("/usr/bin/fusermount3") || probe.executable("/usr/bin/fusermount"),
            "fuse3",
        );
    }
    packages
}

fn snapshot_with(probe: &impl Probe) -> Vec<SetupRequirement> {
    KEYS.iter()
        .map(|key| SetupRequirement {
            key: (*key).into(),
            packages: missing(probe, key).into_iter().map(String::from).collect(),
            installable: installer(probe).is_some(),
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct InstallPlan {
    program: &'static str,
    args: Vec<&'static str>,
}

fn installer(probe: &impl Probe) -> Option<Vec<&'static str>> {
    if !probe.executable("/usr/bin/pkexec") {
        return None;
    }
    if probe.executable("/usr/bin/omarchy") {
        Some(vec!["/usr/bin/omarchy", "pkg", "add"])
    } else if probe.file("/etc/arch-release") && probe.executable("/usr/bin/pacman") {
        Some(vec![
            "/usr/bin/pacman",
            "-S",
            "--needed",
            "--noconfirm",
            "--",
        ])
    } else {
        None
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    if KEYS.contains(&key) {
        Ok(())
    } else {
        Err("Unknown setup capability.".into())
    }
}

fn plan(probe: &impl Probe, key: &str) -> Result<Option<InstallPlan>, String> {
    validate_key(key)?;
    let packages = missing(probe, key);
    if packages.is_empty() {
        return Ok(None);
    }
    let mut args = installer(probe).ok_or("Setup is not supported on this system. Install the listed packages using your system package manager, then check again.")?;
    args.extend(packages);
    Ok(Some(InstallPlan {
        program: "/usr/bin/pkexec",
        args,
    }))
}

// This belongs to the blocking worker, never to a tab or its result receiver.
static INSTALL_BUSY: AtomicBool = AtomicBool::new(false);

struct InstallGuard<'a>(&'a AtomicBool);

impl<'a> InstallGuard<'a> {
    fn acquire(busy: &'a AtomicBool) -> Result<Self, String> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self(busy))
            .map_err(|_| "Support setup is already in progress. Wait for it to finish.".into())
    }
}

impl Drop for InstallGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn install_with(
    probe: &impl Probe,
    key: &str,
    busy: &AtomicBool,
    run: impl FnOnce(&InstallPlan) -> Result<bool, String>,
) -> Result<(), String> {
    validate_key(key)?;
    let _guard = InstallGuard::acquire(busy)?;
    let Some(planned) = plan(probe, key)? else {
        return Ok(());
    };
    match run(&planned) {
        Ok(true) => {},
        Ok(false) => return Err("Setup failed or was cancelled. Support may be partially installed; check again before trying again.".into()),
        Err(_) => return Err("Could not complete setup. Support may be partially installed; check again before trying again.".into()),
    }
    if !missing(probe, key).is_empty() {
        return Err("Setup finished, but required support is still missing. Check again before trying again.".into());
    }
    Ok(())
}

struct LocalProbe;
impl Probe for LocalProbe {
    fn executable(&self, path: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let executable = |candidate: &std::path::Path| {
            std::fs::metadata(candidate)
                .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        };
        let name = path.strip_prefix("/usr/bin/").unwrap_or("");
        if [
            "sshfs",
            "ssh",
            "ssh-keyscan",
            "ssh-keygen",
            "rclone",
            "fusermount3",
            "fusermount",
            "gio",
        ]
        .contains(&name)
        {
            let search = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
            return std::env::split_paths(&search)
                .any(|directory| executable(&directory.join(name)));
        }
        // Authorization and installer executables stay absolute, never PATH-selected.
        executable(std::path::Path::new(path))
    }
    fn file(&self, path: &str) -> bool {
        if let Some(backend) = path.strip_prefix("/usr/share/gvfs/mounts/") {
            if let Some(root) = std::env::var_os("GUST_GVFS_MOUNTS") {
                return std::path::Path::new(&root).join(backend).is_file();
            }
        }
        std::path::Path::new(path).is_file()
    }
}

/// Read local helper availability only. Blocking: call on a worker.
pub fn snapshot() -> Vec<SetupRequirement> {
    snapshot_with(&LocalProbe)
}

/// Explicit user action only. Blocking: retain the worker until this returns.
/// Closing a tab must not kill the package transaction; this never reconnects.
pub fn install(key: &str) -> Result<(), String> {
    install_with(&LocalProbe, key, &INSTALL_BUSY, run_plan)
}

fn run_plan(plan: &InstallPlan) -> Result<bool, String> {
    let mut command = std::process::Command::new(plan.program);
    command.args(&plan.args);
    run_command(command)
}

fn run_command(mut command: std::process::Command) -> Result<bool, String> {
    use std::process::Stdio;
    // No shell, terminal, captured output or deadline: killing a package
    // manager midway through a transaction can damage its database.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|_| "Could not run the system package installer.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Default)]
    struct FakeProbe {
        executables: HashSet<String>,
        files: HashSet<String>,
    }
    impl Probe for FakeProbe {
        fn executable(&self, path: &str) -> bool {
            self.executables.contains(path)
        }
        fn file(&self, path: &str) -> bool {
            self.files.contains(path)
        }
    }

    fn ready_probe() -> FakeProbe {
        FakeProbe {
            executables: [
                "sshfs",
                "ssh",
                "ssh-keyscan",
                "ssh-keygen",
                "rclone",
                "fusermount3",
                "gio",
            ]
            .map(|name| format!("/usr/bin/{name}"))
            .into_iter()
            .chain(["/usr/lib/gvfsd-fuse".into()])
            .collect(),
            files: ["afc", "mtp", "gphoto2", "smb", "nfs"]
                .map(|name| format!("/usr/share/gvfs/mounts/{name}.mount"))
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn planner_uses_absolute_omarchy_or_arch_argv_and_only_fresh_missing_tokens() {
        let mut probe = ready_probe();
        probe.executables.extend([
            "/usr/bin/pkexec".into(),
            "/usr/bin/omarchy".into(),
            "/usr/bin/pacman".into(),
        ]);
        probe.files.insert("/etc/arch-release".into());
        probe.executables.remove("/usr/bin/ssh-keygen");
        assert_eq!(
            plan(&probe, "ssh").unwrap(),
            Some(InstallPlan {
                program: "/usr/bin/pkexec",
                args: vec!["/usr/bin/omarchy", "pkg", "add", "openssh"],
            })
        );
        assert!(snapshot_with(&probe).iter().all(|row| row.installable));
        probe.executables.remove("/usr/bin/omarchy");
        assert_eq!(
            plan(&probe, "ssh").unwrap(),
            Some(InstallPlan {
                program: "/usr/bin/pkexec",
                args: vec![
                    "/usr/bin/pacman",
                    "-S",
                    "--needed",
                    "--noconfirm",
                    "--",
                    "openssh"
                ],
            })
        );
        probe.executables.remove("/usr/bin/fusermount3");
        assert_eq!(
            plan(&probe, "cloud").unwrap().unwrap().args.last(),
            Some(&"fuse3")
        );
        probe.executables.remove("/usr/bin/pkexec");
        assert!(plan(&probe, "ssh").unwrap_err().contains("not supported"));
        assert!(snapshot_with(&probe).iter().all(|row| !row.installable));
    }

    #[test]
    fn ready_is_noop_even_on_unsupported_system_and_missing_is_never_hidden() {
        let mut probe = ready_probe();
        for key in KEYS {
            assert_eq!(plan(&probe, key).unwrap(), None);
        }
        assert!(snapshot_with(&probe)
            .iter()
            .all(|row| row.packages.is_empty()));
        probe.executables.remove("/usr/bin/fusermount3");
        probe.executables.insert("/usr/bin/fusermount".into());
        assert!(missing(&probe, "ssh").is_empty());
        for path in [
            "/usr/bin/ssh",
            "/usr/bin/ssh-keyscan",
            "/usr/bin/ssh-keygen",
        ] {
            probe.executables.remove(path);
            assert_eq!(missing(&probe, "ssh"), ["openssh"]);
            probe.executables.insert(path.into());
        }
        for (key, backend) in [
            ("apple", "afc"),
            ("android", "mtp"),
            ("camera", "gphoto2"),
            ("smb", "smb"),
            ("nfs", "nfs"),
        ] {
            let path = format!("/usr/share/gvfs/mounts/{backend}.mount");
            probe.files.remove(&path);
            assert_eq!(missing(&probe, key).len(), 1);
            probe.files.insert(path);
            probe.executables.remove("/usr/bin/gio");
            assert_eq!(missing(&probe, key), ["glib2"]);
            probe.executables.insert("/usr/bin/gio".into());
            probe.executables.remove("/usr/lib/gvfsd-fuse");
            assert_eq!(missing(&probe, key), ["gvfs"]);
            probe.executables.insert("/usr/lib/gvfsd-fuse".into());
        }
    }

    #[test]
    fn invalid_keys_including_package_and_shell_injection_are_rejected_before_probing() {
        struct NoProbe;
        impl Probe for NoProbe {
            fn executable(&self, _: &str) -> bool {
                panic!("invalid key was probed")
            }
            fn file(&self, _: &str) -> bool {
                panic!("invalid key was probed")
            }
        }
        for key in [
            "",
            "SSH",
            " ssh",
            "ssh ",
            "gvfs-afc",
            "--noconfirm",
            "ssh;touch /x",
            "apple\nandroid",
            "$(id)",
            "ssh\0",
        ] {
            assert!(plan(&NoProbe, key)
                .unwrap_err()
                .contains("Unknown setup capability"));
        }
    }

    #[test]
    fn fallback_requires_arch_marker_and_both_exact_executables() {
        let mut probe = FakeProbe::default();
        probe
            .executables
            .extend(["/usr/bin/pkexec".into(), "/usr/bin/pacman".into()]);
        assert!(plan(&probe, "apple").is_err());
        probe.files.insert("/etc/arch-release".into());
        assert_eq!(
            plan(&probe, "apple").unwrap().unwrap().args,
            [
                "/usr/bin/pacman",
                "-S",
                "--needed",
                "--noconfirm",
                "--",
                "gvfs-afc",
                "glib2",
                "gvfs"
            ]
        );
        probe.executables.remove("/usr/bin/pacman");
        assert!(plan(&probe, "apple").is_err());
    }

    #[test]
    fn quiet_child_fixture() {
        let Some(marker) = std::env::var_os("GUST_SETUP_TEST_MARKER") else {
            return;
        };
        for fd in [0, 1, 2] {
            assert_eq!(
                std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap(),
                std::path::Path::new("/dev/null")
            );
        }
        std::fs::write(marker, "quiet child finished").unwrap();
    }

    #[test]
    fn process_runner_waits_for_a_real_quiet_child_and_checks_exit_or_spawn_failure() {
        let marker = std::env::temp_dir().join(format!("gust-setup-child-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "setup::tests::quiet_child_fixture",
                "--nocapture",
            ])
            .env("GUST_SETUP_TEST_MARKER", &marker);
        assert_eq!(run_command(child), Ok(true));
        let written =
            std::fs::read_to_string(&marker).expect("real child must finish before runner returns");
        std::fs::remove_file(marker).unwrap();
        assert_eq!(written, "quiet child finished");
        assert_eq!(
            run_plan(&InstallPlan {
                program: "/usr/bin/false",
                args: vec![]
            }),
            Ok(false)
        );
        assert!(run_plan(&InstallPlan {
            program: "/nonexistent/gust-setup-test",
            args: vec![]
        })
        .is_err());
    }

    #[test]
    fn local_probe_requires_regular_executable_files_and_public_api_has_exact_types() {
        use std::os::unix::fs::PermissionsExt;
        let _: fn() -> Vec<SetupRequirement> = snapshot;
        let _: fn(&str) -> Result<(), String> = install;
        assert!(install("ssh;uname")
            .unwrap_err()
            .contains("Unknown setup capability"));
        let root = std::env::temp_dir().join(format!("gust-setup-probe-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("helper");
        std::fs::write(&path, "not executed").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(LocalProbe.file(path.to_str().unwrap()));
        assert!(!LocalProbe.executable(path.to_str().unwrap()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(LocalProbe.executable(path.to_str().unwrap()));
        assert!(!LocalProbe.executable(root.to_str().unwrap()));
        assert!(!LocalProbe.file(root.to_str().unwrap()));
        assert!(!LocalProbe.file(root.join("missing").to_str().unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn installable_probe() -> FakeProbe {
        let mut probe = FakeProbe::default();
        probe
            .executables
            .extend(["/usr/bin/pkexec".into(), "/usr/bin/omarchy".into()]);
        probe
    }

    #[test]
    fn install_reprobes_and_reports_postcheck_failure_without_connecting_or_retrying() {
        use std::cell::Cell;
        struct ChangingProbe {
            ready: Cell<bool>,
            base: FakeProbe,
        }
        impl Probe for ChangingProbe {
            fn executable(&self, path: &str) -> bool {
                self.ready.get() || self.base.executable(path)
            }
            fn file(&self, path: &str) -> bool {
                self.ready.get() || self.base.file(path)
            }
        }
        let probe = ChangingProbe {
            ready: Cell::new(false),
            base: installable_probe(),
        };
        let busy = std::sync::atomic::AtomicBool::new(false);
        let mut calls = 0;
        install_with(&probe, "apple", &busy, |planned| {
            calls += 1;
            assert_eq!(
                planned.args,
                [
                    "/usr/bin/omarchy",
                    "pkg",
                    "add",
                    "gvfs-afc",
                    "glib2",
                    "gvfs"
                ]
            );
            probe.ready.set(true);
            Ok(true)
        })
        .unwrap();
        assert_eq!(calls, 1);
        install_with(&probe, "apple", &busy, |_| panic!("ready must not spawn")).unwrap();
        probe.ready.set(false);
        let error = install_with(&probe, "apple", &busy, |_| Ok(true)).unwrap_err();
        assert!(error.contains("still missing"), "{error}");
        assert!(!busy.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn failed_or_cancelled_install_is_honest_and_releases_guard_on_every_exit() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let busy = AtomicBool::new(false);
        let probe = installable_probe();
        let error = install_with(&probe, "ssh", &busy, |_| Ok(false)).unwrap_err();
        assert!(
            error.contains("cancelled") && error.contains("partially installed"),
            "{error}"
        );
        assert!(!busy.load(Ordering::Acquire));
        let error = install_with(&probe, "ssh", &busy, |_| Err("spawn failed".into())).unwrap_err();
        assert!(error.contains("partially installed"), "{error}");
        assert!(!busy.load(Ordering::Acquire));
        assert!(
            install_with(&FakeProbe::default(), "ssh", &busy, |_| panic!(
                "unsupported must not spawn"
            ))
            .is_err()
        );
        assert!(!busy.load(Ordering::Acquire));
        assert!(install_with(&probe, "ssh;echo x", &busy, |_| panic!(
            "invalid must not spawn"
        ))
        .is_err());
        assert!(!busy.load(Ordering::Acquire));
        install_with(&ready_probe(), "ssh", &busy, |_| {
            panic!("ready must not spawn")
        })
        .unwrap();
        assert!(!busy.load(Ordering::Acquire));
        let panic = std::panic::catch_unwind(|| {
            let _ = install_with(&probe, "ssh", &busy, |_| panic!("injected runner panic"));
        });
        assert!(panic.is_err());
        assert!(!busy.load(Ordering::Acquire));
    }

    #[test]
    fn retained_worker_serializes_all_keys_even_after_its_ui_owner_disappears() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        };
        let busy = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let (owner_tx, owner_rx) = mpsc::channel();
        let worker_busy = busy.clone();
        let worker = std::thread::spawn(move || {
            let result = install_with(&installable_probe(), "ssh", &worker_busy, |_| {
                entered_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                Ok(false)
            });
            let _ = owner_tx.send(result);
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("installer runner was not reached");
        drop(owner_rx);
        let held = busy.load(Ordering::Acquire);
        let contender = install_with(&installable_probe(), "cloud", &busy, |_| {
            panic!("must not overlap")
        });
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            held,
            "worker must hold the global gate while awaiting its transaction"
        );
        assert!(contender.unwrap_err().contains("already in progress"));
        assert!(!busy.load(Ordering::Acquire));
        assert!(
            install_with(&installable_probe(), "cloud", &busy, |_| Ok(false))
                .unwrap_err()
                .contains("cancelled")
        );
    }

    #[test]
    fn local_probe_matches_the_transport_environment_in_a_private_child() {
        let root = std::env::temp_dir().join(format!("gust-setup-env-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("mounts")).unwrap();
        std::fs::write(root.join("mounts/afc.mount"), "fixture").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "setup::tests::local_probe_environment_child",
                "--nocapture",
            ])
            .env("GUST_SETUP_PROBE_FIXTURE", &root)
            .env("GUST_GVFS_MOUNTS", root.join("mounts"))
            .env("PATH", root.join("bin"))
            .status()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            status.success(),
            "Capability checks must match the environment used by transports"
        );
    }

    #[test]
    fn local_probe_environment_child() {
        let Some(root) = std::env::var_os("GUST_SETUP_PROBE_FIXTURE").map(std::path::PathBuf::from)
        else {
            return;
        };
        assert!(
            !LocalProbe.executable("/usr/bin/sshfs"),
            "A helper outside PATH is not available to Command::new"
        );
        assert!(
            LocalProbe.file("/usr/share/gvfs/mounts/afc.mount"),
            "Honor the same backend root as phone discovery"
        );
        std::os::unix::fs::symlink("/usr/bin/true", root.join("bin/sshfs")).unwrap();
        assert!(LocalProbe.executable("/usr/bin/sshfs"));
        std::fs::remove_file(root.join("mounts/afc.mount")).unwrap();
        assert!(!LocalProbe.file("/usr/share/gvfs/mounts/afc.mount"));
    }

    #[test]
    fn snapshot_has_exact_seven_keys_static_missing_packages_and_serializable_fields() {
        let rows = snapshot_with(&FakeProbe::default());
        let expected = [
            ("ssh", vec!["sshfs", "openssh", "fuse3"]),
            ("cloud", vec!["rclone", "fuse3"]),
            ("apple", vec!["gvfs-afc", "glib2", "gvfs"]),
            ("android", vec!["gvfs-mtp", "glib2", "gvfs"]),
            ("camera", vec!["gvfs-gphoto2", "glib2", "gvfs"]),
            ("smb", vec!["gvfs-smb", "glib2", "gvfs"]),
            ("nfs", vec!["gvfs-nfs", "glib2", "gvfs"]),
        ];
        assert_eq!(rows.len(), expected.len());
        for (row, (key, packages)) in rows.iter().zip(expected) {
            assert_eq!(row.key, key);
            assert_eq!(row.packages, packages);
            assert!(!row.installable);
            let json = serde_json::to_value(row).unwrap();
            assert_eq!(json.as_object().unwrap().len(), 3);
            assert_eq!(json["key"], key);
            assert_eq!(json["installable"], false);
        }
    }
}
