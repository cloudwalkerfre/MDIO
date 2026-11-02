"""实例 .env → 进程环境变量的兜底注入层。

背景:MDIO_PERSIST 等环境变量由 Rust 侧(标准环境)读取;但部署常态
(uv tool install 的 console script / systemd / 直跑)不经 just(dotenv-load)
也不经 uv run(默认读 .env),需要本层把实例配置文件注入 os.environ。
幂等:os.environ.setdefault——shell 显式导出优先级最高,不覆盖。
仅标准库(零新依赖)。

规范位置与回退链:
  · 实例级真配置 = `$MDIO_HOME/.env`
      安装态 = `~/.mdio/.env`；源码态 = `<仓库根>/.env`（见 mdio_home()）
  · 回退链（逐键 setdefault，先到先得）:
      ① `$MDIO_HOME/.env`
      ② `<runtime_root>/.env`（安装态 = `$MDIO_HOME/genesis/.env`）
      ③ 打包样板 `mdio_py/env.example`（随 wheel 安装；全部注释 ⇒ 注入零变化）
  · 安装态**首跑**：`$MDIO_HOME/.env` 不存在且存在打包样板 ⇒ 复制一份过去
    （仅当不存在；`MDIO_ENV_NO_SEED=1` 关闭），使生产也有一份可编辑的真配置。
  · 可观测：`env_sources_report()` 返回解析账；`MDIO_ENV_DEBUG=1` 时把账打到 stderr。
    （命中不到任何文件时本层是空操作：不注入、不报错。）

持久化默认策略:源码/开发测试态默认关(Rust 侧缺省 false,
测试不污染);安装态(build/wheel 装进 site-packages)默认开——MDIO_PERSIST=1。
显式导出 MDIO_PERSIST=0/1 优先级最高。
"""

import os
import shutil
import sys
from pathlib import Path


def mdio_home() -> Path:
    """mdio 的**地盘**（HOME）：环境变量 `MDIO_HOME`；安装态 = `~/.mdio`；源码态 = 仓库根。
    注意:本文件层级与 config.py 不同,parents 深度按本文件计算。

    命名：这里返回的是**运行时自己的家**，不是「用户的项目」。
    mdio 里「项目」有两个不同坐标，别再拿 `project_root` 一个词混指：
      · 用户项目根（身份）  = worktrees.md 的 `path` 字段（= core 的 external_root，永久不变）
      · 该 wt 的文档根      = 项目根/wt（core 的挂载根）；实现根 = 项目根/plugin；运行态根 = 项目根/.wal
    本函数是定位逻辑的**唯一实现**：gateway/config.py 与 gateway/__main__.py 都复用它。
    """
    env_home = os.environ.get("MDIO_HOME")
    if env_home:
        return Path(env_home)
    _file = Path(__file__).resolve()
    if "site-packages" in str(_file):
        return Path.home() / ".mdio"
    return _file.parents[3]  # <repo>/mdio_py/src/mdio_py/_env.py → 仓库根


def _installed() -> bool:
    """本模块是否来自安装态（site-packages）。"""
    return "site-packages" in str(Path(__file__).resolve())


def ensure_mdio_home() -> None:
    """安装动作:确保地盘(装态 = ~/.mdio)存在。已存在不覆盖,幂等。"""
    try:
        mdio_home().mkdir(parents=True, exist_ok=True)
    except OSError:
        pass


def runtime_root() -> Path:
    """**根 wt（genesis）的根** —— mdio 启动即启动 genesis，所以「运行时根」就是它的根：
      · 安装态: <home>/genesis    （= ~/.mdio/genesis，与目标布局一致）
      · 源码态: <home>/workspace  （可由 MDIO_WORKSPACE_ROOT 覆盖）
    它不是「某个 wt 的地址」的通用说法 —— mdio 可以 load 多个 wt，各自有自己的项目根；
    它也不是地盘本身（地盘见 mdio_home()）。
    """
    env = os.environ.get("MDIO_WORKSPACE_ROOT")
    if env:
        p = Path(env).expanduser()
        if not p.is_absolute():
            # .env 里的 ./workspace 必须锚定到实例地盘，而不是进程 cwd。
            p = (mdio_home() / p).resolve()
        return p
    home = mdio_home()
    install = _installed()
    return home / ("genesis" if install else "workspace")


