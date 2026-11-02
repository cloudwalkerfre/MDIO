#!/usr/bin/env python3
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
web = ROOT / "mdio_py/src/mdio_py/web/client"
if not (web / "index.html").is_file():
    print(f"missing UI bundle: {web}/index.html", file=sys.stderr)
    raise SystemExit(1)
dist = ROOT / "dist"
dist.mkdir(exist_ok=True)
p = subprocess.run([
    "uv", "build", "--package", "mdio-py", "--wheel", "--out-dir", str(dist)
], cwd=ROOT)
if p.returncode:
    raise SystemExit(p.returncode)
wheels = sorted(dist.glob("mdio_py-*.whl"))
print(wheels[-1] if wheels else "wheel built")
