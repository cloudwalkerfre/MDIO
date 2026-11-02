"""MDIO CLI — 客户端入口。

CLI 不构造 runtime；所有命令在 mdio serve 之后走 HTTP 控制面。
"""
from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

import click


# ── HTTP helper（私有，不形成新抽象）──────────────────────────

def _gateway_host() -> str:
    return os.environ.get("MDIO_GATEWAY_HOST", "127.0.0.1")


def _base_url() -> str:
    port = os.environ.get("MDIO_GATEWAY_PORT", "8011")
    return f"http://{_gateway_host()}:{port}"


# 回环不穿代理。宿主（DSH 等）拉起 MCP 子进程时会按代理策略注入 http_proxy /
# no_proxy 等变量，urllib 默认读它们 ⇒ 连 127.0.0.1 也会被代理走。非回环（远程
# 网关）保持默认行为不动，否则远程场景反而会被这里改坏。
_LOOPBACK_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _opener_for(host: str):
    """返回一个与 urlopen 同签名的可调用对象。

    注意：OpenerDirector 本身不可调用，必须交回它的 .open —— 两者签名一致
    （open(fullurl, data=None, timeout=...)），这样调用点不必区分。
    """
    return _LOOPBACK_OPENER.open if host in ("127.0.0.1", "localhost", "::1") else urllib.request.urlopen


def _request_json(method: str, path: str, payload: dict | None = None,
                  timeout: float = 10.0) -> dict:
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(_base_url() + path, data=data,
                                 headers=headers, method=method)
    try:
        with _opener_for(_gateway_host())(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"server returned HTTP {e.code}") from e
    return json.loads(body) if body.strip() else {}


def _get_json(path: str, timeout: float = 10.0) -> dict:
    return _request_json("GET", path, timeout=timeout)


def _post_json(path: str, payload: dict | None = None, timeout: float = 10.0) -> dict:
    return _request_json("POST", path, payload, timeout=timeout)


def _post_worktree(method: str, params: dict, timeout: float = 30.0) -> dict:
    result = _post_json(
        "/worktree",
        {"id": 1, "method": f"worktree.{method}", "params": params},
        timeout=timeout,
    )
    if not result.get("ok"):
        message = (result.get("error") or {}).get("message") or "worktree operation failed"
        raise RuntimeError(message)
    return result.get("result") or {}


def _probe_status() -> dict | None:
    """None = 未启动；dict = 已启动 / 端口被其它进程占用。"""
    try:
        return _get_json("/control/status", timeout=1.5)
    except RuntimeError:
        return {"_probe_error": "non-mdio"}
    except urllib.error.URLError as e:
        if isinstance(e.reason, ConnectionRefusedError):
            return None
        raise
    except Exception:
        return {"_probe_error": "unknown"}


def _get_status_with_retry(timeout: float = 2.0) -> dict:
    """Read status with bounded retries for the gateway boot window.

    A fixed sleep is not enough (and makes every status call slow).
    Retry only transient connection failures; ``ConnectionRefusedError``
    means the process is not listening yet / not running and is reported
    immediately by the caller.
    """
    last: Exception | None = None
    for delay in (0.0, 0.2, 0.5, 1.0):
        if delay:
            time.sleep(delay)
        try:
            return _get_json("/control/status", timeout=timeout)
        except urllib.error.URLError as e:
            if isinstance(e.reason, ConnectionRefusedError):
                raise
            last = e
        except Exception as e:
            last = e
    if last is not None:
        raise last
    raise RuntimeError("gateway status unavailable")


def _not_started() -> None:
    click.echo("- mdio_error: MDIO not started. Run: mdio serve")


_GITHUB_REPO_RE = re.compile(
    r"^https?://github\.com/(?P<owner>[^/]+)/(?P<repo>[^/]+?)(?:\.git)?/?$"
)


def _source_kind(source: str) -> tuple[str, str, str]:
    """Return (kind, name, location): local/github/archive."""
    text = str(source or "").strip()
    if not text:
        raise click.ClickException("source is required")
    parsed = urllib.parse.urlparse(text)
    if parsed.scheme in ("http", "https"):
        match = _GITHUB_REPO_RE.match(text)
        if match:
            owner = match.group("owner")
            repo = match.group("repo")
            return "github", repo, f"https://github.com/{owner}/{repo}"
        if parsed.path.endswith((".tar.gz", ".tgz", ".zip")):
            name = Path(parsed.path).name.split(".")[0]
            return "archive", name, text
        raise click.ClickException(f"unsupported URL: {text}")
    path = Path(text).expanduser()
    if not path.is_dir():
        raise click.ClickException(f"local worktree directory not found: {path}")
    return "local", path.name, str(path)


