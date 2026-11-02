// @ts-check
/**
 * wt 清单 + README 元数据：**结构由引擎解析**（`$ <doc> | tojson` 的 AST），这里只做 AST 遍历，
 * 不做文本/行解析（仓库纪律：md 的结构与内容只有一个读取来源 = 引擎）。
 * 纯函数，无副作用；取件由调用方走 client.readDocAst（HTTP 通道，可读别的工作树）。
 */

/** AST 节点 → 纯文本（t 可能是 inline 节点数组；cd 是行内代码；ct 是子节点或字符串）。 */
export function flattenText(node) {
  if (node == null) return "";
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(flattenText).join("");
  if (typeof node !== "object") return String(node);
  if (typeof node.cd === "string") return node.cd;
  if (node.nt === "sb") return " ";
  if (node.ct != null && typeof node.ct !== "object") return flattenText(node.ct);
  if (node.ct != null) return flattenText(node.ct);
  if (node.t != null) return flattenText(node.t);
  return "";
}

/** `- desc:` 的值：其 ct 里的 bq 块 → 按行取（h 首行当展示名、其余行当正文）。 */
function bqLinesOf(b) {
  const out = [];
  const push = (n) => {
    if (!n || typeof n !== "object") return;
    if (n.nt === "h") { const t = flattenText(n.t).trim(); if (t) out.push({ head: true, text: t }); return; }
    if (n.nt === "p") { const t = flattenText(n).trim(); if (t) out.push({ text: t }); return; }
    if (n.nt === "bq" || (Array.isArray(n.ct) && n.ct.length && n.nt !== "b")) {   // 透明容器
      for (const c of n.ct || []) push(c);
      return;
    }
    const t = flattenText(n).trim();
    if (t) out.push({ text: t });
  };
  for (const n of b.ct || []) {
    if (n && n.nt === "bq") for (const c of n.ct || []) push(c);
    else if (n && n.nt !== "b") push(n);
  }
  return out;
}

/**
 * README 的 `# app` 节 → 元数据。只取 `# app` 这个 h1 的**直接子级**里的 bullet；
 * 因为 HS 拓扑里 `## 说明` 等后续章节是 h1 的子节点，遍历时不会被当成本节 bullet
 * （上一版行解析在「只认下个 # 级标题截断」上出过边界 bug，引擎 AST 从结构上消除它）。
 *
 * 约定：name 必需；version/author/repo/icon/tag/desc 可选；tag 可多条（`app` = 默认列表 / 首页）。
 * desc 的值是引用块，首行 `# 展示名` 当卡片标题、其余行当正文。
 */
export function aboutFromAst(ast) {
  const out = { hasApp: false, name: "", version: "", author: "", repo: "", icon: "", tags: [], descTitle: "", descText: "" };
  const root = ast && Array.isArray(ast.ct) ? ast.ct : [];
  const h1 = root.find((n) => n && n.nt === "h" && Number(n.lv) === 1 && flattenText(n.t).trim().toLowerCase() === "app");
  if (!h1) return out;
  out.hasApp = true;
  const handle = (b) => {
    if (!b || b.nt !== "b") return;
    const key = flattenText(b.t).trim().toLowerCase();
    if (!key) return;
    if (key === "desc") {
      const lines = bqLinesOf(b);
      if (lines.length) {
        if (lines[0].head) { out.descTitle = lines[0].text; out.descText = lines.slice(1).map((l) => l.text).join("\n").trim(); }
        else out.descText = lines.map((l) => l.text).join("\n").trim();
      }
      return;
    }
    const val = flattenText(b.ct).trim();
    if (key === "name") out.name = val;
    else if (key === "version") out.version = val;
    else if (key === "author") out.author = val;
    else if (key === "repo") out.repo = val;
    else if (key === "icon") out.icon = val;
    else if (key === "tag" || key === "tags") { for (const t of val.split(/[,，\s]+/)) if (t) out.tags.push(t); }
  };
  for (const n of h1.ct || []) {
    if (!n) continue;
    if (n.nt === "bl") for (const b of n.ct || []) handle(b);
    else if (n.nt === "b") handle(n);      // 单条 bullet 的兜底（无 bl 壳）
  }
  return out;
}

/** `/.mds/worktrees.md` → 子条目（引擎 AST 遍历；条目是 `# WORKTREES` 下的 h2，取 `uid` bullet）。 */
export function entriesFromAst(ast) {
  const rows = [];
  const root = ast && Array.isArray(ast.ct) ? ast.ct : [];
  const h1 = root.find((n) => n && n.nt === "h" && Number(n.lv) === 1 && flattenText(n.t).trim().toUpperCase() === "WORKTREES");
  if (!h1) return rows;
  const findUid = (node) => {
    for (const c of (node && node.ct) || []) {
      if (!c || typeof c !== "object") continue;
      if (c.nt === "b" && flattenText(c.t).trim() === "uid") { const v = flattenText(c.ct).trim(); if (v) return v; }
      const deep = findUid(c);
      if (deep) return deep;
    }
    return "";
  };
  for (const n of h1.ct || []) {
    if (!n || n.nt !== "h" || Number(n.lv) !== 2) continue;
    const title = flattenText(n.t).trim();
    if (!title) continue;
    rows.push({ title, uid: findUid(n) });
  }
  return rows;
}

/** 一行 wt（树节点）。title 取 README 展示名 → 条目名 → uid。 */
export function makeRow({ title, uid, parent, depth, meta }) {
  const m = meta || {};
  const display = String(m.descTitle || m.name || title || uid || "").trim();
  return {
    title: display || uid,
    raw: title || "",
    uid: String(uid || ""),
    parent: parent || "",
    depth: Number(depth) || 0,
    appName: m.name || "",
    version: m.version || "",
    author: m.author || "",
    repo: m.repo || "",
    tags: Array.isArray(m.tags) ? m.tags : [],
    tagApp: Array.isArray(m.tags) && m.tags.indexOf("app") >= 0,
    desc: m.descText || "",
  };
}

/** 把根树/当前 wt 补进清单（读不到注册表时菜单也别塌成一条）。
 *  根 wt 恒为根；当前 wt 缺失时挂到根下面（不是再起一个根）。 */
export function withCurrent(list, wt, rootWt) {
  const all = Array.isArray(list) ? list.slice() : [];
  const root = rootWt ? String(rootWt) : "";
  const cur = wt ? String(wt) : "";
  if (root && !all.some((w) => w.uid === root)) all.unshift(makeRow({ title: root, uid: root, parent: "", depth: 0, meta: null }));
  if (cur && !all.some((w) => w.uid === cur)) {
    const under = root && root !== cur ? root : "";
    all.push(makeRow({ title: cur, uid: cur, parent: under, depth: under ? 1 : 0, meta: null }));
  }
  return all;
}
