// @ts-check
/**
 * **认领与可见性的策略**（app 层 · 认领与可见性归 `basic_render`）。
 *
 * 规则（与 `ui_core/store/uiStore.gates` 语义一致）：
 *   ① 认领：`activeClaims` **从表尾往前扫、第一个命中即胜者**（= 声明序「后写覆盖」）；**不继承容器**。
 *   ② 可见：`exclude` 优先；没有 `include` 条目 ⇒ 全可见；否则 `include` 命中者可见；两种都用**容器跟孩子走**。
 * 谓词不搬：`claimMatches`（路径门控 + 候选面 + 链上命中）是**能力面**，仍住框架，经 `read` 交给本层。
 */
export function createClaimEval(read) {
  if (!read || typeof read.claimMatches !== "function") return null;
  const claimMatches = read.claimMatches;
  return function claimEval(store, node, chain, parent) {
    // ① 认领：表尾往前扫，第一个命中即胜者
    //    **作用域不在这层预过滤**：条目 path 与「节点所属文档（chain 的 doc_path 段）」的比较在谓词
    //    `claimMatches` 里（多篇同页时 route 表达不了）；这里扫全表。
    let claim = null;
    const list = store.renderClaims;
    for (let i = list.length - 1; i >= 0; i -= 1) {
      const c = list[i];
      if (claimMatches(c, node, chain, parent)) { claim = { name: c.name }; break; }
    }
    // ② 可见：exclude 优先 / 无 include 全可见 / 否则 include 命中者可见（都带容器语义）
    const vlist = store.vis;
    const hit = (name) => {
      for (let i = vlist.length - 1; i >= 0; i -= 1) {
        const e = vlist[i];
        if (!e || e.name !== name || e.docLevel) continue;
        if (claimMatches(e, node, chain, parent, { containers: true })) return true;
      }
      return false;
    };
    let visible = true;
    if (hit("exclude")) visible = false;
    else {
      let incN = 0;
      for (let i = 0; i < vlist.length; i += 1) { const e = vlist[i]; if (e && e.name === "include" && !e.docLevel) incN += 1; }
      visible = !incN ? true : hit("include");
    }
    return { claim, visible };
  };
}
export default createClaimEval;

/**
 * **链上递降**（语义与框架原 `ui_core/uiMd.evalOnChain` 一致）：
 *   · 关系步（Drill/Wildcard/Recursive*）= 沿链下降（容器透明 gap 规则与引擎一致）；
 *   · 同层步（Filter/TYPECHECK/AND/OR/NOT）= **交给能力 `apply`**（不另写一遍过滤）；
 *   · `opts.containers === true` ⇒ 容器跟孩子走（可见性用）。
 *   空 IR ⇒ `null`（没有选择器就没有判定）；畸形 / Slice / tf(limit>0) / 未知动词 ⇒ **抛错**（不静默）。
 *   原语由框架经交付面 `walkPrims` 给（不是 import ui_core —— 层间不互相 import）。
 */
export function createChainWalk(prims) {
  if (!prims || typeof prims.apply !== "function" || typeof prims.filterOf !== "function") return null;
  const apply = prims.apply, filterOf = prims.filterOf, textOf = prims.textOf;
  const isSection = prims.isSection, isContainer = prims.isContainer;
  const LOCAL_FILTER_TAGS = prims.localFilterTags || [];
  const MISSING = new Set();
  return function chainWalk(ir, chain, opts) {
    const steps = Array.isArray(ir) ? ir : [];
    if (!steps.length) return null;
    if (!Array.isArray(chain) || chain.length < 2) return false;
    const withContainers = !opts || opts.containers !== false;
    const last = chain.length - 1;
    const P = [];
    for (const st of steps) {
      if (!Array.isArray(st)) throw new Error("chainWalk：IR 里出现非数组 step（畸形 IR）：" + JSON.stringify(st));
      const t = st[0];
      if (t === "Drill" || t === "Wildcard" || t === "RecursiveDrill" || t === "RecursiveWildcard") { P.push({ k: t, name: st[1], filters: null }); continue; }
      if (t === "TraversalFilter") {
        if (Number(st[3] || 0) > 0) throw new Error("chainWalk：uimd 的 sel 不许 TraversalFilter(limit>0)（装载期应已拒；此处响亮报错）");
        const inner = st[1];
        const ik = Array.isArray(inner) ? inner[0] : null;
        if (ik !== "Drill" && ik !== "Wildcard" && ik !== "RecursiveDrill" && ik !== "RecursiveWildcard") throw new Error("chainWalk：TraversalFilter 的内层不是 traversal（" + String(ik) + "）");
        P.push({ k: ik, name: inner[1], filters: st[2] || [] });
        continue;
      }
      if (LOCAL_FILTER_TAGS.indexOf(t) >= 0) { P.push({ k: "same", step: st }); continue; }
      throw new Error("chainWalk：不认识的动词 " + String(t) + "（装载期应已拒；此处响亮报错）");
    }
    let S = new Set([0]);
    for (const p of P) {
      const next = new Set();
      const f = p.filters && p.filters.length ? filterOf(p.filters.length === 1 ? p.filters[0] : ["AND", p.filters], MISSING) : null;
      if (p.k === "same") {
        for (const q of S) if (apply([chain[q]], [p.step]).hits.length > 0) next.add(q);
      } else if (p.k === "RecursiveDrill" || p.k === "RecursiveWildcard") {
        for (const q of S) for (let r = q + 1; r <= last; r += 1) {
          const w = chain[r];
          if (isContainer(w.nt)) continue;
          if (p.k === "RecursiveDrill" && !(isSection(w) && textOf(w) === p.name)) continue;
          if (f && !f(w)) continue;
          next.add(r);
        }
      } else {
        for (const q of S) for (const r of [q + 1, q + 2]) {
          if (r > last) continue;
          const gap = r - q - 1;
          if (gap > 1) continue;
          if (gap === 1 && !isContainer(chain[q + 1] && chain[q + 1].nt)) continue;
          const w = chain[r];
          if (isContainer(w.nt) && gap !== 1) continue;
          if (p.k === "Drill" && textOf(w) !== p.name) continue;
          if (f && !f(w)) continue;
          next.add(r);
        }
      }
      S = next;
      if (!S.size) return false;
    }
    if (S.has(last)) return true;
    if (!withContainers) return false;
    if (!isContainer(chain[last].nt)) return false;
    for (const q of S) if (q < last) return true;
    return false;
  };
}