def _archive_url(location: str, ref: str | None) -> str:
    revision = str(ref or "HEAD").strip() or "HEAD"
    return f"{location}/archive/{revision}.tar.gz"


def _extract_archive(archive: Path, target: Path) -> None:
    """Extract one archive safely, stripping a single top-level directory."""
    staging = Path(tempfile.mkdtemp(prefix="mdio-load-"))
    try:
        if archive.name.endswith(".zip"):
            with zipfile.ZipFile(archive) as zf:
                for info in zf.infolist():
                    name = info.filename
                    parts = Path(name).parts
                    if name.startswith("/") or ".." in parts:
                        raise click.ClickException(f"unsafe archive member: {name}")
                zf.extractall(staging)
        else:
            with tarfile.open(archive, "r:*") as tf:
                tf.extractall(staging, filter="data")

        entries = list(staging.iterdir())
        root = entries[0] if len(entries) == 1 and entries[0].is_dir() else staging
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            raise click.ClickException(f"destination already exists: {target}")
        if root == staging:
            target.mkdir(parents=True)
            for child in entries:
                shutil.move(str(child), str(target / child.name))
        else:
            shutil.move(str(root), str(target))
    finally:
        shutil.rmtree(staging, ignore_errors=True)
        try:
            archive.unlink()
        except OSError:
            pass


def _download(url: str, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=".mdio-load-", dir=str(target.parent))
    os.close(fd)
    tmp_path = Path(tmp_name)
    try:
        with urllib.request.urlopen(url, timeout=60) as response, open(tmp_path, "wb") as out:
            shutil.copyfileobj(response, out)
        _extract_archive(tmp_path, target)
    except Exception:
        try:
            tmp_path.unlink()
        except OSError:
            pass
        raise


def _pick_wt_root(base: Path, subdir: str) -> Path:
    if subdir:
        candidate = base / subdir
        if candidate.is_dir():
            return candidate
        raise click.ClickException(f"worktree subdir not found: {candidate}")
    if (base / "genesis" / "wt").is_dir():
        return base / "genesis"
    if (base / "wt").is_dir():
        return base
    children = [p for p in base.iterdir() if p.is_dir() and (p / "wt").is_dir()]
    if len(children) == 1:
        return children[0]
    raise click.ClickException(f"cannot determine worktree directory under {base}")


# ── exec ─────────────────────────────────────────────────────

@click.command("exec")
@click.argument("pipeline")
@click.pass_context
def exec_cmd(ctx, pipeline):
    """执行 MDIO 管道 DSL（发送到运行中的 runtime）。"""
    if not sys.stdin.isatty():
        stdin_data = sys.stdin.read().strip()
        if stdin_data:
            pipeline = f'"{stdin_data}" | {pipeline}'

    try:
        result = _post_json("/exec", {
            "code": pipeline,
            "active_doc": "",
            "wt_uid": ctx.obj.get("wt", "genesis"),
        })
    except (RuntimeError, urllib.error.URLError) as e:
        if isinstance(e, urllib.error.URLError) and isinstance(e.reason, ConnectionRefusedError):
            _not_started()
            return
        click.echo(f"- mdio_error: {e}")
        return

    if result.get("status") == "error":
        click.echo(f"- mdio_error: {result.get('result', 'unknown error')}")
    else:
        click.echo(result.get("result", ""))


# ── wt 级：load / offload；folder 级：mount / unmount / share ──

