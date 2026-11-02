// mdio · mod 增量构建插件（探针版）：注入 esmx 自己的 Vite 构建，一次全量 + 之后增量 watch
// 用法（宿主侧）：createViteApp(esmx, { config(ctx) { ctx.config.plugins.push(modWatch({ target: ctx.buildTarget, getInline: () => inlines[ctx.buildTarget] })) } })
import { build } from "vite";

export function modWatch(opts = {}) {
  const target = opts.target || "?";
  const log = (m, extra) => console.log("[mod-watch:" + target + "] " + m, extra === undefined ? "" : JSON.stringify(extra));
  return {
    name: "mdio-mod-watch",
    apply: "build",
    configResolved(cfg) {
      log("resolved", { outDir: cfg.build.outDir, emptyOutDir: cfg.build.emptyOutDir, isProd: cfg.isProduction, plugins: cfg.plugins.length });
    },
    async closeBundle() {
      log("closeBundle", { watchMode: !!this.meta.watchMode });
      if (process.env.MDIO_MOD_WATCH !== "1") { log("skip: MDIO_MOD_WATCH!=1"); return; }
      if (this.meta.watchMode) { log("skip: already watching"); return; }
      const inline = opts.getInline && opts.getInline();
      if (!inline) { log("skip: no inline config captured"); return; }
      const t0 = Date.now();
      const watcher = await build(Object.assign({}, inline, {
        configFile: false,
        build: Object.assign({}, inline.build, { watch: {}, emptyOutDir: false }),
      }));
      log("watcher started", { ms: Date.now() - t0 });
      if (opts.onWatcher) opts.onWatcher(watcher);
    },
  };
}