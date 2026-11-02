from __future__ import annotations

import json
import re
from typing import Any

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.ast_utils import bullets_to_kv
from mdio_py.core.text_utils import unquote_triple, first_stripped, parse_kv_lines
from mdio_py.core.selector_builder import SelectorBuilder, by_tag, drill

from .models import BashConfig, BashResult


_DEFAULT_TOOLS = """
# TOOLS
## edit
- provider: python
- module: mdio_py.plugins.bash.tools.dsh_edit
- entry: run
- ops: view, create, str_replace, edit, insert, write
- help: DSH-compatible host editor: view/create/edit/str_replace/insert/write; read before edit.
- args: path, content, old_string, new_string, replace_all, insert_line, view_range
"""

def _bq_lines(text: str) -> str:
    body = (text or "").strip("\n")
    if not body:
        return ""
    return "\n".join(("  > " + line) if line else "  >" for line in body.split("\n"))


def _unquote(val: Any) -> str:
    return unquote_triple(val)   # 值清洗：成对剥三引号/双/单引号，不解转义


class BashState:
    def __init__(self, plugin):
        self._plugin = plugin

    # ── init ──────────────────────────────────────────────

    async def ensure_docs(self, workspace, wt_uid: str) -> None:
        try:
            if not workspace.path_exists(wt_uid, "/.mds"):
                workspace.mk(wt_uid, "/.mds", parents=True, verbose=False)
            if not workspace.path_exists(wt_uid, "/var"):
                workspace.mk(wt_uid, "/var", parents=True, verbose=False)
            if not workspace.path_exists(wt_uid, "/.mds/bash.md"):
                workspace.mk(wt_uid, "/.mds/bash.md", parents=False, verbose=False)
                workspace.write_locator(
                    wt_uid,
                    "/.mds/bash.md",
                    SelectorBuilder().build(),
                    "+>",
                    (
                        "# CONFIG\n"
                        "- backend: local\n"
                        "- shell: bash\n"
                        "- timeout: 30\n"
                        "- session_engine: pexpect\n"
                        "- stream_chunk: 4096\n"
                        "- output_limit: 65536\n"
                        "\n# CONTEXTS\n"
                    ),
                    auto_scaffold=False,
                    god=True,
                )
            if not workspace.path_exists(wt_uid, "/var/bash.md"):
                workspace.mk(wt_uid, "/var/bash.md", parents=False, verbose=False)
                workspace.write_locator(
                    wt_uid,
                    "/var/bash.md",
                    SelectorBuilder().build(),
                    "+>",
                    "# EXEC\n\n# CONTEXTS\n",
                    auto_scaffold=False,
                    god=True,
                )
            text = workspace.read_full_text(wt_uid, "/.mds/bash.md")
            if "# TOOLS" not in text:
                workspace.write_locator(
                    wt_uid,
                    "/.mds/bash.md",
                    SelectorBuilder().build(),
                    "+>",
                    "\n" + _DEFAULT_TOOLS,
                    auto_scaffold=False,
                    god=True,
                )
        except Exception:
            pass

    # ── config load ───────────────────────────────────────

    async def load_global_config(self, workspace, ctx: RuntimeContext) -> BashConfig:
        config = BashConfig()
        try:
            chunks = workspace.read_section_chunks(ctx.wt_uid, "/.mds/bash.md", "CONFIG")
        except Exception:
            chunks = []

        data = self._parse_kv_chunks(chunks, workspace=workspace)
        config.backend = data.get("backend", config.backend)
        config.shell = data.get("shell", config.shell)
        config.timeout = self.parse_int(data.get("timeout"), config.timeout)
        config.session_engine = data.get("session_engine", config.session_engine)
        config.stream_chunk = self.parse_int(data.get("stream_chunk"), config.stream_chunk)
        config.output_limit = self.parse_int(data.get("output_limit"), config.output_limit)
        return config

    async def load_context_config(self, workspace, ctx: RuntimeContext, name: str) -> BashConfig | None:
        try:
            chunks = workspace.read_section_chunks(ctx.wt_uid, "/.mds/bash.md", f"CONTEXTS.{name}")
        except Exception:
            chunks = []
        if not chunks:
            return None
        base = await self.load_global_config(workspace, ctx)
        data = self._parse_kv_chunks(chunks, workspace=workspace)
        base.shell = data.get("shell", base.shell)
        base.timeout = self.parse_int(data.get("timeout"), base.timeout)
        base.cwd = data.get("cwd", base.cwd)
        base.session_engine = data.get("engine", data.get("session_engine", base.session_engine))
        if "env" in data:
            base.env = self._parse_env_json(data["env"])
        return base

    async def load_effective_context_config(self, workspace, ctx: RuntimeContext, name: str) -> BashConfig:
        config = await self.load_context_config(workspace, ctx, name)
        if config is not None:
            return config
        return await self.load_global_config(workspace, ctx)

    # ── proc update ─────────────────────────────────────

    async def update_run_proc(
        self, workspace, ctx: RuntimeContext, run_id: str, fields: dict[str, Any],
        *, suppress_hooks: bool = False,
    ) -> None:
        for key, value in fields.items():
            workspace.write_locator(
                ctx.wt_uid,
                ctx.proc_path,
                by_tag(f"PROCESS_TABLE.BASH.{run_id}", key),
                "/>",
                str(value if value is not None else ""),
                auto_scaffold=True,
                suppress_hooks=suppress_hooks,
            )

    async def update_context_proc(
        self, workspace, ctx: RuntimeContext, name: str, fields: dict[str, Any],
        *, suppress_hooks: bool = False,
    ) -> None:
        for key, value in fields.items():
            workspace.write_locator(
                ctx.wt_uid,
                ctx.proc_path,
                by_tag(f"PROCESS_TABLE.BASH.{name}", key),
                "/>",
                str(value if value is not None else ""),
                auto_scaffold=True,
                suppress_hooks=suppress_hooks,
            )

    # ── context var write ─────────────────────────────────

    async def ensure_context_config_head(self, workspace, wt_uid: str, name: str) -> None:
        try:
            chunks = workspace.read_section_chunks(wt_uid, "/.mds/bash.md", f"CONTEXTS.{name}")
        except Exception:
            chunks = []
        if chunks:
            return
        workspace.write_locator(
            wt_uid,
            "/.mds/bash.md",
            drill("CONTEXTS"),
            "+>",
            f"\n## {name}\n",
            auto_scaffold=True,
        )

    async def ensure_context_var_head(self, workspace, wt_uid: str, name: str) -> None:
        try:
            chunks = workspace.read_section_chunks(wt_uid, "/var/bash.md", f"CONTEXTS.{name}")
        except Exception:
            chunks = []
        if chunks:
            return
        workspace.write_locator(
            wt_uid,
            "/var/bash.md",
            drill("CONTEXTS"),
            "+>",
            f"## {name}",
            auto_scaffold=True,
        )

    async def write_context_root(self, workspace, wt_uid: str, name: str, fields: dict[str, Any]) -> None:
        await self.ensure_context_var_head(workspace, wt_uid, name)
        for key, value in fields.items():
            workspace.write_locator(
                wt_uid,
                "/var/bash.md",
                by_tag(f"CONTEXTS.{name}", key),
                "/>",
                str(value if value is not None else ""),
                auto_scaffold=True,
            )

    async def append_context_cmd(self, workspace, wt_uid: str, name: str, cmd_id: str) -> None:
        await self.ensure_context_var_head(workspace, wt_uid, name)
        workspace.write_locator(
            wt_uid,
            "/var/bash.md",
            drill(f"CONTEXTS.{name}"),
            "+>",
            f"### {cmd_id}",
            auto_scaffold=True,
        )

    async def write_context_cmd_fields(
        self, workspace, wt_uid: str, name: str, cmd_id: str, fields: dict[str, Any],
    ) -> None:
        for key, value in fields.items():
            workspace.write_locator(
                wt_uid,
                "/var/bash.md",
                by_tag(f"CONTEXTS.{name}.{cmd_id}", key),
                "/>",
                str(value if value is not None else ""),
                auto_scaffold=True,
            )

    async def append_context_event(self, workspace, wt_uid: str, name: str, event_id: str, fields: dict[str, Any]) -> None:
        await self.ensure_context_var_head(workspace, wt_uid, name)
        workspace.write_locator(
            wt_uid,
            "/var/bash.md",
            drill(f"CONTEXTS.{name}"),
            "+>",
            f"### {event_id}",
            auto_scaffold=True,
        )
        await self.write_context_cmd_fields(workspace, wt_uid, name, event_id, fields)

    # ── terminal records (bq) ─────────────────────────────

    async def append_exec_record(
        self, workspace, wt_uid: str, run_id: str,
        fields: dict[str, Any], stdout: str = "", stderr: str = "",
    ) -> None:
        lines = [f"## {run_id}"]
        cb_code = fields.pop("cb", None)
        for key, value in fields.items():
            if value is None:
                continue
            lines.append(f"- {key}: {str(value).replace(chr(10), ' ')}")
        if cb_code:
            lines.extend(["### CB", "- cb:"])
            enc = _bq_lines(str(cb_code))
            if enc:
                lines.append(enc)
        lines.extend(["### RES", "- out:"])
        enc = _bq_lines(stdout)
        if enc:
            lines.append(enc)
        lines.extend(["### ERR", "- err:"])
        enc = _bq_lines(stderr)
        if enc:
            lines.append(enc)
        workspace.write_locator(
            wt_uid, "/var/bash.md", drill("EXEC"), "+>",
            "\n" + "\n".join(lines) + "\n", auto_scaffold=True,
        )

    async def append_context_cmd_record(
        self, workspace, wt_uid: str, name: str, cmd_id: str,
        fields: dict[str, Any], stdout: str = "", stderr: str = "",
    ) -> None:
        await self.ensure_context_var_head(workspace, wt_uid, name)
        lines = [f"### {cmd_id}"]
        for key, value in fields.items():
            if value is None:
                continue
            lines.append(f"- {key}: {str(value).replace(chr(10), ' ')}")
        lines.extend(["### RES", "- out:"])
        enc = _bq_lines(stdout)
        if enc:
            lines.append(enc)
        lines.extend(["### ERR", "- err:"])
        enc = _bq_lines(stderr)
        if enc:
            lines.append(enc)
        workspace.write_locator(
            wt_uid, "/var/bash.md", drill(f"CONTEXTS.{name}"), "+>",
            "\n" + "\n".join(lines) + "\n", auto_scaffold=True,
        )

    async def update_exec_field(self, workspace, wt_uid: str, run_id: str, key: str, value: Any) -> None:
        workspace.write_locator(
            wt_uid, "/var/bash.md", by_tag(f"EXEC.{run_id}", key), "/>",
            str(value if value is not None else ""), auto_scaffold=True,
        )

    @staticmethod
    def parse_exec_parts(block: str) -> tuple[dict[str, str], str, str]:
        fields: dict[str, str] = {}
        res: list[str] = []
        err: list[str] = []
        cb: list[str] = []
        section = None
        for line in (block or "").splitlines():
            if line.startswith("### "):
                section = line[4:].strip()
                continue
            if line.startswith("## "):
                continue
            if section:
                bucket = cb if section == "CB" else (res if section == "RES" else err)
                if line == "  >":
                    bucket.append("")
                    continue
                if line.startswith("  > "):
                    bucket.append(line[4:])
                    continue
                continue
            if line.startswith("- ") and ":" in line:
                key, value = line[2:].split(":", 1)
                fields[key.strip()] = value.strip()
        if cb:
            fields["cb"] = "\n".join(cb).rstrip("\n")
        return fields, "\n".join(res).rstrip("\n"), "\n".join(err).rstrip("\n")

    def read_exec_parts(self, workspace, ctx: RuntimeContext, run_id: str):
        block = self.read_exec_block(workspace, ctx, run_id)
        if not block:
            return None
        return self.parse_exec_parts(block)

    def last_exec_id(self, workspace, wt_uid: str) -> str:
        try:
            chunks = workspace.read_section_chunks(wt_uid, "/var/bash.md", "EXEC", children=True)
        except Exception:
            chunks = []
        for chunk in reversed(chunks):
            text = str(chunk).strip()
            if not text:
                continue
            head = text.split("\n", 1)[0].strip()
            if head.startswith("## "):
                return head[3:].strip()
        return ""

    # ── tools ───────────────────────────────────────────

    @staticmethod
    def _parse_tool_block(text: str) -> dict[str, Any]:
        spec: dict[str, Any] = {}
        for line in (text or "").splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("- ") and ":" in line:
                key, value = line[2:].split(":", 1)
                spec[key.strip()] = value.strip()
        return spec

    def list_tools(self, workspace, wt_uid: str) -> dict[str, dict[str, Any]]:
        try:
            chunks = workspace.read_section_chunks(wt_uid, "/.mds/bash.md", "TOOLS", children=True)
        except Exception:
            chunks = []
        out: dict[str, dict[str, Any]] = {}
        for chunk in chunks:
            text = str(chunk)
            head = text.split("\n", 1)[0].strip().lstrip("#").strip()
            if not head:
                continue
            out[head] = self._parse_tool_block(text)
        return out

    def load_tool(self, workspace, wt_uid: str, tool_id: str) -> dict[str, Any] | None:
        return self.list_tools(workspace, wt_uid).get(tool_id)

    # ── read ──────────────────────────────────────────────

    def read_proc_block(self, workspace, ctx: RuntimeContext, pid: str) -> str:
        try:
            raw = workspace.read_section_chunks(ctx.wt_uid, ctx.proc_path, f"PROCESS_TABLE.BASH.{pid}")
        except Exception:
            raw = []
        return first_stripped(raw)

    def read_proc_fields(self, workspace, ctx: RuntimeContext, handle_id: str) -> dict[str, str]:
        block = self.read_proc_block(workspace, ctx, handle_id)
        if not block:
            return {}
        keys = (
            "kind", "status", "os_pid", "create_time", "pgid", "sid",
            "liveness", "last_seen", "confirmed_at", "kill_requested_at",
            "current_cmd", "cmd",
        )
        return {key: self.extract_strip_value(block, key, workspace) for key in keys}

    def last_context_handle(self, workspace, wt_uid: str) -> str:
        try:
            chunks = workspace.read_section_chunks(wt_uid, "/var/bash.md", "CONTEXTS", children=True)
        except Exception:
            return ""
        last = ""
        for chunk in chunks:
            text = str(chunk)
            parts = re.split(r"(?m)^### (?!RES$|ERR$)", text)
            for part in parts[1:]:
                lines = part.split("\n", 1)
                head = lines[0].strip()
                block = "## " + head + "\n" + (lines[1] if len(lines) > 1 else "")
                fields, _stdout, _stderr = self.parse_exec_parts(block)
                last = fields.get("run_id") or head
        return last

    def find_context_cmd(self, workspace, ctx: RuntimeContext, handle_id: str):
        try:
            chunks = workspace.read_section_chunks(ctx.wt_uid, "/var/bash.md", "CONTEXTS", children=True)
        except Exception:
            return None
        for chunk in chunks:
            text = str(chunk)
            parts = re.split(r"(?m)^### (?!RES$|ERR$)", text)
            for part in parts[1:]:
                lines = part.split("\n", 1)
                head = lines[0].strip()
                block = "## " + head + "\n" + (lines[1] if len(lines) > 1 else "")
                fields, stdout, stderr = self.parse_exec_parts(block)
                if head == handle_id or fields.get("run_id") == handle_id:
                    return fields, stdout, stderr
        return None

    def read_exec_block(self, workspace, ctx: RuntimeContext, run_id: str) -> str:
        try:
            raw = workspace.read_section_chunks(ctx.wt_uid, "/var/bash.md", f"EXEC.{run_id}")
        except Exception:
            raw = []
        return first_stripped(raw)

    async def read_context_var_block(self, workspace, ctx: RuntimeContext, name: str) -> str:
        try:
            raw = workspace.read_section_chunks(ctx.wt_uid, "/var/bash.md", f"CONTEXTS.{name}")
        except Exception:
            raw = []
        return first_stripped(raw)

    def list_context_names(self, workspace, wt_uid: str) -> list[str]:
        try:
            ir = SelectorBuilder().drill("CONTEXTS").any_child().build()
            chunks = workspace.read_locator(wt_uid, "/.mds/bash.md", ir, proj="tag")
        except Exception:
            return []
        return [str(c).strip() for c in chunks if str(c).strip()]

    # ── static utilities ──────────────────────────────────

    @staticmethod
    def parse_int(raw: str | None, default: int, label: str = "value") -> int:
        if raw in (None, ""):
            return default
        try:
            return max(1, int(float(str(raw))))
        except ValueError as e:
            raise ValueError(f"invalid -{label}: {raw}") from e

    @staticmethod
    def _engine_kv(workspace, text: str) -> dict[str, str] | None:
        """引擎文本出口 `workspace.to_json_text` → 信封 → `bullets_to_kv`。

        句柄缺失、引擎/JSON 异常一律 None（本函数不抛），由调用方决定降级到行解析。
        """
        if workspace is None:
            return None
        try:
            return bullets_to_kv(json.loads(workspace.to_json_text(text)))
        except Exception:
            return None

    @staticmethod
    def extract_strip_value(text: str, key: str, workspace=None) -> str:
        """取 `- key: value` 的 value（有 workspace 句柄走引擎，否则行解析）。

        有 workspace 句柄就走引擎（`_engine_kv`），差异（`- m : s` 整键丢失、值里行内标记被去标记、
        懒续行折进值、正文里列 0 的同名行取首个或末个）由 tests/test_plugin_kv_migration.py 逐条钉住。
        """
        kv = BashState._engine_kv(workspace, text)
        if kv is not None:
            return kv.get(key, "")
        match = re.search(rf"(?m)^- {re.escape(key)}:\s*(.*)$", text)
        return match.group(1).strip() if match else ""

    @staticmethod
    def compact_display_block(text: str) -> str:
        if not text:
            return text
        lines = text.splitlines()
        compacted: list[str] = []
        for idx, line in enumerate(lines):
            if line.strip():
                compacted.append(line)
                continue
            prev_line = next((item for item in reversed(compacted) if item.strip()), "")
            next_line = next((item for item in lines[idx + 1:] if item.strip()), "")
            prev_sig = prev_line.lstrip().startswith(("#", "- "))
            next_sig = next_line.lstrip().startswith(("#", "- "))
            if prev_sig and next_sig:
                continue
            compacted.append(line)
        return "\n".join(compacted).strip()

    @staticmethod
    def config_lines(config: BashConfig) -> list[str]:
        lines = [
            f"- shell: {config.shell}",
            f"- timeout: {config.timeout}",
        ]
        if config.cwd:
            lines.append(f"- cwd: {config.cwd}")
        if config.env:
            lines.append(f"- env: {json.dumps(config.env, ensure_ascii=False)}")
        lines.append(f"- engine: {config.session_engine}")
        return lines

    # ── private helpers ───────────────────────────────────

    def _parse_kv_chunks(self, chunks: list[Any], workspace=None) -> dict[str, str]:
        """多 chunk → 合并 kv（有句柄走引擎，否则走行解析）。

        **值后处理**：行解析路径 `parse_kv_lines(..., unquote_value=_unquote)` 的
        unquote 在 strip 之后施加；引擎路径 `bullets_to_kv` 已 strip，故只补 `_unquote`，
        两条路径顺序一致。
        """
        data: dict[str, str] = {}
        for chunk in chunks:
            kv = self._engine_kv(workspace, chunk)
            if kv is None:
                data.update(parse_kv_lines(chunk, unquote_value=_unquote))
            else:
                data.update({k: _unquote(v) for k, v in kv.items()})
        return data

    @staticmethod
    def _parse_env_json(raw: str) -> dict[str, str]:
        try:
            parsed = json.loads(raw)
        except Exception:
            return {}
        if not isinstance(parsed, dict):
            return {}
        return {str(k): str(v) for k, v in parsed.items()}
