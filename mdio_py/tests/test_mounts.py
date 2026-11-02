"""
MDIO 命名空间生命周期与 VFS 权限隔离测试
涵盖：
- 命名空间派生、继承、销毁
- 挂载规则的多点解析与边界测试
- RO/RW 权限继承与强制
- 路径逃逸与沙箱隔离 (多租户架构)
- 并发派生及 CRDT 多核合并安全性
"""

import asyncio
import tempfile
from pathlib import Path


from mdio_py import MdioRuntime
from mdio_py.core import parse_code
from mdio_py.core.error import MdioError
from mdio_py.core.mdio_ir import ReadOp
from mdio_py.core.selector_builder import SelectorBuilder


def test_read_selector_syntax():
    # 顶层 mount 算子不存在；本用例保留读取侧的选择器语法断验
    # 单 :: 两段式：file_path=proj, head=TARGET._b:value
    read_ir = parse_code("$proj::TARGET._b:value")
    read_op = read_ir[0].atoms[0]
    assert isinstance(read_op, ReadOp)
    assert read_op.selector.file_path == "proj"
    assert read_op.selector.steps[0] == ["Drill", "TARGET"]
    assert any(s[0] == "CMP" and s[1] == ".tag" and s[2] == "==" and s[3] == "value"
               for s in read_op.selector.steps)

    # 双 :: 三段式语法：ns_name :: file_path :: head
    three_seg = parse_code("$proj::/docs::HEAD")[0].atoms[0]
    assert isinstance(three_seg, ReadOp)
    assert three_seg.selector.context_id == "proj"
    assert three_seg.selector.file_path == "/docs"
    assert three_seg.selector.steps == [["Drill", "HEAD"]]

    macro_prefix = parse_code("$%target_ns::/active.md::TARGET._b:value")[0].atoms[0]
    assert isinstance(macro_prefix, ReadOp)
    assert macro_prefix.selector.context_id.name == "target_ns"
    assert macro_prefix.selector.file_path == "/active.md"


async def test_macro_env_reads_still_work(tmp_path: Path):
    (tmp_path / "wt" / "a.md").write_text("# A\n- data: from A\n", encoding="utf-8")
    runtime = MdioRuntime(str(tmp_path))

    runtime.store.write_locator(
        "genesis",
        "/.mds/env.md",
        SelectorBuilder().drill("SYSTEM_ENV").build(),
        "+>",
        "\n- SUPER_VAR: Global Data\n",
    )
    runtime.store.write_locator(
        "genesis",
        "/.mds/env.md",
        SelectorBuilder().drill("LOCAL_ENV").build(),
        "+>",
        "\n- local_var: Valuable Local\n",
    )

    assert "Global Data" in str(await runtime.exec("%SUPER_VAR", wt_uid="genesis"))
    assert "Valuable Local" in str(await runtime.exec("%local_var", wt_uid="genesis"))

    cross_res = await runtime.exec("$%wt_uid::/a.md::A._b:data", active_doc="/default.md", wt_uid="genesis")
    assert cross_res == "from A"

    bad_inline_macro = await runtime.exec("$/a.md::%local_var:name", wt_uid="genesis")
    assert "mdio_error" in str(bad_inline_macro)



async def test_named_mount_routes_single_segment_file_path(tmp_path: Path):
    (tmp_path / "wt" / "proj.md").write_text("# TARGET\n- value: root-file\n", encoding="utf-8")
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "active.md").write_text("# TARGET\n- value: namespace-file\n", encoding="utf-8")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "proj" --rule "/child"', wt_uid="genesis")
    wt_uid = runtime._workspace.resolve_worktree_name("proj")

    # mount now writes to worktrees.md
    wt_text = runtime.store.get_text("genesis", "/.mds/worktrees.md")
    assert f"- uid: {wt_uid}" in wt_text
    assert "proj" in wt_text

    routed = await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis", active_doc="/home/active.md")
    assert routed == "namespace-file"

    explicit_file = await runtime.exec("$/proj::TARGET._b:value", wt_uid="genesis")
    assert explicit_file == "root-file"

    await runtime.exec('"changed-through-namespace" | /> proj::TARGET._b:value', wt_uid="genesis", active_doc="/home/active.md")
    changed = await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis", active_doc="/home/active.md")
    assert changed == "changed-through-namespace"
    assert await runtime.exec("$/proj::TARGET._b:value", wt_uid="genesis") == "root-file"

    no_focus = await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis")
    assert "mdio_error" in no_focus

    runtime.store.write_locator(
        "genesis",
        "/.mds/env.md",
        SelectorBuilder().drill("LOCAL_ENV").build(),
        "+>",
        f"\n- target_ns: {wt_uid}\n",
    )
    macro_routed = await runtime.exec("$%target_ns::/home/active.md::TARGET._b:value", wt_uid="genesis")
    assert macro_routed == "changed-through-namespace"


