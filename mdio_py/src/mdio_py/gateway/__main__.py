# __main__.py
import os
import sys
import warnings
import subprocess
from pathlib import Path

from mdio_py.core.runtime import build_python_cmd, get_python_perf_env

sys.path.append(str(Path(__file__).parent))

warnings.filterwarnings("ignore", message="Core Pydantic V1 functionality isn't compatible")

def main():
    from mdio_py.gateway.config import settings  # 自动加载 .env（MDIO_GATEWAY_HOST/PORT）

    # 运行时根（mdio 的家）由 mdio_py._env.runtime_root() 解析
    from mdio_py._env import runtime_root

    root = runtime_root()

    env = os.environ.copy()
    env.update(get_python_perf_env())
    # 直启默认 gateway；调用方已显式设置 MDIO_PROCESS_MODE 时保持不变。
    env.setdefault("MDIO_PROCESS_MODE", "gateway")
    # 落盘开关：MDIO_PERSIST 是 core 侧**唯一** truth source（`persist_enabled()`，
    # 缺省 false = 不写 WAL、不启 flush 线程，纯内存运行）。网关是长驻服务，它的写入必须回盘，
    # 否则「写 → 重启 → 回退」（实测：wt/.mds/ui.md 与 .wal 都不动）。
    # setdefault = 显式导出优先；测试/CLI 不经本文件，仍保持源码态「缺省不落盘」的纪律。
    env.setdefault("MDIO_PERSIST", "1")
    # GIL 控制由调用方设置 PYTHON_GIL=0（CPython 解释器启动时自读该环境变量，不依赖 mdio 代码）。
    # 注意：3.14t 上 granian 要求 GIL-off（否则拒绝启动），服务前调用方须设 PYTHON_GIL=0。
    cmd = build_python_cmd(
        "-m", "granian",
        "--interface", "asgi",
        "--host", settings.MDIO_GATEWAY_HOST,
        "--port", str(settings.MDIO_GATEWAY_PORT),
        "--workers", "1",
        "--loop", "uvloop",
        "mdio_py.gateway:app",
    )
    subprocess.run(cmd, cwd=root, env=env)

if __name__ == "__main__":
    main()
