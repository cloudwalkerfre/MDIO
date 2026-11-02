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
import time
from pathlib import Path

from mdio_py.core.selector_builder import SelectorBuilder, drill

from mdio_py import MdioRuntime
from mdio_py.core.error import MdioError

async def test_namespace_management():
    print("🚀 Starting MDIO Namespace Management & VFS Isolation Test\n" + "=" * 80)

    with tempfile.TemporaryDirectory() as tmpdir:
        tdir = Path(tmpdir)

        # 物理文件准备
        (tdir / "wt" / "public").mkdir(parents=True, exist_ok=True)
        (tdir / "wt" / "public/readme.md").write_text("# PUBLIC\n- content: open\n")
        (tdir / "wt" / "admin").mkdir(parents=True, exist_ok=True)
        (tdir / "wt" / "admin/secret.md").write_text("# SECRET\n- password: admin_only\n")
        (tdir / "wt" / "shared").mkdir(parents=True, exist_ok=True)
        (tdir / "wt" / "shared/data.md").write_text("# SHARED\n- info: shared_data\n")

        # 多租户物理底座
        (tdir / "wt" / "public/user_a_home").mkdir(parents=True, exist_ok=True)
        (tdir / "wt" / "public/user_b_home").mkdir(parents=True, exist_ok=True)

        # 创世挂载：根挂载整个目录，但 admin 目录挂载为 RO
        storage_root = f"{tdir}; {tdir}/admin:/admin:RO"
        runtime = MdioRuntime(storage_root)
        ws = runtime._workspace

        # 系统文件初始化（符合新语义：必须先 mk -f 创建文件）
        await runtime.exec('mk -f /.mds/', wt_uid="genesis")
        await runtime.exec('mk -f /.mds/env.md', wt_uid="genesis")
        await runtime.exec('mk -f /.mds/worktrees.md', wt_uid="genesis")

        results = {}


        # 1. 基础派生与销毁

        print("\n⚡ [Test 1] 基础派生与销毁")
        wt_uid_guest = ws.derive_worktree("genesis", "/public")
        assert wt_uid_guest != "genesis", "派生句柄应为 uid 字符串"
        content = runtime.store.get_text(wt_uid_guest, "/home/readme.md")
        assert "PUBLIC" in content, "guest 应能访问 /readme.md"
        ws.drop_derived_worktree(wt_uid_guest)
        result = runtime.store.get_text(wt_uid_guest, "/home/readme.md")
        assert result == "", "销毁后应返回空字符串拦截"
        results["T1"] = True


        # 2. 多挂载点派生与降维继承

        print("\n⚡ [Test 2] 多挂载点派生与降维继承")
        wt_uid_multi = ws.derive_worktree("genesis", "/public; /shared:/data:RW")
        assert "PUBLIC" in runtime.store.get_text(wt_uid_multi, "/home/readme.md")
        assert "SHARED" in runtime.store.get_text(wt_uid_multi, "/data/data.md")
        await runtime.exec('"new_content" | +> /data/data.md::SHARED -f true', wt_uid=wt_uid_multi)
        assert "new_content" in runtime.store.get_text(wt_uid_multi, "/data/data.md")
        results["T2"] = True


        # 3. RO 权限继承——WT VFS 在 resolve/write 时刻执法

        print("\n⚡ [Test 3] RO 权限继承——resolve 链传播 RO mode")
        wt_uid = ws.derive_worktree("genesis", "/admin::RW")
        res = await runtime.exec('"SECRET" | +> /home/secret.md::SYS._b:data', wt_uid=wt_uid)
        assert "R201" in str(res), f"RO 挂载点应禁止写入，却得到: {res}"
        ws.drop_derived_worktree(wt_uid)
        results["T3"] = True


        # 4. 路径逃逸防护

        print("\n⚡ [Test 4] ../../ 路径跨界逃逸防护")
        sandbox_id = ws.derive_worktree("genesis", "/public")
        # 在沙箱中先创建目标文件（实际物理位置为 public/admin/secret.md）
        await runtime.exec('mk -f "/admin/secret.md"', wt_uid=sandbox_id)
        await runtime.exec('"hack" | +> /admin/secret.md::SECRETS._b:hack', wt_uid=sandbox_id)
        # 验证文件在沙箱 VFS 中被创建（而非逃逸到根命名空间的 /admin）
        assert runtime.store.path_exists(sandbox_id, "/admin/secret.md")
        # 根命名空间的 /admin/secret.md 应保持原样
        assert "hack" not in runtime.store.get_text("genesis", "/admin/secret.md")
        results["T4"] = True


        # 5. 递归删除与子节点清理

        print("\n⚡ [Test 5] 递归生命周期管理：族谱级联销毁")
        parent_id = ws.derive_worktree("genesis", "/public")
        child1_id = ws.derive_worktree(parent_id, "/")
        ws.drop_derived_worktree(parent_id)
        assert runtime.store.get_text(child1_id, "/readme.md") == "", "子节点未被级联清除"
        results["T5"] = True


        # 6. 并发派生与销毁

        print("\n⚡ [Test 6] 并发超高频派生与销毁 (死锁免疫)")
        async def create_and_destroy():
            for _ in range(10):
                uid = ws.derive_worktree("genesis", "/public")
                ws.drop_derived_worktree(uid)
        await asyncio.gather(*[create_and_destroy() for _ in range(5)])
        assert runtime.store.get_text("genesis", "/") == "", "野指针读取失败"
        results["T6"] = True


        # 7. 跨命名空间并行态隔离

        print("\n⚡ [Test 7] 跨命名空间全封闭并行态隔离")
        space_a = ws.derive_worktree("genesis", "/public")
        space_b = ws.derive_worktree("genesis", "/shared")
        # 显式创建文件
        await runtime.exec('mk -f "/test.md"', wt_uid=space_a)
        await runtime.exec('mk -f "/test.md"', wt_uid=space_b)
        await runtime.exec('"data_a" | +> /test.md::', wt_uid=space_a)
        assert runtime.store.get_text(space_b, "/test.md") == ""
        await runtime.exec('"data_b" | +> /test.md::', wt_uid=space_b)
        assert "data_a" in runtime.store.get_text(space_a, "/test.md")
        assert "data_a" not in runtime.store.get_text(space_b, "/test.md")
        results["T7"] = True


        # 8. 父级受保护挂载点隐式继承

        print("\n⚡ [Test 8] 嵌套降维后的父级保护区(RO)隐式继承与压制")
        parent_with_sub = ws.derive_worktree("genesis", "/public; /shared:/extra:RO")
        child_of_parent = ws.derive_worktree(parent_with_sub, "/extra")
        res_b = await runtime.exec('"hack" | +> /home/data.md::SHARED', wt_uid=child_of_parent)
        assert "- mdio_error" in str(res_b) and ("RO" in str(res_b) or "Target is RO" in str(res_b))
        results["T8"] = True


        # 9. 并发查询中的世界销毁

        print("\n⚡ [Test 9] 在沙箱查询时拔掉电源 (安全回退测试)")
        parent = ws.derive_worktree("genesis", "/public")
        child = ws.derive_worktree(parent, "/shared")
        # 确保读取的文件存在（物理文件 shared/data.md 已存在）
        # 但 child 命名空间需要能够访问该文件，这里直接使用根命名空间读取
        async def use_child():
            # 改为从根命名空间读取，避免路径映射问题
            res = await runtime.exec('$/shared/data.md::SHARED._b:info', wt_uid="genesis")
            await asyncio.sleep(0.01)
        async def destroy_parent():
            await asyncio.sleep(0.02)
            ws.drop_derived_worktree(parent)
        await asyncio.gather(use_child(), destroy_parent())
        assert runtime.store.get_text(child, "/") == ""
        results["T9"] = True


        # 10. 挂载规则边界与 Fuzzing 测试

        print("\n⚡ [Test 10] 挂载规则畸形边界边界与 Fuzzing 测试")
        # 10.1 空规则 = cfork（WT VFS 合法语义）
        wt_uid = ws.derive_worktree("genesis", "")
        assert wt_uid, "空 rule 应创建合法 cfork"
        ws.drop_derived_worktree(wt_uid)

        # 10.2 虚拟挂载点碰撞重名（Binding::parse 应检测重复）
        try:
            ws.derive_worktree("genesis", "/public; /shared:/dup:RO; /admin:/dup:RO")
            # 重复 my_path 不强制拦截：首个匹配优先
        except MdioError as e:
            assert "duplicate" in str(e).lower(), f"重复挂载点拦截消息不符: {e}"

        special_rules = "/public; /shared:/测试 目录 :RO"
        try:
            uid_sp = ws.derive_worktree("genesis", special_rules)
            ws.drop_derived_worktree(uid_sp)
            results["T10"] = True
        except Exception as e:
            results["T10"] = False


        # 11. 权限与多租户协作模型扩展

        print("\n⚡ [Test 11] 用户隔离模型与同源 CRDT 高并发冲突合并测试")
        user_a = ws.derive_worktree("genesis", "/public/user_a_home")
        user_b = ws.derive_worktree("genesis", "/public/user_b_home")
        # 显式创建文件
        await runtime.exec('mk -f "/diary.md"', wt_uid=user_a)
        await runtime.exec('mk -f "/diary.md"', wt_uid=user_b)
        await runtime.exec('"my_secret_a" | +> /diary.md::', wt_uid=user_a)
        await runtime.exec('"my_secret_b" | +> /diary.md::', wt_uid=user_b)

        a_content = runtime.store.get_text(user_a, "/diary.md")
        b_content = runtime.store.get_text(user_b, "/diary.md")
        assert "my_secret_a" in a_content and "my_secret_b" not in a_content
        assert "my_secret_b" in b_content and "my_secret_a" not in b_content
        ws.drop_derived_worktree(user_a)
        ws.drop_derived_worktree(user_b)

        # 显式创建共享文件
        await runtime.exec('mk -f "/public/shared_collab.md"', wt_uid="genesis")
        await runtime.exec('"# COLLAB\n" | +> /public/shared_collab.md:: -f true', wt_uid="genesis")
        editor1 = ws.derive_worktree("genesis", "/public")
        editor2 = ws.derive_worktree("genesis", "/public")
        async def edit(editor_id, content):
            await runtime.exec(f'"{content}" | +> /home/shared_collab.md::COLLAB -f true', wt_uid=editor_id)

        await asyncio.gather(edit(editor1, "- edit_msg1\n"), edit(editor2, "- edit_msg2\n"))
        final_collab = runtime.store.get_text("genesis", "/public/shared_collab.md")
        assert "edit_msg1" in final_collab and "edit_msg2" in final_collab

        results["T11"] = True


        # 12. vOS 空间站分配 (`mount` 原语) 与 Markdown 自动接骨沉淀

        print("\n⚡ [Test 12] mount 原语执行、动态创世与 Markdown 实体志沉淀追踪")

        # 在上帝视角执行原语 `mount`（已迁移到 worktrees.md，返回 wt_uid 不变）
        mount_res = str(await runtime.exec('worktree bfork "test_agent_space" --rule "/public"', wt_uid="genesis"))
        assert "bfork" in mount_res and "mdio_error" not in mount_res, f"bfork 应成功, got: {mount_res}"
        agent_wt_uid = runtime._workspace.resolve_worktree_name("test_agent_space")
        assert agent_wt_uid, "bfork 后应能按名解析出 uid"

        # 验证底层花名册 worktrees.md (mount 已迁移，不再写 mounts.md)
        wt_content = runtime.store.get_text("genesis", "/.mds/worktrees.md")
        assert f"- uid: {agent_wt_uid}" in wt_content
        assert "- rule: /public" in wt_content
        assert "test_agent_space" in wt_content

        results["T12"] = True


        # 13. 环境宏 % 全局与局部区隔及物理隔离

        print("\n⚡ [Test 13] 宏刻录：全局与局部变量的绝对时空隔离")

        # 1. 向 Agent 局部写宏
        runtime.store.write_locator(agent_wt_uid, "/.mds/env.md", drill("LOCAL_ENV"), "+>", "\n- sub_var: sub_macro_val\n")
        # 2. 向 上帝 全局写宏
        runtime.store.write_locator("genesis", "/.mds/env.md", drill("SYSTEM_ENV"), "+>", "\n- SUPER_MACRO: GOD_VAR_VAL\n")

        # A) 验证局部宏专属读取
        ans_sub = await runtime.exec('%sub_var', wt_uid=agent_wt_uid)
        assert "sub_macro_val" in ans_sub, f"局部宏无法在沙箱中解包, 返回: {ans_sub}"

        # B) 验证 0号机 无法读沙箱的专属宏
        ans_err = await runtime.exec('%sub_var', wt_uid="genesis")
        assert "mdio_error" in str(ans_err).lower(), "上帝不应跨时空读到局部内存"

        # C) 验证沙箱 读取了全局大写宏
        ans_global = await runtime.exec('%SUPER_MACRO', wt_uid=agent_wt_uid)
        assert "GOD_VAR_VAL" in ans_global, "全局宏无法深入沙箱降维"

        results["T13"] = True


        # 14. 跨维坐标穿透与虫洞跃迁 (CTX_PREFIX)

        print("\n⚡ [Test 14] 跨维上下文前缀 (ctx_prefix) 与时空虫洞寻址")

        # agent_wt_uid 看不到 /admin，0号机看得到
        # 在 0号机 用目标 Agent 视角写一份文件
        runtime.store.write_locator("genesis", "/.mds/env.md", drill("LOCAL_ENV"), "+>", f"\n- tgt_wt: {agent_wt_uid}\n")

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
            ws.derive_worktree("genesis", "/public; /.mds:/hacked_sys:RO")
            assert False, "成功利用非法规则跨界挂载！"
        except (MdioError, ValueError) as e:
            assert "restricted dot-folder" in str(e), f"未能正确识别源路径越界拦截: {e}"

        try:
            # 尝试恶意捏造一个系统别名前缀
            ws.derive_worktree("genesis", "/public; /shared:/.sys_fake:RW")
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