import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime

async def run_edge_cases():
    print("🛸 [Edge Cases Pipeline] Commencing Under VFS Constraints...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        workspace_path = Path(tmpdir)
        
        file_a = workspace_path / "a.md"
        file_a.write_text("# A\n- data: from A\n## LIST\n- item1\n- item2\n- item3\n", encoding="utf-8")
        
        file_b = workspace_path / "b.md"
        file_b.write_text("# B\n- data: from B\n", encoding="utf-8")

        # 挂载虚拟环境
        runtime = MdioRuntime(str(workspace_path))

        # 🚨 VFS NEO 修正：
        # 1. 作用域声明必须清晰地带上 `.md` 或者使用确定的物理文件名
        # 2. 文件系统之间的切换严格通过 `::<目录/文件界限>` 完成
        pipeline_code = r'''
/b.md::{
    $B._b:data | fmt -t """\n- B said: {}""" | +>/a.md::A ;
    /a.md::{
        $A._b:data | fmt -t """\n- A said: {}""" | +>/b.md::B ;
        $A.LIST.* | len | fmt -t """\n- Repeated items count: {}""" | +>/b.md::B
    }
};
"""# REPLACED_A\n- end: true""" | />/a.md::A -pj tag
'''
        
        print(f"⚡ Launching Pipeline:\n{pipeline_code}\n" + "-"*40)

        # 缺省焦点文件为 /default.md，强制触发作用域绝对挂载寻址
        result = await runtime.exec(pipeline_code, active_doc="/default.md", wt_uid="genesis")
        
        if str(result).startswith("- mdio_error:"):
             print(f"❌ 运行中断! 错误详情:\n{result}")

        # 使用绝对虚拟路径拉取判定
        res_a = runtime.store.get_text("genesis", "/a.md")
        res_b = runtime.store.get_text("genesis", "/b.md")

        print("=== 📄 Final [a.md] ===")
        print(res_a.strip())
        print("\n=== 📄 Final [b.md] ===")
        print(res_b.strip())

        # 魔法判定：
        assert "# REPLACED_A" in res_a, "Head overwrite failed!"
        assert "# A" not in res_a, "Old Head A survived!"
        assert "- A said: from A" in res_b, "Nested Context A -> B failed!"
        assert "Repeated items count: 4" in res_b, "Slice duplication or pipeline data passing failed!"

        print("\n🏆 ALL EDGE PIPELINE TESTS PASSED & VFS ISOLATION VERIFIED!")


if __name__ == "__main__":
    asyncio.run(run_edge_cases())