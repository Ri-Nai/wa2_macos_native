#!/usr/bin/env python3
"""Parallel range downloader used by fetch_data_mac.py.

GitHub Releases are slow from here on a single connection (it stalls around
50 KB/s) but much faster when the file is split into ranges. Each range is
fetched by its own curl process, and the bytes are written straight into the
destination file at the right offset - no per-range temporary files, so an
interrupted run never leaves hundreds of fragments behind.

Resume works at file granularity: a part is either complete (and its SHA-256
verified) or it is fetched again.
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
WORKERS = 16


def _head(url: str) -> int:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({"http": PROXY, "https": PROXY}))
    req = urllib.request.Request(url, method="HEAD")
    with opener.open(req, timeout=60) as r:
        return int(r.headers["Content-Length"])


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            block = f.read(1 << 22)
            if not block:
                break
            digest.update(block)
    return digest.hexdigest()


def _write_range(url: str, out: Path, start: int, end: int, progress: dict, lock: threading.Lock) -> None:
    want = end - start + 1
    env = dict(os.environ, http_proxy=PROXY, https_proxy=PROXY, all_proxy=PROXY)
    for attempt in range(8):
        written = 0
        proc = subprocess.Popen(
            ["curl", "-sSL", "--fail", "--max-time", "1800",
             "--retry", "3", "--retry-delay", "3",
             "-r", f"{start}-{end}", url],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        with open(out, "r+b") as f:
            f.seek(start)
            while True:
                block = proc.stdout.read(1 << 20)
                if not block:
                    break
                f.write(block)
                written += len(block)
                with lock:
                    progress["bytes"] += len(block)
        err = proc.stderr.read().decode(errors="replace").strip()
        code = proc.wait()
        if code == 0 and written == want:
            return
        with lock:
            progress["bytes"] -= written
        if attempt == 7:
            raise RuntimeError(f"range {start}-{end} failed (curl {code}): {err or 'short body'}")
        # GitHub starts answering 429/503 when too many ranges land at once;
        # backing off is what recovers, not retrying immediately.
        time.sleep(3 * (attempt + 1))


def download(url: str, out: Path, sha256: str | None = None) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        if sha256 is None or _sha256(out) == sha256:
            print(f"  cached {out.name}", flush=True)
            return
        out.unlink()

    total = _head(url)
    with open(out, "wb") as f:
        f.truncate(total)

    spans = [(s, min(s + CHUNK - 1, total - 1)) for s in range(0, total, CHUNK)]
    progress = {"bytes": 0}
    lock = threading.Lock()
    errors: list[BaseException] = []

    def work(s: int, e: int) -> None:
        try:
            _write_range(url, out, s, e, progress, lock)
        except BaseException as exc:  # noqa: BLE001
            with lock:
                errors.append(exc)

    started = time.monotonic()
    pending = list(spans)
    running: list[threading.Thread] = []
    last_print = 0.0
    while pending or running:
        while pending and len(running) < WORKERS:
            s, e = pending.pop(0)
            t = threading.Thread(target=work, args=(s, e), daemon=True)
            t.start()
            running.append(t)
        running = [t for t in running if t.is_alive()]
        if errors:
            raise errors[0]
        now = time.monotonic()
        if now - last_print >= 5:
            last_print = now
            done = min(progress["bytes"], total)
            rate = done / max(now - started, 0.001) / 1e6
            print(f"  {out.name}: {done / 1e6:8.1f}/{total / 1e6:.1f} MB  {rate:6.1f} MB/s", flush=True)
        time.sleep(0.1)
    if errors:
        raise errors[0]

    if out.stat().st_size != total:
        raise RuntimeError(f"{out.name}: expected {total} bytes, got {out.stat().st_size}")
    if sha256:
        actual = _sha256(out)
        if actual != sha256:
            raise RuntimeError(f"sha256 mismatch for {out.name}: got {actual}")


if __name__ == "__main__":
    download(sys.argv[1], Path(sys.argv[2]), sys.argv[3] if len(sys.argv) > 3 else None)
