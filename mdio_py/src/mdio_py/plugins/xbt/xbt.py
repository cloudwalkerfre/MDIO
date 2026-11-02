# mdio_py/src/mdio_py/plugins/xbt/xbt.py

import textwrap
import asyncio
from typing import Any
from dataclasses import asdict

from mdio_py.plugins.plugin_base import PluginBase
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.text_utils import unquote_plain, str_or_empty
from mdio_py.core.selector_builder import by_tag, drill
from mdio_py.core.error import is_error_text

from .parser import parse_xbt
from .stepper import XBTStepper, resolve_pointer, _kv_from_state_text

class XBTPlugin(PluginBase):
    @property
    def protocol_name(self) -> str:
        return "XBT"

    def _unquote(self, s: Any) -> str:
        return unquote_plain(s)   # 值清洗：成对剥引号，不解转义

    async def __call__(
        self,
        workspace, 
        input_data: Any, 
        chain: list,      
        ctx: RuntimeContext,
        kwargs: dict
    ) -> Any:
        
        # 内核侧：spawn / 世界线 / 黑板
        if "-spawn" in kwargs:
            tid = self._unquote(kwargs.get("-spawn"))
            return await self._sys_spawn(workspace, ctx, tid, kwargs)

        if "-kill" in kwargs:
            tid = self._unquote(kwargs.get("-kill") or input_data)
            is_cascade = self._unquote(kwargs.get("-cascade", "false")).lower() == "true"
            return await self._sys_kill(workspace, ctx, tid, cascade=is_cascade)

        if "-step" in kwargs:
            tid = self._unquote(kwargs.get("-step") or input_data)
            return await XBTStepper.step(workspace, ctx, tid)
            
        if "-inspect" in kwargs:
            tid = self._unquote(kwargs.get("-inspect") or input_data)
            key = self._unquote(kwargs.get("-k", ""))
            return await self._sys_inspect(workspace, ctx, tid, key)
            
        if "-resolve" in kwargs:
            tid = self._unquote(kwargs.get("-resolve", ""))
            answer = self._unquote(input_data)
            return await self._sys_resolve(workspace, ctx, tid, answer)

        # 用户侧：step / 状态查询
        if not chain:
            return await self._list_threads(workspace, ctx)

        tid = self._unquote(chain[0])
        action = self._unquote(chain[1]) if len(chain) > 1 else "state"

        if action == "run":
            blueprint = self._unquote(kwargs.get("-bp", kwargs.get("-blueprint", "")))
            if not blueprint:
                return f"- mdio_error: [@XBT {tid} run] requires -bp parameter."
            spawn_kwargs = {"-bp": blueprint, "-ppid": "root", "-ptr": "0"}
            driver = self._unquote(kwargs.get("-driver", ""))
            if driver: spawn_kwargs["-driver"] = driver
            bb = self._unquote(kwargs.get("-bb", "") or "")
            if bb: spawn_kwargs["-bb"] = bb
            return await self._sys_spawn(workspace, ctx, tid, spawn_kwargs)

        if action == "kill":
            return await self._sys_kill(workspace, ctx, tid, cascade=True)

        if action == "state":
            raw = await workspace.runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
            return raw if raw else f"- mdio_info: Thread [{tid}] not found or finished."

        if action == "pause":
            state_dict = await self._get_state_dict(workspace, ctx, tid)
            if not state_dict: return f"- mdio_info: Thread [{tid}] not found."
            current_st = state_dict.get("status")
            if current_st in ("WAKE_UP", "PENDING", "ROLLBACK"):
                workspace.write_locator(
                    ctx.wt_uid, "/.mds/proc.md",
                    by_tag(f"PROCESS_TABLE.XBT.{tid}", "status"),
                    "/>", "PAUSED", auto_scaffold=True, suppress_hooks=True,
                )
                return f"- mdio_info: Thread [{tid}] is now PAUSED."
            return f"- warn: Cannot pause thread in status: {current_st}."

        if action == "resume":
            state_dict = await self._get_state_dict(workspace, ctx, tid)
            if not state_dict: return f"- mdio_info: Thread [{tid}] not found."
            if state_dict.get("status") == "PAUSED":
                workspace.write_locator(
                    ctx.wt_uid, "/.mds/proc.md",
                    by_tag(f"PROCESS_TABLE.XBT.{tid}", "status"),
                    "/>", "WAKE_UP", auto_scaffold=True,
                )
                return f"- mdio_info: Thread [{tid}] RESUMED."
            return f"- mdio_info: Thread [{tid}] is not PAUSED."
        
        if action == "inspect":
            key = self._unquote(kwargs.get("-k", ""))
            return await self._sys_inspect(workspace, ctx, tid, key)

        return f"- mdio_error: [@XBT] Unknown action '{action}'"

    # 系统调用实现
    async def _ensure_var_dir(self, workspace, wt_uid: str):
        """懒创建 /var/xbt.md 工作区"""
        try:
            if not workspace.path_exists(wt_uid, "/var"):
                workspace.mk(wt_uid, "/var", parents=True, verbose=False)
            if not workspace.path_exists(wt_uid, "/var/xbt.md"):
                workspace.mk(wt_uid, "/var/xbt.md", parents=False, verbose=False)
        except Exception:
            pass

    async def _ensure_blackboard(self, workspace, ctx: RuntimeContext, tid: str, bb_arg: str) -> str:
        """确定本线程的 bb 地址：显式 -bb 先 ensure；缺省在 /XBT_BlackBoard/<tid>.md 建 # BlackBoard 并复用。"""
        addr = str(bb_arg or "").strip() or ("/XBT_BlackBoard/%s.md::BlackBoard" % tid)
        if "::" in addr:
            doc, node = addr.split("::", 1)
        else:
            doc, node = addr, ""
        doc, node = doc.strip(), node.strip()
        if not doc:
            return "- mdio_error: [XBT] invalid -bb address: %r" % addr
        try:
            if not workspace.path_exists(ctx.wt_uid, doc):
                workspace.mk(ctx.wt_uid, doc, parents=True, verbose=False)
            if node:
                current = await workspace.runtime.exec(f"${doc}::{node}", ctx=ctx)
                if not current or "- mdio_error:" in str(current):
                    res = await workspace.runtime.exec(f'"" | +> {doc}::{node}', ctx=ctx)
                    if is_error_text(str(res)):
                        return "- mdio_error: [XBT] can't ensure blackboard %s: %s" % (addr, str(res)[:200])
            return addr
        except Exception as e:
            return "- mdio_error: [XBT] blackboard ensure failed: %s" % str(e)[:200]
    async def _sys_spawn(self, workspace, ctx: RuntimeContext, tid: str, kwargs: dict):
        # print(f"🔄 [SPAWN] 收到 spawn 请求: tid='{tid}', kwargs={kwargs}")
        if not tid: return "- mdio_error: [-spawn] needs tid"
        await self._ensure_var_dir(workspace, ctx.wt_uid)
        
        raw_bp = self._unquote(kwargs.get("-bp", ""))
        bp_sel = raw_bp if "::" in raw_bp else f"{raw_bp}::"

        bb_arg = self._unquote(kwargs.get("-bb", "") or "")
        bb = await self._ensure_blackboard(workspace, ctx, tid, bb_arg)
        if str(bb).startswith("- mdio_error:"):
            return bb

        ppid = self._unquote(kwargs.get("-ppid", "root"))
        payload = {
            "blueprint": bp_sel,
            "pointer": self._unquote(kwargs.get("-ptr", "0")),
            "bb": bb,
        }
        if ppid == "root":
            # 黑板地址只在每棵树的第一次 driver prompt 给一次。
            payload["bb_briefed"] = "false"
        driver = self._unquote(kwargs.get("-driver", ""))
        if driver:
            payload["driver"] = driver
        for flag, key in (("-wl", "wl"), ("-wl_type", "wl_type")):
            value = kwargs.get(flag)
            if value is not None:
                value = self._unquote(value)
                if value:
                    payload[key] = value
        
        # 先建 proc 并写全部簿记（全部 suppress），再注册 status hook，
        # 最后唯一一次非 suppress 写是 status=WAKE_UP（唤醒引擎）。
        await self._register_proc(
            workspace, ctx, identifier=tid, ppid=ppid,
            payload=payload, exact_pid=tid, register_task=False,
            suppress_hooks=True,
        )
        
        cb_code = f'@XBT -step "{tid}"'
        status_ir = by_tag(f"PROCESS_TABLE.XBT.{tid}", "status")
        hook_id = workspace.store.register_hook(ctx.wt_uid, "/.mds/proc.md", status_ir, cb_code, asdict(ctx), is_always=True)

        workspace.write_locator(
            ctx.wt_uid, "/.mds/proc.md",
            by_tag(f"PROCESS_TABLE.XBT.{tid}", "sys_hook_id"),
            "/>", str(hook_id), auto_scaffold=True, suppress_hooks=True,
        )
        workspace.write_locator(
            ctx.wt_uid, "/.mds/proc.md",
            by_tag(f"PROCESS_TABLE.XBT.{tid}", "status"),
            "/>", "WAKE_UP", auto_scaffold=True,
        )
        # print(f"🔄 [SPAWN] 线程 {tid} 创建完成")
        return f"- mdio_info: {bb}"

    async def _sys_kill(self, workspace, ctx: RuntimeContext, tid: str, cascade: bool = False):
        head_regex = f"^{tid}(_|$)" if cascade else f"^{tid}$"
        
        fetch_cmd = f"$/.mds/proc.md::PROCESS_TABLE.XBT.*[.tag.re('{head_regex}')]._b:sys_hook_id"
        raw_ids = await workspace.runtime.exec(fetch_cmd, ctx=ctx)
        
        kill_cmds = []
        if raw_ids and "- mdio_error:" not in raw_ids:
            for h_id in str(raw_ids).strip().split('\n'):
                h_id = h_id.strip()
                if h_id.isdigit(): kill_cmds.append(f'hook -del {h_id}')
                
        kill_cmds.append(f'-> /.mds/proc.md::PROCESS_TABLE.XBT.*[.tag.re("{head_regex}")]')
        
        if kill_cmds:
            await workspace.runtime.exec(" ; ".join(kill_cmds), ctx=ctx)
        return f"- mdio_info: {'Cascading ' if cascade else ''}Kill for [{tid}] executed."

    async def _sys_inspect(self, workspace, ctx: RuntimeContext, tid: str, key: str = None):
        state_dict = await self._get_state_dict(workspace, ctx, tid)
        if not state_dict: return "- mdio_error: Target missing."

        bp_sel = state_dict.get("blueprint")
        pointer = state_dict.get("pointer")
        if not bp_sel or not pointer: return "- mdio_error: Corrupted thread state."

        raw_xbt = await XBTStepper._read_blueprint(workspace, ctx, bp_sel)
        
        try:
            ast = parse_xbt(raw_xbt)
            node, _ = resolve_pointer(ast, pointer)
            if not node: return "- mdio_error: Pointer detached."
                
            if key:
                if key == "condition" and state_dict.get("phase") == "C_CHILD":
                    idx_str = state_dict.get("pending_cond_idx")
                    if idx_str and idx_str.isdigit():
                        children = node.get("children", [])
                        idx = int(idx_str)
                        if idx < len(children):
                            return str(children[idx].get("condition", "")).strip()
                val = node.get(key)
                return str_or_empty(val)
            
            lines = ["# NODE_CONTEXT"]
            for k, v in node.items():
                if k == "children": continue 
                v_str = str(v).replace("\n", " ") if not isinstance(v, list) else ", ".join(str(x) for x in v)
                if v_str: lines.append(f"- {k}: {v_str}")
            return "\n".join(lines)
        except Exception as e:
            return f"- mdio_error: Context extraction failed: {e}"

    async def _sys_resolve(self, workspace, ctx: RuntimeContext, tid: str, answer: str):
        if not tid:
            return "- mdio_error: [-resolve] missing tid"
        
        ans = str(answer).strip().lower()
        
        if ans == "failed":
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                by_tag(f"PROCESS_TABLE.XBT.{tid}", "result"),
                "/>", "FAILED", auto_scaffold=True,
            )
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                by_tag(f"PROCESS_TABLE.XBT.{tid}", "status"),
                "/>", "WAKE_UP", auto_scaffold=True,
            )
            return f"- mdio_info: Force Break (Poison) injected into [{tid}]."

        state_dict = await self._get_state_dict(workspace, ctx, tid)
        if not state_dict:
            return f"- mdio_info: [{tid}] state missing or thread dead."

        # C_CHILD 等相态独立作为路标，控制后续的属性结算方向
        phase = state_dict.get("phase", "")
        if not phase:
            return f"- mdio_info: [{tid}] is not suspended (No valid phase found)."
        
        cmds = []

        if phase == "C_SELF":
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                by_tag(f"PROCESS_TABLE.XBT.{tid}", "cond_passed"),
                "/>", ans, auto_scaffold=True,
            )
        elif phase == "C_CHILD":
            idx = state_dict.get("pending_cond_idx", "0")
            if ans == "true":
                workspace.write_locator(
                    ctx.wt_uid, "/.mds/proc.md",
                    by_tag(f"PROCESS_TABLE.XBT.{tid}", "decision"),
                    "/>", idx, auto_scaffold=True,
                )
            else:
                workspace.write_locator(
                    ctx.wt_uid, "/.mds/proc.md",
                    by_tag(f"PROCESS_TABLE.XBT.{tid}", f"cond_{idx}"),
                    "/>", "false", auto_scaffold=True,
                )
        elif phase == "A_EXE":
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                by_tag(f"PROCESS_TABLE.XBT.{tid}", "result"),
                "/>", str(answer), auto_scaffold=True,
            )
        else:
            return f"- mdio_error: Unrecognized suspension phase: {phase}"

        workspace.write_locator(
            ctx.wt_uid, "/.mds/proc.md",
            by_tag(f"PROCESS_TABLE.XBT.{tid}", "phase"),
            "->", "", auto_scaffold=False,
        )
        workspace.write_locator(
            ctx.wt_uid, "/.mds/proc.md",
            by_tag(f"PROCESS_TABLE.XBT.{tid}", "pending_cond_idx"),
            "->", "", auto_scaffold=False,
        )
        workspace.write_locator(
            ctx.wt_uid, "/.mds/proc.md",
            by_tag(f"PROCESS_TABLE.XBT.{tid}", "status"),
            "/>", "WAKE_UP", auto_scaffold=True,
        )

        # 全局引擎复苏心跳：踹他一脚，让他跑起来！
        await XBTStepper.step(workspace, ctx, tid)

        return f"- mdio_info: Resolving [{tid}] completed, engine resumed."

    @staticmethod
    async def _get_state_dict(workspace, ctx, tid: str) -> dict:
        heading = f"PROCESS_TABLE.XBT.{tid}"
        try:
            chunks = workspace.read_locator(ctx.wt_uid, "/.mds/proc.md", drill(heading))
        except Exception:
            return {}
        if not chunks:
            return {}
        # 引擎信封 + bullets_to_kv；值里的字面 \n 还原成真换行。
        kv = _kv_from_state_text(workspace, str(chunks[0]))
        return {k: v.replace("\\n", "\n") for k, v in kv.items()}

    async def _list_threads(self, workspace, ctx: RuntimeContext):
        raw = await workspace.runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        return raw if raw and "- mdio_error:" not in raw else "- mdio_info: No active threads."
