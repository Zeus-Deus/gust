#!/usr/bin/env python3
"""Exercise the shipped toolbar button: click, unavailable, name and tip.

Requires Qt6 Quick Test. Runs offscreen with deterministic theme tokens; the
application and the user's settings are untouched.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    repo = Path(__file__).resolve().parent.parent
    runner = shutil.which("qmltestrunner6")
    if runner is None:
        candidate = Path("/usr/lib/qt6/bin/qmltestrunner")
        if candidate.is_file():
            runner = str(candidate)
    if runner is None:
        raise SystemExit("Qt6 qmltestrunner is required")
    with tempfile.TemporaryDirectory(prefix="gust-bar-button-") as temp:
        root = Path(temp)
        test = (repo / "tests/qml/tst_bar_button.qml").read_text()
        test = test.replace("@UI_URL@", (repo / "ui").as_uri())
        (root / "tst_bar_button.qml").write_text(test)
        env = os.environ.copy()
        env.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
                   QT_FORCE_STDERR_LOGGING="1", HOME=str(root / "home"),
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