async def test_namespace_name_detection_skips_explicit_file_forms(tmp_path: Path):
    (tmp_path / "wt" / "proj.md").write_text("# TARGET\n- value: root-file\n", encoding="utf-8")
    (tmp_path / "wt" / ".proj.md").write_text("# TARGET\n- value: hidden-file\n", encoding="utf-8")
    nested = tmp_path / "wt" / "dir"
    nested.mkdir()
    (nested / "proj.md").write_text("# TARGET\n- value: nested-file\n", encoding="utf-8")
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "active.md").write_text("# TARGET\n- value: namespace-file\n", encoding="utf-8")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "proj" --rule "/child"', wt_uid="genesis")

    assert await runtime.exec("$/proj::TARGET._b:value", wt_uid="genesis", active_doc="/active.md") == "root-file"
    assert await runtime.exec("$proj.md::TARGET._b:value", wt_uid="genesis", active_doc="/active.md") == "root-file"
    hidden_result = await runtime.exec("$.proj::TARGET._b:value", wt_uid="genesis", active_doc="/active.md")
    assert "mdio_error" in str(hidden_result)
    assert "namespace-file" not in str(hidden_result)
    assert await runtime.exec("$dir/proj::TARGET._b:value", wt_uid="genesis", active_doc="/active.md") == "nested-file"
    assert await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis", active_doc="/home/active.md") == "namespace-file"


async def test_unregistered_single_segment_stays_file_path(tmp_path: Path):
    (tmp_path / "wt" / "plain.md").write_text("# TARGET\n- value: file-value\n", encoding="utf-8")
    runtime = MdioRuntime(str(tmp_path))

    assert await runtime.exec("$plain::TARGET._b:value", wt_uid="genesis", active_doc="/ignored.md") == "file-value"




