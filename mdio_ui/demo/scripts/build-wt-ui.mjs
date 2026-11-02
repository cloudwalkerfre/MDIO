// @ts-check
/**
 * build-wt-ui —— 宿主（demo）用自己的工具链，替某棵工作树的 ui 目录**就地构建**。
 *
 * 用法：node scripts/build-wt-ui.mjs <wtRoot|uiDir> [--force]
 * 产物：<uiDir>/dist/{client,server}/**  +  <uiDir>/.esmx-build.json（戳，含 ok / source / 两枚 hash）
 *
 * 契约依据（E2 实测）：EsmxOptions.root 用绝对路径；Node API 不需要 mod 自带 src/entry.node.ts；
 * 就地构建（chunk 哈希吃绝对路径，跨目录不可复现）。
 *
 * 两条实测约束（Vite Task 的缓存 + H3 复验）：
 *  · **戳必须被 cache.input 排除**（否则任务"改了自己的输入" ⇒ 永不命中）；
 *  · **input 必须排除 node_modules/\*\***（否则 vp 自己的缓存产物会被算成新输入 ⇒ 永不命中）。
 *
 * 失败语义（B4，H2 实测的阻断缺陷）：**任何失败都要写戳且 `ok:false`**——否则旧戳 `ok:true` 会让宿主把坏源码当成功。
 */
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { Esmx } from "@esmx/core";
import { createViteApp } from "@esmx/vite";
// 平台样式层（ui_base）构建时挂 tailwind 插件：style.css 里的 @import "tailwindcss/*" 与 @source
// 在**这里**编译成 esmx 联邦 CSS chunk ⇒ host 经 rc.css() 注入 <link>（官方「共享样式」机制）。
let tailwindPlugin = null;
try { tailwindPlugin = (await import("@tailwindcss/vite")).default; }
catch (e) { console.warn("[build-wt-ui] @tailwindcss/vite 不可用 ⇒ ui_base 平台样式层本次不编译 tailwind"); }

const arg = process.argv[2];
if (!arg) { console.error("用法: node scripts/build-wt-ui.mjs <wtRoot|uiDir>"); process.exit(2); }
const force = process.argv.includes("--force");
// **--atomic**（原子上架）：构建在旁路 `<uiDir>/.staging/` 里跑，成功后把产物搬成新代际
// `dist.gen<ts>`，再用**原子 rename** 把符号链接 `dist` 改指过去 ⇒ 读方要么旧代要么新代，永不见半写。
// 为什么必须这样：esmx 的产物路径硬编码 `dist/client`（core.mjs 实测 4 处）⇒ 不能在配置里改输出目录。
// **默认开**：不变量已实测（4 次构建期间 88 次并发读全部 200、响应体恒定 ⇒ 无缺件/半写窗口）。
// 关掉：`--no-atomic` 或 `MDIO_ATOMIC_BUILD=0`（调试用）。首次对既有实体 `dist` 会自动"转代"（搬成 gen-old + 建链接）。
const atomic = !process.argv.includes("--no-atomic") && String(process.env.MDIO_ATOMIC_BUILD || "1") !== "0";
// **--stage-only**（供 Vite+ 任务对用）：只做旁路构建、保留 `<uiDir>/.staging/dist`、**不发布**；
// 发布交给独立的 `publish-wt-ui.mjs`（那里 `cache:false` ⇒ 每次必跑 ⇒ 重放永远不会覆盖 dist）。
const stageOnly = process.argv.includes("--stage-only");
const resolved = path.resolve(arg);
// 两种入参都接受：工作树根（则取其 ui/）或直接是 ui 包目录（E2 的 fixture 就是后者）
const uiDir = fs.existsSync(path.join(resolved, "ui", "package.json")) ? path.join(resolved, "ui") : resolved;
const stampPath = path.join(uiDir, ".esmx-build.json");
const stagingDir = path.join(uiDir, ".staging");   // 旁路构建目录（仅 --atomic 用；见上）

// **按包的构建锁**（审查 C1 修复）：`ensure-built` 预构建 · 宿主 `[autoBuild]` · 手工构建 · （将来）Vite+ 任务
// 可能同时构建同一个包，而 staging 是固定路径（先 rmSync 再拷贝）⇒ 无锁会互相删改、`.ok.json` 先落者放行、
// 发布出**混合代际**。锁与 `publish-wt-ui` 共用同一路径 ⇒ 构建与发布对同一包永不交错。
import { acquireLockSync, releaseLockSync } from "./lib/flock.mjs";
const LOCK = path.join(uiDir, ".staging.lock");
const lockT0 = Date.now();
acquireLockSync(LOCK, { waitMs: 180000 });
if (Date.now() - lockT0 > 500) console.log("[build-wt-ui] 等锁 " + (Date.now() - lockT0) + "ms（同包有其它构建在跑）");

