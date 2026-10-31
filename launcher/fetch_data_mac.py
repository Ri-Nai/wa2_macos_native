#!/usr/bin/env python3
"""Fetch the WA2 data tree from the data-v1.1.0 release, macOS friendly.

Differences from launcher/fetch_data.py (which targets the Linux AppImage):

* downloads through a local HTTP proxy with parallel ranges, because a single
  stream from GitHub stalls at ~50 KB/s here while 8 ranges reach ~25 MB/s;
* extracts one part at a time and deletes it immediately, so the 45.6 GB of
  zips never sit on disk next to the whole 50.8 GB tree;
* skips the trees a 4x render never reads (bg_8x ~15 GB, sprite_8x ~7 GB,
  the 2x art siblings), which keeps a full install around 21 GB instead of 51.

Everything that is extracted still comes from the signed-off release: each part
is checked against the manifest SHA-256 before a byte of it is written.
"""

from __future__ import annotations

import json
import os
import sys
import time
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from prange import download  # noqa: E402

REPO = "MuRongxin/wa2-linux_native"
BASE = f"https://github.com/{REPO}/releases/download"

# Trees this install needs. 4x is the highest art the renderer decodes on a
# Retina display (a 3024x1964 drawable is a 4x canvas), so the 8x masters are
# dead weight; the 2x siblings are only a fallback for a broken 4x tree.
KEEP = {
    "script", "script_cn",
    "voice", "bgm", "se",
    "font",
    "bg",
    "bg_4x", "sprite_4x", "bg_cn_4x",
    "movie", "movie_2x",
}

# Parts that hold only trees in KEEP's complement, so they never need to be
# downloaded at all: parts 2-3 are bg_2x, 8-15 are the bg_8x masters, 18-19 are
# the 1440p movie remaster, 25-27 are the sprite_8x masters. Together that is
# about 25 GB of the 45.6 GB release. The boundaries were read out of each
# part's zip central directory (`data/<tree>/` entries), and a part is only
# listed here when *every* entry in it is unwanted - parts that straddle two
# trees (4, 7, 16, 17, 20-22, 24, 28) still get downloaded whole.
SKIP_PARTS = {
    "data-v1.1.0.part002.zip", "data-v1.1.0.part003.zip",
    "data-v1.1.0.part008.zip", "data-v1.1.0.part009.zip",
    "data-v1.1.0.part010.zip", "data-v1.1.0.part011.zip",
    "data-v1.1.0.part012.zip", "data-v1.1.0.part013.zip",
    "data-v1.1.0.part014.zip", "data-v1.1.0.part015.zip",
    "data-v1.1.0.part018.zip", "data-v1.1.0.part019.zip",
    "data-v1.1.0.part025.zip", "data-v1.1.0.part026.zip",
    "data-v1.1.0.part027.zip",
}


def extract(part: Path, data: Path, trees: dict[str, dict]) -> tuple[int, int]:
    files = total = 0
    with zipfile.ZipFile(part) as archive:
        for info in archive.infolist():
            parts = Path(info.filename).parts
            if len(parts) < 2 or parts[0] != "data":
                continue
            if parts[1] not in trees or parts[1] not in KEEP:
                continue
            target = data.joinpath(*parts[1:])
            if info.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(info) as src, open(target, "wb") as dst:
                while True:
                    block = src.read(1 << 22)
                    if not block:
                        break
                    dst.write(block)
            files += 1
            total += info.file_size
    return files, total


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    parts_dir = root / ".wa2-release-parts" / "data-v1.1.0"
    data = root / "data"
    manifest_path = parts_dir / "manifest.json"
    if not manifest_path.is_file():
        print(f"missing {manifest_path}; download it from the release first", file=sys.stderr)
        return 2
    manifest = json.loads(manifest_path.read_text())
    trees = manifest["inventory"]["trees"]
    data.mkdir(parents=True, exist_ok=True)

    wanted = [t for t in KEEP if t in trees]
    expect = sum(trees[t]["bytes"] for t in wanted)
    skipped_parts = [p for p in manifest["parts"] if p["name"] in SKIP_PARTS]
    skipped_bytes = sum(p["size"] for p in skipped_parts)
    print(f"extracting {len(wanted)} trees, {expect / 1e9:.1f} GB; "
          f"skipping {len(skipped_parts)} parts ({skipped_bytes / 1e9:.1f} GB) that hold no wanted tree")
    done_files = done_bytes = 0
    for index, part in enumerate(manifest["parts"], 1):
        name, sha = part["name"], part["sha256"]
        local = parts_dir / name
        if name in SKIP_PARTS:
            print(f"[{index}/{len(manifest['parts'])}] {name} - skipped", flush=True)
            continue
        print(f"[{index}/{len(manifest['parts'])}] {name}", flush=True)
        for attempt in range(3):
            try:
                download(f"{BASE}/data-v1.1.0/{name}", local, sha)
                break
            except RuntimeError as exc:
                if attempt == 2:
                    raise
                print(f"   retry after error: {exc}", flush=True)
                time.sleep(20 * (attempt + 1))
        files, nbytes = extract(local, data, trees)
        local.unlink()
        done_files += files
        done_bytes += nbytes
        print(f"   -> {files} files, {nbytes / 1e9:.2f} GB "
              f"(total {done_bytes / 1e9:.2f}/{expect / 1e9:.2f} GB, {done_files} files)", flush=True)
    print(f"done: {done_files} files, {done_bytes / 1e9:.2f} GB in {data}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
