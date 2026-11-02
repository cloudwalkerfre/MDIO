# mdio_py/src/mdio_py/plugins/xbt/stepper.py

import hashlib
import json
import time
from typing import Dict, Any, Optional, List, Tuple
from mdio_py.plugins.xbt.parser import parse_xbt
from mdio_py.plugins.xbt.driver_types import format_invoke, format_open, recipe_for
from mdio_py.plugins import info as xbt_info
from mdio_py.core.selector_builder import by_tag, drill
from mdio_py.core.error import is_error_text
from mdio_py.core.text_utils import str_or

def fast_hash(text: str) -> str:
    return hashlib.md5(text.encode('utf-8')).hexdigest()[:8]

def compute_node_hash(node: Dict[str, Any]) -> str:
    signature = f"{node.get('type')}|{node.get('condition', '')}|{node.get('prompt', '')}"
    return fast_hash(signature)


def _kv_from_state_text(workspace, text: str) -> Dict[str, str]:
    """PROCESS_TABLE.XBT 段文本 → {字段: 值}。

    引擎信封 `workspace.to_json_text(text)` → `bullets_to_kv`；
    值后处理（`\\n` 还原等）由调用方施加，本函数只出原始值。
    """
    from mdio_py.core.ast_utils import bullets_to_kv
    return bullets_to_kv(json.loads(workspace.to_json_text(text)))


def find_node_by_path(nodes: List[Dict], path: str) -> Optional[Dict]:
    if not path: return None
    parts = [int(p) for p in path.split('.')]
    cur = nodes
    for i, idx in enumerate(parts):
        if idx < len(cur):
            if i == len(parts) - 1: return cur[idx]
            cur = cur[idx].get("children", [])
        else: return None
    return None

def find_node_by_hash(nodes: List[Dict], hash_str: str) -> Optional[Dict]:
    queue = list(nodes)
    while queue:
        node = queue.pop(0)
        if compute_node_hash(node) == hash_str: return node
        queue.extend(node.get("children", []))
    return None

def find_path_by_node(nodes: List[Dict], target: Dict, prefix: str = "") -> Optional[str]:
    for i, node in enumerate(nodes):
        current_path = f"{prefix}.{i}" if prefix else str(i)
        if node is target: return current_path
        if node.get("children"):
            found = find_path_by_node(node["children"], target, current_path)
            if found: return found
    return None

def resolve_pointer(ast: List[Dict], pointer: str) -> Tuple[Optional[Dict], str]:
    """只保留拓扑路径和哈希，不含相态。"""
    parts = pointer.split()
    
    # 剥离指针里的相态前缀（A_EXE/C_SELF/C_CHILD/INIT）
    if parts and parts[0] in ("A_EXE", "C_SELF", "C_CHILD", "INIT"):
        parts = parts[1:]
        
    if len(parts) == 0: 
        return ast[0] if ast else None, f"0 {compute_node_hash(ast[0]) if ast else '0'}"
        
    topo_hint = parts[0]
    hash_str = parts[1] if len(parts) > 1 else compute_node_hash(find_node_by_path(ast, topo_hint) or {})
    
    node = find_node_by_path(ast, topo_hint)
    if node and compute_node_hash(node) == hash_str: 
        return node, f"{topo_hint} {hash_str}"
        
    # 发生地址漂移，开启虫洞重定位
    node = find_node_by_hash(ast, hash_str)
    if node:
        new_path = find_path_by_node(ast, node)
        if new_path: return node, f"{new_path} {hash_str}"
            
    return None, f"0 {compute_node_hash(ast[0]) if ast else '0'}"

def substitute_template_vars(text: str, tid: str, ppid: str) -> str:
    """Replace {TID} {PPID} {PROC} {VAR} placeholders in JIT command/condition strings."""
    root_tid = tid.split('_')[0]
    return (text
        .replace("{TID}", tid)
        .replace("{PPID}", ppid)
        .replace("{PROC}", f"/.mds/proc.md::PROCESS_TABLE.XBT.{tid}")
        .replace("{VAR}", f"/var/xbt.md::{root_tid}")
    )

DEFAULT_DRIVER = "XBT_DIRECTOR"


def derive_position_path(ast: List[Dict], pointer: str, node: Dict[str, Any]) -> Tuple[str, Optional[Dict[str, Any]], Optional[int]]:
    """pointer/node 组合 → (node_path, selector, idx)。

    node 是 pointer 指向的节点（自守卫 / action）→ (pointer 拓扑路径, None, None)；
    node 是 selector 的孩子（NLP 子条件）→ (selector 路径 + "." + idx, selector, idx)。
    """
    topo = str(pointer or "0").split()[0]
    pnode, _ = resolve_pointer(ast, pointer)
    if pnode is None:
        return topo, None, None
    if pnode is node:
        return topo, None, None
    for idx, child in enumerate(pnode.get("children") or []):
        if child is node:
            return f"{topo}.{idx}", pnode, idx

    def dfs(nodes: List[Dict], prefix: str) -> Optional[str]:
        for i, n in enumerate(nodes):
            path = f"{prefix}.{i}" if prefix else str(i)
            if n is node:
                return path
            found = dfs(n.get("children") or [], path)
            if found:
                return found
        return None

    return dfs(ast, "") or topo, None, None


