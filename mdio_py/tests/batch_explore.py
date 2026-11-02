"""
批量探索测试：agent_id 标识 agent（1-5），each agent runs N times.
用法: uv run --python "3.14t" python -X jit -Xgil=0 ./mdio_py/tests/batch_explore.py --agent-id 1 --runs 3
输出: workspace/batch_results/a<id>_r<run>.json + .txt

统计策略：stream 模式下 llm.py 的 thinking/tool 信息只走 _print_* → stdout，
不进 stream yield。因此捕获 stdout 合并分析。
"""
import asyncio
import contextlib
import io
import json
import re
import sys
import time
import argparse
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent
WORKSPACE_DIR = PROJECT_ROOT / "workspace"
RESULTS_DIR = WORKSPACE_DIR / "batch_results"

_RE_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_RE_TOOL_CALL = re.compile(r"\[tool_call\]\s+\S+\s+id=")
_RE_THINKING = re.compile(r"\[thinking\]")
_RE_TOOL_PIPELINE = re.compile(r"pipeline:\s*(.+)")
_RE_MDIO_ERR = re.compile(r"- mdio_error:\s*\[(\w+)\]\s*(.+)")

# 全部 MDIO 原语 / std 的检测规则
_FEATURE_CHECKS = [
    # 写操作
    ("+>", lambda p: "+>" in p),
    ("/>", lambda p: "/>" in p),
    ("->", lambda p: "->" in p),
    ("~>", lambda p: "~>" in p),
    # 读操作
    ("$$", lambda p: "$$" in p),
    # 管道结构
    ("ScopeBlock", lambda p: "::{" in p),
    # 写标志
    ("-f", lambda p: " -f " in p or " -f true" in p or " -f false" in p),
    ("-pt", lambda p: " -pt " in p or " -pt true" in p),
    ("-god", lambda p: "-god" in p),
    # 系统
    ("hook", lambda p: "hook " in p or "hook -" in p),
    ("worktree", lambda p: "worktree " in p),
    ("alias", lambda p: "alias " in p),
    # std 字符串
    ("fmt", lambda p: "fmt " in p),
    ("split", lambda p: "split " in p),
    ("join", lambda p: "join " in p),
    ("str_ops", lambda p: "trim " in p or "upper " in p or "lower " in p),
    ("len", lambda p: "len " in p),
    # std JSON
    ("loads", lambda p: "loads " in p),
    ("dumps", lambda p: "dumps " in p),
    ("get", lambda p: "get " in p),
    # 搜索分析
    ("find", lambda p: "find " in p),
    ("tree", lambda p: "tree " in p),
    # 文件系统
    ("mk", lambda p: "mk " in p),
    ("rm", lambda p: "rm " in p or "rm -" in p),
    ("ls", lambda p: "ls " in p or "ls -" in p),
    # 逻辑 / 数学
    ("test", lambda p: "test " in p or "test -" in p),
    ("calc", lambda p: "calc " in p),
    # 元
    ("help", lambda p: "help " in p or "help -" in p or "help\"" in p),
    # 过滤 / 切片
    ("filter", lambda p: ".key." in p or ".val." in p or ".type." in p or ".head." in p or ".name." in p),
    ("slice", lambda p: ("*[" in p) or ("[0:" in p) or ("[-" in p) or ("[::-" in p) or ("[1," in p)),
    # 跨命名空间
    ("cross_wt", lambda p: re.search(r"\d+::", p) is not None),
    # 插件嵌套
    ("LLM", lambda p: "@LLM" in p),
    ("XBT", lambda p: "@XBT" in p),
    # 反模式
    ("echo_misuse", lambda p: "echo " in p),
]


def _detect_features(pipeline: str, stats: dict) -> None:
    feats = stats["features_used"]
    for name, check in _FEATURE_CHECKS:
        if name not in feats and check(pipeline):
            feats.append(name)


def _detect_features_in_text(text: str, stats: dict) -> None:
    """从非 pipeline 文本中补检写操作（可能在普通内容里出现）。"""
    feats = stats["features_used"]
    for op in ("+>", "/>", "->", "~>", "::{"):
        if op not in feats and op in text:
            feats.append(op)


def _extract_errors(text: str) -> list[dict]:
    """从输出文本中提取 mdio_error 详情。"""
    errors = []
    for m in _RE_MDIO_ERR.finditer(text):
        code = m.group(1)
        msg = m.group(2).strip()[:200]
        # 尝试向前找最近的非空行作为上下文
        start = max(0, m.start() - 300)
        ctx_block = text[start:m.end()]
        # 取最后一行 pipeline
        ctx_line = ""
        for line in ctx_block.strip().split("\n"):
            line_s = line.strip()
            if line_s and not line_s.startswith("[") and not line_s.startswith("- mdio_error"):
                ctx_line = line_s[-120:]
        errors.append({"code": code, "message": msg, "near": ctx_line})
    return errors


