import asyncio
import http.server
import inspect
import json
import os
import re
import socketserver
import tempfile
import threading
from contextlib import contextmanager
from pathlib import Path

import psutil
import pytest

from mdio_py import MdioRuntime
from mdio_py.plugins.bash.backends.local import ProcessOwner
from mdio_py.plugins.bash.models import BashDisposeOutcome, BashDisposeReport, BashHandle
from mdio_py.plugins.bash.process_probe import BashProcessProbe, ProcessIdentity


def _pty_available() -> bool:
    """named-context 测试依赖 pexpect/pty；部分沙箱环境禁止 open(/dev/pty*)。"""
    import os
    import pty
    try:
        master, slave = pty.openpty()
        os.close(master)
        os.close(slave)
        return True
    except OSError:
        return False


def requires_pty(fn):
    """PTY 用例：无 pty 时跳过；同时归入 slow —— 容器/WSL 里偶发不稳，默认 test-py 不跑。"""
    fn = pytest.mark.slow(fn)
    return pytest.mark.skipif(not _pty_available(),
                              reason="pty devices unavailable in this environment")(fn)

# PTY/pexpect 与 xdist worker 的多线程 fork 容易偶发死锁；固定到同一 worker。
pytestmark = pytest.mark.xdist_group("bash-pty")


def _head_ids(text: str) -> list[str]:
    return re.findall(r"^##\s+([0-9a-f]+)$", text, re.M)


def _latest_pid(runtime: MdioRuntime) -> str:
    bash_var = runtime.store.get_text("genesis", "/var/bash.md")
    pids = _head_ids(bash_var)
    assert pids, bash_var
    return pids[-1]


async def _wait_until(predicate, timeout: float = 10.0, interval: float = 0.05) -> None:
    deadline = asyncio.get_running_loop().time() + timeout
    while asyncio.get_running_loop().time() < deadline:
        if predicate():
            return
        await asyncio.sleep(interval)
    assert predicate(), "condition not met before timeout"


def _proc_text(runtime: MdioRuntime) -> str:
    return runtime.store.get_text("genesis", "/.mds/proc.md")


def _field(text: str, key: str) -> str:
    match = re.search(rf"^- {re.escape(key)}:\s*(.*)$", text, re.M)
    return match.group(1).strip() if match else ""


def _latest_proc_handle(text: str) -> str:
    ids = re.findall(r"^###\s+([0-9a-f]+)$", text, re.M)
    assert ids, text
    return ids[-1]


async def _spawn_sleep(runtime: MdioRuntime, seconds: int = 20) -> tuple[str, str]:
    ack = await runtime.exec(f'@BASH.spawn -cmd "sleep {seconds}"', wt_uid="genesis")
    match = re.search(r"started ([0-9a-f]+)", ack)
    assert match, ack
    run_id = match.group(1)
    await _wait_until(lambda: run_id in _proc_text(runtime) and _field(_proc_text(runtime), "os_pid") != "")
    return run_id, ack


class _QuietHTTPRequestHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args) -> None:
        pass


