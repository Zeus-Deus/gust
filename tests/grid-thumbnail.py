#!/usr/bin/env python3
"""Exercise the shipped grid image with landscape, portrait and resized PNGs.

Requires Pillow and Qt6 Quick Test. Fixtures and generated test input live in a
private temporary directory; the application and user's settings are untouched.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from PIL import Image


def main():
    repo = Path(__file__).resolve().parent.parent
    runner = shutil.which("qmltestrunner6")
    if runner is None:
        candidate = Path("/usr/lib/qt6/bin/qmltestrunner")
        if candidate.is_file():
            runner = str(candidate)
    if runner is None:
        raise SystemExit("Qt6 qmltestrunner is required")
    with tempfile.TemporaryDirectory(prefix="gust-grid-thumbnail-") as temp:
        root = Path(temp)
        for name, size in {
            "landscape": (128, 96),
            "portrait": (96, 128),
            "wide": (128, 8),
            "tall": (8, 128),
            "square": (128, 128),
            "large": (1024, 768),
        }.items():
            Image.new("RGB", size, (37, 149, 213)).save(root / f"{name}.png")
        test = (repo / "tests/qml/tst_grid_thumbnail.qml").read_text()
        test = test.replace("@UI_URL@", (repo / "ui").as_uri())
        test = test.replace("@FIXTURE_URL@", json.dumps(root.as_uri() + "/"))
        (root / "tst_grid_thumbnail.qml").write_text(test)
        env = os.environ.copy()
        env.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
                   QT_FORCE_STDERR_LOGGING="1", HOME=str(root / "home"),
                   XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"),
                   XDG_DATA_HOME=str(root / "data"),
                   XDG_STATE_HOME=str(root / "state"))
        subprocess.run([runner, "-input", str(root)], env=env, check=True)


if __name__ == "__main__":
    main()
