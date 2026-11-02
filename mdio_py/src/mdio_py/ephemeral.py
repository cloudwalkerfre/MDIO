"""
Ephemeral MDIO runtime — 不落盘的 MDIO 运行时，基于 tempfile。
供测试和 REPL 使用。
"""
import sys
import tempfile
from contextlib import redirect_stdout
from io import StringIO

from mdio_py import MdioRuntime
from mdio_py.core.bootstrap import bootstrap_runtime


class MdioEphemeral:
    """不落盘的 MDIO 运行时，async context manager 封装 tempdir + bootstrap"""

    def __init__(self):
        self._tmpdir = None
        self._runtime = None

    async def __aenter__(self) -> "MdioEphemeral":
        self._tmpdir = tempfile.TemporaryDirectory()
        storage_root = str(self._tmpdir.name)
        self._runtime = MdioRuntime(storage_root)

        is_tty = sys.stdout.isatty()
        if is_tty:
            await bootstrap_runtime(self._runtime)
        else:
            with redirect_stdout(StringIO()):
                await bootstrap_runtime(self._runtime)

        return self

    async def __aexit__(self, *args):
        self._runtime = None
        self._tmpdir.cleanup()

    @property
    def runtime(self) -> MdioRuntime:
        return self._runtime

    async def exec(self, code: str, **kwargs) -> str:
        """执行 MDIO 管道，默认 wt_uid="genesis" """
        kwargs.setdefault("wt_uid", "genesis")
        return await self._runtime.exec(code, **kwargs)
