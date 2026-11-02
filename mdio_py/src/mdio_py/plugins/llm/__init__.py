"""
LLM Plugin Module
"""

from .llm import LLMPlugin

# 对外只暴露 Plugin 类
__all__ = ["LLMPlugin"]
