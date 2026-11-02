import sys
import os
import textwrap
import asyncio

# 确保可以导入 mdio_py 模块
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

async def test_calc_engine():
    print("\n🧪 [TEST] 初始化 MDIO 运行时 & Calc 计算引擎...")
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec('mk -f /game.md', ctx=ctx)


    # 0. 构建虚拟宇宙数据 (用于测试变量引用和群攻)

    print("📁 [TEST] 注入 VFS 测试数据...")
    await runtime.exec('mk -f "/game.md"', ctx=ctx)
    mock_data = textwrap.dedent("""
    # PLAYER
    - hp: 100
    - atk: 25
    - def: 10

    # ENEMIES
    - goblin: 50
    - orc: 120
    - slime: 30
    """).strip()
    await runtime.exec(f'"""{mock_data}""" | +> /game.md:: -f', ctx=ctx)

    async def expect_eq(cmd, expected, desc=""):
        res = await runtime.exec(cmd, ctx=ctx)
        res_str = str(res).strip()
        assert res_str == str(expected), f"❌ {desc} 失败! \n  预期: {expected}\n  实际: {res_str}\n  指令: {cmd}"
        print(f"   ✅ {desc:<20} -> {res_str}")

    async def expect_err(cmd, err_keyword, desc=""):
        res = await runtime.exec(cmd, ctx=ctx)
        res_str = str(res).strip()
        assert "- mdio_error:" in res_str and err_keyword in res_str, \
            f"❌ {desc} 拦截失败! \n  预期错误含: {err_keyword}\n  实际返回: {res_str}"
        print(f"   🛡️ {desc:<20} -> 成功拦截 ({err_keyword})")


    # 1. 基础算术运算与隐式变量

    print("\n🧮 [TEST] 阶段 1: 基础算术与管线占位符 (_)")
    await expect_eq('"10" | calc "_ + 5"', "15", "加法运算")
    await expect_eq('"10" | calc "_ * 2 - 5"', "15", "混合运算优先级")
    await expect_eq('"10" | calc "round(_ / 3, 2)"', "3.33", "浮点除法与保留位")
    await expect_eq('"10" | calc "_ // 3"', "3", "整除运算")
    await expect_eq('"10" | calc "_ % 3"', "1", "取模运算")


    # 2. 变量绑定与 VFS 探针联动

    print("\n🔗 [TEST] 阶段 2: 变量显式绑定与动态求值 ($)")
    await expect_eq('"50" | calc "_ - damage" -damage 20', "30", "外部字面量传入")
    await expect_eq('"" | calc "hp - dmg" -hp 100 -dmg 30', "70", "全权由 kwargs 接管")
    
    # 高级用法：直接从 VFS 的 Markdown 节点动态取值参与运算！
    cmd_vfs = '"" | calc "atk * 2 - target_def" -atk ($/game.md::PLAYER._b:atk) -target_def ($/game.md::PLAYER._b:def)'
    await expect_eq(cmd_vfs, "40", "VFS 跨节点读取与公式求值")


    # 3. 逻辑判定与三元操作 (XBT 的核心引擎)

    print("\n🚦 [TEST] 阶段 3: 布尔逻辑与三元跳变 (If-Else)")
    await expect_eq('"50" | calc "_ > 20"', "true", "大于判定")
    await expect_eq('"50" | calc "_ == 50 and _ < 100"', "true", "And 复合逻辑")
    await expect_eq('"15" | calc "_ < 10 or _ > 20"', "false", "Or 复合逻辑")
    await expect_eq('"true" | calc "not _"', "false", "布尔取反")
    
    # 三元表达式测试：如果血量(100)低于50，扣血量*2，否则正常扣
    cmd_if = '"20" | calc "(_ * 2) if hp < 50 else _" -hp $($/game.md::PLAYER._b:hp)'
    await expect_eq(cmd_if, "20", "三元表达式 (正常线)")

    cmd_if_low = '"20" | calc "(_ * 2) if hp < 50 else _" -hp 30'
    await expect_eq(cmd_if_low, "40", "三元表达式 (濒死双倍)")


    # 4. 业务白名单函数 (边界钳制与随机数)

    print("\n⚔️ [TEST] 阶段 4: 游戏数值特化函数")
    await expect_eq('"-10" | calc "abs(_)"', "10", "绝对值计算")
    await expect_eq('"10" | calc "max(0, _ - 50)"', "0", "锁死下限 (不超负数)")
    await expect_eq('"200" | calc "clamp(_, 0, 100)"', "100", "Clamp 钳制")
    
    # 测试 roll 真随机：测试 10 次，确保总是落在 1-5 之间
    rolls = []
    for _ in range(10):
        res = await runtime.exec('"" | calc "rand(1, 5)"', ctx=ctx)
        rolls.append(int(str(res)))
    assert all(1 <= r <= 5 for r in rolls), "rand() 越界！"
    print(f"   ✅ 真随机函数 rand(1, 5) -> 通过 (抽样结果: {rolls})")


    # 5. 数组广播 (Array Broadcasting / AOE 伤害)

    print("\n🌪️ [TEST] 阶段 5: 数组广播聚变 (AOE)")
    #   "50,120,30" (造字面量) -> split (造数组[]) -> calc (群攻遍历每个元素) -> join (拼回字符串)
    cmd_aoe = '"50,120,30" | split "," | calc "max(0, _ - 40)" | join ","'
    
    # 50 -> 10, 120 -> 80, 30 -> 0
    await expect_eq(cmd_aoe, "10,80,0", "群攻运算下发")



    # 6. 绝对安全防御边界 (黑客/注入攻防拦截)

    print("\n🚧 [TEST] 阶段 6: 架构级安全攻防演练")
    await expect_err('"10" | calc "_ ** 9999"', "二元操作符", "拦截星号指数爆炸 (CPU熔断防御)")
    await expect_err('"10" | calc "_ / 0"', "除零", "拦截除以 0 导致应用崩溃")
    
    # AST 非法节点与未授权函数拦截
    await expect_err('"" | calc "__import__(\'os\').system(\'ls\')"', "非法函数", "拦截非法危险节点 (import)")
    await expect_err('"" | calc "open(\'/etc/passwd\')"', "非法函数", "拦截非法内置函数调用")
    await expect_err('"abc" | calc "_ + 1"', "杂质", "拦截非数字类型的无脑掺入")
    
    await expect_err('"" | calc "a + 1"', "未定义", "拦截未知悬空变量")
    await expect_err('"" | calc "max(a, b)" -max 10 -a 1 -b 2', "保留字", "拦截重写/覆盖系统内置函数")

    print("\n🎉 [TEST] CALC 微内核安全求值器 全场景验证通过！")

if __name__ == "__main__":
    asyncio.run(test_calc_engine())
