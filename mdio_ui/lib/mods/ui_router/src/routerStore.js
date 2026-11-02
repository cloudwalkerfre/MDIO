// @ts-check
/**
 * ui_std/routerStore —— **ROUTER store**。
 *
 * 对应关系：router.md : RouterStore :: demo_ui.md : UiStore ——
 *   type 工厂 + create(snapshot, env) + computed（views getter）+ observer。
 * 自治边界（最小契约）：**编译是 app 的自治面**——本文件只给**默认编译**（公共行：path → name/doc），
 *   私有行/任意 map/后处理 = env.compileRoutes 覆盖（md 单源 ⇒ 编译 ⇒ json，后处理随便上）。
 * env 内容（装配者给）：{ wt, read, compileRoutes? }。
 */
import { types, getRoot } from "mobx-state-tree";
import { envOf } from "ui_core/envOf";   // 统一工具面

/** 默认编译（公共行）：router.md 的「# router / ## routes」下嵌套记录「- route: 下挂 - path: / - component: home」
 *   → [{ path, name }]。数据半边（name 是名字不是组件）；view 由 app 用自己的注册表解析（map/后处理自治）。 */
export function defaultCompileRoutes(doc, read) {
  if (!doc || !read) return [];
  try {
    // 交付形状（HS）：root = 「# router」h1 本身（t=router），其子节 = 「## routes」；
    //   直接 Drill "routes"。
    //   兼容嵌套形态（root 下另有 # router 节）时再回退一次。
    let r = read.apply([doc], [["Drill", "routes"], ["Wildcard"]]);
    if (!r || !(r.hits || []).length) {
      r = read.apply([doc], [["Drill", "router"], ["Drill", "routes"], ["Wildcard"]]);
    }
    const out = [];
    for (const routeNode of r.hits || []) {
      const fields = (read.apply([routeNode], [["Wildcard"]]).hits) || [];
      const f = (k) => { const b = fields.find((n) => n && n.t === k); return b ? read.contentValueOf(b) : null; };
      const path = f("path");
      if (!path) continue;
      out.push({ path: String(path), name: f("component") || "" });
    }
    return out;
  } catch (e) { return []; }
}

export const RouterStore = types
  .model("RouterStore", {
    tableDocId: types.maybe(types.number),   // observe(router.md) 返回的 docId（未就绪 null ⇒ table=[] ⇒ 组件级 default 兜底）
  })
  .actions((self) => ({
    setTableDocId(id) { self.tableDocId = typeof id === "number" ? id : null; },
  }))
  .views((self) => ({
    get table() {
      const env = envOf(self);
      if (!self.tableDocId) return [];
      let wt = null;
      try { wt = getRoot(self).wts && getRoot(self).wts.wt; } catch (e) { wt = null; }
      if (!wt) return [];
      const doc = wt.by && typeof wt.by.get === "function" ? wt.by.get(String(self.tableDocId)) : null;
      if (!doc) return [];   // 未就绪/缺席 ⇒ 空表 ⇒ RouterShell 门控走组件级 default（表未就绪=未命中，同一条 default 规则）
      const compile = typeof env.compileRoutes === "function" ? env.compileRoutes : defaultCompileRoutes;
      try { return compile(doc, env.read) || []; } catch (e) { return []; }
    },
  }));

/** 装配入口：app 建实例时传 env；mod 只出类型与工厂。 */
export function createRouterStore(env) { return RouterStore.create({}, env || {}); }
