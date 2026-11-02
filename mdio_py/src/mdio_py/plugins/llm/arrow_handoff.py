"""arrow-only 世界线 handoff:HIS 原文 → messages(一条 arrow script + Python walk/fold)。

arrow script 里用 temp view 把结构装配推到 DuckDB:
  wl_rec(记录) / wl_ctl(控件) / wl_cfld(控件字段) / wl_csum(控件摘要)
  / wl_rfld(记录字段) / wl_rbq(直接子级 bq 载荷)
final SELECT 每条 record 一行(fields_json / controls_json / payload_raw), Python 只做
wl 开集 + PDS walk + compact/prune 归约 + message fold;不产出 md 链、不写源。
"""

from __future__ import annotations

import bisect
import json
import os
import re

from mdio_py.core.mem_slot import begin_exec_frame, current_temp_frame, end_exec_frame

ROLES = {"user", "assistant", "thinking", "tool_call", "tool_result"}
MESSAGE_FIELDS = {"name", "id", "tool", "status"}
CONTROL_FIELDS = {"type", "path", "next"}

_RECORD_SQL = """
with rec as (
  select n.node, n.tag as tg, n.section as sec_node, s.start_line as st
  from nodes n join span s on s.node=n.node
  where n.type='bullet' and n.role='shell' and n.mark='-'
    and n.tag in ('user','assistant','thinking','tool_call','tool_result')
),
kid as (select parent, child, pos from children),
ctl as (
  select k.parent as rec, k.child as ctl, cn.tag as tg, cn.mark as mk, s.start_line as st
  from kid k join nodes cn on cn.node=k.child join span s on s.node=k.child
  where cn.type='bullet' and cn.mark in ('+','*')
    and __WL_FILTER__
),
cfld as (
  select c.ctl, f.tag as tg, f.content as val
  from ctl c join kid k on k.parent=c.ctl join nodes f on f.node=k.child
  where f.type='bullet' and f.tag in ('type','path','next')
),
cf as (
  select ctl,
    max(case when tg='type' then val end) as tp,
    max(case when tg='path' then val end) as pt,
    max(case when tg='next' then val end) as nx
  from cfld group by ctl
),
cs as (
  select c.ctl, string_agg(p.value, chr(10) order by k.pos) as raw
  from ctl c join kid k on k.parent=c.ctl
  join nodes b on b.node=k.child and b.type='bq'
  left join pj p on p.node=b.node and p.pj='raw'
  group by c.ctl
),
ctl2 as (
  select c.rec, c.tg, c.mk, c.st,
         coalesce(cf.tp,'') as tp, coalesce(cf.pt,'') as pt, coalesce(cf.nx,'') as nx,
         coalesce(cs.raw,'') as summary_raw
  from ctl c left join cf on cf.ctl=c.ctl left join cs on cs.ctl=c.ctl
),
ctl3 as (
  select ctl2.*, regexp_extract(ctl2.pt, '([^.]+)$', 1) as target_heading
  from ctl2
),
ctl4 as (
  select ctl3.*, s.heading as target_section,
         s.start_line as target_start, s.end_line as target_end,
         (s.heading is not null) as target_exists
  from ctl3 left join sections s on s.heading = ctl3.target_heading
),
ctrl as (
  select rec, json_group_array(json_object(
    'tag', tg, 'mk', mk, 'st', st,
    'type', tp, 'path', pt, 'next', nx, 'summary_raw', summary_raw,
    'target_section', target_section, 'target_start', target_start,
    'target_end', target_end, 'target_exists', target_exists
  )) as controls_json
  from ctl4 group by rec
),
rf as (
  select k.parent as rec, json_group_object(f.tag, f.content) as fields_json
  from kid k join nodes f on f.node=k.child
  where f.type='bullet' and f.tag in ('name','id','tool','status')
  group by k.parent
),
rb as (
  select rec, rawtxt from (
    select k.parent as rec, p.value as rawtxt,
           row_number() over (partition by k.parent order by k.pos desc) as rn
    from kid k join nodes b on b.node=k.child and b.type='bq'
    left join pj p on p.node=k.child and p.pj='raw'
  ) t where rn=1
),
secs as (select section, heading, start_line, end_line from sections)
select r.node, r.tg, h.heading as sec, r.st,
  coalesce(h.start_line, 0) as sec_start, coalesce(h.end_line, 0) as sec_end,
  coalesce(rf.fields_json, '{}') as fields_json,
  coalesce(ctrl.controls_json, '[]') as controls_json,
  rb.rawtxt as payload_raw
from rec r
left join secs h on h.section=r.sec_node
left join rf on rf.rec=r.node
left join ctrl on ctrl.rec=r.node
left join rb on rb.rec=r.node
order by r.st
"""

