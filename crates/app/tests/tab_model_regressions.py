#!/usr/bin/env python3
"""Run exact production tab selectors in Qt; not native DirModel/visual QA."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import subprocess
import sys


def block(text, marker):
    start = text.index(marker)
    brace = text.index("{", start)
    depth = 0
    for at in range(brace, len(text)):
        if text[at] == "{":
            depth += 1
        elif text[at] == "}":
            depth -= 1
            if depth == 0:
                return text[start : at + 1]
    raise ValueError(f"Unclosed production block: {marker}")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require_vm():
    # Admission is a runtime boundary, including under python -O/-OO.
    if os.getuid() != 1000 or os.geteuid() != 1000:
        raise RuntimeError("VM QA requires real and effective UID 1000")
    try:
        virt = subprocess.check_output(
            ["systemd-detect-virt"], text=True, stderr=subprocess.PIPE, timeout=5
        ).strip()
    except (OSError, subprocess.SubprocessError) as error:
        raise RuntimeError("VM QA could not determine virtualization") from error
    if virt != "kvm":
        raise RuntimeError(f"VM QA requires KVM, detected {virt!r}")


def replace_once(fixture, marker, value):
    if fixture.count(marker) != 1:
        raise ValueError(f"Fixture requires exactly one {marker}")
    return fixture.replace(marker, value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--counterfactual-refresh", action="store_true")
    parser.add_argument("--insertion", action="store_true")
    args = parser.parse_args()
    require_vm()
    source = args.source.resolve()
    output = args.output.resolve()
    if output.exists():
        raise RuntimeError("Use a new evidence directory for every run")
    output.mkdir(parents=True, mode=0o700)
    paths = [
        source / "ui/Main.qml",
        source / ("ui/tests/tst_tab_insertion.qml.in" if args.insertion else "ui/tests/tst_tab_model.qml.in"),
        source / "crates/app/tests/tab_model_regressions.py",
    ]
    before = {str(path.relative_to(source)): digest(path.read_bytes()) for path in paths}
    text = paths[0].read_text()
    names = [
        "paneTabs", "paneModel", "paneViewMode", "refreshPanes",
        "refreshPaneTabCount", "refreshActiveModel", "refreshTabLabels",
        "activateTab", "nextTab", "runningJobModels",
    ]
    if args.insertion:
        names.append("newTab")
    functions = []
    for name in names:
        body = block(text, "function " + name + "(")
        if name == "activateTab" and args.counterfactual_refresh:
            if body.count("win.focusList()") != 1:
                raise ValueError("Counterfactual requires exactly one win.focusList()")
            body = body.replace("win.focusList()", "win.refreshPaneTabCount()\n        win.focusList()")
        functions.append(body)
    handler = block(text, "onActiveTabChanged:")
    handler = handler[handler.index("{") + 1 : -1]
    fixture = replace_once(paths[1].read_text(), "@FUNCTIONS@", "\n".join(functions))
    fixture = replace_once(fixture, "@ACTIVE_TAB_HANDLER@", handler)
    qml = output / ("tst_tab_insertion.qml" if args.insertion else "tst_tab_model.qml")
    qml.write_text(fixture)
    (output / "Main.qml").write_text(text)
    for directory in ["home", "runtime", "tmp"]:
        (output / directory).mkdir(mode=0o700)
    env = {
        "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
        "HOME": str(output / "home"), "TMPDIR": str(output / "tmp"),
        "XDG_RUNTIME_DIR": str(output / "runtime"),
        "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
        "QSG_RENDER_LOOP": "basic", "QT_FORCE_STDERR_LOGGING": "1",
        "QT_ASSUME_STDERR_HAS_CONSOLE": "1",
    }
    argv = ["/usr/lib/qt6/bin/qmltestrunner", "-input", str(qml)]
    process = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=45)
    log = process.stdout + process.stderr
    (output / "run.log").write_text(log)
    after = {str(path.relative_to(source)): digest(path.read_bytes()) for path in paths}
    receipt = {
        "argv": argv, "exit": process.returncode, "uid": os.getuid(),
        "boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
        "source_hashes": before, "source_unchanged": after == before,
        "source_hashes_after": after,
        "qml_sha256": digest(qml.read_bytes()), "log_sha256": digest(log.encode()),
        "scope": ("Exact Main.qml methods with actual Qt ListModel/Instantiator insertion; external model I/O, CXX transport and visual/focus hooks replaced"
                  if args.insertion else "Exact Main.qml methods and synchronous active-tab signal; external model I/O, objects and visual input replaced"),
        "counterfactual_refresh_only": args.counterfactual_refresh,
        "insertion_lifecycle_only": args.insertion,
    }
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    print(json.dumps(receipt, indent=2))
    if after != before:
        raise RuntimeError("QA source changed during runner execution")
    return process.returncode


if __name__ == "__main__":
    sys.exit(main())
