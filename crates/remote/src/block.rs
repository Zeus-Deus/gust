//! Worker-only device actions over the shared owned helper lifecycle.

use crate::helper::{self, CallContext, ErrorCode, RemoteError};
use gust_core::devices;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockEffect {
    NotStarted,
    Unknown,
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockFailure {
    pub error: RemoteError,
    pub effect: BlockEffect,
}

impl std::fmt::Display for BlockFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for BlockFailure {}

fn discovery_failure(code: ErrorCode) -> BlockFailure {
    let message = match code {
        ErrorCode::Cancelled => {
            "Cancelled while refreshing the device list. No device operation was started."
        }
        ErrorCode::Timeout => {
            "Refreshing the device list took too long. No device operation was started."
        }
        ErrorCode::MissingHelper => {
            "Device discovery is unavailable. Check that lsblk (util-linux) is installed."
        }
        ErrorCode::Busy => {
            "Device helpers are busy. Wait for them to finish before refreshing the device list."
        }
        ErrorCode::Protocol => {
            "The device list is incomplete or invalid. Refresh it before choosing a drive."
        }
        _ => "The device list could not be refreshed. Check device support before retrying.",
    };
    BlockFailure {
        error: RemoteError {
            code,
            message: message.into(),
        },
        // Discovery never sends a device-changing request to the service.
        effect: BlockEffect::NotStarted,
    }
}

fn list_devices_using(
    program: &OsStr,
    ctx: &CallContext<'_>,
) -> Result<Vec<devices::Device>, BlockFailure> {
    let started = Instant::now();
    let remaining = || -> Result<CallContext<'_>, BlockFailure> {
        let context = CallContext {
            cancel: ctx.cancel,
            timeout: ctx.timeout.saturating_sub(started.elapsed()),
        };
        context.check().map_err(|e| discovery_failure(e.code))?;
        Ok(context)
    };
    let mut command = Command::new(program);
    command
        .args([
            "--json",
            "-o",
            "NAME,PATH,PKNAME,SIZE,RM,HOTPLUG,TYPE,FSTYPE,LABEL,MOUNTPOINT",
        ])
        .env("LC_ALL", "C");
    let output = helper::run(command, &[], &remaining()?).map_err(|e| discovery_failure(e.code))?;
    remaining()?;
    if !output.status.success() {
        return Err(discovery_failure(ErrorCode::Failed));
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| discovery_failure(ErrorCode::Protocol))?;
    let raw =
        devices::parse_lsblk_checked(text).map_err(|_| discovery_failure(ErrorCode::Protocol))?;
    let devices = devices::offerable_devices(&raw);
    remaining()?;
    Ok(devices)
}

/// Refresh device selection on a worker within the caller's remaining budget.
pub fn list_devices(ctx: &CallContext<'_>) -> Result<Vec<devices::Device>, BlockFailure> {
    list_devices_using(OsStr::new("lsblk"), ctx)
}

#[derive(Clone, Copy)]
enum Action {
    Mount,
    Unlock,
    Unmount,
    Lock,
}

fn failure(code: ErrorCode, effect: BlockEffect) -> BlockFailure {
    let message = match (code, effect) {
        (ErrorCode::Cancelled, BlockEffect::NotStarted) => {
            "Cancelled before contacting the device service."
        }
        (ErrorCode::Cancelled, _) => {
            "Cancelled. The device service may still finish the request. Check the drive before retrying."
        }
        (ErrorCode::Timeout, BlockEffect::NotStarted) => {
            "The device request's deadline expired before it started."
        }
        (ErrorCode::Timeout, _) => {
            "The device service did not answer in time. It may still finish the request. Check the drive before retrying."
        }
        (ErrorCode::MissingHelper, _) => {
            "Device support is unavailable. Check that UDisks is installed."
        }
        (ErrorCode::Authentication, _) => {
            "That passphrase was not accepted. Check it before retrying."
        }
        (ErrorCode::PermissionDenied, _) => {
            "This account is not allowed to perform that device operation. Check device permissions."
        }
        (ErrorCode::Unsupported, _) => {
            "That device has no mountable filesystem. Check its format or unlock state."
        }
        (ErrorCode::InvalidInput, _) => {
            "Choose a valid device and provide its passphrase when required."
        }
        (ErrorCode::Protocol, BlockEffect::Accepted) => {
            "The device service accepted the request, but returned no usable location. Check the drive in the sidebar before retrying."
        }
        (ErrorCode::Protocol, _) => {
            "The device helper returned an incomplete or oversized response. Check the drive before retrying."
        }
        (ErrorCode::Busy, _) => "Device helpers are busy. Wait for them to finish before retrying.",
        _ => "The device operation failed. Check the drive and device support before retrying.",
    };
    BlockFailure {
        error: RemoteError {
            code,
            message: message.into(),
        },
        effect,
    }
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute() && !path.components().any(|c| c == Component::ParentDir)
}

