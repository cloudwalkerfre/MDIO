# mdio_py/src/mdio_py/plugins/xbt/driver_types.py
"""driver 地址与调用配方：类型.名字 → 插件协议 + invoke/open 旗标模板。

XBT 树逻辑只做三件事：解析地址、查类型配方、按配方拼命令。
介质专有旗标（如 LLM 的 -wl / -branch）只出现在对应类型的配方里，
新增一种 driver 类型 = 在 DRIVER_TYPES 加一行数据。
"""

from typing import Any, Optional

DEFAULT_TYPE = "LLM"


def parse_address(addr: Any) -> tuple:
    """driver 地址 → (类型, 名字, 插件协议名)。

    规则：按第一个点切分；没有点的裸名视为默认类型 LLM。
    类型大小写原样作为插件协议名（插件名是 @LLM / @XBT 这类大写形式）。
    """
    text = str(addr or "").strip()
    if "." in text:
        head, _, tail = text.partition(".")
        proto = head.strip()
        return proto, tail.strip(), proto
    return DEFAULT_TYPE, text, DEFAULT_TYPE


def driver_name(addr: Any) -> str:
    return parse_address(addr)[1]


def _llm_invoke(addr: Any, token: Optional[str]) -> str:
    suffix = ' -wl "%s"' % token if token else ""
    return "@LLM." + driver_name(addr) + suffix


def _llm_open(addr: Any, child: str, fork_meta: dict) -> str:
    return '@LLM.%s -wl "%s" -branch' % (driver_name(addr), child)


DRIVER_TYPES: dict = {
    "LLM": {
        "plugin": "LLM",
        "worldline": True,
        "invoke": _llm_invoke,
        "open": _llm_open,
    },
    # 未来 XBT 类型驱动加一行即可（token 当命名空间，invoke/open 用 XBT 自己的旗标）：
    # "XBT": {"plugin": "XBT", "worldline": True, "invoke": ..., "open": ...},
}


def recipe_for(addr: Any) -> tuple:
    """返回 (类型, 配方)。

    未登记类型：无 worldline、invoke 只带地址、open=None（孩子留在同层）。
    """
    proto, _name, plugin = parse_address(addr)
    recipe = DRIVER_TYPES.get(proto)
    if recipe is not None:
        return proto, recipe
    return proto, {
        "plugin": plugin,
        "worldline": False,
        "invoke": lambda a, token: "@" + str(a),
        "open": None,
    }


def format_invoke(addr: Any, token: Optional[str] = None) -> str:
    """按类型配方拼 invoke 命令（XBT 核心唯一入口）。"""
    _, recipe = recipe_for(addr)
    return recipe["invoke"](addr, token)


def format_open(addr: Any, child_token: str, fork_meta: Optional[dict] = None) -> Optional[str]:
    """按类型配方拼 open 子世界线命令；该类型无 worldline 时返回 None。"""
    _, recipe = recipe_for(addr)
    opener = recipe["open"]
    if opener is None:
        return None
    return opener(addr, child_token, fork_meta or {})
