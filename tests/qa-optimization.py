#!/usr/bin/env python3
"""Exercise native filtering, preferences and chooser sorting in an owned Wayland lab.

Only synthetic files and this process's children are used. Observation timings
end at the QA state receipt, not at presentation; they are not paint latency.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import shlex
import subprocess
import sys
import threading
import time
import tomllib

sys.dont_write_bytecode = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--map-monitor", type=Path,
                        help="optional existing private Wayland toplevel observer; receipt is not a presented frame")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--entries", type=int)
    parser.add_argument("--preferences", action="store_true")
    parser.add_argument("--projection", action="store_true", help="exercise revision-safe sort/filter/selection/live publication; defaults to 100k entries")
    parser.add_argument("--publication-probe", action="store_true",
                        help="enable bounded per-publication attribution in a supporting diagnostic artifact")
    parser.add_argument("--projection-handoff", choices=("menu", "rename", "all"),
                        help="controlled pending-metadata action handoffs; implies --projection")
    parser.add_argument("--projection-search-jobs", action="store_true",
                        help="Find/grep results survive an owned copy completion; implies --projection")
    parser.add_argument("--projection-search-operations", action="store_true",
                        help="qualify deferred Find/grep hit refresh after owned Trash/rename/Undo; implies --projection")
    parser.add_argument("--projection-scalar-sorts", action="store_true",
                        help="qualify Size/Modified ordering, target preservation and refresh; implies --projection")
    parser.add_argument("--projection-size-cache", action="store_true",
                        help="qualify native folder totals through owned Copy/Undo; implies --projection")
    parser.add_argument("--projection-soak-cycles", type=int, default=0,
                        help="after three warm-up cycles, sample resources for this many repeated filter/selection cycles")
    parser.add_argument("--projection-soak-only", action="store_true",
                        help="stop after the declared filter/selection retention cycles; implies --projection")
    parser.add_argument("--projection-native-cpu-control", action="store_true",
                        help="diagnostic external GUI-thread counters and appended single-key control; requires software100k soak-only with five measured cycles")
    parser.add_argument("--pointer-smoke", action="store_true", help="qualify private Wayland pointer with a non-destructive header click")
    parser.add_argument("--chooser-sort", action="store_true", help="exercise transient Size sorting and reload in an Open chooser")
    parser.add_argument("--capture-only", action="store_true", help="capture the actual window without interaction journeys")
    parser.add_argument("--theme", type=Path, help="copy this read-only theme into private application state")
    parser.add_argument("--renderer", choices=("software", "opengl", "vulkan"), default="software")
    parser.add_argument("--render-node", type=Path, help="explicit compositor render node for GPU journeys")
    parser.add_argument("--vulkan-icd", type=Path, help="explicit Vulkan driver manifest for a qualified GPU")
    parser.add_argument("--session-stress", type=int, default=0)
    parser.add_argument("--session-tail-regression", action="store_true", help="restore and save 40 tabs with 160 trailing unknown lines; uses 100 fixture entries")
    parser.add_argument("--location-tail-regression", action="store_true", help="save sort/view choices with 2000 unknown tail comments per document; uses 100 fixture entries")
    args = parser.parse_args()
    if args.projection_handoff or args.projection_search_jobs or args.projection_search_operations or args.projection_scalar_sorts or args.projection_size_cache or args.projection_soak_only:
        args.projection = True
    if sum(bool(x) for x in (args.projection_handoff, args.projection_search_jobs, args.projection_search_operations, args.projection_scalar_sorts, args.projection_size_cache, args.projection_soak_only)) > 1:
        parser.error("handoff, search-jobs, search-operations, scalar-sorts, size-cache and soak-only are separate projection journeys")
    if args.projection_size_cache and args.publication_probe:
        parser.error("size-cache qualification requires the publication probe to stay disabled")
    runner_sha = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    projection_source = Path(__file__).with_name("qa_projection.py")
    projection_sha = hashlib.sha256(projection_source.read_bytes()).hexdigest() if args.projection else None
    if args.entries is None:
        if args.projection_search_jobs or args.projection_search_operations:
            args.entries = 99999
        elif args.projection:
            args.entries = 100000
        else:
            args.entries = 100 if args.session_tail_regression or args.location_tail_regression or args.pointer_smoke else 2000
    if (args.projection_search_jobs or args.projection_search_operations) and args.entries > 99999:
        parser.error("search qualification allows at most 99999 visible entries plus the hidden entry (100000 scan budget)")
    if args.projection_native_cpu_control and not (args.projection_soak_only
            and args.projection_soak_cycles == 5 and args.entries == 100000
            and args.renderer == "software"):
        parser.error("projection-native-cpu-control requires --projection-soak-only --projection-soak-cycles 5 --entries 100000 --renderer software")
    if args.projection and args.pointer_smoke:
        parser.error("projection and pointer-smoke are separate journeys")
    if not 0 <= args.projection_soak_cycles <= 100 or (args.projection_soak_cycles and not args.projection):
        parser.error("projection-soak-cycles requires --projection and must be between 0 and 100")
    if args.projection_soak_only and (not args.projection_soak_cycles or args.projection_handoff or args.projection_search_jobs):
        parser.error("projection-soak-only requires positive soak cycles and a separate journey")
    if (args.projection or args.pointer_smoke) and (args.preferences or args.capture_only or args.chooser_sort or args.session_stress or args.session_tail_regression or args.location_tail_regression):
        parser.error("projection is a separate journey; omit other journey flags")
    if not 0 <= args.session_stress <= 500:
        parser.error("session-stress must be between 0 and 500")
    if args.session_stress and (args.capture_only or args.preferences):
        parser.error("session-stress is a separate journey; omit preferences and capture-only")
    if args.chooser_sort and (args.capture_only or args.preferences or args.session_stress):
        parser.error("chooser-sort is a separate journey; omit preferences, capture-only and session-stress")
    if args.chooser_sort and args.entries <= 256:
        parser.error("chooser-sort requires more than 256 entries to leave metadata unloaded")
    if args.session_tail_regression and (args.preferences or args.capture_only or args.chooser_sort or args.session_stress):
        parser.error("session-tail-regression is a separate journey; omit other journey flags")
    if args.session_tail_regression and args.entries != 100:
        parser.error("session-tail-regression requires exactly 100 fixture entries")
    if args.location_tail_regression and (args.preferences or args.capture_only or args.chooser_sort or args.session_stress or args.session_tail_regression):
        parser.error("location-tail-regression is a separate journey; omit other journey flags")
    if args.location_tail_regression and args.entries != 100:
        parser.error("location-tail-regression requires exactly 100 fixture entries")
    if args.renderer != "software" and (args.render_node is None or not args.render_node.exists()):
        parser.error("GPU journeys require an existing explicit --render-node")
    if args.vulkan_icd and not args.vulkan_icd.is_file():
        parser.error("Vulkan manifest must exist")
    if args.theme and not (args.theme / "colors.toml").is_file():
        parser.error("theme requires colors.toml")
    binary = args.binary.resolve(strict=True)
    if args.map_monitor and not (args.map_monitor.is_file() and os.access(args.map_monitor, os.X_OK)):
        parser.error("map-monitor must be an existing observer executable")
    out = args.output.absolute()
    if len(str(out / "r/wayland-0").encode()) >= 108:
        parser.error("output path is too long for a private Wayland socket; choose a shorter output")
    if not 1 <= args.entries <= 1_000_000:
        parser.error("entries must be between 1 and 1000000")
    if args.projection_handoff and args.entries < 100000:
        parser.error("projection-handoff requires at least 100000 entries for observable pending metadata")
    if not any(out.is_relative_to(base) for base in (Path("/tmp"), Path("/var/tmp"))):
        parser.error("output must be a fresh private directory under /tmp or /var/tmp")
    if any(p.is_symlink() for p in (out, *out.parents)) or out.exists():
        parser.error("output must be fresh and have no symlink ancestors")
    pointer_needed = args.projection or args.pointer_smoke
    required_tools = ("labwc", "dbus-daemon", "wlr-randr", "wtype", "grim")
    if pointer_needed:
        required_tools += ("wayland-scanner", "cc", "pkg-config")
    for tool in required_tools:
        if not shutil.which(tool):
            print(f"SKIP: missing {tool}")
            return 77
    lease = open("/tmp/gust-qa.lock", "a")
    try:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("SKIP: desktop QA lease busy")
        return 75
    out.mkdir(mode=0o700, parents=True)
    pointer_exe, pointer_receipt = None, None
    if pointer_needed:
        build = out / "pointer-build"
        build.mkdir(mode=0o700)
        source = Path(__file__).with_name("wayland-pointer.c")
        protocol = Path(__file__).with_name("protocols") / "wlr-virtual-pointer-unstable-v1.xml"
        header = build / "wlr-virtual-pointer-unstable-v1-client-protocol.h"
        code = build / "protocol.c"
        pointer_exe = build / "wayland-pointer"
        build_env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}
        flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "wayland-client"],
                                                   env=build_env, text=True, timeout=10))
        commands = [["wayland-scanner", "client-header", str(protocol), str(header)],
                    ["wayland-scanner", "private-code", str(protocol), str(code)],
                    ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(source), str(code),
                     "-I" + str(build), *flags, "-o", str(pointer_exe)]]
        with (build / "build.log").open("w") as log:
            for command in commands:
                subprocess.run(command, env=build_env, stdout=log, stderr=log, check=True, timeout=30)
        pointer_receipt = {"source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                           "protocol_sha256": hashlib.sha256(protocol.read_bytes()).hexdigest(),
                           "binary_sha256": hashlib.sha256(pointer_exe.read_bytes()).hexdigest(),
                           "commands": commands, "scope": "private single-output Wayland socket only; no host input device"}
    home, runtime, config, stubs, fixture = (out / n for n in ("home", "r", "compositor", "stubs", "fixture"))
    for p in (home / ".config/gust", home / ".cache", home / ".local/state", home / ".local/share", runtime, config / "labwc", stubs, fixture):
        p.mkdir(mode=0o700, parents=True, exist_ok=True)
    names = [f"file-{i:07d}.txt" for i in range(args.entries)]
    for i, name in enumerate(names):
        with (fixture / name).open("xb") as entry:
            if args.chooser_sort:
                # Unique sparse sizes make ascending Size order the exact
                # reverse of Name order without allocating large fixture data.
                entry.truncate(args.entries - i)
            elif args.projection_scalar_sorts:
                entry.truncate((i * 524287) % 17)
        if args.projection_scalar_sorts:
            stamp = (1700000000 + (i * 524287) % 19) * 1000000000
            os.utime(fixture / name, ns=(stamp, stamp))
    if args.projection_handoff:
        (fixture / names[0]).write_bytes(b"original A: larger\n")
        (fixture / names[1]).write_bytes(b"B\n")
    if args.projection_search_jobs or args.projection_search_operations:
        (fixture / names[0]).write_bytes(b"GUST_R2_SEARCH_MARKER: owned original bytes\n")
    (fixture / ".hidden").write_text("synthetic hidden file\n")
    if args.theme:
        theme = args.theme.resolve(strict=True)
        if not (theme / "colors.toml").is_file():
            raise ValueError("theme requires colors.toml")
        shutil.copytree(theme, out / "theme/current/theme")
        (out / "theme/current/theme.name").write_text(theme.name + "\n")
    pointer_theme = "<theme><maximizedDecoration>none</maximizedDecoration><keepBorder>no</keepBorder></theme>" if pointer_needed else ""
    (config / "labwc/rc.xml").write_text('<labwc_config>' + pointer_theme + '<keyboard><keybind key="A-F4"><action name="Close"/></keybind></keyboard><windowRules><windowRule identifier="*"><action name="Maximize"/></windowRule></windowRules></labwc_config>')
    (config / "labwc/autostart").write_text('printf "%s/%s" "$XDG_RUNTIME_DIR" "$WAYLAND_DISPLAY" > "$GUST_OPTIMIZATION_LAB/display"\n')
    for name in ("tailscale", "ssh", "ssh-keyscan", "sshfs", "rclone", "sudo", "pkexec", "udisksctl", "systemctl", "xdg-open", "xdg-terminal-exec", "zoxide"):
        (stubs / name).write_text("#!/bin/sh\nexit 1\n")
    (stubs / "lsblk").write_text('#!/bin/sh\necho \'{"blockdevices":[]}\'\n')
    if args.projection:
        # Only the owned synthetic activation target reaches this recorder.
        (stubs / "xdg-open").write_text("#!/usr/bin/python3\nimport json,os,sys\nfrom pathlib import Path\np=Path(os.environ['GUST_OPTIMIZATION_LAB'])/'opened.json'\nt=p.with_suffix('.tmp')\nt.write_text(json.dumps(sys.argv[1:]))\nt.replace(p)\n")
    for p in stubs.iterdir():
        p.chmod(0o755)
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL", "XDG_DATA_DIRS")}
    env.update(HOME=str(home), XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(config),
               XDG_DATA_HOME=str(home / ".local/share"), XDG_STATE_HOME=str(home / ".local/state"), XDG_CACHE_HOME=str(home / ".cache"),
               GUST_OPTIMIZATION_LAB=str(out), GUST_QA_STATE=str(out / "state.json"), GUST_OMARCHY_STATE=str(out / "theme"),
               PATH=f"{stubs}:/usr/bin:/bin", WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1", WLR_HEADLESS_OUTPUTS="1",
               QT_QPA_PLATFORM="wayland", QT_QUICK_BACKEND="software", GIO_USE_VFS="local", GVFS_DISABLE_FUSE="1")
    if args.publication_probe:
        env["GUST_QA_PUBLICATION_PROBE"] = "1"
    if args.chooser_sort:
        # Match the portal's child environment without registering a portal
        # or accepting/activating any file.
        env.update(GUST_CHOOSER="open", GUST_CHOOSER_RESULT=str(out / "chooser-result"),
                   GUST_CHOOSER_TITLE="Chooser sort regression")
    if args.renderer != "software":
        env.pop("QT_QUICK_BACKEND")
        env["QSG_RHI_BACKEND"] = args.renderer
        env["QSG_INFO"] = "1"
        env["WLR_RENDERER"] = "gles2"
        env["WLR_RENDER_DRM_DEVICE"] = str(args.render_node)
        if args.vulkan_icd:
            env["VK_DRIVER_FILES"] = str(args.vulkan_icd)
    children, logs, checks, samples, chooser_states = [], [], [], [], []
    startup = {}
    monitor_lines, monitor_thread = [], None
    session_tail = {}
    location_tail = {}

    def spawn(argv, log_name, **kwargs):
        log = (out / log_name).open("w")
        logs.append(log)
        p = subprocess.Popen(argv, env=env, stdout=kwargs.pop("stdout", log), stderr=log, start_new_session=True, **kwargs)
        children.append(p)
        return p

    def run(argv):
        # A new virtual keyboard needs a compositor turn after pointer-device
        # hotplug. Without this, the first key can disappear before Qt sees it.
        # This input delivery aid is part of receipt latency, never a frame claim.
        if argv[0] == "wtype":
            argv = [argv[0], "-s", "50", *argv[1:]]
        return subprocess.run(argv, env=env, check=True, capture_output=True, text=True, timeout=15)

    def state():
        try:
            return json.loads((out / "state.json").read_text())
        except (OSError, ValueError):
            return {}

    def wait(predicate, label, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if predicate():
                return
            time.sleep(.01)
        raise AssertionError(label)

    def check(ok, label):
        checks.append({"check": label, "passed": bool(ok)})
        print(("PASS: " if ok else "FAIL: ") + label, flush=True)
        if not ok:
            raise AssertionError(label)

    def key(name, modifier=None):
        run(["wtype", *(["-M", modifier] if modifier else []), "-k", name, *(["-m", modifier] if modifier else [])])

    def offer_text(text):
        owner = spawn(["wl-copy", "--foreground", "--type", "text/plain;charset=utf-8"],
                      "owned-text-clipboard.log", stdin=subprocess.PIPE, text=True)
        owner.stdin.write(text)
        owner.stdin.close()
        def ready_text():
            if owner.poll() is not None:
                raise AssertionError("owned text clipboard exited before admission")
            try:
                return run(["wl-paste", "--no-newline"]).stdout == text
            except subprocess.CalledProcessError:
                return False
        wait(ready_text, "owned text clipboard is ready")

    def pointer(point, button="left", end=None):
        check(pointer_exe is not None and state().get("windowWidth") == 1280
              and state().get("windowHeight") == 1000, "private client coordinates match the undecorated output")
        argv = [str(pointer_exe), str(int(point[0])), str(int(point[1])), "1280", "1000", button]
        if end is not None:
            argv += [str(int(end[0])), str(int(end[1]))]
        run(argv)

    def shot(name):
        # QA properties can update before their next Wayland frame. This wait
        # is only for visual evidence, outside the input-to-receipt samples.
        time.sleep(.2)
        run(["grim", str(out / f"{name}.png")])

    def ready():
        wait(lambda: state().get("model", {}).get("count") == args.entries and state()["model"].get("settled"), "native listing ready", 45)
        observed = time.monotonic()
        time.sleep(.4)
        return observed

    def settings_attempt(snapshot=None):
        value = (state() if snapshot is None else snapshot).get("settings", {}).get("completed_attempt", "")
        check(str(value).isascii() and str(value).isdecimal(), "settings exposes a decimal completed attempt")
        return int(value)

    def refresh_completed(before, failed):
        current = state()
        value = str(current.get("settings", {}).get("completed_attempt", ""))
        expected_status = current.get("settingsStatusText", "")
        return (value.isascii() and value.isdecimal() and int(value) > before
                and current.get("settingsOpen") and not current.get("prefsPending", True)
                and bool(current.get("prefsError")) == failed
                and (expected_status.startswith("Problem: ") if failed else expected_status == "Saved"))

    def record_refresh(label, before):
        current = state()
        samples.append({"journey": "preferences-refresh", "stage": label,
                        "attempt_before": before, "completed_attempt": settings_attempt(current),
                        "status": current.get("settingsStatusText"), "error": current.get("prefsError"),
                        "settings": current.get("settings")})

    def chooser_sort_journey(app):
        def observe(label):
            current = state()
            snapshot = {"stage": label, "chooser_mode": current.get("chooserMode"),
                        "model": current.get("model", {}), "palette_open": current.get("paletteOpen"),
                        "palette_text": current.get("paletteText"), "palette_results": current.get("paletteResults")}
            if not chooser_states or snapshot != chooser_states[-1]:
                chooser_states.append(snapshot)
                (out / "chooser-sort-states.json").write_text(json.dumps(chooser_states, indent=2))
            return snapshot["model"]

        def settled_after(label, generation):
            current = observe(label)
            # load_current publishes "reading…" before settled=false, so a
            # receipt can briefly pair the new generation with old rows.
            return (current.get("generation", 0) > generation and current.get("settled")
                    and current.get("status") == f"{args.entries} items"
                    and current.get("count") == args.entries
                    and current.get("meta_known") == args.entries + 1)

        def assert_size_order(current, label):
            check(current.get("location") == str(fixture) and current.get("filter") == ""
                  and current.get("count") == args.entries, f"{label}: unchanged chooser location and exact count")
            check(current.get("sort_field") == "size" and current.get("sort_ascending") is True,
                  f"{label}: transient ascending Size sort retained")
            check(current.get("settled") and current.get("meta_known") == args.entries + 1,
                  f"{label}: settled metadata for every fixture entry including hidden file")
            check(current.get("visible") == list(reversed(names))[:40],
                  f"{label}: exact first 40 rows in ascending size order")

        initial = observe("initial-name-order")
        check(state().get("chooserMode") == "open", "native Open chooser is active")
        check(initial.get("sort_field") == "name" and initial.get("sort_ascending") is True
              and initial.get("visible") == names[:40], "chooser begins in exact ascending Name order")
        check(initial.get("meta_known", args.entries + 1) < args.entries + 1,
              "initial Name listing leaves metadata missing and forces Size reload")
        sorts_path = home / ".config/gust/sorts.toml"
        remembered = sorts_path.read_bytes() if sorts_path.exists() else None
        # Only Return while the palette is observably open, with the exact
        # known action first. Never Return on the chooser's file list.
        for _ in range(8):
            run(["wtype", ":"])
            time.sleep(.2)
            if state().get("paletteOpen"):
                break
        wait(lambda: state().get("paletteOpen"), "chooser command palette opened")
        run(["wtype", "Sort by size"])
        wait(lambda: state().get("paletteOpen") and state().get("paletteText") == "Sort by size"
             and state().get("paletteResults", [])[:1] == ["Sort by size"], "exact Sort by size action offered first")
        observe("size-action-offered")
        shot("chooser-sort-palette")
        key("Return")
        wait(lambda: settled_after("size-metadata-reload", initial["generation"]), "Size metadata reload settles in a newer generation", 45)
        sorted_model = observe("size-settled")
        assert_size_order(sorted_model, "Size metadata reload")
        check(not state().get("paletteOpen") and app.poll() is None, "sort action leaves chooser open")
        shot("chooser-sort-size")
        # Size must remain the reload input even though chooser policy never
        # admitted it to shared remembered preferences.
        key("r", "ctrl")
        wait(lambda: settled_after("repeat-reload", sorted_model["generation"]), "repeat reload settles in a newer generation", 45)
        reloaded = observe("repeat-reload-settled")
        assert_size_order(reloaded, "Repeat reload")
        key("End")
        wait(lambda: state().get("model", {}).get("cursor_name") == names[0], "Size order ends at the largest fixture file")
        key("Home")
        wait(lambda: state().get("model", {}).get("cursor_name") == names[-1], "Size order begins at the smallest fixture file")
        shot("chooser-sort-reloaded")
        retained = observe("repeat-reload-retained")
        assert_size_order(retained, "Retained settled generation")
        check(retained.get("generation") == reloaded["generation"], "settled metadata generation stays retained after repeat reload")
        check(state().get("chooserMode") == "open" and not (out / "chooser-result").exists(),
              "no chooser file was accepted")
        key("F4", "alt")
        wait(lambda: app.poll() is not None, "chooser cancels by closing the owned window", 10)
        check(app.returncode == 0, "chooser exits normally without activation")
        check((sorts_path.read_bytes() if sorts_path.exists() else None) == remembered,
              "chooser Size choice never changes remembered sort preferences")
        check(not (out / "chooser-result").exists(), "cancelled chooser produces no accepted file result")

    def filter_to(query):
        run(["wtype", "/"])
        wait(lambda: state().get("filterBarActive"), "filter input opened")
        key("a", "ctrl")
        start = time.monotonic()
        run(["wtype", query])
        expected = [name for name in names if subsequence(query, name)]
        wait(lambda: state().get("model", {}).get("filter") == query and state()["model"]["count"] == len(expected), "matching query/count observed")
        elapsed = (time.monotonic() - start) * 1000
        current = state()["model"]
        check(current["visible"] == expected[:40], f"filter {query}: exact first rows and listing order")
        samples.append({"query": query, "entries": args.entries, "matches": len(expected), "input_to_qa_receipt_ms": elapsed})
        key("Return")
        shot("filter-" + query)
        key("Escape")
        key("Escape")
        wait(lambda: state().get("model", {}).get("filter") == "" and state()["model"]["count"] == args.entries, "filter cleared")

    def session_journey(app):
        nav = [out / "nav-a", out / "nav-b"]
        for p in nav:
            p.mkdir()
        for _ in range(8):
            key("End"); time.sleep(.2)
            if state().get("model", {}).get("cursor_name") == names[-1]:
                break
        wait(lambda: state().get("model", {}).get("cursor_name") == names[-1], "keyboard focus established")
        key("Home")
        key("comma", "ctrl")
        wait(lambda: state().get("settingsOpen") and state().get("prefsReady"), "settings ready")
        run(["wtype", "-k", "Down", "-k", "Down", "-k", "Down", "-k", "Down", "-k", "Right"])
        wait(lambda: state().get("settings", {}).get("restore_tabs") is True and not state().get("prefsPending", True), "restore-tabs enabled and saved")
        key("Escape")
        key("t", "ctrl")
        wait(lambda: state().get("tabCount") == 2, "second tab opened")
        session_path = home / ".local/state/gust/tabs.toml"
        def goto(path):
            key("l", "ctrl")
            wait(lambda: state().get("paletteOpen"), "jump palette open")
            key("a", "ctrl")
            run(["wtype", str(path)])
            key("Return")
            wait(lambda: state().get("model", {}).get("location") == str(path) and state()["model"].get("settled"), "navigation settled")
        sizes = []
        for i in range(args.session_stress):
            stamp = session_path.stat().st_mtime_ns if session_path.exists() else None
            goto(nav[i % 2])
            expected = "local:" + os.fsencode(nav[i % 2]).hex() + " rows"
            wait(lambda: session_path.exists() and session_path.stat().st_mtime_ns != stamp and expected in session_path.read_text() and not state().get("prefsPending", True) and not state().get("prefsError"), "navigation session persisted after debounce")
            text = session_path.read_text()
            check(sum(line.startswith("# Gust: the tabs") for line in text.splitlines()) <= 1,
                  f"navigation {i+1}: canonical header does not accumulate")
            check(sum(line.startswith("tab =") for line in text.splitlines()) == 2, f"navigation {i+1}: both tabs serialized")
            sizes.append(len(text.encode()))
        (out / "session-sizes.json").write_text(json.dumps(sizes))
        check(len(set(sizes[1:])) <= 1, "repeated equal-length locations keep session file size stable")
        shot("session-before-restart")
        key("F4", "alt")
        wait(lambda: app.poll() is not None, "first session window closes", 10)
        check(app.returncode == 0, "session flush exits normally")
        (out / "state.json").unlink(missing_ok=True)
        app = spawn([str(binary)], "gust-restart.log")
        last = nav[(args.session_stress-1) % 2]
        wait(lambda: state().get("tabCount") == 2 and state().get("activeTab") == 1 and state().get("model", {}).get("location") == str(last), "restart restores both tabs and active location", 45)
        check(state().get("tabCount") == 2, "both tabs survived repeated saves and restart")
        shot("session-restored")
        key("comma", "ctrl")
        wait(lambda: state().get("settingsOpen"), "settings reopened after restore")
        run(["wtype", "-k", "Down", "-k", "Right"])
        wait(lambda: state().get("settings", {}).get("density") == "roomy" and not state().get("prefsPending", True) and not state().get("prefsError"), "settings still save after session stress")
        check(state().get("settingsStatusText") == "Saved", "later preference reaches Saved")
        shot("session-later-setting-saved")
        key("Escape"); key("F4", "alt")
        wait(lambda: app.poll() is not None, "restored window closes", 10)
        check(app.returncode == 0, "restored window exits normally")

    def seed_session_tail():
        paths = [out / f"tail-tab-{i:02d}" for i in range(39)] + [fixture]
        for path in paths[:-1]:
            path.mkdir(mode=0o700)
        tokens = ["local:" + os.fsencode(path).hex() + " rows" for path in paths]
        headers = ["# Gust: the tabs that were open, restored at the next start.",
                   "# Keys are hex-encoded OS bytes. Delete this file to start fresh."]
        unknown = [f"# QA preserved tail comment {i:03d}" if i % 2 == 0
                   else f'qa_tail_{i:03d} = "preserve-{i:03d}"' for i in range(160)]
        body = "\n".join(headers + ["active = 39"] + [f'tab = "{token}"' for token in tokens] + unknown) + "\n"
        check(len(body.encode()) < 64 * 1024, "40-tab seed with 160 unknown tail lines fits the 64 KiB limit")
        session_path = home / ".local/state/gust/tabs.toml"
        session_path.parent.mkdir(mode=0o700, exist_ok=True)
        session_path.write_text(body)
        session_path.chmod(0o600)
        settings_path = home / ".config/gust/settings.toml"
        settings_path.write_text("restore_tabs = true\n")
        settings_path.chmod(0o600)
        (out / "session-tail-seed.toml").write_text(body)
        session_tail.update(expected_tokens=tokens, unknown_lines=unknown,
                            seed_bytes=len(body.encode()), seed_mtime_ns=session_path.stat().st_mtime_ns,
                            files=[], windows=[])

    def session_tail_journey(app):
        session_path = home / ".local/state/gust/tabs.toml"

        def window(label):
            current = state()
            session_tail["windows"].append({"stage": label, "tab_count": current.get("tabCount"),
                                            "active_tab": current.get("activeTab"), "tab_paths": current.get("tabPaths"),
                                            "model": current.get("model"), "settings": current.get("settings"),
                                            "prefs_pending": current.get("prefsPending"), "prefs_error": current.get("prefsError")})
            return current

        def restored(current):
            model = current.get("model", {})
            return (current.get("tabCount") == 40 and current.get("activeTab") == 39
                    and model.get("location") == str(fixture) and model.get("count") == 100
                    and model.get("settled"))

        def inspect_file(label):
            body = session_path.read_bytes()
            (out / f"session-tail-{label}.toml").write_bytes(body)
            lines = body.decode().splitlines()
            fields = [(i, *line.split("=", 1)) for i, line in enumerate(lines) if "=" in line and not line.lstrip().startswith("#")]
            tabs = [value.strip().strip('"') for _, key, value in fields if key.strip() == "tab"]
            active = [value.strip() for _, key, value in fields if key.strip() == "active"]
            positions = [i for i, key, _ in fields if key.strip() in ("tab", "active")]
            preserved = [line for line in lines if line in session_tail["unknown_lines"]]
            record = {"stage": label, "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(),
                      "active_values": active, "tokens": tabs, "canonical_line_numbers": [i + 1 for i in positions],
                      "canonical_within_first_160": len(positions) == 41 and max(positions) < 160,
                      "unknown_lines_preserved": preserved == session_tail["unknown_lines"]}
            session_tail["files"].append(record)
            check(active == ["39"] and tabs == session_tail["expected_tokens"], f"{label}: active index and all 40 raw tokens are exact")
            check(record["unknown_lines_preserved"], f"{label}: all 160 unknown comment/field lines retain their order")
            return body, record

        check(restored(window("initial-restored")), "seed restores 40 tabs with fixture active at index 39 without a path argument")
        check(state()["model"].get("visible") == names[:40], "restored active fixture has the exact initial rows")
        # Establish keyboard focus without navigating or activating a file.
        for _ in range(8):
            key("End")
            time.sleep(.2)
            if state().get("model", {}).get("cursor_name") == names[-1]:
                break
        wait(lambda: state().get("model", {}).get("cursor_name") == names[-1], "session-tail keyboard focus established")
        key("Home")
        shot("session-tail-initial")
        key("F4", "alt")
        wait(lambda: app.poll() is not None, "first 40-tab window closes after its admitted session save", 10)
        check(app.returncode == 0, "first session-tail close exits normally")
        check(session_path.stat().st_mtime_ns != session_tail["seed_mtime_ns"], "session save actually rewrites the seeded file")
        first_bytes, first = inspect_file("first-save")
        (out / "state.json").unlink(missing_ok=True)
        app = spawn([str(binary)], "gust-tail-restart.log")
        wait(lambda: state().get("prefsStarted") and state().get("prefsReady")
             and state().get("model", {}).get("settled") and not state().get("prefsPending", True),
             "pathless session-tail restart finishes loading preferences and listing", 45)
        restarted = window("after-first-save-restart")
        shot("session-tail-restarted")
        # Capture the actual restart even when the writer moved canonical
        # records beyond the parser budget, so the failure proves tab loss.
        check(first["canonical_within_first_160"], "first save keeps active and all 40 tab records within the first 160 lines")
        check(restored(restarted), "restart after saved unknown tail restores all 40 tabs and active index 39")
        key("comma", "ctrl")
        wait(lambda: state().get("settingsOpen") and state().get("prefsReady")
             and state().get("settingsStatusText") == "Saved", "session-tail settings open with Saved status")
        run(["wtype", "-k", "Down", "-k", "Right"])
        wait(lambda: state().get("settings", {}).get("density") == "roomy"
             and not state().get("prefsPending", True) and not state().get("prefsError")
             and state().get("settingsStatusText") == "Saved", "later preference after 40-tab restore reaches Saved")
        window("later-preference-saved")
        shot("session-tail-later-setting-saved")
        key("Escape")
        key("F4", "alt")
        wait(lambda: app.poll() is not None, "second same-session window closes normally", 10)
        check(app.returncode == 0, "second session-tail close exits normally")
        second_bytes, second = inspect_file("second-save")
        check(second["canonical_within_first_160"], "second save keeps all canonical records inside the parser budget")
        check(second_bytes == first_bytes, "subsequent same-session close preserves exact session bytes")

    def seed_location_tail():
        # Never visit this folder in the UI. Its raw non-UTF-8 byte must
        # survive serialization while the fixture's preferences change.
        unrelated = out / os.fsdecode(b"unrelated-\xff")
        unrelated.mkdir(mode=0o700)
        tokens = {"fixture": "local:" + os.fsencode(fixture).hex(),
                  "unrelated": "local:" + os.fsencode(unrelated).hex()}
        location_tail.update(tokens=tokens, documents={}, windows=[])
        for document, initial, other, changed in (("sorts", "name-desc", "modified-desc", "kind-asc"),
                                                   ("views", "grid", "grid", "columns")):
            unknown = [f"# QA {document} unknown tail {i:04d}: preserve exactly" for i in range(2000)]
            body = "\n".join([f'{tokens["fixture"]} = "{initial}"',
                              f'{tokens["unrelated"]} = "{other}"'] + unknown) + "\n"
            check(len(body.encode()) < 512 * 1024, f"{document}: seeded records and 2000 tail comments fit the 512 KiB limit")
            path = home / f".config/gust/{document}.toml"
            path.write_text(body)
            path.chmod(0o600)
            (out / f"location-tail-{document}-seed.toml").write_text(body)
            location_tail["documents"][document] = {"initial": initial, "unrelated": other, "changed": changed,
                                                       "unknown_lines": unknown, "seed_mtime_ns": path.stat().st_mtime_ns,
                                                       "seed_bytes": len(body.encode()), "files": []}

    def location_tail_journey(app):
        def revision(current, key):
            value = str(current.get(key, ""))
            return int(value) if value.isascii() and value.isdecimal() else -1

        def observed(label):
            current = state()
            location_tail["windows"].append({"stage": label, "view": current.get("view"), "model": current.get("model"),
                                             "prefs_revision": current.get("prefsRevision"), "prefs_saved_revision": current.get("prefsSavedRevision"),
                                             "prefs_pending": current.get("prefsPending"), "prefs_error": current.get("prefsError")})
            return current

        def palette_action(label):
            for _ in range(8):
                run(["wtype", ":"])
                time.sleep(.2)
                if state().get("paletteOpen"):
                    break
            wait(lambda: state().get("paletteOpen"), f"palette opens for {label}")
            run(["wtype", label])
            wait(lambda: state().get("paletteOpen") and state().get("paletteText") == label
                 and state().get("paletteResults", [])[:1] == [label], f"exact {label} action offered first")
            # Return belongs only to the known action in the open palette.
            key("Return")
            wait(lambda: not state().get("paletteOpen"), f"{label} palette closes")

        def acknowledged(document, before):
            current = state()
            path = home / f".config/gust/{document}.toml"
            info = location_tail["documents"][document]
            expected = f'{location_tail["tokens"]["fixture"]} = "{info["changed"]}"'
            return (not current.get("prefsPending", True) and not current.get("prefsError")
                    and revision(current, "prefsSavedRevision") > before
                    and revision(current, "prefsSavedRevision") >= revision(current, "prefsRevision")
                    and path.stat().st_mtime_ns != info["seed_mtime_ns"] and expected in path.read_text().splitlines())

        def inspect_document(document):
            path = home / f".config/gust/{document}.toml"
            body = path.read_bytes()
            (out / f"location-tail-{document}-saved.toml").write_bytes(body)
            lines = body.decode().splitlines()
            records = [(i, *line.split("=", 1)) for i, line in enumerate(lines) if "=" in line and not line.lstrip().startswith("#")]
            actual = {key.strip(): value.strip().strip('"') for _, key, value in records}
            info = location_tail["documents"][document]
            expected = {location_tail["tokens"]["fixture"]: info["changed"],
                        location_tail["tokens"]["unrelated"]: info["unrelated"]}
            comments = [line for line in lines if line.startswith(f"# QA {document} unknown tail ")]
            record = {"bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(), "records": actual,
                      "record_line_numbers": [i + 1 for i, _, _ in records],
                      "records_within_first_2000": len(records) == 2 and all(i < 2000 for i, _, _ in records),
                      "unknown_lines_preserved": comments == info["unknown_lines"]}
            info["files"].append(record)
            check(len(records) == 2 and actual == expected, f"{document}: changed fixture and unrelated raw-byte-token choice retained exactly")
            check(record["unknown_lines_preserved"], f"{document}: all 2000 unknown comments preserved exactly in order")
            return record

        wait(lambda: state().get("prefsStarted") and state().get("prefsReady")
             and not state().get("prefsPending", True) and state().get("model", {}).get("settled"), "initial location preferences fully loaded")
        initial = observed("initial-saved-choices")
        check(initial["model"].get("sort_field") == "name" and initial["model"].get("sort_ascending") is False
              and initial.get("view") == "grid", "seeded descending Name sort and Grid view are adopted")
        check(initial["model"].get("visible") == list(reversed(names))[:40], "seeded descending Name listing has exact first 40 rows")
        shot("location-tail-initial")
        before_sort = revision(initial, "prefsSavedRevision")
        check(before_sort >= 0, "initial saved preference revision is decimal")
        palette_action("Sort by kind")
        wait(lambda: state().get("model", {}).get("sort_field") == "kind"
             and state().get("model", {}).get("sort_ascending") is True
             and acknowledged("sorts", before_sort), "Kind sort reaches a newer saved revision and rewritten file")
        sorted_state = observed("kind-save-acknowledged")
        palette_action("View: Columns")
        wait(lambda: state().get("view") == "columns" and acknowledged("views", revision(sorted_state, "prefsSavedRevision")),
             "Columns view reaches a newer saved revision and rewritten file")
        changed = observed("both-choices-saved")
        check(changed["model"].get("visible") == names[:40], "Kind sort has exact first 40 rows for equal-extension fixture")
        shot("location-tail-choices-saved")
        documents = {document: inspect_document(document) for document in ("sorts", "views")}
        key("F4", "alt")
        wait(lambda: app.poll() is not None, "location-tail first window closes normally", 10)
        check(app.returncode == 0, "acknowledged sort/view choices permit normal exit")
        (out / "state.json").unlink(missing_ok=True)
        app = spawn([str(binary), str(fixture)], "gust-location-restart.log")
        ready()
        wait(lambda: state().get("prefsStarted") and state().get("prefsReady")
             and not state().get("prefsPending", True), "location-tail restart finishes loading preferences")
        restarted = observed("after-save-restart")
        shot("location-tail-restarted")
        # Observe both documents' actual restart before asserting either
        # parser-budget check; a misplaced record must leave native evidence.
        for document, record in documents.items():
            check(record["records_within_first_2000"], f"{document}: both recognized records stay inside the first 2000 lines")
        check(restarted["model"].get("location") == str(fixture)
              and restarted["model"].get("sort_field") == "kind" and restarted["model"].get("sort_ascending") is True
              and restarted.get("view") == "columns", "restart retains explicit Kind sort and Columns view")
        check(restarted["model"].get("visible") == names[:40], "restarted Kind listing has exact first 40 rows")
        key("F4", "alt")
        wait(lambda: app.poll() is not None, "location-tail restarted window closes normally", 10)
        check(app.returncode == 0, "location-tail restarted process exits normally")

    result = 1
    try:
        bus = spawn(["dbus-daemon", "--session", "--nofork", "--print-address=1", "--nopidfile"], "bus.log", stdout=subprocess.PIPE)
        env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().decode().strip()
        compositor = spawn(["labwc"], "labwc.log")
        wait(lambda: (out / "display").exists() or compositor.poll() is not None, "private compositor started")
        check(compositor.poll() is None, "private compositor stays alive")
        env["WAYLAND_DISPLAY"] = (out / "display").read_text()
        run(["wlr-randr", "--output", "HEADLESS-1", "--custom-mode", "1280x1000"])
        env["XDG_CONFIG_HOME"] = str(home / ".config")
        if args.session_tail_regression:
            seed_session_tail()
        elif args.location_tail_regression:
            seed_location_tail()
        if args.map_monitor:
            observer = args.map_monitor.resolve(strict=True)
            monitor_sha = hashlib.sha256(observer.read_bytes()).hexdigest()
            monitor = spawn([str(observer)], "map-monitor.log", stdout=subprocess.PIPE, text=True)
            def read_monitor():
                for line in monitor.stdout:
                    monitor_lines.append(line.rstrip("\n"))
            monitor_thread = threading.Thread(target=read_monitor, daemon=True)
            monitor_thread.start()
            wait(lambda: "READY" in monitor_lines or monitor.poll() is not None,
                 "private toplevel observer is ready", 10)
            check("READY" in monitor_lines and monitor.poll() is None,
                  "private toplevel observer remains live before app launch")
        launch_at = time.monotonic()
        app = spawn([str(binary)] + ([] if args.session_tail_regression else [str(fixture)]), "gust.log")
        listing_at = ready()
        startup = {"launch_monotonic_s": launch_at,
                   "listing_receipt_monotonic_s": listing_at,
                   "launch_to_listing_receipt_ms": (listing_at - launch_at) * 1000,
                   "scope": "launch to QA settled receipt and optional external toplevel announcement; neither proves useful pixels or presentation"}
        if args.map_monitor:
            def gust_maps():
                return [line for line in monitor_lines if line.startswith("MAP ")
                        and "gust" in line.lower()]
            wait(lambda: bool(gust_maps()), "Gust toplevel announcement is observed", 10)
            mapped_at = float(gust_maps()[0].split()[1])
            check(launch_at <= mapped_at <= time.monotonic(),
                  "toplevel observer uses the same monotonic clock interval")
            startup.update(toplevel_receipt_monotonic_s=mapped_at,
                           launch_to_toplevel_receipt_ms=(mapped_at - launch_at) * 1000,
                           observer_sha256=monitor_sha,
                           observer_unchanged=hashlib.sha256(observer.read_bytes()).hexdigest() == monitor_sha)
            check(startup["observer_unchanged"], "toplevel observer binary stays unchanged")
        if args.pointer_smoke:
            wait(lambda: state().get("nameHeaderPoint") is not None, "native header geometry is available")
            pointer(state()["nameHeaderPoint"])
            wait(lambda: state().get("model", {}).get("sort_field") == "name"
                 and state()["model"].get("sort_ascending") is False
                 and state()["model"].get("visible") == list(reversed(names))[:40],
                 "private pointer click changes the Name sort")
            check(state()["model"]["visible"] == list(reversed(names))[:40], "private pointer targets the reported header geometry")
            shot("pointer-smoke")
            key("F4", "alt")
            wait(lambda: app.poll() is not None, "pointer smoke closes normally", 10)
            check(app.returncode == 0, "pointer smoke exits normally")
        elif args.projection:
            from types import SimpleNamespace
            from qa_projection import run_handoff, run_journey, run_search_jobs, run_search_operations, run_scalar_sorts, run_size_cache
            journey = (run_handoff if args.projection_handoff else
                       run_search_jobs if args.projection_search_jobs else
                       run_search_operations if args.projection_search_operations else
                       run_size_cache if args.projection_size_cache else
                       run_scalar_sorts if args.projection_scalar_sorts else run_journey)
            journey(SimpleNamespace(run=run, key=key, state=state, wait=wait,
                check=check, shot=shot, pointer=pointer, out=out, fixture=fixture, names=names,
                subsequence=subsequence, samples=samples, soak_cycles=args.projection_soak_cycles,
                offer_text=offer_text, handoff_kind=args.projection_handoff,
                soak_only=args.projection_soak_only,
                native_cpu_control=args.projection_native_cpu_control), app)
        elif args.session_tail_regression:
            session_tail_journey(app)
        elif args.location_tail_regression:
            location_tail_journey(app)
        elif args.chooser_sort:
            chooser_sort_journey(app)
        elif args.session_stress:
            session_journey(app)
        elif args.capture_only:
            check(state()["model"]["visible"] == names[:40], "exact initial rows in listing order")
            shot("window")
            if args.theme:
                from PIL import Image
                background = tomllib.loads((out / "theme/current/theme/colors.toml").read_text())["background"]
                rgb = tuple(bytes.fromhex(background.lstrip("#")))
                image = Image.open(out / "window.png").convert("RGB")
                pixels = sum(count for count, color in image.getcolors(image.width * image.height) if color == rgb)
                check(pixels >= image.width * image.height // 20, "theme background covers at least 5 percent of window")
            if args.preferences:
                for _ in range(4):
                    if state().get("settingsOpen"):
                        break
                    key("comma", "ctrl")
                    time.sleep(.2)
                wait(lambda: state().get("settingsOpen") and state().get("prefsReady"), "settings ready")
                wait(lambda: state().get("settingsStatusText") == "Saved", "acknowledged save status is displayed")
                time.sleep(.2)
                shot("settings")
        else:
            # A new virtual keyboard can precede the first focused Wayland frame.
            # Establish focus observably rather than counting that first key as input.
            for _ in range(8):
                key("End")
                time.sleep(.2)
                if state().get("model", {}).get("cursor_name") == names[-1]:
                    break
            wait(lambda: state().get("model", {}).get("cursor_name") == names[-1], "native keyboard reaches final row")
            key("Home")
            check(state()["model"]["count"] == args.entries, "hidden entry excluded by default")
            for query in ("file", "zzz", "19"):
                filter_to(query)
            key("Home")
            key("space", "ctrl")
            wait(lambda: state().get("selCount") == 1, "one byte-name target selected")
            filter_to("zzz")
            check(state().get("selCount") == 0, "filter-hidden target does not stay selected")
            if args.preferences:
                key("comma", "ctrl")
                wait(lambda: state().get("settingsOpen") and state().get("prefsReady"), "settings ready")
                # Density: normal -> roomy; Hidden files: hide -> show.
                run(["wtype", "-k", "Down", "-k", "Right", "-k", "Down", "-k", "Right"])
                wait(lambda: state().get("settings", {}).get("density") == "roomy" and state()["settings"].get("show_hidden") is True, "both rapid changes visible")
                wait(lambda: not state().get("prefsPending", True) and not state().get("prefsError"), "both rapid changes saved")
                settings_path = home / ".config/gust/settings.toml"
                saved = tomllib.loads(settings_path.read_text())
                check(saved["density"] == "roomy" and saved["show_hidden"], "two rapid settings changes both persisted")
                shot("preferences-saved")
                config_dir = home / ".config/gust"
                config_dir.chmod(0o500)
                try:
                    key("Up")
                    key("Left")
                    wait(lambda: bool(state().get("prefsError")), "save refusal surfaced")
                    key("Down")
                    check(bool(state().get("prefsError")), "save error survives unrelated key")
                    shot("preferences-error")
                    key("F4", "alt")
                    wait(lambda: state().get("confirmOpen") and state().get("confirmTitle") == "Settings not saved", "native unsaved-close confirmation", 8)
                    check(app.poll() is None, "unsaved preferences keep the process alive")
                    shot("preferences-close-error")
                    key("Escape")
                    wait(lambda: not state().get("confirmOpen") and not state().get("prefsClosing"), "native keep-window closes only the confirmation")
                    check(app.poll() is None and state().get("settingsOpen"), "keep-window preserves the settings panel")
                finally:
                    config_dir.chmod(0o700)
                run(["wtype", "r"])
                wait(lambda: not state().get("prefsPending", True) and not state().get("prefsError"), "explicit retry saved changes")
                check("Settings not saved" not in state().get("hintShown", ""), "recovery clears the stale unsaved footer")
                shot("preferences-recovered")
                preserved = {k: state()["settings"][k] for k in ("key_preset", "density", "show_hidden", "sidebar", "restore_tabs", "renderer")}
                read_attempt = settings_attempt()
                settings_path.chmod(0o000)
                try:
                    key("Escape")
                    wait(lambda: not state().get("settingsOpen"), "settings closed before failed refresh")
                    key("comma", "ctrl")
                    wait(lambda: refresh_completed(read_attempt, True), "new unreadable-settings refresh completes with Problem status")
                    record_refresh("read-error", read_attempt)
                    check({k: state()["settings"][k] for k in preserved} == preserved,
                          "failed read preserves last-known settings rather than publishing defaults")
                    shot("preferences-read-error")
                finally:
                    settings_path.chmod(0o600)
                recovery_attempt = settings_attempt()
                key("Escape")
                wait(lambda: not state().get("settingsOpen"), "settings closed before recovery refresh")
                key("comma", "ctrl")
                wait(lambda: refresh_completed(recovery_attempt, False), "new read-recovery refresh completes with Saved status")
                record_refresh("read-recovered", recovery_attempt)
                check({k: state()["settings"][k] for k in preserved} == preserved,
                      "recovered read keeps the same settings choices")
                shot("preferences-read-recovered")
                key("Escape")
                key("F4", "alt")
                wait(lambda: app.poll() is not None, "window closes without forced termination", 10)
                check(app.returncode == 0, "preferences flush permits normal exit")
                (out / "state.json").unlink(missing_ok=True)
                app = spawn([str(binary), str(fixture)], "gust-restart.log")
                wait(lambda: state().get("model", {}).get("count") == args.entries + 1 and state()["model"].get("settled"), "restart adopts saved hidden-file preference", 45)
                key("comma", "ctrl")
                wait(lambda: state().get("settingsOpen"), "settings reopened")
                check(state()["settings"]["density"] == "normal" and state()["settings"]["show_hidden"], "restart retains recovered preferences")
                shot("preferences-restart")
                key("Escape")
                key("F4", "alt")
                wait(lambda: app.poll() is not None, "restarted window closes", 10)
                check(app.returncode == 0, "restarted window exits normally")
        if args.publication_probe:
            events = state().get("model", {}).get("projection", {}).get("callbacks", {}).get("publication_events", {})
            check(events.get("enabled") is True, "publication attribution is enabled in the diagnostic artifact")
            check(type(events.get("recorder_id")) is int and events["recorder_id"] > 0,
                  "publication recorder has an opaque process-local id")
            for name in ("worst_end", "worst_total"):
                rows = events.get(name)
                check(isinstance(rows, list) and len(rows) <= 8,
                      name + " stays within the fixed eight-record bound")
                for row in rows:
                    times = [row.get(key) for key in ("total_ns", "end_ns", "end_data_ns")]
                    check(all(type(value) is int and value >= 0 for value in times)
                          and times[2] <= times[1] <= times[0],
                          name + ": correlated durations use coherent nanoseconds")
            (out / "publication-probe.json").write_text(json.dumps(events, indent=2))
        result = 0
    except (AssertionError, subprocess.SubprocessError) as error:
        checks.append({"check": str(error), "passed": False})
        print("FAIL: " + str(error), flush=True)
        try:
            shot("failure")
        except subprocess.SubprocessError:
            pass
    finally:
        for p in reversed(children):
            if p.poll() is None:
                try:
                    os.killpg(p.pid, signal.SIGTERM)
                    p.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait(timeout=3)
                except ProcessLookupError:
                    pass
        for log in logs:
            log.close()
        if monitor_thread is not None:
            monitor_thread.join(timeout=1)
            (out / "map-monitor-events.log").write_text("\n".join(monitor_lines) + "\n")
        warnings, qt_diagnostics = [], []
        for log in out.glob("gust*.log"):
            for line in log.read_text(errors="replace").splitlines():
                if line.startswith(("qt.", "DelegateModel::")):
                    qt_diagnostics.append({"log": log.name, "line": line})
                if any(marker in line for marker in ("ReferenceError:", "TypeError:", "Binding loop", "Unable to assign", "Cannot assign", "is not a type")):
                    warnings.append(line)
        (out / "qml-warnings.txt").write_text("\n".join(warnings) + ("\n" if warnings else ""))
        if warnings:
            checks.append({"check": "no QML runtime/binding warnings", "passed": False})
            result = 1
        unchanged = runner_sha == hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        if args.projection:
            unchanged = unchanged and projection_sha == hashlib.sha256(projection_source.read_bytes()).hexdigest()
        if not unchanged:
            checks.append({"check": "QA runner sources unchanged during the journey", "passed": False})
            result = 1
        receipt = {"runner_sha256": runner_sha, "runner_source_unchanged": unchanged, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "entries": args.entries, "projection_journey": args.projection, "projection_soak_cycles": args.projection_soak_cycles, "pointer_smoke": args.pointer_smoke, "pointer": pointer_receipt, "projection_runner_sha256": projection_sha, "session_stress": args.session_stress, "session_tail_regression": args.session_tail_regression, "session_tail": session_tail, "location_tail_regression": args.location_tail_regression, "location_tail": location_tail, "chooser_sort": args.chooser_sort, "chooser_states": chooser_states, "renderer": args.renderer, "render_node": str(args.render_node) if args.render_node else None,
                   "vulkan_icd": str(args.vulkan_icd) if args.vulkan_icd else None, "theme": args.theme.name if args.theme else None,
                   "checks": checks, "samples": samples, "result": result, "qml_warning_count": len(warnings),
                   "qt_diagnostics": qt_diagnostics,
                   "projection_handoff": args.projection_handoff,
                   "projection_search_jobs": args.projection_search_jobs,
                   "projection_search_operations": args.projection_search_operations,
                   "projection_scalar_sorts": args.projection_scalar_sorts,
                   "projection_size_cache": args.projection_size_cache,
                   "projection_soak_only": args.projection_soak_only,
                   "projection_native_cpu_control": args.projection_native_cpu_control,
                   "publication_probe_requested": args.publication_probe,
                   "startup": startup,
                   "input_delivery": "Each wtype connection waits 50 ms before input so the private compositor can admit its virtual keyboard after pointer hotplug; receipt timings include this aid.",
                   "scope": "Native Rust/CXX/QML in a private Wayland session with the recorded renderer environment; input-to-QA-receipt timings include observer overhead and are not presented-frame latency. No host desktop, accounts or real files driven."}
        (out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    return result


def subsequence(needle, name):
    remaining = iter(name.lower())
    return all(any(c == wanted for c in remaining) for wanted in needle.lower())


if __name__ == "__main__":
    raise SystemExit(main())