fn valid_device(device: &str) -> bool {
    let path = Path::new(device);
    valid_path(path)
        && path.starts_with("/dev")
        && path != Path::new("/dev")
        && !device.chars().any(char::is_control)
}

fn invoke(
    action: Action,
    device: &str,
    secret: &[u8],
    ctx: &CallContext<'_>,
    program: &OsStr,
) -> Result<helper::Output, BlockFailure> {
    ctx.check()
        .map_err(|e| failure(e.code, BlockEffect::NotStarted))?;
    if !valid_device(device) || (matches!(action, Action::Unlock) && secret.is_empty()) {
        return Err(failure(ErrorCode::InvalidInput, BlockEffect::NotStarted));
    }
    let args = match action {
        Action::Mount => devices::mount_args(device),
        Action::Unlock => devices::unlock_args(device),
        Action::Unmount => devices::unmount_args(device),
        Action::Lock => devices::lock_args(device),
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .env("LC_ALL", "C.UTF-8")
        .env("LANGUAGE", "C")
        .env("CHARSET", "UTF-8")
        .env("G_FILENAME_ENCODING", "UTF-8")
        .env_remove("G_BROKEN_FILENAMES");
    let output = helper::run(command, secret, ctx).map_err(|e| {
        let effect = match e.code {
            ErrorCode::MissingHelper | ErrorCode::Busy | ErrorCode::InvalidInput => {
                BlockEffect::NotStarted
            }
            _ => BlockEffect::Unknown,
        };
        failure(e.code, effect)
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // CORE's known diagnoses are static. Never expose its raw fallback,
        // which could contain a helper echo of secret input.
        let reason = if matches!(action, Action::Unlock) {
            devices::unlock_failure(&stderr)
        } else {
            devices::mount_failure(&stderr)
        };
        let code = if reason.starts_with("This account is not allowed") {
            ErrorCode::PermissionDenied
        } else if matches!(action, Action::Unlock) && reason.starts_with("That passphrase") {
            ErrorCode::Authentication
        } else if matches!(action, Action::Mount) && reason.starts_with("There is no filesystem") {
            ErrorCode::Unsupported
        } else {
            ErrorCode::Failed
        };
        return Err(failure(code, BlockEffect::Unknown));
    }
    Ok(output)
}

fn mount_using(
    program: &OsStr,
    device: &str,
    ctx: &CallContext<'_>,
) -> Result<PathBuf, BlockFailure> {
    let output = invoke(Action::Mount, device, &[], ctx, program)?;
    let parse = || {
        let line = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        let rest = line.strip_prefix(b"Mounted ")?;
        let split = rest.windows(4).position(|part| part == b" at ")?;
        let reported = std::str::from_utf8(&rest[..split]).ok()?;
        if !valid_device(reported) {
            return None;
        }
        // Mount replies have no terminal period. Keep name bytes, spaces and
        // periods exactly, without stat/is_dir or a guessed destination.
        let bytes = &rest[split + 4..];
        // g_print is a UTF-8 text interface, not a raw filename transport.
        // Invalid names may be rendered differently by GLib and are unsupported.
        std::str::from_utf8(bytes).ok()?;
        let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
        (valid_path(&path) && !bytes.contains(&0)).then_some(path)
    };
    parse().ok_or_else(|| failure(ErrorCode::Protocol, BlockEffect::Accepted))
}

/// Mount a previously validated device on a worker. No candidate metadata I/O.
pub fn mount(device: &str, ctx: &CallContext<'_>) -> Result<PathBuf, BlockFailure> {
    mount_using(OsStr::new("udisksctl"), device, ctx)
}

fn unlock_using(
    program: &OsStr,
    device: &str,
    secret: &[u8],
    ctx: &CallContext<'_>,
) -> Result<String, BlockFailure> {
    let output = invoke(Action::Unlock, device, secret, ctx, program)?;
    let parse = || {
        let line = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        let line = std::str::from_utf8(line).ok()?;
        let cleartext = devices::unlocked_as(line)?;
        let (prefix, _) = line.rsplit_once(" as ")?;
        let reported = prefix.strip_prefix("Unlocked ")?;
        // Detect lossy legacy parser normalization rather than acting on a
        // different device (for example, trimming a name's own final dot).
        (valid_device(reported)
            && valid_device(&cleartext)
            && line == format!("Unlocked {reported} as {cleartext}."))
        .then_some(cleartext)
    };
    parse().ok_or_else(|| failure(ErrorCode::Protocol, BlockEffect::Accepted))
}

/// The caller retains and wipes the borrowed secret after this worker call.
pub fn unlock(device: &str, secret: &[u8], ctx: &CallContext<'_>) -> Result<String, BlockFailure> {
    unlock_using(OsStr::new("udisksctl"), device, secret, ctx)
}

pub fn unmount(device: &str, ctx: &CallContext<'_>) -> Result<(), BlockFailure> {
    invoke(Action::Unmount, device, &[], ctx, OsStr::new("udisksctl")).map(|_| ())
}

pub fn lock(device: &str, ctx: &CallContext<'_>) -> Result<(), BlockFailure> {
    invoke(Action::Lock, device, &[], ctx, OsStr::new("udisksctl")).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gust_core::lister::CancelToken;
    use std::fs::DirBuilder;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct Fixture {
        root: PathBuf,
        helper: PathBuf,
    }
    impl Fixture {
        fn new(body: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "gust-block-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&root).unwrap();
            let source = root.join("fixture-source");
            let helper = root.join("fixture-helper");
            std::fs::write(&source, format!("#!/usr/bin/python3\n{body}\n")).unwrap();
            // A concurrent test's fork can inherit this process's write
            // descriptor until that child execs, making our exec fail with
            // ETXTBSY. A separate process creates the executable instead.
            let installed = Command::new("install")
                .args(["-m", "700"])
                .arg(&source)
                .arg(&helper)
                .status()
                .unwrap();
            assert!(installed.success());
            Self { root, helper }
        }
        fn program(&self) -> &OsStr {
            self.helper.as_os_str()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn context(cancel: &CancelToken) -> CallContext<'_> {
        CallContext {
            cancel,
            timeout: Duration::from_secs(3),
        }
    }

    #[test]
    fn discovery_preserves_core_filtering_and_cleartext_join_without_path_io() {
        let json = r#"{"blockdevices":[
            {"name":"system","path":"/dev/system","rm":false,"hotplug":false,"type":"disk"},
            {"name":"fixture","path":"/dev/fixture","size":"4G","rm":true,"hotplug":true,"type":"part","fstype":"crypto_LUKS","label":"Synthetic","children":[
                {"name":"dm-fixture","path":"/dev/dm-fixture","pkname":"fixture","type":"crypt","fstype":"ext4","mountpoint":"/synthetic/not-created"}
            ]},
            {"name":"loop-fixture","path":"/dev/loop-fixture","rm":true,"type":"loop"}
        ]}"#;
        let fixture = Fixture::new(&format!(
            "import os,sys\nassert os.environ['LC_ALL']=='C'\nassert sys.argv[1:]==['--json','-o','NAME,PATH,PKNAME,SIZE,RM,HOTPLUG,TYPE,FSTYPE,LABEL,MOUNTPOINT']\nassert sys.stdin.buffer.read()==b''\nsys.stdout.write({json:?})"
        ));
        let cancel = CancelToken::new();
        let listed = list_devices_using(fixture.program(), &context(&cancel)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "/dev/fixture");
        assert!(listed[0].encrypted);
        assert_eq!(listed[0].cleartext.as_deref(), Some("/dev/dm-fixture"));
        assert_eq!(
            listed[0].mount_point,
            Some(PathBuf::from("/synthetic/not-created"))
        );

        let empty = Fixture::new("print('{\"blockdevices\":[]}')");
        assert!(list_devices_using(empty.program(), &context(&cancel))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn discovery_preserves_braces_in_mounted_device_state() {
        let json = r#"{"blockdevices":[{"name":"fixture","path":"/dev/fixture","pkname":null,"size":"4G","rm":true,"hotplug":true,"type":"part","fstype":"ext4","label":"BACKUP{A}","mountpoint":"/synthetic/BACKUP{A}"}]}"#;
        let fixture = Fixture::new(&format!("print({json:?})"));
        let cancel = CancelToken::new();
        let listed = list_devices_using(fixture.program(), &context(&cancel)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "/dev/fixture");
        assert_eq!(listed[0].label.as_deref(), Some("BACKUP{A}"));
        assert_eq!(
            listed[0].mount_point,
            Some(PathBuf::from("/synthetic/BACKUP{A}"))
        );
        assert!(listed[0].is_mounted());
    }

    #[test]
    fn discovery_decodes_json_escapes_without_changing_mount_locations() {
        let cancel = CancelToken::new();
        for path in [
            "/synthetic/a\nb",
            "/synthetic/a\tb",
            "/synthetic/café\\share",
            "/synthetic/a\"b",
        ] {
            let json = serde_json::json!({"blockdevices":[{
                "name":"fixture","path":"/dev/fixture","pkname":null,"size":"4G",
                "rm":true,"hotplug":true,"type":"part","fstype":"ext4",
                "label":"Synthetic","mountpoint":path
            }]})
            .to_string();
            let fixture = Fixture::new(&format!("print({json:?})"));
            let listed = list_devices_using(fixture.program(), &context(&cancel)).unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].mount_point, Some(PathBuf::from(path)));
        }
    }

    #[test]
    fn discovery_refuses_invalid_device_objects_and_children() {
        let cancel = CancelToken::new();
        for entry in [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!("row"),
            serde_json::json!({}),
        ] {
            let json = serde_json::json!({"blockdevices":[entry]}).to_string();
            let fixture = Fixture::new(&format!("print({json:?})"));
            let e = list_devices_using(fixture.program(), &context(&cancel)).unwrap_err();
            assert_eq!(e.error.code, ErrorCode::Protocol);
            assert_eq!(e.effect, BlockEffect::NotStarted);
        }
        for children in [serde_json::json!([null]), serde_json::json!("children")] {
            let json = serde_json::json!({"blockdevices":[{
                "name":"fixture","path":"/dev/fixture","pkname":null,"size":"4G",
                "rm":true,"hotplug":true,"type":"part","fstype":"ext4",
                "label":"Synthetic","mountpoint":null,"children":children
            }]})
            .to_string();
            let fixture = Fixture::new(&format!("print({json:?})"));
            let e = list_devices_using(fixture.program(), &context(&cancel)).unwrap_err();
            assert_eq!(e.error.code, ErrorCode::Protocol);
            assert_eq!(e.effect, BlockEffect::NotStarted);
        }
    }

    #[test]
    fn discovery_refuses_positional_envelopes_rows_and_recursive_children() {
        let object = serde_json::json!({
            "name":"fixture", "path":"/dev/fixture", "type":"part", "size":"4G",
            "rm":true, "hotplug":false, "fstype":"ext4", "label":"Synthetic",
            "mountpoint":"/synthetic/mounted"
        });
        // Complete positional structs, rather than empty arrays that also fail
        // because required fields are missing in the typed representation.
        let row = serde_json::json!([
            "fixture",
            "/dev/fixture",
            "part",
            "4G",
            true,
            false,
            "ext4",
            "Synthetic",
            "/synthetic/mounted",
            null,
            []
        ]);
        let mut with_child = object.clone();
        with_child["children"] = serde_json::json!([row.clone()]);
        let mut encrypted = object.clone();
        encrypted["fstype"] = serde_json::json!("crypto_LUKS");
        encrypted["children"] = serde_json::json!([[
            "dm-fixture",
            "/dev/dm-fixture",
            "crypt",
            "4G",
            false,
            false,
            "ext4",
            "Synthetic",
            "/synthetic/mounted",
            "fixture",
            []
        ]]);
        let cancel = CancelToken::new();
        for reply in [
            serde_json::json!([[]]),
            serde_json::json!([[object]]),
            serde_json::json!({"blockdevices":[row]}),
            serde_json::json!({"blockdevices":[with_child]}),
            serde_json::json!({"blockdevices":[encrypted]}),
        ] {
            let fixture = Fixture::new(&format!("print({:?})", reply.to_string()));
            let e = list_devices_using(fixture.program(), &context(&cancel)).unwrap_err();
            assert_eq!(e.error.code, ErrorCode::Protocol);
            assert_eq!(e.effect, BlockEffect::NotStarted);
            assert_eq!(e, discovery_failure(ErrorCode::Protocol));
        }
    }

    #[test]
    fn discovery_keeps_valid_object_cleartext_and_empty_controls() {
        let cancel = CancelToken::new();
        let reply = serde_json::json!({"blockdevices":[{
            "name":"fixture", "path":"/dev/fixture", "type":"part", "rm":true,
            "fstype":"crypto_LUKS", "label":"Synthetic{A}", "children":[{
                "name":"dm-fixture", "path":"/dev/dm-fixture", "type":"crypt",
                "pkname":"fixture", "fstype":"ext4", "mountpoint":"/synthetic/café\n{A}"
            }]
        }]});
        let fixture = Fixture::new(&format!("print({:?})", reply.to_string()));
        let listed = list_devices_using(fixture.program(), &context(&cancel)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "/dev/fixture");
        assert_eq!(listed[0].cleartext.as_deref(), Some("/dev/dm-fixture"));
        assert_eq!(listed[0].label.as_deref(), Some("Synthetic{A}"));
        assert_eq!(
            listed[0].mount_point,
            Some(PathBuf::from("/synthetic/café\n{A}"))
        );
        assert!(listed[0].is_mounted());
        let fixture = Fixture::new("print('{\"blockdevices\":[]}')");
        assert!(list_devices_using(fixture.program(), &context(&cancel))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn discovery_refuses_parser_limits_instead_of_returning_an_empty_inventory() {
        // Valid JSON below the helper's output bound, above CORE's parse bound.
        let fixture = Fixture::new(
            "import sys\nsys.stdout.buffer.write(b' '*(4*1024*1024)+b'{\"blockdevices\":[]}')",
        );
        let cancel = CancelToken::new();
        let e = list_devices_using(fixture.program(), &context(&cancel)).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Protocol);
        assert_eq!(e.effect, BlockEffect::NotStarted);
    }

    #[test]
    fn discovery_failures_never_become_empty_success_or_expose_diagnostics() {
        let cancel = CancelToken::new();
        for reply in [
            "",
            "{",
            "{}",
            "[]",
            "{\"blockdevices\":null}",
            "{\"blockdevices\":[]} trailing",
        ] {
            let fixture = Fixture::new(&format!("print({reply:?})"));
            let e = list_devices_using(fixture.program(), &context(&cancel)).unwrap_err();
            assert_eq!(e.error.code, ErrorCode::Protocol);
            assert_eq!(e.effect, BlockEffect::NotStarted);
        }
        let invalid_utf8 = Fixture::new("import sys\nsys.stdout.buffer.write(b'\\xff')");
        assert_eq!(
            list_devices_using(invalid_utf8.program(), &context(&cancel))
                .unwrap_err()
                .error
                .code,
            ErrorCode::Protocol
        );
        let failed = Fixture::new(
            "import sys\nsys.stderr.write('synthetic-private-diagnostic')\nsys.exit(1)",
        );
        let e = list_devices_using(failed.program(), &context(&cancel)).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Failed);
        assert!(!format!("{e:?} {e}").contains("synthetic-private-diagnostic"));
        let e = list_devices_using(failed.root.join("missing").as_os_str(), &context(&cancel))
            .unwrap_err();
        assert_eq!(e.error.code, ErrorCode::MissingHelper);
        assert!(e.error.message.contains("lsblk"));
    }

    #[test]
    fn discovery_honors_pre_cancel_deadline_and_output_bounds() {
        let fixture =
            Fixture::new("import pathlib\npathlib.Path(__file__).with_name('launched').touch()");
        let cancel = CancelToken::new();
        let zero = CallContext {
            cancel: &cancel,
            timeout: Duration::ZERO,
        };
        assert_eq!(
            list_devices_using(fixture.program(), &zero)
                .unwrap_err()
                .error
                .code,
            ErrorCode::Timeout
        );
        cancel.cancel();
        assert_eq!(
            list_devices_using(fixture.program(), &context(&cancel))
                .unwrap_err()
                .error
                .code,
            ErrorCode::Cancelled
        );
        assert!(!fixture.root.join("launched").exists());

        let cancel = CancelToken::new();
        let flood = Fixture::new("import sys\nsys.stdout.buffer.write(b'x'*(9*1024*1024))");
        let e = list_devices_using(flood.program(), &context(&cancel)).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Protocol);
        assert_eq!(e.effect, BlockEffect::NotStarted);
        let quiet = Fixture::new("import time\ntime.sleep(30)");
        let short = CallContext {
            cancel: &cancel,
            timeout: Duration::from_millis(150),
        };
        let start = Instant::now();
        let e = list_devices_using(quiet.program(), &short).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Timeout);
        assert_eq!(e.effect, BlockEffect::NotStarted);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn discovery_observes_cancellation_while_the_helper_is_active() {
        let fixture = Fixture::new("import pathlib,time\npathlib.Path(__file__).with_name('started').touch()\ntime.sleep(30)");
        let token = Arc::new(CancelToken::new());
        let cancel = Arc::clone(&token);
        let marker = fixture.root.join("started");
        let worker = std::thread::spawn(move || {
            let start = Instant::now();
            while !marker.exists() && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(marker.exists());
            cancel.cancel();
        });
        let e = list_devices_using(fixture.program(), &context(&token)).unwrap_err();
        worker.join().unwrap();
        assert_eq!(e.error.code, ErrorCode::Cancelled);
        assert_eq!(e.effect, BlockEffect::NotStarted);
    }

    #[test]
    fn mount_preserves_glib_utf8_output_without_touching_the_reported_path() {
        let fixture = Fixture::new(
            "import ctypes,locale,sys\nlocale.setlocale(locale.LC_ALL,'')\nassert sys.argv[1:]==['mount','-b','/dev/mapper/fixture','--no-user-interaction']\nglib=ctypes.CDLL('libglib-2.0.so.0')\nglib.g_print.argtypes=[ctypes.c_char_p]\nglib.g_print(b'Mounted %s at %s\\n',ctypes.c_char_p(b'/dev/dm-fixture'),ctypes.c_char_p(b'/synthetic/not-created/caf\\xc3\\xa9 at place. '))",
        );
        let cancel = CancelToken::new();
        let path =
            mount_using(fixture.program(), "/dev/mapper/fixture", &context(&cancel)).unwrap();
        assert_eq!(
            path.as_os_str().as_bytes(),
            "/synthetic/not-created/café at place. ".as_bytes()
        );
    }

    #[test]
    fn mount_preserves_legacy_glib_charset_conversion() {
        // GLib 2.80's print handler uses this conversion stage. Newer GLib
        // prints UTF-8 directly, so exercise the legacy stage explicitly too.
        let fixture = Fixture::new(
            r#"
import ctypes,locale,os
locale.setlocale(locale.LC_ALL,'')
glib=ctypes.CDLL('libglib-2.0.so.0')
glib.g_get_charset.argtypes=[ctypes.POINTER(ctypes.c_char_p)]
glib.g_convert_with_fallback.argtypes=[ctypes.c_char_p,ctypes.c_long,ctypes.c_char_p,ctypes.c_char_p,ctypes.c_char_p,ctypes.c_void_p,ctypes.c_void_p,ctypes.c_void_p]
glib.g_convert_with_fallback.restype=ctypes.c_void_p
glib.g_free.argtypes=[ctypes.c_void_p]
handler_type=ctypes.CFUNCTYPE(None,ctypes.c_char_p)
@handler_type
def legacy_print(message):
    charset=ctypes.c_char_p()
    if glib.g_get_charset(ctypes.byref(charset)):
        data=message
    else:
        converted=glib.g_convert_with_fallback(message,-1,charset.value,b'UTF-8',b'?',None,None,None)
        assert converted
        data=ctypes.string_at(converted)
        glib.g_free(converted)
    os.write(1,data)
glib.g_set_print_handler.argtypes=[handler_type]
glib.g_set_print_handler(legacy_print)
glib.g_print.argtypes=[ctypes.c_char_p]
glib.g_print(b'Mounted /dev/fixture at /synthetic/caf\xc3\xa9 at place.\n')
"#,
        );
        let cancel = CancelToken::new();
        assert_eq!(
            mount_using(fixture.program(), "/dev/fixture", &context(&cancel)).unwrap(),
            PathBuf::from("/synthetic/café at place.")
        );
    }

    #[test]
    fn mount_refuses_non_utf8_glib_or_raw_output_without_a_guessed_path() {
        let cancel = CancelToken::new();
        for body in [
            "import ctypes,locale\nlocale.setlocale(locale.LC_ALL,'')\nglib=ctypes.CDLL('libglib-2.0.so.0')\nglib.g_print.argtypes=[ctypes.c_char_p]\nglib.g_print(b'Mounted /dev/fixture at %s\\n',ctypes.c_char_p(b'/synthetic/name\\xff'))",
            "import sys\nsys.stdout.buffer.write(b'Mounted /dev/fixture at /synthetic/name\\xff\\n')",
        ] {
            let fixture = Fixture::new(body);
            let e = mount_using(fixture.program(), "/dev/fixture", &context(&cancel)).unwrap_err();
            assert_eq!(e.error.code, ErrorCode::Protocol);
            assert_eq!(e.effect, BlockEffect::Accepted);
        }
    }

    #[test]
    fn unlock_uses_core_arguments_and_secret_stdin_without_retaining_output() {
        let fixture = Fixture::new(
            "import os,sys\nsecret=sys.stdin.buffer.read()\nassert secret==b'synthetic passphrase\\x00\\n'\nassert 'synthetic passphrase' not in repr(sys.argv)\nassert all('synthetic passphrase' not in v for v in os.environ.values())\nassert sys.argv[1:]==['unlock','-b','/dev/fixture','--no-user-interaction','--key-file','/dev/stdin']\nprint('Unlocked /dev/fixture as /dev/dm-fixture.')",
        );
        let cancel = CancelToken::new();
        assert_eq!(
            unlock_using(
                fixture.program(),
                "/dev/fixture",
                b"synthetic passphrase\0\n",
                &context(&cancel)
            )
            .unwrap(),
            "/dev/dm-fixture"
        );
    }

    #[test]
    fn preflight_refusals_and_pre_cancel_never_launch_a_helper() {
        let fixture =
            Fixture::new("import pathlib\npathlib.Path(__file__).with_name('launched').touch()");
        let cancel = CancelToken::new();
        for device in [
            "/dev",
            "/device/fixture",
            "/dev/../fixture",
            "/dev/fi\nxture",
        ] {
            let e = mount_using(fixture.program(), device, &context(&cancel)).unwrap_err();
            assert_eq!(e.effect, BlockEffect::NotStarted);
            assert_eq!(e.error.code, ErrorCode::InvalidInput);
        }
        assert_eq!(
            unlock_using(fixture.program(), "/dev/fixture", &[], &context(&cancel))
                .unwrap_err()
                .effect,
            BlockEffect::NotStarted
        );
        let zero = CallContext {
            cancel: &cancel,
            timeout: Duration::ZERO,
        };
        assert_eq!(
            mount_using(fixture.program(), "/dev/fixture", &zero)
                .unwrap_err()
                .error
                .code,
            ErrorCode::Timeout
        );
        cancel.cancel();
        let e = mount_using(fixture.program(), "/dev/fixture", &context(&cancel)).unwrap_err();
        assert_eq!(e.effect, BlockEffect::NotStarted);
        assert_eq!(e.error.code, ErrorCode::Cancelled);
        assert!(!fixture.root.join("launched").exists());
    }

    #[test]
    fn incomplete_success_is_accepted_without_a_guessed_location() {
        let cancel = CancelToken::new();
        for reply in ["Mounted /dev/fixture", "Mounted /dev/fixture at relative"] {
            let fixture = Fixture::new(&format!("print({reply:?})"));
            let e = mount_using(fixture.program(), "/dev/fixture", &context(&cancel)).unwrap_err();
            assert_eq!(e.error.code, ErrorCode::Protocol);
            assert_eq!(e.effect, BlockEffect::Accepted);
        }
        // The legacy unlock parser trims final dots. Verify its reconstruction
        // rather than returning the wrong cleartext device.
        let fixture = Fixture::new("print('Unlocked /dev/fixture as /dev/mapper/name..')");
        let e = unlock_using(
            fixture.program(),
            "/dev/fixture",
            b"synthetic",
            &context(&cancel),
        )
        .unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Protocol);
        assert_eq!(e.effect, BlockEffect::Accepted);
    }

    #[test]
    fn failed_secret_diagnostics_are_never_exposed() {
        let cancel = CancelToken::new();
        for (reason, code) in [
            ("Incorrect passphrase", ErrorCode::Authentication),
            ("NotAuthorized", ErrorCode::PermissionDenied),
            ("unknown failure", ErrorCode::Failed),
        ] {
            let fixture = Fixture::new(&format!(
                "import sys\nsecret=sys.stdin.buffer.read()\nsys.stderr.buffer.write({reason:?}.encode()+b': '+secret)\nsys.exit(1)"
            ));
            let e = unlock_using(
                fixture.program(),
                "/dev/fixture",
                b"synthetic-private-phrase",
                &context(&cancel),
            )
            .unwrap_err();
            assert_eq!(e.error.code, code);
            assert_eq!(e.effect, BlockEffect::Unknown);
            assert!(!format!("{e:?} {e}").contains("synthetic-private-phrase"));
            assert!(!e.to_string().contains("Nothing was changed"));
        }
    }

    #[test]
    fn output_floods_and_quiet_helpers_remain_bounded_and_uncertain() {
        let cancel = CancelToken::new();
        let fixture = Fixture::new("import sys\nsys.stdout.buffer.write(b'x'*(9*1024*1024))");
        let e = mount_using(fixture.program(), "/dev/fixture", &context(&cancel)).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Protocol);
        assert_eq!(e.effect, BlockEffect::Unknown);
        let fixture = Fixture::new("import time\ntime.sleep(30)");
        let ctx = CallContext {
            cancel: &cancel,
            timeout: Duration::from_millis(150),
        };
        let started = Instant::now();
        let e = mount_using(fixture.program(), "/dev/fixture", &ctx).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Timeout);
        assert_eq!(e.effect, BlockEffect::Unknown);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancelling_an_active_request_does_not_claim_service_rollback() {
        let fixture = Fixture::new(
            "import pathlib,time\npathlib.Path(__file__).with_name('accepted').touch()\ntime.sleep(30)",
        );
        let token = Arc::new(CancelToken::new());
        let cancel = Arc::clone(&token);
        let marker = fixture.root.join("accepted");
        let worker = std::thread::spawn(move || {
            let started = Instant::now();
            while !marker.exists() && started.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(marker.exists());
            cancel.cancel();
        });
        let e = mount_using(fixture.program(), "/dev/fixture", &context(&token)).unwrap_err();
        worker.join().unwrap();
        assert_eq!(e.error.code, ErrorCode::Cancelled);
        assert_eq!(e.effect, BlockEffect::Unknown);
        assert!(fixture.root.join("accepted").exists());
    }

    #[test]
    fn eject_actions_share_the_same_runtime_and_missing_support_is_not_started() {
        let fixture = Fixture::new(
            "import sys\nassert sys.argv[1] in ('unmount','lock')\nassert sys.argv[2:]==['-b','/dev/fixture','--no-user-interaction']\nassert sys.stdin.buffer.read()==b''",
        );
        let cancel = CancelToken::new();
        for action in [Action::Unmount, Action::Lock] {
            invoke(
                action,
                "/dev/fixture",
                &[],
                &context(&cancel),
                fixture.program(),
            )
            .unwrap();
        }
        let missing = fixture.root.join("not-installed");
        let e = mount_using(missing.as_os_str(), "/dev/fixture", &context(&cancel)).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::MissingHelper);
        assert_eq!(e.effect, BlockEffect::NotStarted);
    }
}
