import os
import shutil
import asyncio
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime

PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent


def _own_workspace() -> Path:
    """本测试**自己的 wt**：临时目录 + 拷一份 llm_config.md。

    不碰用户/开发树的工作区，避免污染真实运行数据。配置来源唯一：
    tests/fixtures/genesis（无 secret）。
    """
    tmp = Path(tempfile.mkdtemp(prefix="mdio_llm2_"))
    src = Path(__file__).resolve().parent / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md"
    # 文档根是 <root>/wt：配置必须落在 wt/.mds/ 才能被节读取到
    (tmp / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    shutil.copy(src, tmp / "wt" / ".mds" / "llm_config.md")
    return tmp


WORKSPACE_DIR = _own_workspace()

async def main():
    print("🧹 [System] 初始化虚拟内核与物理宇宙观...")
    print(f"   （本次用独立工作区：{WORKSPACE_DIR}）")

    # 只要这行一运行，底层的 /.mds/proc.md, /.mds/llm.md 等已经由 Rust 全自动就位并刻上好了 Head！
    runtime = MdioRuntime(str(WORKSPACE_DIR))
    sandbox_id = "genesis"
    
    # 我们只需要建立业务文件
    await runtime.exec('mk -f /debate_hall.md', wt_uid=sandbox_id)
    
    print("\n🎬 [SysCall] 注入灵魂：基于 MDIO 预配置初始 Actor！")
    print("=" * 80)
    
    sysp_director = """
    [CONTEXT]
    - @LLM 已预置 `-m "DEFAULT"`。

    [守则]
    1. 必须使用 tool(`execute_mdio`) 依次指挥李白和杜甫。调用他们时使用 `@LLM.actor_libai.run` 和 `@LLM.actor_dufu.run`。他们已经 init 完毕，直接用即可，无需再提供 `-m` 参数。
    2. 第1步：发给李白辩题：'''AI会抢走人类的饭碗，请正方发言''' | @LLM.actor_libai.run
    3. 第2步：等待李白回复后，利用 MDIO 提取他的回答交给杜甫反驳。例如：$ /debate_hall.md::ACTOR_1._b[.t=="assistant"][-1]._bq -pj content | fmt("李白说：{}。杜甫请反驳！") | @LLM.actor_dufu.run
       注意, actor历史对话需要至少跑一轮才有, 刚进来是空的, 第一轮：直接发 prompt 给对手, 后续轮：用 $ /debate_hall.md::ACTOR_1._b[.t=="assistant"][-1]._bq -pj content 取上轮（模板占位符只有 `{}`，`{_}` 不是占位符）再传给对手
    4. 第3步：等待杜甫说完，用同样的格式提取杜甫的回答交还给李白。
    5. 第4步：重复此反驳过程进行至少 3 轮。
    6. 第5步：辩论结束后，停止调用工具，直接由你输出总结陈词。

    [任务]
    - 严格通过 MDIO 工具指挥两人对线，最后进行法官总结。
    """.strip()
    
    init_cmd_libai = '@LLM.actor_libai.init -m "DEFAULT" -sysp \'\'\'你是李白，浪漫的程序员。绝不说废话，用两句诗意的短句反驳对方。\'\'\' -his "/debate_hall.md::ACTOR_1" -tool false'
    init_cmd_dufu = '@LLM.actor_dufu.init -m "DEFAULT" -sysp \'\'\'你是杜甫，悲观的程序员。绝不说废话，用两句沉郁的短句反驳对方。\'\'\' -his "/debate_hall.md::ACTOR_2" -tool false'
    init_cmd_director = f'@LLM.director.init -m "DEFAULT" -tool true -sysp """{sysp_director}""" -his "/debate_hall.md::DIRECTOR"'
    
    init_pipeline = f"{init_cmd_libai} ; {init_cmd_dufu} ; {init_cmd_director}"
    
    print("🧠 [Kernel] 正在编译并预埋初始基因树...")
    await runtime.exec(init_pipeline, active_doc="/", wt_uid=sandbox_id)
    
    print("\n🔥 [Action] 激战开始！向导演下达启动指令...")
    print("=" * 80)

    try:
        trigger_cmd = f'"请立即开始执行任务，指挥李白和杜甫互相驳斥！" | @LLM.director.stream'
        
        stream_res = await runtime.exec(trigger_cmd, wt_uid=sandbox_id, stream=True)
        if isinstance(stream_res, str):
            print(f"\n❌ [Premature Interrupt]: {stream_res}")
        else:
            async for chunk in stream_res:
                print(chunk, end="", flush=True)

    except Exception as e:
        print(f"\n❌ [Runtime Crash]: {e}")

    print("\n\n" + "=" * 80)
    print("✅ [Conclusion] 导演挂机！前往 VFS 的公共大屏 `debate_hall.md` 提取存盘录像带：\n")
    
    final_tape = runtime.store.get_text(sandbox_id, "/debate_hall.md")
    print(final_tape)
    
    assert "ACTOR_1" in final_tape, "❌ 李白根本没有收到信号发声！"
    assert "ACTOR_2" in final_tape, "❌ 杜甫没有接茬，跨体 IPC 通讯断裂！"
    assert "DIRECTOR" in final_tape, "❌ 导演没有进行最后的总结录入！"

    # 跨体传递得是真内容，不是原样模板：fmt 的模板位只有 {}（{_} 不是占位符）
    assert "{_}" not in final_tape, "❌ fmt 的 {_} 没被替换就进了对话（模板位只有 {}）"
    assert "李白说：" in final_tape and "杜甫说：" in final_tape, "❌ fmt 没把上轮发言拼进跨体 prompt"

    print("🎉 大获全胜！一切基于真正的 OS 自动拉起机制运行！")

if __name__ == "__main__":
    asyncio.run(main())
