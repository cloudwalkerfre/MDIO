#!/usr/bin/env python3
"""Build the mdio DuckDB v2 extension as part of the normal project build.

This is the single supported build path: it compiles the mdio_frame crate
with cargo and appends the official DuckDB extension metadata
using the vendored extension-ci-tools script.
"""
from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT_DIR = ROOT / "mdio_core" / "ext" / "arrow"
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or (ROOT / "target"))
EXT_VERSION = os.environ.get("EXT_VERSION", "0.1.0")
DUCKDB_VERSION = os.environ.get("DUCKDB_VERSION", "v2.0.0")

system = platform.system()
machine = platform.machine()
if system == "Darwin" and machine == "arm64":
    platform_tag, dylib_ext = "osx_arm64", "dylib"
elif system == "Darwin":
    platform_tag, dylib_ext = "osx_amd64", "dylib"
elif system == "Linux" and machine in ("aarch64", "arm64"):
    platform_tag, dylib_ext = "linux_arm64", "so"
elif system == "Linux":
    platform_tag, dylib_ext = "linux_amd64", "so"
else:
    raise SystemExit(f"unsupported platform: {system}-{machine}")

env = os.environ.copy()
if system == "Darwin":
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") +
                        " -C link-args=-undefined -C link-args=dynamic_lookup").strip()
env.setdefault("CARGO_TARGET_DIR", str(TARGET))

subprocess.run(["cargo", "build", "--release", "--manifest-path", "mdio_core/ext/arrow/Cargo.toml"], cwd=ROOT, env=env, check=True)

built = TARGET / "release" / f"libmdio_frame.{dylib_ext}"

# DuckDB metadata is appended after the linker signature. On macOS, appending
# bytes invalidates a signature that was created after the append (codesign
# strict validation rejects the trailing metadata), but a signature created on
# the raw dylib before the append remains valid for the loadable pages. Sign a
# copy first, then append metadata to that signed copy.
library_for_metadata = built
if system == "Darwin":
    library_for_metadata = TARGET / "release" / f"libmdio_frame_signed.{dylib_ext}"
    shutil.copy2(built, library_for_metadata)
    subprocess.run(["codesign", "--force", "--sign", "-", str(library_for_metadata)], check=True)

out = OUT_DIR / "mdio_frame.duckdb_extension"
OUT_DIR.mkdir(parents=True, exist_ok=True)
subprocess.run([
    sys.executable,
    str(ROOT / "tools" / "append_extension_metadata.py"),
    "-l", str(library_for_metadata),
    "-o", str(out),
    "-n", "mdio_frame",
    "-dv", DUCKDB_VERSION,
    "-ev", EXT_VERSION,
    "-p", platform_tag,
    "--abi-type", "C_STRUCT_UNSTABLE",
], check=True)

pkg_ext = ROOT / "mdio_py" / "src" / "mdio_py" / "ext"
pkg_ext.mkdir(parents=True, exist_ok=True)
shutil.copy2(out, pkg_ext / "mdio_frame.duckdb_extension")
print(f"built: {out}")
print(f"packaged: {pkg_ext / 'mdio_frame.duckdb_extension'}")
