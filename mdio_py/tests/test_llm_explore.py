"""
让 LLM 在纯 MDIO 文档指引下自由探索环境。

设计原则：
- 不给 sysp，不给任务指引
- 只给 tool 权限 + 一份 mdio_tool_docs.md
- 观察 LLM 的行为，反向诊断文档的不足之处
"""
import asyncio
import shutil
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime

FIXTURE_CONFIG = Path(__file__).resolve().parent / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md"
AGENT_NAME = "explorer"


async def main():
    tmp = Path(tempfile.mkdtemp(prefix="mdio_explore_"))
    (tmp / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    shutil.copy(FIXTURE_CONFIG, tmp / "wt" / ".mds" / "llm_config.md")
    runtime = MdioRuntime(str(tmp))
    wt_uid = "genesis"

    print("=" * 60)
    print("[Setup] 清理旧 agent + 创建空白探索空间")
    print("=" * 60)

    await runtime.exec(f"mk -f /exploration_log.md; -> /.mds/llm.md::AGENTS.{AGENT_NAME}", wt_uid=wt_uid)

    # 不传 -sysp，让 LLM 完全依赖 mdio_tool_docs.md
    init_cmd = f'"" | @LLM.{AGENT_NAME}.init -m "DEFAULT" -tool true -his "/exploration_log.md::HISTORY"'
    init_res = await runtime.exec(init_cmd, wt_uid=wt_uid)
    print(f"Init: {init_res}")

    print()
    print("=" * 60)
    print("[Run] 启动自由探索 (stream 模式)")
    print("=" * 60)
    print()

    trigger = '"请探索MDIO" | @LLM.explorer.stream'
    stream_res = await runtime.exec(trigger, wt_uid=wt_uid, stream=True)

    if isinstance(stream_res, str):
        print(f"\n[Error] {stream_res}")
    else:
        async for chunk in stream_res:
            print(chunk, end="", flush=True)
        print()

    print()
    print("=" * 60)
    print("[Result] exploration_log.md 内容")
    print("=" * 60)
    log_text = runtime.store.get_text(wt_uid, "/exploration_log.md")
    print(log_text)


if __name__ == "__main__":
    asyncio.run(main())
