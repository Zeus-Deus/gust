//! Test-only helpers shared by every crate's fixture tests.
//!
//! The rule this crate exists for: a test that did not run must never be
//! reported as `ok`. libtest has no runtime skip, so a fixture test that
//! returns early is counted as a pass. `skip!` records the skip in a ledger
//! file that `tests/run-tests.sh` reads back, subtracts from the pass count
//! and prints as `SKIP`, and `GUST_<NAME>_REQUIRED=1` turns the skip into a
//! failure for a release gate (`just test-required`).

use std::io::Write;
use std::path::PathBuf;

/// True when `GUST_<name>_REQUIRED` is `1` or `true`.
pub fn required(name: &str) -> bool {
    std::env::var(format!("GUST_{name}_REQUIRED"))
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// True when every `GUST_*_REQUIRED` gate should be treated as set.
pub fn all_required() -> bool {
    required("ALL")
}

/// Where skip records go: `$GUST_SKIP_LEDGER`, else `target/test-skips.log`
/// under the workspace root.
pub fn ledger_path() -> PathBuf {
    if let Some(p) = std::env::var_os("GUST_SKIP_LEDGER") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("target/test-skips.log"))
        .unwrap_or_else(|| PathBuf::from("test-skips.log"))
}

/// Record a skip. Returns normally so the caller can `return`.
///
/// Panics instead when the gate is required: a release gate must never pass
/// on a fixture that was not there.
pub fn record_skip(gate: &str, test: &str, reason: &str) {
    if required(gate) || all_required() {
        panic!(
            "GUST_{gate}_REQUIRED is set, so this case may not skip: {reason} \
             (test {test})"
        );
    }
    eprintln!("SKIP [{gate}] {test}: {reason}");
    let line = format!(
        "SKIP\t{gate}\t{test}\t{}\n",
        reason.replace(['\t', '\n'], " ")
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// The current test's name, from the libtest thread name.
pub fn current_test() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unknown")
        .to_string()
}

/// `skip!("SSHFS", "fixture not running")` — record and return.
#[macro_export]
macro_rules! skip {
    ($gate:expr, $($reason:tt)+) => {{
        $crate::record_skip($gate, &$crate::current_test(), &format!($($reason)+));
        return;
    }};
}

/// `skip_unless!(cond, "SSHFS", "fixture not running")`.
#[macro_export]
macro_rules! skip_unless {
    ($cond:expr, $gate:expr, $($reason:tt)+) => {
        if !$cond {
            $crate::skip!($gate, $($reason)+);
        }
    };
}

/// Is a program on PATH?
pub fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|d| {
                let p = d.join(program);
                std::fs::metadata(&p)
                    .map(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.is_file() && m.permissions().mode() & 0o111 != 0
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skip_is_written_to_the_ledger() {
        let dir = std::env::temp_dir().join(format!("gust-testkit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = dir.join("skips.log");
        // Only this test touches these variables in this process.
        std::env::set_var("GUST_SKIP_LEDGER", &ledger);
        std::env::remove_var("GUST_UNITTEST_REQUIRED");
        std::env::remove_var("GUST_ALL_REQUIRED");
        record_skip("UNITTEST", "a_test", "no fixture");
        let body = std::fs::read_to_string(&ledger).unwrap();
        assert!(
            body.starts_with("SKIP\tUNITTEST\ta_test\tno fixture"),
            "{body}"
        );
        std::env::set_var("GUST_UNITTEST_REQUIRED", "1");
        let r = std::panic::catch_unwind(|| record_skip("UNITTEST", "b", "gone"));
        assert!(r.is_err(), "a required gate must fail, not skip");
        std::env::remove_var("GUST_UNITTEST_REQUIRED");
        std::env::remove_var("GUST_SKIP_LEDGER");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn have_finds_sh_and_not_nonsense() {
        assert!(have("sh"));
        assert!(!have("gust-no-such-program-xyz"));
    }
}