_RECORD_SQL_NOCTL = """
with rec as (
  select n.node, n.tag as tg, n.section as sec_node, s.start_line as st
  from nodes n join span s on s.node=n.node
  where n.type='bullet' and n.role='shell' and n.mark='-'
    and n.tag in ('user','assistant','thinking','tool_call','tool_result')
),
kid as (select parent, child, pos from children),
rf as (
  select k.parent as rec, json_group_object(f.tag, f.content) as fields_json
  from kid k join nodes f on f.node=k.child
  where f.type='bullet' and f.tag in ('name','id','tool','status')
  group by k.parent
),
rb as (
  select rec, rawtxt from (
    select k.parent as rec, p.value as rawtxt,
           row_number() over (partition by k.parent order by k.pos desc) as rn
    from kid k join nodes b on b.node=k.child and b.type='bq'
    left join pj p on p.node=k.child and p.pj='raw'
  ) t where rn=1
),
secs as (select section, heading, start_line, end_line from sections)
select r.node, r.tg, h.heading as sec, r.st,
  coalesce(h.start_line, 0) as sec_start, coalesce(h.end_line, 0) as sec_end,
  coalesce(rf.fields_json, '{}') as fields_json,
  '[]' as controls_json,
  rb.rawtxt as payload_raw
from rec r
left join secs h on h.section=r.sec_node
left join rf on rf.rec=r.node
left join rb on rb.rec=r.node
order by r.st
"""


def _record_sql(wl: str) -> str:
    """wl 为空 => 不需要控件信息(基线);否则 SQL 只保留会开门的 +/* 控件。"""
    w = str(wl or "").strip()
    if not w:
        return _RECORD_SQL_NOCTL
    esc = w.replace("'", "''")
    flt = "(cn.tag = '%s' or starts_with('%s', cn.tag || '_'))" % (esc, esc)
    return _RECORD_SQL.replace("__WL_FILTER__", flt)


_AVAILABLE = None


def available() -> bool:
    """arrow 扩展 + duckdb 是否可用(只探测一次)。"""
    global _AVAILABLE
    if _AVAILABLE is not None:
        return _AVAILABLE
    try:
        import duckdb  # noqa: F401
        from mdio_py.std import arrow_ops
        _AVAILABLE = os.path.isfile(arrow_ops._ext_path())
    except Exception:
        _AVAILABLE = False
    return _AVAILABLE


def _txt(v) -> str:
    return "" if v is None else str(v)


_BQ_LINE_RE = re.compile(r"^(\s*)>( ?)(.*)$")


def _bq_text(row: dict) -> str:
    """bq 节点的载荷原文:优先 raw 面按一层引用前缀剥回, 退回 content 面。

    content 面会把行首空白/嵌套引用符规范化(live 实测: "> 依..." 的 "> " 被吃掉),
    raw 面按 md 源文本保留, 剥掉一层 "  > " 即原文。
    """
    raw = _txt(row.get("rawtxt"))
    out = []
    for ln in raw.rstrip("\n").split("\n"):
        m = _BQ_LINE_RE.match(ln)
        seg = m.group(3) if m else ln
        # payload 的口径: 纯空白内容行 -> ""; 其余保留行尾空白
        # (嵌套引用 "> " 的尾空格在 real chat 里会被保留, 不能一律 rstrip)。
        out.append("" if seg.strip() == "" else seg)
    return "\n".join(out)


async def _fetch_records(his_text: str, workspace, ctx, wl: str = "") -> list:
    """一条 arrow script(单个 WITH 查询):DuckDB 内装配记录/控件/载荷。"""
    from mdio_py.std import arrow_ops

    own = current_temp_frame() is None
    token = begin_exec_frame(workspace) if own else None
    try:
        out = await arrow_ops.arrow(
            workspace, his_text, [], {"-duckdb": _record_sql(wl), "-json": True}, ctx)
    finally:
        if own:
            end_exec_frame(token)
    out = str(out or "")
    if out.lstrip().startswith("- mdio_error:"):
        raise RuntimeError("arrow 读取失败: " + out[:240])
    rows = json.loads(out)
    if not isinstance(rows, list):
        raise RuntimeError("arrow -json 输出不是 rows: %r" % (type(rows),))
    return rows


