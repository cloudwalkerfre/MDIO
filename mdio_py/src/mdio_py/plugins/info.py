# mdio_py/src/mdio_py/plugins/info.py
"""插件 info 套件：可门控（默认关闭）的统一事件输出 + LLM 视角报告。

为什么单独一层：事件分散在 XBT stepper 与 LLM 插件里，各自 print 会互相穿插、
格式不齐；这里统一事件模型，输出一种 JSONL；报告默认只给「LLM 当前看到什么」。

开关（默认关闭，生产零输出）：
  MDIO_XBT_INFO=1                     打开；JSONL 写 stdout
  MDIO_XBT_INFO_FILE=/tmp/x.jsonl    打开；JSONL 写文件（stdout 保持干净）
  MDIO_XBT_INFO=0 / 不设              关闭

报告：
  render(path)          LLM 视角：每次调用的 sysp + 收到的 messages + 回答
  render_timeline(path) 事件时间线（step/driver/spawn/guard/exec/bubble，排查用）
  python -m mdio_py.plugins.info <events.jsonl> [--timeline] [--full]

事件公共字段：seq / ts / ev / tid / ppid
事件类型：
  step          微步开始：ptr / wl / wl_type / status / phase / node / cond / action
  driver        driver 调用前：kind / driver / ptr / wl / cond / action / blackboard / position
  driver_reply  driver 返回：kind / reply / ms
  spawn         fork 子线程：child / ptr / wl / wl_type / driver
  bubble        结果上冒：res
  exec          动作管道执行：pipeline / ok / result
  guard         门控判定：verdict / cond
  llm           LLM 真正收到的：agent / wl / model / sysp / messages
  llm_reply     LLM 返回：agent / wl / reply
"""

from __future__ import annotations

import json
import os
import sys
import threading
import time

_LOCK = threading.Lock()
_SEQ = 0


def enabled() -> bool:
    raw = os.environ.get("MDIO_XBT_INFO", "")
    return raw.strip().lower() not in ("", "0", "false", "off", "no")


def _now() -> str:
    t = time.time()
    return time.strftime("%H:%M:%S", time.localtime(t)) + ".%03d" % int((t % 1) * 1000)


def emit(ev: str, **fields) -> None:
    """发一条统一事件；关闭时直接返回（唯一开销是一次 env 查询）。"""
    if not enabled():
        return
    global _SEQ
    with _LOCK:
        _SEQ += 1
        rec = {"seq": _SEQ, "ts": _now(), "ev": ev,
               "tid": fields.pop("tid", ""), "ppid": fields.pop("ppid", "")}
        rec.update({k: v for k, v in fields.items() if v is not None and v != ""})
        line = json.dumps(rec, ensure_ascii=False, separators=(",", ":"))
        path = os.environ.get("MDIO_XBT_INFO_FILE", "").strip()
        if path:
            with open(path, "a", encoding="utf-8") as fh:
                fh.write(line + "\n")
        else:
            print("[XBT-INFO] " + line, flush=True)


def _load(path: str) -> list:
    rows = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    rows.sort(key=lambda r: r.get("seq", 0))
    return rows


def _short(text, limit: int = 160) -> str:
    text = str(text or "").replace("\n", " / ")
    return text if len(text) <= limit else text[:limit] + "…"


def _block(text, full: bool = False) -> str:
    """多行文本保留缩进（报告里拼在 key 后面的缩进由调用方给）。"""
    text = str(text or "")
    limit = 10 ** 9 if full else 2400
    if len(text) > limit:
        text = text[:limit] + "…（截断，原文 %d 字）" % len(text)
    lines = text.splitlines() or [""]
    return lines[0] + "".join("\n      " + ln for ln in lines[1:])


