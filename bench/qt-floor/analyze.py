#!/usr/bin/env python3
"""Verify saved evidence and emit independent PSS, DRM and mapping summaries."""
import json
from pathlib import Path
import re
import statistics
import struct
import sys

out = Path(sys.argv[1]).resolve()
rows = json.loads((out / "results.json").read_text())
manifest = json.loads((out / "manifest.json").read_text())
assert len(rows) == manifest["rounds"] * 12, "incomplete matrix"
assert len({r["label"] for r in rows}) == len(rows)
groups = {}
for row in rows:
    assert not row["errors"], row["errors"]
    assert row["sample_start_after_map_s"] >= 3
    clients = row["drm_clients"]
    assert len({(c["device"], c["client_id"]) for c in clients}) == len(clients)
    log = (out / (row["label"] + ".log")).read_text()
    if row["renderer"] == "software":
        assert "Loading backend software" in log or "api=1" in log
        assert not clients, "software unexpectedly opened DRM client"
    else:
        backend = {"gl": "OpenGL", "vulkan": "Vulkan"}[row["renderer"]]
        assert f"Creating QRhi with backend {backend}" in log
        assert clients
    if row["app"] == "floor":
        count = 0 if row["fixture"] == "empty" else 100000
        assert f"FLOOR_SETTLED rows={count} qt=6.12.0" in log
        assert "size=1280x900" in log
    if row["iteration"] == 0:
        png = (out / (row["label"] + ".png")).read_bytes()
        assert struct.unpack(">II", png[16:24]) == (1280, 900)
    paths = {}
    for proc in row["processes"]:
        maps = json.loads((out / (row["label"] + "-proc") / f'{proc["pid"]}.mappings.json').read_text())
        assert sum(m["fields_kib"].get("Pss", 0) for m in maps) == proc["pss_kib"]
        assert any("libQt6Quick.so.6.12.0" in m["path"] for m in maps)
        for mapping in maps:
            bucket = paths.setdefault(mapping["path"], {"Size": 0, "Rss": 0, "Pss": 0})
            for field in bucket:
                bucket[field] += mapping["fields_kib"].get(field, 0)
    row["mapping_paths_kib"] = paths
    groups.setdefault((row["renderer"], row["fixture"], row["app"]), []).append(row)

def stats(values):
    return {"median": statistics.median(values), "min": min(values), "max": max(values)}

summary = []
for (renderer, fixture, app), values in groups.items():
    keys = sorted({k for r in values for k in r["drm_region_totals_bytes"]})
    paths = sorted({k for r in values for k in r["mapping_paths_kib"]})
    summary.append({"renderer": renderer, "fixture": fixture, "app": app, "n": len(values),
                    "pss_kib": stats([r["pss_kib"] for r in values]),
                    "uss_kib": stats([r["uss_kib"] for r in values]),
                    "drm_region_bytes": {k: stats([r["drm_region_totals_bytes"].get(k, 0) for r in values]) for k in keys},
                    "mapping_paths_kib": {p: {f: stats([r["mapping_paths_kib"].get(p, {}).get(f, 0) for r in values])
                                               for f in ("Size", "Rss", "Pss")} for p in paths}})
(out / "summary.json").write_text(json.dumps(summary, indent=2))
lines = ["# Matched Qt Quick floor: measured evidence", "",
         "Medians of three launches; all numbers MiB. CPU PSS and DRM region counters are independent views; never add them.", "",
         "| Renderer | Case | App | PSS | resident VRAM | shared VRAM | resident GTT | shared GTT |",
         "|---|---|---|---:|---:|---:|---:|---:|"]
for s in summary:
    drm = s["drm_region_bytes"]
    cols = [s["pss_kib"]["median"] / 1024] + [drm.get(k, {}).get("median", 0) / 1024**2
            for k in ("drm-resident-vram", "drm-shared-vram", "drm-resident-gtt", "drm-shared-gtt")]
    lines.append(f'| {s["renderer"]} | {s["fixture"]} | {s["app"]} | ' + " | ".join(f"{v:.2f}" for v in cols) + " |")
lines += ["", "Software has no observed application DRM client; its zero cells mean no counter was exposed, not zero compositor GPU cost.", "",
          "## Per-path mapping attribution", "", "PSS follows the mapping pathname, not allocation provenance. Anonymous/heap mappings cannot be assigned to Qt vs the driver with smaps alone. All individual mappings and fields remain in `*-proc/*.mappings.json` and raw `.smaps` files."]
for s in summary:
    lines += ["", f'### {s["renderer"]} / {s["fixture"]} / {s["app"]}', "",
              "| Mapping path | PSS MiB | RSS MiB | virtual size MiB |", "|---|---:|---:|---:|"]
    ordered = sorted(s["mapping_paths_kib"].items(), key=lambda v: v[1]["Pss"]["median"], reverse=True)
    selected = ordered[:8] + [(p, v) for p, v in ordered[8:] if re.search(r"gallium|vulkan|/dev/dri|Qt6Quick|libdrm", p)]
    for path, value in selected:
        lines.append(f'| `{path}` | ' + " | ".join(f'{value[f]["median"] / 1024:.2f}' for f in ("Pss", "Rss", "Size")) + " |")
lines += ["", "## Full DRM counter matrix", "", "All exposed memory/total/resident/shared/active/purgeable counters are retained separately below. Values overlap; neither rows nor columns form physical ownership totals."]
keys = sorted({k for s in summary for k in s["drm_region_bytes"]})
lines += ["", "| Counter (MiB) | " + " | ".join(f'{s["renderer"]}/{s["fixture"]}/{s["app"]}' for s in summary) + " |",
          "|---|" + "---:|" * len(summary)]
for key in keys:
    lines.append(f"| {key} | " + " | ".join(f'{s["drm_region_bytes"][key]["median"] / 1024**2:.3f}'
                 if key in s["drm_region_bytes"] else "n/a" for s in summary) + " |")
(out / "summary.md").write_text("\n".join(lines) + "\n")
print(f"PASS: {len(rows)} samples; renderer logs, floor dimensions/counts, screenshot dimensions, client uniqueness, per-mapping PSS sums, no /proc errors")
print("SUMMARY", out / "summary.md")