def render_candidates(selector_node: Dict[str, Any], current_idx: int) -> str:
    """selector 的全部候选 → 候选段（`- k:` + `    > ` 载荷）。"""
    children = selector_node.get("children") or []
    if not children:
        return ""

    lines: List[str] = [f"这个 selector 的全部候选（当前问第 {current_idx} 个）："]
    for i, child in enumerate(children):
        cond = _candidate_condition(child)
        brief = _candidate_action(child)
        head = cond + (" → 成立时执行：" + brief if brief else "")
        lines.append(f"- {i}:")
        for ln in head.splitlines():
            lines.append("    > " + ln if ln else "    >")
    lines.append(f"请只针对第 {current_idx} 个返回 true/false，不要回答整组。")
    return "\n".join(lines)


def _candidate_condition(child: Dict[str, Any]) -> str:
    cond = child.get("condition")
    if cond is None or not str(cond).strip():
        return "[]"
    cond = str(cond).strip()
    if child.get("is_cmd_cond"):
        return "[{{ " + cond + " }}]"
    return "[" + cond + "]"


def _candidate_action(child: Dict[str, Any]) -> str:
    node_type = child.get("type")
    if node_type == "action":
        body = str(child.get("prompt") or "").strip()
        if child.get("is_cmd"):
            return "{{ " + body + " }}" if body else "{{ }}"
        return body
    if node_type == "control":
        return "（子树）"
    return ""


def _tag_driver(node: Dict[str, Any]) -> str:
    tags = node.get("tags") or []
    for raw in tags:
        name = str(raw).strip().lstrip("#")
        if name:
            return name
    return ""


def find_driver(ast: List[Dict], pointer: str, state: Dict[str, Any]) -> str:
    """最近祖先/自身的 #tag 作为 LLM agent 名；否则进程 driver；否则系统默认。"""
    path = str(pointer or "0").split()[0]
    nodes = ast
    found = ""
    for part in path.split("."):
        try:
            idx = int(part)
        except ValueError:
            break
        if idx < 0 or idx >= len(nodes):
            break
        node = nodes[idx]
        tag = _tag_driver(node)
        if tag:
            found = tag
        nodes = node.get("children", [])
    if found:
        return found
    return str(state.get("driver") or DEFAULT_DRIVER)


def _thread_wl(state: Dict[str, Any]) -> str:
    """线程运行的世界线 token；root 线程固定 main（所有介质的通用默认层）。"""
    return str(state.get("wl") or "").strip() or "main"


def _thread_wl_type(state: Dict[str, Any]) -> str:
    """开这把世界线 token 的 driver 类型（插件协议名）；root 线程为空。"""
    return str(state.get("wl_type") or "").strip()


def _fork_site(tid: str, pointer: str) -> str:
    """fork 身份：父线程 tid + 父节点拓扑路径 → 8 位 hash，保证不同树/线程不撞名。"""
    topo = str(pointer or "0").split()[0]
    return fast_hash(str(tid) + "|" + topo)


def _fork_wl(parent_wl: str, tid: str, pointer: str, idx: int) -> str:
    """子世界线 token：root 层 b<site>_<i>，嵌套层 <parent_wl>_<site>_<i>（前缀链可推导）。"""
    site = _fork_site(tid, pointer)
    if not parent_wl or parent_wl == "main":
        return f"b{site}_{idx}"
    return f"{parent_wl}_{site}_{idx}"


def clean_driver_pipeline(text: Any) -> str:
    raw = str(text or "").strip()
    if raw.startswith("```"):
        lines = raw.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip().startswith("```"):
            lines = lines[:-1]
        raw = "\n".join(lines).strip()
    if raw.startswith("MDIO:"):
        raw = raw[5:].strip()
    return raw


def build_driver_prompt(tid: str, pointer: str, node: Dict[str, Any], kind: str,
                        blackboard_text: str = "", ast: Optional[List[Dict]] = None,
                        bp_text: str = "", bp_node_text: str = "",
                        blackboard_addr: str = "") -> str:
    """driver prompt：蓝图节点原文 + 当前任务 + 位置 +（selector 候选）+ 已知 + ask。

    bp_node_text 是蓝图节点原文（含 - BB: 等声明），原样给导演看；
    bp_text 只用于解析，显示时作为 fallback。
    """
    tree = ast or []
    node_path, selector, idx = derive_position_path(tree, pointer, node)

    lines: List[str] = []
    node_blueprint = str(bp_node_text or "").strip()
    blueprint = str(bp_text or "").strip()
    if node_blueprint:
        lines += ["这是你要驱动的行为树：", "", node_blueprint, ""]
    elif blueprint:
        lines += ["这是你要驱动的行为树：", "", "```xbt", blueprint, "```", ""]

    natural = node.get("condition") if kind == "condition" else node.get("prompt")
    lines.append("当前要判断的条件：" if kind == "condition" else "当前要做的动作：")
    for ln in str(natural or "").strip().splitlines():
        lines.append("> " + ln if ln else ">")
    lines.append("")

    lines += ["你在这棵树里的位置：", "", "```text",
              render_position(tree, node_path, kind), "```", ""]

    if kind == "condition" and selector is not None and idx is not None:
        lines += [render_candidates(selector, idx), ""]

    bb_addr = str(blackboard_addr or "").strip()
    if bb_addr:
        lines += [f"黑板：{bb_addr}", ""]
    lines.append("请结合上下文，只返回 true 或 false。" if kind == "condition"
                 else '请按 action prompt 执行。')
    return "\n".join(lines)


