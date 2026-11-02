// @ts-check
/**
 * ui_std/router —— **std 级 ROUTER mod（esmx 实现）**。
 *
 * 定位：esmx 是 ROUTER mod 的**一个可选实现**，本文件是 std 提供的默认实现；
 *   app 可换（父子各用各的框架）。
 *
 * 契约（组件级）：
 *   · RouterShell({ table, initial, renderNode, appRouter })
 *       table   = 编译好的路由条目 [{ path, view }]（view = 组件；编译规则是 app/router store 的自治面，
 *                 本壳只吃成品 —— md 单源 ⇒ 编译 ⇒ 这张表，映射/后处理随便上）。
 *   · fallback = default 行语义：表未就绪 / 路径未命中 ⇒ 组件级 renderNode() 渲染当前文档，不装 app、不占槽。
 *
 * esmx 语义坑（实测，DOC 本仓校正已记）：
 *   · **首导航未命中（含空表）会抛 "No active route"**（RouterProvider 挂载即读 router.route）⇒
 *     **门控**：表为空 / 未命中时**不挂 Provider/RouterView**，直接 renderNode() 兜底；
 *   · esmx 动态 = 换实例（parsedOptions frozen、无运行时增删）⇒ 表变（computed 重算）⇒ new Router ⇒ 换 Provider。
 */
import { createElement as h, useEffect, useMemo, useState } from "react";
import { Router, RouterMode } from "@esmx/router";
import { RouterProvider, RouterView, useRouter, useRoute } from "@esmx/router-react";

export { useRouter, useRoute };

/**
 * RouterShell —— 组件级路由壳（esmx 实现）。
 * @param {object} p
 * @param {Array<{path:string, view?:any}>} p.table   编译条目（空 ⇒ 纯 default 渲染）
 * @param {string} [p.initial]                       初始页（宿主 appRouter.route()）
 * @param {Function} [p.renderNode]                  组件级默认渲染（default 行落点）
 * @param {object} [p.appRouter]                     URL 真相（hash 视图）——桥：订阅它喂内层 memory router
 */
export function RouterShell({ table, initial, renderNode, appRouter }) {
  const [rt, setRt] = useState(null);
  const [err, setErr] = useState("");
  const [fallbackView, setFallbackView] = useState(null);

  const spec = useMemo(
    () => (table || []).map((r) => ({ path: String(r.path || "/"), component: r.view })),
    [table]
  );
  const fallback = useMemo(
    () => () => {
      const node = typeof renderNode === "function" ? renderNode() : null;
      setFallbackView(node ? h("div", { "data-fallback": "md" }, node) : null);
    },
    [renderNode]
  );

  useEffect(() => {
    let alive = true;
    let made = null;
    (async () => {
      try {
        made = new Router({
          mode: RouterMode.memory,
          base: new URL("/edit", location.origin),
          routes: spec,
          fallback,   // RouteHandleHook：未命中即触发、每次导航一次（渲染全归 handler —— 组件级）
        });
        // 命中即清 fallback：先注册再首推（否则首推的 afterEach 会漏掉注册前的清理）。
        made.afterEach((to) => { if (to && to.matched && to.matched.length > 0) setFallbackView(null); });
        await made.push(initial || "/");
        if (!alive) { try { made.destroy(); } catch (e) { /* 不吞错 */ } return; }
        setRt(made);
      } catch (e) { if (alive) setErr(String((e && e.message) || e)); }
    })();
    // **表变 = 换实例**（esmx 动态语义）：清理时销毁本 effect 建的实例，避免已销毁的 router（fallback 路径 route=null）
    //   在新表下被 Provider 挂上（挂载即读 route ⇒ 抛 "No active route"）。
    return () => { alive = false; if (made) { try { made.destroy(); } catch (e) { /* 忽略 */ } } };
  }, [spec, fallback, initial]);

  // 桥：URL 真相（appRouter）→ 内层 memory router（esmx 无 hash 模式 ⇒ memory 桥是正解）
  useEffect(() => {
    if (!rt || !appRouter || typeof appRouter.subscribe !== "function") return;
    const onUrl = (page) => {
      try {
        const want = String(page || "/");
        if (want !== String(rt.route ? rt.route.path : "")) rt.push(want).catch(() => {});
      } catch (e) { /* route 未就绪忽略 */ }
    };
    const off = appRouter.subscribe(onUrl);
    return off;
  }, [rt, appRouter]);

  // 命中回表 ⇒ 清 fallback（default 渲染只活到下一次命中）——注册点已挪进建表 effect（首推前注册，避免漏清）。

  // **门控（成文 · 实测三处）**：表空 / 表未就绪 / 实例无活路由 ⇒ Provider 挂载即读 route 抛 "No active route"
  //   （esmx：RouterProvider 的 useSyncExternalStore 快照读 router.route，fallback 路径 transition.route 永不前进）。
  //   三判：rt 未建 · spec 空 · rt.route 试读抛（已销毁的实例）——任一命中 ⇒ 组件级 default（renderNode）。
  let hasRoute = false;
  if (rt) { try { hasRoute = !!rt.route; } catch (e) { hasRoute = false; } }
  if (err) return h("div", { "data-shell": "err", style: { color: "#f88" } }, "router 失败：" + err);
  if (!rt || !spec.length || !hasRoute) return fallbackView || (typeof renderNode === "function" ? renderNode() : null);
  return h(
    RouterProvider,
    { router: rt },
    h("div", { "data-shell": "host" }, fallbackView, h(RouterView))
  );
}

export default RouterShell;