def build_plan(rows: list) -> list:
    """script final SELECT 的 rows -> records(walk/reduce 直接消费)。

    口径:只认直接子级字段/控件/bq;payload 取最后一个直接 bq
    (raw 面剥一层引用前缀);控件按文档序。
    """
    records = []
    for r in rows:
        fields = json.loads(r.get("fields_json") or "{}")
        fields = {k: _txt(v).strip() for k, v in fields.items() if k in MESSAGE_FIELDS}
        controls = []
        for c in json.loads(r.get("controls_json") or "[]"):
            cf = {}
            for k in CONTROL_FIELDS:
                v = _txt(c.get(k)).strip()
                if v:
                    cf[k] = v
            controls.append({
                "tag": _txt(c.get("tag")).strip(),
                "mk": _txt(c.get("mk")).strip(),
                "start_line": int(c.get("st") or 0),
                "fields": cf,
                "summary": _bq_text({"rawtxt": c.get("summary_raw") or ""}),
                "target_section": _txt(c.get("target_section")).strip(),
                "target_start": int(c.get("target_start") or 0),
                "target_end": int(c.get("target_end") or 0),
                "target_exists": bool(c.get("target_exists")),
            })
        controls.sort(key=lambda x: x["start_line"])
        records.append({
            "msg_role": _txt(r.get("tg")).strip(),
            "section": (_txt(r.get("sec")).strip() or None),
            "start_line": int(r.get("st") or 0),
            "sec_start": int(r.get("sec_start") or 0),
            "sec_end": int(r.get("sec_end") or 0),
            "fields": fields,
            "content": _bq_text({"rawtxt": r.get("payload_raw") or ""}),
            "controls": controls,
        })
    records.sort(key=lambda x: x["start_line"])
    return records


def walk(records: list, wl: str):
    """PDS 自然路径(branch 甩尾, join 回跳, star 记录)。

    records 按 start_line 平铺；段边界来自 SQL sections（record 行带 sec_start/sec_end，
    gate 行带 target_start/target_end/target_exists）。
    """
    recs = sorted(records, key=lambda r: r["start_line"])
    starts = [r["start_line"] for r in recs]
    trace, stars = [], []
    if not recs:
        return trace, stars
    main_recs = [r for r in recs if r.get("section") == "main"]
    if not main_recs:
        return trace, stars
    main_start = int(main_recs[0].get("sec_start") or main_recs[0]["start_line"])
    main_end = int(main_recs[0].get("sec_end") or main_recs[0]["start_line"])

    frames = [{"sec": "main", "idx": bisect.bisect_left(starts, main_start),
               "end_line": main_end, "seg_start": 0, "alive": True, "end": -1}]
    guard = 0
    while frames:
        guard += 1
        if guard > 500000:
            raise ValueError("walk 步数上限:branch/join 拓扑可能有环")
        fr = frames[-1]
        if (not fr["alive"] or fr["idx"] >= len(recs)
                or recs[fr["idx"]]["start_line"] >= fr["end_line"]):
            fr["end"] = len(trace)
            frames.pop()
            continue
        rec = recs[fr["idx"]]
        if rec.get("section") != fr["sec"]:
            fr["idx"] += 1
            continue
        moved = False
        for ctl in rec["controls"]:
            tag = ctl["tag"]
            if not wl_open(tag, wl):
                continue
            mk = _txt(ctl.get("mk")).strip()
            f = ctl["fields"]
            if mk == "+":
                kind = "branch" if f.get("type", "").strip() == "branch" else "join"
                target = _txt(ctl.get("target_section")).strip()
                if not target or not ctl.get("target_exists"):
                    raise ValueError(
                        "gate path 解析不出同文档段: tag=%r path=%r"
                        % (tag, f.get("path", "")))
                if kind == "branch":
                    fr["alive"] = False
                else:
                    fr["idx"] += 1
                tstart = int(ctl.get("target_start") or 0)
                tend = int(ctl.get("target_end") or 0)
                frames.append({"sec": target,
                               "idx": bisect.bisect_left(starts, tstart),
                               "end_line": tend,
                               "seg_start": len(trace), "alive": True, "end": -1})
                moved = True
                break
            if mk == "*":
                kind = "compact" if f.get("type", "").strip() == "compact" else "prune"
                try:
                    n = int(f.get("next", "0") or 0)
                except Exception:
                    n = 0
                stars.append({
                    "tag": tag, "kind": kind, "n": n,
                    "summary": ctl.get("summary", ""),
                    "trace_pos": len(trace),
                    "seg_start": fr["seg_start"],
                    "frame": fr,
                })
        if moved:
            continue
        trace.append(rec)
        fr["idx"] += 1
    return trace, stars


def reduce_trace(trace: list, stars: list) -> list:
    """compact/prune 归约: 段前 n 条换成一条摘要; next=0 插在 gate 记录之后。"""
    tokens = [{"o": i, "rec": trace[i]} for i in range(len(trace))]
    for st in stars:
        if st["kind"] == "prune":
            continue
        gi = next((i for i, t in enumerate(tokens) if t["o"] == st["trace_pos"]), None)
        if gi is None:
            continue
        synthetic = {"msg_role": "user", "fields": {}, "content": st.get("summary", "")}
        if st["n"] <= 0:
            tokens.insert(gi + 1, {"o": "sum", "rec": synthetic})
            continue
        scope = [i for i, t in enumerate(tokens)
                 if isinstance(t["o"], int)
                 and st["seg_start"] <= t["o"] < st["frame"]["end"]]
        if not scope:
            continue
        si = scope[0]
        n_eff = min(st["n"], len(scope))
        tokens[si:si + n_eff] = [{"o": "sum", "rec": synthetic}]
    return [t["rec"] for t in tokens]