def _node_path_nodes(ast: List[Dict], pointer: str) -> list:
    """pointer 拓扑前缀 → [(path, node)]，按树根到当前节点顺序。"""
    topo = str(pointer or "0").split()[0]
    out = []
    nodes = ast
    acc = []
    for part in topo.split("."):
        if not part:
            continue
        try:
            idx = int(part)
        except ValueError:
            break
        if idx < 0 or idx >= len(nodes):
            break
        node = nodes[idx]
        acc.append(part)
        out.append((".".join(acc), node))
        nodes = node.get("children", [])
    return out

def _node_source_line(node: Dict[str, Any], keep_body: bool = False) -> str:
    """AST 节点 → XBT 源行（不暴露数字路径）。"""
    mark = str(node.get("dm") or "").strip()
    cond = node.get("condition")
    cond_part = "" if cond is None else f"[{str(cond).strip()}]"
    tags = " ".join(str(t) for t in (node.get("tags") or []))
    parts = [p for p in (mark, cond_part, tags) if p]
    head = " ".join(parts)
    if keep_body and node.get("type") == "action":
        body = str(node.get("prompt") or "").strip()
        if node.get("is_cmd"):
            body = "{{ " + body + " }}"
        if body:
            head = (head + " " + body).strip()
    return head

def render_xbt_skeleton(ast: List[Dict], current_path: str = "", current_kind: str = "") -> str:
    """整棵树的骨架：control node 保留、action 体默认洗掉；当前节点标 ← 当前。"""
    lines: List[str] = []

    def walk(children: List[Dict], prefix: str, depth: int) -> None:
        for i, node in enumerate(children):
            path = f"{prefix}.{i}" if prefix else str(i)
            is_current = path == current_path
            line = _node_source_line(node, keep_body=(is_current and current_kind == "action"))
            if is_current:
                line += "   ← 当前"
            lines.append("    " * depth + line)
            if node.get("children"):
                walk(node["children"], path, depth + 1)

    if not ast:
        return ""
    if ast[0].get("type") == "root":
        walk(ast[0].get("children", []), "0", 0)
    else:
        walk(ast, "", 0)
    return "\n".join(lines)

def render_position(ast: List[Dict], node_path: str, current_kind: str = "") -> str:
    """当前位置 = 只画 root→当前节点的路径，父的兄弟/当前节点的兄弟都不进来。"""
    path_nodes = _node_path_nodes(ast, node_path)
    nodes = [n for _, n in path_nodes]
    if nodes and nodes[0].get("type") == "root":
        nodes = nodes[1:]
    if not nodes:
        return ""
    lines: List[str] = []
    for depth, node in enumerate(nodes):
        is_current = depth == len(nodes) - 1
        line = "    " * depth + _node_source_line(
            node, keep_body=(is_current and current_kind == "action"))
        if is_current:
            line += "   ← 当前"
        lines.append(line)
    return "\n".join(lines)



