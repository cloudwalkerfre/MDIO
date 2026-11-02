import asyncio
import atexit
import faulthandler
import os
import signal
import sys
import threading
import time
import traceback
import types
import warnings
from pathlib import Path

import pytest


# ── 测试基础设施追踪（仅 MDIO_PYTEST_TRACE=1 时启用）─────────────────
# 目标：xdist/PTY 偶发收尾死锁时，留下“最后执行的测试 + 全线程栈”。
_TRACE_ENABLED = os.environ.get("MDIO_PYTEST_TRACE") == "1"
_SIGDUMP_ENABLED = os.environ.get("MDIO_PYTEST_SIGDUMP") == "1"
_TRACE_DIR = Path(os.environ.get("MDIO_PYTEST_TRACE_DIR", "/tmp/mdio_pytest_trace"))
_TRACE_LOCK = threading.Lock()
_TRACE_FH = None
_TRACE_LAST = 0.0
_TRACE_STOP = threading.Event()
_TRACE_WATCHDOG = None
_TRACE_STALLS = 0
_TRACE_SIGNAL_FH = None


def _trace(msg: str) -> None:
    global _TRACE_LAST
    if not _TRACE_ENABLED:
        return
    now = time.monotonic()
    with _TRACE_LOCK:
        _TRACE_LAST = now
        if _TRACE_FH is None:
            return
        try:
            _TRACE_FH.write(
                f"{time.strftime('%H:%M:%S')} pid={os.getpid()} "
                f"worker={os.environ.get('PYTEST_XDIST_WORKER', 'master')} "
                f"{msg}\n"
            )
            _TRACE_FH.flush()
        except Exception:
            pass


