#!/usr/bin/env python3
"""Keep an owned compiler process group inside its caller's lease lifetime."""

from pathlib import Path
import os
import signal
import subprocess
import time


def cancel_on_signal(signum, frame):
    raise KeyboardInterrupt


def live_group(group):
    # Grandchildren are not waitable children. Linux proc state distinguishes
    # stopped execution from zombies awaiting their actual parent's reaping.
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(") ", 1)[1].split()
            if int(fields[2]) == group and fields[0] not in {"Z", "X"}:
                return True
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return False


def terminate_owned_group(child):
    # run() keeps its non-raising cancellation handler installed throughout
    # entry, TERM, escalation and retirement. There is no handler transition
    # here for a second signal to interrupt.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except OSError:
        pass
    try:
        child.wait(timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except OSError:
        pass
    # A timeout or unavailable retirement observation is not permission to
    # release the caller's lease. Keep ownership and retry observation until
    # established; do not repeatedly signal a potentially recycled group ID.
    while True:
        try:
            child.wait(timeout=0.2)
            if not live_group(child.pid):
                return
        except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
            pass
        time.sleep(0.05)


def run(command, *, cwd, env, log, timeout=None):
    pending = False

    def defer_cancel(signum, frame):
        nonlocal pending
        pending = True

    # Defer cancellation for the entire ownership lifetime, including Popen
    # startup and cleanup entry. Polling observes cancellation without an
    # exception-raising signal handler that could interrupt retirement.
    handlers = {sig: signal.signal(sig, defer_cancel)
                for sig in (signal.SIGINT, signal.SIGTERM)}
    child = None
    try:
        child = subprocess.Popen(command, cwd=cwd, env=env, stdout=log,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            if pending:
                raise KeyboardInterrupt
            try:
                code = child.wait(timeout=0.2)
            except subprocess.TimeoutExpired:
                if deadline is not None and time.monotonic() >= deadline:
                    raise subprocess.TimeoutExpired(command, timeout)
            else:
                if pending:
                    raise KeyboardInterrupt
                return code
    except BaseException:
        if child is not None:
            terminate_owned_group(child)
        raise
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)
