#!/usr/bin/env python3
"""Drive real copy/move cancellation against held reads on a private FUSE mount.

Uses only synthetic files, a private Wayland compositor/bus and owned
processes. Every cancelled reader remains held until the fixture releases it.
"""

import argparse
import fcntl
import hashlib
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import time
from pathlib import Path

FUSE_SOURCE = r"""
#define FUSE_USE_VERSION 31
#include <fuse3/fuse.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static const char *control, *entered;
static int held(void) {
    FILE *f = fopen(control,"r"); if (!f) return 0;
    int c = fgetc(f); fclose(f); return c == 'h';
}
static int attrs(const char *p, struct stat *s, struct fuse_file_info *fi) {
    (void)fi; memset(s,0,sizeof(*s)); s->st_uid=getuid(); s->st_gid=getgid();
    s->st_mtime=s->st_ctime=1700000000;
    if (!strcmp(p,"/")) { s->st_mode=S_IFDIR|0700; s->st_nlink=2; s->st_ino=1; }
    else if (!strcmp(p,"/held.bin")) { s->st_mode=S_IFREG|0600; s->st_nlink=1; s->st_ino=2; s->st_size=2*1024*1024; }
    else return -ENOENT;
    return 0;
}
static int listing(const char *p, void *b, fuse_fill_dir_t fill, off_t off, struct fuse_file_info *fi, enum fuse_readdir_flags flags) {
    (void)off; (void)fi; (void)flags; if(strcmp(p,"/")) return -ENOENT;
    const char *ns[]={".","..","held.bin"};
    for(int i=0;i<3;i++) { struct stat s; attrs(i==2?"/held.bin":"/",&s,NULL); if(fill(b,ns[i],&s,0,0))break; }
    return 0;
}
static int open_file(const char *p, struct fuse_file_info *fi) {
    if(strcmp(p,"/held.bin")) return -ENOENT;
    fi->direct_io=1; return 0;
}
static int contents(const char *p, char *b, size_t size, off_t off, struct fuse_file_info *fi) {
    (void)p; (void)fi;
    if(held()) { FILE *f=fopen(entered,"a"); if(f){fputs("read\n",f);fclose(f);} while(held())usleep(10000); }
    size_t length=2*1024*1024; if(off<0 || (size_t)off>=length)return 0;
    if(size>length-(size_t)off)size=length-(size_t)off;
    memset(b,0x5a,size); return (int)size;
}
int main(int argc,char **argv) {
    if(argc!=4)return 2;
    control=argv[2];entered=argv[3];
    struct fuse_operations ops={.getattr=attrs,.readdir=listing,.open=open_file,.read=contents};
    char *args[]={argv[0],argv[1],"-f","-o","subtype=sshfs,fsname=gust-read-fixture,attr_timeout=0,entry_timeout=0",NULL};
    return fuse_main(5,args,&ops,NULL);
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    out = args.output.resolve()
    repo = Path(__file__).resolve().parent.parent
    if out.exists() or out.is_relative_to(repo):
        raise ValueError("Use a fresh output outside the checkout")
    required = [
        "cc",
        "pkg-config",
        "fusermount3",
        "labwc",
        "wlr-randr",
        "grim",
        "wtype",
        "dbus-daemon",
    ]
    if (
        not binary.is_file()
        or any(not shutil.which(x) for x in required)
        or not os.access("/dev/fuse", os.R_OK | os.W_OK)
    ):
        print("SKIP: native compositor/FUSE fixture prerequisites missing")
        return 77
    flags = subprocess.run(
        ["pkg-config", "--cflags", "--libs", "fuse3"],
        check=False,
        capture_output=True,
        text=True,
    )
    if flags.returncode:
        print("SKIP: FUSE development headers unavailable")
        return 77
    with open("/tmp/gust-qa.lock", "a") as lease:
        try:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print("SKIP: desktop QA lease busy")
            return 75
        return exercise(binary, out, flags.stdout)


def exercise(binary, out, flags):
    out.mkdir(mode=0o700)
    home = out / "home"
    runtime = out / "run"
    config = out / "compositor"
    mount = out / "source"
    stubs = out / "stubs"
    for p in [
        home / "Documents",
        home / ".config/gust",
        home / ".local/share",
        home / ".local/state",
        home / ".cache",
        runtime,
        config / "labwc",
        mount,
        stubs,
    ]:
        p.mkdir(parents=True, mode=0o700, exist_ok=True)
    (home / "Documents/local.txt").write_text("local fixture\n")
    for i in range(200):
        (home / f"Documents/file-{i:04}.txt").write_text(
            "synthetic navigation fixture\n"
        )
    (config / "labwc/rc.xml").write_text(
        '<labwc_config><windowRules><windowRule identifier="*"><action name="Maximize"/></windowRule></windowRules></labwc_config>'
    )
    (config / "labwc/autostart").write_text(
        'printf "%s/%s" "$XDG_RUNTIME_DIR" "$WAYLAND_DISPLAY" > '
        + shlex.quote(str(out / "display"))
        + "\n"
    )
    (stubs / "tailscale").write_text('#!/bin/sh\necho \'{"BackendState":"Stopped"}\'\n')
    (stubs / "lsblk").write_text("#!/bin/sh\necho '{\"blockdevices\":[]}'\n")
    for name in [
        "ssh",
        "ssh-keyscan",
        "sshfs",
        "rclone",
        "sudo",
        "pkexec",
        "udisksctl",
        "systemctl",
        "xdg-open",
        "xdg-terminal-exec",
        "zoxide",
    ]:
        (stubs / name).write_text("#!/bin/sh\nexit 1\n")
    for p in stubs.iterdir():
        p.chmod(0o755)
    (out / "fixture.c").write_text(FUSE_SOURCE)
    subprocess.run(
        [
            "cc",
            "-Wall",
            "-Wextra",
            "-Werror",
            str(out / "fixture.c"),
            "-o",
            str(out / "fixture"),
            *shlex.split(flags),
        ],
        check=True,
    )
    control = out / "control"
    control.write_text("hold")
    entered = out / "entered"
    env = {
        k: v
        for k, v in os.environ.items()
        if k in ["PATH", "LANG", "LC_ALL", "XDG_DATA_DIRS"]
    }
    env.update(
        HOME=str(home),
        XDG_RUNTIME_DIR=str(runtime),
        XDG_CONFIG_HOME=str(config),
        XDG_DATA_HOME=str(home / ".local/share"),
        XDG_STATE_HOME=str(home / ".local/state"),
        XDG_CACHE_HOME=str(home / ".cache"),
        PATH=f"{stubs}:/usr/bin:/bin",
        WLR_BACKENDS="headless",
        WLR_LIBINPUT_NO_DEVICES="1",
        WLR_HEADLESS_OUTPUTS="1",
        QT_QPA_PLATFORM="wayland",
        QT_QUICK_BACKEND="software",
        GIO_USE_VFS="local",
        GVFS_DISABLE_FUSE="1",
        GUST_QA_STATE=str(out / "state.json"),
        GUST_OMARCHY_STATE=str(out / "theme-state"),
    )
    children = []
    logs = []
    checks = []
    timings = []
    result = 1

    def spawn(argv, name):
        log = (out / name).open("w")
        logs.append(log)
        p = subprocess.Popen(
            argv, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True
        )
        children.append(p)
        return p

    def run(argv):
        return subprocess.run(
            argv, env=env, capture_output=True, text=True, check=True, timeout=10
        )

    def state():
        try:
            return json.loads((out / "state.json").read_text())
        except (OSError, ValueError):
            return {}

    def wait(predicate, label, timeout=10):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if predicate():
                return
            time.sleep(0.02)
        raise AssertionError(label + ": " + json.dumps(state().get("model", {})))

    def check(ok, label):
        checks.append({"check": label, "passed": bool(ok)})
        print(("PASS: " if ok else "FAIL: ") + label, flush=True)
        if not ok:
            raise AssertionError(label)

    def key(k, ctrl=False):
        run(
            [
                "wtype",
                *(["-M", "ctrl"] if ctrl else []),
                "-k",
                k,
                *(["-m", "ctrl"] if ctrl else []),
            ]
        )

    def navigate(path):
        for _ in range(5):
            key("l", True)
            time.sleep(0.25)
            if state().get("paletteOpen"):
                break
        wait(lambda: state().get("paletteOpen"), "path input")
        run(["wtype", str(path)])
        key("Return")
        wait(
            lambda: (
                state().get("model", {}).get("location") == str(path)
                and state()["model"]["settled"]
            ),
            "navigation",
        )

    def read_count():
        return len(entered.read_text().splitlines()) if entered.exists() else 0

    def source_workers():
        return sum(
            p.read_text().strip() == "gust-source-rea"
            for p in Path(f"/proc/{app.pid}/task").glob("*/comm")
        )

    try:
        fuse = spawn(
            [str(out / "fixture"), str(mount), str(control), str(entered)], "fuse.log"
        )
        wait(lambda: os.path.ismount(mount) or fuse.poll() is not None, "FUSE startup")
        if fuse.poll() is not None:
            print("SKIP: private FUSE mount refused")
            result = 77
            return result
        bus = subprocess.Popen(
            [
                "dbus-daemon",
                "--session",
                "--nofork",
                "--print-address=1",
                "--nopidfile",
            ],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        children.append(bus)
        env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().decode().strip()
        spawn(["labwc"], "labwc.log")
        wait(lambda: (out / "display").exists(), "private compositor")
        env["WAYLAND_DISPLAY"] = (out / "display").read_text()
        run(["wlr-randr", "--output", "HEADLESS-1", "--custom-mode", "1280x900"])
        env["XDG_CONFIG_HOME"] = str(home / ".config")
        app = spawn([str(binary), str(home / "Documents")], "gust.log")
        wait(lambda: state().get("model", {}).get("settled"), "app startup")
        time.sleep(0.7)
        navigate(home / "Documents")
        for mode in ["grid", "columns", "rows"]:
            run(["wtype", ":"])
            wait(lambda: state().get("paletteOpen"), "view palette")
            run(["wtype", "View: " + mode])
            time.sleep(0.2)
            key("Return")
            wait(lambda mode=mode: state().get("view") == mode, "view switch")
            key("End")
            wait(lambda: state().get("model", {}).get("cursor") == 200, "far jump")
            key("Home")
            wait(
                lambda: state().get("model", {}).get("cursor") == 0,
                "return to first row",
            )
            key("Down")
            wait(lambda: state().get("model", {}).get("cursor", 0) > 0, "cursor step")
            time.sleep(0.2)
            run(["grim", str(out / (mode + "-cursor.png"))])
            check(
                state()["model"]["cursor_name"].startswith("file-"),
                mode + " cursor survives far jumps and reuse",
            )
        key("F3")
        wait(lambda: state().get("paneCount") == 2, "two panes opened")
        navigate(mount)
        for i in range(4):
            key("Home")
            time.sleep(0.2)
            key("a", True)
            wait(lambda: state().get("selCount") == 1, "source selected")
            key("F6" if i == 1 else "F5")
            wait(lambda expected=i + 1: read_count() == expected, "held source read")
            started = time.monotonic()
            key("Escape")
            wait(
                lambda: (
                    state().get("jobState") == "Cancelled"
                    and not state().get("jobRunning")
                ),
                "cancel completes while source remains held",
                timeout=2,
            )
            timings.append(round((time.monotonic() - started) * 1000, 1))
            check(
                control.read_text() == "hold"
                and not (home / "Documents/held.bin").exists(),
                "cancelled transfer publishes no destination before source release",
            )
            check(
                not list((home / "Documents").glob(".gust-staging-*")),
                "cancelled copy removes owned partial staging",
            )
            check(
                (mount / "held.bin").stat().st_size == 2 * 1024 * 1024,
                "cancelled copy/move retains source",
            )
        check(
            source_workers() == 4,
            "four cancelled reads retain exactly four bounded source workers",
        )
        key("F5")
        wait(
            lambda: (
                state().get("jobState") == "Failed" and not state().get("jobRunning")
            ),
            "saturated reader refusal",
        )
        check(
            "waiting for a drive" in state().get("jobMessage", ""),
            "reader exhaustion gives an actionable refusal",
        )
        check(
            read_count() == 4
            and source_workers() == 4
            and not list((home / "Documents").glob(".gust-staging-*")),
            "reader exhaustion neither starts a fifth read nor creates staging",
        )
        run(["grim", str(out / "cancelled-and-bounded.png")])
        control.write_text("release")
        wait(lambda: source_workers() == 0, "released read workers retire")
        key("F5")
        wait(
            lambda: (
                state().get("jobState") == "Confirmed" and not state().get("jobRunning")
            ),
            "copy after recovery",
        )
        check(
            source_workers() == 0,
            "successful copy retires its source reader before completion",
        )
        check(
            hashlib.sha256((home / "Documents/held.bin").read_bytes()).hexdigest()
            == hashlib.sha256(b"Z" * (2 * 1024 * 1024)).hexdigest(),
            "recovered source copies every byte",
        )
        key("z", True)
        wait(
            lambda: not (home / "Documents/held.bin").exists(),
            "identity-checked undo removes copied file",
        )
        check(
            (mount / "held.bin").stat().st_size == 2 * 1024 * 1024,
            "undo retains the original source",
        )
        run(["grim", str(out / "recovered-and-undone.png")])
        errors = [
            line
            for line in (out / "gust.log").read_text().splitlines()
            if re.search(
                r"ReferenceError|TypeError|Binding loop|Cannot assign|Unable to assign|is not a function|QML creation FAILED",
                line,
            )
        ]
        check(not errors, "native cancellation journey has no QML runtime errors")
        print("Cancellation milliseconds: " + json.dumps(timings), flush=True)
        result = 0
    finally:
        control.write_text("release")
        for p in reversed(children):
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGTERM)
                try:
                    p.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait()
        for log in logs:
            log.close()
        subprocess.run(
            ["fusermount3", "-u", str(mount)],
            check=False,
            capture_output=True,
            timeout=10,
        )
        (out / "receipt.json").write_text(
            json.dumps(
                {
                    "exit": result,
                    "checks": checks,
                    "cancel_ms": timings,
                    "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                    "scope": "Native Rust/CXX/QML, private Wayland and FUSE; synthetic files only.",
                },
                indent=2,
            )
            + "\n"
        )
    return result


if __name__ == "__main__":
    raise SystemExit(main())