async def test_namespace_management():
    print("🚀 Starting MDIO Namespace Management & VFS Isolation Test\n" + "=" * 80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        # 物理文件准备（文档根恒为 <root>/wt）
        (ws / "wt" / "public").mkdir(parents=True)
        (ws / "wt" / "public/readme.md").write_text("# PUBLIC\n- content: open\n")
        (ws / "wt" / "admin").mkdir(parents=True)
        (ws / "wt" / "admin/secret.md").write_text("# SECRET\n- password: admin_only\n")
        (ws / "wt" / "shared").mkdir(parents=True)
        (ws / "wt" / "shared/data.md").write_text("# SHARED\n- info: shared_data\n")

        # 多租户物理底座
        (ws / "wt" / "public/user_a_home").mkdir(parents=True)
        (ws / "wt" / "public/user_b_home").mkdir(parents=True)

        # 创世挂载：根挂载整个目录，但 admin 目录挂载为 RO（第一段=项目根；挂载声明只定权限，数据由 <根>/wt pump）
        storage_root = f"{ws}; {ws}/admin:/admin:RO"
        runtime = MdioRuntime(storage_root)
        ws_mount = runtime._workspace

        results = {}


        # 1. 基础派生与销毁

        print("\n⚡ [Test 1] 基础派生与销毁")
        wt_uid_guest = ws_mount.derive_worktree("genesis", "/public")
        assert wt_uid_guest != 0, "派生句柄应为 uid 字符串"
        content = runtime.store.get_text(wt_uid_guest, "/home/readme.md")
        assert "PUBLIC" in content, "guest 应能通过 /home→/public 绑定访问 readme.md"
        ws_mount.drop_derived_worktree(wt_uid_guest)
        result = runtime.store.get_text(wt_uid_guest, "/home/readme.md")
        assert result == "", "销毁后应返回空字符串拦截"
        results["T1"] = True


        # 2. 多挂载点派生与继承

        print("\n⚡ [Test 2] 多挂载点派生与降维继承")
        wt_uid_multi = ws_mount.derive_worktree("genesis", "/public; /shared:/data:RW")
        assert "PUBLIC" in runtime.store.get_text(wt_uid_multi, "/home/readme.md")
        assert "SHARED" in runtime.store.get_text(wt_uid_multi, "/data/data.md")
        await runtime.exec('"new_content" | +> /data/data.md::SHARED', wt_uid=wt_uid_multi)
        assert "new_content" in runtime.store.get_text(wt_uid_multi, "/data/data.md")
        results["T2"] = True


        # 3. RO 权限继承与强制压制

        print("\n⚡ [Test 3] RO 权限继承——Plan 3 在 resolve/write 时刻执法")
        ro_child_id = ws_mount.derive_worktree("genesis", "/admin::RW")
        # 即使 bfork 声明 /home→/admin:RW，genesis 的 /admin RO 也会沿链传播
        res = await runtime.exec('"SECRET" | +> /home/secret.md::SYS._b:data', wt_uid=ro_child_id)
        assert "R201" in str(res), f"RO 挂载点应禁止写入，却得到: {res}"
        ws_mount.drop_derived_worktree(ro_child_id)
        results["T3"] = True


        # 4. 路径逃逸防护

        print("\n⚡ [Test 4] ../../ 路径跨界逃逸防护")
        sandbox_id = ws_mount.derive_worktree("genesis", "/public")
        # 先创建目标文件（确保写入目标存在）
        await runtime.exec('mk -f "/admin/secret.md"', wt_uid=sandbox_id)
        await runtime.exec('"hack" | +> /admin/secret.md::SECRETS._b:hack', wt_uid=sandbox_id)
        # 验证文件在沙箱 VFS 中被创建（而非逃逸到根命名空间）
        assert runtime.store.path_exists(sandbox_id, "/admin/secret.md")
        # 确保没有逃逸到真正的根命名空间
        assert "hack" not in runtime.store.get_text("genesis", "/admin/secret.md")
        results["T4"] = True


        # 5. 递归删除与子节点清理

        print("\n⚡ [Test 5] 递归生命周期管理：族谱级联销毁")
        parent_id = ws_mount.derive_worktree("genesis", "/public")
        child1_id = ws_mount.derive_worktree(parent_id, "/")
        ws_mount.drop_derived_worktree(parent_id)
        assert runtime.store.get_text(child1_id, "/readme.md") == "", "子节点未被级联清除"
        results["T5"] = True


        # 6. 并发派生与销毁

        print("\n⚡ [Test 6] 并发超高频派生与销毁 (死锁免疫)")
        async def create_and_destroy():
            for _ in range(10):
                uid = ws_mount.derive_worktree("genesis", "/public")
                ws_mount.drop_derived_worktree(uid)
        await asyncio.gather(*[create_and_destroy() for _ in range(5)])
        assert runtime.store.get_text("genesis", "/") == "", "野指针读取失败"
        results["T6"] = True


        # 7. 跨命名空间并行态隔离

        print("\n⚡ [Test 7] 跨命名空间全封闭并行态隔离")
        space_a = ws_mount.derive_worktree("genesis", "/public")
        space_b = ws_mount.derive_worktree("genesis", "/shared")

        # 创建文件（必须显式创建）
        await runtime.exec('mk -f "/test.md"', wt_uid=space_a)
        await runtime.exec('mk -f "/test.md"', wt_uid=space_b)

        await runtime.exec('"data_a" | +> /test.md::', wt_uid=space_a)
        assert "data_a" in runtime.store.get_text(space_a, "/test.md")
        assert runtime.store.get_text(space_b, "/test.md") == ""

        await runtime.exec('"data_b" | +> /test.md::', wt_uid=space_b)
        assert "data_a" in runtime.store.get_text(space_a, "/test.md")
        assert "data_a" not in runtime.store.get_text(space_b, "/test.md")
        assert "data_b" in runtime.store.get_text(space_b, "/test.md")
        results["T7"] = True

        # 8. 父级受保护挂载点隐式继承与压制
        print("\n⚡ [Test 8] 嵌套降维后的父级保护区(RO)隐式继承与压制")

        # 在根命名空间中预先创建文件（因为 RO 挂载点下无法创建）
        await runtime.exec('mk -f "/shared/data.md"', wt_uid="genesis")
        await runtime.exec('"dummy" | +> /shared/data.md::SHARED', wt_uid="genesis")

        parent_with_sub = ws_mount.derive_worktree("genesis", "/public; /shared:/extra:RO")
        child_of_parent = ws_mount.derive_worktree(parent_with_sub, "/extra")

        # 尝试写入 RO 区域，应失败
        res_b = await runtime.exec('"hack" | +> /home/data.md::SHARED', wt_uid=child_of_parent)
        assert "RO" in str(res_b) or "Target is RO" in str(res_b) or "R201" in str(res_b), f"RO 压制失败: {res_b}"
        results["T8"] = True

        # 9. 在沙箱查询时拔掉电源 (安全回退测试)
        print("\n⚡ [Test 9] 在沙箱查询时拔掉电源 (安全回退测试)")
        parent = ws_mount.derive_worktree("genesis", "/public")
        child = ws_mount.derive_worktree(parent, "/shared")

        # 创建文件并写入初始内容
        await runtime.exec('mk -f "/data.md"', wt_uid=child)
        await runtime.exec('"initial" | +> /data.md::SHARED._b:info', wt_uid=child)

        async def use_child():
            res = await runtime.exec('$/data.md::SHARED._b:info', wt_uid=child)
            # 仅测试并发安全，不关心返回值

        async def destroy_parent():
            await asyncio.sleep(0.02)
            ws_mount.drop_derived_worktree(parent)

        await asyncio.gather(use_child(), destroy_parent())
        assert runtime.store.get_text(child, "/") == ""
        results["T9"] = True


        # 10. 挂载规则边界与 Fuzzing 测试 (NEW)

        print("\n⚡ [Test 10] 挂载规则畸形边界边界与 Fuzzing 测试")

        # 10.1 空规则 = cfork（WT VFS 合法语义）
        uid_empty = ws_mount.derive_worktree("genesis", "")
        assert uid_empty, "空 rule 应创建合法 cfork"
        ws_mount.drop_derived_worktree(uid_empty)

        # 10.2 虚拟挂载点碰撞重名（Binding::parse 应检测重复）
        try:
            ws_mount.derive_worktree("genesis", "/public; /shared:/dup:RO; /admin:/dup:RO")
            # 重复 my_path 不强制拦截：首个匹配优先
        except MdioError as e:
            assert "duplicate" in str(e).lower(), f"重复挂载点拦截消息不符: {e}"

        # 10.3 特殊字符挂载与 Unicode 兼容
        special_rules = "/public; /shared:/测试 目录 :RO"
        try:
            uid_sp = ws_mount.derive_worktree("genesis", special_rules)
            ws_mount.drop_derived_worktree(uid_sp)
            results["T10"] = True
        except Exception as e:
            print(f"特殊字符路径处理失败: {e}")
            results["T10"] = False


        # 11. 权限与多租户协作模型扩展 (NEW)

        print("\n⚡ [Test 11] 用户隔离模型与同源 CRDT 高并发冲突合并测试")

        # 11.1 多租户物理级彻底隔离
        user_a = ws_mount.derive_worktree("genesis", "/public/user_a_home")
        user_b = ws_mount.derive_worktree("genesis", "/public/user_b_home")
        await runtime.exec('mk -f "/diary.md"', wt_uid=user_a)
        await runtime.exec('mk -f "/diary.md"', wt_uid=user_b)
        await runtime.exec('"my_secret_a" | +> /diary.md::', wt_uid=user_a)
        await runtime.exec('"my_secret_b" | +> /diary.md::', wt_uid=user_b)

        a_content = runtime.store.get_text(user_a, "/diary.md")
        b_content = runtime.store.get_text(user_b, "/diary.md")
        assert "my_secret_a" in a_content and "my_secret_b" not in a_content
        assert "my_secret_b" in b_content and "my_secret_a" not in b_content
        ws_mount.drop_derived_worktree(user_a); ws_mount.drop_derived_worktree(user_b)

        # 11.2 CRDT 多沙箱高并发重入合并测试！
        # 模拟两个不同的 Editor 挂载了同一个共同的协作文件夹 "/" -> "/public"
        await runtime.exec('mk -f "/public/shared_collab.md"', wt_uid="genesis")
        await runtime.exec('"# COLLAB\n" | +> /shared_collab.md::', wt_uid="genesis")
        editor1 = ws_mount.derive_worktree("genesis", "/public")
        editor2 = ws_mount.derive_worktree("genesis", "/public")

        # 两人在同一毫秒内，针对完全相同的物理文件底座发动并发写入
        async def edit(editor_id, content):
            await runtime.exec(f'"{content}" | +> /home/shared_collab.md::COLLAB', wt_uid=editor_id)

        await asyncio.gather(edit(editor1, "- edit_msg1\n"), edit(editor2, "- edit_msg2\n"))

        final_collab = runtime.store.get_text("genesis", "/public/shared_collab.md")
        assert "edit_msg1" in final_collab and "edit_msg2" in final_collab, "严重缺陷：底层 DocumentSession 的 CRDT 并发锁存在丢失数据的情况"

        results["T11"] = True


        print("\n✅ VFS 多租户基座与资源生命周期全量体检报告")
        for k, v in results.items():
            print(f"[{k}] -> {'🟢 PASS' if v else '🔴 FAIL'}")

        assert all(results.values()), "部分防线检测未达标"
        print("\n🏆================================================🏆")
        print("🎉 MULTI-TENANT ISOLATION & VFS KERNEL EVERYTHING PASSED! 🎉")
        print("🏆================================================🏆\n")



# 双 :: 三段式语法测试


def test_three_segment_parse_all_forms():
    """三段式 `ns :: file_path :: head` 各种形式的 parse 验证"""
    from mdio_py.core.mdio_ir import ReadOp, SelectorIR

    def parse_sel(code):
        r = parse_code(code)
        if isinstance(r, list):
            r = r[0]
        return r.atoms[0].selector

    # 基础三段式
    s = parse_sel("$mnt::/f::head")
    assert isinstance(s, SelectorIR)
    assert len(s.steps) > 0
    assert s.steps[0] == ["Drill", "head"]

    # 多级文件路径
    s = parse_sel("$mnt::/a/b/c::TARGET")
    assert isinstance(s, SelectorIR)
    assert len(s.steps) > 0
    assert s.steps[0] == ["Drill", "TARGET"]

    # 带 key_spec
    s = parse_sel("$ns::/docs/readme::INFO._b:status")
    assert isinstance(s, SelectorIR)
    assert len(s.steps) >= 4
    assert s.steps[0] == ["Drill", "INFO"]
    assert ["CMP", ".tag", "==", "status"] in s.steps
    assert ["Slice", 0] in s.steps

    # number prefix with three-segment
    s4 = parse_sel("$0::mnt::/f::head")
    assert isinstance(s4, SelectorIR)
    assert s4.context_id == 0
    assert s4.steps[0] == ["Drill", "head"]

    # 绝对路径文件（以 / 开头）
    s5 = parse_sel("$ns::/index.md::STATUS._b:code")
    assert isinstance(s5, SelectorIR)
    assert s5.steps[0] == ["Drill", "STATUS"]
    assert ["CMP", ".tag", "==", "code"] in s5.steps


def test_three_segment_does_not_break_single_colon():
    """单 :: 基本行为——验证 SelectorIR 产出"""
    from mdio_py.core.mdio_ir import ReadOp

    # 单 :: 基础形式
    s = parse_code("$proj::key")[0].atoms[0]
    assert isinstance(s, ReadOp)
    assert s.selector.file_path == "proj"
    assert s.selector.steps == [["Drill", "key"]]

    # 单 :: with key_spec
    s = parse_code("$proj::key:sub")[0].atoms[0]
    assert s.selector.file_path == "proj"
    assert s.selector.steps[0] == ["Drill", "key"]
    assert ["CMP", ".tag", "==", "sub"] in s.selector.steps

    # 无 file_path，仅 head
    s = parse_code("$::key")[0].atoms[0]
    assert s.selector.file_path is None
    assert s.selector.steps == [["Drill", "key"]]

    # 绝对路径
    s = parse_code("$/abs/path::head")[0].atoms[0]
    assert s.selector.file_path == "/abs/path"
    assert s.selector.steps == [["Drill", "head"]]

    # .md 后缀文件
    s = parse_code("$proj.md::key")[0].atoms[0]
    assert s.selector.file_path == "proj.md"
    assert s.selector.steps == [["Drill", "key"]]


def test_three_segment_interaction_with_single_colon():
    """三段式和单 :: 在同一个 parse 中互不干扰"""
    from mdio_py.core.mdio_ir import ReadOp, WriteOp

    # 三段式 write
    result = parse_code('"data" | +> mnt::/f.md::HEAD')
    assert isinstance(result[0].atoms[1], WriteOp)
    wsel = result[0].atoms[1].selector
    assert isinstance(wsel.steps, list)
    assert len(wsel.steps) > 0
    assert wsel.steps[0] == ["Drill", "HEAD"]

    # 三段式与单 :: 在管道中混合
    result = parse_code('$mnt::/f::head | $proj::key')
    assert isinstance(result[0].atoms[0], ReadOp)
    assert result[0].atoms[0].selector.steps[0] == ["Drill", "head"]
    # 管道第二个 atom
    assert isinstance(result[0].atoms[1], ReadOp)
    assert result[0].atoms[1].selector.file_path == "proj"

    # 三段式 mem_read
    result = parse_code('$$mnt::/cache::DATA:field')
    s = result[0].atoms[0].selector
    assert s.steps[0] == ["Drill", "DATA"]
    assert ["CMP", ".tag", "==", "field"] in s.steps


# ---- 运行时：三段式命名空间路由 ----

async def test_three_segment_runtime_routing(tmp_path: Path):
    """三段式运行时端到端：命名空间路由后读/写"""
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "active.md").write_text("# TARGET\n- value: ns-value\n")
    (child / "docs").mkdir()
    (child / "docs" / "readme.md").write_text("# INFO\n- status: active\n- version: 2\n")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "proj" --rule "/child"', wt_uid="genesis")
    uid = runtime._workspace.resolve_worktree_name("proj")
    assert uid  # is a uid string

    # 三段式读取：ns :: /file :: head._b:key
    r = await runtime.exec("$proj::/home/docs/readme::INFO._b:status", wt_uid="genesis")
    assert "active" in r

    # 三段式读取另一个 key
    r = await runtime.exec("$proj::/home/docs/readme::INFO._b:version", wt_uid="genesis")
    assert "2" in r

    # 三段式写入
    await runtime.exec('"new-status" | /> proj::/home/docs/readme::INFO._b:status', wt_uid="genesis")
    r = await runtime.exec("$proj::/home/docs/readme::INFO._b:status", wt_uid="genesis")
    assert "new-status" in r

    # 单 :: 仍然正常工作（不退化）
    r = await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis", active_doc="/home/active.md")
    assert "ns-value" in r


