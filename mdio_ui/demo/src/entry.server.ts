// @ts-check
/**
 * entry.server —— SSR 侧：只出静态骨架 + rc.state 初始数据（判官 R7 边界）。
 *
 * 本文件无浏览器、无 SAB、无 WebSocket、不引用 wasm 常量：帧驱动的文档视图与交互态
 * 全部在水合后由 entry.client 接管（SSR 边界：服务端不触浏览器 API）。
 * 注入四件套顺序 = 官方模板：head 放 preload+css，body 放 importmap → moduleEntry → modulePreload。
 *
 * **dev 的 SRI：只删「入口」那一条**：
 *   · 入口名是**构建号**（`entry.client.<buildId>.final.mjs`）：同名不同内容 ⇒ 它的 `integrity` 会**过期**，
 *     浏览器按旧哈希**拒绝执行** ⇒ 页面卡在「正在装载…」。所以 dev 里**只删入口标签上的** integrity。
 *   · 其余 chunk 名**自带内容哈希**（`…​.BNt2tJBR.final.mjs`）：integrity 有效，而且**必须留**——
 *     去掉它会让 `modulepreload` 与真实 import **失配**，同一个模块被加载两份（A/B 实测：
 *     `check-module-graph` 从「各 1 份」变「各 2 份」，资源 56 → 77 条）。
 * 生产构建（`NODE_ENV=production`）两条都原样保留。判据：`scripts/check-dev-no-sri.mjs`。
 */
import type { RenderContext } from "@esmx/core";

/** 生产构建：SRI 全留。dev：只删入口那一条（见文件头）。 */
const KEEP_SRI = process.env.NODE_ENV === "production";

export default async function render(rc: RenderContext): Promise<void> {
  // 资源清单与 import map 由 commit() 落定（preload/css/importmap/moduleEntry/modulePreload 的数据源）
  await rc.commit();
  const hostEntry = (() => {
    try {
      return new URL(String(rc.params?.url || "/"), "http://mdio.local").searchParams.get("hostBridge") === "parent";
    } catch { return false; }
  })();
  rc.html = `<!doctype html>
<html lang="zh-CN" data-theme="dark">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>mdio demo</title>
    ${rc.preload()}
    ${rc.css()}
  </head>
  <body>
    <div id="root"><div id="boot-note" style="position:fixed;inset:0;display:flex;flex-direction:column;gap:14px;align-items:center;justify-content:center;background:#0f1115;color:#c9d1d9;font:13px/1.6 ui-monospace,Menlo,monospace"><style>@keyframes mdiospin{to{transform:rotate(360deg)}}#boot-note .ring{width:26px;height:26px;border:2px solid #ffffff22;border-top-color:#7aa2f7;border-radius:50%;animation:mdiospin .9s linear infinite}#boot-note .t{opacity:.78}#boot-note .h{opacity:.35;font-size:11px}</style><div class="ring"></div><div class="t">正在装载…</div><div class="h">超过 10 秒仍未完成会给出原因与重试入口</div></div></div>
    ${rc.state("mdioBootstrap", { entries: [] })}
    ${rc.importmap()}
    ${entryTag(hostEntry)}
    ${hostEntry ? "" : rc.modulePreload()}
  </body>
</html>`;
  function entryTag(host: boolean) {
    // The default entry is an import-map bare specifier; the host entry is a
    // dev-only source URL served by Vite, so it must be requested explicitly.
    let t = host
      ? '<script type="module" src="/demo/src/entry.host.client.ts"></script>'
      : rc.moduleEntry();
    return KEEP_SRI ? t : t.replace(/\s+integrity="[^"]*"/g, "");
  }
}
