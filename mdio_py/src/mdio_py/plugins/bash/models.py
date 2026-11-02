from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import AsyncIterator


@dataclass
class BashConfig:
    backend: str = "local"
    shell: str = "bash"
    timeout: int = 30
    cwd: str = ""
    env: dict[str, str] = field(default_factory=dict)
    session_engine: str = "pexpect"
    stream_chunk: int = 4096
    output_limit: int = 65536


@dataclass
class BashExecutionSpec:
    command: str
    shell: str
    cwd: str
    timeout: int
    stdin_text: str | None = None
    env: dict[str, str] = field(default_factory=dict)
    stream_chunk: int = 4096
    output_limit: int = 65536


@dataclass
class BashStarted:
    os_pid: str = ""
    create_time: str = ""
    pgid: str = ""
    sid: str = ""
    liveness: str = "alive"


@dataclass
class BashHandle:
    id: str
    kind: str = "run"
    target: str = "host"
    session: str = ""
    state: str = "RUNNING"
    started: float = 0.0
    command: str = ""
    config: BashConfig | None = None
    cwd: str = ""
    shell: str = "bash"
    timeout: int = 0
    stdout: str = ""
    os_pid: str = ""
    create_time: str = ""
    pgid: str = ""
    sid: str = ""
    liveness: str = "unknown"
    gen: int = 0
    result: BashResult | None = None
    task: asyncio.Task | None = None

    def get(self, key: str, default=None):
        return getattr(self, key, default)

    def __getitem__(self, key: str):
        return getattr(self, key)

    def __setitem__(self, key: str, value) -> None:
        setattr(self, key, value)

    def keys(self):
        return self.__dict__.keys()

    def update(self, values) -> None:
        for key, value in dict(values).items():
            setattr(self, key, value)


@dataclass
class BashDisposeOutcome:
    handle_id: str
    confirmed: bool = False
    survivors: int = 0
    method: str = ""
    error: str | None = None


@dataclass
class BashDisposeReport:
    outcomes: list[BashDisposeOutcome] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return all(outcome.confirmed for outcome in self.outcomes)


class BashDisposeError(Exception):
    def __init__(self, message: str, report: BashDisposeReport) -> None:
        super().__init__(message)
        self.report = report


@dataclass
class BashResult:
    stdout: str = ""
    stderr: str = ""
    exit_code: str = ""
    status: str = "DONE"
    shell: str = "bash"
    cwd: str = ""
    timeout: int = 30
    os_pid: str = ""
    pgid: str = ""
    create_time: str = ""
    sid: str = ""
    liveness: str = "unknown"
    signal: str = ""


@dataclass
class BashContextHandle:
    name: str
    shell: str
    cwd: str
    timeout: int
    env: dict[str, str] = field(default_factory=dict)
    engine: str = "pexpect"
    status: str = "IDLE"
    current_cmd: str = ""
    os_pid: str = ""
    pgid: str = ""
    create_time: str = ""
    sid: str = ""


@dataclass
class BashStreamHandle:
    stream: AsyncIterator[str]
    done: asyncio.Future[BashResult]