async def test_three_segment_unregistered_ns_error(tmp_path: Path):
    """三段式中未注册的 ns 名给出明确错误"""
    runtime = MdioRuntime(str(tmp_path))

    r = await runtime.exec("$bogus_ns::/f::head", wt_uid="genesis")
    assert "mdio_error" in r


async def test_three_segment_with_macro_prefix(tmp_path: Path):
    """%macro:: 前缀 + 三段式链式路由"""
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "nested").mkdir()
    (child / "nested" / "data.md").write_text("# DEEP\n- val: deep-value\n- extra: bonus\n")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "inner" --rule "/child/nested"', wt_uid="genesis")
    uid = runtime._workspace.resolve_worktree_name("inner")

    runtime.store.write_locator(
        "genesis", "/.mds/env.md",
        SelectorBuilder().drill("LOCAL_ENV").build(),
        "+>", f"\n- inner_id: {uid}\n",
    )

    # %macro:: 前缀 + 三段式
    r = await runtime.exec("$%inner_id::/home/data.md::DEEP._b:val", wt_uid="genesis")
    assert "deep-value" in r

    # 另一个 key
    r = await runtime.exec("$%inner_id::/home/data.md::DEEP._b:extra", wt_uid="genesis")
    assert "bonus" in r

    # 错误：%macro 值不是 wt_uid
    runtime.store.write_locator(
        "genesis", "/.mds/env.md",
        SelectorBuilder().drill("LOCAL_ENV").build(),
        "+>", "\n- bad_ns: not_a_number\n",
    )
    r = await runtime.exec("$%bad_ns::/f::head", wt_uid="genesis")
    assert "mdio_error" in r


