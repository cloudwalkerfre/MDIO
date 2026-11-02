#!/usr/bin/env python3
"""Run the Python test suite in N sequential processes.

Why: macOS runners cap a process at roughly `kern.num_threads / 5` threads
(~2047 on the hosted VMs), and every engine instance keeps about six native
threads alive until its loop is closed. One long pytest process therefore runs
out of threads around 450 tests and later setups die with EAGAIN. Splitting the
suite into several processes bounds the threads per process; exiting reclaims
them.

Chunks are built from `pytest --collect-only`, so the pytest config still
decides what exists (norecursedirs / addopts / ignores). Tests are grouped by
file and bins are balanced by collected test count, then each chunk is run as a
plain `pytest <files...>` invocation.

Usage: MDIO_PYTEST_CHUNKS=10 python tools/build_grid/pytest_chunks.py <pytest args>
Chunks default to 1 (one pytest invocation over mdio_py/tests).
"""
from __future__ import annotations

import os
import subprocess
import sys
from collections import OrderedDict
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


def _collect_tests() -> list[str]:
    """Node ids of every test pytest collects under mdio_py/tests."""
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
    return [
        line.strip()
        for line in out.stdout.splitlines()
        if "::" in line and not line.strip().startswith(("=", "no tests"))
    ]


def _file_bins(nodes: list[str], chunks: int) -> list[list[str]]:
    """Group files into `chunks` bins, balanced by collected test count."""
    counts: OrderedDict[str, int] = OrderedDict()
    for node in nodes:
        path = node.split("::", 1)[0]
        counts[path] = counts.get(path, 0) + 1

    bins: list[list[str]] = [[] for _ in range(chunks)]
    loads = [0] * chunks
    for path, count in counts.items():
        index = loads.index(min(loads))
        bins[index].append(path)
        loads[index] += count
    return bins


def main() -> int:
    args = sys.argv[1:]
    try:
        chunks = int(os.environ.get("MDIO_PYTEST_CHUNKS", "1"))
    except ValueError:
        print("MDIO_PYTEST_CHUNKS must be an integer", file=sys.stderr)
        return 2

    if chunks <= 1:
        return subprocess.run(_pytest_cmd(args, [str(TESTS)]), cwd=ROOT).returncode

    nodes = _collect_tests()
    if not nodes:
        print(f"no tests collected under {TESTS}", file=sys.stderr)
        return 2
    print(f"[pytest-chunks] collected {len(nodes)} test(s)", flush=True)

    status = 0
    for index, files in enumerate(_file_bins(nodes, chunks), start=1):
        if not files:
            continue
        print(f"[pytest-chunks] {index}/{chunks}: {len(files)} file(s)", flush=True)
        code = subprocess.run(_pytest_cmd(args, files), cwd=ROOT).returncode
        if code != 0:
            status = code
    return status


if __name__ == "__main__":
    raise SystemExit(main())
