"""
MDIO CLI REPL。

- 默认：连接运行中 server 的 /ws/cli（CLI 是客户端）
- --standalone：本地临时 runtime（测试/开发）
"""
import asyncio
import json
import os
import sys
from pathlib import Path

from prompt_toolkit import PromptSession
from prompt_toolkit.history import FileHistory

from mdio_py.ephemeral import MdioEphemeral

_BANNER_SERVER = """
╔══════════════════════════════════╗
║      MDIO NEO v0.8.0 — Server   ║
║   输入 exit / quit / Ctrl-D 退出  ║
╚══════════════════════════════════╝"""

_BANNER_STANDALONE = """
╔══════════════════════════════════╗
║  MDIO NEO v0.8.0 — Standalone   ║
║   输入 exit / quit / Ctrl-D 退出  ║
╚══════════════════════════════════╝"""

_PROMPT = [("class:prompt", "mdio> ")]
_EXIT_COMMANDS = {"exit", "quit"}
_MARKER = "__MDIO_BEGIN__"
_MARKER_END = "__MDIO_END__"


class ServerReplSession:
    """/ws/cli 的极薄 JSON-RPC 客户端。"""

    def __init__(self, ws):
        self.ws = ws
        self._next_id = 1

    @classmethod
    async def connect(cls, host: str | None = None, port: int | str | None = None):
        import websockets
        host = host or os.environ.get("MDIO_GATEWAY_HOST", "127.0.0.1")
        port = port or os.environ.get("MDIO_GATEWAY_PORT", "8011")
        ws = await websockets.connect(f"ws://{host}:{port}/ws/cli")
        session = cls(ws)
        await session._hello()
        return session

    async def _hello(self):
        req_id = self._next_id
        self._next_id += 1
        await self.ws.send(json.dumps({"id": req_id, "method": "hello"}))
        await self._recv_until(req_id)

    async def _recv_until(self, req_id: int) -> dict:
        while True:
            raw = await self.ws.recv()
            msg = json.loads(raw)
            if msg.get("kind") == "evt":
                continue
            if msg.get("id") == req_id:
                return msg

    async def exec(self, code: str, *, wt_uid: str = "genesis") -> str:
        req_id = self._next_id
        self._next_id += 1
        await self.ws.send(json.dumps({
            "id": req_id,
            "method": "exec.run",
            "params": {"code": code, "wt_uid": wt_uid},
        }))
        resp = await self._recv_until(req_id)
        if not resp.get("ok", False):
            err = resp.get("error") or {}
            return err.get("message", str(err))
        result = resp.get("result") or {}
        return str(result.get("raw", ""))

    async def close(self):
        await self.ws.close()


async def _repl_tty(exec_fn, banner: str) -> None:
    history_file = Path.home() / ".mdio_history"
    session = PromptSession(history=FileHistory(str(history_file)))

    print(banner)
    print(f"  历史记录: {history_file}")
    print()

    while True:
        try:
            line = await session.prompt_async(_PROMPT)
        except EOFError:
            print("\n退出。")
            break
        except KeyboardInterrupt:
            print()
            continue

        line = line.strip()
        if not line:
            continue
        if line in _EXIT_COMMANDS:
            break

        result = await exec_fn(line)
        print(result)


async def _repl_pipe(exec_fn) -> None:
    while True:
        try:
            line = sys.stdin.readline()
        except KeyboardInterrupt:
            break
        if not line:
            break

        line = line.strip()
        if not line:
            continue

        result = await exec_fn(line)
        sys.stdout.write(f"{_MARKER}\n{result}\n{_MARKER_END}\n")
        sys.stdout.flush()


async def _standalone_repl(force_repl: bool) -> None:
    async with MdioEphemeral() as mdio:
        if force_repl or sys.stdin.isatty():
            await _repl_tty(mdio.exec, _BANNER_STANDALONE)
        else:
            await _repl_pipe(mdio.exec)


async def _server_repl(force_repl: bool) -> None:
    session = await ServerReplSession.connect()
    try:
        if force_repl or sys.stdin.isatty():
            await _repl_tty(session.exec, _BANNER_SERVER)
        else:
            await _repl_pipe(session.exec)
    finally:
        await session.close()


def main() -> None:
    """UV CLI 入口"""
    args = sys.argv[1:]
    force_repl = "-cli" in args
    standalone = "--standalone" in args
    asyncio.run(_main_async(force_repl=force_repl, standalone=standalone))


async def _main_async(force_repl: bool = False, standalone: bool = False) -> None:
    if standalone:
        await _standalone_repl(force_repl)
    else:
        await _server_repl(force_repl)


if __name__ == "__main__":
    main()
