"""
XBT Plugin Module
"""

from .xbt import XBTPlugin

# 对外只暴露 Plugin 类
__all__ = ["XBTPlugin"]