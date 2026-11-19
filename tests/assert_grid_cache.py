"""Verify section 10q against the completed Grid pass for its fixture directory."""

import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from urllib.parse import parse_qs, unquote_to_bytes, urlsplit


def digest(value):
    return hashlib.md5(value.encode(), usedforsecurity=False).hexdigest()


def completed_grid(state, display_location, names):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        snapshot = json.loads(state.read_text())
        model = snapshot["model"]
        if (snapshot["view"] == "grid" and model["location"] == display_location
                and model["settled"] and set(model["visible"]) == names
                and model["full_thumbnail_pass_complete"]):
            return model["visible_thumbnails"]
        time.sleep(0.1)
    sys.exit(f"thumbnail pass did not finish for {display_location} within 15 seconds")


def check_png(path, source_name):
    if not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
        sys.exit(f"missing or oversized thumbnail: {path}")
    # Decode the pixels with the PNG reader, keeping fixture paths out of ImageMagick syntax.
    with path.open("rb") as png:
        result = subprocess.run(
            ["magick", "-regard-warnings", "-limit", "width", "128", "-limit", "height", "128",
             "+ping", "png:-", "-format", "%w %h", "info:"],
            stdin=png, stdout=subprocess.PIPE, text=True,
        )
    if result.returncode:
        sys.exit(f"invalid or oversized normal PNG thumbnail: {path}")
    width, height = map(int, result.stdout.split())
    if source_name == "wide.png" and width <= height:
        sys.exit(f"expected a landscape thumbnail for {source_name}: {path} ({width}x{height})")
    if source_name == "tall.jpg" and height <= width:
        sys.exit(f"expected a portrait thumbnail for {source_name}: {path} ({width}x{height})")


def main():
    state, pictures = map(Path, sys.argv[1:3])
    display_location, cache_dir, *image_names = sys.argv[3:]
    cache = Path(cache_dir)
    images = set(image_names)
    refused = {"corrupt.png", "logo.svg", "notes.txt", "program"}
    urls = completed_grid(state, display_location, images | refused)
    if set(urls) != images:
        sys.exit(f"Grid thumbnail URLs differ: expected {sorted(images)}, saw {sorted(urls)}")

    for name in sorted(images):
        key = digest((pictures / name).as_uri())
        url = urlsplit(urls[name])
        versions = parse_qs(url.query).get("gust", [])
        if url.scheme != "file" or url.netloc or len(versions) != 1:
            sys.exit(f"invalid Grid thumbnail URL for {name}: {urls[name]}")
        displayed = Path(os.fsdecode(unquote_to_bytes(url.path)))
        if displayed != cache / "gust-grid" / "normal" / f"{key}-{digest(versions[0])}.png":
            sys.exit(f"unexpected Grid thumbnail path for {name}: {displayed}")
        check_png(displayed, name)
        check_png(cache / "normal" / f"{key}.png", name)

    for name in sorted(refused):
        key = digest((pictures / name).as_uri())
        for pattern in (f"*/{key}.png", f"gust-grid/*/{key}-*.png"):
            unexpected = next(cache.glob(pattern), None)
            if unexpected is not None:
                sys.exit(f"unexpected thumbnail for {name}: {unexpected}")


if __name__ == "__main__":
    main()
