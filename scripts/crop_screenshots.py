#!/usr/bin/env python3
"""Crop Playwright tour screenshots into feature-focused PNGs.

The Playwright tour writes full-viewport PNGs into docs/screenshots/raw/
alongside a crops.json manifest listing bounding boxes for the subregions
that illustrate each feature. This script reads that manifest and writes
the cropped PNGs to docs/screenshots/.

Each entry in crops.json is shaped:

    {
      "name":   "<output-basename-without-extension>",
      "source": "<filename under raw/>",
      "x": int, "y": int,
      "width": int, "height": int,
      "pad":    int
    }

Pad is added to every side and clamped to the source image bounds so the
cropped region keeps a little breathing room around the UI element.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from PIL import Image


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    raw_dir = repo_root / "docs" / "screenshots" / "raw"
    out_dir = repo_root / "docs" / "screenshots"

    manifest_path = raw_dir / "crops.json"
    if not manifest_path.exists():
        print(f"no manifest at {manifest_path}", file=sys.stderr)
        return 1

    entries = json.loads(manifest_path.read_text())
    out_dir.mkdir(parents=True, exist_ok=True)

    produced: set[str] = set()
    for entry in entries:
        src = raw_dir / entry["source"]
        if not src.exists():
            print(f"skip {entry['name']}: source {src.name} missing", file=sys.stderr)
            continue

        with Image.open(src) as image:
            if entry["width"] <= 0 or entry["height"] <= 0:
                print(f"skip {entry['name']}: empty box", file=sys.stderr)
                continue
            box = clamp_box(
                image.width,
                image.height,
                entry["x"],
                entry["y"],
                entry["width"],
                entry["height"],
                entry.get("pad", 0),
            )
            cropped = image.crop(box)
            target = out_dir / f"{entry['name']}.png"
            cropped.save(target, format="PNG", optimize=True)
            produced.add(target.name)
            print(f"{target.relative_to(repo_root)}  {box}")

    # Crops the manifest no longer names are left in place but called out,
    # so a tour that lost a step is visible instead of silently serving an
    # old image to the guide.
    stale = sorted(
        path.name
        for path in out_dir.glob("*.png")
        if path.name not in produced
    )
    for name in stale:
        print(f"stale (not produced this run): {name}", file=sys.stderr)

    return 0


def clamp_box(
    max_w: int,
    max_h: int,
    x: int,
    y: int,
    width: int,
    height: int,
    pad: int,
) -> tuple[int, int, int, int]:
    left = max(0, x - pad)
    top = max(0, y - pad)
    right = min(max_w, x + width + pad)
    bottom = min(max_h, y + height + pad)
    if right <= left or bottom <= top:
        return (0, 0, max_w, max_h)
    return (left, top, right, bottom)


if __name__ == "__main__":
    raise SystemExit(main())
