import asyncio
import tempfile

import pytest
from pathlib import Path

from mdio_py import MdioRuntime

async def run_hook_test():
    print("🚀 Starting MDIO Hook Primitive (Shadow Engine) Test...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        workspace = Path(tmpdir)
        
        # 1. 准备物理底座
        config_file = workspace / "config.md"
        config_file.write_text("# CTX\n## SYS\n- host: localhost\n- port: 8080\n", encoding="utf-8")
        
        log_file = workspace / "log.md"
        log_file.write_text("# LOGS\n", encoding="utf-8")
        
        loop_file = workspace / "loop.md"
        loop_file.write_text("# LOOP\n- A: init\n- B: init\n", encoding="utf-8")

        print(f"📦 Workspace mounted at: {workspace}")

        # 启动沙箱与引擎
        runtime = MdioRuntime(str(workspace))
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        # 钩子注册与透明管线穿透测试 (Transparent Pipeline)
        print("\n⚡ [1/4] 注册测试钩子...")
        
        # 使用 CTX.SYS 嵌套选择器
        register_code = r'''
        hook -target /config.md::CTX.SYS._b:host -cb '"Host 变了!" | +> /log.md::LOGS' -is_always ;
        hook -target /config.md::CTX.SYS._b:port -cb '"Port 变了! (仅一次)" | +> /log.md::LOGS' ;
        "data_stream" | hook -target /config.md::CTX.SYS._b:new_key -cb '"NewKey 被触碰了!" | +> /log.md::LOGS' -is_always | fmt -t "PassThrough: {}" | +> /log.md::LOGS ;
        '''
        
        await runtime.exec(register_code, active_doc="/config.md", wt_uid="genesis")
        
        log_text_after_reg = runtime.store.get_text("genesis", "/log.md")
        assert "PassThrough: data_stream" in log_text_after_reg, "❌ 透明管道拦截失败：数据流在穿过 hook 后丢失！"
        print("✅ 钩子装载完毕，透明管道数据穿透成功。")

        # 触发变动与物理抗漂移测试
        print("\n⚡ [2/4] 触发变动，测试异步 Shadow Worker...")
        
        # 回调上下文统一取 CTX.SYS
        trigger_code = r'''
        "192.168.1.1" | /> /config.md::CTX.SYS._b:host ;
        "9090" | /> /config.md::CTX.SYS._b:port ;
        "ghost_data" | +> /config.md::CTX._b:drift ;
        '''
        await runtime.exec(trigger_code, active_doc="/config.md", wt_uid="genesis")
        
        # 等待 0.5 秒让异步 hook consumer 完成处理
        await asyncio.sleep(0.5)

        log_text_p2 = runtime.store.get_text("genesis", "/log.md")
        print("📄 Log State (Phase 2):\n" + log_text_p2.strip())
        
        assert "Host 变了!" in log_text_p2, "❌ 持久钩子未触发"
        assert "Port 变了! (仅一次)" in log_text_p2, "❌ 一次性钩子未触发"
        assert log_text_p2.count("Host 变了!") == 1, "❌ 持久钩子在一次触发后应只执行一次"
        assert log_text_p2.count("Port 变了! (仅一次)") == 1, "❌ 一次性钩子在销毁前应只触发一次"
        print("✅ 异步触发成功，且抗物理漂移 (Spatial Diff) 逻辑完美生效！")

        # 生命周期校验 (One-time vs Persist)
        print("\n⚡ [3/4] 再次触发，验证 One-time 自动销毁机制...")
        
        # 回调上下文统一取 CTX.SYS
        trigger_code_2 = r'''
        "10.0.0.1" | /> /config.md::CTX.SYS._b:host ;
        "80" | /> /config.md::CTX.SYS._b:port ;
        '''
        await runtime.exec(trigger_code_2, active_doc="/config.md", wt_uid="genesis")
        await asyncio.sleep(0.5)

        log_text_p3 = runtime.store.get_text("genesis", "/log.md")
        print("📄 Log State (Phase 3):\n" + log_text_p3.strip())
        
        assert log_text_p3.count("Host 变了!") == 2, "❌ 持久钩子(-is_always)未能二次触发"
        assert log_text_p3.count("Port 变了! (仅一次)") == 1, "❌ 一次性钩子在首次用完后不应再次触发"
        print("✅ 生命周期校验准确：一次性钩子已灰飞烟灭，持久钩子屹立不倒。")

        # 死循环熔断
        print("\n⚡ [4/4] 启动高危链式反应，测试系统 Depth 防爆墙...")
        
        # 使用 +> 不断追加脏数据，强行绕过底层的稳态收敛机制，引发真正的无限雪崩
        loop_register = r'''
        hook -target /loop.md::LOOP._b:A -cb '"bounce_a " | +> /loop.md::LOOP._b:B' -is_always ;
        hook -target /loop.md::LOOP._b:B -cb '"bounce_b " | +> /loop.md::LOOP._b:A' -is_always ;
        '''
        await runtime.exec(loop_register, active_doc="/default.md", wt_uid="genesis")
        
        # 触发 A 变更启动链式循环
        await runtime.exec('"ignite!" | +> /loop.md::LOOP:A', active_doc="/default.md", wt_uid="genesis")
        
        # 等待异步钩子链式触发完成
        await asyncio.sleep(0.5)
        
        try:
            sys_err_text = runtime.store.get_text("genesis", "/system_error.md")
            print(f"-- 🚨 灾备中心截获拦截日志 --\n{sys_err_text.strip()}")
            assert "熔断" in sys_err_text or "风暴" in sys_err_text, "❌ 循环熔断器应将异常写入 /system_error.md"
        except Exception:
            assert False, "❌ `/system_error.md` 未生成！循环截瘫防护代码失效！"

        print("✅ 物理防爆墙生效！成功切断高频循环引信，系统安然无恙！")
        

        print("\n🏆================================================================🏆")
        print("🎉 THE MDIO HOOK SYSTEM (SHADOW ENGINE & PIPELINE & FFI) IS FLAWLESS! 🎉")
        print("🏆================================================================🏆\n")

if __name__ == "__main__":
    asyncio.run(run_hook_test())


# hook 触发行为契约（编辑类型 × 目标位置的触发不变式）
# 目标行内编辑触发、行外内容编辑不触发、结构编辑保守、多 hook 独立、目标消失触发。

import threading
from mdio_py.core.selector_builder import by_tag, drill, SelectorBuilder
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.core import parse_code


class _TriggerRecorder:
    def __init__(self):
        self.lock = threading.Lock()
        self.calls = []

    def __call__(self, cb_code: str, ctx_dict: dict):
        with self.lock:
            self.calls.append(cb_code)


def _write_doc(store, path: str, doc: str):
    store.mk("genesis", path, parents=True, verbose=False)
    store.write_locator("genesis", path, SelectorBuilder().build(), "/>", doc,
                        auto_scaffold=True, god=True)


def _reg(store, path, q, cb, always: bool = True):
    """注册 hook（always=False = 一次性——触发即自动删除）。"""
    steps = parse_code(q)[0].atoms[0].selector.steps
    store.register_hook("genesis", path, SelectorIR(steps=steps), cb, {}, always)


async def test_trigger_contract_basic(runtime):
    """多层文档：目标行内触发 / 行外内容不触发 / 结构保守 / 多 hook 独立 / 删除触发。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    doc = "# T\n\n## A\n\n- status: IDLE\n\n- x: 1\n\n## B\n\n- status: IDLE\n\n- x: 2\n"
    _write_doc(runtime.store, "/d.md", doc)
    _reg(runtime.store, "/d.md", "$T.A._b:status", "@cb A.status")
    _reg(runtime.store, "/d.md", "$T.B._b:status", "@cb B.status")
    _reg(runtime.store, "/d.md", "$T.A", "@cb A.head")

    def fired_since(n):
        return rec.calls[n:]

    # 1) A.status 目标行内内容编辑 → A.status 触发；B.status 不触发；A.head 触发（section 内）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md", by_tag("T.A", "status"), "/>", "UP",
                                auto_scaffold=True, god=True, proj="content")
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb A.status" in fired, f"A.status 目标行内编辑应触发: {fired}"
    assert "@cb B.status" not in fired, f"B.status 不应触发: {fired}"
    assert "@cb A.head" in fired, f"A.head（section 语义）应触发: {fired}"

    # 2) A.x 内容编辑（A.status 目标行外）→ A.status/B.status 不触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md", by_tag("T.A", "x"), "/>", "9",
                                auto_scaffold=True, god=True, proj="content")
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb A.status" not in fired, f"A.status 行外内容编辑不应触发: {fired}"
    assert "@cb B.status" not in fired, f"B.status 不应触发: {fired}"

    # 3) B.status 目标行内编辑 → 只 B.status 触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md", by_tag("T.B", "status"), "/>", "GO",
                                auto_scaffold=True, god=True, proj="content")
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb B.status" in fired, f"B.status 目标行内编辑应触发: {fired}"
    assert "@cb A.status" not in fired, f"A.status 不应触发: {fired}"

    # 4) 结构编辑：A 追加字段 → A.head 触发（section 内容变）；A.status 不触发（行字节没变）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md", by_tag("T.A", "x"), "+>", "- new: 1\n",
                                auto_scaffold=False, god=True)
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb A.head" in fired, f"A 追加（结构）应触发 A.head: {fired}"
    assert "@cb A.status" not in fired, f"A.status 行未变不应触发: {fired}"

    # 5) 删除 A.status → A.status 触发（目标消失）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md", by_tag("T.A", "status"), "->", "",
                                auto_scaffold=False, god=True)
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb A.status" in fired, f"A.status 目标消失应触发: {fired}"


async def test_trigger_contract_nested(runtime):
    """深层嵌套（BQ 内 list 内 BQ 内 heading）：递归 hook + 叶子目标剪枝。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    doc = ("# T\n\n## A\n\n> - root:\n>   - b:\n>      > # H4\n"
           ">      > - B3:\n>      >   - leaf:  hi\n")
    _write_doc(runtime.store, "/d.md", doc)
    _reg(runtime.store, "/d.md", "$T..leaf", "@cb leaf")
    _reg(runtime.store, "/d.md", "$T..H4", "@cb H4")

    def fired_since(n):
        return rec.calls[n:]

    # 1) 叶子目标行内编辑 → leaf 触发（递归命中）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md",
                                parse_code("$T..leaf")[0].atoms[0].selector,
                                "/>", "NEW", auto_scaffold=False, god=True, proj="content")
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb leaf" in fired, f"leaf 目标行内编辑应触发: {fired}"

    # 2) 祖先（root）内容编辑 → leaf 不触发（leaf 行字节没变）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md",
                                parse_code("$T..root")[0].atoms[0].selector,
                                "/>", "R2", auto_scaffold=False, god=True, proj="content")
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb leaf" not in fired, f"祖先内容编辑不应触发 leaf: {fired}"

    # 3) B3 内容编辑（B3 在 H4 section 内）→ H4 触发（section 内容变）；leaf 不触发（行没变）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md",
                                parse_code("$T..B3")[0].atoms[0].selector,
                                "/>", "Z2", auto_scaffold=False, god=True, proj="content")
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb H4" in fired, f"B3 内容编辑应触发 H4（section 语义）: {fired}"
    assert "@cb leaf" not in fired, f"leaf 行未变不应触发: {fired}"

    # 4) 删除 leaf → leaf 触发（目标消失）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/d.md",
                                parse_code("$T..leaf")[0].atoms[0].selector,
                                "->", "", auto_scaffold=False, god=True)
    await asyncio.sleep(0.3)
    fired = fired_since(n)
    assert "@cb leaf" in fired, f"leaf 目标消失应触发: {fired}"