def _dump_all_threads(reason: str) -> None:
    global _TRACE_STALLS
    if not _TRACE_ENABLED:
        return
    _TRACE_STALLS += 1
    path = _TRACE_DIR / f"stall-{os.getpid()}-{_TRACE_STALLS:02d}-{reason}.txt"
    try:
        _TRACE_DIR.mkdir(parents=True, exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(f"reason={reason} pid={os.getpid()} ts={time.time()}\n")
            f.write("threads:\n")
            for t in threading.enumerate():
                f.write(
                    f"  name={t.name!r} ident={t.ident} daemon={t.daemon} alive={t.is_alive()}\n"
                )
            f.write("\nfaulthandler dump:\n")
            faulthandler.dump_traceback(file=f, all_threads=True)
            f.flush()
            os.fsync(f.fileno())
        _trace(f"STALL DUMP {path}")
    except Exception:
        _trace(f"STALL DUMP FAILED {path}: {traceback.format_exc()}")


def _watchdog() -> None:
    while not _TRACE_STOP.wait(5.0):
        with _TRACE_LOCK:
            last = _TRACE_LAST
        if last and (time.monotonic() - last) > 60.0:
            _dump_all_threads("no-activity-60s")
            with _TRACE_LOCK:
                globals()["_TRACE_LAST"] = time.monotonic()


def _trace_init(config) -> None:
    global _TRACE_FH, _TRACE_LAST, _TRACE_WATCHDOG, _TRACE_SIGNAL_FH
    if (not _TRACE_ENABLED and not _SIGDUMP_ENABLED) or _TRACE_FH is not None:
        return
    _TRACE_DIR.mkdir(parents=True, exist_ok=True)
    if _SIGDUMP_ENABLED:
        _TRACE_SIGNAL_FH = open(
            _TRACE_DIR / f"signal-{os.getpid()}.txt", "w", encoding="utf-8", buffering=1
        )
        try:
            faulthandler.register(
                signal.SIGUSR1,
                file=_TRACE_SIGNAL_FH,
                all_threads=True,
                chain=False,
            )
        except Exception:
            pass
    if not _TRACE_ENABLED:
        return
    _TRACE_FH = open(
        _TRACE_DIR / f"trace-{os.getpid()}-{os.environ.get('PYTEST_XDIST_WORKER', 'master')}.log",
        "w",
        encoding="utf-8",
        buffering=1,
    )
    _TRACE_LAST = time.monotonic()
    # GIL 死锁时 watchdog 线程无法运行；SIGUSR1 由 C faulthandler 直接落栈。
    _TRACE_SIGNAL_FH = open(
        _TRACE_DIR / f"signal-{os.getpid()}.txt", "w", encoding="utf-8", buffering=1
    )
    try:
        faulthandler.register(
            signal.SIGUSR1,
            file=_TRACE_SIGNAL_FH,
            all_threads=True,
            chain=False,
        )
    except Exception:
        pass
    _TRACE_WATCHDOG = threading.Thread(
        target=_watchdog, name="mdio-pytest-trace-watchdog", daemon=True
    )
    _TRACE_WATCHDOG.start()
    _trace(f"INIT argv={sys.argv!r} threads={len(threading.enumerate())}")
    atexit.register(lambda: _trace("ATEXIT"))
    try:
        threading._register_atexit(lambda: _trace("THREADING_ATEXIT"))
    except Exception:
        pass


def _trace_shutdown(reason: str) -> None:
    if not _TRACE_ENABLED:
        return
    _trace(f"SHUTDOWN {reason} threads={len(threading.enumerate())}")
    _dump_all_threads(reason)
    _TRACE_STOP.set()
    if _TRACE_WATCHDOG is not None and _TRACE_WATCHDOG.is_alive():
        _TRACE_WATCHDOG.join(timeout=2.0)
    with _TRACE_LOCK:
        try:
            if _TRACE_FH is not None:
                _TRACE_FH.flush()
        except Exception:
            pass


def _trace_hook(name: str, item=None):
    suffix = f" {item.nodeid}" if item is not None else ""
    _trace(f"{name}{suffix}")


def pytest_sessionstart(session):
    _trace(f"SESSION_START root={session.config.rootpath}")


def pytest_collection_finish(session):
    _trace(f"COLLECTION_FINISH items={len(session.items)}")


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_setup(item):
    _trace_hook("SETUP_START", item)
    yield
    _trace_hook("SETUP_END", item)


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_call(item):
    _trace_hook("CALL_START", item)
    yield
    _trace_hook("CALL_END", item)


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_teardown(item, nextitem):
    _trace_hook("TEARDOWN_START", item)
    yield
    _trace_hook("TEARDOWN_END", item)


def pytest_runtest_logstart(nodeid, location):
    _trace(f"LOGSTART {nodeid}")


def pytest_runtest_logfinish(nodeid, location):
    _trace(f"LOGFINISH {nodeid}")


def pytest_sessionfinish(session, exitstatus):
    _trace(f"SESSION_FINISH exitstatus={exitstatus} threads={len(threading.enumerate())}")


def pytest_unconfigure(config):
    _trace_shutdown("pytest_unconfigure")

# omp 测试隔离：cf 基线兜底
# 生产根 .env 写入 MDIO_PROJ_MODE=hs 时，测试进程默认也应保持 cf 基线
# （避免 .env 污染判官）；真实进程 env 优先（setdefault 不覆盖），
# env-hs 判官显式注入仍可压过兜底。必须在任何 import mdio_py 之前执行
# （load_project_env 也是 setdefault——两者共存：谁先谁打底）。
os.environ.setdefault("MDIO_PROJ_MODE", "cf")

# 后端为 any-llm，backend.py 惰性 import 它；测试进程不为 LLM 依赖付导入成本，无需 stub。

import pytest
from mdio_py import MdioRuntime
from mdio_py.std.worktree_ops import _ckpt_text_cache


@pytest.fixture(autouse=True)
def _xbt_no_default_driver(monkeypatch):
    """XBT 生产默认 driver 是 XBT_DIRECTOR；引擎测试默认按“无 driver”基线跑。
    需要验证默认 driver 的用例自己 monkeypatch 回去。"""
    from mdio_py.plugins.xbt import stepper as xbt_stepper
    monkeypatch.setattr(xbt_stepper, "DEFAULT_DRIVER", "")
    yield


@pytest.fixture(autouse=True)
def _persist_marker_env(request):
    """AP:按 pytest.mark.persist 分面设置 MDIO_PERSIST——
    持久化断言系测试(需 env=1)与缺省系(隔离语义,需 env 空)在同一进程共存。
    每个测试执行前按 marker 设好,执行后恢复(无泄漏到其它测试)。"""
    if request.node.get_closest_marker("persist"):
        os.environ["MDIO_PERSIST"] = "1"
    else:
        os.environ.pop("MDIO_PERSIST", None)
    yield
    os.environ.pop("MDIO_PERSIST", None)


@pytest.fixture(autouse=True)
def _clear_ckpt_cache():
    """Clear the module-level ckpt text cache between tests to prevent
    state leakage across temp directories."""
    _ckpt_text_cache.clear()
    yield
    _ckpt_text_cache.clear()


# Telemetry: pytest --telemetry[=bench] 全局开关

def pytest_addoption(parser):
    parser.addoption("--telemetry", nargs="?", const="trace", default=None,
                     choices=["trace", "bench", "auto"],
                     help="Enable telemetry for all tests")

_tel_mode = None

def pytest_configure(config):
    global _tel_mode
    _tel_mode = config.getoption("--telemetry") or os.environ.get("MDIO_TELEMETRY")
    config.addinivalue_line(
        "markers", "live: 需要真模型（MDIO_LIVE_LLM=1）——离线套件里由 live_llm 夹具跳过")
    config.addinivalue_line(
        "markers", "perf: 性能基准（默认跳过；MDIO_RUN_PERF=1 才跑）")
    _trace_init(config)


def _llm_secret_source() -> str:
    """返回真模型测试可用的 secret 来源；没有则返回空串。

    只看测试唯一 genesis fixture 的 llm_config.md：
    - api_key: env:NAME → 只有 NAME 在环境里才算有 secret；
    - api_key: literal → 有值即算；
    - fixture 本身不带 secret。
    """
    root = Path(__file__).resolve().parent
    candidates = [
        root / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md",
    ]
    for path in candidates:
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        for line in text.splitlines():
            stripped = line.strip()
            if not stripped.startswith("- api_key:"):
                continue
            value = stripped.split(":", 1)[1].strip()
            if not value:
                continue
            if value.startswith("env:"):
                env_name = value[4:].strip()
                if env_name and os.environ.get(env_name, "").strip():
                    return f"{path}:env:{env_name}"
                continue
            return f"{path}:literal"
    return ""


@pytest.fixture
def live_llm():
    """真模型测试的前置：要求 MDIO_LIVE_LLM=1 且存在可用 secret。

    无 secret 时不报错：warn 提醒后 skip，避免 clean CI 因缺凭据红。
    """
    if not os.environ.get("MDIO_LIVE_LLM"):
        pytest.skip("真模型测试：需 MDIO_LIVE_LLM=1")
    if not _llm_secret_source():
        warnings.warn(
            "LLM live tests skipped: no API secret found in llm_config.md "
            "(use api_key: env:NAME and export NAME, or provide a local config)",
            UserWarning,
            stacklevel=2,
        )
        pytest.skip("真模型测试：未检测到可用 LLM secret（已提醒，不报错）")
    return True

@pytest.fixture(autouse=True)
def _tel_session(request):
    if not _tel_mode:
        yield; return
    rt = None
    for name in ["runtime", "runtime_std", "runtime_ls", "runtime_mem",
                 "runtime_pid", "runtime_llm"]:
        if name in request.fixturenames:
            rt = request.getfixturevalue(name)
            break
    if rt is None:
        yield; return
    from mdio_py.telemetry import TelemetrySession
    sess = TelemetrySession(
        rt.store, request.node.name[:40].replace("/", "_"),
        mode="bench" if _tel_mode == "bench" else "trace")
    yield
    sess.report()


@pytest.fixture
async def runtime(tmp_path):
    """Ephemeral MdioRuntime backed by a temp directory.

    Async fixture — MdioRuntime needs an event loop during __init__.
    Teardown 显式 close（BUG-H）：_hook_consumer 常驻 Task + Rust 侧
    dispatcher Py 引用保活 store → 6 worker 线程永不退出 → 反复创建
    runtime 后线程耗尽。close() 切断保活链，线程随 channel 关闭退出。
    """
    rt = MdioRuntime(str(tmp_path))
    yield rt
    rt.close()
    # 让事件循环处理 cancel 的消费协程（消除 "Task was destroyed" 噪音）
    try:
        await asyncio.sleep(0)
    except Exception:
        pass


@pytest.fixture
def workdir(tmp_path):
    """Convenience alias — the temp directory as a Path."""
    return tmp_path


@pytest.fixture
def wt_uid():
    """Default wt_uid — genesis uid string."""
    return "genesis"

@pytest.fixture
def uid():
    """Default WT uid — genesis."""
    return "genesis"


@pytest.fixture
def sandbox_id():
    """Default sandbox wt_uid — genesis uid string."""
    return "genesis"


# pre-seeded runtimes


@pytest.fixture(autouse=True)
def _ensure_wt_dir(tmp_path):
    """文档根恒为 `<root>/wt`（定则）：每个用例预建 wt/，夹具写文档可直接落进去。"""
    (tmp_path / "wt").mkdir(exist_ok=True)


@pytest.fixture
async def runtime_std(tmp_path):
    """Runtime with test_std_test files pre-populated（文档根恒为 <root>/wt）。"""
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "config.md").write_text("# CONFIG\n- DB_HOST: localhost\n- PORT: 5432\n- EMPTY_KEY: \n")
    (tmp_path / "wt" / "data.md").write_text("# DATA\n- LOGS: \n    error1\n    error2\n")
    (tmp_path / "wt" / "empty.md").write_text("")
    (tmp_path / "wt" / "ro.md").write_text("# RO\n- readonly: yes")
    (tmp_path / "wt" / "sub").mkdir()
    (tmp_path / "wt" / "sub" / "nested.md").write_text("# NESTED\n- value: inside")
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('mk -f /emptydir/', wt_uid="genesis")
    return rt


