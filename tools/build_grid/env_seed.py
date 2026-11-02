#!/usr/bin/env python3
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[2]
src = ROOT / ".env.example"
dst = ROOT / "mdio_py/src/mdio_py/env.example"
dst.parent.mkdir(parents=True, exist_ok=True)
shutil.copyfile(src, dst)
print(f"env.example -> {dst}")
