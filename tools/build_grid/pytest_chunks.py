#!/usr/bin/env python3
"""Run the Python test suite in N sequential processes.

Why: macOS runners have a tight per-user thread limit, and engines created by
tests do not always release their native threads before the process exits. One
long pytest process accumulates threads until `spawn` starts failing (EAGAIN)
and hundreds of tests error at setup. Splitting the suite across several
processes bounds the leaked threads per process; exiting reclaims them.

Chunks are built from `pytest --collect-only`, so the pytest config still
decides what exists (norecursedirs / addopts / ignores) — globbing the tree
would pull in directories the project deliberately excludes.

Usage: MDIO_PYTEST_CHUNKS=4 python tools/build_grid/pytest_chunks.py <pytest args>
Chunks default to 1 (a single pytest invocation over mdio_py/tests).
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TESTS = ROOT / "mdio_py" / "tests"
# 显式指定 pytest 配置：按文件/节点列表调用时 rootdir 不再落到 mdio_py，
# 否则 asyncio_mode / markers / addopts 全部丢失（异步 fixture 当场报错）。
CONFIG = ROOT / "mdio_py" / "pyproject.toml"


def _pytest_cmd(args: list[str], targets: list[str]) -> list[str]:
    cmd = [sys.executable, "-m", "pytest"]
    if CONFIG.is_file():
        cmd += ["-c", str(CONFIG), "--rootdir", str(ROOT)]
    return [*cmd, *args, *targets]


def _collect(args: list[str]) -> list[str]:
    """Node ids of every test pytest would collect under mdio_py/tests."""
    out = subprocess.run(
        _pytest_cmd(["--collect-only", "-q"], [str(TESTS)]),
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stdout)
        sys.stderr.write(out.stderr)
        raise SystemExit(out.returncode)
    nodes: list[str] = []
    for line in out.stdout.splitlines():
        line = line.strip()
        if "::" in line and not line.startswith(("=", "no tests")):
            nodes.append(line)
    return nodes


def main() -> int:
    args = sys.argv[1:]
    try:
        chunks = int(os.environ.get("MDIO_PYTEST_CHUNKS", "1"))
    except ValueError:
        print("MDIO_PYTEST_CHUNKS must be an integer", file=sys.stderr)
        return 2

    if chunks <= 1:
        return subprocess.run(_pytest_cmd(args, [str(TESTS)]), cwd=ROOT).returncode

    nodes = _collect(args)
    if not nodes:
        print(f"no tests collected under {TESTS}", file=sys.stderr)
        return 2

    status = 0
    for index in range(chunks):
        bucket = nodes[index::chunks]
        if not bucket:
            continue
        print(f"[pytest-chunks] {index + 1}/{chunks}: {len(bucket)} test(s)", flush=True)
        code = subprocess.run(_pytest_cmd(args, bucket), cwd=ROOT).returncode
        if code != 0:
            status = code
    return status


if __name__ == "__main__":
    raise SystemExit(main())
