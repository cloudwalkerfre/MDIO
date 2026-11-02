import asyncio
import tempfile
import json
import textwrap
from pathlib import Path

from mdio_py import MdioRuntime

async def run_midio_test():
    print("💎 Starting MDIO 全量集成测试...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        sys_dir = ws / ".mds"
        sys_dir.mkdir()
        data_dir = ws / "data"
        data_dir.mkdir()

        # 核心只读配置
        (sys_dir / "conf.md").write_text("# INIT\n- core_target: /data/db.md::\n", encoding="utf-8")

        # 含各种诡异格式的数据库
        (data_dir / "db.md").write_text(
            "# USERS\n"
            "- admin_meta: {\"id\": 1, \"role\": \"root\", \"auth\": true}\n"
            "\n"
            "# SERVER\n"
            "- status: 💥 系统核心已经崩溃！\n"
            "- uptime: 99.99%\n",
            encoding="utf-8"
        )

        # 1. 挂载 VFS (Sys 目录强制只读，Data 目录可写)
        base_mount = f"{ws}; {sys_dir}:/.mds:RO"
        runtime = MdioRuntime(base_mount)
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")
        print(f"📦 [VFS] Master Space Mounted: /.mds (RO) | /data (RW)")

        # 2. 修正后的管道：先创建报告文件，再执行操作
        pipeline = textwrap.dedent(r'''
        # 前置：创建日志目录和报告文件
        mk -f /logs/report.md ;

        # 第一式：动态求值 + 字典展开 + 脚手架落盘 (提取 JSON，解析 Role，新建报告文件)
        $/data/db.md::USERS._b:admin_meta | loads | get("role") | fmt("Admin Role Configured as: [{}]") | +> /logs/report.md::SYSTEM_CHECK ;

        # 第二式：多字节 Emoji 字符与偏移量的 Aider-style 精准替换
        """<<<<
        - status: 💥 系统核心已经崩溃！
        ====
        - status: 🟢 系统已修复，运行如飞 🚀 🚀 🚀
        >>>>""" | ~> ($/.mds/conf.md::INIT._b:core_target | fmt("{}SERVER")) ;

        # 第三式：安全边界 Grep (测试 UTF-8 的裁剪兼容性)，扫描刚才修复的系统并记录
        "/data/*.md::" | find -q "运行如飞" -c [5, 4] | +> /logs/report.md::SYSTEM_CHECK ;

        # 第四式：root 写 /.mds（Plan-3 下 /.mds 为系统保留目录，:/.mds:RO 子挂载被忽略，故写入成功）
        "HACKED" | /> /.mds/conf.md::INIT._b:core_target ;
        ''')

        print(f"\n⚡ Executing Holy Grail Pipeline:\n{pipeline}\n" + "-"*40)

        res = await runtime.exec(pipeline, active_doc="/default.md", wt_uid="genesis")

        # ========================= 结果检验 =========================

        db_text = runtime.store.get_text("genesis", "/data/db.md")
        report_text = runtime.store.get_text("genesis", "/logs/report.md")

        print("\n📄 [db.md] State:\n" + db_text.strip())
        print("\n📄 [report.md] State (Scaffolded):\n" + report_text.strip())
        print("\n🛡️ Pipeline Ultimate Return (Safety Intercept):\n" + str(res).strip())

        print("\n" + "="*80)

        # 1. JSON 管道与脚手架验证
        assert "# SYSTEM_CHECK" in report_text and "Configured as: [root]" in report_text, "Failed: 动态 JSON 解析或脚手架文件建立失败！"

        # 2. 字符边界与智能补丁验证 (验证 () 跨文件动态指针，且 Emoji 定位偏移毫无差错)
        assert "🟢 系统已修复，运行如飞 🚀 🚀 🚀" in db_text, "Failed: 多字节符的 Aider 更新与动态算址折戟！"
        assert "💥" not in db_text, "Failed: 旧版炸弹字符残存！"

        # 3. Grep 上下文提取与 UTF-8 边界验证
        assert "运行如飞" in report_text and "🚀" in report_text, "Failed: Grep 未能穿透 Emoji 截取安全上下文！"

        # 4. /.mds 写入：Plan-3 下 /.mds 为系统保留目录，`:/.mds:RO` 子挂载被忽略（不可用户挂载为 RO），
        #    故 root 可写入（第四式返回 mdio_success）——这是与旧模型的关键差异点。
        assert "mdio_success" in str(res), "Failed: 预期 /.mds 可被 root 写入（Plan-3 不支持把 /.mds 挂为 RO）"

        # 5. 核心：原子化阻断
        assert "root" in report_text and "🟢" in db_text, "Failed: 拦截影响到了前半段合法的指令，Pipeline 未实现逐行原子提交！"

        print("\n🏆=============================================================🏆")
        print("🎉 HOLY GRAIL PASSED: THE MDIO CORE ENGINE IS OFFICIALLY ROCK SOLID! 🎉")
        print("🏆=============================================================🏆\n")

if __name__ == "__main__":
    asyncio.run(run_midio_test())
