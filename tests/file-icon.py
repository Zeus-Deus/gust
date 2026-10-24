#!/usr/bin/env python3
"""Check shipped FileIcon pixels under Qt Quick's offscreen software renderer.

Requires Qt6 Quick Test. Only Tokens is stubbed; FileIcon.qml is loaded directly
from ui/. Use --artifacts DIR to retain screenshots for each checked frame.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--runner", type=Path,
                        help="qmltestrunner from the selected Qt SDK")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    runner = str(args.runner) if args.runner else shutil.which("qmltestrunner6")
    if runner is None:
        candidate = Path("/usr/lib/qt6/bin/qmltestrunner")
        if candidate.is_file():
            runner = str(candidate)
    if runner is None:
        raise SystemExit("Qt6 qmltestrunner is required")
    artifacts = ""
    if args.artifacts:
        args.artifacts.mkdir(parents=True, exist_ok=True)
        artifacts = str(args.artifacts.resolve()) + "/"
    with tempfile.TemporaryDirectory(prefix="gust-file-icon-") as temp:
        root = Path(temp)
        test = (repo / "tests/qml/tst_file_icon.qml").read_text()
        test = test.replace("@UI_URL@", (repo / "ui").as_uri())
        test = test.replace("@ARTIFACTS@", json.dumps(artifacts))
        (root / "tst_file_icon.qml").write_text(test)
        env = os.environ.copy()
        env.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
                   QT_FORCE_STDERR_LOGGING="1",
                   HOME=str(root / "home"),
                   XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"),
                   XDG_DATA_HOME=str(root / "data"),
                   XDG_STATE_HOME=str(root / "state"))
        result = subprocess.run(
            [runner, "-import", str(repo / "tests/qml/fixtures"),
             "-input", str(root)], env=env, check=False)
        raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
