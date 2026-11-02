#!/usr/bin/env python3
from __future__ import annotations

import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TMP = Path("/tmp/mdio_grid_verify")
VENV = TMP / "venv"
HOME = TMP / "home"
PORT = 18092


def http(path: str, timeout: float = 3.0) -> tuple[int, bytes]:
    with urllib.request.urlopen(f"http://127.0.0.1:{PORT}{path}", timeout=timeout) as r:
        return int(r.status), r.read()


def main() -> int:
    wheels = sorted((ROOT / "dist").glob("mdio_py-*.whl"), key=lambda p: p.stat().st_mtime)
    if not wheels:
        print("no wheel in dist/", file=sys.stderr)
        return 1
    wheel = wheels[-1]
    shutil.rmtree(TMP, ignore_errors=True)
    TMP.mkdir(parents=True)
    subprocess.run(["uv", "venv", str(VENV), "--python", "3.14"], check=True)
    subprocess.run(["uv", "pip", "install", "--python", str(VENV / "bin/python"), str(wheel)], check=True)

    genesis = HOME / "genesis"
    genesis.mkdir(parents=True)
    shutil.copytree(ROOT / "mdio_py/tests/fixtures/genesis/wt", genesis / "wt")
    (genesis / "wt/README.md").write_text("# GENESIS\n- name: grid-verify\n", encoding="utf-8")

    env = os.environ.copy()
    env["MDIO_HOME"] = str(HOME)
    env["MDIO_GATEWAY_PORT"] = str(PORT)
    log = TMP / "gateway.log"
    proc = subprocess.Popen(
        [str(VENV / "bin/mdio"), "serve", "--port", str(PORT)],
        env=env, cwd=str(ROOT), stdout=log.open("wb"), stderr=subprocess.STDOUT,
    )
    try:
        deadline = time.time() + 30
        while time.time() < deadline:
            try:
                code, _ = http("/control/status")
                if code == 200:
                    break
            except Exception:
                pass
            if proc.poll() is not None:
                raise RuntimeError("gateway exited during boot")
            time.sleep(0.5)
        else:
            raise RuntimeError("gateway did not become ready")
        code, html = http("/")
        if code != 200 or b"<html" not in html.lower():
            raise RuntimeError(f"root UI failed: {code}")
        refs = re.findall(rb'href="(/demo/[^"]+\.css)"', html)
        if not refs:
            raise RuntimeError("index.html has no /demo/*.css asset")
        for ref in refs[:2]:
            c, _ = http(ref.decode())
            if c != 200:
                raise RuntimeError(f"asset {ref!r} -> {c}")
        refs2 = re.findall(rb'href="(/ui_base/[^"]+\.css)"', html)
        if not refs2:
            raise RuntimeError("index.html has no /ui_base/*.css asset")
        c, _ = http(refs2[0].decode())
        if c != 200:
            raise RuntimeError(f"ui_base asset -> {c}")
        print(f"verified {wheel.name}")
        print(f"gateway: http://127.0.0.1:{PORT}  UI/assets 200")
        print("genesis: " + str(genesis))
        return 0
    finally:
        subprocess.run([str(VENV / "bin/mdio"), "stop", "--force"], env=env, cwd=str(ROOT))
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        print("--- gateway log tail ---")
        print(log.read_text(errors="replace")[-2000:])


if __name__ == "__main__":
    raise SystemExit(main())
