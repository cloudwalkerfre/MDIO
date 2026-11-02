// 判官棘轮：违规即红（exit 1）。规则来源 = 判据清单（+ 附录 B5）。
import { armWatchdog, installGuards } from "../test/lib/watchdog.mjs";
installGuards("judge");
armWatchdog("judge", 10000)   /* = 10s */;   // 实测 max=0.2s（本机 2 跑）⇒ ×2=0.4s，取下限 10s ⇒ 10s
// R3（loader 条款）不存在；R6b 的 fetch 条款指向「client.fullTree 之外」；
// R7 = SSR 边界（entry.server.* 不得 new WebSocket、不得引用 wasm/SAB 常量）。
// R0 语法门 = esbuild 解析全部源文件（js/mjs/jsx/ts；详见文件下部 R0 段）。
// 用法：node scripts/judge.mjs
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, dirname, basename } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const SRC = join(ROOT, "src");
const SERVICES = join(SRC, "std", "services");

const issues = [];
const red = (file, rule, msg, line) => issues.push({ file, rule, msg, line });

function filesUnder(dir, acc = []) {
  // 目录可能不存在（S3 之后 `src/std/services` 已下沉到库包）⇒ 返回空集，不抛。
  if (!existsSync(dir)) return acc;
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    const s = statSync(p);
    if (s.isDirectory()) { if (e !== "node_modules") filesUnder(p, acc); }
    else acc.push(p);
  }
  return acc;
}

// ── 官方库包也进判据面（新增）────────────────────────────────────────────
// 由来：devkit 下沉成库包 `ui_devview`（lib/mods/ui_devview/src/devkit）后，判官源文件数 36 → 27：
// 9 件"前端生产代码"因为不在 demo/src 下而**悄悄脱离覆盖**。位置变了，规则没变 ⇒ 这些与位置无关的
// 规则（R6a 无全局探针 / R6b 数据经 client / R6c 无拓扑词 / R6d 无分类词 / R0 语法门）必须继续管到库包。
// 识别面 = `lib/mods/<pkg>/src/**`（跳过 node_modules / dist* / 容器自身）。
const LIB_MODS = join(ROOT, "..", "lib", "mods");
const LIB_SRC_ROOTS = existsSync(LIB_MODS)
  ? readdirSync(LIB_MODS)
      .filter((n) => !n.startsWith(".") && n !== "node_modules" && n !== "src" && !n.startsWith("dist"))
      .map((n) => join(LIB_MODS, n, "src"))
      .filter((p) => existsSync(p))
  : [];
const libFiles = LIB_SRC_ROOTS.flatMap((d) => filesUnder(d));

/** fetch 白名单（judge 与源规则④共用这一处；不许写第二份）。 */
const FETCH_OK_METHODS = ["fullTree", "readDoc", "readDocAst"];

const svcFiles = [...filesUnder(SERVICES), ...LIB_SRC_ROOTS.map((d) => join(d, "services")).filter((p) => existsSync(p)).flatMap((p) => filesUnder(p))]
  .filter((f) => /\.(js|jsx|mjs)$/.test(f));
const allFiles = [...filesUnder(SRC), ...libFiles].filter((f) => /\.(js|jsx|mjs)$/.test(f));

