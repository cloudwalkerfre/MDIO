#!/usr/bin/env python3
"""Run the Python test suite in N sequential processes.

Why: macOS runners have a tight per-user thread limit, and engines created by
tests do not always release their native threads before the process exits.
One long pytest process accumulates threads until `spawn` starts failing
(EAGAIN) and hundreds of tests error at setup. Splitting the files across
several processes bounds the leaked threads per process; exiting reclaims them.

Usage: MDIO_PYTEST_CHUNKS=4 python tools/build_grid/pytest_chunks.py <pytest args>
Chunks default to 1 (a single pytest invocation, exactly the old behaviour).
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TESTS = ROOT / "mdio_py" / "tests"


def main() -> int:
    args = sys.argv[1:]
    try:
        chunks = int(os.environ.get("MDIO_PYTEST_CHUNKS", "1"))
    except ValueError:
        print("MDIO_PYTEST_CHUNKS must be an integer", file=sys.stderr)
        return 2
    if chunks <= 1:
        return subprocess.run([sys.executable, "-m", "pytest", *args], cwd=ROOT).returncode

    files = sorted(
        str(p.relative_to(ROOT)) for p in TESTS.rglob("test_*.py")
    )
    if not files:
        print(f"no test files under {TESTS}", file=sys.stderr)
        return 2

    status = 0
    for index in range(chunks):
        bucket = files[index::chunks]
        if not bucket:
            continue
        print(f"[pytest-chunks] {index + 1}/{chunks}: {len(bucket)} file(s)", flush=True)
        code = subprocess.run(
            [sys.executable, "-m", "pytest", *args, *bucket], cwd=ROOT
        ).returncode
        if code != 0:
            status = code
    return status


if __name__ == "__main__":
    raise SystemExit(main())
