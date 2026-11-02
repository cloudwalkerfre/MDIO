// 探针：宿主进程用 esmx Node API 编一个 mod，并注入 mod-watch 插件（看能否补上"无 watch/无增量"的短板）
import path from "node:path";
import { Esmx } from "@esmx/core";
import { createViteApp } from "@esmx/vite";
import react from "@vitejs/plugin-react";
import { modWatch } from "./vite-plugin-mod-watch.mjs";

const root = path.resolve(process.argv[2]);
const inlines = {};
console.log("[probe] root =", root);
const esmx = new Esmx({
  root,
  isProd: true,
  devApp: (e) =>
    createViteApp(e, {
      config(ctx) {
        const target = ctx.buildTarget;
        inlines[target] = ctx.config;
        ctx.config.cacheDir = path.resolve(e.root, ".vite-cache");
        ctx.config.plugins = [
          ...(ctx.config.plugins || []),
          react(),
          modWatch({ target, getInline: () => inlines[target], onWatcher: (w) => { globalThis.__w = w; } }),
        ];
      },
    }),
});
const t0 = Date.now();
const ok = await esmx.init(esmx.COMMAND.build);
console.log("[probe] init ok =", ok, "· 墙钟", Date.now() - t0, "ms · targets =", Object.keys(inlines).join(","));
const t1 = Date.now();
const r = await esmx.build();
console.log("[probe] build 返回 =", r === undefined ? "undefined" : typeof r, "· 墙钟", Date.now() - t1, "ms");
const fs = await import("node:fs");
const man = path.join(root, "dist/client/manifest.json");
console.log("[probe] manifest 存在 =", fs.existsSync(man), "· dist/package.json =", fs.existsSync(path.join(root, "dist/package.json")));
if (process.env.MDIO_MOD_WATCH === "1") {
  console.log("[probe] 保持 12s 以便观察 watch 重建（改文件用）…");
  await new Promise((res) => setTimeout(res, 12000));
  if (globalThis.__w) { try { await globalThis.__w.close(); } catch {} }
}
process.exit(0);