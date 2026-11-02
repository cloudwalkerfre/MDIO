#!/usr/bin/env python3
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[2]
for p in [ROOT / "mdio_py/src/mdio_py/web", ROOT / "dist"]:
    shutil.rmtree(p, ignore_errors=True)
    print(f"removed {p}")