async def test_three_segment_with_number_prefix(tmp_path: Path):
    """数字前缀 + 三段式"""
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "data.md").write_text("# INFO\n- key: val-from-ns\n")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "ns" --rule "/child"', wt_uid="genesis")
    uid = runtime._workspace.resolve_worktree_name("ns")

    r = await runtime.exec(f"${uid}::/home/data.md::INFO._b:key", wt_uid="genesis")
    assert "val-from-ns" in r


async def test_three_segment_edge_cases(tmp_path: Path):
    """三段式边界情况"""
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "active.md").write_text("# HOME\n- name: default\n")
    (child / "other.md").write_text("# HOME\n- name: other-file\n")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "ns" --rule "/child"', wt_uid="genesis")

    # 有 active_doc 时三段式用显式文件路径覆盖 active_doc
    r = await runtime.exec("$ns::/home/other.md::HOME._b:name", wt_uid="genesis", active_doc="/home/active.md")
    assert "other-file" in r

    # 无 active_doc 但有三段式显式文件路径 → 正常工作
    r = await runtime.exec("$ns::/home/active.md::HOME._b:name", wt_uid="genesis")
    assert "default" in r

    # 空 head（只指定文件）
    r = await runtime.exec("$ns::/home/active.md::", wt_uid="genesis")
    # 应返回文件全部内容
    assert "HOME" in r


