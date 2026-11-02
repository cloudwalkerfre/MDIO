"""
LLM 知识库自治理实验

任务：整理 /inbox/ 中的零散笔记，按主题归类到 /kb/ 目录。
验证：只看结果 — 每个主题文件是否存在、内容是否覆盖了对应笔记。
"""
import asyncio
import re
import shutil
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime

FIXTURE_CONFIG = Path(__file__).resolve().parent / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md"
AGENT_NAME = "inspector"

INBOX_DATA = """# INBOX
- db_pool_size: 建议设为 20
- api_timeout: 生产环境建议 30s，开发环境 10s
- deploy_region: 先上华东，再切华北
- cache_strategy: 热点数据用 Redis，冷数据走 MySQL
- db_pool_size: 生产环境至少 50
- api_retry: 失败重试 3 次，间隔 1s
- cache_ttl: Redis 默认 TTL 设为 3600
- deploy_branch: release/v2.1 用于华东，v2.2 用于华北
- api_timeout: 文件上传接口建议 120s
- db_backup: 每日凌晨 2 点全量备份
- cache_strategy: CDN 静态资源缓存 7 天
- api_retry: 幂等接口可重试 5 次
- deploy_rollback: 华北回滚窗口 10 分钟
- db_replica: 只读副本至少 2 个
- api_gateway: 统一走 Kong 网关，限流 1000/s
- cache_ttl: 用户 session TTL 设为 7200"""

TRIGGER_MSG = "请整理 /inbox/ 中的零散笔记并按主题归类，每个主题一个文件存入 /kb/ 目录中"

# 结果验证：每个主题必须覆盖的核心关键词
TOPIC_CHECKS = {
    "database": {
        "must_have": ["pool", "backup", "replica"],
        "nice_to_have": ["20", "50", "凌晨", "只读"],
    },
    "cache": {
        "must_have": ["Redis", "CDN", "TTL"],
        "nice_to_have": ["3600", "7200", "session", "MySQL"],
    },
    "deploy": {
        "must_have": ["华东", "华北", "回滚"],
        "nice_to_have": ["v2.1", "v2.2", "release", "10 分钟"],
    },
    "api": {
        "must_have": ["timeout", "retry", "gateway"],
        "nice_to_have": ["30s", "120s", "Kong", "1000/s", "幂等"],
    },
}


async def main():
    tmp = Path(tempfile.mkdtemp(prefix="mdio_inspect_"))
    (tmp / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    shutil.copy(FIXTURE_CONFIG, tmp / "wt" / ".mds" / "llm_config.md")
    runtime = MdioRuntime(str(tmp))
    wt_uid = "genesis"

    print("=" * 64)
    print("[Phase 1] 初始化测试环境")
    print("=" * 64)

    await runtime.exec("rm -rf /inbox/ ; rm -rf /kb/", wt_uid=wt_uid)
    await runtime.exec("mk -f /inbox/notes.md", wt_uid=wt_uid)
    await runtime.exec(
        f'"""{INBOX_DATA}""" | +> /inbox/notes.md::', wt_uid=wt_uid
    )

    verify = await runtime.exec("$ /inbox/notes.md::", wt_uid=wt_uid)
    if not verify or verify.startswith("- mdio_error"):
        print(f"  [FATAL] inbox 数据未就绪: {verify}")
        return
    print(f"  ✓ inbox 就绪 ({len(verify)} chars)")

    await runtime.exec(f"-> /.mds/llm.md::AGENTS.{AGENT_NAME}", wt_uid=wt_uid)

    print()
    print("=" * 64)
    print("[Phase 2] LLM 自主执行 (stream)")
    print(f"  prompt: {TRIGGER_MSG}")
    print("=" * 64)
    print()

    init_cmd = (
        f'"" | @LLM.{AGENT_NAME}.init '
        f'-m "DEFAULT" '
        f'-tool true '
        f'-his "/.mds/task_log.md::HISTORY"'
    )
    init_res = await runtime.exec(init_cmd, wt_uid=wt_uid)
    print(f"[init] {init_res}")

    trigger = f'"{TRIGGER_MSG}" | @LLM.{AGENT_NAME}.stream'
    stream_res = await runtime.exec(trigger, wt_uid=wt_uid, stream=True)

    if isinstance(stream_res, str):
        print(f"\n[FATAL] {stream_res}")
        return

    async for chunk in stream_res:
        print(chunk, end="", flush=True)
    print()

    print()
    print("=" * 64)
    print("[Phase 3] 结果验证")
    print("=" * 64)

    # 获取 LLM 实际创建的文件
    ls_kb = await runtime.exec("ls -l /kb/", wt_uid=wt_uid)
    kb_files_raw = str(ls_kb) if ls_kb else ""

    # 匹配每个主题到实际文件名 (ls -l 格式: "... <filename>")
    file_map = {}
    for topic in TOPIC_CHECKS:
        candidates = [f"{topic}.md"]
        if topic == "deploy":
            candidates.append("deployment.md")
        if topic == "database":
            candidates.append("db.md")

        for cand in candidates:
            if cand in kb_files_raw:
                file_map[topic] = cand
                break
        if topic not in file_map:
            for cand in candidates:
                # 模糊匹配
                for line in kb_files_raw.split("\n"):
                    parts = line.strip().split()
                    for p in parts:
                        if cand in p or topic in p.lower():
                            if ".md" in p:
                                file_map[topic] = p.split("/")[-1]
                                break

    # 验证每个主题
    all_pass = True
    for topic, checks in TOPIC_CHECKS.items():
        fname = file_map.get(topic)
        if not fname:
            print(f"  [{topic}] ✗ 未找到对应文件 (ls -l /kb/: {kb_files_raw[:100]})")
            all_pass = False
            continue

        content = await runtime.exec(f"$ /kb/{fname}::", wt_uid=wt_uid)
        if not content or content.startswith("- mdio_error"):
            print(f"  [{topic}] ✗ 文件 {fname} 为空或不可读: {content}")
            all_pass = False
            continue

        must_matched = [k for k in checks["must_have"] if k in str(content)]
        nice_matched = [k for k in checks["nice_to_have"] if k in str(content)]

        must_ok = len(must_matched) == len(checks["must_have"])
        nice_ratio = len(nice_matched) / len(checks["nice_to_have"]) if checks["nice_to_have"] else 1.0

        status = "✓" if must_ok else "✗"
        print(f"  [{topic}] {status} {fname} ({len(content)} chars)")
        print(f"         must: {len(must_matched)}/{len(checks['must_have'])} matched")
        print(f"         nice: {len(nice_matched)}/{len(checks['nice_to_have'])} matched")

        if not must_ok:
            missing_must = set(checks["must_have"]) - set(must_matched)
            print(f"         MISSING (must): {missing_must}")
            all_pass = False

    # 展示产出
    print()
    print("--- LLM 产出 ---")
    for topic, fname in file_map.items():
        content = await runtime.exec(f"$ /kb/{fname}::", wt_uid=wt_uid)
        if content and not content.startswith("- mdio_error"):
            print(f"\n  [{fname}]")
            for line in str(content).split("\n"):
                print(f"    {line}")

    # 汇总
    print()
    print("=" * 64)
    print("[Summary]")
    print("=" * 64)
    print(f"  结果: {'✓ 全部通过' if all_pass else '✗ 存在差异'}")
    print(f"  LLM 产出文件: {list(file_map.values())}")


if __name__ == "__main__":
    asyncio.run(main())
