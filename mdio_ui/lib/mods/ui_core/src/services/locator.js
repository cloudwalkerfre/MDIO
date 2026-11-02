// @ts-check
/**
 * locator —— **位置语法**（框架面）：把声明里的位置串解析成 {scope, wt, file, sel}，并按 wt 链解析 ui 件。
 *
 * 从 `demo/src/entry.client.ts` 搬来（装配收口）：
 * 位置串是**声明与渲染件之间的公共语言**，解析它不属于"装配"；装配点只该"读 ?wt/?app → 装载 → 挂 app"。
 *
 * **已裁（业主 · 窄版禁 2）**：`禁 2` 收窄为「**选择器编译必须走引擎**」（`sel compile` → `parse_code`，已是现状）；
 *   **地址串的切分属前端装配职责** ⇒ 本文件的 `parseSpec` 手写切分**不违规**。它仍是**唯一**的解析面：
 *   将来若改成引擎解析，也只在这里动。（「待裁」注记的结论：不悬空。）
 */

/**
 * ui 空间的 esmx 模块名：wt 链上的**每个** ui 目录是一个 esmx 模块，宿主用 modules.links
 * 把它们挂进同一张 import map；键 = 各模块 manifest 里的 identifier = '<模块名>/<导出名>'
 * （实测 wtui_genesis/std.js → /wtui_genesis/std.js.<hash>.final.mjs）。
 * uid 小写化：npm 包名不允许大写，而 import map 键区分大小写 ⇒ 模块名与 uid 的映射只有这一条规则
 * （各模块 package.json 的 name 逐字照此命名）。
 * @param {string} uid
 * @returns {string}
 */
export const uiModuleOf = (uid) => "wtui_" + String(uid || "").toLowerCase();

/**
 * **位置语法（规范形）**：`<wt>.<scope>::<file_path>[::<scope_sel>]`
 *   · 前导点 = **本 wt**（省略 wt 也行）：`.ui::stats.jsx`、`.md::/stats.md::NOTE.面板`
 *   · ui scope **没有 sel** ⇒ `.ui::<file>`；md scope 的 sel 就是 md 选择器
 *   · 显式写 wt 亦可：`c8page01.ui::stats.jsx`（ui 件可跨 wt ⇒ 走 `wtui_<uid>/<件>` 标识符）
 *   · **只认规范形**：`<wt>.<scope>::<file>[::<sel>]`；另有**裸模块名**（esmx 模块 id，如 `ui_std/std`）。
 *   · 非规范前缀（`ui:` / `ui::` / `md:<addr>`）**不存在** ⇒ 出现即报错，不猜、不兼容。
 * @param {string} spec
 * @returns {{ scope: "ui" | "md", wt: string | null, file: string, sel: string } | null}
 */
export function parseSpec(spec) {
  const m = /^([A-Za-z0-9_]*)\.(ui|md)::(.*)$/.exec(String(spec || "").trim());
  if (!m) return null;
  const wt = m[1] ? m[1] : null;
  const rest = m[3];
  if (m[2] === "ui") return { scope: "ui", wt, file: rest.replace(/^\/+/, "").trim(), sel: "" };
  const i = rest.indexOf("::");
  return { scope: "md", wt, file: (i < 0 ? rest : rest.slice(0, i)).trim(), sel: i < 0 ? "" : rest.slice(i + 2).trim() };
}

/** ui 件位置 → 件路径：只剥**规范形** `.ui::` 前缀；裸模块名（`ui_std/std`）原样返回。旧前缀 `ui:`/`ui::` ⇒ 抛错。
 *  @param {string} spec
 *  @returns {string} */
export function uiPathOf(spec) {
  const s = String(spec || "").trim();
  if (/^ui::?/.test(s)) throw new Error("位置串用了旧前缀（" + s.slice(0, 40) + "）：请写规范形 .ui::<件> 或裸模块名");
  return s.replace(/^\.ui::/, "").replace(/^\/+/, "").trim();
}

/** 链 `[自己, 父, …]`（子覆父）：读宿主内联的 #mdio-wt-chain；取不到退化为 [当前 wt]。
 *  `currentWt` 是**入参**（不读装配点的全局 WT）——框架件不读装配点的全局。
 *  @param {string} currentWt
 *  @param {Document} [doc] 可注入（测试/多文档场景）；默认取全局 document
 *  @returns {string[]} */
export function wtChain(currentWt, doc = document) {
  try {
    const el = doc.getElementById("mdio-wt-chain");
    const j = el ? JSON.parse(el.textContent || "{}") : null;
    if (j && j.wt_uid === currentWt && Array.isArray(j.chain) && j.chain.length) return j.chain.map(String);
  } catch (e) { /* 退化 */ }
  return [currentWt];
}

/**
 * ui 件的链式取件（§2.2 边界项）：沿链**依次**试 `wtui_<uid>/<path>`，import 失败即下一级；
 * 全链未命中 ⇒ 抛错并列出试过的标识符（区分「链断」与「件缺」）。importer 可注入。
 * @param {string} spec
 * @param {string[]} chain
 * @param {(id: string) => Promise<any>} importer
 * @returns {Promise<{ id: string, ns: any, tried: string[] }>}
 */
export async function resolveUiSpec(spec, chain, importer) {
  const p = uiPathOf(spec);
  /** @type {string[]} */
  const tried = [];
  // **路径形态**（`.ui::x.js` / `ui:x.js`）额外支持"资产路由回退"：/ui-assets/<uid>/<path>。
  // 为什么需要：迁移到"一 mod 一包"之后，import map 里**不再有** `wtui_<uid>/<path>` 键，
  // 于是**单 js 文件形态的 mod**（第三方"不大而全"的那种）无从加载。资产路由把树里的**原始 ESM 文件**
  // 以 text/javascript 直接供出 ⇒ 浏览器按 URL 动态 import 它即可；它里面的裸名（react/mobx）仍由页面
  // 的 import map 解析 ⇒ **零配置**。边界：文件必须是**浏览器可直接执行的 ESM**（无 JSX/TS 语法）。
  const pathForm = /^\.[A-Za-z0-9_]*\.ui::/.test(String(spec || "").trim());   // 只认规范形（`ui:` 形式不存在）
  for (const uid of chain) {
    const id = uiModuleOf(uid) + "/" + p;
    tried.push(id);
    try { return { id, ns: await importer(id), tried }; } catch (e) { /* 下一级 */ }
    if (pathForm) {
      const url = "/ui-assets/" + uid + "/" + p;
      tried.push(url);
      try { return { id: url, ns: await importer(url), tried }; } catch (e) { /* 下一级 */ }
    }
  }
  const err = new Error("ui 件未命中链上任何模块（" + tried.join(" → ") + "）");
  err.tried = tried;
  throw err;
}
