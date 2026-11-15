#!/usr/bin/env python3
"""Disposable matched Qt floor/Gust runs. All generated files stay in --output."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import shutil
import signal
import subprocess
import threading
import time

import psutil

SOURCE = Path(__file__).resolve().parent
HEADER = re.compile(r"^[0-9a-f]+-[0-9a-f]+\s")


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def fields(text):
    return dict(line.split(":", 1) for line in text.splitlines() if ":" in line)


def mappings(text):
    result = []
    for line in text.splitlines():
        if HEADER.match(line):
            parts = line.split(maxsplit=5)
            result.append({"header": line, "path": parts[5] if len(parts) == 6 else "[anonymous]",
                           "fields_kib": {}})
        elif result:
            match = re.match(r"(\w+):\s+(\d+) kB$", line)
            if match:
                result[-1]["fields_kib"][match[1]] = int(match[2])
    return result


def snapshot(pid, dest):
    dest.mkdir()
    root = psutil.Process(pid)
    procs = [root] + root.children(recursive=True)
    result = {"processes": [], "errors": [], "drm_clients": [], "pss_kib": 0, "uss_kib": 0}
    clients = {}
    for proc in procs:
        prefix = Path(f"/proc/{proc.pid}")
        row = {"pid": proc.pid}
        result["processes"].append(row)
        try:
            row.update(name=proc.name(), exe=proc.exe(), created=proc.create_time())
            smaps = (prefix / "smaps").read_text()
            (dest / f"{proc.pid}.smaps").write_text(smaps)
            maps = mappings(smaps)
            (dest / f"{proc.pid}.mappings.json").write_text(json.dumps(maps, indent=2))
            row["pss_kib"] = sum(m["fields_kib"].get("Pss", 0) for m in maps)
            row["uss_kib"] = sum(m["fields_kib"].get(k, 0) for m in maps
                                 for k in ("Private_Clean", "Private_Dirty"))
            result["pss_kib"] += row["pss_kib"]
            result["uss_kib"] += row["uss_kib"]
            cpu = proc.cpu_times()
            row["cpu_s"] = cpu.user + cpu.system
        except (OSError, psutil.Error) as exc:
            result["errors"].append(f"pid {proc.pid} smaps/identity: {exc}")
        try:
            fds = list((prefix / "fdinfo").iterdir())
        except OSError as exc:
            result["errors"].append(f"pid {proc.pid} fdinfo: {exc}")
            continue
        for fd in fds:
            try:
                raw = fd.read_text()
                if "drm-client-id:" not in raw:
                    continue
                (dest / f"{proc.pid}.fdinfo-{fd.name}").write_text(raw)
                info = {k: v.strip() for k, v in fields(raw).items()}
                stat = (prefix / "fd" / fd.name).stat()
                device = info.get("drm-pdev") or str(
                    Path(f"/sys/dev/char/{os.major(stat.st_rdev)}:{os.minor(stat.st_rdev)}/device").resolve())
                key = (device, info["drm-client-id"])
                ref = {"pid": proc.pid, "fd": fd.name, "node": os.readlink(prefix / "fd" / fd.name)}
                if key in clients:
                    clients[key]["references"].append(ref)
                    if clients[key]["fields"] != {k: v for k, v in info.items() if k.startswith("drm-")}:
                        clients[key]["later_observations"].append({"reference": ref, "fields": info})
                else:
                    clients[key] = {"device": device, "client_id": key[1], "references": [ref],
                                    "fields": {k: v for k, v in info.items() if k.startswith("drm-")},
                                    "later_observations": []}
            except OSError as exc:
                result["errors"].append(f"pid {proc.pid} fd {fd.name}: {exc}")
    result["drm_clients"] = list(clients.values())
    # Each device+client appears once. Each region/counter remains separate.
    region_totals = {}
    for client in clients.values():
        for key, value in client["fields"].items():
            if re.match(r"drm-(memory|total|resident|shared|active|purgeable)-", key):
                # amdgpu prints unitless zero for empty shared regions.
                if value == "0":
                    region_totals.setdefault(key, 0)
                    continue
                match = re.fullmatch(r"(\d+) (KiB|kB|B|MiB)", value)
                if not match:
                    raise ValueError(f"Unknown DRM memory unit: {key}: {value}")
                factor = {"KiB": 1024, "kB": 1024, "B": 1, "MiB": 1024**2}[match[2]]
                region_totals[key] = region_totals.get(key, 0) + int(match[1]) * factor
    result["drm_region_totals_bytes"] = region_totals
    (dest / "snapshot.json").write_text(json.dumps(result, indent=2))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--gust", type=Path, required=True, help="Existing binary; never built here")
    parser.add_argument("--fixtures", type=Path, required=True, help="Existing empty/ and 100k/ directories")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--render-node", default="/dev/dri/renderD129")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    if not (args.fixtures / "empty").is_dir() or not (args.fixtures / "100k").is_dir():
        raise ValueError("Missing fixtures")
    if args.rounds < 1:
        raise ValueError("rounds must be positive")
    build = out / "build"
    build.mkdir()
    with (out / "build.log").open("w") as log:
        def build_cmd(argv):
            subprocess.run(argv, cwd=build, stdout=log, stderr=subprocess.STDOUT, check=True)
        build_cmd(["qmake6", str(SOURCE / "floor.pro")])
        build_cmd(["make", "-j2"])
        protocol = "/usr/share/wayland-protocols/staging/ext-foreign-toplevel-list/ext-foreign-toplevel-list-v1.xml"
        build_cmd(["wayland-scanner", "client-header", protocol, "toplevel.h"])
        build_cmd(["wayland-scanner", "private-code", protocol, "toplevel.c"])
        build_cmd(["cc", "-O2", "-Wall", "-Wextra", "-I", str(build), str(SOURCE / "monitor.c"),
                   "toplevel.c", "-lwayland-client", "-o", "monitor"])
    gust = out / "gust-snapshot"
    shutil.copy2(args.gust, gust)
    binaries = {"floor": build / "qt-floor", "gust": gust}
    manifest = {"started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "host": subprocess.check_output(["uname", "-a"], text=True),
                "qmake": subprocess.check_output(["qmake6", "-query"], text=True),
                "sources": {p.name: sha(p) for p in SOURCE.iterdir() if p.is_file()},
                "binaries": {name: {"path": str(p), "sha256": sha(p),
                                     "ldd": subprocess.check_output(["ldd", str(p)], text=True)}
                             for name, p in binaries.items()},
                "gust_original": str(args.gust.resolve()), "rounds": args.rounds,
                "fixtures": {name: {"path": str((args.fixtures / name).resolve()),
                                     "entry_count": sum(1 for _ in (args.fixtures / name).iterdir())}
                             for name in ("empty", "100k")},
                "drm_devices": {p.name: (p / "device/uevent").read_text()
                                for p in Path("/sys/class/drm").glob("renderD*")},
                "qt_libraries": {str(p): sha(p) for p in
                                 (Path("/usr/lib/libQt6Core.so.6"), Path("/usr/lib/libQt6Gui.so.6"),
                                  Path("/usr/lib/libQt6Qml.so.6"), Path("/usr/lib/libQt6Quick.so.6"))},
                "scope": "1280x900 headless labwc GLES2; no maximized decoration; process+live descendants only",
                "cache": "Fresh per-launch HOME/XDG; warm host caches, no cache dropping",
                "sampling": "map+3 seconds; sequential /proc reads, not atomic; no paint/input-ready claim",
                "accounting": "smaps PSS separate from DRM; dedup by device+client-id; first fd observation; all raw fds retained; no ownership subtraction",
                "renderer_env": {"gl": {"QSG_RHI_BACKEND": "opengl"},
                                 "vulkan": {"QSG_RHI_BACKEND": "vulkan", "VK_DRIVER_FILES": "/usr/share/vulkan/icd.d/radeon_icd.json"},
                                 "software": {"QT_QUICK_BACKEND": "software"}}}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    # Do not queue behind Stage1 or disturb an existing desktop run.
    lock = open("/tmp/gust-qa.lock", "a")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    runtime = out / "runtime"
    runtime.mkdir(mode=0o700)
    config = out / "compositor"
    (config / "labwc").mkdir(parents=True)
    (config / "labwc/rc.xml").write_text(
        '<labwc_config><theme><maximizedDecoration>none</maximizedDecoration></theme>'
        '<windowRules><windowRule identifier="*"><action name="Maximize"/>'
        '</windowRule></windowRules></labwc_config>')
    (config / "labwc/autostart").write_text(
        'printf "%s/%s" "$XDG_RUNTIME_DIR" "$WAYLAND_DISPLAY" > "$GUST_QT_FLOOR_OUT/display"\n')
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL", "XDG_DATA_DIRS")}
    env.update(GUST_QT_FLOOR_OUT=str(out), XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(config), WLR_BACKENDS="headless",
               WLR_RENDERER="gles2", WLR_RENDER_DRM_DEVICE=args.render_node,
               WLR_LIBINPUT_NO_DEVICES="1", WLR_HEADLESS_OUTPUTS="1", QT_QPA_PLATFORM="wayland",
               GIO_USE_VFS="local", GVFS_DISABLE_FUSE="1")
    owned, logs, results = [], [], []

    def spawn(argv, name, process_env=None, pipe=False):
        log = (out / name).open("w")
        logs.append(log)
        proc = subprocess.Popen(argv, env=process_env or env, stdout=subprocess.PIPE if pipe else log,
                                stderr=log, start_new_session=True, text=True, bufsize=1)
        owned.append(proc)
        return proc

    def stop(proc):
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()

    try:
        bus = spawn(["dbus-daemon", "--session", "--nofork", "--print-address=1", "--nopidfile"], "bus.log", pipe=True)
        # dbus-daemon's address is available immediately in this private session.
        env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
        compositor = spawn(["labwc"], "labwc.log")
        deadline = time.monotonic() + 15
        while not (out / "display").exists():
            if compositor.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("Private compositor unavailable")
            time.sleep(.02)
        env["WAYLAND_DISPLAY"] = (out / "display").read_text()
        subprocess.run(["wlr-randr", "--output", "HEADLESS-1", "--custom-mode", "1280x900"], env=env, check=True, capture_output=True)
        monitor = spawn([str(build / "monitor")], "monitor.log", pipe=True)
        events = queue.Queue()

        def reader():
            for line in monitor.stdout:
                events.put(line.strip())
        threading.Thread(target=reader, daemon=True).start()
        if events.get(timeout=10) != "READY":
            raise RuntimeError("Map observer not ready")
        for renderer, settings in manifest["renderer_env"].items():
            for fixture in ("empty", "100k"):
                for iteration in range(args.rounds):
                    for app in (["floor", "gust"] if iteration % 2 == 0 else ["gust", "floor"]):
                        label = f"{renderer}-{fixture}-{iteration}-{app}"
                        home = out / label
                        for sub in (".config", ".cache", ".local/state", ".local/share", "Documents"):
                            (home / sub).mkdir(parents=True, exist_ok=True)
                        app_env = dict(env, HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"),
                                       XDG_CACHE_HOME=str(home / ".cache"), XDG_STATE_HOME=str(home / ".local/state"),
                                       XDG_DATA_HOME=str(home / ".local/share"), GUST_OMARCHY_STATE=str(home / "theme"),
                                       QSG_INFO="1", QT_FORCE_STDERR_LOGGING="1", QT_LOGGING_TO_CONSOLE="1", **settings)
                        while not events.empty():
                            events.get_nowait()
                        argv = [str(binaries[app]), str(0 if fixture == "empty" else 100000) if app == "floor"
                                else str((args.fixtures / fixture).resolve())]
                        start = time.monotonic()
                        proc = spawn(argv, label + ".log", app_env)
                        mapped = None
                        while time.monotonic() < start + 25 and proc.poll() is None:
                            try:
                                line = events.get(timeout=.05)
                            except queue.Empty:
                                continue
                            if line.startswith("MAP "):
                                mapped = float(line.split()[1])
                                break
                        if mapped is None:
                            raise RuntimeError(f"{label}: no map, return code {proc.poll()}")
                        time.sleep(max(0, mapped + 3 - time.monotonic()))
                        sample_started = time.monotonic()
                        sample = snapshot(proc.pid, out / (label + "-proc"))
                        row = {"label": label, "app": app, "renderer": renderer, "fixture": fixture,
                               "iteration": iteration, "map_ms": (mapped - start) * 1000,
                               "sample_start_after_map_s": sample_started - mapped,
                               "sample_duration_s": time.monotonic() - sample_started, **sample}
                        results.append(row)
                        (out / "results.json").write_text(json.dumps(results, indent=2))
                        if iteration == 0:
                            subprocess.run(["grim", str(out / (label + ".png"))], env=env, check=True, capture_output=True)
                        print(json.dumps({k: row[k] for k in ("label", "pss_kib", "uss_kib", "drm_region_totals_bytes")}), flush=True)
                        stop(proc)
                        time.sleep(.15)
    finally:
        for proc in reversed(owned):
            stop(proc)
        for log in logs:
            log.close()
        fcntl.flock(lock, fcntl.LOCK_UN)
        lock.close()
    print("EVIDENCE", out, flush=True)


if __name__ == "__main__":
    main()
