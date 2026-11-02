// @ts-check
/**
 * DevStore —— 外壳店（app 时代一棵主树的 dev 子树）。
 *
 * 归属：ui_devview mod 出类型；runtime（demo）把它拼进总 store（UiStore 的 dev 子树，快照 {dev: {}} 建）。
 * 形状：
 *   · 纯字段：dark / devMode / mona / log / 栏宽 / panels（槽配置：槽→appview 身份）—— 壳态（前端自己的：不投影 md、不进 sessionStorage、跨 wt 不丢——店只建一次）；
 *   · 统一读接口（无参 getter = computed）：route / doc / entry / openDocId / model ——
 *     数据一律同树 getRoot(self).decl / getRoot(self).wts.wt，不走 env。
 * 纪律：跨店数据走 getRoot；工具（client 等）走 getEnv（env 只装工具）。
 */
import { types, getRoot } from "mobx-state-tree";
import { envOf } from "ui_core/envOf";   // 统一工具面

export const DevStore = types
  .model("DevStore", {
    dark:    types.optional(types.boolean, true),   // 主题（纯字段；刷新回默认）
    devMode: types.optional(types.boolean, false),  // 开发者模式默认 **关**（系统默认是产品面；要开发面在页头点开）
    mona:    types.optional(types.boolean, true),   // Monaco 显示开关（关 = 卸载编辑器/关会话；左格 app 按它条件渲染）
    log:     types.optional(types.boolean, true),   // 日志面板
    wtList:  types.array(types.frozen()),           // wt 清单（树：每棵树子条目 + README `# app` 元数据）
    wtAll:   types.optional(types.boolean, false),  // wt 菜单：false=默认（只显示 README tag: app）/ true=全部
    leftW:   types.optional(types.number, 640),     // 左格合并 app 的整格宽（可拖拽）
    logW:    types.optional(types.number, 340),
    footH:   types.optional(types.number, 30),    // footer 槽高度（console app 收起时=一行输入；出结果由 app 请求高度，可拖）
    /** 槽位配置（devview 拥有槽位编排）：左/右槽按 appview 身份匹配代码宿主 wt；URL &leftwt=/&logwt= 可覆盖。 */
    panels:  types.optional(types.frozen(), { left: { appview: "MONACO_APP" }, log: { appview: "LOG_APP" }, foot: { appview: "CONSOLE_APP" } }),
  })
  .actions((self) => ({
    toggleDark() { self.dark = !self.dark; },
    toggleDev()  { self.devMode = !self.devMode; },
    toggleMona() { self.mona = !self.mona; },
    toggleLog()  { self.log = !self.log; },
    setList(l)   { self.wtList.replace(Array.isArray(l) ? l : []); },
    toggleWtAll() { self.wtAll = !self.wtAll; },
    setWtAll(v)  { self.wtAll = !!v; },
    setLeftW(w)  { self.leftW = Number(w) || self.leftW; },
    setLogW(w)   { self.logW = Number(w) || self.logW; },
    setFootH(h)  { self.footH = Number(h) || self.footH; },
    /** 换 wt = 改主树的 currentWT 一个字段（store 层）+ 连接层换树（env.client.switchTo）。 */
    switchWT(uid) {
      if (!uid || uid === self.currentWT) return;
      try { getRoot(self).wts.setWT(uid); } catch (e) { /* 无根忽略 */ }
      const env = envOf(self);
      if (env.client && typeof env.client.switchTo === "function") env.client.switchTo(uid);
    },
  }))
  .views((self) => ({
    get currentWT() { try { return getRoot(self).wts.currentWT; } catch (e) { return ""; } },
    // ── 统一读接口：数据同树 getRoot（总 store 的 ui / wts 子树）──
    get route() { const d = getRoot(self).decl; return d && typeof d.route === "string" ? d.route : ""; },
    get doc()   { const d = getRoot(self).decl; return d ? d.doc : null; },
    get entry() {
      const p = this.route;
      const w = getRoot(self).wts.wt;
      const files = (w && w.files) || [];
      return files.find((f) => String(f && f.path) === p) || null;
    },
    get openDocId() { const e = this.entry; return e ? e.id : null; },
    get model() { return this.doc; },
  }));

/** 装配入口：mod 出类型与工厂；runtime 拼进总 store 时用快照建（dev: {}），工厂留给独立使用/测试。 */
export function createDevStore(env) { return DevStore.create({}, env || {}); }