@click.command("load")
@click.argument("source")
@click.option("--pwt", default="genesis", help="父 worktree 名称/uid；仅单 wt 时覆盖 manifest parent")
@click.option("--rule", default="", help="可选子侧挂载规则")
@click.option("--dest", default=None, help="下载目标目录，缺省 <mdio home>/<repo>")
@click.option("--ref", default=None, help="GitHub ref，缺省 HEAD")
@click.option("--subdir", default="", help="容器内 worktree/WTS 子目录，缺省自动识别")
@click.option("--all", "load_all", is_flag=True, help="加载 install.md 中全部 wt（默认行为）")
@click.option("--wt", "wt_name", default=None, help="只加载 install.md 中的指定 wt")
@click.option("--list", "list_only", is_flag=True, help="只列出 WTS 内可加载的 wt，不注册")
@click.option("--dry-run", is_flag=True, help="只解析并打印，不下载、不启动、不注册")
def load_cmd(source, pwt, rule, dest, ref, subdir, load_all, wt_name, list_only, dry_run):
    """从本地目录或 GitHub repo 加载 WTS/worktree；多 wt WTS 默认全部加载。"""
    from mdio_py.cli.wts_manifest import load_install_md

    if load_all and wt_name:
        raise click.UsageError("--all and --wt are mutually exclusive")

    kind, name, location = _source_kind(source)
    archive = None
    if kind == "local":
        if dest:
            raise click.ClickException("--dest is only valid for remote sources")
        root = Path(location)
        if subdir:
            root = root / subdir
        if not root.is_dir():
            raise click.ClickException(f"WTS directory not found: {root}")
        target_dir = root
    else:
        from mdio_py._env import mdio_home

        dest_path = Path(dest).expanduser() if dest else mdio_home() / name
        archive = _archive_url(location, ref) if kind == "github" else location
        if dry_run and not dest_path.exists():
            expected = dest_path / subdir if subdir else dest_path / "genesis"
            click.echo(f"source:  {location}")
            click.echo(f"archive: {archive}")
            click.echo(f"target:  {dest_path}")
            click.echo(f"wt:      {expected}")
            click.echo(f"parent:  {pwt}")
            click.echo(f"exists:  {str(dest_path.exists()).lower()}")
            return
        if not dest_path.exists():
            click.echo(f"downloading {archive}")
            _download(archive, dest_path)
        root = dest_path / subdir if subdir else dest_path
        target_dir = root

    specs = load_install_md(root / "install.md")
    root_spec = None
    root_path = None
    if specs is not None:
        root_spec = next((spec for spec in specs if spec.root), None)
        if root_spec is not None:
            root_path = root / root_spec.path
            if not root_path.is_dir():
                raise click.ClickException(f"root worktree path not found: {root_path}")
        if list_only:
            for spec in specs:
                flags = " required" if spec.required else ""
                flags += " root" if spec.root else ""
                click.echo(f"{spec.name:16s} path={spec.path} parent={spec.parent}{flags}")
            return
        if wt_name:
            specs = [spec for spec in specs if spec.name == wt_name]
            if not specs:
                raise click.ClickException(f"wt not found in install.md: {wt_name}")
        if not wt_name and pwt != "genesis":
            raise click.UsageError("--pwt is only valid with --wt for a WTS manifest")
        selected = []
        for spec in specs:
            wt_root = root / spec.path
            if not wt_root.is_dir():
                raise click.ClickException(f"worktree path not found: {wt_root}")
            parent = pwt if wt_name and pwt != "genesis" else spec.parent
            selected.append((spec, wt_root, parent))
        if dry_run:
            click.echo(f"source:  {root}")
            if root_spec is not None:
                click.echo(f"root:    {root_spec.name} -> {root_path}")
            for spec, wt_root, parent in selected:
                if not spec.root:
                    click.echo(f"wt:      {spec.name} -> {wt_root} (parent {parent})")
            return
    else:
        if wt_name:
            raise click.ClickException(
                f"{root} has no install.md; cannot select --wt {wt_name!r}")
        try:
            wt_root = _pick_wt_root(root, "")
        except click.ClickException:
            # Backward compatibility: old local load accepted a bare directory.
            # Only reject when the directory actually looks like a multi-wt WTS.
            if any(child.is_dir() and (child / "wt").is_dir() for child in root.iterdir()):
                raise
            wt_root = root
        if list_only:
            click.echo(f"legacy: {wt_root}")
            return
        if dry_run:
            click.echo(f"source:  {root}")
            click.echo(f"target:  {wt_root}")
            click.echo(f"parent:  {pwt}")
            return
        selected = [(None, wt_root, pwt)]

    host = _gateway_host()
    port = int(os.environ.get("MDIO_GATEWAY_PORT", "8011"))
    try:
        _ensure_gateway(host, port, mode="control", root=str(root_path) if root_path else None)
    except RuntimeError as e:
        click.echo(f"- mdio_error: {e}")
        raise click.exceptions.Exit(1)

    if root_path is not None:
        try:
            actual_root = _get_json("/control/status", timeout=2).get("root")
        except Exception:
            actual_root = None
        if actual_root and Path(actual_root).resolve() != root_path.resolve():
            click.echo(
                f"- mdio_error: gateway already running with root {actual_root}; "
                f"stop it before loading WTS root {root_path}"
            )
            raise click.exceptions.Exit(1)

    for spec, wt_root, parent in selected:
        if spec is not None and spec.root:
            click.echo(f"worktree: root '{wt_root}'")
            continue
        wt_label = spec.name if spec is not None else name
        try:
            result = _post_worktree("load", {
                "path": str(wt_root),
                "rule": rule,
                "parent": parent,
            })
        except (RuntimeError, urllib.error.URLError) as e:
            click.echo(f"- mdio_error: {e}")
            raise click.exceptions.Exit(1)
        state = "new" if result.get("is_new") else "already loaded"
        click.echo(
            f"worktree: {wt_label} '{wt_root}' ({state}, wt_uid {result.get('uid')})"
        )