@pytest.fixture
async def runtime_ls(tmp_path):
    """Runtime with test_ls files pre-populated（文档根恒为 <root>/wt）。"""
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "a.md").write_text("# A\n- content: hello")
    (tmp_path / "wt" / "b.md").write_text("# B\n- content: world")
    (tmp_path / "wt" / ".hidden.md").write_text("# HIDDEN\n- secret: value")
    (tmp_path / "wt" / "ignore.txt").write_text("ignored")
    sub = tmp_path / "wt" / "sub"
    sub.mkdir()
    (sub / "c.md").write_text("# C\n- content: nested")
    (sub / ".hidden_in_sub.md").write_text("# HIDDEN SUB\n- secret: sub")
    deep = sub / "deep"
    deep.mkdir()
    (deep / "d.md").write_text("# D\n- content: deep")
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('mk -f /system_error.md', wt_uid="genesis")
    return rt


@pytest.fixture
async def runtime_mem(tmp_path):
    """Runtime for test_memread."""
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('mk -f /system_error.md', wt_uid="genesis")
    return rt


@pytest.fixture
async def runtime_pid(tmp_path):
    """Runtime for test_pid with hook/proc system files（文档根恒为 <root>/wt）。"""
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "config.md").write_text("# CTX\n## SYS\n- host: localhost\n- port: 8080\n")
    (tmp_path / "wt" / "log.md").write_text("# LOGS\n")
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('mk -f /.mds/env.md', wt_uid="genesis")
    await rt.exec('"# SYSTEM_ENV\n" | +> /.mds/env.md::', wt_uid="genesis")
    await rt.exec('mk -f /.mds/hook.md', wt_uid="genesis")
    await rt.exec('mk -f /.mds/proc.md', wt_uid="genesis")
    return rt


@pytest.fixture
async def runtime_llm(tmp_path):
    """Runtime for LLM tests — 唯一配置来源：fixtures/genesis/wt/.mds/llm_config.md。

    fixture 本身不含 secret；需要 live 时用 env:MDIO_LLM_SECRET。
    """
    import shutil

    real_config = (
        Path(__file__).resolve().parent
        / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md"
    )

    # 文档根是 <root>/wt：配置要落在 wt/.mds/ 才能被「节读取」拿到
    (tmp_path / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    shutil.copy(real_config, tmp_path / "wt" / ".mds" / "llm_config.md")

    rt = MdioRuntime(str(tmp_path))
    await rt.exec('mk -f /.mds/llm.md', wt_uid="genesis")
    await rt.exec('mk -f /.mds/proc.md', wt_uid="genesis")
    return rt