def ensure_runtime_root() -> None:
    """确保根 wt 的根存在（幂等）。"""
    try:
        runtime_root().mkdir(parents=True, exist_ok=True)
    except OSError:
        pass


# 兼容别名：project_root 指向 mdio_home() 的值（脚下=地盘）
project_root = mdio_home
ensure_project_root = ensure_mdio_home


def package_env_example() -> Path:
    """打包样板路径（随 wheel 安装；源码树里由 build.sh 投放，通常不存在）。"""
    return Path(__file__).resolve().parent / "env.example"


def env_source_paths() -> list[Path]:
    """回退链的三个候选文件（顺序即优先级；不保证存在）。"""
    return [mdio_home() / ".env", runtime_root() / ".env", package_env_example()]


# 解析账：env_sources_report() 读它（最近一次 load_project_env 的结果）。
_REPORT: dict = {
    "home": None,
    "runtime_root": None,
    "package_example": None,
    "seeded": None,
    "loaded": [],
}


def env_sources_report() -> dict:
    """返回最近一次 env 解析账：命中哪些文件、各注入几个键、是否落地样板。

    用途：诊断"配置到底有没有生效"。
    """
    return dict(_REPORT)


def _inject_file(path: Path) -> int:
    """逐行解析并 setdefault 注入；返回实际注入的键数（已存在的键不覆盖、不计入）。"""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return 0
    n = 0
    for line in lines:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip()
        if not key:
            continue
        # 剥离成对引号
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        if key in os.environ:
            continue
        os.environ[key] = value
        n += 1
    return n


def _seed_home_env() -> Path | None:
    """安装态首跑：把打包样板落地为 `$MDIO_HOME/.env`（仅当不存在）。

    落地的是**全注释样板**（注入零变化），只为让生产有一份可编辑的真配置，
    与 `scripts/public_release/export.sh` 的".env.example 随快照"约定一致。
    `MDIO_ENV_NO_SEED=1` 可关闭。
    """
    if not _installed() or os.environ.get("MDIO_ENV_NO_SEED") == "1":
        return None
    dest = mdio_home() / ".env"
    src = package_env_example()
    if dest.exists() or not src.is_file():
        return None
    try:
        shutil.copyfile(src, dest)
        return dest
    except OSError:
        return None


def load_project_env() -> None:
    """安装动作 + 环境注入:建地盘(mdio_home),再按回退链 setdefault 注入。

    幂等:已存在的键不覆盖;os.environ.setdefault 语义——shell 显式导出优先级最高。
    解析账写入 env_sources_report();`MDIO_ENV_DEBUG=1` 时同时打到 stderr。
    """
    ensure_mdio_home()
    # build/安装态默认持久化(开发测试态的"默认关"仅源码态成立);
    # 显式导出 MDIO_PERSIST=0 仍可关闭——setdefault 不覆盖。
    if _installed():
        os.environ.setdefault("MDIO_PERSIST", "1")

    seeded = _seed_home_env()
    loaded = []
    for path in env_source_paths():
        n = _inject_file(path) if path.is_file() else 0
        loaded.append({"path": str(path), "exists": path.is_file(), "injected": n})

    _REPORT.update({
        "home": str(mdio_home()),
        "runtime_root": str(runtime_root()),
        "package_example": str(package_env_example()),
        "seeded": str(seeded) if seeded else None,
        "loaded": loaded,
    })

    if os.environ.get("MDIO_ENV_DEBUG"):
        hits = ", ".join(f"{Path(x['path']).name}:{x['injected']}" for x in loaded if x["exists"]) or "无"
        seed_note = f" seeded={seeded}" if seeded else ""
        print(f"[env] home={mdio_home()} 命中={hits}{seed_note}", file=sys.stderr)