@click.command("mount")
@click.option("--ticket", "-t", required=True, help="iroh:// 或 iroh-share:// ticket（folder 级：远程 folder → 挂进本地 wt）")
@click.option("--as", "as_name", default=None, help="挂载点名字（缺省 remote）")
@click.option("--wt", "--target", "target", default="genesis", help="目标 WT（缺省 genesis）")
def mount_cmd(ticket, as_name, target):
    """挂载一个远程 folder（iroh share）。"""
    try:
        result = _post_json("/gateway/mount", {
            "ticket": ticket,
            "name": as_name or "remote",
            "target": target,
        }, timeout=60)
    except (RuntimeError, urllib.error.URLError) as e:
        click.echo(f"- mdio_error: {e}")
        return
    if "error" in result:
        click.echo(f"- mdio_error: {result['error']}")
    else:
        click.echo(f"folder: mounted (wt_uid {result.get('wt_uid', '?')}, files {result.get('files', 0)})")


@click.command("unmount")
@click.option("--path", "-p", required=True, help="要断开的远程 folder 路径（/share/<name>）")
@click.option("--wt", "--name", "-n", "name", default=None, help="目标 WT（缺省=服务端 active WT）")
def unmount_cmd(path, name):
    """断开一个远程 folder（文件保留为本地）。"""
    try:
        result = _post_json("/gateway/unmount", {"path": path, "name": name})
    except (RuntimeError, urllib.error.URLError) as e:
        click.echo(f"- mdio_error: {e}")
        return
    if "error" in result:
        click.echo(f"- mdio_error: {result['error']}")
    else:
        click.echo(f"folder: unmounted {result.get('path')} from {result.get('wt_uid')}")


@click.command("share")
@click.option("--wt", required=True, help="目标 WT uid")
def share_cmd(wt):
    """生成 iroh share ticket。"""
    try:
        result = _post_json(f"/gateway/{wt}/share", {})
    except (RuntimeError, urllib.error.URLError) as e:
        click.echo(f"- mdio_error: {e}")
        return
    if "error" in result:
        click.echo(f"- mdio_error: {result['error']}")
    else:
        click.echo(result.get("ticket", ""))


@click.command("offload")
@click.argument("wt_name", required=False)
@click.option("--id", "wt_id", default="", help="按 wt uid 卸载")
@click.option("--path", "wt_path", default="", help="按本地项目根卸载")
def offload_cmd(wt_name, wt_id, wt_path):
    """卸载一个 worktree；name、--id、--path 三选一。"""
    selectors = [str(wt_name or "").strip(), str(wt_id or "").strip(),
                 os.path.expanduser(str(wt_path or "").strip())]
    chosen = [value for value in selectors if value]
    if len(chosen) != 1:
        raise click.UsageError("offload requires exactly one of WT_NAME, --id, --path")
    if str(wt_name or "").strip():
        payload = {"wt": str(wt_name).strip()}
        target = str(wt_name).strip()
    elif str(wt_id or "").strip():
        payload = {"id": str(wt_id).strip()}
        target = str(wt_id).strip()
    else:
        payload = {"path": os.path.expanduser(str(wt_path).strip())}
        target = payload["path"]
    try:
        result = _post_worktree("offload", payload)
    except (RuntimeError, urllib.error.URLError) as e:
        if isinstance(e, urllib.error.URLError) and isinstance(e.reason, ConnectionRefusedError):
            _not_started()
        else:
            click.echo(f"- mdio_error: {e}")
        raise click.exceptions.Exit(1)
    click.echo(f"worktree: offloaded '{target}' (wt_uid {result.get('uid')})")