# NESTED 异构深嵌套矩阵（NESTED_ALL 舞台——BLT 深层 / BQ-in-BLT /
# BLT-in-BQ / 双层 BQ / 4 层交替容器 / fence / setext / comment 混排）
#
# 契约按实证基准断言（探针全量干净复现验证）：
#   · 行级精确——写目标行只触发自身 hook，跨拓扑零串扰
#   · 容器 hook 子树敏感；叶 hook 行级精确（写祖先行不触发孙叶）
#   · heading hook = 全节敏感（BQ 壳内跨 fence 覆盖）
#   · 级联/递归 Drill 命中同一目标时同写同触发；Drill = 精确匹配
#   · once 触发即删；目标从无到有 / 消失均触发；常驻每变必触发
#   · heading 行内容写 = 整节替换（写面语义）→ 链下 hook 全触发（消失）
#   · scaffold 陷阱锁定：级联链目标为裸项（tag=''）→ 落空 → 静默追加
#     标题树（auto_scaffold=False 不拦截）→ 追加标题被递归 hook 假命中
# 等待用轮询（冷启动 evaluate 延迟可达数百 ms；固定短 sleep 会漏触发）；op 用字面量。

NESTED_HOOK_DOC = """# H1
## H2
### H3

## BLT_DEEP
- k1
    - k1_1:  123
    - k1_2:
        - k1_2_1:  abc
            - k1_2_1_1
                - k1_2_1_1_1:  deep
    - k1_3:
        - k1_3_1
- k2:
    - k2_1:  x
    - k2_2:
       > # b_bq_nested
       > - b_bq_blt:  y
       >   - b_bq_blt_1:  z
       > ```python
       > print("k2")
       > ```
- k3:
    > 1. b_bq_ol1
    > 2. b_bq_ol2

## BQ_RICH
> # q1
> ## q2
> - qb1:  v1
>   - qb1_1:  v2
> 1. qo1
>   1. qo1_1:  w
>
> ```rust
> fn q() {}
> ```
>     qic
>
> | c1 | c2 |
> | --- | --- |
> | 1 | 2 |
>
> ---
> q paragraph
> > qn1
> > - qnb1:  v3
> >   - qnb1_1:  v4
> <!-- qc -->
> <div>qh</div>

## MIX
- m1:
  - m2:
     > # m_bq1
     > - m3:
     >   - m4:
     >      > # m_bq2
     >      > - m5:  hi

<!-- comment block -->
SETEXT
======
setext body"""