def _to_messages(recs: list) -> list:
    return records_to_messages([
        {"role": r["msg_role"],
         "fields": r.get("fields") or {},
         "content": r.get("content") or ""}
        for r in recs
    ])


async def handoff_records(his_text: str, wl: str, workspace, ctx) -> list:
    """HIS 原文 -> 当前 wl 下 reduce 后的 Content 记录（写侧取可见记录用）。

    返回的是 record dict（msg_role / fields / content），不含 control；与
    handoff() 的 messages 共用同一 walk/reduce，顺序一致。空 HIS 返回 []。
    """
    if not str(his_text or "").strip():
        return []
    rows = await _fetch_records(his_text, workspace, ctx, wl)
    records = build_plan(rows)
    trace, stars = walk(records, wl)
    return reduce_trace(trace, stars)


async def handoff(his_text: str, wl: str, workspace, ctx) -> list:
    """HIS 原文 -> OpenAI messages。空 HIS 返回 []; 其余错误上抛给调用方回退。"""
    return _to_messages(await handoff_records(his_text, wl, workspace, ctx))



# 纯语义 helper



def wl_open(label: str, wl: str) -> bool:
    """开集判定:label 是 wl 的祖先(左前缀)或相等。空 wl 不开门(ε 基线)。"""
    w = str(wl or "").strip()
    lab = str(label or "").strip()
    if not w or not lab:
        return False
    return lab == w or w.startswith(lab + "_")


def target_section(path: str) -> str:
    """control.path -> 段名。path 形如 '/wl.md::HIS.b1' 或 '$current_path.md::..*HIS.b1'。"""
    p = str(path or "").strip()
    if not p:
        return ""
    if "::" in p:
        p = p.rsplit("::", 1)[1].strip()
    if not p:
        return ""
    tail = p.rstrip(". ")
    tail = tail.replace("*", "")
    points = [x for x in tail.split(".") if x]
    return points[-1].strip() if points else ""


def _repair(messages: list) -> list:
    tc_result_ids = {m["tool_call_id"] for m in messages if m.get("role") == "tool"}
    out = []
    for msg in messages:
        if msg.get("role") == "assistant":
            msg.setdefault("reasoning_content", "")
            if "tool_calls" in msg:
                valid = [tc for tc in msg["tool_calls"] if tc["id"] in tc_result_ids]
                if valid:
                    msg["tool_calls"] = valid
                    out.append(msg)
                elif msg.get("content"):
                    out.append({k: v for k, v in msg.items() if k != "tool_calls"})
            else:
                out.append(msg)
        elif msg.get("role") == "tool":
            if msg.get("tool_call_id") in {
                tc["id"] for m in messages
                if m.get("role") == "assistant" and "tool_calls" in m
                for tc in m["tool_calls"]
            }:
                out.append(msg)
        else:
            out.append(msg)
    return out


def _tc_arguments(fields: dict, content: str) -> str:
    tool = (fields or {}).get("tool")
    if tool:
        return json.dumps({tool: content}, ensure_ascii=False)
    return content.strip() or "{}"


def records_to_messages(records: list) -> list:
    messages: list[dict] = []
    pending_reasoning = ""
    seen_tool_ids: set = set()
    for rec in records:
        role, content, fields = rec["role"], rec["content"], rec["fields"]
        if role == "user":
            messages.append({"role": "user", "content": content})
        elif role == "thinking":
            pending_reasoning = content
        elif role == "assistant":
            messages.append({"role": "assistant", "content": content or " ",
                             "reasoning_content": pending_reasoning})
            pending_reasoning = ""
        elif role == "tool_call":
            tc = {
                "id": fields.get("id") or "fallback_%d" % len(messages),
                "type": "function",
                "function": {"name": fields.get("name") or "execute_mdio",
                             "arguments": _tc_arguments(fields, content)},
            }
            if messages and messages[-1].get("role") == "assistant":
                messages[-1].setdefault("tool_calls", []).append(tc)
            else:
                messages.append({"role": "assistant", "content": None,
                                 "tool_calls": [tc]})
        elif role == "tool_result":
            tc_id = fields.get("id") or ""
            if tc_id and tc_id in seen_tool_ids:
                continue
            seen_tool_ids.add(tc_id)
            messages.append({"role": "tool", "tool_call_id": tc_id,
                             "name": fields.get("name", "execute_mdio"),
                             "content": content})
    return _repair(messages)