# ── repl ─────────────────────────────────────────────────────

@click.command()
@click.option("--standalone", is_flag=True, help="使用本地临时 runtime，不连 server")
def repl(standalone):
    """启动交互式 MDIO REPL（默认连接运行中 server）。"""
    import asyncio as _asyncio
    from mdio_py._repl import _main_async

    _asyncio.run(_main_async(force_repl=True, standalone=standalone))


# ── 进程启动 / 停止 / 状态 ────────────────────────────────────

def _launch_granian(*, port: int, host: str, mode: str, root: str | None = None,
                    detach: bool = False) -> None:
    """启动长驻 runtime。

    detach=False（默认）：前台阻塞，供 `mdio serve` 使用。
    detach=True：后台常驻并立即返回，供 `mdio mcp` 的「没跑就起一个」使用；
                 此时 stdout/stderr 落到 <mdio home>/gateway.log —— 否则网关的
                 报错会进 /dev/null。
    """
    # 运行时根（mdio 的家）：唯一实现见 mdio_py._env.runtime_root()
    from mdio_py._env import ensure_runtime_root, mdio_home, runtime_root

    ensure_runtime_root()
    _root_dir = runtime_root()

    env = os.environ.copy()
    env["MDIO_PROCESS_MODE"] = mode
    env["MDIO_GATEWAY_HOST"] = host
    env["MDIO_GATEWAY_PORT"] = str(port)
    if root is not None:
        env["MDIO_VFS_MOUNTS"] = root
        env["MDIO_WORKSPACE_ROOT"] = root

    cmd = [
        sys.executable,
        "-m", "granian",
        "--interface", "asgi",
        "--host", host, "--port", str(port),
        "--workers", "1", "--loop", "uvloop",
        "mdio_py.gateway:app",
    ]
    if detach:
        log_path = mdio_home() / "gateway.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with open(log_path, "ab", buffering=0) as log:
            subprocess.Popen(
                cmd, cwd=_root_dir, env=env,
                stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                start_new_session=True,
            )
        return
    subprocess.run(cmd, cwd=_root_dir, env=env)


@click.command("serve")
@click.option("--port", "-p", type=int, default=None, help="监听端口")
@click.option("--host", "-h", default=None, help="监听地址")
@click.option("--root", default=None, help="覆盖 workspace root")
def serve(port, host, root):
    """启动 MDIO Gateway（API/WS/MCP/UI）；前台常驻，不自动打开浏览器。"""
    host = host or os.environ.get("MDIO_GATEWAY_HOST", "127.0.0.1")
    port = port or int(os.environ.get("MDIO_GATEWAY_PORT", "8011"))
    os.environ["MDIO_GATEWAY_HOST"] = host
    os.environ["MDIO_GATEWAY_PORT"] = str(port)
    probe = _probe_status()
    if probe is not None:
        if probe.get("_probe_error") == "non-mdio":
            click.echo("- mdio_error: port is occupied by a non-MDIO process")
        else:
            click.echo(f"already running (pid {probe.get('pid', '?')})")
        return
    _launch_granian(port=port, host=host, mode="gateway", root=root)