async def _wait_fired(rec: _TriggerRecorder, n: int, timeout: float = 3.0) -> bool:
    """轮询等待 dispatcher 收到第 n 个之后的触发；超时返回 False。"""
    loop = asyncio.get_event_loop()
    deadline = loop.time() + timeout
    while len(rec.calls) <= n:
        if loop.time() > deadline:
            return False
        await asyncio.sleep(0.05)
    return True


async def _wait_silent(rec: _TriggerRecorder, n: int, window: float = 0.5) -> bool:
    """窗口期内无新增触发（静默断言用——防迟到假触发）。"""
    loop = asyncio.get_event_loop()
    deadline = loop.time() + window
    while loop.time() < deadline:
        if len(rec.calls) > n:
            return False
        await asyncio.sleep(0.05)
    return True


def _write_nested(store, path: str = "/m.md"):
    _write_doc(store, path, NESTED_HOOK_DOC)


def _nested_sel(q):
    return parse_code(q)[0].atoms[0].selector


async def test_nested_hook_matrix_row_precision(runtime):
    """跨拓扑行级精确：写目标行只触发自身 hook（BLT 浅层 / BQ 壳内叶 / 双层 BQ 内叶）。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k1_1", "@k1_1")
    _reg(runtime.store, "/m.md", "$BQ_RICH.q1.qb1.qb1_1", "@qb1_1")
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2.qnb1.qnb1_1", "@qnb1_1")

    async def expect(q, payload, want):
        n = len(rec.calls)
        runtime.store.write_locator("genesis", "/m.md", _nested_sel(q), "/>", payload,
                                    auto_scaffold=False, god=True, proj="content")
        assert await _wait_fired(rec, n), f"写 {q} 后应有触发（超时）"
        assert rec.calls[n:] == [want], f"写 {q}: 触发 {rec.calls[n:]} != {[want]}"

    await expect("$BLT_DEEP.k1_1", "x1", "@k1_1")
    await expect("$BQ_RICH.q1.qb1.qb1_1", "v2x", "@qb1_1")
    await expect("$BQ_RICH.q2.qnb1.qnb1_1", "v4x", "@qnb1_1")


async def test_nested_hook_matrix_container_vs_leaf(runtime):
    """容器 hook 子树敏感（写子树任一行触发）；叶 hook 行级精确（写祖先不触发）。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k1_2", "@k1_2_box")
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k1_2.k1_2_1", "@k1_2_1")

    # 1) 写子树内叶 k1_2_1 → 叶自身 + 容器（子树 hash 变）都触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k1_2.k1_2_1"),
                                "/>", "def", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "写子树叶后应有触发"
    assert set(rec.calls[n:]) == {"@k1_2_1", "@k1_2_box"}, rec.calls[n:]

    # 2) 写堂兄弟 k1_1（k1_2 子树外）→ 容器与叶都静默
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k1_1"),
                                "/>", "x1", auto_scaffold=False, god=True, proj="content")
    assert await _wait_silent(rec, n), f"堂兄弟写不应触发容器/叶: {rec.calls[n:]}"


