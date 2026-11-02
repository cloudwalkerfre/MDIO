from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Awaitable, Callable

from ..models import BashContextHandle, BashDisposeReport, BashExecutionSpec, BashResult, BashStarted, BashStreamHandle


class BashBackend(ABC):
    @abstractmethod
    async def run(
        self,
        wt_uid: str,
        run_id: str,
        spec: BashExecutionSpec,
        on_start: Callable[[BashStarted], Awaitable[None]] | None = None,
    ) -> BashResult:
        raise NotImplementedError

    @abstractmethod
    def stream(
        self,
        wt_uid: str,
        run_id: str,
        spec: BashExecutionSpec,
        on_start: Callable[[BashStarted], Awaitable[None]] | None = None,
    ) -> BashStreamHandle:
        raise NotImplementedError

    @abstractmethod
    async def kill_run(self, wt_uid: str, run_id: str) -> bool:
        raise NotImplementedError

    @abstractmethod
    async def open_context(self, wt_uid: str, name: str, spec: BashExecutionSpec) -> BashContextHandle:
        raise NotImplementedError

    @abstractmethod
    async def get_context(self, wt_uid: str, name: str) -> BashContextHandle | None:
        raise NotImplementedError

    @abstractmethod
    def list_contexts(self, wt_uid: str) -> list[BashContextHandle]:
        raise NotImplementedError

    @abstractmethod
    async def context_run(
        self,
        wt_uid: str,
        name: str,
        command_id: str,
        command: str,
        timeout: int | None = None,
    ) -> BashResult:
        raise NotImplementedError

    @abstractmethod
    def context_stream(
        self,
        wt_uid: str,
        name: str,
        command_id: str,
        command: str,
        timeout: int | None = None,
    ) -> BashStreamHandle:
        raise NotImplementedError

    @abstractmethod
    async def restart_context(self, wt_uid: str, name: str, spec: BashExecutionSpec) -> BashContextHandle:
        raise NotImplementedError

    @abstractmethod
    async def kill_context(self, wt_uid: str, name: str) -> bool:
        raise NotImplementedError

    @abstractmethod
    async def close_context(self, wt_uid: str, name: str) -> bool:
        raise NotImplementedError

    async def dispose_wt(self, wt_uid: str) -> BashDisposeReport:
        """Release all processes and contexts owned by one worktree."""
        return BashDisposeReport()
