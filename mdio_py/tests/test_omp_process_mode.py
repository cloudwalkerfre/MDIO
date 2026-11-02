"""omp 测试通道冒烟：store.set_mode 切换进程级模式（仅测试/诊断；生产=重启）。"""

import json
import os
import pytest
from mdio_py import mdio_core


@pytest.fixture
def store():
    s = mdio_core.ProjectStore("/mem")
    s.tree_ensure()
    return s


def _envelope_mode(store, text="# H\n\ntext\n"):
    doc_id, _ = store.write_locator(
        "genesis", "/t.md", {"steps": []}, "/>", text, True, False, False, False
    )
    return json.loads(store.get_ast(doc_id))["mode"]


def test_initial_mode_matches_process_env(store):
    """进程初始模式 = 进程 env 解析（cf 兜底或 env-hs 显式注入均成立）。"""
    expect = "hs" if os.environ.get("MDIO_PROJ_MODE") == "hs" else "cf"
    assert _envelope_mode(store) == expect, "初始模式必须与进程 env 解析一致"


def test_set_mode_switches_envelope_mode(store):
    """set_mode("hs") → get_ast 信封 mode=="hs" → 回切 "cf" → mode=="cf"。

    生产默认不依赖本测试（进程 env 决定）；本测试只证切换通道。
    """
    store.set_mode("hs")
    assert _envelope_mode(store) == "hs", "set_mode(hs) 后信封 mode 必须 hs"
    store.set_mode("cf")
    assert _envelope_mode(store) == "cf", "回切 cf 后信封 mode 必须 cf"


def test_set_mode_rejects_unknown(store):
    """非法模式报错（显式优于静默回落）。"""
    with pytest.raises(Exception) as ei:
        store.set_mode("xx")
    assert "unknown mode" in str(ei.value)
