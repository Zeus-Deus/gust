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
    handlers = {sig: signal.signal(sig, signal.SIG_IGN)
                for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass
        # The immediate parent can exit while a descendant ignores TERM.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        deadline = time.monotonic() + 10
        while live_group(child.pid):
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired("owned group termination", 10)
            time.sleep(0.01)
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def run(command, *, cwd, env, log, timeout=None):
    pending = False

    def defer_startup_cancel(signum, frame):
        nonlocal pending
        pending = True

    # Defer Python cancellation during Popen startup until we own its PID.
    # Unlike blocking signals, this leaves the child's signal mask untouched.
    handlers = {sig: signal.signal(sig, defer_startup_cancel)
                for sig in (signal.SIGINT, signal.SIGTERM)}
    child = None
    try:
        child = subprocess.Popen(command, cwd=cwd, env=env, stdout=log,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        for sig in handlers:
            signal.signal(sig, cancel_on_signal)
        if pending:
            raise KeyboardInterrupt
        return child.wait(timeout=timeout)
    except BaseException:
        if child is not None:
            terminate_owned_group(child)
        raise
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)
