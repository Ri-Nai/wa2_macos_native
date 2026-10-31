#!/usr/bin/env python3
"""Parallel range downloader used by fetch_data_mac.py.

GitHub Releases are slow from here on a single connection (it stalls around
50 KB/s) but fast when the file is split into ranges: eight parallel HTTP
requests reach tens of MB/s. The ranges are driven through curl, which keeps
connection reuse and retry handling out of this script.

Each range lands in ``<out>.part<N>`` so an interrupted run resumes instead of
starting over; the pieces are concatenated and checked against the manifest
SHA-256 before the caller ever sees the file.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

PROXY = os.environ.get("WA2_PROXY", "http://127.0.0.1:7897")
CHUNK = 16 * 1024 * 1024
WORKERS = 6


def _head(url: str) -> int:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({"http": PROXY, "https": PROXY}))
    req = urllib.request.Request(url, method="HEAD")
    with opener.open(req, timeout=60) as r:
        return int(r.headers["Content-Length"])


def _fetch_range(url: str, start: int, end: int, target: Path) -> None:
    want = end - start + 1
    if target.exists() and target.stat().st_size == want:
        return
    env = dict(os.environ, http_proxy=PROXY, https_proxy=PROXY, all_proxy=PROXY)
    for attempt in range(10):
        proc = subprocess.run(
            ["curl", "-sSL", "--fail", "--max-time", "1200",
             "--retry", "3", "--retry-delay", "3",
             "-r", f"{start}-{end}", "-o", str(target), url],
            env=env, capture_output=True,
        )
        if proc.returncode == 0 and target.exists() and target.stat().st_size == want:
            return
        if attempt == 9:
            detail = proc.stderr.decode(errors="replace").strip() or f"curl exit {proc.returncode}"
            raise RuntimeError(f"range {start}-{end} of {url.rsplit('/', 1)[-1]} failed: {detail}")
        # GitHub starts answering 429/503 when too many ranges land at once;
        # backing off is what recovers, not retrying immediately.
        time.sleep(3 * (attempt + 1))


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            block = f.read(1 << 22)
            if not block:
                break
            digest.update(block)
    return digest.hexdigest()


def download(url: str, out: Path, sha256: str | None = None) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        if sha256 is None or _sha256(out) == sha256:
            print(f"  cached {out.name}", flush=True)
            return
        out.unlink()

    total = _head(url)
    spans = [(s, min(s + CHUNK - 1, total - 1)) for s in range(0, total, CHUNK)]
    pieces = [out.with_suffix(out.suffix + f".part{i}") for i in range(len(spans))]
    done = {"bytes": 0}
    for piece, (s, e) in zip(pieces, spans):
        if piece.exists() and piece.stat().st_size == e - s + 1:
            done["bytes"] += e - s + 1
    errors: list[BaseException] = []
    lock = threading.Lock()

    def work(i: int, s: int, e: int) -> None:
        try:
            _fetch_range(url, s, e, pieces[i])
            with lock:
                done["bytes"] += e - s + 1
        except BaseException as exc:  # noqa: BLE001
            with lock:
                errors.append(exc)

    started = time.monotonic()
    pending = list(enumerate(spans))
    running: list[threading.Thread] = []
    last_print = 0.0
    while pending or running:
        while pending and len(running) < WORKERS:
            i, (s, e) = pending.pop(0)
            t = threading.Thread(target=work, args=(i, s, e), daemon=True)
            t.start()
            running.append(t)
        running = [t for t in running if t.is_alive()]
        if errors:
            raise errors[0]
        now = time.monotonic()
        if now - last_print >= 5:
            last_print = now
            rate = done["bytes"] / max(now - started, 0.001) / 1e6
            print(f"  {out.name}: {done['bytes'] / 1e6:8.1f}/{total / 1e6:.1f} MB  {rate:6.1f} MB/s", flush=True)
        time.sleep(0.1)
    if errors:
        raise errors[0]

    with open(out, "wb") as dst:
        for piece in pieces:
            with open(piece, "rb") as src:
                while True:
                    block = src.read(1 << 22)
                    if not block:
                        break
                    dst.write(block)
            piece.unlink()

    if sha256:
        actual = _sha256(out)
        if actual != sha256:
            out.unlink()
            raise RuntimeError(f"sha256 mismatch for {out.name}: got {actual}")


if __name__ == "__main__":
    download(sys.argv[1], Path(sys.argv[2]), sys.argv[3] if len(sys.argv) > 3 else None)
