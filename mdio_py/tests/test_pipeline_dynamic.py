import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def run_pipeline_dynamic_test():
    print("🚀 Starting MDIO `$()` & `_` Pipeline Edge Case Test (VFS NEO)...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        # 1. 构造测试数据
        router_file = ws / "router.md"
        router_file.write_text(
            "# ROUTING\n"
            "- ptr_doc: /db.md\n"
            "- ptr_full: /db.md::SYS_DATA._b:users\n"
            "- write_tgt: /report_dyn.md::DYNAMIC_WRITE\n",
            encoding="utf-8"
        )

        db_file = ws / "db.md"
        db_file.write_text(
            "# SYS_DATA\n"
            "- users: Alice, Bob, Charlie\n"
            "- admins: Root, System\n"
            "- dirty_ptr: /db.md::SYS_DATA._b:users,IGNORE_THIS_TAIL\n"
            "- json_dict: {\"file\": \"/db\", \"sys\": \"SYS_DATA\", \"key\": \"admins\"}\n"
            "- json_arr: [\"Apple\", \"Banana\", \"Cherry\"]\n",
            encoding="utf-8"
        )

        # VFS 初始化
        storage_root = str(ws)
        print(f"📦 Workspace mounted at: {storage_root}")
        runtime = MdioRuntime(storage_root)
        await runtime.exec('mk -f /report.md', wt_uid="genesis")
        await runtime.exec('mk -f /report_dyn.md', wt_uid="genesis")
        await runtime.exec('mk -f /default.md', wt_uid="genesis")

        pipeline = r'''
        # T1: 基础管道隐式传递
        $/db.md::SYS_DATA._b:users | fmt -t "T1_IMPLICIT:\n{}" | +> /report.md:: ;

        # T2: `_` 显式占位符传递
        $/db.md::SYS_DATA._b:admins | fmt -t "T2_EXPLICIT:\n{}" -v _ | +> /report.md:: ;

        # T3: 单层 $() 指针解引用
        $($/router.md::ROUTING._b:ptr_full) | fmt -t "T3_POINTER:\n{}" | +> /report.md:: ;

        # T4: $() 内管道混合计算
        $($/router.md::ROUTING._b:ptr_doc | fmt("{}::SYS_DATA._b:admins")) | fmt -t "T4_NESTED:\n{}" | +> /report.md:: ;

        # T5: $() 作为 Kwarg 参数预执行
        "Result" | fmt -t "T5_ARG_EXEC:\n|{}|" -v $($/router.md::ROUTING._b:ptr_doc | fmt("Connected to {}")) | +> /report.md:: ;

        # T6: 动态写入目标验证
        "Dynamic Data T6" | +> ($/router.md::ROUTING._b:write_tgt) ;

        # T7: 管道清洗 (split + get)
        $($/db.md::SYS_DATA._b:dirty_ptr | split(",") | get(0)) | fmt -t "T7_CLEANED_READ:\n{}" | +> /report.md:: ;

        # T8: JSON 解析器与字典解构
        $($/db.md::SYS_DATA._b:json_dict | loads | fmt("{file}.md::{sys}._b:{key}")) | fmt -t "T8_JSON_ROUTING:\n{}" | +> /report.md:: ;

        # T9: JSON 数组提取
        $/db.md::SYS_DATA._b:json_arr | loads | get(1) | fmt -t "T9_ARR_GET:\n{}" | +> /report.md:: ;

        # T10: 管道字面量流转
        $({"sys": "db.md", "node": "SYS_DATA", "k": "users"} | loads | fmt("/{sys}::{node}._b:{k}")) | fmt -t "T10_DICT_LITERAL:\n{}" | +> /report.md:: ;

        # T11: 毒气隔离
        "POISON_DATA" | len ;
        "PURE" | fmt -t "T11_ISOLATION:\n{}" | +> /report.md:: ;

        # T12: 动态精准覆盖写
        "Old Value" | +> ($/router.md::ROUTING._b:write_tgt | fmt("{}._b:t12_key")) ;
        "Golden Value" | /> ($/router.md::ROUTING._b:write_tgt | fmt("{}._b:t12_key")) ;

        # T13: 三引号特工原生穿透
        """Line A
        Line B""" | fmt -t "T13_MULTILINE:\n{}" | +> /report.md:: ;
        '''

        print("⚡ Executing Expanded Dynamic Pipeline...")
        res = await runtime.exec(pipeline, active_doc="/default.md", wt_uid="genesis")

        if isinstance(res, str) and "- mdio_error:" in res:
            print(f"🚨 PIPELINE ENGINE ERROR:\n{res}\n")

        sys_text = runtime.store.get_text("genesis", "/report.md")
        dyn_text = runtime.store.get_text("genesis", "/report_dyn.md")
        final_text = sys_text + "\n" + dyn_text

        print("\n✅ Final Combined Report:\n" + "-"*50)
        print(final_text)
        print("-" * 50)

        # 断言
        assert "T1_IMPLICIT:" in final_text and "Alice, Bob, Charlie" in final_text, "Failed T1"
        assert "T2_EXPLICIT:" in final_text and "Root, System" in final_text, "Failed T2"
        assert "T3_POINTER:" in final_text and "Alice, Bob, Charlie" in final_text, "Failed T3"
        assert "T4_NESTED:" in final_text and "Root, System" in final_text, "Failed T4"
        assert "|Connected to /db.md|" in final_text, "Failed T5"

        assert "# DYNAMIC_WRITE" in final_text and "- Dynamic Data T6" in final_text, "Failed T6"

        assert "T7_CLEANED_READ:" in final_text and "Alice, Bob, Charlie" in final_text, "Failed T7"
        assert "T8_JSON_ROUTING:" in final_text and "Root, System" in final_text, "Failed T8"
        assert "T9_ARR_GET:" in final_text and "Banana" in final_text, "Failed T9"
        assert "T10_DICT_LITERAL:" in final_text and "Alice, Bob, Charlie" in final_text, "Failed T10"

        assert "T11_ISOLATION:" in final_text and "POISON" not in final_text and "PURE" in final_text, "Failed T11"
        assert "Old Value" not in final_text and "t12_key: Golden Value" in final_text, "Failed T12"
        assert "T13_MULTILINE:" in final_text and "Line A" in final_text and "Line B" in final_text, "Failed T13"

        print("\n🏆========================================================🏆")
        print("🎉 ALL 13 ULTIMATE DYNAMIC EXECUTION EDGE CASES PASSED! 🎉")
        print("🏆========================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(run_pipeline_dynamic_test())