/** 写戳：**任何路径（成功/失败）都写**，失败时 ok:false（B4）。 */
function writeStamp(fields) {
  const prev = (() => { try { return JSON.parse(fs.readFileSync(stampPath, "utf8")); } catch (e) { return {}; } })();
  const stamp = Object.assign({ source: "build" }, prev, fields, { source: "build", stampedAt: new Date().toISOString() });
  try { fs.writeFileSync(stampPath, JSON.stringify(stamp, null, 2) + "\n"); } catch (e) { console.warn("[build-wt-ui] 写戳失败:", e.message); }
  return stamp;
}

/** 目录内容的稳定指纹（排序后逐个 sha256；跳过大目录）。 */
function hashDir(root, skip) {
  const h = createHash("sha256");
  const walk = (dir) => {
    let names = [];
    try { names = fs.readdirSync(dir).sort(); } catch (e) { return; }
    for (const n of names) {
      if (skip.includes(n) || n.indexOf("dist.gen") === 0 || n.indexOf(".staging") === 0) continue;   // 世代目录/旁路（含 .staging.lock）不参与指纹
      const p = path.join(dir, n);
      let st = null;
      try { st = fs.lstatSync(p); } catch (e) { continue; }
      if (st.isSymbolicLink()) { h.update(p + "|link\n"); continue; }
      if (st.isDirectory()) { h.update(p + "|dir\n"); walk(p); continue; }
      h.update(p + "|" + st.size + "|");
      try { h.update(fs.readFileSync(p)); } catch (e) { h.update("unreadable"); }
      h.update("\n");
    }
  };
  walk(root);
  return h.digest("hex").slice(0, 32);
}

if (!fs.existsSync(path.join(uiDir, "package.json"))) {
  console.error("[build-wt-ui] 不是可构建的 ui 目录（缺 package.json）:", uiDir);
  process.exit(3);
}

// 版本桶：@esmx/vite 在 devDependencies（I2：取错 bucket 会让字段被静默丢掉）
const demoPkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const esmxViteVersion = (demoPkg.dependencies || {})["@esmx/vite"] || (demoPkg.devDependencies || {})["@esmx/vite"] || "unknown";

const t0 = Date.now();
let built = false;
/** 把「源」复制到旁路（不含 dist 系目录、node_modules、.vite-cache、.staging、戳文件）。 */
function prepareStaging() {
  fs.rmSync(stagingDir, { recursive: true, force: true });
  fs.mkdirSync(stagingDir, { recursive: true });
  const SKIP = new Set(["dist", "node_modules", ".vite-cache", ".staging", ".esmx-build.json", ".git"]);
  for (const n of fs.readdirSync(uiDir)) {
    if (SKIP.has(n) || n.indexOf("dist.gen") === 0) continue;
    fs.cpSync(path.join(uiDir, n), path.join(stagingDir, n), { recursive: true, verbatimSymlinks: true });
  }
}

/** 原子发布：搬代 + 原子改指 + 保留两代。 */
function publishAtomic() {
  const gen = "dist.gen" + Date.now();
  fs.renameSync(path.join(stagingDir, "dist"), path.join(uiDir, gen));
  const cur = path.join(uiDir, "dist");
  const linkTmp = path.join(uiDir, ".dist-link.tmp");
  try { fs.unlinkSync(linkTmp); } catch (e) { /* 不存在 */ }
  fs.symlinkSync(gen, linkTmp);
  let st = null;
  try { st = fs.lstatSync(cur); } catch (e) { /* 还没有 dist */ }
  if (st && !st.isSymbolicLink()) fs.renameSync(cur, path.join(uiDir, "dist.gen-old-" + Date.now()));   // 首次：实体目录转代
  fs.renameSync(linkTmp, cur);
  const gens = fs.readdirSync(uiDir).filter((n) => n.indexOf("dist.gen") === 0).map((n) => ({ n, t: fs.statSync(path.join(uiDir, n)).mtimeMs })).sort((a, b) => b.t - a.t);
  for (const g of gens.slice(2)) { try { fs.rmSync(path.join(uiDir, g.n), { recursive: true, force: true }); } catch (e) { /* 删不掉就算了 */ } }
  return gen;
}