async def test_nested_hook_matrix_bq_heading_section(runtime):
    """BQ 壳内 heading hook = 全节敏感：壳内 fence 前后行都触发；壳外写静默。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k2_2.b_bq_nested", "@h_bq")
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k2_2.b_bq_blt", "@blt")

    # 1) 写壳内 fence 前叶 b_bq_blt → 叶自身 + heading（节含）都触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k2_2.b_bq_blt"),
                                "/>", "y9", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "写壳内叶后应有触发"
    assert set(rec.calls[n:]) == {"@blt", "@h_bq"}, rec.calls[n:]

    # 2) 写 blt 的子行 b_bq_blt_1 → 父容器（子树敏感）+ heading（节内）都触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k2_2.b_bq_blt_1"),
                                "/>", "z2", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "写 blt 子行后应有触发"
    assert set(rec.calls[n:]) == {"@blt", "@h_bq"}, rec.calls[n:]

    # 3) 写壳外兄弟 k2_1 → 全静默（节边界：壳内内容到 fence 结束，壳外行不在节内）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k2_1"),
                                "/>", "xx", auto_scaffold=False, god=True, proj="content")
    assert await _wait_silent(rec, n), f"壳外写不应触发: {rec.calls[n:]}"


async def test_nested_hook_matrix_recursive_and_chain(runtime):
    """同一目标的级联链与递归 hook：两种写形态都双触发。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2.qnb1", "@full")
    _reg(runtime.store, "/m.md", "$..qnb1", "@rec")

    for q in ["$BQ_RICH.q2.qnb1", "$..qnb1"]:
        n = len(rec.calls)
        runtime.store.write_locator("genesis", "/m.md", _nested_sel(q),
                                    "/>", f"v{n}", auto_scaffold=False, god=True, proj="content")
        assert await _wait_fired(rec, n), f"写 {q} 超时"
        assert set(rec.calls[n:]) == {"@full", "@rec"}, f"写 {q}: {rec.calls[n:]}"


