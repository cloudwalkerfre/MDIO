#!/usr/bin/env python3
"""Minimal declarative build grid runner."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


@dataclass
class Target:
    name: str
    run: str | None = None
    depends: list[str] = field(default_factory=list)
    env: dict[str, str] = field(default_factory=dict)
    cwd: str | None = None
    description: str = ""
    outputs: list[str] = field(default_factory=list)


def _target_from(name: str, raw: dict) -> Target:
    return Target(
        name=name,
        run=raw.get("run"),
        depends=list(raw.get("depends", [])),
        env={str(k): str(v) for k, v in raw.get("env", {}).items()},
        cwd=raw.get("cwd"),
        description=raw.get("description", ""),
        outputs=list(raw.get("outputs", [])),
    )


def _load(path: Path, prefix: str = "") -> dict[str, Target]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    grid = data.get("grid", {})
    local_prefix = str(grid.get("prefix", "")).strip(":")
    out: dict[str, Target] = {}
    for name, raw in data.get("targets", {}).items():
        full = ":".join(x for x in (local_prefix, name) if x)
        target = _target_from(full, raw)
        if local_prefix:
            target.depends = [
                dep if ":" in dep else f"{local_prefix}:{dep}"
                for dep in target.depends
            ]
        out[full] = target
    for inc in grid.get("includes", []):
        inc_path = (path.parent / inc).resolve()
        inc_data = tomllib.loads(inc_path.read_text(encoding="utf-8"))
        inc_prefix = str(inc_data.get("grid", {}).get("prefix", "")).strip(":")
        child = _load(inc_path, inc_prefix)
        for name, target in child.items():
            out[name] = target
    return out


def resolve(targets: dict[str, Target], name: str, stack: tuple[str, ...] = ()) -> list[Target]:
    if name in stack:
        raise SystemExit("dependency cycle: " + " -> ".join((*stack, name)))
    target = targets.get(name)
    if target is None:
        raise SystemExit("unknown target: " + name)
    plan: list[Target] = []
    for dep in target.depends:
        plan.extend(resolve(targets, dep, (*stack, name)))
    if target not in plan:
        plan.append(target)
    return plan


def main() -> int:
    ap = argparse.ArgumentParser(description="MDIO build grid")
    ap.add_argument("target", nargs="?", default="doctor")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    targets = _load(ROOT / "build_grid.toml")
    if args.list:
        for name in sorted(targets):
            desc = targets[name].description
            print(f"{name:20s} {desc}")
        return 0
    plan = resolve(targets, args.target)
    if args.dry_run or args.json:
        data = [{"target": t.name, "run": t.run, "depends": t.depends} for t in plan]
        print(json.dumps(data, indent=2, ensure_ascii=False) if args.json else "\n".join(t.name for t in plan))
        return 0
    for t in plan:
        print(f"[build_grid] {t.name}", flush=True)
        if not t.run:
            continue
        env = os.environ.copy()
        env.update(t.env)
        cwd = ROOT / t.cwd if t.cwd else ROOT
        proc = subprocess.run(t.run, shell=True, cwd=cwd, env=env)
        if proc.returncode != 0:
            print(f"[build_grid] FAILED {t.name} ({proc.returncode})", file=sys.stderr)
            return proc.returncode
    print("[build_grid] OK " + args.target)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