let publishedGen = null;
try {
  if (atomic || stageOnly) prepareStaging();
  const buildRoot = (atomic || stageOnly) ? stagingDir : uiDir;
  const esmx = new Esmx({
    root: buildRoot,
    isProd: true,
    devApp: (e) => createViteApp(e, { config(ctx) {
      ctx.config.cacheDir = path.join(uiDir, ".vite-cache");
      ctx.config.base = "./";
      ctx.config.worker = Object.assign({}, ctx.config.worker || {}, { format: "es" });
      // tailwind 的 `@source` 是**相对 style.css** 的；--atomic 会把源拷进 `<uiDir>/.staging/`
      // ⇒ 相对路径整体深一层，`../../../../` 落到 `mdio_ui/lib` 而不是仓库根，扫描面变空
      //（实测：新代际 ui_base CSS 里 0 个 utility ⇒ chat 等 app 的 tailwind 类全失效）。
      // 构建期把相对的 `../../../../` 改写成**仓库根的绝对路径**，与是否 staging 无关。
      // **只扫本仓**：host 的平台样式层不读外部项目；WTS 等发布物在运行时自带各自 CSS
      //（见 MDIO_WTS/tools/gen-app-css.mjs → css@app → 影子根），host 构建不依赖挂载树。
      const repoRoot = path.resolve(uiDir, "../../../..");
      ctx.config.plugins.push({
        name: "mdio-tailwind-abs-source",
        enforce: "pre",
        transform(code, id) {
          if (!id.endsWith("style.css") || code.indexOf('@source "../../../../') < 0) return null;
          return { code: code.replace(/@source "\.\.\/\.\.\/\.\.\/\.\.\//g, '@source "' + repoRoot + "/"), map: null };
        },
      });
      // 平台样式层只归 ui_base：其它 mod 不挂 tailwind（无依赖、无扫描需求）
      try {
        const pkg = JSON.parse(fs.readFileSync(path.join(uiDir, "package.json"), "utf8"));
        if (tailwindPlugin && pkg && pkg.name === "ui_base") ctx.config.plugins.push(tailwindPlugin());
      } catch (e2) { /* 读不到 package.json 就不挂，不阻塞构建 */ }
    } }),
  });
  await esmx.init(esmx.COMMAND.build);   // I8：init(COMMAND.build) **自身就构建**，不要再调 build()（否则 Build completed ×2、watcher 倍增、emptyOutDir 擦盘抢写）
  built = fs.existsSync(path.join(buildRoot, "dist", "client", "manifest.json"))
    && fs.existsSync(path.join(buildRoot, "dist", "server", "manifest.json"));
  if (!built) throw new Error("构建结束但缺 dist/{client,server}/manifest.json");
  if (stageOnly) {
    // 只构建到旁路：保留 .staging/dist，不碰 dist（发布由 publish-wt-ui.mjs 负责）。
    if (!fs.existsSync(path.join(stagingDir, "dist", "client", "manifest.json"))) throw new Error("--stage-only 结束但缺 .staging/dist/client/manifest.json");
    // **成功标记**：发布侧据此拒绝"失败构建留下的半成品"（只验 manifest 存在是不够的 —— 失败可能发生在 server 阶段之后）。
    fs.writeFileSync(path.join(stagingDir, ".ok.json"), JSON.stringify({ ok: true, at: new Date().toISOString(), sourceHash: hashDir(uiDir, ["dist", ".vite-cache", "node_modules", ".esmx-build.json", ".staging"]) }, null, 2) + "\n");
  } else if (atomic) { publishedGen = publishAtomic(); fs.rmSync(stagingDir, { recursive: true, force: true }); }
} catch (err) {
  const e = /** @type {Error} */ (err);
  writeStamp({ ok: false, error: String(e && e.message || e).slice(0, 400), ms: Date.now() - t0, esmxViteVersion });
  console.error("[build-wt-ui] 构建失败 ⇒ 已写 ok:false 戳，产物不可信：", uiDir);
  console.error("  " + String(e && e.message || e));
  process.exit(1);
}

// 修：`.staging.lock`（构建锁，内容每次不同）原先没被排除 ⇒ 戳里的 sourceHash **每次都变**，
// 同一个源也会给出不同指纹（排查成本：被人当成"源漂移"追了半天）。改由 hashDir 按前缀排 `.staging*`。
const sourceHash = hashDir(uiDir, ["dist", ".vite-cache", "node_modules", ".esmx-build.json"]);
const lockHash = (() => {
  const f = path.join(uiDir, "pnpm-lock.yaml");
  try { return createHash("sha256").update(fs.readFileSync(f)).digest("hex").slice(0, 16); } catch (e) { return null; }
})();
const stamp = writeStamp({ ok: true, published: !stageOnly, name: JSON.parse(fs.readFileSync(path.join(uiDir, "package.json"), "utf8")).name, sourceHash, lockHash, esmxViteVersion, ms: Date.now() - t0 });
console.log("[build-wt-ui]", stamp.name, "ok =", stamp.ok, "·", stamp.ms, "ms · sourceHash =", sourceHash, "· ui =", uiDir);
