import traceback
import sys
import os
import json
from typing import Optional, Dict

# 错误协议：失败 = 结果文本以该前缀开头（容忍前导空白）。
ERROR_PREFIX = "- mdio_error:"


def is_error_text(text) -> bool:
    """失败判定唯一实现。"""
    return isinstance(text, str) and text.lstrip().startswith(ERROR_PREFIX)

class MdioError(Exception):
    """MDIO 核心异常统一载体"""
    
    def __init__(self, level: str, code: str, message: str, details: dict = None, cause: Exception = None):
        self.level = level
        self.code = code
        self.message = message
        self.details = details or {}
        self.cause = cause
        super().__init__(self.message)

        # 留给人类的后门：如果开启 Debug，打印真·异常堆栈
        mdio_debug = os.getenv("MDIO_DEBUG", "False").lower() in ("true", "1")
        # 如果是语法层面的错误 (P001, P002)，或者是 LLM 在瞎尝试，我们不打印恐怖的红屏堆栈！
        # 让它保持静默的赛博博弈
        if mdio_debug and cause and not str(self.code).startswith("P00"):
            print(f"\n\033[41m\033[37m[FATAL] {self.level.upper()} ERROR [{self.code}]\033[0m", file=sys.stderr)
            print(f"\033[31mMessage: {self.message}\033[0m", file=sys.stderr)
            traceback.print_exception(type(cause), cause, cause.__traceback__, file=sys.stderr)

    def to_markdown(self) -> str:
        """核心降维方法：将异常压扁为受控的 Markdown Strip"""
        base = f"- mdio_error: [{self.code}] {self.message}"
        if self.details:
            # 确保 detail 内无破坏性字符（如换行）
            safe_details = {}
            for k, v in self.details.items():
                try:
                    val_str = str(v).replace('\n', ' ')
                    # 裁剪超长报错，防止把 Token 撑爆
                    if len(val_str) > 500: 
                        val_str = val_str[:497] + "..."
                    safe_details[str(k)] = val_str
                except Exception:
                    safe_details[str(k)] = "<Unserializable Object>"

            base += f" {json.dumps(safe_details, ensure_ascii=False)}"
        return base

# --- 标准错误码字典 ---
class ErrCode:
    PARSE_TOKEN = "P001"      # 意外的 Token
    PARSE_CHAR = "P002"       # 无法识别的字符
    PARSE_UNKNOWN = "P500"    # Lark 内部崩溃
    
    PERM_DENIED = "I101"      # VFS Token 权限不足/worktree 越界
    INTERP_CRASH = "I500"     # 解释器崩溃
    
    RUST_IO = "R201"          # VFS worktree 不存在/读写失败
    RUST_PARSE = "R202"       # Rust 侧 Selector IR 解析失败
    
    PLG_TIMEOUT = "C301"      # 插件超时 (如 LLM API)
    PLG_CRASH = "C500"        # 插件代码崩溃

    VFS_ESCAPE    = "V001"    # 尝试越出 jail_root 边界
    VFS_DENIED    = "V002"    # Token 评级不足以访问隐秘空间
    VFS_NO_FOC    = "V003"    # 目录作用域下使用了缺省 `::`
    VFS_USURP     = "V004"    # 尝试横向越权或向上夺权

    # SNAP 时间子系统
    SNAP_NO_FILE     = "S001"  # 目标文件不存在
    SNAP_NO_PARENT   = "S002"  # parent snap 不存在
    SNAP_NO_SNAP     = "S003"  # snap ID 不存在
    SNAP_NO_ANCESTOR = "S004"  # diff 无公共祖先
    SNAP_NO_MATCH    = "S005"  # glob/目录展开后无匹配文件
    SNAP_PERM        = "S006"  # 权限不足
    SNAP_CKPT_ERR    = "S500"  # ckpt 损坏/缺失