@click.command("web")
@click.option("--port", "-p", type=int, default=None, help="监听端口")
@click.option("--host", "-h", default=None, help="监听地址")
@click.option("--root", default=None, help="覆盖 workspace root")
@click.option("--no-open", is_flag=True, help="只确保服务运行并打印 URL，不打开浏览器")
def web_cmd(port, host, root, no_open):
    """确保 gateway/UI 运行，并在系统浏览器打开 Web UI。"""
    host = host or os.environ.get("MDIO_GATEWAY_HOST", "127.0.0.1")
    port = port or int(os.environ.get("MDIO_GATEWAY_PORT", "8011"))
    os.environ["MDIO_GATEWAY_HOST"] = host
    os.environ["MDIO_GATEWAY_PORT"] = str(port)
    try:
        _ensure_gateway(host, port, root=root)
    except RuntimeError as e:
        click.echo(f"- mdio_error: {e}", err=True)
        raise click.exceptions.Exit(1)
    try:
        status = _get_json("/control/status", timeout=2)
        if status.get("gateway_enabled") is False:
            _post_json("/control/gateway/on", {})
    except Exception as e:
        click.echo(f"- mdio_error: failed to enable gateway/UI: {e}", err=True)
        raise click.exceptions.Exit(1)
    browser_host = "127.0.0.1" if host in ("0.0.0.0", "::", "[::]") else host
    url = f"http://{browser_host}:{port}/?wt=genesis"
    click.echo(f"MDIO Web: {url}")
    if no_open:
        return
    try:
        import webbrowser
        opened = webbrowser.open(url)
    except Exception:
        opened = False
    if not opened:
        click.echo("  browser not opened; open the URL manually", err=True)


@click.command("status")
def status_cmd():
    """查看运行中 runtime 状态。"""
    try:
        result = _get_status_with_retry()
    except urllib.error.URLError as e:
        if isinstance(e.reason, ConnectionRefusedError):
            click.echo("not running")
            return
        click.echo(f"- mdio_error: gateway not ready: {e}")
        return
    except Exception as e:
        click.echo(f"- mdio_error: gateway not ready: {e}")
        return

    click.echo(f"running: {result.get('running')}")
    click.echo(f"pid: {result.get('pid')}")
    click.echo(f"mode: {result.get('mode')}")
    click.echo(f"root: {result.get('root')}")
    click.echo(f"host: {result.get('host')}")
    click.echo(f"port: {result.get('port')}")
    click.echo(f"gateway_enabled: {result.get('gateway_enabled')}")
    try:
        gw = _get_json("/control/gateway/status", timeout=2)
    except Exception:
        gw = None
    if gw is not None:
        click.echo(f"gateway_actives: {gw.get('actives')}")
        click.echo(f"gateway_scopes: {gw.get('scopes')}")
    click.echo(f"wts: {', '.join(result.get('wts', []))}")


def _wait_process(pid: int, force: bool) -> None:
    deadline = time.time() + (3.0 if force else 10.0)
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            click.echo("stopped")
            return
        except PermissionError:
            pass
        time.sleep(0.1)
    if force:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        click.echo("force stopped")
    else:
        click.echo("- mdio_error: process did not exit in time; retry with --force")


@click.command("stop")
@click.option("--force", is_flag=True, help="超时后 SIGKILL")
def stop_cmd(force):
    """停止运行中的 runtime 进程。"""
    try:
        status = _get_json("/control/status", timeout=3)
    except urllib.error.URLError as e:
        if isinstance(e.reason, ConnectionRefusedError):
            click.echo("not running")
            return
        click.echo(f"- mdio_error: {e}")
        return
    except Exception as e:
        click.echo(f"- mdio_error: {e}")
        return

    pid = status.get("pid")
    if not isinstance(pid, int):
        click.echo("- mdio_error: cannot determine server pid")
        return

    try:
        _post_json("/control/stop", {}, timeout=5)
    except Exception as e:
        click.echo(f"- mdio_error: stop request failed: {e}")
        return

    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        click.echo("stopped")
        return
    except PermissionError as e:
        click.echo(f"- mdio_error: cannot signal {pid}: {e}")
        return

    _wait_process(pid, force)


# ── mcp：stdio ↔ http 桥 ──────────────────────────────────────
#
# 为什么需要它：MCP 只把「客户端拥有进程」这一件事交给 stdio transport；http
# transport 在规范里就是「连一个 URL」，没有启动命令的位置（dsh-mcp-client 的
# streamable-http 变体也确实没有 command 字段）。所以「已经在跑就复用、没跑就起
# 一个」这条语义只能住在 stdio 行的 command 位里 —— 本命令就是住在那里的进程。
#
# 分工：只做「确保网关 + 逐行转发」，自己不持运行时。运行时必须由单网关持有，
# 否则每个 MCP 客户端各起一份，就等于把 mdio 的共享工作区拆成互不可见的几份。

