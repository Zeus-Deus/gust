"""Native projection journey used by qa-optimization's owned Wayland lab.

All observations are GUI state receipts, never presented-frame timestamps.
The caller supplies the private fixture, input and process ownership helpers.
"""
import json
import hashlib
import os
import time
from pathlib import Path
from urllib.parse import quote_from_bytes


CPU_COUNTERS = ("on_cpu_ns", "runqueue_wait_ns", "timeslices", "minor_faults",
                "major_faults", "voluntary_context_switches", "involuntary_context_switches")
CPU_SLOTS = ("running", "pending", "completion", "candidate", "retiring")


def parse_thread_stat(text):
    # comm can contain spaces, newlines and closing parentheses. Fields after
    # its final ')' have fixed kernel positions (state is field 3).
    prefix, close, suffix = text.rpartition(")")
    pid_text, opening, _ = prefix.partition(" (")
    fields = suffix.split()
    assert close and opening and len(fields) >= 20, "incomplete thread stat"
    pid = int(pid_text)
    values = {"tid": pid, "minor_faults": int(fields[7]),
              "major_faults": int(fields[9]), "starttime_ticks": int(fields[19])}
    assert pid > 0 and fields[0] not in ("Z", "X", "x"), "thread is not live"
    assert all(value >= 0 for value in values.values()), "negative thread stat"
    return values


def parse_thread_counters(schedstat, stat, status, pid):
    values = parse_thread_stat(stat)
    sched = schedstat.split()
    assert len(sched) == 3, "schedstat accounting unsupported/incomplete"
    values.update(zip(("on_cpu_ns", "runqueue_wait_ns", "timeslices"), map(int, sched)))
    fields = {}
    for line in status.splitlines():
        key, sep, value = line.partition(":")
        if sep and key in ("Pid", "Tgid", "voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"):
            assert key not in fields, "duplicate status field"
            fields[key] = int(value.strip())
    assert values["tid"] == fields["Pid"] == fields["Tgid"] == pid, "GUI tid must equal app pid"
    values["voluntary_context_switches"] = fields["voluntary_ctxt_switches"]
    values["involuntary_context_switches"] = fields["nonvoluntary_ctxt_switches"]
    assert all(values[key] >= 0 for key in CPU_COUNTERS), "negative thread counter"
    assert values["on_cpu_ns"] > 0 and values["timeslices"] > 0, "schedstat accounting unavailable"
    return values


def thread_counter_delta(before, after):
    assert before["identity"] == after["identity"], "GUI thread identity changed"
    assert before["sample_end_ns"] <= after["sample_begin_ns"] <= after["sample_end_ns"], "nonmonotonic counter bounds"
    delta = {key: after[key] - before[key] for key in CPU_COUNTERS}
    assert all(value >= 0 for value in delta.values()), "nonmonotonic thread counter"
    return {**delta,
            "wall_lower_ns": after["sample_begin_ns"] - before["sample_end_ns"],
            "wall_upper_ns": after["sample_end_ns"] - before["sample_begin_ns"]}


class ThreadCounterObserver:
    """External procfs only; no GUI-thread clock or runtime change."""
    def __init__(self, pid):
        self.pid = pid
        self.boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        self.previous = None

    def sample(self):
        begin = time.monotonic_ns()
        root = Path(f"/proc/{self.pid}/task/{self.pid}")
        try:
            stat = (root / "stat").read_text()
            value = parse_thread_counters((root / "schedstat").read_text(), stat,
                                          (root / "status").read_text(), self.pid)
            last = parse_thread_stat((root / "stat").read_text())
            assert (last["tid"], last["starttime_ticks"]) == (value["tid"], value["starttime_ticks"]), "thread replaced during sample"
            for key in ("minor_faults", "major_faults"):
                assert last[key] >= value[key], "thread faults decreased during sample"
        except (OSError, ValueError, KeyError) as error:
            raise AssertionError(f"GUI thread counters unavailable: {error}") from error
        value.update(identity={"pid": self.pid, "tid": self.pid,
                               "starttime_ticks": value["starttime_ticks"], "boot_id": self.boot_id},
                     sample_begin_ns=begin, sample_end_ns=time.monotonic_ns())
        if self.previous is not None:
            thread_counter_delta(self.previous, value)
        self.previous = value
        return value


def cpu_host_context():
    return {"monotonic_ns": time.monotonic_ns(), "loadavg": list(os.getloadavg()),
            "pressure": {kind: Path("/proc/pressure", kind).read_text()
                         for kind in ("cpu", "memory", "io")},
            "observer_affinity": sorted(os.sched_getaffinity(0))}


def run_native_cpu_control(api, app, observer, observe):
    """Predeclared 1 warmup + 10 measured cycles, four one-request steps."""
    path = api.out / "native-cpu-control.json"
    evidence = {"warmup_cycles": 1, "measured_cycles": 10, "steps": [], "cycles": [],
                "scope": "external GUI-thread windows include input, software rendering and QA dumps; OFF has no per-event duration; no Qt CPU attribution"}

    def save():
        path.write_text(json.dumps(evidence, indent=2))

    def endpoint(current, query, revision=None):
        model = current.get("model", {})
        p = model.get("projection", {})
        return (bool(p) and current.get("view") == "rows"
                and current.get("filterBarActive") is True
                and model.get("filter") == query and p.get("shown_filter") == query
                and all(p.get(key) is False for key in ("sort_pending", "filter_pending", "selection_pending"))
                and all(p.get("slots", {}).get(key) is False for key in CPU_SLOTS)
                and p.get("shown_request_revision") == p.get("request_revision")
                and p.get("shown_selection_revision") == p.get("selection_revision")
                and (revision is None or p.get("request_revision") == revision))

    def stable(query, revision=None):
        since = None
        signature = None
        def poll():
            nonlocal since, signature
            current = api.state()
            p = current.get("model", {}).get("projection", {})
            value = (p.get("request_revision"), p.get("selection_revision"),
                     p.get("display_revision"), json.dumps(p.get("publications"), sort_keys=True))
            if not endpoint(current, query, revision):
                since, signature = None, None
                return False
            if value != signature or since is None:
                signature, since = value, time.monotonic_ns()
            return time.monotonic_ns() - since >= 200_000_000
        api.wait(poll, "CPU control coherent endpoint stable for 200 ms: " + query, 45)
        current = api.state()
        api.check(endpoint(current, query, revision), "CPU control endpoint remains coherent")
        return current

    initial = api.state()
    model = initial["model"]
    api.check(initial.get("view") == "rows" and model.get("sort_field") == "name"
              and model.get("sort_ascending") is False, "CPU control begins in descending Name Rows")
    expected = {query: [name for name in reversed(api.names) if api.subsequence(query, name)]
                for query in ("1", "z", "")}
    api.check(0 < len(expected["1"]) < len(api.names) and not expected["z"]
              and expected[""] == list(reversed(api.names)), "full fixture oracle proves partial 1, zero z and full clear")
    # Return in the old soak leaves the bar visible but returns keyboard focus
    # to the pane. '/' refocuses that same editor; no intermediate clear request.
    api.run(["wtype", "/"])
    api.wait(lambda: api.state().get("filterBarActive"), "CPU control refocuses existing filter editor")
    stable("")

    def oracle(current, query):
        m = current["model"]
        api.check(m.get("count") == len(expected[query]) and m.get("visible") == expected[query][:40]
                  and current.get("selCount") == 0 and m.get("marked") == 0,
                  "CPU control exact fixture count/first40 and zero selection/marks: " + query)

    for cycle in range(11):
        boundary = {"cycle": cycle, "warmup": cycle == 0, "before": cpu_host_context()}
        evidence["cycles"].append(boundary)
        current = stable("")
        oracle(current, "")
        for step, (kind, query) in enumerate((("partial1", "1"), ("clearafter1", ""),
                                              ("emptyz", "z"), ("clearafterz", ""))):
            # Ctrl+A and replacement go to the same focused editor. Sampling
            # includes Ctrl+A and the single character/BackSpace subprocesses.
            before_state = api.state()
            before_p = before_state["model"]["projection"]
            row = {"cycle": cycle, "warmup": cycle == 0, "step": step, "kind": kind,
                   "query": query, "before_state": before_state, "before": observer.sample()}
            evidence["steps"].append(row)
            save()
            try:
                api.key("a", "ctrl")
                if query:
                    api.run(["wtype", query])
                else:
                    api.key("BackSpace")
                current = stable(query, before_p["request_revision"] + 1)
                row["after"] = observer.sample()
                row["after_state"] = current
                row["delta"] = thread_counter_delta(row["before"], row["after"])
                p = current["model"]["projection"]
                before_pub, after_pub = before_p["publications"], p["publications"]
                row["publication_delta"] = {key: after_pub[key] - before_pub[key] for key in before_pub}
                row["end_count_delta"] = (p["callbacks"]["publish_phases"]["end"]["count"]
                                           - before_p["callbacks"]["publish_phases"]["end"]["count"])
                save()
                api.check(row["end_count_delta"] == 1 and row["publication_delta"] ==
                          {"resets": 1, "layouts": 0, "selection": 0, "marks": 0},
                          "CPU control exactly one end/reset, zero layouts/selection/marks")
                api.check(p["selection_revision"] == before_p["selection_revision"],
                          "CPU control selection revision unchanged")
                oracle(current, query)
                row["passed"] = True
            except Exception as error:
                row["error"] = str(error)
                row["failure_state"] = api.state()
                save()
                raise
            save()
        boundary["after"] = cpu_host_context()
        save()
    api.check(sum(not row["warmup"] for row in evidence["steps"]) == 40
              and all(row.get("passed") for row in evidence["steps"]),
              "CPU control all 40 measured steps and four warmup steps validated")
    observe("cpu-control-complete")
    api.shot("projection-cpu-control-complete")


