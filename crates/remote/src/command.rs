//! Bounded local command output driven by stdout readiness.

use gust_core::lister::CancelToken;
use std::io::{self, Error, ErrorKind};
use std::process::{Command, ExitStatus};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

pub(crate) struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
}

/// Wait for pipe readiness under the shared helper deadline. Unlike joining
/// a reader, this stays bounded when a descendant inherits stdout. The child
/// status is still required: closing stdout alone never proves completion.
pub(crate) fn capture_stdout(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
    cancel: Option<&CancelToken>,
) -> io::Result<Captured> {
    let default_cancel = CancelToken::new();
    let ctx = crate::helper::CallContext {
        cancel: cancel.unwrap_or(&default_cancel),
        timeout,
    };
    let output = crate::helper::run_borrowed(command, &[], &ctx).map_err(|error| {
        let kind = match error.code {
            crate::helper::ErrorCode::Cancelled => ErrorKind::Interrupted,
            crate::helper::ErrorCode::Timeout => ErrorKind::TimedOut,
            crate::helper::ErrorCode::MissingHelper => ErrorKind::NotFound,
            crate::helper::ErrorCode::Protocol => ErrorKind::InvalidData,
            _ => ErrorKind::Other,
        };
        Error::new(
            kind,
            if kind == ErrorKind::InvalidData {
                "Local command output exceeded limit".into()
            } else {
                error.message
            },
        )
    })?;
    if output.stdout.len() > limit {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Local command output exceeded limit",
        ));
    }
    Ok(Captured {
        status: output.status,
        stdout: output.stdout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn drains_large_output_and_checks_exit_status() {
        let output = capture_stdout(
            &mut shell("head -c 262144 /dev/zero; exit 7"),
            Duration::from_secs(2),
            262144,
            None,
        )
        .unwrap();
        assert_eq!(output.stdout, vec![0; 262144]);
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn output_limit_is_enforced_without_waiting_for_eof() {
        let start = Instant::now();
        let error = capture_stdout(
            &mut shell("exec head -c 262144 /dev/zero"),
            Duration::from_secs(2),
            64,
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn closed_stdout_does_not_claim_a_running_command_finished() {
        let start = Instant::now();
        let error = capture_stdout(
            &mut shell("exec 1>&-; exec sleep 2"),
            Duration::from_millis(40),
            64,
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(start.elapsed() >= Duration::from_millis(40));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn descendant_holding_stdout_cannot_extend_the_deadline() {
        let start = Instant::now();
        let error = capture_stdout(
            &mut shell("sleep 0.2 & exit 0"),
            Duration::from_millis(40),
            64,
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_millis(180));
    }

    #[test]
    fn zero_deadline_and_pre_cancel_do_not_spawn() {
        for (timeout, cancel) in [
            (Duration::ZERO, None),
            (Duration::from_secs(1), Some(CancelToken::new())),
        ] {
            if let Some(cancel) = &cancel {
                cancel.cancel();
            }
            // A missing binary would produce NotFound if spawning happened.
            let error = capture_stdout(
                &mut Command::new("/no/such/gust-command"),
                timeout,
                64,
                cancel.as_ref(),
            )
            .err()
            .unwrap();
            assert_eq!(
                error.kind(),
                if cancel.is_some() {
                    ErrorKind::Interrupted
                } else {
                    ErrorKind::TimedOut
                }
            );
        }
    }

    #[test]
    fn cancellation_interrupts_a_quiet_command() {
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            capture_stdout(
                &mut shell("exec sleep 2"),
                Duration::from_secs(1),
                64,
                Some(&worker_cancel),
            )
            .err()
            .unwrap()
            .kind()
        });
        std::thread::sleep(Duration::from_millis(40));
        let start = Instant::now();
        cancel.cancel();
        assert_eq!(worker.join().unwrap(), ErrorKind::Interrupted);
        assert!(start.elapsed() < Duration::from_millis(300));
    }
}