async def run_one(agent_id: int, run_idx: int) -> dict:
    from mdio_py import MdioRuntime

    agent_name = f"e{agent_id}r{run_idx}"
    log_path = f"/batch_log_{agent_name}.md"

    stats = {
        "agent_id": agent_id,
        "run": run_idx,
        "agent_name": agent_name,
        "tool_calls": 0,
        "thinking_chunks": 0,
        "content_chunks": 0,
        "errors": {"P001": 0, "V003": 0, "I500": 0, "R201": 0, "FATAL": 0, "CANCELLED": 0, "OTHER": 0},
        "error_details": [],
        "features_used": [],
        "duration_s": 0,
        "self_close": False,
    }
    t0 = time.time()
    full_output = ""

    try:
        runtime = MdioRuntime(str(WORKSPACE_DIR))
        wt_uid = "genesis"

        await runtime.exec(f"mk -f {log_path}; -> /.mds/llm.md::AGENTS.{agent_name}", wt_uid=wt_uid)
        init_cmd = f'"" | @LLM.{agent_name}.init -m "DEEPSEEK_V4F" -tool true -his "{log_path}::HISTORY"'
        await runtime.exec(init_cmd, wt_uid=wt_uid)

        trigger = f'"请探索MDIO" | @LLM.{agent_name}.stream'
        stream_res = await runtime.exec(trigger, wt_uid=wt_uid, stream=True)

        if isinstance(stream_res, str):
            full_output = stream_res
        else:
            capture = io.StringIO()
            try:
                with contextlib.redirect_stdout(capture):
                    async for chunk in stream_res:
                        chunk_str = str(chunk)
                        full_output += chunk_str + "\n"
            except asyncio.CancelledError:
                stats["errors"]["CANCELLED"] += 1
                stats["self_close"] = True
            except Exception as e:
                stats["errors"]["FATAL"] += 1
                stats["error_details"].append({"code": "FATAL", "message": str(e)[:200], "near": "stream_iter"})
            finally:
                full_output += capture.getvalue()

        clean_output = _RE_ANSI.sub("", full_output)

        stats["tool_calls"] = len(_RE_TOOL_CALL.findall(clean_output))
        stats["thinking_chunks"] = len(_RE_THINKING.findall(clean_output))

        for m in _RE_TOOL_PIPELINE.finditer(clean_output):
            _detect_features(m.group(1), stats)

        stats["errors"]["P001"] = clean_output.count("[P001]")
        stats["errors"]["V003"] = clean_output.count("[V003]")
        stats["errors"]["I500"] = clean_output.count("[I500]")
        stats["errors"]["R201"] = clean_output.count("R201")
        if "KeyError" in clean_output:
            stats["errors"]["FATAL"] += 1
        if "CancelledError" in clean_output or "Cancelled" in clean_output:
            stats["errors"]["CANCELLED"] += 1
        other_count = len(_RE_MDIO_ERR.findall(clean_output)) - sum(
            clean_output.count(f"[{c}]") for c in ("P001", "V003", "I500")
        )
        if other_count > 0:
            stats["errors"]["OTHER"] += other_count

        stats["error_details"] = _extract_errors(clean_output)
        _detect_features_in_text(clean_output, stats)

    except asyncio.CancelledError:
        stats["errors"]["CANCELLED"] += 1
        stats["self_close"] = True
    except Exception as e:
        stats["errors"]["FATAL"] += 1
        stats["error_msg"] = str(e)[:500]

    stats["duration_s"] = round(time.time() - t0, 1)
    return stats, full_output


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agent-id", type=int, required=True)
    parser.add_argument("--runs", type=int, required=True)
    args = parser.parse_args()

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    all_stats = []
    for r in range(1, args.runs + 1):
        print(f"[Agent {args.agent_id}] Run {r}/{args.runs} starting...", flush=True)
        s, full_out = await run_one(args.agent_id, r)
        all_stats.append(s)

        # 存 JSON
        out_path = RESULTS_DIR / f"a{args.agent_id}_r{r}.json"
        out_path.write_text(json.dumps(s, ensure_ascii=False, indent=2))

        # 存 full output
        txt_path = RESULTS_DIR / f"a{args.agent_id}_r{r}_output.txt"
        txt_path.write_text(full_out, errors="replace")

        print(f"[Agent {args.agent_id}] Run {r}/{args.runs} done: calls={s['tool_calls']}, "
              f"think={s['thinking_chunks']}, err={s['errors']}, "
              f"feats={s['features_used']}, self_close={s['self_close']}, {s['duration_s']}s", flush=True)
        if s["error_details"]:
            for ed in s["error_details"][:5]:
                print(f"  ⚠️ [{ed['code']}] {ed['message'][:100]}  ← {ed['near'][:80]}", flush=True)

    # 汇总
    summary_path = RESULTS_DIR / f"summary_a{args.agent_id}.json"
    summary = {
        "agent_id": args.agent_id,
        "total_runs": args.runs,
        "runs": all_stats,
        "totals": {
            "tool_calls": sum(s["tool_calls"] for s in all_stats),
            "thinking_chunks": sum(s["thinking_chunks"] for s in all_stats),
            "P001": sum(s["errors"]["P001"] for s in all_stats),
            "V003": sum(s["errors"]["V003"] for s in all_stats),
            "I500": sum(s["errors"]["I500"] for s in all_stats),
            "R201": sum(s["errors"]["R201"] for s in all_stats),
            "FATAL": sum(s["errors"]["FATAL"] for s in all_stats),
            "CANCELLED": sum(s["errors"]["CANCELLED"] for s in all_stats),
            "OTHER": sum(s["errors"]["OTHER"] for s in all_stats),
        },
        "avg_duration_s": round(sum(s["duration_s"] for s in all_stats) / max(len(all_stats), 1), 1),
    }
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2))
    print(f"[Agent {args.agent_id}] Summary: {json.dumps(summary['totals'], ensure_ascii=False)}", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