def palette_command(api, text, label, after=()):
    for _ in range(8):
        api.run(["wtype", ":"])
        time.sleep(.1)
        if api.state().get("paletteOpen"):
            break
    api.wait(lambda: api.state().get("paletteOpen"), "command palette opens")
    api.run(["wtype", text])
    api.wait(lambda: api.state().get("paletteResults", [])[:1] == [label],
             "exact command is offered: " + label)
    if after:
        api.run(["wtype", "-k", "Return", *after])
    else:
        api.key("Return")
    api.wait(lambda: not api.state().get("paletteOpen"), "command palette closes")


def resources(pid):
    """Observer snapshot, including currently live descendants; not a peak."""
    import psutil
    root = psutil.Process(pid)
    rows, errors, exited = [], [], []
    for process in [root, *root.children(recursive=True)]:
        try:
            if process.pid != pid and process.status() == psutil.STATUS_ZOMBIE:
                exited.append({"pid": process.pid, "reason": "exited child (zombie), no address space"})
                continue
            fields = {}
            for line in Path(f"/proc/{process.pid}/smaps_rollup").read_text().splitlines():
                key, _, value = line.partition(":")
                if value.strip().endswith(" kB"):
                    fields[key] = int(value.split()[0])
            cpu = process.cpu_times()
            rows.append({"pid": process.pid, "created": process.create_time(),
                         "pss_kib": fields["Pss"],
                         "uss_kib": fields.get("Private_Clean", 0) + fields.get("Private_Dirty", 0),
                         "rss_kib": fields["Rss"], "threads": process.num_threads(),
                         "cpu_s": cpu.user + cpu.system})
        except (OSError, psutil.Error) as error:
            if process.pid != pid and (isinstance(error, psutil.NoSuchProcess)
                                      or isinstance(error, OSError) and error.errno in (2, 3)):
                exited.append({"pid": process.pid, "reason": "child exited during snapshot"})
            else:
                errors.append(str(error))
    pressure = {}
    for kind in ("cpu", "memory"):
        try:
            pressure[kind] = Path("/proc/pressure/" + kind).read_text()
        except OSError as error:
            pressure[kind] = "unavailable: " + str(error)
    return {"processes": rows, "errors": errors, "excluded_exited_children": exited,
            "observer_host_pressure": pressure,
            "observer_host_loadavg": list(os.getloadavg()),
            "observer_affinity": sorted(os.sched_getaffinity(0)),
            **{key: sum(row[key] for row in rows) for key in
               ("pss_kib", "uss_kib", "rss_kib", "threads", "cpu_s")}}


def context_menu_journey(api, palette_action, idle, observe):
    """One actual right click must select its row and open its own menu."""
    target = api.names[min(2, len(api.names) - 1)]
    for label, mode in (("View: Rows", "rows"), ("View: Grid", "grid"), ("View: Columns", "columns")):
        palette_action(label)
        api.wait(lambda: api.state().get("view") == mode and idle(), "context test view: " + mode)
        api.key("Home")
        api.wait(lambda: api.state().get("selCount") == 0
                 and target in api.state().get("rowPoints", {}), "unselected context target is visible")
        api.pointer(api.state()["rowPoints"][target], button="right")
        # No retry: a second click would mask the regression being tested.
        api.wait(lambda: api.state().get("menuOpen") and idle()
                 and api.state().get("selCount") == 1
                 and api.state()["model"].get("cursor_name") == target,
                 "first right click opens its selected row menu in " + mode)
        api.check(api.state().get("menuItems", [""])[0] == "Open",
                  "context actions reflect one coherent selected target in " + mode)
        observe("first-context-" + mode)
        api.shot("projection-first-context-" + mode)
        api.key("Escape")
        api.wait(lambda: not api.state().get("menuOpen"), "context menu closes")
        api.key("Escape")
        api.wait(lambda: idle() and api.state().get("selCount") == 0, "context selection clears")
    palette_action("View: Rows")
    api.key("Home")
    api.wait(lambda: idle() and api.state()["model"].get("cursor_name") == api.names[0],
             "context journey restores the initial activation target")


def clipboard_marks_journey(api, idle, observe):
    publications = api.state()["model"]["projection"].get("publications", {})
    api.check({"resets", "layouts", "selection", "marks"} <= publications.keys(),
              "structural and auxiliary publication counters are available")
    def structural():
        counts = api.state()["model"]["projection"]["publications"]
        return counts["resets"], counts["layouts"]

    api.key("space", "ctrl")
    api.wait(lambda: idle() and api.state().get("selCount") == 1, "clipboard target selection applies")
    before = structural()
    marks = api.state()["model"]["projection"]["publications"]["marks"]
    api.key("c", "ctrl")
    api.wait(lambda: idle() and api.state()["model"].get("marked") == 1
             and api.state()["model"]["projection"]["publications"]["marks"] > marks,
             "clipboard marks publish through their auxiliary barrier")
    api.check(structural() == before, "copying a target causes no model reset or layout")
    observe("clipboard-marked")
    api.key("Escape")
    api.wait(lambda: idle() and api.state().get("selCount") == 0, "clipboard target selection clears")
    api.key("Escape")
    api.wait(lambda: idle() and api.state()["model"].get("marked") == 0, "clipboard mark clear publishes")
    api.check(structural() == before, "clipboard clear and selection gestures reuse the displayed row structure")
    observe("clipboard-cleared")


