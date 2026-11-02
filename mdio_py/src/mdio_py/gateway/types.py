"""Gateway 类型 — RemoteDecl + Active + RemotePhase。

RemoteDecl 是不可变声明，Active 是生命周期容器，RemotePhase 是 Gateway 统一阶段。
"""

from __future__ import annotations

import base64
import json
from dataclasses import dataclass, field
from enum import Enum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mdio_py.gateway.crdt_scope import CrdtScope
    from mdio_py.gateway.iroh_transport import ConnectionHandler


# RemotePhase — Gateway 统一阶段（协议无关）

class RemotePhase(Enum):
    """Gateway 层对 remote 关系阶段的统一表述，协议无关。

    ACTIVE    — 可以广播 CRDT/FileOp（连接就绪 + doc_map 完备）
    PENDING   — 不可以（连接中/重试中/等对端/等 doc_map/短暂断连）

    iroh connected+doc_map 就绪 → ACTIVE，其余 → PENDING
    WS open → ACTIVE，closed → PENDING
    """

    ACTIVE = "active"
    PENDING = "pending"


# RemoteDecl — 不可变声明

@dataclass(frozen=True)
class RemoteDecl:
    """对一条 remote 条目的不可变声明。

    两个构造路径：
    - from_ticket(): mount 操作时从 iroh ticket 解析
    - from_files():  reconcile 时从 rule + gateway.md join 得到
    """

    wt_uid: str
    path: str               # "/share/team-a"
    entry_id: str           # "team-a"
    token: str
    addr: str | None        # mount 侧有（远端地址），share 侧无
    local_prefix: str
    remote_prefix: str | None

    @staticmethod
    def from_ticket(ticket_str: str, wt_uid: str, name: str) -> "RemoteDecl":
        """从 iroh ticket 解析声明。

        ticket 格式: iroh-share://<base64> 或 iroh://<base64>
        base64 解码后: {"addr": "...", "token": "...", "path": "/..."}
        """
        b64_part = ticket_str.replace("iroh-share://", "").replace("iroh://", "")
        padding = 4 - len(b64_part) % 4
        if padding != 4:
            b64_part += "=" * padding
        data = json.loads(base64.urlsafe_b64decode(b64_part))

        addr_str = data["addr"]
        token = data.get("token", "")
        remote_prefix = data.get("path", "/")
        path = f"/share/{name}"
        local_prefix = path

        # 尝试将 addr_str 解析为 iroh EndpointAddr（新 ticket 格式）
        try:
            import iroh as _iroh
            ticket = _iroh.EndpointTicket.from_string(addr_str)
            addr = ticket.endpoint_addr()
        except Exception:
            addr = addr_str

        return RemoteDecl(
            wt_uid=wt_uid,
            path=path,
            entry_id=name,
            token=token,
            addr=addr,
            local_prefix=local_prefix,
            remote_prefix=remote_prefix,
        )

    @staticmethod
    def from_files(
        wt_uid: str,
        rule_entries: list[tuple[str, str]],
        gateway_entries: list[dict],
    ) -> list["RemoteDecl"]:
        """从 rule + gateway.md join 得到声明列表。

        rule_entries:  [(path, entry_id)]  ← Rule::remote_entries(wt_uid) 输出
        gateway_entries: [{_title, shared_token, remote_token, remote_addr,
                           local_prefix, remote_prefix, ...}]
        按 entry_id 做 inner join：rule-only 或 gateway-only 的条目都会过滤掉。
        """
        result: list[RemoteDecl] = []

        # 建立 gateway entry 按 _title 索引
        gw_by_title: dict[str, dict] = {}
        for e in gateway_entries:
            title = e.get("_title", "")
            if title:
                gw_by_title[title] = e

        for path, entry_id in rule_entries:
            gw = gw_by_title.get(entry_id)
            if gw is None:
                continue  # rule 声明了但 gateway.md 未找到 -> 过滤

            token = gw.get("shared_token") or gw.get("remote_token") or ""
            addr = gw.get("remote_addr") or gw.get("addr") or None
            local_prefix = gw.get("local_prefix", path)
            remote_prefix = gw.get("remote_prefix") or None

            result.append(RemoteDecl(
                wt_uid=wt_uid,
                path=path,
                entry_id=entry_id,
                token=token,
                addr=addr,
                local_prefix=local_prefix,
                remote_prefix=remote_prefix,
            ))

        return result


# Active — 生命周期容器 + 状态投影

@dataclass
class Active:
    """一条 gateway 条目的运行时容器。

    - key: "wt_uid:entry_id"
    - decl: 不可变声明
    - scope: 共享 CrdtScope 引用
    - handler: ConnectionHandler（mount/reconcile 创建，share 由对端注入）
    - phase: Gateway 统一阶段（ACTIVE=可广播，PENDING=不可）
    """

    key: str
    decl: RemoteDecl
    scope: "CrdtScope"
    handler: "ConnectionHandler | None" = None
    phase: RemotePhase = RemotePhase.PENDING
