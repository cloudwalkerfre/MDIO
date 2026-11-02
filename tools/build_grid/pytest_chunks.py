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
# 显式指定 pytest 配置：按文件列表调用时 rootdir 不再落到 mdio_py，
# 否则 asyncio_mode / markers / addopts 全部丢失（异步 fixture 当场报错）。
CONFIG = ROOT / "mdio_py" / "pyproject.toml"


def _pytest_cmd(args: list[str], files: list[str]) -> list[str]:
    cmd = [sys.executable, "-m", "pytest"]
    if CONFIG.is_file():
        cmd += ["-c", str(CONFIG)]
    return [*cmd, *args, *files]


def main() -> int:
    args = sys.argv[1:]
    try:
        chunks = int(os.environ.get("MDIO_PYTEST_CHUNKS", "1"))
    except ValueError:
        print("MDIO_PYTEST_CHUNKS must be an integer", file=sys.stderr)
        return 2

    files = sorted(
        str(p.relative_to(ROOT)) for p in TESTS.rglob("test_*.py")
    )
    if not files:
        print(f"no test files under {TESTS}", file=sys.stderr)
        return 2

    if chunks <= 1:
        return subprocess.run(_pytest_cmd(args, files), cwd=ROOT).returncode

    status = 0
    for index in range(chunks):
        bucket = files[index::chunks]
        if not bucket:
            continue
        print(f"[pytest-chunks] {index + 1}/{chunks}: {len(bucket)} file(s)", flush=True)
        code = subprocess.run(_pytest_cmd(args, bucket), cwd=ROOT).returncode
        if code != 0:
            status = code
    return status


if __name__ == "__main__":
    raise SystemExit(main())