def _probe_ready() -> bool:
    """网关是否已在跑。端口被非 mdio 进程占用时抛 RuntimeError。"""
    probe = _probe_status()
    if probe is None:
        return False
    if probe.get("_probe_error") == "non-mdio":
        raise RuntimeError("port is occupied by a non-MDIO process")
    return not probe.get("_probe_error")


def _ensure_gateway(host: str, port: int, wait_s: float = 30.0,
                    mode: str = "gateway", root: str | None = None) -> None:
    """没跑就 detach 起一个，已在跑直接复用，然后等到就绪。

    探针与 `mdio serve` 完全同一套（/control/status）；差别只在「没跑」时
    serve 是前台阻塞，这里是后台常驻并等就绪。mode=control 供 load 使用。
    """
    os.environ["MDIO_GATEWAY_HOST"] = host
    os.environ["MDIO_GATEWAY_PORT"] = str(port)
    # 探针有瞬时误判的可能，而 granian 允许 SO_REUSEPORT ⇒ 起重复实例不会报错，
    # 只会静默把流量分走（实测：两个网关可同占同一端口、各自的 workspace root 不一样）。
    # 所以「没跑」要连续确认几次才认。
    for attempt in range(3):
        if _probe_ready():
            return
        if attempt < 2:
            time.sleep(0.5)
    _launch_granian(port=port, host=host, mode=mode, root=root, detach=True)
    deadline = time.monotonic() + wait_s
    while time.monotonic() < deadline:
        time.sleep(0.5)
        if _probe_ready():
            return
    raise RuntimeError(f"gateway did not become ready within {wait_s:.0f}s (port {port})")


def _forward_stdin(url: str, timeout: float = 300.0) -> None:
    """逐行读 stdin 的 JSON-RPC，POST 到 url，响应写回 stdout。

    stdout 只允许出现 JSON-RPC（协议要求）；诊断一律走 stderr。
    """
    out = sys.stdout
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            print(f"mdio mcp: non-JSON line skipped ({len(line)} bytes)", file=sys.stderr)
            continue
        req = urllib.request.Request(
            url, data=line.encode("utf-8"),
            headers={"Content-Type": "application/json"}, method="POST",
        )
        try:
            with _opener_for("127.0.0.1")(req, timeout=timeout) as resp:
                body = resp.read()
        except urllib.error.HTTPError as e:
            body = e.read()
        except Exception as e:  # 单个请求失败不能掀掉整条会话
            err = {"jsonrpc": "2.0", "id": msg.get("id"),
                   "error": {"code": -32603, "message": f"mdio mcp bridge: {e}"}}
            out.write(json.dumps(err) + "\n")
            out.flush()
            continue
        if body:  # 通知被网关回 204（无 body）⇒ 什么都不写
            out.write(body.decode("utf-8", "replace").rstrip("\n") + "\n")
            out.flush()


@click.command("mcp")
@click.option("--port", "-p", type=int, default=None,
              help="网关端口（默认读 MDIO_GATEWAY_PORT，再取 8011）")
@click.option("--host", "-h", default=None, help="网关地址")
@click.option("--no-ensure", is_flag=True, help="不自动拉起网关，只做转发")
def mcp_cmd(port, host, no_ensure):
    """stdio MCP 桥：确保网关在跑，再把 stdin 的 JSON-RPC 转发到 /mcp。

    供 MCP 客户端（如 DSH）以 stdio 方式拉起用：已在跑就复用，没跑就 detach 起
    一个，然后逐行转发。协议层与 /mcp 端点完全一致（含 2026-07-28 无状态世代），
    本命令不改写任何内容。

    \b
    客户端配置示例（dsh-mcp-client）:
      transport: stdio
      command: ~/.local/bin/mdio
      args: [mcp]
    """
    host = host or _gateway_host()
    port = port or int(os.environ.get("MDIO_GATEWAY_PORT", "8011"))
    url = os.environ.get("MDIO_MCP_URL") or f"http://{host}:{port}/mcp"

    if not no_ensure:
        try:
            _ensure_gateway(host, port)
        except RuntimeError as e:
            print(f"mdio mcp: {e}", file=sys.stderr)
            raise SystemExit(1)

    _forward_stdin(url)
