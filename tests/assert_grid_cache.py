"""Verify section 10q against the completed Grid pass for its fixture directory."""

import hashlib
import json
import sys
import time
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit


def digest(value):
    return hashlib.md5(value.encode(), usedforsecurity=False).hexdigest()


def completed_grid(state, pictures, names):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        snapshot = json.loads(state.read_text())
        model = snapshot["model"]
        if (snapshot["view"] == "grid" and model["location"] == str(pictures)
                and model["settled"] and set(model["visible"]) == names
                and model["full_thumbnail_pass_complete"]):
            return model["visible_thumbnails"]
        time.sleep(0.1)
    sys.exit(f"thumbnail pass did not finish for {pictures} within 15 seconds")


def check_png(path, source_name):
    from PIL import Image

    if not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
        sys.exit(f"missing or oversized thumbnail: {path}")
    with Image.open(path, formats=["PNG"]) as image:
        if max(image.size) > 128:
            sys.exit(f"oversized normal thumbnail: {path} ({image.size})")
        image.load()
        if source_name == "wide.png" and image.width <= image.height:
            sys.exit(f"expected a landscape thumbnail for {source_name}: {path} ({image.size})")
        if source_name == "tall.jpg" and image.height <= image.width:
            sys.exit(f"expected a portrait thumbnail for {source_name}: {path} ({image.size})")


def main():
    state, pictures, cache = map(Path, sys.argv[1:4])
    images = set(sys.argv[4:])
    refused = {"corrupt.png", "logo.svg", "notes.txt", "program"}
    urls = completed_grid(state, pictures, images | refused)
    if set(urls) != images:
        sys.exit(f"Grid thumbnail URLs differ: expected {sorted(images)}, saw {sorted(urls)}")

    for name in sorted(images):
        key = digest((pictures / name).as_uri())
        url = urlsplit(urls[name])
        versions = parse_qs(url.query).get("gust", [])
        if url.scheme != "file" or url.netloc or len(versions) != 1:
            sys.exit(f"invalid Grid thumbnail URL for {name}: {urls[name]}")
        displayed = Path(unquote(url.path))
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
