#!/usr/bin/env python3
"""Interrupt the desktop copy with a kernel file-size limit on this Gust child."""

import ctypes
import os
from pathlib import Path
import resource
import signal
import stat
import struct
import subprocess
import sys
import time

FILE_COUNT = 2000
# Stop early in the name-sorted clipboard, keeping the batch journal small.
IN_FLIGHT = "k-00009.dat"
LIMIT = 8 * 1024 * 1024
PREFIX = bytes(range(256)) * (LIMIT // 256)
PAYLOAD = PREFIX + b"end of the oversized source\n"


def originals():
    for n in range(1, FILE_COUNT + 1):
        name = f"k-{n:05d}.dat"
        yield name, PAYLOAD if name == IN_FLIGHT else (name + "\n").encode()


def prepare(root):
    src = root / "src"
    src.mkdir(parents=True)
    (root / "dest").mkdir()
    for name, payload in originals():
        (src / name).write_bytes(payload)


def matches(path, data):
    info = path.lstat()
    return stat.S_ISREG(info.st_mode) and info.st_size == len(data) and path.read_bytes() == data


# Retain create/rename events through process death. A premature final that the
# copier removed again would be invisible to a postmortem directory listing.
IN_MOVED_TO, IN_CREATE = 0x80, 0x100
IN_Q_OVERFLOW, IN_IGNORED = 0x4000, 0x8000
EVENT = struct.Struct("iIII")


def watch_publication(destination):
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1")
    if libc.inotify_add_watch(fd, os.fsencode(destination), IN_CREATE | IN_MOVED_TO) < 0:
        error = ctypes.get_errno()
        os.close(fd)
        raise OSError(error, "inotify_add_watch", destination)
    return fd


def check_publication_events(fd):
    while True:
        try:
            events = os.read(fd, 64 * 1024)
        except BlockingIOError:
            return
        pos = 0
        while pos < len(events):
            _, mask, _, length = EVENT.unpack_from(events, pos)
            pos += EVENT.size
            name = events[pos:pos + length].rstrip(b"\0")
            if mask & (IN_Q_OVERFLOW | IN_IGNORED):
                raise ValueError("lost destination publication events")
            if name == os.fsencode(IN_FLIGHT):
                raise ValueError(f"{IN_FLIGHT} appeared under its final name")
            pos += length


def observe(root, command, timeout=90):
    destination = root / "dest"
    fd = watch_publication(destination)
    try:
        # Popen restores the default SIGXFSZ disposition at exec. Arm only
        # after navigation: the renderer can create large files at startup.
        signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGXFSZ})
        with subprocess.Popen(command) as child:
            try:
                deadline = time.monotonic() + timeout
                while not (root / "ready-to-paste").exists():
                    if child.poll() is not None:
                        raise ValueError(f"Gust exited before paste: {child.returncode}")
                    if time.monotonic() >= deadline:
                        raise TimeoutError("desktop did not reach the paste")
                    time.sleep(0.05)
                # The PID belongs to our unreaped child, even if it just exited.
                resource.prlimit(child.pid, resource.RLIMIT_CORE, (0, 0))
                resource.prlimit(child.pid, resource.RLIMIT_FSIZE, (LIMIT, LIMIT))
                (root / "armed").touch()
                status = child.wait(timeout=timeout)
            finally:
                if child.poll() is None:
                    child.kill()
            if status != -signal.SIGXFSZ:
                raise ValueError(f"expected Gust {child.pid} to die from SIGXFSZ, got {status}")
        check_publication_events(fd)
    finally:
        os.close(fd)

    stages = list(destination.glob(".gust-staging-*"))
    if len(stages) != 1 or not stages[0].name.startswith(f".gust-staging-{child.pid}-"):
        raise ValueError(f"expected one staging file owned by Gust {child.pid}")
    stage = stages[0]
    if not matches(stage, PREFIX):
        raise ValueError(f"{stage.name} did not contain the expected {LIMIT}-byte prefix")
    if os.path.lexists(destination / IN_FLIGHT):
        raise ValueError(f"{IN_FLIGHT} survived under its final name")
    source = root / "src"
    if sum(1 for _ in source.iterdir()) != FILE_COUNT:
        raise ValueError("source file count changed")
    for name, payload in originals():
        if not matches(source / name, payload):
            raise ValueError(f"original payload changed: {name}")
    print(f"Gust {child.pid} died from SIGXFSZ with {LIMIT}/{len(PAYLOAD)} bytes of "
          f"{IN_FLIGHT} staged and no publication of its final name", flush=True)


if __name__ == "__main__":
    mode, directory, *argv = sys.argv[1:]
    if mode == "prepare":
        prepare(Path(directory))
        print(FILE_COUNT)
    elif mode == "observe" and argv:
        observe(Path(directory), argv)
    else:
        sys.exit("expected prepare DIRECTORY or observe DIRECTORY COMMAND...")