def seed_cached_png(source, cache_home, number, preserve_mtime_ns=None):
    """Owned deterministic image and full-version cache, not helper rendering."""
    from PIL import Image, ImageDraw, PngImagePlugin
    image = Image.new("RGB", (160, 120), ((number * 61) % 256, (number * 29) % 256, (number * 97) % 256))
    draw = ImageDraw.Draw(image)
    draw.rectangle((10, 10, 80, 70), fill="white")
    draw.text((16, 24), f"thumb {number}", fill="black")
    # Stored deflate blocks keep source length identical when pixels change.
    image.save(source, compress_level=0)
    if preserve_mtime_ns is not None:
        os.utime(source, ns=(source.stat().st_atime_ns, preserve_mtime_ns))
    stat = source.stat()
    version = ":".join(str(v) for v in (stat.st_dev, stat.st_ino, stat.st_size,
        stat.st_mtime_ns // 1_000_000_000, stat.st_mtime_ns % 1_000_000_000,
        stat.st_ctime_ns // 1_000_000_000, stat.st_ctime_ns % 1_000_000_000))
    uri = "file://" + quote_from_bytes(os.fsencode(source), safe="/-._~")
    digest = lambda value: hashlib.md5(value.encode(), usedforsecurity=False).hexdigest()
    cache = cache_home / "thumbnails/normal" / (digest(uri) + ".png")
    cache.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    metadata = PngImagePlugin.PngInfo()
    metadata.add_text("Thumb::MTime", str(stat.st_mtime_ns // 1_000_000_000))
    metadata.add_text("Gust::SourceVersion", version)
    metadata.add_text("Thumb::URI", uri)
    thumb = image.copy()
    thumb.thumbnail((128, 128))
    thumb.save(cache, pnginfo=metadata, compress_level=1)
    immutable = cache_home / "thumbnails/gust-grid/normal" / (digest(uri) + "-" + digest(version) + ".png")
    return {"source": str(source), "cache": str(cache), "version": version,
            "url": "file://" + quote_from_bytes(os.fsencode(immutable), safe="/-._~") + "?gust=" + version,
            "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}


def grid_live_journey(api, directory, palette_action, idle, observe):
    assert directory.is_relative_to(api.out)
    model = api.state()["model"]
    api.check(isinstance(model.get("thumbs"), int)
              and isinstance(model.get("visible_thumbnails"), dict)
              and isinstance(model["projection"].get("validated_thumbnail_count"), int),
              "bounded thumbnail publication diagnostics are available")
    cache_home = api.out / "home/.cache"
    seeds = [seed_cached_png(directory / f"zz-thumb-{i:02d}.png", cache_home, i + 1) for i in range(8)]
    expected = {Path(seed["source"]).name: seed["url"] for seed in seeds}
    count = 2 + len(seeds)  # owned.txt plus the right-drag target folder.
    api.wait(lambda: idle() and api.state()["model"].get("count") == count, "owned image fixture appears", 45)
    palette_action("View: Grid")

    def urls_match():
        urls = api.state()["model"].get("visible_thumbnails", {})
        return all(urls.get(name) == url for name, url in expected.items())

    api.wait(lambda: idle() and urls_match(), "full-version cached images reach the visible grid", 45)
    observe("grid-live-before")
    api.shot("projection-grid-live-before")
    for change in range(6):
        listing = api.state()["model"]["projection"]["listing_revision"]
        (directory / f"grid-live-{change}.txt").write_text("owned live sibling\n")
        count += 1
        api.wait(lambda: idle() and api.state()["model"].get("count") == count
                 and api.state()["model"]["projection"]["listing_revision"] > listing,
                 "grid live replacement publishes", 45)
        api.check(urls_match() and api.state()["model"]["projection"]["validated_thumbnail_count"] >= len(expected),
                  "visible thumbnail URLs were validated before live publication")
        observe("grid-live-" + str(change))
        api.shot("projection-grid-live-" + str(change))

    changed = Path(seeds[0]["source"])
    before = changed.stat()
    listing = api.state()["model"]["projection"]["listing_revision"]
    replacement = seed_cached_png(changed, cache_home, 77, before.st_mtime_ns)
    after = changed.stat()
    api.check((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
              == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
              and before.st_ctime_ns != after.st_ctime_ns,
              "image edit preserves listing size/mtime/inode while changing the full version")
    api.check(replacement["url"] != expected[changed.name], "full source version changes thumbnail identity")
    expected[changed.name] = replacement["url"]
    api.wait(lambda: idle() and urls_match()
             and api.state()["model"]["projection"]["listing_revision"] > listing,
             "live image edit uses its freshly validated version", 45)
    observe("grid-live-version-change")
    api.shot("projection-grid-live-version-change")

    # Find disables the directory watch. Revalidating this viewport therefore
    # renews a cache in the SAME backing, rather than hiding reinsertion bugs
    # behind a new listing. Only preseeded caches are used, as above.
    palette_command(api, "find " + changed.name,
                    f'Find "{changed.name}" in this folder and below')
    api.wait(lambda: idle() and api.state().get("finding")
             and api.state()["model"].get("visible") == [changed.name]
             and api.state()["model"].get("visible_thumbnails", {}).get(changed.name) == replacement["url"],
             "Find grid starts with one cached source version", 45)
    listing = api.state()["model"]["projection"]["listing_revision"]
    renewal = seed_cached_png(changed, cache_home, 78, after.st_mtime_ns)
    palette_action("View: Rows")
    palette_action("View: Grid")
    api.wait(lambda: idle() and api.state().get("finding")
             and api.state()["model"]["projection"]["listing_revision"] == listing
             and api.state()["model"].get("visible_thumbnails", {}).get(changed.name) == renewal["url"],
             "thumbnail cache renews its URL within the same backing", 45)
    observe("grid-cache-renewed")
    api.shot("projection-grid-cache-renewed")

    exhausted = seed_cached_png(changed, cache_home, 79, after.st_mtime_ns)
    palette_action("View: Rows")
    palette_action("View: Grid")
    api.wait(lambda: idle() and api.state().get("finding")
             and api.state()["model"]["projection"]["listing_revision"] == listing
             and changed.name not in api.state()["model"].get("visible_thumbnails", {})
             and api.state()["model"].get("thumbs") == 0,
             "third cache version invalidates stale pixels and defers refresh during Find", 45)
    observe("grid-cache-exhausted")
    api.key("Escape")
    expected[changed.name] = exhausted["url"]
    api.wait(lambda: idle() and not api.state().get("finding") and urls_match()
             and api.state()["model"]["projection"]["listing_revision"] > listing,
             "leaving Find refreshes the exhausted cache on a new backing", 45)
    observe("grid-cache-refreshed")
    api.shot("projection-grid-cache-refreshed")
    (api.out / "grid-thumbnail-fixture.json").write_text(json.dumps({"initial": seeds,
        "replacement": replacement, "same_backing_renewal": renewal, "exhausted_version": exhausted,
        "scope": "owned preseeded full-version PNG caches; cache validation/publication/renewal, not sandboxed helper rendering"}, indent=2))


def right_drag_journey(api, directory, palette_action, idle, observe):
    """Use owned files to verify cancellation and Copy/Undo release the hold."""
    source = directory / "owned.txt"
    target = directory / "drop-target"
    target.mkdir(mode=0o700)
    original = source.read_bytes()
    palette_action("View: Rows")
    api.wait(lambda: idle() and api.state()["model"].get("count") == 2
             and {"owned.txt", "drop-target"} <= api.state().get("rowPoints", {}).keys(),
             "right-drag source and folder are visible", 45)

    def prove_released(label):
        for _ in range(8):
            api.run(["wtype", "/"])
            time.sleep(.1)
            if api.state().get("filterBarActive"):
                break
        api.wait(lambda: api.state().get("filterBarActive"), "post-drag filter editor opens")
        api.key("a", "ctrl")
        api.run(["wtype", "owned"])
        api.wait(lambda: idle() and api.state()["model"].get("visible") == ["owned.txt"]
                 and api.state()["model"]["projection"].get("shown_filter") == "owned",
                 "row publication resumes after right-drag " + label, 45)
        api.check(api.state()["model"]["count"] == 1,
                  "released right-drag hold permits a real filtered replacement: " + label)
        observe("right-drag-" + label)
        api.shot("projection-right-drag-" + label)
        api.key("Return")
        api.key("Escape")
        api.key("Escape")
        api.wait(lambda: idle() and api.state()["model"].get("count") == 2
                 and api.state()["model"]["projection"].get("shown_filter") == "",
                 "post-drag filter clear publishes", 45)

    for action in ("escape", "cancel", "copy"):
        api.wait(lambda: {"owned.txt", "drop-target"} <= api.state().get("rowPoints", {}).keys(),
                 "right-drag geometry available")
        points = api.state()["rowPoints"]
        api.pointer(points["owned.txt"], button="right", end=points["drop-target"])
        api.wait(lambda: api.state().get("menuOpen")
                 and "Copy here" in api.state().get("menuItems", []), "right drag opens its drop menu")
        api.check(not (target / source.name).exists(),
                  "right-drag target is absent before choosing " + action)
        if action == "escape":
            api.key("Escape")
        else:
            label = "Cancel" if action == "cancel" else "Copy here"
            def menu_laid_out():
                points = api.state().get("menuPoints", {})
                labels = ("Copy here", "Move here", "Link here", "Cancel")
                return (all(name in points for name in labels)
                        and all(points[a][1] < points[b][1]
                                for a, b in zip(labels, labels[1:])))
            # Repeater delegates exist before Column's polish pass positions
            # them. Their initial coincident centres must never choose an action.
            api.wait(menu_laid_out, "drop action rows have distinct ordered geometry")
            (api.out / ("right-drag-menu-" + action + ".json")).write_text(json.dumps(api.state(), indent=2))
            api.pointer(api.state()["menuPoints"][label])
        api.wait(lambda: not api.state().get("menuOpen"), "right-drag menu ends: " + action)
        copied = target / source.name
        if action == "copy":
            api.wait(lambda: copied.is_file() and copied.read_bytes() == original
                     and bool(api.state().get("undoLabel"))
                     and api.state().get("jobState") in ("", "Confirmed"),
                     "right-drag copy completes with undo", 30)
        else:
            api.check(not api.state().get("jobRunning"), "cancelled right drag admits no job: " + action)
            api.check(not copied.exists(), "cancelled right drag changes no owned file: " + action)
        prove_released(action)
        if action != "copy":
            api.check(not copied.exists() and not api.state().get("jobRunning"),
                      "cancelled right drag stays inactive after publication: " + action)
        if action == "copy":
            api.key("z", "ctrl")
            api.wait(lambda: not copied.exists() and api.state().get("jobState") in ("", "Confirmed"),
                     "right-drag copy undo removes only its created target", 30)
            api.check(source.read_bytes() == original, "copy and undo preserve the original bytes")


def run_search_jobs(api, app):
    """Natural copy completion must not replace the active Find/grep view."""
    def idle():
        p = api.state().get("model", {}).get("projection", {})
        return bool(p) and not any(p.get(k, True) for k in
            ("sort_pending", "filter_pending", "selection_pending"))

    def command(text, label):
        palette_command(api, text, label)

    api.key("End")
    api.wait(lambda: api.state()["model"].get("cursor_name") == api.names[-1],
             "search job journey establishes keyboard focus")
    source = api.fixture / api.names[0]
    original = source.read_bytes()
    for kind in ("find", "grep"):
        needle = source.name if kind == "find" else "GUST_R2_SEARCH_MARKER"
        label = (f'Find "{needle}" in this folder and below' if kind == "find" else
                 f'Search inside files for "{needle}" in this folder and below')
        command(kind + " " + needle, label)
        api.wait(lambda: idle() and api.state().get("finding")
                 and api.state()["model"].get("visible") == [source.name],
                 kind + " publishes its exact owned hit", 45)
        before = api.state()
        load_revision = before["model"]["projection"]["slots"]["load_revision"]
        names_before = {p.name for p in api.fixture.iterdir()}
        command("Duplicate", "Duplicate")
        api.wait(lambda: api.state().get("jobState") == "Confirmed"
                 and not api.state().get("jobRunning")
                 and len({p.name for p in api.fixture.iterdir()} - names_before) == 1,
                 "owned copy finishes while " + kind + " remains active", 45)
        added = {p.name for p in api.fixture.iterdir()} - names_before
        copy = api.fixture / added.pop()
        api.check(copy.read_bytes() == original, kind + " copy has exact original bytes")
        after = api.state()
        (api.out / ("search-job-" + kind + ".json")).write_text(json.dumps({
            "before": before, "after": after, "copied": str(copy)}, indent=2))
        api.check(after["finding"] and after["model"]["visible"] == [source.name]
                  and after["model"]["projection"]["slots"]["load_revision"] == load_revision,
                  kind + " keeps its search ticket and rows through copy completion")
        api.shot("search-job-" + kind)
        api.key("Escape")
        api.wait(lambda: idle() and not api.state().get("finding")
                 and api.state()["model"].get("count") == len(api.names) + 1,
                 kind + " clear refreshes deferred origin contents", 45)
        api.key("z", "ctrl")
        api.wait(lambda: not copy.exists() and idle()
                 and api.state()["model"].get("count") == len(api.names),
                 kind + " copy Undo restores the owned origin", 45)
        api.check(source.read_bytes() == original, kind + " copy and Undo preserve the original bytes")
    api.key("F4", "alt")
    api.wait(lambda: app.poll() is not None, "search job window closes normally", 10)
    api.check(app.returncode == 0, "search job process exits normally")


def run_search_operations(api, app):
    """Operation refreshes defer until Find/grep clears; exact bytes and Undo survive."""
    def idle():
        p = api.state().get("model", {}).get("projection", {})
        return bool(p) and not any(p.get(k, True) for k in
            ("sort_pending", "filter_pending", "selection_pending"))

    def record(label):
        (api.out / (label + ".json")).write_text(json.dumps(api.state(), indent=2))
        api.shot(label)

    source = api.fixture / api.names[0]
    original = source.read_bytes()
    api.key("End")
    api.wait(lambda: api.state()["model"].get("cursor_name") == api.names[-1],
             "search operations journey establishes keyboard focus")
    for kind in ("find", "grep"):
        for operation in ("trash", "rename"):
            needle = source.name if kind == "find" else "GUST_R2_SEARCH_MARKER"
            label = (f'Find "{needle}" in this folder and below' if kind == "find" else
                     f'Search inside files for "{needle}" in this folder and below')
            palette_command(api, kind + " " + needle, label)
            api.wait(lambda: idle() and api.state().get("finding")
                     and api.state()["model"].get("visible") == [source.name],
                     kind + " presents its owned operation target", 45)
            load = api.state()["model"]["projection"]["slots"]["load_revision"]
            renamed = source.with_name("renamed-" + source.name)
            if operation == "trash":
                api.key("Delete")
                api.wait(lambda: api.state().get("confirmOpen"), "Trash confirmation opens")
                api.check(api.state().get("confirmTitle") == "Move to Trash?",
                          "owned hit uses recoverable Trash, never permanent deletion")
                api.key("Return")
                api.wait(lambda: not source.exists() and not api.state().get("confirmOpen")
                         and idle() and bool(api.state().get("undoLabel")),
                         "confirmed Trash removes only the owned hit and offers Undo", 45)
            else:
                api.key("F2")
                api.wait(lambda: api.state().get("promptOpen")
                         and api.state().get("promptMode") == "rename", "hit rename prompt opens")
                api.key("a", "ctrl")
                api.run(["wtype", renamed.name])
                api.key("Return")
                api.wait(lambda: not source.exists() and renamed.is_file()
                         and not api.state().get("promptOpen") and idle(),
                         "rename applies the owned hit name", 45)
                api.check(renamed.read_bytes() == original, "renamed hit retains exact original bytes")
            after = api.state()
            api.check(after.get("finding") and after["model"].get("visible") == [source.name]
                      and after["model"]["projection"]["slots"]["load_revision"] == load,
                      kind + " " + operation + " retains its ticket and pre-operation row until clear")
            record("search-" + kind + "-" + operation + "-deferred")
            api.key("Escape")
            expected = len(api.names) - (operation == "trash")
            api.wait(lambda: idle() and not api.state().get("finding")
                     and api.state()["model"].get("count") == expected,
                     "clearing search refreshes operation results", 45)
            if operation == "rename":
                api.check(api.state()["model"].get("cursor_name") == renamed.name,
                          "clear applies the renamed hit's deferred focus")
            api.key("z", "ctrl")
            api.wait(lambda: source.is_file() and not renamed.exists() and idle()
                     and api.state()["model"].get("count") == len(api.names),
                     kind + " " + operation + " Undo restores the original origin", 45)
            api.check(source.read_bytes() == original,
                      kind + " " + operation + " Undo preserves exact original bytes")
            record("search-" + kind + "-" + operation + "-undone")
    api.key("F4", "alt")
    api.wait(lambda: app.poll() is not None, "search operations window closes normally", 10)
    api.check(app.returncode == 0, "search operations process exits normally")


def run_size_cache(api, app):
    """Real folder totals after Copy/Undo; screenshots verify rendered totals."""
    directory = api.out / "size-sandbox"
    directory.mkdir(mode=0o700)
    target = directory / "Target"
    sibling = directory / "Untouched"
    target.mkdir(mode=0o700)
    sibling.mkdir(mode=0o700)
    source = directory / "source.bin"
    original = b"s" * 2048
    source.write_bytes(original)
    (target / "existing.bin").write_bytes(b"t" * 1024)
    (sibling / "existing.bin").write_bytes(b"u" * 512)
    owned = (source, target / "existing.bin", sibling / "existing.bin")

    def identity(path):
        stat = path.stat()
        return [stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns,
                stat.st_mode, hashlib.sha256(path.read_bytes()).hexdigest()]

    identities = {str(path): identity(path) for path in owned}
    snapshots = []

    def idle():
        p = api.state().get("model", {}).get("projection", {})
        return (bool(p) and not any(p.get(k, True) for k in
                ("sort_pending", "filter_pending", "selection_pending"))
                and p.get("shown_request_revision") == p.get("request_revision")
                and p.get("shown_load_revision") == p.get("slots", {}).get("load_revision")
                and not any(p.get("slots", {}).get(k, True) for k in CPU_SLOTS))

    def navigate(path):
        palette_command(api, str(path), "Go to " + str(path))
        api.wait(lambda: idle() and api.state()["model"].get("location") == str(path)
                 and api.state()["model"].get("settled"), "size fixture navigation settles", 45)

    def record(label, expected_bytes):
        current = api.state()
        api.check(current["model"]["projection"]["callbacks"]
                  .get("publication_events", {}).get("enabled") is False,
                  label + ": causal publication probe stays disabled")
        api.check(current["model"].get("visible") == ["Target", "Untouched", "source.bin"]
                  and current["model"].get("sized") == 2,
                  label + ": both folder totals are populated in the exact listing")
        api.check({str(path): identity(path) for path in owned} == identities,
                  label + ": source and unrelated files keep exact identity and bytes")
        snapshots.append({"stage": label, "state": current,
                          "resources": resources(app.pid),
                          "expected_rendered_folder_bytes": expected_bytes,
                          "owned_file_identities": identities})
        (api.out / "size-cache-states.json").write_text(json.dumps({
            "scope": "normal Copy/Undo and relisting; no same-backing renewal claim; rendered totals require screenshot inspection",
            "snapshots": snapshots}, indent=2))
        api.shot("size-cache-" + label)

    # Focus the pane before using the normal path palette.
    api.key("End")
    api.wait(lambda: api.state()["model"].get("cursor_name") == api.names[-1],
             "size journey establishes native keyboard focus")
    navigate(directory)
    palette_command(api, "View: Rows", "View: Rows")
    api.wait(lambda: idle() and api.state().get("view") == "rows"
             and api.state()["model"].get("meta_known") == 3
             and api.state()["model"].get("sized") == 2,
             "initial folder metadata and totals settle", 45)
    record("before", {"Target": 1024, "Untouched": 512})

    api.key("End")
    api.wait(lambda: api.state()["model"].get("cursor_name") == source.name,
             "owned Copy source is focused")
    api.key("c", "ctrl")
    api.wait(lambda: api.state()["model"].get("marked") == 1,
             "clipboard contains exactly the owned source")
    navigate(target)
    api.key("v", "ctrl")
    copied = target / source.name
    api.wait(lambda: copied.is_file() and copied.read_bytes() == original
             and not api.state().get("jobRunning") and bool(api.state().get("undoLabel")),
             "normal Paste completes its owned Copy and offers Undo", 45)
    api.check(source.read_bytes() == original and copied.stat().st_ino != source.stat().st_ino,
              "Copy creates a distinct inode with exact source bytes")
    navigate(directory)
    api.wait(lambda: idle() and api.state()["model"].get("sized") == 2,
             "both parent folder totals return after Copy", 45)
    record("copied", {"Target": 3072, "Untouched": 512})

    before_undo_listing = api.state()["model"]["projection"]["listing_revision"]
    api.key("z", "ctrl")
    api.wait(lambda: not copied.exists() and not api.state().get("jobRunning")
             and idle() and api.state()["model"].get("sized") == 2
             and api.state()["model"]["projection"]["listing_revision"] != before_undo_listing,
             "Undo removes only the created Copy and refreshes folder totals", 45)
    api.check(list(target.iterdir()) == [target / "existing.bin"]
              and list(sibling.iterdir()) == [sibling / "existing.bin"],
              "Undo preserves both pre-existing folder members")
    record("undone", {"Target": 1024, "Untouched": 512})
    api.key("F4", "alt")
    api.wait(lambda: app.poll() is not None, "size-cache window closes normally", 10)
    api.check(app.returncode == 0, "size-cache process exits normally")


def run_scalar_sorts(api, app):
    """Scalar-sort integration; state/resource observations are not frame timings."""
    snapshots = []

    def projection():
        return api.state().get("model", {}).get("projection", {})

    def idle():
        p = projection()
        return (bool(p) and not any(p.get(k, True) for k in
            ("sort_pending", "filter_pending", "selection_pending"))
            and str(p.get("shown_request_revision")) == str(p.get("request_revision")))

    def record(label):
        current = api.state()
        snapshots.append({"stage": label, "state": current, "resources": resources(app.pid)})
        (api.out / "scalar-sort-states.json").write_text(json.dumps(snapshots, indent=2))
        p = current["model"]["projection"]
        api.check(all(0 <= int(p["slots"][k]) <= 1 for k in
                      ("running", "pending", "completion", "candidate", "retiring")),
                  label + ": per-pane admission remains bounded")

    api.key("End")
    api.wait(lambda: api.state()["model"].get("cursor_name") == api.names[-1],
             "scalar journey establishes native keyboard focus")
    # The first scalar sort may reload missing metadata. Accepted Stage 2's
    # cold reload resets the cursor; record it separately from the subsequent
    # same-backing sorts whose remapping this journey qualifies.
    record("cold-name")
    palette_command(api, "Sort by size", "Sort by size")
    api.wait(lambda: idle() and api.state()["model"].get("settled")
             and api.state()["model"].get("sort_field") == "size"
             and api.state()["model"].get("meta_known") == len(api.names) + 1,
             "cold Size sort loads all metadata", 45)
    record("cold-size-reloaded")
    palette_command(api, "Sort by name", "Sort by name")
    api.wait(lambda: idle() and api.state()["model"].get("sort_field") == "name"
             and api.state()["model"].get("sort_ascending") is True,
             "warm scalar fixture returns to ascending Name order", 45)
    api.key("Home")
    target = api.names[min(7, len(api.names) - 1)]
    for _ in range(min(7, len(api.names) - 1)):
        api.key("Down")
    api.wait(lambda: api.state()["model"].get("cursor_name") == target, "scalar target is focused")
    api.key("space", "ctrl")
    api.wait(lambda: idle() and api.state().get("selCount") == 1, "scalar target selection applies")
    record("initial-name")
    numbers = {name: i for i, name in enumerate(api.names)}
    for field, label, modulus in (("size", "Sort by size", 17), ("modified", "Sort by date", 19)):
        for ascending in (True, False):
            palette_command(api, label, label)
            api.wait(lambda: idle() and api.state()["model"].get("settled")
                     and api.state()["model"].get("sort_field") == field
                     and api.state()["model"].get("sort_ascending") == ascending
                     and api.state()["model"].get("meta_known") == len(api.names) + 1,
                     field + " metadata and ordering settle", 45)
            expected = sorted(api.names, key=lambda name: ((numbers[name] * 524287) % modulus, name),
                              reverse=not ascending)
            current = api.state()
            api.check(current["model"].get("visible") == expected[:40]
                      and current["model"].get("count") == len(api.names),
                      field + " " + str(ascending) + ": exact first rows and membership count")
            api.check(current["model"].get("cursor_name") == target and current.get("selCount") == 1,
                      field + " preserves displayed cursor and selected target through remapping")
            record(field + ("-ascending" if ascending else "-descending"))
            api.shot("scalar-" + field + ("-ascending" if ascending else "-descending"))
    api.key("c", "ctrl")
    api.wait(lambda: idle() and api.state()["model"].get("marked") == 1,
             "scalar selected target reaches clipboard")
    payload = api.run(["wl-paste", "--no-newline", "--type", "text/uri-list"]).stdout
    expected_uri = "file://" + quote_from_bytes(os.fsencode(api.fixture / target), safe="/-._~")
    api.check(payload.strip() == expected_uri, "scalar sorts preserve exact selected clipboard URI")
    api.key("Return")
    opened = api.out / "opened.json"
    api.wait(opened.exists, "scalar activation reaches the private opener")
    api.check(json.loads(opened.read_text()) == [str(api.fixture / target)],
              "scalar activation resolves the original displayed raw target")
    generation = api.state()["model"]["generation"]
    api.key("r", "ctrl")
    api.wait(lambda: idle() and api.state()["model"].get("generation", 0) > generation
             and api.state()["model"].get("settled") and api.state()["model"].get("sort_field") == "modified"
             and api.state()["model"].get("sort_ascending") is False
             and api.state()["model"].get("visible") == expected[:40],
             "explicit refresh preserves requested scalar order", 45)
    record("modified-refresh")
    api.shot("scalar-modified-refreshed")
    before = projection()["worker"]
    palette_command(api, "Sort by size", "Sort by size", ("-s", "1", "/", "-s", "1", target))
    api.wait(lambda: idle() and projection().get("shown_filter") == target
             and api.state()["model"].get("sort_field") == "size"
             and api.state()["model"].get("visible") == [target],
             "rapid scalar sort/filter publishes only the newest requested rows", 45)
    after = projection()["worker"]
    api.check(after["superseded"] > before["superseded"]
              and after["cancel_observed"] > before["cancel_observed"],
              "native rapid scalar sort/filter observes supersession and cooperative cancellation")
    # Counters establish overlap, not which algorithm phase was cancelled;
    # controlled core/scheduler trials qualify scalar cancellation separately.
    record("scalar-filter-superseded")
    api.shot("scalar-filter-superseded")
    api.key("Return")
    api.key("Escape")
    api.key("Escape")
    api.wait(lambda: idle() and projection().get("shown_filter") == ""
             and api.state()["model"].get("count") == len(api.names),
             "superseded scalar/filter journey recovers the full origin", 45)
    api.key("F4", "alt")
    api.wait(lambda: app.poll() is not None, "scalar sort window closes normally", 10)
    api.check(app.returncode == 0, "scalar sort process exits normally")


def run_handoff(api, app):
    """Resolve actions on held rows while a real metadata reload is pending."""
    def projection():
        return api.state().get("model", {}).get("projection", {})

    def idle():
        p = projection()
        return bool(p) and not any(p.get(k, True) for k in
            ("sort_pending", "filter_pending", "selection_pending"))

    def record(label):
        snapshot = api.state()
        (api.out / ("handoff-" + label + ".json")).write_text(json.dumps(snapshot, indent=2))
        api.shot("handoff-" + label)
        return snapshot

    def wait_held(label, nudge):
        # A held completion changes no visible model property, so it need not
        # produce another QA receipt. Harmless menu/editor input requests a
        # fresh snapshot while keeping the target hold intact.
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            nudge()
            time.sleep(.15)
            if projection().get("slots", {}).get("completion") and api.state().get("liveHold"):
                return
        raise AssertionError(label)

    def palette(label, after=()):
        for _ in range(8):
            api.run(["wtype", ":"])
            time.sleep(.1)
            if api.state().get("paletteOpen"):
                break
        api.wait(lambda: api.state().get("paletteOpen"), "handoff palette opens")
        api.run(["wtype", label])
        api.wait(lambda: api.state().get("paletteResults", [])[:1] == [label],
                 "handoff palette offers exact action: " + label)
        # One virtual-keyboard connection keeps the interval after requesting
        # metadata short. The observed pending flags/completion prove overlap.
        api.run(["wtype", "-k", "Return", *after])

    api.key("End")
    api.wait(lambda: api.state()["model"].get("cursor_name") == api.names[-1],
             "handoff establishes native keyboard focus")
    api.key("Home")
    api.wait(lambda: idle() and api.names[2] in api.state().get("rowPoints", {}),
             "handoff has settled Name rows")
    api.check(api.state()["model"].get("meta_known", len(api.names) + 1) < len(api.names) + 1,
              "handoff starts with missing metadata for the controlled reload")

    if api.handoff_kind in ("menu", "all"):
        name = api.names[2]
        point = api.state()["rowPoints"][name]
        palette("Sort by size", ("-s", "10", "/", "-s", "10", api.names[0]))
        api.pointer(point, button="right")
        api.wait(lambda: api.state().get("menuOpen")
                 and api.state()["model"].get("cursor_name") == name
                 and not projection().get("selection_pending", True),
                 "original target menu opens before the metadata replacement")
        wait_held("filtered replacement is complete and held behind the menu",
                  lambda: api.key("Down"))
        api.check(projection().get("filter_pending"), "held menu still shows the original filter")
        record("menu-held")
        api.wait(lambda: {"Open", "Copy"} <= api.state().get("menuPoints", {}).keys()
                 and api.state()["menuPoints"]["Open"][1] < api.state()["menuPoints"]["Copy"][1],
                 "ordinary menu Copy has polished geometry")
        api.pointer(api.state()["menuPoints"]["Copy"])
        api.wait(lambda: not api.state().get("menuOpen") and idle(),
                 "menu action releases the prepared replacement")
        payload = api.run(["wl-paste", "--no-newline", "--type", "text/uri-list"]).stdout
        expected = "file://" + quote_from_bytes(os.fsencode(api.fixture / name), safe="/-._~")
        offered = [line for line in payload.splitlines() if line and not line.startswith("#")]
        (api.out / "handoff-copy-target.json").write_text(json.dumps({"expected": expected, "offered": offered}, indent=2))
        api.check(offered == [expected], "menu Copy keeps the exact original raw target through held filtering")
        record("menu-applied")
        api.key("Escape")
        api.wait(lambda: idle() and api.state()["model"].get("count") == len(api.names),
                 "handoff filter clears")
        api.key("Escape")
        palette("Sort by name")
        api.wait(lambda: idle() and api.state()["model"].get("visible") == api.names[:40],
                 "handoff restores ascending Name order")

    if api.handoff_kind in ("rename", "all"):
        if api.handoff_kind == "all":
            generation = api.state()["model"]["generation"]
            api.key("r", "ctrl")
            api.wait(lambda: idle() and api.state()["model"].get("generation", 0) > generation
                     and api.state()["model"].get("meta_known", len(api.names) + 1) < len(api.names) + 1,
                     "rename handoff begins on a fresh sparse Name backing", 45)
        first, second = (api.fixture / name for name in api.names[:2])
        expected = [first.read_bytes(), second.read_bytes()]
        api.key("Home")
        api.key("space", "ctrl")
        api.wait(lambda: idle() and api.state().get("selCount") == 1, "rename selects first original")
        api.key("Down")
        api.key("space", "ctrl")
        api.wait(lambda: idle() and api.state().get("selCount") == 2, "rename selects second original")
        palette("Sort by size", ("-s", "10", "-k", "F2"))
        api.wait(lambda: api.state().get("renameCardOpen") and projection().get("sort_pending"),
                 "rename card protects original line order during metadata sorting")
        api.offer_text("handoff-A.txt\nhandoff-B.txt")
        api.key("a", "ctrl")
        api.key("v", "ctrl")
        api.wait(lambda: api.state().get("renameCardOk")
                 and api.state().get("renameCardSummary") == "2 files will be renamed",
                 "rename card checks both replacement lines")
        def recheck_names():
            api.key("a", "ctrl")
            api.key("v", "ctrl")
        wait_held("reordering completion waits behind the checked rename card", recheck_names)
        api.check(projection().get("sort_pending"), "rename card still protects the original ordering")
        record("rename-held")
        api.key("Return", "ctrl")
        renamed = [api.fixture / "handoff-A.txt", api.fixture / "handoff-B.txt"]
        api.wait(lambda: not api.state().get("renameCardOpen") and all(p.exists() for p in renamed),
                 "held rename applies both names", 45)
        actual = [p.read_bytes() for p in renamed]
        (api.out / "handoff-rename-association.json").write_text(json.dumps({
            "originals": [str(first), str(second)], "renamed": [str(p) for p in renamed],
            "expected_sha256": [hashlib.sha256(b).hexdigest() for b in expected],
            "actual_sha256": [hashlib.sha256(b).hexdigest() for b in actual]}, indent=2))
        api.check(actual == expected, "rename keeps each checked line associated with its original file bytes")
        api.wait(lambda: idle() and bool(api.state().get("undoLabel")), "rename publishes and offers Undo", 45)
        api.key("z", "ctrl")
        api.wait(lambda: first.exists() and second.exists() and not any(p.exists() for p in renamed),
                 "rename Undo restores only the original names", 45)
        api.check([first.read_bytes(), second.read_bytes()] == expected,
                  "rename Undo preserves both original byte identities")
        record("rename-undone")
    api.key("F4", "alt")
    api.wait(lambda: app.poll() is not None, "handoff window closes normally", 10)
    api.check(app.returncode == 0, "handoff process exits normally")


def run_journey(api, app):
    records = []
    snapshots = []
    cpu_observer = (ThreadCounterObserver(app.pid)
                    if getattr(api, "native_cpu_control", False) else None)
    expected = {}
    descending = list(reversed(api.names))
    for query in ("file", "zzz", "19"):
        expected[query] = [name for name in descending
                           if api.subsequence(query, name)]

    def projection(current=None):
        current = api.state() if current is None else current
        return current.get("model", {}).get("projection", {})

    def idle(current=None):
        p = projection(current)
        return (bool(p) and not any(p.get(k, True) for k in
                ("sort_pending", "filter_pending", "selection_pending"))
                and str(p.get("shown_request_revision")) == str(p.get("request_revision"))
                and str(p.get("shown_selection_revision")) == str(p.get("selection_revision")))

    def observe(label):
        current = api.state()
        p = projection(current)
        api.check(bool(p), label + ": projection instrumentation is present")
        slots = p.get("slots", {})
        api.check(all(isinstance(slots.get(key), (int, bool)) and 0 <= int(slots[key]) <= 1
                      for key in ("running", "pending", "completion", "candidate", "retiring")),
                  label + ": per-pane work and retirement slots remain bounded")
        snapshots.append({"stage": label, "projection": p,
                          "model": current.get("model"), "selection": current.get("selCount"),
                          "cursor_point": current.get("cursorPoint"), "view": current.get("view"),
                          "columns_side": current.get("columnsSide"),
                          "columns_points": current.get("columnsPreviewPoints")})
        if cpu_observer is not None:
            snapshots[-1]["gui_thread_counters"] = cpu_observer.sample()
        (api.out / "projection-states.json").write_text(json.dumps(snapshots, indent=2))
        return current

    def palette_action(label):
        for _ in range(8):
            api.run(["wtype", ":"])
            time.sleep(.2)
            if api.state().get("paletteOpen"):
                break
        api.wait(lambda: api.state().get("paletteOpen"), "palette opens for " + label)
        api.run(["wtype", label])
        api.wait(lambda: api.state().get("paletteText") == label
                 and api.state().get("paletteResults", [])[:1] == [label],
                 "exact palette action offered: " + label)
        api.key("Return")
        api.wait(lambda: not api.state().get("paletteOpen"), "palette closes for " + label)

    # Establish focus and a cached, nonexecutable activation target before sort.
    for _ in range(8):
        api.key("End")
        time.sleep(.2)
        if api.state().get("model", {}).get("cursor_name") == api.names[-1]:
            break
    api.wait(lambda: api.state().get("model", {}).get("cursor_name") == api.names[-1],
             "projection journey establishes native keyboard focus")
    api.key("Home")
    api.wait(lambda: api.state().get("model", {}).get("cursor_name") == api.names[0]
             and api.state()["model"].get("cursor_meta") and idle(),
             "initial cursor target has details and a coherent displayed projection")
    initial = observe("initial")
    api.check(initial["model"].get("visible") == api.names[:40], "initial projection has exact Name order")
    context_menu_journey(api, palette_action, idle, observe)
    clipboard_marks_journey(api, idle, observe)

    palette_action("Sort by name")
    api.wait(lambda: api.state().get("model", {}).get("sort_field") == "name"
             and api.state()["model"].get("sort_ascending") is False,
             "descending Name request acknowledged")
    at_activation = observe("sort-activation")
    # The opener is a private recorder, never the user's desktop handler.
    api.key("Return")
    opened = api.out / "opened.json"
    api.wait(opened.exists, "native activation reaches the private opener")
    argv = json.loads(opened.read_text())
    api.check(argv == [str(api.fixture / api.names[0])],
              "activation during or after sort resolves the displayed byte-name target")
    records.append({"journey": "projection-activation", "sort_pending_observed":
                    projection(at_activation).get("sort_pending"), "argv": argv,
                    "scope": "pending overlap is claimed only when the recorded flag is true"})
    api.wait(lambda: idle() and api.state()["model"].get("visible") == descending[:40],
             "descending Name permutation is displayed", 45)
    api.check(api.state()["model"].get("cursor_name") == api.names[0],
              "sort preserves cursor file identity after row remapping")
    api.shot("projection-sort")

    api.run(["wtype", "/"])
    api.wait(lambda: api.state().get("filterBarActive"), "rapid filter editor opened")
    for query in ("zzz", "file", "zzz", "19"):
        api.key("a", "ctrl")
        start = time.monotonic()
        api.run(["wtype", query])
        api.wait(lambda: api.state().get("model", {}).get("filter") == query,
                 "latest filter request echoed: " + query)
        records.append({"journey": "filter-request", "query": query,
                        "input_to_requested_state_ms": (time.monotonic() - start) * 1000})
    start = time.monotonic()
    api.wait(lambda: idle() and projection().get("shown_filter") == "19"
             and api.state()["model"].get("count") == len(expected["19"]),
             "newest filter revision publishes", 45)
    final = observe("latest-filter")
    api.check(final["model"].get("visible") == expected["19"][:40],
              "latest filtered first rows preserve descending listing order")
    records.append({"journey": "filter-completion-observer", "query": "19",
                    "after_request_echo_to_qa_receipt_ms": (time.monotonic() - start) * 1000,
                    "scope": "observer state receipt; not input-to-frame latency"})
    api.shot("projection-filter")
    api.key("Return")
    api.key("Escape")
    api.key("Escape")
    api.wait(lambda: idle() and projection().get("shown_filter") == ""
             and api.state()["model"].get("count") == len(api.names), "filter clear publishes", 45)

    api.key("a", "ctrl")
    api.wait(lambda: idle() and api.state().get("selCount") == len(api.names),
             "worker selection covers all displayed rows", 45)
    observe("select-all")
    api.key("Escape")
    api.wait(lambda: idle() and api.state().get("selCount") == 0,
             "large selection clears without retaining stale targets", 45)
    observe("selection-cleared")

    if api.soak_cycles:
        soak = []
        # Identical cycles and idle endpoints make allocator/cache retention
        # comparable. Three cycles warm this declared workload only.
        for cycle in range(3 + api.soak_cycles):
            for query in ("file", "19", "zzz", ""):
                api.run(["wtype", "/"])
                api.wait(lambda: api.state().get("filterBarActive"), "soak filter opens")
                api.key("a", "ctrl")
                if query:
                    api.run(["wtype", query])
                else:
                    api.key("BackSpace")
                api.wait(lambda: idle() and projection().get("shown_filter") == query,
                         "soak filter publishes: " + query, 45)
                api.key("Return")
            api.key("a", "ctrl")
            api.wait(lambda: idle() and api.state().get("selCount") == len(api.names),
                     "soak selection publishes", 45)
            api.key("Escape")
            api.wait(lambda: idle() and api.state().get("selCount") == 0
                     and not any(projection().get("slots", {}).get(k, True) for k in
                                 ("running", "pending", "completion", "candidate", "retiring")),
                     "soak cycle retires all replaced storage", 45)
            time.sleep(.2)
            soak.append({"cycle": cycle, "warmup": cycle < 3,
                         "resources": resources(app.pid), "projection": projection()})
            if cpu_observer is not None:
                soak[-1]["gui_thread_counters"] = cpu_observer.sample()
            (api.out / "projection-soak.json").write_text(json.dumps({
                "warmup_cycles": 3, "measured_cycles": api.soak_cycles,
                "scope": "quiescent process-tree snapshots, sampled after identical cycles; not peak memory or an indefinite plateau proof",
                "samples": soak}, indent=2))
        api.check(all(not row["resources"]["errors"] for row in soak),
                  "all declared warm-up and measured resource snapshots completed")
        if getattr(api, "soak_only", False):
            observe("soak-complete")
            api.shot("projection-soak-complete")
            if cpu_observer is not None:
                run_native_cpu_control(api, app, cpu_observer, observe)
            api.key("F4", "alt")
            api.wait(lambda: app.poll() is not None, "retention journey closes normally", 10)
            api.check(app.returncode == 0, "retention process exits normally")
            return

    api.key("Home")
    api.run(["wtype", *(part for _ in range(15) for part in ("-k", "Down"))])
    api.wait(lambda: api.state().get("live") and idle(), "local live watcher is active")
    time.sleep(.4)  # Geometry settle, excluded from every latency sample.
    before_live = observe("before-live")
    cursor_name = before_live["model"]["cursor_name"]
    api.key("Menu")
    api.wait(lambda: api.state().get("menuOpen") and api.state().get("liveHold"),
             "row-dependent context menu holds publication")
    held_revision = projection().get("display_revision")
    live_name = "zz-live-added.txt"
    (api.fixture / live_name).write_text("owned synthetic live update\n")
    time.sleep(.5)  # Cross the inotify coalescing interval while the hold remains.
    held = observe("held-live")
    api.check(held["model"]["count"] == len(api.names)
              and projection(held).get("display_revision") == held_revision,
              "live publication cannot move rows under the open menu")
    api.shot("projection-live-held")
    api.key("Escape")
    api.wait(lambda: idle() and api.state()["model"].get("count") == len(api.names) + 1,
             "live replacement publishes after the hold ends", 45)
    time.sleep(.4)
    after_live = observe("live-published")
    api.check(after_live["model"]["cursor_name"] == cursor_name,
              "live reset preserves the cursor byte-name target")
    before_point, after_point = before_live.get("cursorPoint"), after_live.get("cursorPoint")
    api.check(bool(before_point and after_point) and abs(before_point[1] - after_point[1]) <= 1,
              "live reset preserves the visible cursor pixel offset")
    api.shot("projection-live")

    for label, mode in (("View: Grid", "grid"), ("View: Columns", "columns"), ("View: Rows", "rows")):
        palette_action(label)
        api.wait(lambda: api.state().get("view") == mode and idle(), "published rows shown in " + mode)
        observe("view-" + mode)
        api.shot("projection-" + mode)

    # Only this new folder is a directory, so dirs-first puts it at Home.
    # Its child token is constructed independently from OS bytes, never labels.
    byte_folder = api.fixture / os.fsdecode(b"preview-\xff")
    byte_child = byte_folder / os.fsdecode(b"child-\xfe")
    byte_child.mkdir(parents=True, mode=0o700)
    (byte_child / "owned.txt").write_text("owned synthetic byte-token target\n")
    folder_token = "local:" + os.fsencode(byte_folder).hex()
    child_token = "local:" + os.fsencode(byte_child).hex()
    api.wait(lambda: idle() and api.state()["model"].get("count") == len(api.names) + 2,
             "owned byte-name directory appears through live publication", 45)
    api.key("Home")
    palette_action("View: Columns")
    api.wait(lambda: api.state().get("view") == "columns" and idle(), "Columns view opens for byte-token preview")

    def byte_preview_ready():
        current = api.state()
        side = current.get("columnsSide") or {}
        points = current.get("columnsPreviewPoints") or {}
        return ((side.get("peek") or {}).get("token") == folder_token
                and points.get("revision") == side.get("revision")
                and child_token in (points.get("peek") or {}))

    api.wait(byte_preview_ready, "current Columns preview exposes exact raw-byte child token geometry", 45)
    preview = observe("columns-byte-preview")
    api.shot("projection-columns-byte-preview")
    api.pointer(preview["columnsPreviewPoints"]["peek"][child_token])
    api.wait(lambda: idle() and projection().get("shown_location") == child_token
             and api.state()["model"].get("visible") == ["owned.txt"],
             "native preview click opens the exact raw-byte child path", 45)
    api.check(projection().get("shown_location") == child_token,
              "Columns navigation preserves invalid UTF-8 bytes rather than joining display labels")
    observe("columns-byte-navigated")
    api.shot("projection-columns-byte-navigated")

    # The pointer journey changed the active Wayland input device. Establish
    # keyboard admission observably, as at the start of this journey.
    for _ in range(8):
        api.key("t", "ctrl")
        time.sleep(.2)
        if api.state().get("tabCount") != 1:
            break
    api.wait(lambda: api.state().get("tabCount") == 2 and idle()
             and projection().get("shown_location") == child_token
             and api.state()["model"].get("visible") == ["owned.txt"],
             "new tab duplicates the exact displayed byte-token location", 45)
    api.check(projection().get("shown_location") == child_token,
              "new-tab navigation never decodes the escaped display path")
    observe("byte-location-new-tab")
    api.shot("projection-byte-new-tab")
    api.key("w", "ctrl")
    api.wait(lambda: api.state().get("tabCount") == 1 and idle()
             and projection().get("shown_location") == child_token,
             "closing the new tab preserves the original raw location", 45)
    right_drag_journey(api, byte_child, palette_action, idle, observe)
    grid_live_journey(api, byte_child, palette_action, idle, observe)

    last = observe("complete")
    records.append({"journey": "projection-counters", "projection": projection(last),
                    "scope": "bounded approximate callback histogram and worker maxima; no frame/presentation claim"})
    api.samples.extend(records)
    (api.out / "projection-observations.json").write_text(json.dumps(records, indent=2))
    api.key("F4", "alt")
    api.wait(lambda: app.poll() is not None, "projection journey closes normally", 10)
    api.check(app.returncode == 0, "projection window exits normally")