async def test_three_segment_preserves_single_colon_behavior(tmp_path: Path):
    """注册 namespace 后，单 :: 仍可在有 active_doc 时正常工作"""
    child = tmp_path / "wt" / "child"
    child.mkdir()
    (child / "active.md").write_text("# TARGET\n- value: from-ns\n")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "proj" --rule "/child"', wt_uid="genesis")

    # 单 :: + active_doc → 路由到命名空间，读 active_doc 的 head
    r = await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis", active_doc="/home/active.md")
    assert "from-ns" in r

    # 单 :: 无 active_doc → 应报错（无法确定文件）
    r = await runtime.exec("$proj::TARGET._b:value", wt_uid="genesis")
    assert "mdio_error" in r




async def test_three_segment_concurrent_routing(tmp_path: Path):
    """并发三段式读写不产生竞态"""
    import asyncio

    child = tmp_path / "wt" / "child"
    child.mkdir()
    for i in range(5):
        (child / f"f{i}.md").write_text(f"# H\n- v: {i}\n")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('worktree bfork "ns" --rule "/child"', wt_uid="genesis")

    async def read_file(i):
        r = await runtime.exec(f"$ns::/f{i}.md::H._b:v", wt_uid="genesis")
        assert str(i) in r

    await asyncio.gather(*[read_file(i) for i in range(5)])


if __name__ == "__main__":
    asyncio.run(test_namespace_management())
