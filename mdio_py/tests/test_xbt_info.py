"""插件 info 套件：默认关闭零输出；统一 JSONL；LLM 视角 / 时间线两种报告。"""

import json

from mdio_py.plugins import info


def _off(monkeypatch):
    monkeypatch.delenv("MDIO_XBT_INFO", raising=False)
    monkeypatch.delenv("MDIO_XBT_INFO_FILE", raising=False)


def test_disabled_emits_nothing(monkeypatch, capsys):
    _off(monkeypatch)
    info.emit("step", tid="t1", ptr="0")
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == ""


def test_enabled_writes_unified_jsonl(monkeypatch, tmp_path, capsys):
    path = tmp_path / "events.jsonl"
    monkeypatch.setenv("MDIO_XBT_INFO", "1")
    monkeypatch.setenv("MDIO_XBT_INFO_FILE", str(path))
    info.emit("step", tid="t1", ptr="0", wl="main")
    info.emit("driver", tid="t1", kind="condition", blackboard="- 背景", position="A")
    rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
    assert [r["ev"] for r in rows] == ["step", "driver"]
    for row in rows:
        assert {"seq", "ts", "ev", "tid", "ppid"} <= set(row)
    assert rows[1]["blackboard"] == "- 背景"
    assert capsys.readouterr().out == ""   # 写文件时 stdout 不掺事件


def test_render_llm_view_shows_sysp_and_input(monkeypatch, tmp_path):
    path = tmp_path / "events.jsonl"
    monkeypatch.setenv("MDIO_XBT_INFO", "1")
    monkeypatch.setenv("MDIO_XBT_INFO_FILE", str(path))
    info.emit("llm", agent="dev", wl="w1", model="deepseek-v4-flash",
              sysp="你是节点", tools=[], messages=[{"role": "user", "content": "请写一句"}])
    info.emit("llm_reply", agent="dev", wl="w1", thinking="先想一下", reply="true")
    text = info.render(str(path))
    assert "# XBT info · LLM 视角" in text
    assert "agent=dev" in text and "wl=w1" in text
    assert "sysp:" in text and "你是节点" in text
    assert "可用工具: (无)" in text
    assert "收到的信息（1 条）" in text and "请写一句" in text
    assert "thinking:" in text and "先想一下" in text
    assert "回答:" in text and "true" in text


def test_timeline_keeps_blackboard_and_position(monkeypatch, tmp_path):
    path = tmp_path / "events.jsonl"
    monkeypatch.setenv("MDIO_XBT_INFO", "1")
    monkeypatch.setenv("MDIO_XBT_INFO_FILE", str(path))
    info.emit("driver", tid="t1", kind="condition", blackboard="- 背景", position="A\nB")
    text = info.render_timeline(str(path))
    assert "黑板有啥" in text and "- 背景" in text
    assert "我在哪" in text and "B" in text