class XBTStepper:
    @staticmethod
    async def step(workspace, ctx, tid: str) -> str:
        """单次微步：读状态快照 → 判 WAKE_UP/ROLLBACK → 处理一个节点。

        并发保护在 hook 检测层：Register 水位（HookDef.active_from / DetectTask.snap_seq）
        让注册前的旧快照不投递，同一 tid 的重复脉冲消失；本层无需 per-tid 串行锁。
        """
        state = await XBTStepper._read_state(workspace, ctx, tid)
        if state is None: return f"- mdio_info: Thread {tid} not found"
        
        if state.get("result") == "FAILED":
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
            return f"- mdio_info: Thread {tid} poisoned and terminated."

        if state.get("status") not in ("WAKE_UP", "ROLLBACK"): 
            return f"- mdio_info: Thread {tid} ignored"

        blueprint_sel = state.get("blueprint")
        if not blueprint_sel:
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
            return "- mdio_error: Missing blueprint"

        bp_txt = await XBTStepper._read_blueprint(workspace, ctx, blueprint_sel)
        if not bp_txt:
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
            return "- mdio_error: Blueprint read error."

        # Read metadata from blueprint head (if not already in state)
        if "meta" not in state:
            meta_text = await XBTStepper._read_blueprint(workspace, ctx, blueprint_sel, "meta")
            if meta_text:
                safe_meta = meta_text.strip().replace('\n', '\\n')
                await XBTStepper._update_fields(workspace, ctx, tid, {"meta": safe_meta})
                state["meta"] = safe_meta

        try:
            ast = parse_xbt(bp_txt)
        except Exception as e:
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
            return f"- mdio_error: Blueprint parse crash: {e}"

        pointer = state.get("pointer", f"0 {compute_node_hash(ast[0])}")
        node, new_pointer = resolve_pointer(ast, pointer)
        
        if node is None:
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
            return "- mdio_error: Pointer resolution failed."
        
        if new_pointer != pointer:
            await XBTStepper._update_fields(workspace, ctx, tid, {"pointer": new_pointer})
            state["pointer"] = new_pointer

        xbt_info.emit("step", tid=tid, ppid=state.get("ppid", ""),
                      ptr=str(state.get("pointer", "")).split(" ")[0], wl=_thread_wl(state),
                      wl_type=_thread_wl_type(state), status=state.get("status", ""),
                      phase=state.get("phase", ""), node=node.get("type", ""),
                      cond=str(node.get("condition") or ""),
                      action=str(node.get("prompt") or ""))

        # C_SELF：门禁隔离与 SKIPPED 因果
        cond_res = await XBTStepper._evaluate_self_condition(workspace, ctx, tid, state, node, ast)
        
        if cond_res == "SUSPENDED":
            await XBTStepper._update_fields(workspace, ctx, tid, {
                "phase": "C_SELF", "status": "WAITING"
            }, suppress_hooks=True)
            return f"- mdio_info: Thread {tid} suspended at C_SELF."
            
        if cond_res == "SKIPPED":
            # 守卫条件不满足时该节点视为放过：向上级汇报 SUCCESS，
            # 包含它的 >> 序列继续行进
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "SUCCESS")
            return f"- mdio_info: Thread {tid} safely bypassed."

        if cond_res == "FAILED":
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
            return f"- mdio_info: Thread {tid} condition trap/error."
            
        node_type = node.get("node_type", ">>") 
        node_kind = node.get("type", "unknown")
        
        if node_kind == "action" and "prompt" in node:
            return await XBTStepper._handle_action(workspace, ctx, tid, state, node, ast)
        if node_kind in ("control", "root"):
            return await XBTStepper._handle_control(workspace, ctx, tid, state, node, node_type, ast)
            
        await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
        return f"- mdio_error: Unknown ast type {node_kind}"

    @staticmethod
    async def _handle_action(workspace, ctx, tid: str, state: Dict, node: Dict, ast: List[Dict]) -> str:
        if "result" in state:
            await XBTStepper._bubble_up(workspace, ctx, tid, state, state["result"])
            return f"- mdio_info: Action completed."

        if node.get("is_cmd"):
            cmd = node.get("prompt", "")
            cmd = substitute_template_vars(cmd, tid, state.get("ppid", "root"))
            try:
                sys_context_lines = [f"# T", f"- tid: {tid}", f"- ppid: {state.get('ppid')}"]
                if state.get("meta"):
                    sys_context_lines.append(f"- meta: {state.get('meta')}")
                sys_context = "\n".join(sys_context_lines)
                res = await workspace.runtime.exec(f'"""{sys_context.strip()}""" | {cmd}', ctx=ctx)
                xbt_info.emit("exec", tid=tid, ppid=state.get("ppid", ""),
                              ptr=str(state.get("pointer", "")).split(" ")[0], wl=_thread_wl(state),
                              pipeline=cmd, ok=not is_error_text(res), result=str(res)[:300])
                if is_error_text(res):
                    await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
                else:
                    await XBTStepper._bubble_up(workspace, ctx, tid, state, "SUCCESS")
                return f"- mdio_info: JIT Action Executed."
            except Exception as e:
                await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
                return f"- mdio_error: JIT Trapped: {e}"

        driver = find_driver(ast, state.get("pointer", ""), state)
        if driver:
            # action 的语义：XBT 只 exec 一次导演，成败=这次 exec 的成败；
            # 导演返回的文本不当管道二次执行（业务执行由导演自己完成）。
            _director_text, err = await XBTStepper._driver_pipeline(
                workspace, ctx, tid, state, node, ast, driver, "action")
            if err:
                await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
                return f"- mdio_error: XBT driver failed: {err}"
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "SUCCESS")
            return f"- mdio_info: NLP action directed by {driver}."

        await XBTStepper._update_fields(workspace, ctx, tid, {
            "phase": "A_EXE", "status": "WAITING"
        }, suppress_hooks=True)
        return f"- mdio_info: Suspended for External Action."

    @staticmethod
    async def _driver_pipeline(workspace, ctx, tid: str, state: Dict, node: Dict, ast: List[Dict], driver: str, kind: str) -> Tuple[str, str]:
        bp_sel = state.get("blueprint", "")
        bp_text = await XBTStepper._read_blueprint(workspace, ctx, bp_sel)
        bp_node_text = await XBTStepper._read_blueprint_node(workspace, ctx, bp_sel)
        root_tid, root_state = await XBTStepper._root_state(workspace, ctx, tid, state)
        bb_briefed = root_state.get("bb_briefed") == "true"
        bb_addr = "" if bb_briefed else str(state.get("bb") or "")
        prompt = build_driver_prompt(
            tid, state.get("pointer", ""), node, kind, "", ast, bp_text, bp_node_text,
            bb_addr)
        xbt_info.emit("driver", tid=tid, ppid=state.get("ppid", ""),
                      kind=kind, driver=driver,
                      ptr=str(state.get("pointer", "")).split(" ")[0],
                      wl=_thread_wl(state), wl_type=_thread_wl_type(state),
                      node=node.get("type", ""),
                      cond=str(node.get("condition") or ""),
                      action=str(node.get("prompt") or ""),
                      blueprint=str(bp_node_text or ""),
                      bb=str(state.get("bb") or ""),
                      position=render_position(
                          ast, derive_position_path(ast, state.get("pointer", ""), node)[0], kind))
        proto, recipe = recipe_for(driver)
        wl = _thread_wl(state)
        wl_type = _thread_wl_type(state)
        token = None
        if recipe["worldline"]:
            if wl_type and wl_type != proto:
                print(f"[XBT] driver {driver!r} 不属于世界线介质 {wl_type!r}：按无 token 调用")
            else:
                token = wl
        elif wl_type and wl_type != proto:
            print(f"[XBT] driver {driver!r} 不属于世界线介质 {wl_type!r}：按无 token 调用")
        invoke = format_invoke(driver, token)
        _t0 = time.perf_counter()
        try:
            raw = await workspace.runtime.exec(json.dumps(prompt, ensure_ascii=False) + " | " + invoke, ctx=ctx)
        except Exception as e:
            xbt_info.emit("driver_reply", tid=tid, kind=kind, ok=False,
                          ms=int((time.perf_counter() - _t0) * 1000),
                          reply="EXC: " + str(e)[:400])
            return "", str(e)
        _ms = int((time.perf_counter() - _t0) * 1000)
        if not raw or is_error_text(raw):
            xbt_info.emit("driver_reply", tid=tid, kind=kind, ok=False, ms=_ms,
                          reply=str(raw or "empty driver response")[:400])
            return "", str(raw or "empty driver response")[:500]
        pipeline = clean_driver_pipeline(raw)
        if not pipeline:
            xbt_info.emit("driver_reply", tid=tid, kind=kind, ok=False, ms=_ms,
                          reply="empty driver pipeline")
            return "", "empty driver pipeline"
        if not bb_briefed and str(state.get("bb") or "").strip():
            await XBTStepper._update_fields(
                workspace, ctx, root_tid, {"bb_briefed": "true"}, suppress_hooks=True)
        xbt_info.emit("driver_reply", tid=tid, kind=kind, ok=True, ms=_ms, reply=pipeline)
        return pipeline, ""

    @staticmethod
    async def _evaluate_self_condition(workspace, ctx, tid: str, state: Dict, node: Dict, ast: List[Dict]) -> str:
        if state.get("cond_passed") == "true": return "SUCCESS"
            
        status, err_payload = await XBTStepper._eval_condition(workspace, ctx, tid, state, node, ast)

        if node.get("condition"):
            xbt_info.emit("guard", tid=tid, ppid=state.get("ppid", ""),
                          cond=str(node.get("condition") or ""), verdict=status)

        if status == "TRUE":
            await XBTStepper._update_fields(workspace, ctx, tid, {"cond_passed": "true"})
            return "SUCCESS"
        elif status == "FALSE":
            if err_payload: 
                await XBTStepper._update_fields(workspace, ctx, tid, {"jit_cond_err": err_payload})
                return "FAILED"
            # 正常判定为假，退行状态降为 SKIPPED（放过）
            return "SKIPPED" 
        elif status == "NLP":
            return "SUSPENDED"
        return "FAILED"

    @staticmethod
    async def _eval_condition(workspace, ctx, tid: str, state: Dict, cond_node: Dict, ast: List[Dict]) -> Tuple[str, Optional[str]]:
        cond_raw = cond_node.get("condition")
        if not cond_raw: return "TRUE", None
        
        is_cmd_cond = cond_node.get("is_cmd_cond", False)
        clean_cond = str(cond_raw).strip()
        if clean_cond.startswith("["): clean_cond = clean_cond[1:-1].strip()
        
        if not is_cmd_cond and clean_cond.startswith("{{") and clean_cond.endswith("}}"):
            exec_cond = clean_cond[2:-2].strip()
            is_cmd_cond = True
        else:
            exec_cond = clean_cond

        if is_cmd_cond:
            exec_cond = substitute_template_vars(exec_cond, tid, state.get("ppid", "root"))
            try:
                sys_context_lines = [f"# T", f"- tid: {tid}", f"- ppid: {state.get('ppid')}"]
                if state.get("meta"):
                    sys_context_lines.append(f"- meta: {state.get('meta')}")
                sys_context = "\n".join(sys_context_lines)
                res = await workspace.runtime.exec(f'"""{sys_context.strip()}""" | {exec_cond}', ctx=ctx)
                if str(res).strip().lower() == "true": return "TRUE", None
                return "FALSE", None
            except Exception as e:
                return "FALSE", str(e)

        driver = _tag_driver(cond_node) or find_driver(ast, state.get("pointer", ""), state)
        if driver:
            pipeline, err = await XBTStepper._driver_pipeline(workspace, ctx, tid, state, cond_node, ast, driver, "condition")
            if err:
                return "FALSE", f"XBT driver failed: {err}"
            if pipeline.strip().lower() in ("true", "false"):
                return ("TRUE" if pipeline.strip().lower() == "true" else "FALSE"), None
            try:
                res = await workspace.runtime.exec(pipeline, ctx=ctx)
                if is_error_text(res):
                    return "FALSE", f"XBT driver pipeline failed: {str(res).strip()[:300]}"
                out = str(res).strip().lower()
                if out == "true":
                    return "TRUE", None
                if out == "false":
                    return "FALSE", None
                return "FALSE", f"driver pipeline output is not boolean: {out[:200]}"
            except Exception as e:
                return "FALSE", f"XBT driver pipeline trapped: {e}"

        return "NLP", None

    @staticmethod
    async def _handle_control(workspace, ctx, tid: str, state: Dict, node: Dict, dm: str, ast: List[Dict]) -> str:
        children = node.get("children", [])
        if not children:
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "SUCCESS")
            return "- mdio_info: Empty block."

        expected = len(children)
        child_res = {k: v for k, v in state.items() if k.startswith(f"res_{tid}_")}

        if dm == "??":
            chosen = state.get("decision")
            if chosen is None:
                for idx, child in enumerate(children):
                    if state.get(f"cond_{idx}") == "false": continue
                    
                    status, err_payload = await XBTStepper._eval_condition(workspace, ctx, tid, state, child, ast)
                    if child.get("condition"):
                        xbt_info.emit("guard", tid=tid, ppid=state.get("ppid", ""),
                                      idx=idx, cond=str(child.get("condition") or ""),
                                      verdict=status)
                    if status == "TRUE":
                        await XBTStepper._update_fields(workspace, ctx, tid, {"decision": str(idx)})
                        chosen = str(idx)
                        break
                    elif status == "FALSE":
                        if err_payload: await XBTStepper._update_fields(workspace, ctx, tid, {f"jit_err_{idx}": err_payload})
                        # 此路不同，闭锁此项路标，转重试
                        await XBTStepper._update_fields(workspace, ctx, tid, {f"cond_{idx}": "false"})
                        continue
                    elif status == "NLP":
                        # 局部栈帧的隔离
                        await XBTStepper._update_fields(workspace, ctx, tid, {
                            "phase": "C_CHILD", "pending_cond_idx": str(idx), "status": "WAITING"
                        }, suppress_hooks=True)
                        return f"- mdio_info: Suspended at child condition {idx}."
                
                if chosen is None:
                    await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
                    return "- mdio_info: Selector Exhausted."
                    
            chosen_idx = int(chosen)
            res_key = f"res_{tid}_{chosen_idx}"
            
            if res_key in child_res:
                raw_r = child_res[res_key]
                if "SUCCESS" in raw_r:
                    await XBTStepper._bubble_up(workspace, ctx, tid, state, "SUCCESS")
                    return f"- mdio_info: Selector completed with child {chosen_idx}."
                else:
                    # 遭遇死路回滚
                    workspace.write_locator(
                        ctx.wt_uid, "/.mds/proc.md",
                        by_tag(f"PROCESS_TABLE.XBT.{tid}", f"cond_{chosen_idx}"),
                        "/>", "false", auto_scaffold=True,
                    )
                    workspace.write_locator(
                        ctx.wt_uid, "/.mds/proc.md",
                        by_tag(f"PROCESS_TABLE.XBT.{tid}", res_key),
                        "->", "", auto_scaffold=False,
                    )
                    workspace.write_locator(
                        ctx.wt_uid, "/.mds/proc.md",
                        by_tag(f"PROCESS_TABLE.XBT.{tid}", "decision"),
                        "->", "", auto_scaffold=False,
                    )
                    workspace.write_locator(
                        ctx.wt_uid, "/.mds/proc.md",
                        by_tag(f"PROCESS_TABLE.XBT.{tid}", "status"),
                        "/>", "ROLLBACK", auto_scaffold=True,
                    )
                    return "- mdio_info: Selector branch failed, Rolling back."
                
            if f"child_{chosen_idx}_forked" not in state:
                await XBTStepper._update_fields(workspace, ctx, tid, {"status": "PENDING", f"child_{chosen_idx}_forked": "true"}, suppress_hooks=True)
                await XBTStepper._fork_single_child(workspace, ctx, tid, state, children[chosen_idx], chosen_idx, extra_fields={"cond_passed": "true"})
                return "- mdio_info: Selector Forked."
            
            await XBTStepper._update_fields(workspace, ctx, tid, {"status": "PENDING"}, suppress_hooks=True)
            return "- mdio_info: Selector Pending."

        if dm == "==":
            if len(child_res) == expected:
                final = "FAILED" if any("FAILED" in r for r in child_res.values()) else "SUCCESS"
                await XBTStepper._bubble_up(workspace, ctx, tid, state, final)
                return "- mdio_info: Parallel finished."
                
            if "children_forked" not in state:
                await XBTStepper._update_fields(workspace, ctx, tid, {"status": "PENDING", "children_forked": "true"}, suppress_hooks=True)
                spawned = await XBTStepper._fork_children(workspace, ctx, tid, state, children, ast)
                if spawned == 0:
                    await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
                    return "- mdio_error: Parallel fork produced no runnable child."
                return "- mdio_info: Parallel Forked."
                
            await XBTStepper._update_fields(workspace, ctx, tid, {"status": "PENDING"}, suppress_hooks=True)
            return "- mdio_info: Parallel Pending."

        if dm == ">>":
            for idx in range(expected):
                r_key = f"res_{tid}_{idx}"
                if r_key in child_res:
                    if "FAILED" in child_res[r_key]:
                        await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
                        return "- mdio_info: Sequence Broken."
                    
                    # SUCCESS 与 SKIPPED 都继续放行
                    continue 
                    
                if f"child_{idx}_forked" not in state:
                    await XBTStepper._update_fields(workspace, ctx, tid, {"status": "PENDING", f"child_{idx}_forked": "true"}, suppress_hooks=True)
                    await XBTStepper._fork_single_child(workspace, ctx, tid, state, children[idx], idx)
                    return "- mdio_info: Sequence executing."
                    
                await XBTStepper._update_fields(workspace, ctx, tid, {"status": "PENDING"}, suppress_hooks=True)
                return "- mdio_info: Sequence queued."
            
            await XBTStepper._bubble_up(workspace, ctx, tid, state, "SUCCESS")
            return "- mdio_info: Sequence Completed."

        await XBTStepper._bubble_up(workspace, ctx, tid, state, "FAILED")
        return f"- mdio_error: Unknown DM: {dm}"

    @staticmethod
    async def _fork_fail_child(workspace, ctx, p_tid: str, i: int):
        """建世界线/spawn 失败的孩子：在父节写 FAILED（== 汇总按 res_ 计数）。"""
        workspace.write_locator(
            ctx.wt_uid, "/.mds/proc.md",
            by_tag(f"PROCESS_TABLE.XBT.{p_tid}", f"res_{p_tid}_{i}"),
            "/>", "FAILED", auto_scaffold=True,
        )

    @staticmethod
    async def _fork_children(workspace, ctx, p_tid: str, state: Dict, children: List[Dict], ast: List[Dict]):
        """== 分叉：每个孩子按 driver 类型配方 open 世界线后逐个 spawn。

        返回 spawn 成功数；0 = 一个都没起来（调用方立即 FAILED，防父线程永久 PENDING）。
        """
        p_base = state.get("pointer", "0").split(' ')[0]
        bp = state.get("blueprint", "")
        parent_wl = _thread_wl(state)
        parent_type = _thread_wl_type(state)
        drv = f' -driver "{state.get("driver")}"' if state.get("driver") else ""
        spawned = 0
        for i, child in enumerate(children):
            c_tid = f"{p_tid}_{i}"
            ptr = f"{p_base}.{i} {compute_node_hash(child)}"
            driver = find_driver(ast, ptr, state)
            wl = ""
            wl_type = ""
            if driver:
                proto, recipe = recipe_for(driver)
                if parent_wl != "main" and parent_type and parent_type != proto:
                    print(f"[XBT] == 跨世界线介质: {proto!r} vs {parent_type!r}；孩子 {c_tid} FAILED")
                    await XBTStepper._fork_fail_child(workspace, ctx, p_tid, i)
                    continue
                if recipe["worldline"]:
                    child_wl = _fork_wl(parent_wl, p_tid, p_base, i)
                    open_cmd = format_open(driver, child_wl, {"p_tid": p_tid, "p_base": p_base, "index": i})
                    if open_cmd:
                        opened = await workspace.runtime.exec(open_cmd, ctx=ctx)
                        if is_error_text(str(opened)):
                            print(f"[XBT] open 世界线失败: {str(opened)[:160]}；孩子 {c_tid} FAILED")
                            await XBTStepper._fork_fail_child(workspace, ctx, p_tid, i)
                            continue
                    wl = child_wl
                    wl_type = proto
                elif parent_wl != "main":
                    wl = parent_wl
                    wl_type = parent_type
            cmd = f'@XBT -spawn "{c_tid}" -ppid "{p_tid}" -ptr "{ptr}" -bp "{bp}"{drv}'
            if wl:
                cmd += f' -wl "{wl}"'
            if wl_type:
                cmd += f' -wl_type "{wl_type}"'
            if state.get("bb"):
                cmd += f' -bb "{state["bb"]}"'
            result = await workspace.runtime.exec(cmd, ctx=ctx)
            if is_error_text(str(result)):
                print(f"[XBT] spawn 失败: {c_tid}: {str(result)[:160]}")
                await XBTStepper._fork_fail_child(workspace, ctx, p_tid, i)
                continue
            xbt_info.emit("spawn", tid=p_tid, ppid=state.get("ppid", ""), child=c_tid,
                          ptr=str(ptr).split(" ")[0], wl=wl, wl_type=wl_type, driver=driver)
            spawned += 1
        return spawned

    @staticmethod
    async def _fork_single_child(workspace, ctx, p_tid: str, state: Dict, child: Dict, i: int, extra_fields: Dict[str, str] = None):
        p_base = state.get("pointer", "0").split(' ')[0]
        c_hash = compute_node_hash(child)
        c_tid = f"{p_tid}_{i}"
        ptr = f"{p_base}.{i} {c_hash}"
        bp = state.get("blueprint", "")

        drv = f' -driver "{state.get("driver")}"' if state.get("driver") else ""
        wl = "" if _thread_wl(state) == "main" else _thread_wl(state)
        wl_type = _thread_wl_type(state)
        inherit = ""
        if wl:
            inherit += f' -wl "{wl}"'
        if wl_type:
            inherit += f' -wl_type "{wl_type}"'
        if state.get("bb"):
            inherit += f' -bb "{state["bb"]}"'
        await workspace.runtime.exec(f'@XBT -spawn "{c_tid}" -ppid "{p_tid}" -ptr "{ptr}" -bp "{bp}"{drv}{inherit}', ctx=ctx)
        xbt_info.emit("spawn", tid=p_tid, ppid=state.get("ppid", ""), child=c_tid,
                      ptr=str(ptr).split(" ")[0], wl=wl, wl_type=wl_type,
                      driver=state.get("driver", ""))
        if extra_fields:
            for k, v in extra_fields.items():
                workspace.write_locator(
                    ctx.wt_uid, "/.mds/proc.md",
                    by_tag(f"PROCESS_TABLE.XBT.{c_tid}", k),
                    "/>", str(v), auto_scaffold=True,
                )

    @staticmethod
    async def _bubble_up(workspace, ctx, tid: str, state: Dict, result: str):
        ppid = state.get("ppid")
        xbt_info.emit("bubble", tid=tid, ppid=state.get("ppid", ""), res=result)
        
        if ppid and ppid != "root":
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                drill(f"PROCESS_TABLE.XBT.{ppid}"),
                "+>", f"- res_{tid}: {result}\n", auto_scaffold=False,
            )
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                by_tag(f"PROCESS_TABLE.XBT.{ppid}", "status"),
                "/>", "WAKE_UP", auto_scaffold=False,
            )
            await workspace.runtime.exec(f'@XBT -kill "{tid}"', ctx=ctx)
            return

        if result == "FAILED":
            await workspace.runtime.exec(f'@XBT -kill "{tid}" -cascade "true"', ctx=ctx)
            return

        # 轮回寿命结算 (Samsara)
        loop_remain = state.get("loop_remain")
        if not loop_remain:
            bp_sel = state.get("blueprint", "")
            if bp_sel:
                raw_loop = await XBTStepper._read_blueprint(workspace, ctx, bp_sel, "loop")
                loop_val = str_or(raw_loop)   # 循环值按字符串处理
                loop_remain = loop_val if loop_val else "1"
            else:
                loop_remain = "1"

        is_infinite = loop_remain.lower() in ("is_always", "always", "infinite", "inf")
        if not is_infinite:
            try:
                if int(float(loop_remain)) <= 1:
                    await workspace.runtime.exec(f'@XBT -kill "{tid}" -cascade "true"', ctx=ctx)
                    return
            except ValueError:
                await workspace.runtime.exec(f'@XBT -kill "{tid}" -cascade "true"', ctx=ctx)
                return

        fetch_cmd = f"$/.mds/proc.md::PROCESS_TABLE.XBT.*[.tag.re('^{tid}_')]._b:sys_hook_id"
        raw_ids = await workspace.runtime.exec(fetch_cmd, ctx=ctx)
        
        respawn_cmds = []
        if raw_ids and "- mdio_error:" not in raw_ids:
            for h_id in str(raw_ids).strip().split('\n'):
                h_id = h_id.strip()
                if h_id.isdigit(): 
                    respawn_cmds.append(f'hook -del {h_id}')

        # 彻底的轮回剔骨：清算所有非核心记忆
        respawn_cmds.extend([
            f'-> /.mds/proc.md::PROCESS_TABLE.XBT.*[.tag.re("^{tid}_")]',
            f'-> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}.*[ .tag.re("^(res_|child_|cond_|decision|jit_|result|phase|pending_cond_idx)") ]'
        ])

        if not is_infinite:
            respawn_cmds.append(f'"{loop_remain}" | calc "_ - 1" | /> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}._b:loop_remain')
        else:
            respawn_cmds.append(f'"is_always" | /> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}._b:loop_remain')

        respawn_cmds.extend([
            f'"0" | /> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}._b:pointer',
            f'"WAKE_UP" | /> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}._b:status'
        ])
        
        await workspace.runtime.exec(" ; ".join(respawn_cmds), ctx=ctx)
        await XBTStepper.step(workspace, ctx, tid)

    @staticmethod
    async def _root_state(workspace, ctx, tid: str, state: Optional[Dict[str, str]]):
        """沿 ppid 找到真正的 root tid/state；root tid 可能含下划线，不能靠 split。"""
        cur_tid, cur_state = str(tid), dict(state or {})
        seen = set()
        while True:
            ppid = str(cur_state.get("ppid") or "")
            if not ppid or ppid == "root" or ppid in seen:
                break
            seen.add(ppid)
            parent_state = await XBTStepper._read_state(workspace, ctx, ppid)
            if not parent_state:
                break
            cur_tid, cur_state = ppid, parent_state
        return cur_tid, cur_state

    @staticmethod
    async def _read_state(workspace, ctx, tid: str) -> Optional[Dict[str, str]]:
        raw = await workspace.runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
        if not raw or "- mdio_error:" in raw: return None
        # 引擎信封 + bullets_to_kv；值里的字面 \n 还原成真换行。
        kv = _kv_from_state_text(workspace, raw)
        return {k: v.replace('\\n', '\n') for k, v in kv.items()}

    @staticmethod
    async def _update_fields(workspace, ctx, tid: str, updates: Dict[str, Any], suppress_hooks: bool = False):
        if not updates: return
        for field, value in updates.items():
            workspace.write_locator(
                ctx.wt_uid, "/.mds/proc.md",
                by_tag(f"PROCESS_TABLE.XBT.{tid}", field),
                "/>", str(value), auto_scaffold=True, suppress_hooks=suppress_hooks,
            )

    @staticmethod
    async def _read_blueprint(workspace, ctx, selector: str, key: str = "xbt") -> str:
        if key == "xbt":
            result = await workspace.runtime.exec(f"${selector}._code:xbt", ctx=ctx)
        else:
            result = await workspace.runtime.exec(f"${selector}:{key}", ctx=ctx)
        if not result or "- mdio_error:" in result:
            result = await workspace.runtime.exec(f"${selector}", ctx=ctx)
        return result if result and "- mdio_error:" not in result else ""

    @staticmethod
    async def _read_blueprint_node(workspace, ctx, selector: str) -> str:
        """读蓝图节点原文（含 - BB: 行与 xbt 代码块）；解析仍走 _read_blueprint。"""
        try:
            result = await workspace.runtime.exec(f"${selector}", ctx=ctx)
        except Exception:
            return ""
        text = str(result or "")
        return "" if not text or "- mdio_error:" in text else text
