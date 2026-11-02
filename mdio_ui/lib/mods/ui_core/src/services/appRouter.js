// @ts-check
/**
 * 应用路由能力（mdio_ui 提供）· 宿主注入，应用不必自拼 URL。
 *
 * 实测背景（note §8.2⑤ / §22 / §28 三次独立复现）：凡让库或应用自己写**整条 URL** 的动作，都会抹掉壳的参数（?wt&app）。
 * 所以：**壳参数由本模块原样透传**；应用只表达"我在哪一页"，并自选表达方式：
 *   · hash 视图（默认）：页内路由写进 hash（不占 path/query）——适合不想动 URL 形状的应用；
 *   · path 视图：页内路由就是真实路径（如 /edit/xxx）——适合已有 URL 形状的应用。
 *
 * 用法：
 *   const ar = appRouter;                       // 宿主注入（ui_core/appRouter 亦可自建）
 *   ar.route()            ar.hash.route()   ar.path.route()      // 当前页
 *   ar.push('/x')         ar.path.push('/x')                     // 换页（留历史）
 *   ar.replace('/')       ar.path.replace('/')                   // 换页（不留历史）
 *   ar.subscribe(fn)      ar.path.subscribe(fn)                  // 订阅 URL 侧变化
 *   ar.href('/x')         ar.path.href('/x')                     // 真链接（含壳参数）
 */

export function createAppRouter(opts) {
  const o = opts || {};
  const hasLoc = typeof location !== 'undefined';
  const shellSearch = () => (hasLoc ? String(location.search || '') : '');
  const pathname = () => (hasLoc ? String(location.pathname || '/') : '/');

  function read(via) {
    if (!hasLoc) return String(o.initial || '/');
    if (via === 'path') return pathname() || '/';
    const hash = String(location.hash || '').replace(/^#/, '');
    if (hash && hash.charAt(0) === '/') { try { return decodeURIComponent(hash); } catch (e) { return hash; } }
    return String(o.initial || '/');
  }

  function build(page, via) {
    const p = String(page == null ? read(via) : page);
    if (via === 'path') return (p && p.charAt(0) === '/' ? p : '/' + p) + shellSearch();
    return pathname() + shellSearch() + (p && p !== '/' ? '#' + p : '');
  }

  function go(page, via, replace, notify) {
    if (!hasLoc) return;
    const p = String(page == null ? '/' : page);
    if (replace) { try { history.replaceState(null, '', build(p, via)); } catch (e) {} if (notify) notify(); return; }
    if (via === 'path') { try { history.pushState(null, '', build(p, via)); } catch (e) {} if (notify) notify(); return; }
    try { location.hash = p === '/' ? '' : p; } catch (e) {}
  }

  function api(via) {
    const subs = [];
    const fire = () => { subs.slice().forEach((f) => { try { f(read(via)); } catch (e) {} }); };
    function subscribe(fn) {
      if (!hasLoc || typeof window === 'undefined') return () => {};
      const on = () => { try { fn(read(via)); } catch (e) {} };
      subs.push(on);
      window.addEventListener('hashchange', on);
      window.addEventListener('popstate', on);
      return () => { const i = subs.indexOf(on); if (i >= 0) subs.splice(i, 1); window.removeEventListener('hashchange', on); window.removeEventListener('popstate', on); };
    }
    return {
      via: via,
      route: () => read(via),
      href: (p) => build(p, via),
      push: (p) => go(p, via, false, fire),
      replace: (p) => go(p, via, true, fire),
      subscribe: subscribe,
    };
  }

  // ── 层（面板）的容器：**由壳提供**，应用只管容器内的路由上下文 ──
  // 实测约束：壳不能自己渲染 RouterProvider（@esmx/router 只在 wt 的构建型 mod 里）⇒ 壳给容器，应用 portal 进来。
  // 「层的落点」：默认 document.body；壳建好隔离边界（影子根）后**注入**边界，面板就落在边界内
  //   （否则 app 自己的样式作用域罩不到面板内容 —— 实测口径见 note「方案评审」§4）。
  let layerRoot = null;
  function setLayerRoot(root) { layerRoot = root || null; }
  function layerHost() {
    if (!hasLoc || typeof document === 'undefined') return null;
    const parent = layerRoot || document.body;
    let el = typeof parent.getElementById === 'function' ? parent.getElementById('mdio-layer-host') : null;
    if (!el && typeof parent.querySelector === 'function') el = parent.querySelector('#mdio-layer-host');
    if (!el) {
      el = document.createElement('div');
      el.id = 'mdio-layer-host';
      el.setAttribute('data-layer-host', '1');
      el.style.cssText = 'position:fixed;inset:0;z-index:10000;pointer-events:none;';
      parent.appendChild(el);
    }
    return el;
  }
  const hashApi = api('hash');
  const pathApi = api('path');
  // **S4：app 级同页换树通道** —— 壳把 switchWt 注入（装配点 boot 时调 setTreeSwitcher），
  //   app 经 appRouter.switchTo(uid) 换树（同页重跑 boot；URL 单写者纪律不变：还是壳写 ?wt）。
  let treeSwitcher = null;
  function setTreeSwitcher(fn) { treeSwitcher = typeof fn === "function" ? fn : null; }
  function switchTo(uid) {
    if (!treeSwitcher || !uid) return Promise.resolve(false);
    return Promise.resolve(treeSwitcher(String(uid)));
  }
  // **文档指针归一入口**：壳注入 handler（onDocPointer），app 经 setDocPointer **显式请求**。
  //   壳段（?app=）只由壳写；app 只写页内段（/observe/方案.md 原则 2）。壳不监听 hash 推导（DevView onPage 不存在）。
  let docPointerSink = null;
  function setDocPointerSink(fn) { docPointerSink = typeof fn === "function" ? fn : null; }
  function setDocPointer(path) {
    if (!docPointerSink) return;
    try { docPointerSink(path == null ? null : String(path)); } catch (e) { /* 壳写失败不影响 app */ }
  }
  const shell = { shellSearch: shellSearch, base: pathname(), layerHost: layerHost, setLayerRoot: setLayerRoot, setTreeSwitcher: setTreeSwitcher, switchTo: switchTo, setDocPointerSink: setDocPointerSink, setDocPointer: setDocPointer };
  return Object.assign({}, hashApi, shell, { hash: hashApi, path: pathApi });
}

/**
 * **全应用共用的那一份**（无状态 URL 视图：只读 location、只写 history/hash）。
 * 为什么要有它：节点件（NodeView 里的 `<Comp>`）与入口件（appview 那块地的主人）必须拿到**同一个**实例 ——
 * 否则「同一件事有多个落点」又要长第二份（note §3 的主要风险），而且两边的 `href`/`subscribe` 会各算各的。
 * 投递纪律仍是 **走 props**（源头单例、投递 props，note §3）：本文件只负责“唯一一份”。
 */
export const appRouter = createAppRouter();

export default createAppRouter;