async def test_nested_hook_matrix_mix_deepest_leaf(runtime):
    """MIX 4 层交替容器（BLT/BQ 交错）最深叶：完整链写触发递归 hook；容器行写不触发叶。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$..m5", "@m5")

    # 1) 完整链（跨 4 容器 2 BQ 壳）写 m5
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$MIX.m1.m2.m3.m4.m5"),
                                "/>", "hi9", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "写 m5 超时"
    assert rec.calls[n:] == ["@m5"], rec.calls[n:]

    # 2) 写容器 m1 行自身（m5 行字节不变）→ m5 静默
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$MIX.m1"),
                                "/>", "mm", auto_scaffold=False, god=True, proj="content")
    assert await _wait_silent(rec, n), f"容器行写不应触发叶: {rec.calls[n:]}"


async def test_nested_hook_matrix_lifecycle(runtime):
    """生命周期：once 触发即删；从无到有触发；目标消失（删行）触发。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k2_2.b_bq_blt_1", "@once", always=False)
    _reg(runtime.store, "/m.md", "$BLT_DEEP.k9", "@k9")              # 注册时 k9 不存在
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2.qnb1.qnb1_1", "@nb1_1")

    # 1) once 首写触发；再写静默（已自动删除）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k2_2.b_bq_blt_1"),
                                "/>", "o1", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "once 首写超时"
    assert rec.calls[n:] == ["@once"], rec.calls[n:]
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k2_2.b_bq_blt_1"),
                                "/>", "o2", auto_scaffold=False, god=True, proj="content")
    assert await _wait_silent(rec, n), f"once 二次写不应触发: {rec.calls[n:]}"

    # 2) 从无到有：+> 创建 k9 → 触发；再写内容 → 常驻再触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP"),
                                "+>", "- k9:  appear", auto_scaffold=False, god=True)
    assert await _wait_fired(rec, n), "+> 创建 k9 超时"
    assert rec.calls[n:] == ["@k9"], rec.calls[n:]
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BLT_DEEP.k9"),
                                "/>", "appear2", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "写 k9 超时"
    assert rec.calls[n:] == ["@k9"], rec.calls[n:]

    # 3) 目标消失：删 qnb1_1 行 → 触发
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BQ_RICH.q2.qnb1.qnb1_1"),
                                "->", "", auto_scaffold=False, god=True)
    assert await _wait_fired(rec, n), "删 qnb1_1 超时"
    assert rec.calls[n:] == ["@nb1_1"], rec.calls[n:]