def render(path: str, full: bool = False) -> str:
    """LLM 视角报告：每次调用打印 sysp + 收到的 messages + 回答。"""
    rows = _load(path)
    calls = [r for r in rows if r.get("ev") == "llm"]
    replies = [r for r in rows if r.get("ev") == "llm_reply"]
    used = set()
    out = ["# XBT info · LLM 视角 · %d 次调用 / %d 条事件" % (len(calls), len(rows))]
    for i, call in enumerate(calls, 1):
        agent = call.get("agent", "")
        wl = call.get("wl", "")
        reply = None
        for j, r in enumerate(replies):
            if j in used:
                continue
            if r.get("agent") == agent and r.get("wl") == wl and r.get("seq", 0) > call.get("seq", 0):
                reply = r
                used.add(j)
                break
        out.append("")
        out.append("## LLM 调用 %d · agent=%s · wl=%s · model=%s" % (
            i, agent, wl, call.get("model", "")))
        sysp = str(call.get("sysp") or "")
        out.append("sysp:")
        out.append(("      " + _block(sysp, full)) if sysp else "      (无)")
        tool_names = call.get("tools") or []
        out.append("可用工具: " + (", ".join(str(t) for t in tool_names) if tool_names else "(无)"))
        msgs = call.get("messages") or []
        out.append("收到的信息（%d 条）:" % len(msgs))
        for k, m in enumerate(msgs, 1):
            role = m.get("role") or "?"
            content = str(m.get("content") or "")
            out.append("  [%d] %s: %s" % (k, role, _block(content, full)))
        if reply is None:
            out.append("回答:")
            out.append("      (无)")
            continue
        if reply.get("tool"):
            out.append("工具调用: %s" % reply.get("tool"))
            out.append("  args: %s" % _block(reply.get("tool_args", ""), full))
        thinking = str(reply.get("thinking") or "")
        if thinking:
            out.append("thinking:")
            out.append("      " + _block(thinking, full))
        out.append("回答:")
        out.append("      " + _block(reply.get("reply", ""), full))
    return "\n".join(out)


def render_timeline(path: str) -> str:
    """事件时间线（排查机制用）。"""
    rows = _load(path)
    out = ["# XBT info · %d events" % len(rows)]
    for r in rows:
        ev = r.get("ev")
        ts = r.get("ts", "")
        tid = r.get("tid", "")
        if ev == "step":
            tail = ""
            if r.get("cond"):
                tail += " cond=[%s]" % _short(r["cond"], 60)
            if r.get("action"):
                tail += " action=%s" % _short(r["action"], 60)
            out.append("[%s] step   %-18s ptr=%-10s wl=%-14s status=%-8s node=%s%s" % (
                ts, tid, r.get("ptr", ""), r.get("wl", ""), r.get("status", ""),
                r.get("node", ""), tail))
        elif ev == "driver":
            out.append("[%s] driver %-18s kind=%-9s driver=%-9s wl=%-14s ptr=%s" % (
                ts, tid, r.get("kind", ""), r.get("driver", ""), r.get("wl", ""), r.get("ptr", "")))
            if r.get("blackboard"):
                out.append("         黑板有啥:")
                for ln in str(r["blackboard"]).splitlines():
                    out.append("           " + ln)
            else:
                out.append("         黑板有啥: (空)")
            if r.get("position"):
                out.append("         我在哪:")
                for ln in str(r["position"]).splitlines():
                    out.append("           " + ln)
        elif ev == "driver_reply":
            out.append("[%s] reply  %-18s kind=%-9s %sms  %s" % (
                ts, tid, r.get("kind", ""), r.get("ms", ""), _short(r.get("reply", ""), 160)))
        elif ev == "spawn":
            out.append("[%s] spawn  %-18s → %-18s wl=%-14s driver=%s" % (
                ts, tid, r.get("child", ""), r.get("wl") or "-", r.get("driver") or "-"))
        elif ev == "bubble":
            out.append("[%s] bubble %-18s → %-10s %s" % (
                ts, tid, r.get("ppid", ""), r.get("res", "")))
        elif ev == "exec":
            out.append("[%s] exec   %-18s ok=%-5s %s" % (
                ts, tid, r.get("ok", ""), _short(r.get("pipeline", ""), 140)))
        elif ev == "guard":
            out.append("[%s] guard  %-18s verdict=%-5s cond=[%s]" % (
                ts, tid, r.get("verdict", ""), _short(r.get("cond", ""), 70)))
        elif ev == "llm":
            out.append("[%s] llm    %-18s agent=%s wl=%s sysp=%s msgs=%d" % (
                ts, tid, r.get("agent", ""), r.get("wl", ""),
                "(有)" if r.get("sysp") else "(无)", len(r.get("messages") or [])))
        elif ev == "llm_reply":
            out.append("[%s] llm_reply %-16s %s" % (ts, tid, _short(r.get("reply", ""), 140)))
        else:
            out.append("[%s] %-6s %s" % (ts, ev, json.dumps(r, ensure_ascii=False, sort_keys=True)))
    return "\n".join(out)


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    timeline = "--timeline" in argv
    full = "--full" in argv
    files = [a for a in argv if not a.startswith("--")]
    if len(files) != 1:
        print("用法: python -m mdio_py.plugins.info <events.jsonl> [--timeline] [--full]", file=sys.stderr)
        return 2
    print(render_timeline(files[0]) if timeline else render(files[0], full=full))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
