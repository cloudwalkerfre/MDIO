// @ts-check
/**
 * modLoader —— **mod 装载线**（从 `demo/src/entry.client.ts` 搬进框架）。
 *
 * 为什么放 core：装载线是"声明 → 取件 → 模块"的公共语义（md 代码块 / ui 件 / npm 件 / http 件），
 * 与哪个宿主无关；它不住在装配点里，宿主换一层不用抄。
 *
 * **零 import**（判据 R1：`services/` 下不许有层间 import）：所有依赖**由装配点注入**——
 * 与 `boot.js`（registerStep）和 `cssAdopt.js`（setCssAdoptLog）同一套接线纪律。
 *
 * 注入面（deps）：
 *   getClient / getWt   —— 取当前的连接与 wt（**必须传函数**：换树时它们会变）
 *   loadMd             —— md 装载线（`services/mdLine.js`）
 *   parseSpec · uiModuleOf · resolveUiSpec · wtChain —— 位置语法（`services/locator.js`）
 *   adoptCssMod        —— 装载成功后统一采纳"mod 携带的样式表"（`services/cssAdopt.js`）
 *   log                —— 可选日志口
 *
 * 返回值里的 `modSrcPaths` 是热重载判据的事实（见下），调用方按约定在每轮装载前清空。
 *
 * @param {object} deps 见上
 */
export function createModLoader(deps) {
  const { getClient, getWt, loadMd, createScope, loadScopedMd, parseSpec, uiModuleOf, resolveUiSpec, wtChain, adoptCssMod, log } = deps || {};
  const say = typeof log === "function" ? log : function () {};

  /** 本轮声明里**所有 md 件的源文档路径**：这些文档一改（帧到达）就要重装声明。
   *  为什么：md 件的代码住在文档里（「代码也是数据」）⇒ 在 Monaco 里改组件代码应当立即生效。
   *  只在**声明文档**变化时重装会漏掉 jsx@page 块代码（实测：卡片仍显示旧徽标）。 */
  const modSrcPaths = new Set();   // md 件源文档路径（源文档订阅/open 循环用）

  /** 当前 wt/app 的局域 md scope（每轮 UiDecl.reload 由 beginScope 换新）。 */
  let scope = null;

  /** md 位置 → 源文档路径（取 `::` 之前那段，缺前导斜杠就补上）。 */
  function mdSrcPath(addr) {
    const s = String(addr == null ? "" : addr);
    const i = s.indexOf("::");
    const p = (i < 0 ? s : s.slice(0, i)).trim();
    return p.startsWith("/") ? p : "/" + p;
  }

  /** md 件的装载：记下源文档路径（源文档订阅用）。 */
  async function loadMdMod(name, addr) {
    modSrcPaths.add(mdSrcPath(addr));
    return loadMd(name, addr, (code) => getClient().exec(code, { wt_uid: getWt() }));
  }

  /**
   * 换一轮 wt/app 的局部 scope。decl.mods 的 .md:: 项自动成为 @app/<NAME> 的 binding；
   * 旧 scope 立刻 dispose（在飞的 load 结果会被丢弃）。
   * 未注入 createScope/loadScopedMd 时保持旧行为（无 scope）。
   */
  function beginScope(decl) {
    if (scope) {
      try { scope.dispose(); } catch (e) { /* ignore */ }
      scope = null;
    }
    if (typeof createScope !== "function" || typeof loadScopedMd !== "function") return;
    const bindings = new Map();
    const wt = String(getWt());
    for (const m of ((decl && decl.mods) || [])) {
      const name = String((m && m.name) || "");
      if (!name) continue;
      const canon = parseSpec(String((m && m.spec) || ""));
      if (!canon || canon.scope !== "md") continue;
      if (canon.wt && String(canon.wt).toLowerCase() !== wt.toLowerCase()) continue;
      const addr = canon.sel ? canon.file + "::" + canon.sel : canon.file;
      bindings.set(name, { name, addr });
    }
    scope = createScope({
      bindings,
      loadModule(name, addr, loadDep) {
        modSrcPaths.add(mdSrcPath(addr));
        return loadScopedMd(name, addr, function (code) {
          return getClient().exec(code, { wt_uid: getWt() });
        }, loadDep);
      },
      log: say,
    });
    say("md-scope: begin · " + bindings.size + " local bindings");
  }

  /** 装载入口：装载成功后统一处理「mod 携带的样式表」（采纳失败不拖垮装载）。 */
  async function loadMod(name, spec) {
    const r = await loadModRaw(name, spec);
    try { adoptCssMod(name, r && r.ns); } catch (e) { /* 样式采纳失败不拖垮装载 */ }
    return r;
  }

  /**
   * 装载翻译（§2.2）：md 代码块 → `services/mdLine.js`（唯一自写装载线）；
   * ui/npm 件 = 浏览器原生 ESM（import map 解析，esmx 注入），ui: 沿 wt 链试探。
   */
  async function loadModRaw(name, spec) {
    const s = String(spec == null ? "" : spec).trim();
    if (!s) throw new Error("装载：空位置");
    const WT = getWt();
    const canon = parseSpec(s);                   // 规范形优先：<wt>.<scope>::<file>[::<sel>]
    if (canon && canon.scope === "ui") {
      const uid = canon.wt && canon.wt.toLowerCase() !== String(WT).toLowerCase() ? canon.wt : WT;
      const id = uiModuleOf(uid) + "/" + canon.file;
      try {
        return { name, ns: await import(/* @vite-ignore */ id), url: id, via: "native" };
      } catch (e) {
        // **资产路由回退**：迁移后 import map 里没有 `wtui_<uid>/<file>` 键，
        // 于是**单文件 mod**（树里一个原始 ESM 文件，无 package.json）在这里会失败。
        // 回退到 `/ui-assets/<uid>/<file>`：资产路由以 text/javascript 直接供出该文件，
        // 浏览器按 URL 装载它；它内部的裸名（react/mobx）仍由**页面的 import map** 解析 ⇒ 零配置。
        const url = "/ui-assets/" + uid + "/" + canon.file;
        say("装载：native 失败，回退资产路由 " + url);
        return { name, ns: await import(/* @vite-ignore */ url), url, via: "asset" };
      }
    }
    if (canon && canon.scope === "md") {
      if (canon.wt && canon.wt.toLowerCase() !== String(WT).toLowerCase()) throw new Error("md 位置暂不支持跨 wt（" + canon.wt + "）");
      if (scope && scope.has(name)) return scope.load(name);
      const addr = canon.sel ? canon.file + "::" + canon.sel : canon.file;
      return loadMdMod(name, addr);
    }
    if (s.startsWith("md:")) return loadMdMod(name, s.slice(3));
    if (/^\.?ui::?/.test(s)) {
      // ui 件 = esmx 模块的导出名：沿 wt 链（子覆父）依次试，命中即用（走宿主注入的 import map）
      const r = await resolveUiSpec(s, wtChain(WT), (id) => import(/* @vite-ignore */ id));
      return { name, ns: r.ns, url: r.id, via: "native" };
    }
    const id = s.startsWith("http:") ? s.slice(5).trim() : s;   // http 件 = 裸包名（npm 原生 ESM）
    return { name, ns: await import(/* @vite-ignore */ id), url: id, via: "native" };
  }

  /** 每轮装载前清空两段判据事实（声明换新后旧的源文档不应再触发重装）。 */
  function resetSources() { modSrcPaths.clear(); }

  return { loadMod, loadModRaw, loadMdMod, mdSrcPath, modSrcPaths, resetSources, beginScope };
}