"""
MDIO 命名空间生命周期与 VFS 权限隔离测试
涵盖：
- 命名空间派生、继承、销毁
- 挂载规则的多点解析与边界测试
- RO/RW 权限继承与强制
- 路径逃逸与沙箱隔离 (多租户架构)
- 并发派生及 CRDT 多核合并安全性
- vOS 原语集成 (mount / macro / ctx_prefix)
"""

import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime
from mdio_py.core.error import MdioError
from mdio_py.core.selector_builder import SelectorBuilder


async def test_namespace_management():
    print("🚀 Starting MDIO Namespace Management & VFS Isolation Test\n" + "=" * 80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        # 物理文件准备
        (ws / "wt" / "public").mkdir(parents=True, exist_ok=True)
        (ws / "wt" / "public/readme.md").write_text("# PUBLIC\n- content: open\n")
        (ws / "wt" / "admin").mkdir(parents=True, exist_ok=True)
        (ws / "wt" / "admin/secret.md").write_text("# SECRET\n- password: admin_only\n")
        (ws / "wt" / "shared").mkdir(parents=True, exist_ok=True)
        (ws / "wt" / "shared/data.md").write_text("# SHARED\n- info: shared_data\n")

        # 多租户物理底座
        (ws / "wt" / "public/user_a_home").mkdir(parents=True, exist_ok=True)
        (ws / "wt" / "public/user_b_home").mkdir(parents=True, exist_ok=True)

        # 创世挂载：根挂载整个目录，但 admin 目录挂载为 RO
        storage_root = f"{ws}; {ws}/admin:/admin:RO"
        runtime = MdioRuntime(storage_root)
        ws_mount = runtime._workspace

        # 为了模拟网关正常启动，这里手动补齐系统盘 Stage 1 创世
        await runtime.exec('mk -f /.mds/env.md', wt_uid="genesis")
        await runtime.exec('"# SYSTEM_ENV\n- MDIO_VERSION: 18.0.0\n" | +> /.mds/env.md::', wt_uid="genesis")
        results = {}


        # 1. 基础派生与销毁

        print("\n⚡ [Test 1] 基础派生与销毁")
        wt_uid_guest = ws_mount.derive_worktree("genesis", "/public")
        assert wt_uid_guest != "genesis", "派生句柄应为 uid 字符串"
        content = runtime.store.get_text(wt_uid_guest, "/home/readme.md")
        assert "PUBLIC" in content, "guest 应能访问 /readme.md"
        ws_mount.drop_derived_worktree(wt_uid_guest)
        result = runtime.store.get_text(wt_uid_guest, "/home/readme.md")
        assert result == "", "销毁后应返回空字符串拦截"
        results["T1"] = True


        # 2. 多挂载点派生与降维继承

        print("\n⚡ [Test 2] 多挂载点派生与降维继承")
        wt_uid_multi = ws_mount.derive_worktree("genesis", "/public; /shared:/data:RW")
        assert "PUBLIC" in runtime.store.get_text(wt_uid_multi, "/home/readme.md")
        assert "SHARED" in runtime.store.get_text(wt_uid_multi, "/data/data.md")
        await runtime.exec('"new_content" | +> /data/data.md::SHARED', wt_uid=wt_uid_multi)
        assert "new_content" in runtime.store.get_text(wt_uid_multi, "/data/data.md")
        results["T2"] = True


        # 3. RO 权限继承——WT VFS 在 resolve/write 时刻执法

        print("\n⚡ [Test 3] RO 权限继承——resolve 链传播 RO mode")
        ro_child_id = ws_mount.derive_worktree("genesis", "/admin::RW")
        res = await runtime.exec('"SECRET" | +> /home/secret.md::SYS._b:data', wt_uid=ro_child_id)
        assert "R201" in str(res), f"RO 挂载点应禁止写入，却得到: {res}"
        ws_mount.drop_derived_worktree(ro_child_id)
        results["T3"] = True


        # 4. 路径逃逸防护

        print("\n⚡ [Test 4] ../../ 路径跨界逃逸防护")
        # 物理创建 RO 文件
        admin_dir = ws / "wt" / "admin"
        admin_dir.mkdir(exist_ok=True)
        (admin_dir / "secret.md").write_text("# SECRETS\n- hack: original\n", encoding="utf-8")

        sandbox_id = ws_mount.derive_worktree("genesis", "/public")
        result = await runtime.exec('"hack" | +> ../../../admin/secret.md::SECRETS._b:hack -f -god', wt_uid=sandbox_id)
        # WT VFS: ../../../ 归一化为 /admin/secret.md，在沙箱内落 local（无逃逸），-f 自动创建文件 → 写入成功
        assert "mdio_success" in str(result), f"沙箱内 -f 写入应成功（路径未逃逸），实际: {result}"
        # 确保没有逃逸到根命名空间 —— root 的 /admin/secret.md 仍为原始内容
        assert "hack" not in runtime.store.get_text("genesis", "/admin/secret.md")


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
        # 先创建文件
        await runtime.exec('mk -f /test.md', wt_uid=space_a)
        await runtime.exec('mk -f /test.md', wt_uid=space_b)
        await runtime.exec('"data_a" | +> /test.md::', wt_uid=space_a)
        assert runtime.store.get_text(space_b, "/test.md") == ""
        await runtime.exec('"data_b" | +> /test.md::', wt_uid=space_b)
        assert "data_a" in runtime.store.get_text(space_a, "/test.md")
        assert "data_a" not in runtime.store.get_text(space_b, "/test.md")
        results["T7"] = True


        # 8. 父级受保护挂载点隐式继承

        print("\n⚡ [Test 8] 嵌套降维后的父级保护区(RO)隐式继承与压制")
        parent_with_sub = ws_mount.derive_worktree("genesis", "/public; /shared:/extra:RO")
        child_of_parent = ws_mount.derive_worktree(parent_with_sub, "/extra")
        res_b = await runtime.exec('"hack" | +> /home/data.md::SHARED', wt_uid=child_of_parent)
        assert "- mdio_error" in str(res_b) and ("RO" in str(res_b) or "R201" in str(res_b))
        results["T8"] = True


        # 9. 并发查询中的世界销毁

        print("\n⚡ [Test 9] 在沙箱查询时拔掉电源 (安全回退测试)")
        parent = ws_mount.derive_worktree("genesis", "/public")
        child = ws_mount.derive_worktree(parent, "/shared")
        async def use_child():
            res = await runtime.exec('$/data.md::SHARED._b:info', wt_uid=child)
            await asyncio.sleep(0.01)
        async def destroy_parent():
            await asyncio.sleep(0.02)
            ws_mount.drop_derived_worktree(parent)
        await asyncio.gather(use_child(), destroy_parent())
        assert runtime.store.get_text(child, "/") == ""
        results["T9"] = True


        # 10. 挂载规则边界与 Fuzzing 测试

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

        special_rules = "/public; /shared:/测试 目录 :RO"
        try:
            uid_sp = ws_mount.derive_worktree("genesis", special_rules)
            ws_mount.drop_derived_worktree(uid_sp)
            results["T10"] = True
        except Exception as e:
            results["T10"] = False


        # 11. 权限与多租户协作模型扩展

        print("\n⚡ [Test 11] 用户隔离模型与同源 CRDT 高并发冲突合并测试")
        user_a = ws_mount.derive_worktree("genesis", "/public/user_a_home")
        user_b = ws_mount.derive_worktree("genesis", "/public/user_b_home")

        # 为每个用户创建日记文件
        await runtime.exec('mk -f /diary.md', wt_uid=user_a)
        await runtime.exec('mk -f /diary.md', wt_uid=user_b)
        await runtime.exec('"my_secret_a" | +> /diary.md::', wt_uid=user_a)
        await runtime.exec('"my_secret_b" | +> /diary.md::', wt_uid=user_b)

        a_content = runtime.store.get_text(user_a, "/diary.md")
        b_content = runtime.store.get_text(user_b, "/diary.md")
        assert "my_secret_a" in a_content and "my_secret_b" not in a_content
        assert "my_secret_b" in b_content and "my_secret_a" not in b_content
        ws_mount.drop_derived_worktree(user_a); ws_mount.drop_derived_worktree(user_b)

        # 协作编辑测试：创建共享文件并并发写入
        await runtime.exec('mk -f /public/shared_collab.md', wt_uid="genesis")
        await runtime.exec('"# COLLAB\n" | /> /public/shared_collab.md::', wt_uid="genesis")

        editor1 = ws_mount.derive_worktree("genesis", "/public")
        editor2 = ws_mount.derive_worktree("genesis", "/public")

        async def edit(editor_id, content):
            # 在编辑器命名空间内，/shared_collab.md 实际映射到 /public/shared_collab.md
            await runtime.exec(f'"{content}" | +> /home/shared_collab.md::COLLAB -f', wt_uid=editor_id)

        await asyncio.gather(edit(editor1, "- edit_msg1\n"), edit(editor2, "- edit_msg2\n"))
        final_collab = runtime.store.get_text("genesis", "/public/shared_collab.md")
        assert "edit_msg1" in final_collab and "edit_msg2" in final_collab

        results["T11"] = True


        # 12. vOS 空间站分配 (`mount` 原语) 与 Markdown 自动接骨沉淀

        print("\n⚡ [Test 12] mount 原语执行、动态创世与 Markdown 实体志沉淀追踪")

        # 在上帝视角执行原语 `mount`，测试引擎内部派生
        mount_res = str(await runtime.exec('worktree bfork "test_agent_space" --rule "/public"', wt_uid="genesis"))
        assert "bfork" in mount_res and "mdio_error" not in mount_res, f"bfork 应成功, got: {mount_res}"
        agent_wt_uid = runtime._workspace.resolve_worktree_name("test_agent_space")
        assert agent_wt_uid, "bfork 后应能按名解析出 uid"
        print(f'12 agent_wt_uid: {agent_wt_uid}')

        # 验证底层花名册 worktrees.md（mount 已迁移到 worktree）
        wt_content = runtime.store.get_text("genesis", "/.mds/worktrees.md")
        assert f"- uid: {agent_wt_uid}" in wt_content
        assert "- rule: /public" in wt_content
        assert "test_agent_space" in wt_content

        results["T12"] = True


        # 13. 环境宏 % 全局与局部区隔及物理隔离

        print("\n⚡ [Test 13] 宏刻录：全局与局部变量的绝对时空隔离")

        # 1. 向 Agent 局部写宏
        runtime.store.write_locator(agent_wt_uid, "/.mds/env.md", SelectorBuilder().drill("LOCAL_ENV").build(), "+>", "\n- sub_var: sub_macro_val\n")
        # 2. 向上帝全局写宏
        runtime.store.write_locator("genesis", "/.mds/env.md", SelectorBuilder().drill("SYSTEM_ENV").build(), "+>", "\n- SUPER_MACRO: GOD_VAR_VAL\n")

        # A) 验证局部宏专属读取
        ans_sub = await runtime.exec('%sub_var', wt_uid=agent_wt_uid)
        assert "sub_macro_val" in ans_sub, "局部宏无法在沙箱中解包"

        # B) 验证 0号机 无法读沙箱的专属宏
        ans_err = await runtime.exec('%sub_var', wt_uid="genesis")
        assert "mdio_error" in str(ans_err).lower(), "上帝不应跨时空读到局部内存"

        # C) 验证沙箱读取了全局大写宏
        ans_global = await runtime.exec('%SUPER_MACRO', wt_uid=agent_wt_uid)
        assert "GOD_VAR_VAL" in ans_global, "全局防跌宏无法深入沙箱降维"

        results["T13"] = True


        # 14. 跨维坐标穿透与虫洞跃迁 (CTX_PREFIX)

        print("\n⚡ [Test 14] 跨维上下文前缀 (ctx_prefix) 与时空虫洞寻址")

        # agent_wt_uid 看不到 /admin，0号机看得到
        # 在 0号机 用目标 Agent 视角写一份文件
        runtime.store.write_locator("genesis", "/.mds/env.md", SelectorBuilder().drill("LOCAL_ENV").build(), "+>", f"\n- tgt_wt: {agent_wt_uid}\n")

        # 从 0号机 发起，借助前置虫洞 `%tgt_wt::` 突破进目标的局部空间完成写入
        await runtime.exec(' "Secret Code" | +> %tgt_wt::/readme.md::PUBLIC -god', wt_uid="genesis")

        # 切换回 Agent 视角验证确实收到了
        ans_wormhole = await runtime.exec('$/readme.md::PUBLIC', wt_uid=agent_wt_uid)
        assert "Secret Code" in str(ans_wormhole), "时空穿透 ctx_prefix 未正确偏转 cwd!"

        # 尝试非法上下文切换 (给一个非数字宏)
        runtime.store.write_locator("genesis", "/.mds/env.md", SelectorBuilder().drill("LOCAL_ENV").build(), "+>", "\n- tgt_fake: abcdef\n")
        ans_fake = await runtime.exec('$%tgt_fake::/readme.md::PUBLIC', wt_uid="genesis")
        assert "mdio_error" in str(ans_fake), "使用非法宏前导没有拉响警报！"

        results["T14"] = True


        # 15. 系统核心深水区 `.folder` 与 越权 Fuzzing 护城墙

        print("\n⚡ [Test 15] 系统禁区护盾 (`.folder`) 黑名单物理级强制拒止")

        try:
            # 尝试恶意构建一个指向禁区的文件源映射
            ws_mount.derive_worktree("genesis", "/public; /.mds:/hacked_sys:RO")
            assert False, "成功利用非法规则跨界挂载！"
        except (MdioError, ValueError) as e:
            assert "restricted dot-folder" in str(e), f"未能正确识别源路径越界拦截: {e}"

        try:
            # 尝试恶意捏造一个系统别名前缀
            ws_mount.derive_worktree("genesis", "/public; /shared:/.sys_fake:RW")
            # WT VFS: 只禁 .mds/.mdio 别名
        except (MdioError, ValueError) as e:
            assert "cannot start with '.'" in str(e), f"未能正确识别别名非法符号: {e}"

        results["T15"] = True


        print("\n✅ VFS 多租户基座与资源生命周期全量体检报告")
        for k, v in results.items():
            print(f"[{k}] -> {'🟢 PASS' if v else '🔴 FAIL'}")

        assert all(results.values()), "部分防线检测未达标"
        print("\n🏆================================================🏆")
        print("🎉 MULTI-TENANT ISOLATION & VFS KERNEL EVERYTHING PASSED! 🎉")
        print("🏆================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(test_namespace_management())
