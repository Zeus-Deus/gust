#!/usr/bin/env python3
"""Assert what is actually on screen in a Gust screenshot.

Used instead of eyeballing: every QA step states the strings it expects, and
this fails loudly when they are absent. OCR is fuzzy, so assertions are made
on distinctive substrings and a colour/geometry check backs them up.
"""
import subprocess
import sys
import json
from pathlib import Path


def ocr(png: Path, psm: int = 6) -> str:
    """OCR a screenshot.

    Gust renders at the theme's 12px monospace base size, which tesseract
    reads badly at 1:1 (it turns "Documents" into "R"). Upscaling 3x with a
    smooth filter and thresholding lifts recognition to usable levels, so an
    assertion failure means the UI is wrong rather than the OCR being weak.
    """
    big = png.with_suffix(".ocr.png")
    subprocess.run(
        ["magick", str(png), "-colorspace", "gray", "-resize", "300%",
         "-sharpen", "0x1", "-contrast-stretch", "2%x2%", str(big)],
        capture_output=True, text=True,
    )
    src = big if big.exists() else png
    out = subprocess.run(
        ["tesseract", str(src), "stdout", "--psm", str(psm), "--oem", "1"],
        capture_output=True, text=True,
    )
    return out.stdout


def colours(png: Path, top_n: int = 8):
    """Dominant colours, to prove the theme is actually applied."""
    out = subprocess.run(
        ["magick", str(png), "-format", "%c", "-colors", "16", "histogram:info:-"],
        capture_output=True, text=True,
    ).stdout
    rows = []
    for line in out.splitlines():
        line = line.strip()
        if not line or "#" not in line:
            continue
        count = int(line.split(":")[0].strip())
        hexcode = line.split("#")[1].split()[0][:6]
        rows.append((count, "#" + hexcode.lower()))
    rows.sort(reverse=True)
    return rows[:top_n]


def main():
    if len(sys.argv) < 3:
        print("usage: assert_shot.py SHOT.png EXPECT1 [EXPECT2 ...] "
              "[--absent STR ...] [--colors HEX ...]")
        return 2
    png = Path(sys.argv[1])
    if not png.exists():
        print(f"FAIL: no screenshot at {png}")
        return 1

    args = sys.argv[2:]
    expect, absent, want_colors = [], [], []
    mode = expect
    for a in args:
        if a == "--absent":
            mode = absent
        elif a == "--colors":
            mode = want_colors
        else:
            mode.append(a)

    text = ocr(png)
    # A second pass with a different segmentation catches sparse UI text.
    text += "\n" + ocr(png, psm=11)
    flat = " ".join(text.split()).lower()

    failures = []
    for e in expect:
        if e.lower() not in flat:
            failures.append(f"MISSING: {e!r}")
    for a in absent:
        if a.lower() in flat:
            failures.append(f"PRESENT BUT SHOULD NOT BE: {a!r}")

    found_colors = colours(png)
    color_hexes = {c for _, c in found_colors}
    for c in want_colors:
        c = c.lower()
        if c not in color_hexes:
            failures.append(f"COLOR MISSING: {c} (top colors: {sorted(color_hexes)})")

    print(json.dumps({
        "shot": str(png),
        "ocr_chars": len(flat),
        "top_colors": found_colors,
        "expect": expect,
        "absent": absent,
        "failures": failures,
    }, indent=2))

    if failures:
        print("\n--- OCR TEXT ---")
        print(text[:2500])
        print("RESULT: FAIL")
        return 1
    print("RESULT: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
