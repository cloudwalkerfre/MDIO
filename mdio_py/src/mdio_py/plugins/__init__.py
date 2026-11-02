"""
MDIO 插件注册表
"""

from typing import List
from .plugin_base import PluginBase

# 导入所有已实现的插件
from .bash import BASHPlugin
from .llm import LLMPlugin
# from .py import PYPlugin
from .xbt import XBTPlugin
from .cron import CRONPlugin


def get_all_plugins() -> List[PluginBase]:
    return [
        BASHPlugin(),
        LLMPlugin(),
        # PYPlugin(),
        XBTPlugin(),
        CRONPlugin(),
    ]

def register_plugins(workspace) -> None:
    """
    将所有插件一次性注册到 Workspace 容器中。
    """
    for plugin in get_all_plugins():
        workspace.register_plugin(plugin)

__all__ = ["register_plugins", "PluginBase"]
