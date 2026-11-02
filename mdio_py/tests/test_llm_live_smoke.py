"""真模型冒烟：@LLM 在 HIS 记录格式下走一次真 API 的一轮对话。

默认跳过（离线套件保持无网络）：夹具 `live_llm` 要求 `MDIO_LIVE_LLM=1`。
跑法：MDIO_LIVE_LLM=1 .venv/bin/python -m pytest mdio_py/tests/test_llm_live_smoke.py -q
配置来源：开了 `MDIO_LIVE_LLM` 时 `runtime_llm` 会拷贝 dev 树的
`worktrees/genesis/wt/llm_config.md`。跑在临时 runtime 里，不碰 genesis 工作树。
"""

import asyncio
import os

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm.llm import _parse_his_records, _read_his, _rebuild_messages

ALIAS = os.environ.get("MDIO_LIVE_ALIAS", "DEFAULT")


async def test_live_one_turn(runtime_llm, live_llm):
    rt = runtime_llm
    ws, ctx = rt._workspace, RuntimeContext(wt_uid="genesis")

    init = await rt.exec(f'@LLM.smoke.init -m "{ALIAS}" -sysp "只回一句话，不超过十五个字"',
                         wt_uid="genesis")
    print("\n[init]", init)
    assert "mdio_error" not in init, init

    res = await asyncio.wait_for(
        rt.exec('"用一句话说你好" | @LLM.smoke.run', wt_uid="genesis"), timeout=180)
    print("[模型回复]", res)

    raw = _read_his(ws, "smoke", "HIS", {"his": "HIS"}, ctx)
    print("[HIS]\n" + raw)
    roles = [r["role"] for r in _parse_his_records(raw, workspace=ws)]
    print("[roles]", roles)

    assert "mdio_error" not in res, res
    assert res.strip(), "模型回复为空"
    assert roles[0] == "user", roles
    assert "assistant" in roles, roles
    if "thinking" in roles:                              # 开了 thinking 的模型会多一条
        assert roles.index("thinking") < roles.index("assistant"), roles
    assert all(ln == "  >" or ln.startswith("  > ")
               for ln in raw.split("\n") if ln.startswith("  >")), raw

    # 真数据走一遍重建：thinking 应归到它后面那条 assistant
    msgs = _rebuild_messages(raw)
    print("[messages]", [(m["role"], (m.get("content") or "")[:12]) for m in msgs])
    assert [m["role"] for m in msgs] == ["user", "assistant"], msgs
    if "thinking" in roles:
        assert msgs[1].get("reasoning_content"), msgs[1]
