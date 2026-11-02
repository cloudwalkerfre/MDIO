#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def version(cmd: str) -> str:
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, text=True, capture_output=True)
        return (p.stdout or p.stderr).strip().splitlines()[0] if (p.stdout or p.stderr).strip() else "<empty>"
    except Exception as e:
        return f"<missing: {e}>"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ui", action="store_true")
    args = ap.parse_args()
    checks = {
        "python3": version("python3 --version"),
        "uv": version("uv --version"),
        "cargo": version("cargo --version"),
        "rustc": version("rustc --version"),
        "wasm-bindgen": version("wasm-bindgen --version"),
        "docker": version("docker --version"),
        "node": version("node --version"),
        "pnpm": version("pnpm --version"),
    }
    for name, value in checks.items():
        print(f"{name:14s} {value}")
    env = ROOT / ".node-version"
    want_node = env.read_text().strip() if env.exists() else "?"
    print(f"pinned node    {want_node}")
    pkg = ROOT / "mdio_ui/demo/package.json"
    text = pkg.read_text()
    m = re.search(r'"packageManager"\s*:\s*"pnpm@([^"]+)"', text)
    print(f"pinned pnpm    {m.group(1) if m else '?'}")
    missing = []
    if "missing" in checks["uv"]:
        missing.append("uv")
    if "missing" in checks["cargo"]:
        missing.append("cargo")
    if args.ui:
        if "missing" in checks["wasm-bindgen"]:
            missing.append("wasm-bindgen")
        if "missing" in checks["docker"]:
            missing.append("docker (ui:bundle)")
    if missing:
        print("MISSING: " + ", ".join(missing), file=sys.stderr)
        return 1
    if checks["node"] != "v" + want_node:
        print("WARN: host node differs from pinned version; ui:bundle uses fixed Docker node", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