@contextmanager
def _serve_directory(directory: Path):
    handler = lambda *args, **kwargs: _QuietHTTPRequestHandler(  # noqa: E731
        *args,
        directory=str(directory),
        **kwargs,
    )
    with socketserver.TCPServer(("127.0.0.1", 0), handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            host, port = server.server_address
            yield f"http://{host}:{port}"
        finally:
            server.shutdown()
            thread.join()


async def test_bash_run(runtime: MdioRuntime) -> None:
    res = await runtime.exec('"printf hello" | @BASH', wt_uid="genesis")
    assert res == "hello", res
    first_pid = _latest_pid(runtime)

    default_res = await runtime.exec('"printf world" | @BASH', wt_uid="genesis")
    assert default_res == "world", default_res

    bash_var = runtime.store.get_text("genesis", "/var/bash.md")

    assert f"## {first_pid}" in bash_var
    assert "- mode: command" in bash_var
    assert "- cmd: printf hello" in bash_var
    assert "- out:" in bash_var and "> hello" in bash_var
    assert "- exit: 0" in bash_var

    inspect_res = await runtime.exec(f'@BASH.inspect -id "{first_pid}"', wt_uid="genesis")
    assert "# BASH" in inspect_res and "state: DONE" in inspect_res
    assert first_pid in inspect_res
    assert "printf hello" in inspect_res

    listing = await runtime.exec("@BASH", wt_uid="genesis")
    assert "# BASH" in listing
    assert first_pid in listing
    print("✅ test_bash_run passed")


async def test_bash_legacy_syntax_removed(runtime: MdioRuntime) -> None:
    run_res = await runtime.exec('"printf hi" | @BASH.run', wt_uid="genesis")
    assert "mdio_error" in run_res, run_res
    ctx_res = await runtime.exec("@BASH.dev", wt_uid="genesis")
    assert "mdio_error" in ctx_res, ctx_res
    pid_res = await runtime.exec('@BASH.inspect -pid "deadbeef"', wt_uid="genesis")
    assert "mdio_error" in pid_res, pid_res
    print("✅ test_bash_legacy_syntax_removed passed")


async def test_bash_run_with_cmd_and_stdin(runtime: MdioRuntime) -> None:
    res = await runtime.exec(
        r'''"hello\nworld" | @BASH -cmd "python3 -c \"import sys; print(sys.stdin.read().replace(chr(10), '|'))\"" -stdin _''',
        wt_uid="genesis",
    )
    assert res.strip() == "hello|world", res

    pid = _latest_pid(runtime)
    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    bash_var = runtime.store.get_text("genesis", "/var/bash.md")
    assert "- mode: stdin" in bash_var, bash_var
    assert "- cmd: python3 -c " in bash_var, bash_var
    assert "- stdin: hello" in bash_var, bash_var
    assert "world" in inspect_res, inspect_res
    print("✅ test_bash_run_with_cmd_and_stdin passed")


async def test_bash_run_with_cmd_and_empty_stdin(runtime: MdioRuntime) -> None:
    res = await runtime.exec(
        '@BASH -cmd "python3 -c \\"import sys; print(repr(sys.stdin.read()))\\"" -stdin _',
        wt_uid="genesis",
    )
    assert res == "''\n", res

    pid = _latest_pid(runtime)
    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    bash_var = runtime.store.get_text("genesis", "/var/bash.md")
    assert "- mode: stdin" in bash_var, bash_var
    assert "- stdin:" in bash_var, bash_var
    print("✅ test_bash_run_with_cmd_and_empty_stdin passed")


async def test_bash_cwd_and_env(runtime: MdioRuntime, workdir: Path) -> None:
    host_dir = workdir.as_posix()
    res = await runtime.exec(
        f'"printf %s \\"$BASH_TEST_TOKEN\\" && pwd" | @BASH -cwd "{host_dir}" -env \'{{"BASH_TEST_TOKEN":"OK"}}\'',
        wt_uid="genesis",
    )
    assert "OK" in res, res
    assert host_dir in res, res
    print("✅ test_bash_cwd_and_env passed")


async def test_bash_failure_and_timeout(runtime: MdioRuntime) -> None:
    fail_res = await runtime.exec('"false" | @BASH', wt_uid="genesis")
    assert "mdio_error" in fail_res, fail_res
    fail_pid = _latest_pid(runtime)
    fail_inspect = await runtime.exec(f'@BASH.inspect -id "{fail_pid}"', wt_uid="genesis")
    assert "FAILED" in fail_inspect, fail_inspect

    timeout_res = await runtime.exec('"sleep 2" | @BASH -timeout "1"', wt_uid="genesis")
    assert "mdio_error" in timeout_res, timeout_res
    timeout_pid = _latest_pid(runtime)
    timeout_inspect = await runtime.exec(f'@BASH.inspect -id "{timeout_pid}"', wt_uid="genesis")
    assert "TIMEOUT" in timeout_inspect, timeout_inspect
    print("✅ test_bash_failure_and_timeout passed")


async def test_bash_stderr_capture(runtime: MdioRuntime) -> None:
    res = await runtime.exec('"printf oops >&2; exit 7" | @BASH', wt_uid="genesis")
    assert "mdio_error" in res, res
    assert "oops" in res, res

    pid = _latest_pid(runtime)
    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "FAILED" in inspect_res, inspect_res
    assert "## ERR" in inspect_res and "> oops" in inspect_res, inspect_res
    assert "- exit: 7" in inspect_res, inspect_res
    print("✅ test_bash_stderr_capture passed")


async def test_bash_invalid_inputs(runtime: MdioRuntime) -> None:
    bad_env = await runtime.exec('"printf hi" | @BASH -env "NOT_JSON_OR_PAIR"', wt_uid="genesis")
    assert "mdio_error" in bad_env, bad_env

    bad_cwd = await runtime.exec('"printf hi" | @BASH -cwd "/definitely/not/here"', wt_uid="genesis")
    assert "mdio_error" in bad_cwd, bad_cwd

    bad_shell = await runtime.exec('"printf hi" | @BASH -shell "definitely_not_a_shell"', wt_uid="genesis")
    assert "mdio_error" in bad_shell, bad_shell
    bad_shell_pid = _latest_pid(runtime)
    bad_shell_inspect = await runtime.exec(f'@BASH.inspect -id "{bad_shell_pid}"', wt_uid="genesis")
    assert "FAILED" in bad_shell_inspect, bad_shell_inspect
    assert "SPAWN_ERROR" in bad_shell_inspect, bad_shell_inspect

    miss_inspect = await runtime.exec('@BASH.inspect -id "deadbeef"', wt_uid="genesis")
    assert "mdio_error" in miss_inspect, miss_inspect

    miss_kill = await runtime.exec('@BASH.kill -id "deadbeef"', wt_uid="genesis")
    assert "mdio_error" in miss_kill, miss_kill

    bad_stdin = await runtime.exec('@BASH -cmd "printf hi" -stdin', wt_uid="genesis")
    assert "mdio_error" in bad_stdin, bad_stdin

    missing_cmd = await runtime.exec('@BASH -stdin _', wt_uid="genesis")
    assert "mdio_error" in missing_cmd, missing_cmd

    ignored_pipe = await runtime.exec('"printf hi" | @BASH -cmd "printf yo"', wt_uid="genesis")
    assert "mdio_error" in ignored_pipe, ignored_pipe

    bad_ctx_cwd = await runtime.exec('@BASH.sess -create dev -cwd "/definitely/not/here"', wt_uid="genesis")
    assert "mdio_error" in bad_ctx_cwd, bad_ctx_cwd

    bad_ctx_override = await runtime.exec('"printf hi" | @BASH -sess dev -cwd "/tmp"', wt_uid="genesis")
    assert "mdio_error" in bad_ctx_override, bad_ctx_override

    bad_ctx_stdin = await runtime.exec('"printf hi" | @BASH -sess dev -cmd "cat" -stdin _', wt_uid="genesis")
    assert "mdio_error" in bad_ctx_stdin, bad_ctx_stdin
    print("✅ test_bash_invalid_inputs passed")


async def test_bash_stream_and_kill(runtime: MdioRuntime) -> None:
    stream = await runtime.exec(
        '"while true; do echo tick; sleep 1; done" | @BASH.stream',
        wt_uid="genesis",
        stream=True,
    )
    assert inspect.isasyncgen(stream), type(stream)

    chunks: list[str] = []

    async def consume() -> None:
        async for chunk in stream:
            chunks.append(str(chunk))

    task = asyncio.create_task(consume())
    await _wait_until(lambda: bool(chunks))

    # 等 proc.md 登记 PID，取最后一条（避免旧残留）
    pid = None
    for _ in range(50):
        proc_md = runtime.store.get_text("genesis", "/.mds/proc.md")
        matches = list(re.finditer(r"^###\s+([0-9a-f]+)$", proc_md, re.M))
        if matches:
            pid = matches[-1].group(1)
            break
        await asyncio.sleep(0.1)
    assert pid, f"PID not found in proc.md:\n{proc_md}"

    kill_res = await runtime.exec(f'@BASH.kill -id "{pid}"', wt_uid="genesis")
    # 进程可能已自然结束，kill 返回 not running 也算成功
    assert "kill sent" in kill_res or "not running" in kill_res, kill_res

    await task
    final_stream = "".join(chunks)
    assert "tick" in final_stream, final_stream
    assert "mdio_error" in final_stream, final_stream

    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "KILLED" in inspect_res, inspect_res

    print("✅ test_bash_stream_and_kill passed")


async def test_bash_stream_observability(runtime: MdioRuntime) -> None:
    stream = await runtime.exec(
        '"for i in 1 2 3; do echo $i; sleep 5; done" | @BASH.stream',
        wt_uid="genesis",
        stream=True,
    )

    chunks: list[str] = []

    async def consume() -> None:
        async for chunk in stream:
            chunks.append(str(chunk))

    task = asyncio.create_task(consume())
    await _wait_until(lambda: bool(chunks))

    proc_md = runtime.store.get_text("genesis", "/.mds/proc.md")
    matches = list(re.finditer(r"^###\s+([0-9a-f]+)$", proc_md, re.M))
    assert matches, proc_md
    pid = matches[-1].group(1)

    listing = await runtime.exec("@BASH", wt_uid="genesis")
    assert pid in listing, listing

    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "state: RUNNING" in inspect_res, inspect_res
    assert "state: RUNNING" in inspect_res, inspect_res

    kill_res = await runtime.exec(f'@BASH.kill -id "{pid}"', wt_uid="genesis")
    assert "kill sent" in kill_res or "not running" in kill_res, kill_res
    await task
    print("✅ test_bash_stream_observability passed")


async def test_bash_stream_timeout(runtime: MdioRuntime) -> None:
    stream = await runtime.exec(
        '"sleep 2" | @BASH.stream -timeout "1"',
        wt_uid="genesis",
        stream=True,
    )

    chunks: list[str] = []
    async for chunk in stream:
        chunks.append(str(chunk))

    combined = "".join(chunks)
    assert "mdio_error" in combined, combined

    pid = _latest_pid(runtime)
    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "TIMEOUT" in inspect_res, inspect_res
    assert "TIMEOUT" in inspect_res, inspect_res
    print("✅ test_bash_stream_timeout passed")


async def test_bash_stream_stdout_and_stderr(runtime: MdioRuntime) -> None:
    stream = await runtime.exec(
        '"printf out; printf err >&2; exit 9" | @BASH.stream',
        wt_uid="genesis",
        stream=True,
    )

    chunks: list[str] = []
    async for chunk in stream:
        chunks.append(str(chunk))

    combined = "".join(chunks)
    assert "out" in combined, combined
    assert "mdio_error" in combined, combined
    assert "err" in combined, combined

    pid = _latest_pid(runtime)
    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "## RES" in inspect_res and "> out" in inspect_res, inspect_res
    assert "## ERR" in inspect_res and "> err" in inspect_res, inspect_res
    assert "- exit: 9" in inspect_res, inspect_res
    print("✅ test_bash_stream_stdout_and_stderr passed")


async def test_bash_defaults_from_config(runtime: MdioRuntime) -> None:
    # Ensure config file exists before editing defaults.
    await runtime.exec("@BASH", wt_uid="genesis")
    await runtime.exec('"sh" | /> /.mds/bash.md::CONFIG._b:shell', wt_uid="genesis")
    await runtime.exec('"1" | /> /.mds/bash.md::CONFIG._b:timeout', wt_uid="genesis")

    timeout_res = await runtime.exec('"sleep 2" | @BASH', wt_uid="genesis")
    assert "mdio_error" in timeout_res, timeout_res

    pid = _latest_pid(runtime)
    inspect_res = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "shell: sh" in inspect_res, inspect_res
    assert "timeout: 1" in inspect_res, inspect_res
    print("✅ test_bash_defaults_from_config passed")


@requires_pty
async def test_bash_named_context_state_and_run(runtime: MdioRuntime, workdir: Path) -> None:
    base_dir = workdir.resolve()
    nested_dir = (workdir / "nested").resolve()
    nested_dir.mkdir()

    init_res = await runtime.exec(
        f'@BASH.sess -create dev -cwd "{base_dir.as_posix()}" -timeout "15"',
        wt_uid="genesis",
    )
    assert "configured" in init_res, init_res

    state_res = await runtime.exec("@BASH.sess -inspect dev", wt_uid="genesis")
    assert "# BASH_CONTEXT" in state_res, state_res
    assert f"- cwd: {base_dir.as_posix()}" in state_res, state_res

    first = await runtime.exec('"pwd" | @BASH -sess dev', wt_uid="genesis")
    assert base_dir.as_posix() in first, first

    second = await runtime.exec(
        f'"cd {nested_dir.as_posix()} && pwd" | @BASH -sess dev',
        wt_uid="genesis",
    )
    assert nested_dir.as_posix() in second, second

    third = await runtime.exec('"pwd" | @BASH -sess dev', wt_uid="genesis")
    assert nested_dir.as_posix() in third, third

    inspect_res = await runtime.exec("@BASH.sess -inspect dev", wt_uid="genesis")
    assert "# BASH_CONTEXT_INSPECT" in inspect_res, inspect_res
    assert "state: IDLE" in inspect_res, inspect_res
    assert "cmd_" in inspect_res, inspect_res
    assert "cd " in inspect_res, inspect_res
    assert "- cmd: pwd" in inspect_res, inspect_res
    print("✅ test_bash_named_context_state_and_run passed")


@requires_pty
async def test_bash_named_context_stream_restart_and_close(runtime: MdioRuntime, workdir: Path) -> None:
    base_dir = workdir.resolve()
    moved_dir = (workdir / "moved").resolve()
    moved_dir.mkdir()

    await runtime.exec(
        f'@BASH.sess -create devstream -cwd "{base_dir.as_posix()}" -timeout "15"',
        wt_uid="genesis",
    )

    stream = await runtime.exec(
        '"printf hello-from-context" | @BASH.stream -sess devstream',
        wt_uid="genesis",
        stream=True,
    )
    assert inspect.isasyncgen(stream), type(stream)

    chunks: list[str] = []
    async for chunk in stream:
        chunks.append(str(chunk))
    combined = "".join(chunks)
    assert "hello-from-context" in combined, combined

    moved = await runtime.exec(
        f'"cd {moved_dir.as_posix()} && pwd" | @BASH -sess devstream',
        wt_uid="genesis",
    )
    assert moved_dir.as_posix() in moved, moved

    restart_res = await runtime.exec("@BASH.sess -restart devstream", wt_uid="genesis")
    assert "restarted" in restart_res, restart_res

    after_restart = await runtime.exec('"pwd" | @BASH -sess devstream', wt_uid="genesis")
    assert base_dir.as_posix() in after_restart, after_restart

    close_res = await runtime.exec("@BASH.sess -close devstream", wt_uid="genesis")
    assert "closed" in close_res, close_res

    state_res = await runtime.exec("@BASH.sess -inspect devstream", wt_uid="genesis")
    assert "CLOSED" in state_res, state_res
    assert "status: CLOSED" in state_res, state_res
    print("✅ test_bash_named_context_stream_restart_and_close passed")


@requires_pty
async def test_bash_named_context_environment_persists(runtime: MdioRuntime, workdir: Path) -> None:
    base_dir = workdir.resolve()

    init_res = await runtime.exec(
        f'@BASH.sess -create envbox -cwd "{base_dir.as_posix()}" -timeout "15"',
        wt_uid="genesis",
    )
    assert "configured" in init_res, init_res

    export_res = await runtime.exec(
        '"export BASH_SESSION_TOKEN=alpha-123" | @BASH -sess envbox',
        wt_uid="genesis",
    )
    assert export_res.strip() == "", export_res

    token_res = await runtime.exec(
        '"printf %s \\"$BASH_SESSION_TOKEN\\"" | @BASH -sess envbox',
        wt_uid="genesis",
    )
    assert token_res.strip() == "alpha-123", token_res

    listing = await runtime.exec("@BASH", wt_uid="genesis")
    assert "envbox" in listing, listing

    inspect_res = await runtime.exec("@BASH.sess -inspect envbox", wt_uid="genesis")
    assert "BASH_SESSION_TOKEN" in inspect_res, inspect_res
    assert "alpha-123" in inspect_res, inspect_res
    print("✅ test_bash_named_context_environment_persists passed")


async def test_bash_mdio_python_roundtrip(runtime: MdioRuntime) -> None:
    await runtime.exec('mk -f /job2.md ; mk -f /roundtrip_report.md', wt_uid="genesis")
    await runtime.exec(
        '''"""# INPUTS
- phrase: hello mdio world
- payload: {"numbers":[2,3,5],"tag":"neo"}
""" | /> /job2.md::''',
        wt_uid="genesis",
    )

    pipeline = r'''
$/job2.md::INPUTS._b:phrase | fmt -t """python3 -c "import sys; print(sys.stdin.read().strip().title())" <<< "{}" """ | @BASH | trim | fmt -t "- title: {}" | +> /roundtrip_report.md::REPORT ;
$/job2.md::INPUTS._b:payload | fmt -t """python3 -c "import json,sys; data=json.loads(sys.stdin.read()); print(sum(data['numbers']))" <<< '{}' """ | @BASH | trim | fmt -t "- number_sum: {}" | +> /roundtrip_report.md::REPORT
'''
    res = await runtime.exec(pipeline, wt_uid="genesis")
    assert "mdio_error" not in str(res), res

    report = runtime.store.get_text("genesis", "/roundtrip_report.md")
    assert "- title: Hello Mdio World" in report, report
    assert "- number_sum: 10" in report, report
    print("✅ test_bash_mdio_python_roundtrip passed")


async def test_bash_oneshot_heredoc_pipeline(runtime: MdioRuntime, workdir: Path) -> None:
    project_dir = workdir / "heredoc_project"
    project_dir.mkdir()

    res = await runtime.exec(
        f'''"""
cd "{project_dir.as_posix()}"
cat <<'EOF' > notes.txt
alpha
beta alpha
gamma
EOF
grep -n "alpha" notes.txt | python3 -c "import sys; print('|'.join(line.strip() for line in sys.stdin if line.strip()))"
""" | @BASH''',
        wt_uid="genesis",
    )
    assert res.strip() == "1:alpha|2:beta alpha", res

    notes = (project_dir / "notes.txt").read_text(encoding="utf-8")
    assert notes == "alpha\nbeta alpha\ngamma\n", notes
    print("✅ test_bash_oneshot_heredoc_pipeline passed")


@requires_pty
async def test_bash_named_context_agent_file_workflow(runtime: MdioRuntime, workdir: Path) -> None:
    project_dir = workdir / "agent_flow"
    project_dir.mkdir()

    init_res = await runtime.exec(
        f'@BASH.sess -create agent -cwd "{project_dir.as_posix()}" -timeout "15"',
        wt_uid="genesis",
    )
    assert "configured" in init_res, init_res

    seed_res = await runtime.exec(
        '''"""
mkdir -p src docs
cat <<'EOF' > src/app.py
print("hello")
# TODO: wire cli
EOF
cat <<'EOF' > src/utils.py
def slugify(text):
    return text.lower().replace(" ", "-")
EOF
cat <<'EOF' > docs/notes.md
# Notes
TODO: add examples
EOF
""" | @BASH -sess agent''',
        wt_uid="genesis",
    )
    assert seed_res.strip() == "", seed_res

    find_res = await runtime.exec('"find . -type f | sort" | @BASH -sess agent', wt_uid="genesis")
    assert "./docs/notes.md" in find_res, find_res
    assert "./src/app.py" in find_res, find_res
    assert "./src/utils.py" in find_res, find_res

    grep_res = await runtime.exec('"""grep -RIn "TODO" . | sort""" | @BASH -sess agent', wt_uid="genesis")
    assert "./docs/notes.md:2:TODO: add examples" in grep_res, grep_res
    assert "./src/app.py:2:# TODO: wire cli" in grep_res, grep_res

    py_res = await runtime.exec(
        '''"""
python3 - <<'PY'
from pathlib import Path
files = sorted(str(p.relative_to(Path("."))) for p in Path(".").rglob("*") if p.is_file())
print(len(files))
print(files[0])
print(files[-1])
PY
""" | @BASH -sess agent''',
        wt_uid="genesis",
    )
    lines = [line for line in py_res.splitlines() if line]
    assert lines == ["3", "docs/notes.md", "src/utils.py"], py_res

    inspect_res = await runtime.exec("@BASH.sess -inspect agent", wt_uid="genesis")
    assert "grep -RIn" in inspect_res, inspect_res
    assert "python3 - <<'PY'" in inspect_res, inspect_res
    print("✅ test_bash_named_context_agent_file_workflow passed")


async def test_bash_practical_tool_chains(runtime: MdioRuntime, workdir: Path) -> None:
    npm_dir = workdir / "npm_app"
    npm_dir.mkdir()
    (npm_dir / "package.json").write_text(
        json.dumps({"name": "mdio-demo", "version": "1.0.0"}),
        encoding="utf-8",
    )

    api_dir = workdir / "api"
    api_dir.mkdir()
    (api_dir / "tool.json").write_text(
        json.dumps({"tool": "curl", "mode": "wget", "status": "ok"}),
        encoding="utf-8",
    )

    with _serve_directory(api_dir) as base_url:
        job_doc = f'''"""# INPUTS
- phrase: hello from mdio
- npm_dir: {npm_dir.as_posix()}
- curl_url: {base_url}/tool.json
- wget_url: {base_url}/tool.json
""" | /> /job.md::'''
        await runtime.exec('mk -f /job.md ; mk -f /tool_report.md', wt_uid="genesis")
        await runtime.exec(job_doc, wt_uid="genesis")

        pipeline = r'''
$/job.md::INPUTS._b:phrase | fmt -t """python3 -c "print('{}'.title())" """ | @BASH | trim | fmt -t "- python_title: {}" | +> /tool_report.md::REPORT ;
$/job.md::INPUTS._b:npm_dir | fmt -t """cd "{}" && npm prefix""" | @BASH | trim | fmt -t "- npm_prefix: {}" | +> /tool_report.md::REPORT ;
$/job.md::INPUTS._b:curl_url | fmt -t "curl -fsSL {}" | @BASH | loads | get("tool") | fmt -t "- curl_tool: {}" | +> /tool_report.md::REPORT ;
$/job.md::INPUTS._b:wget_url | fmt -t "wget -qO- {}" | @BASH | loads | get("mode") | fmt -t "- wget_mode: {}" | +> /tool_report.md::REPORT
'''
        res = await runtime.exec(pipeline, wt_uid="genesis")
        assert "mdio_error" not in str(res), res

    report = runtime.store.get_text("genesis", "/tool_report.md")
    assert "- python_title: Hello From Mdio" in report, report
    assert f"- npm_prefix: {npm_dir.as_posix()}" in report, report
    assert "- curl_tool: curl" in report, report
    assert "- wget_mode: wget" in report, report
    print("✅ test_bash_practical_tool_chains passed")


async def test_bash_spawn_and_cb(runtime: MdioRuntime) -> None:
    ack = await runtime.exec('"printf spawned" | @BASH.spawn', wt_uid="genesis")
    assert "started" in ack, ack
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(150):
        doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
        if "state: DONE" in doc:
            break
        await asyncio.sleep(0.1)
    assert "state: DONE" in doc, doc
    assert "> spawned" in doc, doc

    await runtime.exec("mk -f /spawn_cb.md", wt_uid="genesis")
    ack2 = await runtime.exec(
        '@BASH.spawn -cmd "printf callback" -cb "+> /spawn_cb.md::"',
        wt_uid="genesis",
    )
    run_id2 = re.search(r"started ([0-9a-f]+)", ack2).group(1)
    for _ in range(50):
        cb_doc = runtime.store.get_text("genesis", "/spawn_cb.md")
        if "callback" in cb_doc:
            break
        await asyncio.sleep(0.1)
    assert "# BASH" in cb_doc and "> callback" in cb_doc, cb_doc
    print("✅ test_bash_spawn_and_cb passed")


async def test_bash_tools(runtime: MdioRuntime) -> None:
    await runtime.exec("@BASH", wt_uid="genesis")
    manifest = '''"""# TOOLS
## echo
- provider: cli
- ops: run
- argv: {"run": ["/bin/echo", "{text}"]}
- args: text
- help: echo {text}
""" | +> /.mds/bash.md::'''
    await runtime.exec(manifest, wt_uid="genesis")
    listing = await runtime.exec("@BASH.tools", wt_uid="genesis")
    assert "echo" in listing, listing
    help_res = await runtime.exec("@BASH.tools.echo", wt_uid="genesis")
    assert "/bin/echo" in help_res, help_res
    run_res = await runtime.exec("@BASH.tools.echo -op run -args '{\"text\": \"tool-hi\"}'", wt_uid="genesis")
    assert run_res.strip() == "tool-hi", run_res
    print("✅ test_bash_tools passed")


@requires_pty
async def test_bash_dispose_wt(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create disp -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    await runtime.exec('"printf dispose" | @BASH -sess disp', wt_uid="genesis")
    service = runtime._workspace.get_plugin("BASH")._service
    assert service._backend.list_contexts("genesis")
    await runtime._workspace.dispose_plugins_wt("genesis")
    assert not service._backend.list_contexts("genesis")
    print("✅ test_bash_dispose_wt passed")


@requires_pty
async def test_bash_session_timeout_reset(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create resetbox -cwd "{workdir.as_posix()}" -timeout "1"',
        wt_uid="genesis",
    )
    timed = await runtime.exec('"sleep 2" | @BASH -sess resetbox', wt_uid="genesis")
    assert "mdio_error" in timed or "TIMEOUT" in timed, timed
    listing = await runtime.exec("@BASH", wt_uid="genesis")
    assert "- resetbox: status=CONFIG_ONLY" in listing, listing
    state = await runtime.exec("@BASH.sess -inspect resetbox", wt_uid="genesis")
    assert "status: IDLE" not in state, state
    assert "state: CLOSED" in state, state
    after = await runtime.exec('"printf after-reset" | @BASH -sess resetbox', wt_uid="genesis")
    assert after.strip() == "after-reset", after
    print("✅ test_bash_session_timeout_reset passed")


async def test_bash_spawn_failure_cb_and_kill(runtime: MdioRuntime) -> None:
    await runtime.exec("mk -f /cb_fail.md", wt_uid="genesis")
    ack = await runtime.exec('@BASH.spawn -cmd "false" -cb "+> /cb_fail.md::"', wt_uid="genesis")
    fail_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(100):
        doc = await runtime.exec(f'@BASH.inspect -id "{fail_id}"', wt_uid="genesis")
        if "cb_status" in doc:
            break
        await asyncio.sleep(0.1)
    assert "state: FAILED" in doc, doc
    assert "cb_status: DONE" in doc, doc
    assert "callback" not in runtime.store.get_text("genesis", "/cb_fail.md")  # empty output

    await runtime.exec("mk -f /cb_kill.md", wt_uid="genesis")
    ack2 = await runtime.exec('@BASH.spawn -cmd "sleep 5" -cb "+> /cb_kill.md::"', wt_uid="genesis")
    kill_id = re.search(r"started ([0-9a-f]+)", ack2).group(1)
    await asyncio.sleep(0.3)
    await runtime.exec(f'@BASH.kill -id "{kill_id}"', wt_uid="genesis")
    for _ in range(50):
        kdoc = await runtime.exec(f'@BASH.inspect -id "{kill_id}"', wt_uid="genesis")
        if "state: KILLED" in kdoc:
            break
        await asyncio.sleep(0.1)
    assert "state: KILLED" in kdoc, kdoc
    for _ in range(50):
        kdoc = await runtime.exec(f'@BASH.inspect -id "{kill_id}"', wt_uid="genesis")
        if "cb_status: DONE" in kdoc or "cb_status: FAILED" in kdoc:
            break
        await asyncio.sleep(0.1)
    assert "cb_status" in kdoc, kdoc
    print("✅ test_bash_spawn_failure_cb_and_kill passed")


async def test_bash_bq_output_safe(runtime: MdioRuntime) -> None:
    res = await runtime.exec('"""printf \'## heading\\n~~~\\n> q\\n\'""" | @BASH', wt_uid="genesis")
    assert "## heading" in res, res
    pid = _latest_pid(runtime)
    doc = await runtime.exec(f'@BASH.inspect -id "{pid}"', wt_uid="genesis")
    assert "> ## heading" in doc, doc
    assert "> ~~~" in doc, doc
    assert "> > q" in doc, doc
    print("✅ test_bash_bq_output_safe passed")


async def test_bash_tools_allowlist(runtime: MdioRuntime) -> None:
    await runtime.exec("@BASH", wt_uid="genesis")
    manifest = '''"""# TOOLS
## blocked
- provider: cli
- ops: run
- argv: {"run": ["/bin/echo", "hi"]}
- allow: ["/bin/true"]
""" | +> /.mds/bash.md::'''
    await runtime.exec(manifest, wt_uid="genesis")
    res = await runtime.exec("@BASH.tools.blocked -op run", wt_uid="genesis")
    assert "not allowed" in res, res
    print("✅ test_bash_tools_allowlist passed")


@requires_pty
async def test_bash_spawn_session(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create spawnsess -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    ack = await runtime.exec('"printf session-spawn" | @BASH.spawn -sess spawnsess', wt_uid="genesis")
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(50):
        doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
        if "state: DONE" in doc:
            break
        await asyncio.sleep(0.1)
    assert "state: DONE" in doc, doc
    assert "> session-spawn" in doc, doc
    assert "- session: spawnsess" in doc, doc
    assert f"- run_id: {run_id}" in doc, doc
    var = runtime.store.get_text("genesis", "/var/bash.md")
    assert "- session: spawnsess" in var, var
    assert f"- run_id: {run_id}" in var, var
    last = await runtime.exec("@BASH.inspect -last", wt_uid="genesis")
    assert run_id in last, last
    listing = await runtime.exec("@BASH", wt_uid="genesis")
    assert "## LAST_SESSION_HANDLE" in listing, listing
    assert run_id in listing, listing
    print("✅ test_bash_spawn_session passed")


async def test_bash_spawn_inspect_shows_output(runtime: MdioRuntime) -> None:
    ack = await runtime.exec('@BASH.spawn -cmd "echo spawn-output; sleep 20"', wt_uid="genesis")
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    await _wait_until(
        lambda: run_id in _proc_text(runtime) and _field(_proc_text(runtime), "os_pid") != "",
        timeout=10,
    )
    for _ in range(50):
        doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
        if "spawn-output" in doc:
            break
        await asyncio.sleep(0.05)
    assert "spawn-output" in doc, doc
    await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")


@requires_pty
async def test_bash_spawn_session_cb(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create cbsess -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    await runtime.exec("mk -f /sess_cb.md", wt_uid="genesis")
    ack = await runtime.exec(
        '@BASH.spawn -cmd "printf sess-cb" -cb "+> /sess_cb.md::" -sess cbsess',
        wt_uid="genesis",
    )
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(50):
        cb = runtime.store.get_text("genesis", "/sess_cb.md")
        if "sess-cb" in cb:
            break
        await asyncio.sleep(0.1)
    assert "sess-cb" in cb, cb
    assert f"- run_id: {run_id}" in runtime.store.get_text("genesis", "/var/bash.md")


@requires_pty
async def test_bash_spawn_session_kill_by_handle(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create killsess -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    ack = await runtime.exec(
        '@BASH.spawn -cmd "sleep 20" -sess killsess',
        wt_uid="genesis",
    )
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    service = runtime._workspace.get_plugin("BASH")._service
    await _wait_until(lambda: (run_id, ) and (run_id in [k[1] for k in service._live]), timeout=10)
    killed = await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")
    assert "kill sent" in killed, killed
    await _wait_until(lambda: run_id not in [k[1] for k in service._live], timeout=10)
    after = await runtime.exec('"printf after-kill" | @BASH -sess killsess', wt_uid="genesis")
    assert after.strip() == "after-kill", after


@requires_pty
async def test_bash_spawn_session_inspect_shows_output(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create outsess -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    ack = await runtime.exec(
        '@BASH.spawn -cmd "echo session-output; sleep 20" -sess outsess',
        wt_uid="genesis",
    )
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(100):
        doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
        if "session-output" in doc:
            break
        await asyncio.sleep(0.05)
    assert "session-output" in doc, doc
    killed = await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")
    assert "kill sent" in killed, killed


async def test_bash_spawn_identity_and_liveness(runtime: MdioRuntime) -> None:
    run_id, _ = await _spawn_sleep(runtime)
    proc = _proc_text(runtime)
    assert f"### {run_id}" in proc
    os_pid = _field(proc, "os_pid")
    assert os_pid
    assert _field(proc, "create_time")
    assert _field(proc, "pgid")
    assert _field(proc, "sid")
    doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
    assert "liveness: alive" in doc, doc
    assert f"os_pid: {os_pid}" in doc, doc
    assert "- mode: command" in doc, doc
    assert f"- run_id: {run_id}" in doc, doc
    await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")
    pid = int(os_pid)
    await _wait_until(lambda: not psutil.pid_exists(pid), timeout=10)
    await _wait_until(lambda: run_id not in _proc_text(runtime), timeout=10)


async def test_bash_stream_aclose_cleans_process_tree(runtime: MdioRuntime) -> None:
    stream = await runtime.exec(
        '"echo READY; sleep 20" | @BASH.stream',
        wt_uid="genesis",
        stream=True,
    )
    first = await stream.__anext__()
    assert "READY" in str(first), first
    proc = _proc_text(runtime)
    run_id = _latest_proc_handle(proc)
    os_pid = int(_field(proc, "os_pid"))
    await stream.aclose()
    await _wait_until(lambda: not psutil.pid_exists(os_pid), timeout=10)
    await _wait_until(lambda: run_id not in _proc_text(runtime), timeout=10)


async def test_bash_pid_reuse_is_mismatch_and_never_killed() -> None:
    probe = BashProcessProbe()
    pid = os.getpid()
    current = probe.capture(pid)
    assert current is not None
    stale = ProcessIdentity(
        pid=pid,
        create_time=current.create_time + 1000,
        pgid=current.pgid,
        sid=current.sid,
    )
    fact = probe.probe(stale)
    assert fact.liveness == "mismatch", fact
    outcome = probe.terminate_tree("stale", stale, grace_ms=100)
    assert not outcome.confirmed
    assert outcome.method == "mismatch"


async def test_bash_dispose_wt_strict_kills_spawn(runtime: MdioRuntime) -> None:
    run_id, _ = await _spawn_sleep(runtime)
    proc = _proc_text(runtime)
    os_pid = int(_field(proc, "os_pid"))
    await runtime._workspace.dispose_plugins_wt("genesis", strict=True)
    await _wait_until(lambda: not psutil.pid_exists(os_pid), timeout=10)
    assert f"### {run_id}" not in _proc_text(runtime)
    doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
    assert "state: KILLED" in doc, doc


@requires_pty
async def test_bash_context_stream_aclose_resets_session(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create aclosebox -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    stream = await runtime.exec(
        '"echo READY; sleep 20" | @BASH.stream -sess aclosebox',
        wt_uid="genesis",
        stream=True,
    )
    first = await stream.__anext__()
    assert "READY" in str(first), first
    await stream.aclose()
    await asyncio.sleep(0.5)
    after = await runtime.exec('"printf after-aclose" | @BASH -sess aclosebox', wt_uid="genesis")
    assert after.strip() == "after-aclose", after
    await runtime.exec("@BASH.sess -close aclosebox", wt_uid="genesis")


@pytest.mark.slow  # macOS runner：SIGKILL 后 reparent 的孙进程可能残留，属进程树清理的边界行为
async def test_bash_probe_terminates_reparented_session_children() -> None:
    probe = BashProcessProbe()
    import subprocess
    proc = subprocess.Popen(["bash", "-lc", "sleep 30 & wait"], start_new_session=True)
    try:
        identity = probe.capture(proc.pid)
        assert identity is not None
        child_pids: list[int] = []
        for _ in range(100):
            child_pids = [child.pid for child in psutil.Process(proc.pid).children(recursive=True)]
            if child_pids:
                break
            await asyncio.sleep(0.02)
        assert child_pids
        os.kill(proc.pid, 9)
        proc.wait(timeout=5)
        outcome = probe.terminate_tree("reparented", identity, grace_ms=500)
        assert outcome.confirmed, outcome
        loop = asyncio.get_running_loop()
        deadline = loop.time() + 10.0
        for pid in child_pids:
            while psutil.pid_exists(pid) and loop.time() < deadline:
                await asyncio.sleep(0.02)
            assert not psutil.pid_exists(pid), f"survivor {pid}"
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5)


async def test_bash_probe_term_trap_escalates_to_sigkill() -> None:
    probe = BashProcessProbe()
    import subprocess
    proc = subprocess.Popen(["bash", "-lc", "trap '' TERM; sleep 30"], start_new_session=True)
    try:
        identity = probe.capture(proc.pid)
        assert identity is not None
        outcome = probe.terminate_tree("term-trap", identity, grace_ms=100)
        assert outcome.confirmed, outcome
        proc.wait(timeout=10)
        assert not psutil.pid_exists(proc.pid)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5)


async def test_bash_dispose_wt_marks_unconfirmed(runtime: MdioRuntime, monkeypatch) -> None:
    run_id, _ = await _spawn_sleep(runtime)
    service = runtime._workspace.get_plugin("BASH")._service
    original = service._backend._probe.terminate_tree

    def fake(handle_id, identity, grace_ms=1000):
        from mdio_py.plugins.bash.models import BashDisposeOutcome
        return BashDisposeOutcome(handle_id=handle_id, confirmed=False, error="fake survivor")

    monkeypatch.setattr(service._backend._probe, "terminate_tree", fake)
    with pytest.raises(Exception):
        await runtime._workspace.dispose_plugins_wt("genesis", strict=True)
    proc = _proc_text(runtime)
    assert f"### {run_id}" in proc, proc
    assert "status: UNCONFIRMED" in proc, proc
    monkeypatch.setattr(service._backend._probe, "terminate_tree", original)
    await runtime._workspace.dispose_plugins_wt("genesis", strict=False)
    await _wait_until(lambda: run_id not in _proc_text(runtime), timeout=10)


@requires_pty
async def test_bash_context_proc_identity(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create identbox -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    await runtime.exec('"printf ident" | @BASH -sess identbox', wt_uid="genesis")
    proc = _proc_text(runtime)
    assert "### identbox" in proc, proc
    assert _field(proc, "os_pid"), proc
    assert _field(proc, "create_time"), proc
    assert _field(proc, "sid"), proc
    assert "liveness: alive" in proc, proc
    await runtime.exec("@BASH.sess -close identbox", wt_uid="genesis")


async def test_bash_inspect_last_follows_spawn(runtime: MdioRuntime) -> None:
    run_id, _ = await _spawn_sleep(runtime)
    doc = await runtime.exec("@BASH.inspect -last", wt_uid="genesis")
    assert run_id in doc, doc
    assert "liveness: alive" in doc or "state: RUNNING" in doc, doc
    await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")


async def test_bash_context_dispose_strict(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create strictbox -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    await runtime.exec('"printf strict" | @BASH -sess strictbox', wt_uid="genesis")
    service = runtime._workspace.get_plugin("BASH")._service
    assert service._backend.list_contexts("genesis")
    await runtime._workspace.dispose_plugins_wt("genesis", strict=True)
    assert not service._backend.list_contexts("genesis")


async def test_bash_dispose_cancels_foreground_stream(runtime: MdioRuntime) -> None:
    chunks: list[str] = []
    stream = await runtime.exec(
        '"echo READY; sleep 20" | @BASH.stream',
        wt_uid="genesis",
        stream=True,
    )

    async def consume() -> None:
        async for chunk in stream:
            chunks.append(str(chunk))

    task = asyncio.create_task(consume())
    await _wait_until(lambda: bool(chunks))
    proc = _proc_text(runtime)
    run_id = _latest_proc_handle(proc)
    os_pid = int(_field(proc, "os_pid"))
    await runtime._workspace.dispose_plugins_wt("genesis", strict=True)
    await _wait_until(lambda: not psutil.pid_exists(os_pid), timeout=10)
    try:
        await asyncio.wait_for(task, timeout=5)
    except asyncio.CancelledError:
        pass
    assert run_id not in _proc_text(runtime)


@requires_pty
async def test_bash_spawn_session_has_proc_identity(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create idsess -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    ack = await runtime.exec(
        '@BASH.spawn -cmd "sleep 20" -sess idsess',
        wt_uid="genesis",
    )
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    await _wait_until(
        lambda: "### idsess" in _proc_text(runtime) and _field(_proc_text(runtime), "os_pid") != "",
        timeout=10,
    )
    proc = _proc_text(runtime)
    assert f"### {run_id}" not in proc, proc
    assert _field(proc, "create_time"), proc
    assert _field(proc, "sid"), proc
    doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
    assert "state: RUNNING" in doc, doc
    await runtime._workspace.dispose_plugins_wt("genesis", strict=True)
    assert run_id not in _proc_text(runtime)
    assert not runtime._workspace.get_plugin("BASH")._service._backend.list_contexts("genesis")


async def test_bash_list_state_shows_liveness(runtime: MdioRuntime) -> None:
    run_id, _ = await _spawn_sleep(runtime)
    listing = await runtime.exec("@BASH", wt_uid="genesis")
    assert "liveness=alive" in listing, listing
    assert "## ACTIVE" in listing, listing
    assert "## CONTEXTS" in listing, listing
    assert "## RECENT" in listing, listing
    await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")


async def test_bash_signal_in_completion_doc(runtime: MdioRuntime) -> None:
    run_id, _ = await _spawn_sleep(runtime)
    await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")
    for _ in range(100):
        doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
        if "state: KILLED" in doc:
            break
        await asyncio.sleep(0.05)
    assert "state: KILLED" in doc, doc
    assert "signal: SIG" in doc, doc


@requires_pty
async def test_bash_tools_session_metadata(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec("@BASH", wt_uid="genesis")
    manifest = '''"""# TOOLS
## echo
- provider: cli
- ops: run
- argv: {"run": ["/bin/echo", "{text}"]}
- help: echo {text}
""" | +> /.mds/bash.md::'''
    await runtime.exec(manifest, wt_uid="genesis")
    await runtime.exec(
        f'@BASH.sess -create toolsess -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    res = await runtime.exec(
        '@BASH.tools.echo -op run -args \'{"text":"session-tool"}\' -sess toolsess',
        wt_uid="genesis",
    )
    assert "session-tool" in res, res
    var = runtime.store.get_text("genesis", "/var/bash.md")
    assert "- tool: echo" in var, var
    assert "- op: run" in var, var
    await runtime.exec("@BASH.sess -close toolsess", wt_uid="genesis")


async def test_bash_router_validation(runtime: MdioRuntime) -> None:
    assert "mdio_error" in await runtime.exec("@BASH.stream", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.spawn", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.spawn -sess nosess", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec(
        '"printf hi" | @BASH -cb "+> /tmp/x.md::"', wt_uid="genesis",
    )
    assert "mdio_error" in await runtime.exec("@BASH -sess", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec('"x" | @BASH.sess', wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.sess -create", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec(
        "@BASH.sess -create a -inspect a", wt_uid="genesis",
    )
    assert "mdio_error" in await runtime.exec("@BASH.sess -inspect missing", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.sess -kill missing", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.inspect", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec('"x" | @BASH.inspect -id x', wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.inspect -last", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.kill", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec('"x" | @BASH.kill -id x', wt_uid="genesis")
    assert "mdio_error" in await runtime.exec('"x" | @BASH.tools', wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.tools.missing", wt_uid="genesis")
    assert "# BASH" in await runtime.exec("@BASH.help", wt_uid="genesis")


@requires_pty
async def test_bash_session_actions(runtime: MdioRuntime, workdir: Path) -> None:
    init = await runtime.exec(
        f'@BASH.sess -create actions -cwd "{workdir.as_posix()}" -timeout "10" -env \'{{"ACT_TOKEN":"v1"}}\'',
        wt_uid="genesis",
    )
    assert "configured" in init, init
    listed = await runtime.exec("@BASH.sess", wt_uid="genesis")
    assert "actions" in listed, listed
    state = await runtime.exec("@BASH.sess -inspect actions", wt_uid="genesis")
    assert "state: CONFIGURED" in state, state
    assert "ACT_TOKEN" in state, state
    ran = await runtime.exec('"printf action-ok" | @BASH -sess actions', wt_uid="genesis")
    assert ran.strip() == "action-ok", ran
    state = await runtime.exec("@BASH.sess -inspect actions", wt_uid="genesis")
    assert "state: IDLE" in state, state
    killed = await runtime.exec("@BASH.sess -kill actions", wt_uid="genesis")
    assert "killed" in killed, killed
    listed = await runtime.exec("@BASH", wt_uid="genesis")
    assert "- actions: status=CONFIG_ONLY" in listed, listed
    state = await runtime.exec("@BASH.sess -inspect actions", wt_uid="genesis")
    assert "state: KILLED" in state, state
    closed = await runtime.exec("@BASH.sess -close actions", wt_uid="genesis")
    assert "already closed" in closed, closed


async def test_bash_spawn_stdin_and_cb_pipe(runtime: MdioRuntime) -> None:
    ack = await runtime.exec(
        '"spawn-stdin" | @BASH.spawn -cmd "cat" -stdin _', wt_uid="genesis",
    )
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(150):
        doc = await runtime.exec(f'@BASH.inspect -id "{run_id}"', wt_uid="genesis")
        if "spawn-stdin" in doc:
            break
        await asyncio.sleep(0.1)
    assert "spawn-stdin" in doc, doc

    await runtime.exec("mk -f /spawn_cb_pipe.md", wt_uid="genesis")
    ack2 = await runtime.exec(
        '@BASH.spawn -cmd "printf cb-pipe" -cb "+> /spawn_cb_pipe.md::"',
        wt_uid="genesis",
    )
    assert "started" in ack2, ack2
    for _ in range(100):
        cb = runtime.store.get_text("genesis", "/spawn_cb_pipe.md")
        if "cb-pipe" in cb:
            break
        await asyncio.sleep(0.1)
    assert "cb-pipe" in cb, cb


async def test_bash_tools_validation(runtime: MdioRuntime) -> None:
    await runtime.exec("@BASH", wt_uid="genesis")
    manifest = '''"""# TOOLS
## echo
- provider: cli
- ops: run
- argv: {"run": ["/bin/echo", "{text}"]}
- args: text
## cat_nostdin
- provider: cli
- ops: run
- argv: {"run": ["/bin/cat"]}
- stdin: false
""" | +> /.mds/bash.md::'''
    await runtime.exec(manifest, wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.tools.missing", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.tools.echo -op nope -args '{}'", wt_uid="genesis")
    assert "mdio_error" in await runtime.exec("@BASH.tools.echo -op run -args 'not-json'", wt_uid="genesis")
    assert "does not accept" in await runtime.exec('"data" | @BASH.tools.cat_nostdin -op run', wt_uid="genesis")


@requires_pty
async def test_bash_inspect_session_active_handle(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create activebox -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    ack = await runtime.exec(
        '@BASH.spawn -cmd "sleep 20" -sess activebox', wt_uid="genesis",
    )
    run_id = re.search(r"started ([0-9a-f]+)", ack).group(1)
    for _ in range(100):
        doc = await runtime.exec("@BASH.sess -inspect activebox", wt_uid="genesis")
        if f"active_handle: {run_id}" in doc:
            break
        await asyncio.sleep(0.05)
    assert f"active_handle: {run_id}" in doc, doc
    assert "state: IDLE" in doc or "state: RUNNING" in doc, doc
    service = runtime._workspace.get_plugin("BASH")._service
    await runtime.exec(f'@BASH.kill -id "{run_id}"', wt_uid="genesis")
    await _wait_until(lambda: run_id not in [k[1] for k in service._live], timeout=10)


def test_bash_process_owner_and_handle() -> None:
    owner = ProcessOwner()

    class DummyProcess:
        pid = os.getpid()

    owner.remember_process("wt", "r1", DummyProcess())
    assert "r1" in owner.processes["wt"]
    owner.mark_run_killed("wt", "r1")
    assert owner.was_run_killed("wt", "r1")
    started, identity = owner.started_for(os.getpid())
    assert started.os_pid == str(os.getpid())
    assert identity is not None
    owner.forget_process("wt", "r1")
    assert "r1" not in owner.processes.get("wt", {})
    assert not owner.was_run_killed("wt", "r1")

    handle = BashHandle("h1")
    handle.update({"state": "DONE", "stdout": "ok"})
    assert handle.get("state") == "DONE"
    assert handle["stdout"] == "ok"
    assert "state" in handle.keys()
    assert BashDisposeReport(outcomes=[BashDisposeOutcome("h1", confirmed=True)]).ok
    assert not BashDisposeReport(outcomes=[BashDisposeOutcome("h1", confirmed=False)]).ok


def test_bash_probe_edges() -> None:
    import subprocess
    probe = BashProcessProbe()
    assert probe.capture(None) is None
    assert probe.capture("not-a-pid") is None
    assert probe.probe(None).liveness == "unknown"
    assert probe.is_quiescent(None)
    proc = subprocess.Popen(["bash", "-lc", "sleep 30"], start_new_session=True)
    try:
        identity = probe.capture(proc.pid)
        assert identity is not None
        assert not probe.is_quiescent(identity)
        outcome = probe.terminate_tree("probe-edge", identity, grace_ms=100)
        assert outcome.confirmed, outcome
        proc.wait(timeout=5)
        assert probe.is_quiescent(identity)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5)


def _edit_args(args: dict) -> str:
    payload = json.dumps(args)
    return payload.replace("\\n", "\\u000a").replace("\\r", "\\u000d")


async def _edit_call(runtime: MdioRuntime, op: str, args: dict, sess: str | None = None) -> str:
    cmd = f"@BASH.tools.edit -op {op} -args '{_edit_args(args)}'"
    if sess:
        cmd += f" -sess {sess}"
    return await runtime.exec(cmd, wt_uid="genesis")


async def test_bash_tools_edit_default_manifest(runtime: MdioRuntime) -> None:
    await runtime.exec("@BASH", wt_uid="genesis")
    listing = await runtime.exec("@BASH.tools", wt_uid="genesis")
    assert "edit" in listing, listing
    help_res = await runtime.exec("@BASH.tools.edit", wt_uid="genesis")
    assert "python" in help_res, help_res
    assert "str_replace" in help_res, help_res
    assert "read before edit" in help_res, help_res


async def test_bash_tools_edit_create_view_edit_and_write(runtime: MdioRuntime, workdir: Path) -> None:
    path = workdir / "edit_demo.txt"
    path.unlink(missing_ok=True)
    created = await _edit_call(runtime, "create", {"path": str(path), "content": "alpha\nbeta\n"})
    assert "New file created" in created, created
    assert path.read_text(encoding="utf-8") == "alpha\nbeta\n"

    view = await _edit_call(runtime, "view", {"path": str(path)})
    assert "alpha" in view and "beta" in view, view
    assert "1  alpha" in view, view

    edited = await _edit_call(runtime, "edit", {
        "path": str(path), "old_string": "beta", "new_string": "gamma",
    })
    assert "updated successfully" in edited, edited
    assert path.read_text(encoding="utf-8") == "alpha\ngamma\n"

    inserted = await _edit_call(runtime, "insert", {
        "path": str(path), "insert_line": 1, "new_str": "inserted",
    })
    assert "edited successfully" in inserted, inserted
    assert path.read_text(encoding="utf-8") == "alpha\ninserted\ngamma\n"

    replaced = await _edit_call(runtime, "str_replace", {
        "path": str(path), "old_string": "gamma", "new_string": "delta",
    })
    assert "edited successfully" in replaced, replaced
    assert path.read_text(encoding="utf-8") == "alpha\ninserted\ndelta\n"

    rewritten = await _edit_call(runtime, "write", {"path": str(path), "content": "replaced"})
    assert "Updated file" in rewritten, rewritten
    assert path.read_text(encoding="utf-8") == "replaced"

    last = await runtime.exec("@BASH.inspect -last", wt_uid="genesis")
    assert "kind: tool" in last and "edit.write" in last, last


async def test_bash_tools_edit_guards_and_errors(runtime: MdioRuntime, workdir: Path) -> None:
    missing = workdir / "edit_missing.txt"
    missing.unlink(missing_ok=True)
    assert "FS_NOT_FOUND" in await _edit_call(runtime, "view", {"path": str(missing)})

    binary = workdir / "edit_binary.bin"
    binary.write_bytes(b"\x00\x01")
    assert "FS_NOT_TEXT" in await _edit_call(runtime, "view", {"path": str(binary)})

    unread = workdir / "edit_unread.txt"
    unread.write_text("hello\n", encoding="utf-8")
    assert "FS_NOT_OBSERVED" in await _edit_call(runtime, "edit", {
        "path": str(unread), "old_string": "hello", "new_string": "bye",
    })
    await _edit_call(runtime, "view", {"path": str(unread)})
    unread.write_text("changed externally\n", encoding="utf-8")
    assert "FS_STALE_VERSION" in await _edit_call(runtime, "edit", {
        "path": str(unread), "old_string": "changed", "new_string": "new",
    })

    ambiguous = workdir / "edit_ambiguous.txt"
    ambiguous.write_text("x x x\n", encoding="utf-8")
    await _edit_call(runtime, "view", {"path": str(ambiguous)})
    assert "FS_AMBIGUOUS_EDIT" in await _edit_call(runtime, "edit", {
        "path": str(ambiguous), "old_string": "x", "new_string": "y",
    })
    replaced = await _edit_call(runtime, "edit", {
        "path": str(ambiguous), "old_string": "x", "new_string": "y", "replace_all": True,
    })
    assert "All occurrences" in replaced, replaced
    assert ambiguous.read_text(encoding="utf-8") == "y y y\n"


async def test_bash_tools_edit_line_endings_and_view_range(runtime: MdioRuntime, workdir: Path) -> None:
    crlf = workdir / "edit_crlf.txt"
    crlf.write_bytes(b"one\r\ntwo\r\nthree\r\n")
    await _edit_call(runtime, "view", {"path": str(crlf)})
    await _edit_call(runtime, "edit", {
        "path": str(crlf), "old_string": "two", "new_string": "TWO",
    })
    assert crlf.read_bytes() == b"one\r\nTWO\r\nthree\r\n"

    ranged = await _edit_call(runtime, "view", {
        "path": str(crlf), "view_range": [2, 2],
    })
    assert "TWO" in ranged, ranged
    assert "one" not in ranged, ranged
    assert "three" not in ranged, ranged


async def test_bash_tools_edit_session_cwd_and_stdin_content(runtime: MdioRuntime, workdir: Path) -> None:
    await runtime.exec(
        f'@BASH.sess -create editcwd -cwd "{workdir.as_posix()}" -timeout "10"',
        wt_uid="genesis",
    )
    relative = workdir / "edit_relative.txt"
    relative.unlink(missing_ok=True)
    created = await _edit_call(runtime, "create", {
        "path": "edit_relative.txt", "content": "session",
    }, sess="editcwd")
    assert "New file created" in created, created
    assert relative.read_text(encoding="utf-8") == "session"

    stdin_path = workdir / "edit_stdin.txt"
    stdin_path.unlink(missing_ok=True)
    args = _edit_args({"path": str(stdin_path)})
    written = await runtime.exec(
        f'"""line1\nline2\n""" | @BASH.tools.edit -op write -args \'{args}\'',
        wt_uid="genesis",
    )
    assert "Created file" in written, written
    assert stdin_path.read_text(encoding="utf-8") == "line1\nline2\n"


async def run_all_tests() -> None:
    print("🧪 [TEST BASH] Launching @BASH tests...\n" + "=" * 60)

    with tempfile.TemporaryDirectory() as tmpdir:
        runtime = MdioRuntime(tmpdir)
        workdir = Path(tmpdir).resolve()

        await test_bash_run(runtime)
        await test_bash_run_with_cmd_and_stdin(runtime)
        await test_bash_run_with_cmd_and_empty_stdin(runtime)
        await test_bash_cwd_and_env(runtime, workdir)
        await test_bash_failure_and_timeout(runtime)
        await test_bash_stderr_capture(runtime)
        await test_bash_invalid_inputs(runtime)
        await test_bash_stream_and_kill(runtime)
        await test_bash_stream_observability(runtime)
        await test_bash_stream_timeout(runtime)
        await test_bash_stream_stdout_and_stderr(runtime)
        await test_bash_defaults_from_config(runtime)
        await test_bash_named_context_state_and_run(runtime, workdir)
        await test_bash_named_context_stream_restart_and_close(runtime, workdir)
        await test_bash_named_context_environment_persists(runtime, workdir)
        await test_bash_mdio_python_roundtrip(runtime)
        await test_bash_oneshot_heredoc_pipeline(runtime, workdir)
        await test_bash_named_context_agent_file_workflow(runtime, workdir)
        await test_bash_practical_tool_chains(runtime, workdir)
        await test_bash_spawn_and_cb(runtime)
        await test_bash_tools(runtime)
        await test_bash_dispose_wt(runtime, workdir)
        await test_bash_session_timeout_reset(runtime, workdir)
        await test_bash_spawn_failure_cb_and_kill(runtime)
        await test_bash_bq_output_safe(runtime)
        await test_bash_tools_allowlist(runtime)
        await test_bash_spawn_session(runtime, workdir)
        await test_bash_spawn_identity_and_liveness(runtime)
        await test_bash_stream_aclose_cleans_process_tree(runtime)
        await test_bash_pid_reuse_is_mismatch_and_never_killed()
        await test_bash_dispose_wt_strict_kills_spawn(runtime)
        await test_bash_context_stream_aclose_resets_session(runtime, workdir)
        await test_bash_probe_terminates_reparented_session_children()
        await test_bash_probe_term_trap_escalates_to_sigkill()
        await test_bash_context_proc_identity(runtime, workdir)
        await test_bash_inspect_last_follows_spawn(runtime)
        await test_bash_context_dispose_strict(runtime, workdir)

        print("\n🧪 ALL @BASH TESTS PASSED!")


if __name__ == "__main__":
    asyncio.run(run_all_tests())