for (const f of svcFiles) {
  const t = readFileSync(f, "utf8");
  const rel = f.slice(ROOT.length + 1);
  // R1 层间不 import：services 之间的相对/别名 import 即红；供应商包（mobx/mobx-state-tree 等）放行
  const im = t.split("\n").findIndex((l) => {
    const m = /^\s*import\s+[^'"]*['"]([^'"]+)['"]/.exec(l);
    if (!m) return false;
    const src = m[1];
    return src.startsWith(".") || src.startsWith("/") || src.indexOf("services/") >= 0;
  });
  if (im >= 0) red(rel, "R1 层间不 import", "services 文件出现层间 import（供应商包放行）", im + 1);

  if (f.endsWith("boot.js")) {
    // R2 boot 零依赖零拓扑：无 URL/主机字样
    const u = t.split("\n").findIndex((l) => /(https?:\/\/|wss?:\/\/|127\.0\.0\.1|localhost|:8011|:5178)/.test(l));
    if (u >= 0) red(rel, "R2 boot 零拓扑", "boot.js 出现 URL/主机字样", u + 1);
  }

  if (f.endsWith("client.js")) {
    // R4a 对讲机段词汇纯净：sel/decl/window 禁在**对讲机段**（协议/ws/wasm/热路径与 client 类）。
    // **MST 数据模型段除外**（Node/MD/TreeEntry/Wt/Wts —— 主树 UiStore 的子树，有权 getRoot 树内寻址：
    // Node.gates 走 getRoot(self).decl 是树内导航，不是对讲机漂移；主树架构收口，按段守不按文件守）。
    const lines = t.split("\n");
    const mStart = lines.findIndex((l) => /^export const Node\s*=\s*types\.model/.test(l));
    const mEnd = lines.findIndex((l) => /^const NOOP_PERFMARK/.test(l));
    const inModel = (i) => mStart >= 0 && i >= mStart && (mEnd < 0 || i < mEnd);
    const b = lines.findIndex((l, i) => !inModel(i) && /\b(sel|decl|window)\b/.test(l));
    if (b >= 0) red(rel, "R4a client 纯对讲机", "出现 sel/decl/window 词汇", b + 1);
    // R4b 热路径（_dispatchMessage 函数体）不得出现 tel
    const start = lines.findIndex((l) => /^  _dispatchMessage\s*\(/.test(l));
    if (start >= 0) {
      let end = start;
      for (let i = start + 1; i < lines.length; i++) {
        if (/^  [a-zA-Z_$][a-zA-Z0-9_$]*\(/.test(lines[i]) || /^  \/\/ ──/.test(lines[i])) { end = i; break; }
      }
      if (end === start) end = lines.length - 1;
      for (let i = start; i <= end; i++) {
        if (/tel/i.test(lines[i])) red(rel, "R4b 热路径零 telemetry", "热路径出现 tel 引用", i + 1);
      }
    }
  }

  if (f.endsWith("uiMd.js")) {
    const b = t.split("\n").findIndex((l) => /\bsplit\s*\(/.test(l));
    if (b >= 0) red(rel, "R5a uiMd 零文本解析", "出现 split( 切条目", b + 1);
    const w = t.split("\n").findIndex((l) => /\bwindow\b/.test(l));
    if (w >= 0) red(rel, "R5b uiMd 零文本解析", "出现 window", w + 1);
  }
}

for (const f of allFiles) {
  const t = readFileSync(f, "utf8");
  const rel = f.slice(ROOT.length + 1);
  // R6 豁免（wasm 收编进 ui_wasm）：ui_wasm/src/wasm/** = wasm-bindgen 快照胶水 = **引擎本体**
  //（「transport 在 wasm 内」是架构契约，client.js 头注原文）。生成物不进 R6a-d（R0 语法门仍管）。
  if (rel.includes("/ui_wasm/src/wasm/")) continue;
  // R6e（wasm 收编防回潮）：生产代码禁硬编码 /wasm/、/crdt/ 根路径（适配器资产必须走模块引入）。
  // 豁免：entry.node（服务端守卫，主动 404）与 ui_wasm 自身。
  if (!rel.includes("/entry.node") && !rel.includes("/ui_wasm/")) {
    const hp = t.split("\n").findIndex((l) => /["']\/(wasm|crdt)\//.test(l));
    if (hp >= 0) red(rel, "R6e wasm 裸路径", "出现 /wasm/ 或 /crdt/ 硬编码（应走 ui_wasm 模块资产）", hp + 1);
  }
  // R6a 生产代码不读全局探针
  const w = t.split("\n").findIndex((l) => /window\.__/.test(l));
  if (w >= 0) red(rel, "R6a 无全局探针", "出现 window.__*", w + 1);
  // R6b 组件数据一律经 client
  const ws = t.split("\n").findIndex((l) => /new\s+WebSocket\b/.test(l));
  if (ws >= 0) red(rel, "R6b 数据经 client", "出现 new WebSocket", ws + 1);
  // fetch 条款：全项目只放行 client.js 的 fullTree 方法体，其余文件一律红
  const lines6 = t.split("\n");
  // 放行面 = client.js 里**显式白名单**的方法体（原先只硬编码 fullTree 一个；加 readDoc）。
  // 白名单之外一律红：数据仍然只能从通道走，组件不许裸发请求。
  // （名字表在文件级定义：源规则④ 用它查“白名单不悬空”。）
  const okRanges = [];
  if (f.endsWith("client.js")) {
    for (const name of FETCH_OK_METHODS) {
      const s = lines6.findIndex((l) => new RegExp("^\\s*(?:async\\s+)?" + name + "\\s*\\(").test(l));
      if (s < 0) continue;
      let e = lines6.length - 1;
      for (let k = s + 1; k < lines6.length; k++) {
        if (/^  (?:async\s+)?[a-zA-Z_$][a-zA-Z0-9_$]*\s*\(/.test(lines6[k])) { e = k; break; }
      }
      okRanges.push([s, e]);
    }
  }
  for (let i = 0; i < lines6.length; i++) {
    if (!/\bfetch\s*\(/.test(lines6[i])) continue;
    if (okRanges.some(([s, e]) => i > s && i < e)) continue;   // 白名单方法体内：放行
    // 唯一豁免：public/ 下的 **vendor 大件**（自包含大 bundle，不进 vite 模块链路；
    // 需在 fetch 行或其紧邻上两行显式标 `vendor-asset` —— 数据仍然一律经 client）。
    const near = [lines6[i - 2] || "", lines6[i - 1] || "", lines6[i]].join(" ");
    if (/vendor-asset/.test(near)) continue;
    red(rel, "R6b 数据经 client", "client.fullTree 之外出现 fetch(", i + 1);
  }
  // R6c 前端无拓扑概念：cf/hs 词、mode 词（client 的 MODE_* 坐标常量行除外）
  const cf = t.split("\n").findIndex((l) => /\b(cf|hs)\b/.test(l));
  if (cf >= 0) red(rel, "R6c 无拓扑概念", "出现 cf/hs 词汇", cf + 1);
  // **R6c 放宽（业主口径）**：禁的是**逻辑用词**，不禁**声明** ——
  //   交付信封里本来就有这个键（`projection.rs` §5.6 明文），结构即数据 ⇒ 模型必须逐字声明它。
  //   放行形状：① 模型字段声明 `mode:`；② 快照键字面量 `"mode"` / 'mode'。
  //   仍然红灯的形状：`x.mode` / `mode === …` / `if (mode` 这类**拿它做判断**的写法。
  const declShaped = (l) => /^\s*mode\s*:/.test(l) || /["']mode["']/.test(l);
  const noMode = t.split("\n").filter((l) => !/MODE_/.test(l) && !declShaped(l)).join("\n");
  const md = noMode.split("\n").findIndex((l) => /\bmode\b/.test(l));
  if (md >= 0) red(rel, "R6c 无拓扑概念", "出现 mode 词汇（逻辑用词；声明形状与 MODE_* 除外）", md + 1);
  // R6d 前端无来源分类概念
  const kd = t.split("\n").findIndex((l) => /\b(kind|scheme|spaceOf|kindOf)\b/.test(l.replace(/\b(node|it)\.kind\b|\["kind"\]/g, "")));
  if (kd >= 0) red(rel, "R6d 无来源分类概念", "出现 kind/scheme 词汇", kd + 1);
}

// ── 源规则：四条今天全绿的断言，先立起来守住后续每一步 ──────────────
// 扫描面 = allFiles（demo/src + lib/mods/<pkg>/src）；vendor 与 demo/test 不在面内。
// ① 全局口收紧：R6a 只扫 `window.__`，扫不到 `globalThis.__` 的**赋值**（读取放行）。
for (const f of allFiles) {
  const rel = f.slice(ROOT.length + 1);
  const lines0 = readFileSync(f, "utf8").split("\n");
  const isCssAdopt = f.endsWith(join("services", "cssAdopt.js"));
  for (let i = 0; i < lines0.length; i += 1) {
    const raw = lines0[i];
    const code = raw.trim();
    if (code.startsWith("//") || code.startsWith("*") || code.startsWith("/*")) continue;
    if (isCssAdopt || !/globalThis\s*\)?\s*\.\s*__[A-Za-z0-9_$]*\s*=/.test(raw)) continue;
    red(rel, "① 全局口收紧", "globalThis.__* 只许在 services/cssAdopt.js 里赋值（读取不受限）", i + 1);
  }
  // ② URL 写点白名单：只有这三个文件能写 URL（自动挡住“app 自己拼整条 URL”，正对应 J3）
  const isUrlWriter = ["DevView.jsx", "appRouter.js", "entry.client.ts"].indexOf(basename(f)) >= 0;
  if (!isUrlWriter) {
    const u = lines0.findIndex((l) => /history\.(?:replaceState|pushState)\s*\(|location\.assign\s*\(|location\.href\s*=/.test(l));
    if (u >= 0) red(rel, "② URL 写点白名单", "URL 写点只许在 DevView.jsx / appRouter.js / entry.client.ts", u + 1);
  }
}
// ③ provides 三处一致：两个 entry 的 platform.provides 逐字相同，且 package.json 的 esmx.provides ⊇ 它
{
  const baseDir = join(LIB_MODS, "ui_base");
  const grab = (p) => { try { const m = /provides\s*:\s*\[([^\]]*)\]/.exec(readFileSync(p, "utf8")); return m ? m[1].split(",").map((s) => s.trim().replace(/^["']|["']$/g, "")).filter(Boolean) : null; } catch (e) { return null; } };
  const pc = grab(join(baseDir, "src", "entry.client.ts"));
  const ps = grab(join(baseDir, "src", "entry.server.ts"));
  let pp = null;
  try { const pj = JSON.parse(readFileSync(join(baseDir, "package.json"), "utf8")); pp = (pj.esmx && pj.esmx.provides) || null; } catch (e) { pp = null; }
  if (!pc || !ps) red("lib/mods/ui_base", "③ provides 三处一致", "读不到 entry.client / entry.server 的 platform.provides", 0);
  else if (pc.join(",") !== ps.join(",")) red("lib/mods/ui_base", "③ provides 三处一致", "两个 entry 的 provides 不一致：" + JSON.stringify({ client: pc, server: ps }), 0);
  else if (!pp) red("lib/mods/ui_base", "③ provides 三处一致", "package.json 缺 esmx.provides", 0);
  else {
    const missing = pc.filter((n) => pp.indexOf(n) < 0);
    if (missing.length) red("lib/mods/ui_base", "③ provides 三处一致", "esmx.provides 缺：" + missing.join(","), 0);
  }
}
// ④ fetch 白名单不悬空：FETCH_OK_METHODS 的每个名字必须在 client.js 里真实存在
{
  let csrc = "";
  try { csrc = readFileSync(join(LIB_MODS, "ui_core", "src", "services", "client.js"), "utf8"); } catch (e) { csrc = ""; }
  for (const name of FETCH_OK_METHODS) {
    if (!new RegExp("^\\s*(?:async\\s+)?" + name + "\\s*\\(", "m").test(csrc)) red("lib/mods/ui_core/src/services/client.js", "④ fetch 白名单不悬空", "FETCH_OK_METHODS 里的 " + name + " 在 client.js 里不存在", 0);
  }
}

// R7 SSR 边界（§6 / 附录 B5）：entry.server.* 无浏览器——不得 new WebSocket、不得引用 wasm/SAB 常量
// （entry.node.* 是壳宿主，SAB/COOP/COEP 正是它的职责，不在本条管辖内）
const entryServerFiles = filesUnder(SRC).filter((f) => /^entry\.server\./.test(basename(f)));
if (entryServerFiles.length === 0) {
  red("src/entry.server.*", "R7 SSR 边界", "未找到 entry.server.*（SSR 边界文件缺失）", 0);
}
for (const f of entryServerFiles) {
  const rel = f.slice(ROOT.length + 1);
  const lines7 = readFileSync(f, "utf8").split("\n");
  const ws7 = lines7.findIndex((l) => /new\s+WebSocket\b/.test(l));
  if (ws7 >= 0) red(rel, "R7 SSR 边界", "entry.server 出现 new WebSocket", ws7 + 1);
  const sab7 = lines7.findIndex((l) => /(SharedArrayBuffer|\bAtomics\b|editor_core|WebAssembly)/.test(l));
  if (sab7 >= 0) red(rel, "R7 SSR 边界", "entry.server 引用 wasm/SAB 常量", sab7 + 1);
}

// R8 业务面纪律（§2.5 / 收口）：认领组件交付只许 { node, exec }；
// handle/data 包装层不存在，不得复活。检查面 = src 下全部视图层（交付形状在 doc/NodeView.jsx：const FunC = compFor(ui, node)）。
// 扫描面含库包（S3 之后渲染面在 `lib/mods/ui_core/src`，不在 demo/src）：只扫 SRC 会让 R8 假红
//    （"交付形状缺失"其实是"没扫到"）——与 R6a–R6d 同一条教训。
const dvFiles = [...filesUnder(SRC), ...libFiles].filter((f) => /\.(js|jsx)$/.test(f));
const dvAll = dvFiles.map((f) => readFileSync(f, "utf8")).join("\n");
if (/\bcreateHandle\b|\bdataAt\b/.test(dvAll)) red("src+lib 视图层", "R8 业务面", "包装层复活：出现 createHandle/dataAt", 0);
if (/<Comp[^>]*\b(handle|data)=/.test(dvAll)) red("src+lib 视图层", "R8 业务面", "组件交付 props 超出 {node, exec}（出现 handle/data）", 0);
if (!/<\w+\s+node=\{node\}\s+exec=\{exec\}/.test(dvAll)) red("src/*", "R8 业务面", "组件交付形状缺失：<FunC node={node} exec={exec} />（组件只许收这两个 prop）", 0);

// ── R0 语法门：esbuild 解析全部源文件（.js/.mjs/.jsx/.ts/.tsx） ──
// 覆盖范围与原因：
//   · 覆盖 src 下全部源文件——含 .jsx 与 .ts；loader 取超集（js/mjs/jsx → jsx，ts/tsx → tsx），
//     即容忍 .js 里写 JSX、.ts 里写 JSX，只判语法不判风格；
//   · 为什么不用 node --check：它 (a) 在 .jsx 上直接 ERR_UNKNOWN_FILE_EXTENSION（DevView.jsx 恒失败），
//     (b) 不认 .ts 的类型标注，而 entry.* 三件套是 TS；
//   · 为什么原实现是惰性的（实录）：spawnSync 从未导入，ReferenceError 被本门 catch 吞掉 ⇒
//     语法错长期漏网。这里用 esbuild 真实解析，且**失败关闭**：解析器取不到就报红，绝不静默放行；
//   · esbuild 来源：本仓 node_modules/.pnpm/esbuild@*/（vite 的依赖，pnpm 严格布局不提升到本包 node_modules），
//     退路依次为「沿 vite 的安装位置解析」→「按 ROOT 解析」；三路皆空 ⇒ R0 红。
const SRC_EXT = /\.(js|mjs|jsx|ts|tsx)$/;
const srcFiles = [...filesUnder(SRC), ...libFiles].filter((f) => SRC_EXT.test(f));
const loaderOf = (f) => (/\.tsx?$/.test(f) ? "tsx" : "jsx");
function resolveEsbuildEntry() {
  const pnpmDir = join(ROOT, "node_modules", ".pnpm");
  if (existsSync(pnpmDir)) {
    const hit = readdirSync(pnpmDir)
      .filter((d) => /^esbuild@/.test(d))
      .map((d) => join(pnpmDir, d, "node_modules", "esbuild", "lib", "main.js"))
      .find((p) => existsSync(p));
    if (hit) return hit;
  }
  try {
    const req = createRequire(import.meta.url);
    const vitePkg = req.resolve("vite/package.json", { paths: [ROOT] });
    return createRequire(vitePkg).resolve("esbuild");
  } catch (e) { /* 退到下一路 */ }
  try { return createRequire(import.meta.url).resolve("esbuild", { paths: [ROOT] }); } catch (e) { return null; }
}
let esbuildApi = null;
const esbuildEntry = resolveEsbuildEntry();
if (esbuildEntry) {
  try {
    const mod = await import(pathToFileURL(esbuildEntry).href);
    const api = mod && mod.transform ? mod : (mod && mod.default) || null;
    if (api && typeof api.transform === "function") esbuildApi = api;
  } catch (e) { esbuildApi = null; }
}
if (!esbuildApi) {
  red("src/**", "R0 语法门", "esbuild 解析器不可用（R0 失败关闭，不静默放行）", 0);
} else {
  for (const f of srcFiles) {
    const rel = f.slice(ROOT.length + 1);
    try {
      await esbuildApi.transform(readFileSync(f, "utf8"), { loader: loaderOf(f), sourcefile: rel });
    } catch (e) {
      const err = e && Array.isArray(e.errors) && e.errors[0] ? e.errors[0] : null;
      const line = err && err.location ? err.location.line : 0;
      red(rel, "R0 语法门", "esbuild 解析失败：" + String(err ? err.text : (e && e.message) || e).split("\n")[0].slice(0, 140), line);
    }
  }
}

if (issues.length) {
  console.log("✗ 判官红，共 " + issues.length + " 条：");
  for (const i of issues) console.log("  [" + i.rule + "] " + i.file + ":" + (i.line || "?") + " " + i.msg);
  process.exit(1);
}
console.log("✓ 判官绿（" + srcFiles.length + " 个源文件，无违规）");
// 看门狗非 unref ⇒ 必须显式退出（否则「打印完绿却不退」，10s 后被 exit 2 —— 实测抓到过）。
process.exit(0);   // 走到这里 = issues 为空（红的那条上面已经 exit 1）