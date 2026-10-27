#!/usr/bin/env python3
"""Native failed-location retry and deferred tag-dot regressions on private FUSE.

The fixture serves two synthetic text files, an injected listing failure and
held tag metadata. It opens no network connection and drives only its own
headless compositor. Screenshots and logs stay in a fresh external directory.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import time

try:
    from PIL import Image
except ImportError:
    print("SKIP: Pillow unavailable")
    raise SystemExit(77)


FUSE_SOURCE = r'''
#define FUSE_USE_VERSION 31
#include <fuse3/fuse.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static const char *control;
static int state_is(const char *wanted) {
    char state[32] = {0};
    FILE *f = fopen(control, "r");
    if (!f) return 0;
    (void)fgets(state, sizeof(state), f);
    fclose(f);
    return !strcmp(state, wanted);
}
static int attrs(const char *path, struct stat *st, struct fuse_file_info *fi) {
    (void)fi;
    memset(st, 0, sizeof(*st));
    st->st_uid = getuid(); st->st_gid = getgid();
    st->st_mtime = st->st_ctime = 1700000000;
    if (!strcmp(path, "/")) {
        st->st_mode = S_IFDIR | 0700; st->st_nlink = 2; st->st_ino = 1;
    } else if (!strcmp(path, "/a-tagged.txt") || !strcmp(path, "/z-plain.txt")) {
        st->st_mode = S_IFREG | 0600; st->st_nlink = 1;
        st->st_ino = !strcmp(path, "/a-tagged.txt") ? 2 : 3;
        st->st_size = 8;
    } else return -ENOENT;
    return 0;
}
static int listing(const char *path, void *buf, fuse_fill_dir_t fill, off_t off,
                   struct fuse_file_info *fi, enum fuse_readdir_flags flags) {
    (void)off; (void)fi; (void)flags;
    if (strcmp(path, "/")) return -ENOENT;
    if (state_is("fail")) return -EACCES;
    const char *names[] = {".", "..", "a-tagged.txt", "z-plain.txt"};
    for (int i = 0; i < 4; i++) {
        struct stat st;
        (void)attrs(i < 2 ? "/" : (i == 2 ? "/a-tagged.txt" : "/z-plain.txt"), &st, NULL);
        if (fill(buf, names[i], &st, 0, 0)) break;
    }
    return 0;
}
static int tags(const char *path, const char *name, char *buf, size_t size) {
    if (strcmp(path, "/a-tagged.txt") || strcmp(name, "user.xdg.tags")) return -ENODATA;
    while (state_is("hold-tags")) usleep(10000);
    const char value[] = "Red,Blue";
    if (!size) return sizeof(value) - 1;
    if (size < sizeof(value) - 1) return -ERANGE;
    memcpy(buf, value, sizeof(value) - 1);
    return sizeof(value) - 1;
}
static int contents(const char *path, char *buf, size_t size, off_t off,
                    struct fuse_file_info *fi) {
    (void)path; (void)fi;
    const char value[] = "fixture\n";
    if (off < 0 || off >= (off_t)(sizeof(value) - 1)) return 0;
    size_t left = sizeof(value) - 1 - off;
    if (size > left) size = left;
    memcpy(buf, value + off, size);
    return size;
}
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    control = argv[2];
    struct fuse_operations ops = {.getattr = attrs, .readdir = listing,
                                  .getxattr = tags, .read = contents};
    char *args[] = {argv[0], argv[1], "-f", "-o", "subtype=sshfs,fsname=gust-state-fixture,attr_timeout=0,entry_timeout=0", NULL};
    return fuse_main(5, args, &ops, NULL);
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    binary, out = args.binary.resolve(), args.output.resolve()
    if out.is_relative_to(repo) or out.exists():
        raise ValueError("output must be fresh and outside the checkout")
    required = ["cc", "pkg-config", "fusermount3", "labwc", "wlr-randr", "grim", "wtype", "dbus-daemon"]
    if not binary.is_file() or any(not shutil.which(tool) for tool in required):
        print("SKIP: binary or native fixture tools unavailable")
        return 77
    if not os.access("/dev/fuse", os.R_OK | os.W_OK):
        print("SKIP: unprivileged FUSE unavailable")
        return 77
    flags = subprocess.run(["pkg-config", "--cflags", "--libs", "fuse3"], capture_output=True, text=True)
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
    out.mkdir(parents=True, mode=0o700)
    home, runtime, compositor, stubs, mount = (out / n for n in ["home", "run", "compositor", "stubs", "mount"])
    for path in [home / "Documents", home / ".config/gust", home / ".cache",
                 home / ".local/share", home / ".local/state", runtime, compositor / "labwc", stubs, mount]:
        path.mkdir(parents=True, mode=0o700, exist_ok=True)
    for name in ["local-a.txt", "local-z.txt"]:
        (home / "Documents" / name).write_text("synthetic local file\n")
    (home / ".config/gust/views.toml").write_text("local:" + os.fsencode(mount).hex() + ' = "grid"\n')
    (compositor / "labwc/rc.xml").write_text('<labwc_config><windowRules><windowRule identifier="*"><action name="Maximize"/></windowRule></windowRules></labwc_config>')
    (compositor / "labwc/autostart").write_text('printf "%s/%s" "$XDG_RUNTIME_DIR" "$WAYLAND_DISPLAY" > "' + str(out / "display") + '"\n')
    (stubs / "tailscale").write_text('#!/bin/sh\necho \'{"BackendState":"Stopped"}\'\n')
    (stubs / "lsblk").write_text('#!/bin/sh\necho \'{"blockdevices":[]}\'\n')
    for name in ["ssh", "ssh-keyscan", "sshfs", "rclone", "sudo", "pkexec", "udisksctl", "systemctl", "xdg-open", "xdg-terminal-exec", "zoxide"]:
        (stubs / name).write_text("#!/bin/sh\nexit 1\n")
    for path in stubs.iterdir():
        path.chmod(0o755)
    (out / "fixture.c").write_text(FUSE_SOURCE)
    subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(out / "fixture.c"), "-o", str(out / "fixture"), *shlex.split(flags)], check=True)
    control = out / "control"
    control.write_text("fail")
    env = {k: v for k, v in os.environ.items() if k in ["PATH", "LANG", "LC_ALL", "XDG_DATA_DIRS"]}
    env.update(HOME=str(home), XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(compositor),
               XDG_DATA_HOME=str(home / ".local/share"), XDG_STATE_HOME=str(home / ".local/state"),
               XDG_CACHE_HOME=str(home / ".cache"), PATH=f"{stubs}:/usr/bin:/bin",
               WLR_BACKENDS="headless", WLR_LIBINPUT_NO_DEVICES="1", WLR_HEADLESS_OUTPUTS="1",
               QT_QPA_PLATFORM="wayland", QT_QUICK_BACKEND="software", GIO_USE_VFS="local",
               GVFS_DISABLE_FUSE="1", GUST_QA_STATE=str(out / "state.json"), GUST_OMARCHY_STATE=str(out / "theme-state"))
    processes, logs, checks = [], [], []

    def spawn(argv, name):
        log = (out / name).open("w")
        logs.append(log)
        child = subprocess.Popen(argv, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(child)
        return child

    def run(argv):
        return subprocess.run(argv, env=env, capture_output=True, text=True, check=True, timeout=10)

    def state():
        try:
            return json.loads((out / "state.json").read_text())
        except (OSError, ValueError):
            return {}

    def wait(predicate, label, timeout=12):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            if predicate():
                return
            time.sleep(.025)
        raise AssertionError(label)

    def check(condition, label):
        checks.append({"check": label, "passed": bool(condition)})
        print(("PASS: " if condition else "FAIL: ") + label, flush=True)

    def navigate(path):
        run(["wtype", "-M", "ctrl", "-k", "l", "-m", "ctrl"])
        wait(lambda: state().get("paletteOpen"), "path input focused")
        run(["wtype", str(path)])
        run(["wtype", "-k", "Return"])
        time.sleep(.1)

    result = 1
    try:
        fuse = spawn([str(out / "fixture"), str(mount), str(control)], "fuse.log")
        wait(lambda: os.path.ismount(mount) or fuse.poll() is not None, "FUSE startup")
        if fuse.poll() is not None:
            print("SKIP: private FUSE mount refused")
            result = 77
            return 77
        assert any(str(mount) in line and " - fuse.sshfs " in line
                   for line in Path("/proc/self/mountinfo").read_text().splitlines())
        bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1", "--nopidfile"],
                               env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
        processes.append(bus)
        env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().decode().strip()
        spawn(["labwc"], "labwc.log")
        wait(lambda: (out / "display").exists(), "private compositor")
        env["WAYLAND_DISPLAY"] = (out / "display").read_text()
        run(["wlr-randr", "--output", "HEADLESS-1", "--custom-mode", "1280x900"])
        env["XDG_CONFIG_HOME"] = str(home / ".config")
        spawn([str(binary), str(home / "Documents")], "gust.log")
        wait(lambda: state().get("model", {}).get("settled") and state()["model"]["count"] == 2, "local startup")
        time.sleep(.8)
        for _ in range(6):
            run(["wtype", "-k", "End"])
            time.sleep(.3)
            if state().get("model", {}).get("cursor") == 1:
                break
        wait(lambda: state().get("model", {}).get("cursor") == 1, "private compositor keyboard focus")
        navigate(mount)
        wait(lambda: state().get("model", {}).get("location") == str(mount) and state()["model"]["settled"], "failed remote request")
        failed = state()
        check(failed["model"]["count"] == 0 and bool(failed["emptyReason"]), "failed remote listing shows an error instead of empty-folder text")
        run(["grim", str(out / "failed-location.png")])
        control.write_text("ok")
        navigate(mount)
        until = time.monotonic() + 3
        while time.monotonic() < until and state().get("model", {}).get("count") != 2:
            time.sleep(.025)
        recovered = state()
        check(recovered["model"]["count"] == 2 and recovered["model"]["generation"] > failed["model"]["generation"]
              and not recovered["emptyReason"], "recovered location can be retried by submitting the identical path")
        check(not recovered["notice"], "retry retires the previous remote failure notice")

        navigate(home / "Documents")
        wait(lambda: state().get("model", {}).get("location") == str(home / "Documents") and state()["model"]["settled"], "local reset")
        control.write_text("hold-tags")
        navigate(mount)
        wait(lambda: state().get("model", {}).get("location") == str(mount) and state()["model"]["settled"]
             and state()["model"]["count"] == 2 and "a-tagged.txt" in state().get("rowPoints", {}), "names before metadata")
        before = state()
        check(not before["rowTags"], "initial remote delegates exist before held tag metadata arrives")
        run(["grim", str(out / "before-tags.png")])
        control.write_text("ok")
        wait(lambda: state().get("rowTags", {}).get("a-tagged.txt") == "red,blue", "tag metadata publication")
        time.sleep(.4)
        after = state()
        run(["grim", str(out / "after-tags.png")])
        x, y = after["rowPoints"]["a-tagged.txt"]
        bounds = (max(180, int(x) - 65), max(65, int(y) - 85), int(x) + 65, int(y) + 30)

        def red_pixels(name):
            image = Image.open(out / name).convert("RGB").crop(bounds)
            pixels = image.load()
            count = 0
            for y in range(image.height):
                for x in range(image.width):
                    r, g, b = pixels[x, y]
                    count += r > 220 and 75 < g < 155 and 100 < b < 180
            return count

        check(red_pixels("before-tags.png") == 0 and red_pixels("after-tags.png") >= 9,
              "existing remote delegate paints its red tag dot when metadata arrives")
        check(after["model"]["generation"] == before["model"]["generation"] and after["view"] == before["view"] == "grid"
              and after["listScroll"] == before["listScroll"] and after["rowPoints"] == before["rowPoints"],
              "tag update needs no navigation, scroll, sort or delegate repositioning")
        errors = [line for line in (out / "gust.log").read_text().splitlines()
                  if re.search(r"ReferenceError|TypeError|Binding loop|Cannot assign|is not a function|QML creation FAILED", line)]
        (out / "qml-warnings.txt").write_text("\n".join(errors))
        check(not errors, "native journey has no QML runtime errors")
        result = 0 if all(item["passed"] for item in checks) else 1
    finally:
        control.write_text("ok")
        for child in reversed(processes):
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
        for log in logs:
            log.close()
        subprocess.run(["fusermount3", "-u", str(mount)], capture_output=True, timeout=10)
        (out / "receipt.json").write_text(json.dumps({"exit": result, "checks": checks,
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "driver_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "scope": "Native Rust/CXX/QML on synthetic FUSE in a private Wayland session; no live provider or network."}, indent=2) + "\n")
    return result


if __name__ == "__main__":
    raise SystemExit(main())
