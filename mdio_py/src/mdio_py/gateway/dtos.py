import msgspec
from typing import Optional, Any, List

class CommandRequest(msgspec.Struct):
    code: str
    active_doc: str = "/"   # 缺省焦点为虚拟根
    wt_uid: str = "genesis"       # 缺省使用根 worktree(genesis)

class CommandResponse(msgspec.Struct):
    status: str  # "ok" | "error"
    result: Optional[str] = None
    diffs: Optional[List[Any]] = None

class GatewayShareRequest(msgspec.Struct):
    wt_uid: str
    path: str = "/"
    name: str = ""