@pytest.mark.skip(reason="write API no longer renames heading via content projection; old-anchor premise expired")
async def test_nested_hook_matrix_heading_rename(runtime):
    """heading 行写（旧假设 = 整节替换/改名）→ 链下全部 hook 触发；
    新锚写入后原链静默。当前 content 投影不会重命名 heading tag，前提已失效。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2", "@q2h")
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2.qnb1", "@nb1")
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2.qnb1.qnb1_1", "@nb1_1")

    # 1) 写 q2 heading → 三 hook 全触发（heading 自身 + 链下目标消失）
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BQ_RICH.q2"),
                                "/>", "q2R", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "写 q2 heading 超时"
    assert set(rec.calls[n:]) == {"@q2h", "@nb1", "@nb1_1"}, rec.calls[n:]

    # 2) 新锚（q2R）下写 qnb1 → 原链（q2）hook 静默
    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BQ_RICH.q2R.qnb1"),
                                "/>", "v9", auto_scaffold=False, god=True, proj="content")
    assert await _wait_silent(rec, n), f"原链 hook 不应在新锚写后触发: {rec.calls[n:]}"


async def test_nested_hook_matrix_repeat_writes(runtime):
    """稳定性守卫：连续 8 次写同一目标，每次写窗口内必触发（无触发丢失）。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$BQ_RICH.q2.qnb1", "@nb1")

    for i in range(8):
        n = len(rec.calls)
        runtime.store.write_locator("genesis", "/m.md", _nested_sel("$BQ_RICH.q2.qnb1"),
                                    "/>", f"v{i}", auto_scaffold=False, god=True, proj="content")
        assert await _wait_fired(rec, n, timeout=3.0), f"第 {i} 次写未触发（触发丢失）"


async def test_nested_hook_matrix_drill_exact_match(runtime):
    """Drill 注册目标 = tag/标题文本精确匹配：无前缀/大小写模糊命中。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_doc(runtime.store, "/e.md", "# T\n- alpha: 1\n- alpha2: 2\n")
    _reg(runtime.store, "/e.md", "$..alpha", "@alpha")
    _reg(runtime.store, "/e.md", "$..alpha2", "@alpha2")

    async def expect(q, payload, want):
        n = len(rec.calls)
        runtime.store.write_locator("genesis", "/e.md", _nested_sel(q),
                                    "/>", payload, auto_scaffold=False, god=True, proj="content")
        assert await _wait_fired(rec, n), f"写 {q} 超时"
        assert rec.calls[n:] == [want], f"写 {q}: {rec.calls[n:]} != {[want]}"

    await expect("$..alpha", "1x", "@alpha")
    await expect("$..alpha2", "2x", "@alpha2")


async def test_nested_hook_matrix_bare_item_scaffold(runtime):
    """陷阱锁定：级联链断在裸项（tag=''）→ 落空写静默 scaffold 追加标题树，
    auto_scaffold=False 不拦截——追加标题被同名递归 hook 假命中。
    属写面 B 类行为（记录不修），本测试锁当前行为防静默漂移。"""
    rec = _TriggerRecorder()
    runtime.store.register_hook_dispatcher(rec)
    _write_nested(runtime.store)
    _reg(runtime.store, "/m.md", "$..k1_2_1_1_1", "@rec_leaf")

    n = len(rec.calls)
    runtime.store.write_locator("genesis", "/m.md",
                                _nested_sel("$BLT_DEEP.k1_2.k1_2_1.k1_2_1_1.k1_2_1_1_1"),
                                "/>", "deepX", auto_scaffold=False, god=True, proj="content")
    assert await _wait_fired(rec, n), "scaffold 追加标题应被递归 hook 命中（触发）"
    text = runtime.store.get_text("genesis", "/m.md")
    assert "#### k1_2_1_1" in text, "断链落空应 scaffold 追加标题树（当前写面行为）"
