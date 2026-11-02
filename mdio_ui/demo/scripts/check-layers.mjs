// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-layers");
armWatchdog("check-layers", 10000);   /* = 10s；实测 0.2s（扫描 1 跑）⇒ ×2 */
/**
 * check-layers —— 分层方向检查（原则二：依赖单向）。
 *
 * 规则（src/ 内）：
 *   1) src/std/**   不得 import src/app/**        （底层不许依赖实现）
 *   2) src/std/**   不得 import src/entry.*       （底层不许依赖入口面）
 *   3) src/app/**   可以 import src/std/**        （实现依赖底层，允许）
 *   4) src/entry.*  两边都可（入口面 = 装配点）
 *   5) 任何 src/**  不得 import src 之外的相对路径（不许 ../.. 越界），包与 node: 内建不限
 * 退出码即判据：0 = 通过，1 = 有违规（逐条打印 文件:行 → 目标）。
 * 用法：node scripts/check-layers.mjs
 */
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { globSync as fsGlobSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SRC = path.resolve(HERE, "../src");
const bad = [];

function walk(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = path.join(dir, e);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (/\.(js|jsx|ts|tsx|mjs|css)$/.test(p)) out.push(p);
  }
  return out;
}
const rel = (p) => path.relative(SRC, p).split(path.sep).join("/");
const layerOf = (r) => (r.startsWith("std/") ? "std" : r.startsWith("app/") ? "app" : r.startsWith("entry.") ? "entry" : "other");

for (const f of walk(SRC)) {
  const from = rel(f);
  const fromLayer = layerOf(from);
  const lines = readFileSync(f, "utf8").split("\n");
  lines.forEach((line, i) => {
    for (const m of line.matchAll(/from\s+"([^"]+)"|@import\s+"([^"]+)"/g)) {
      const spec = m[1] || m[2];
      if (!spec || !spec.startsWith(".")) continue;
      const abs = path.resolve(path.dirname(f), spec);
      const to = path.relative(SRC, abs);
      const toPosix = to.split(path.sep).join("/");
      if (toPosix.startsWith("..")) { bad.push([from, i + 1, spec, "越出 src/"]); continue; }
      // 规则 6：相对 import 必须能解析到真实文件（这条本可提前抓住 ../ui.css 那类"静默卡死"）
      const cands = [abs, abs + ".js", abs + ".jsx", abs + ".ts", abs + ".tsx", abs + ".css", path.join(abs, "index.js")];
      if (!cands.some((c) => existsSync(c))) { bad.push([from, i + 1, spec, "目标不存在（悬空 import）"]); continue; }
      const toLayer = layerOf(toPosix);
      if (fromLayer === "std" && toLayer === "app") bad.push([from, i + 1, spec, "std 依赖 app（违反规则 1）"]);
      if (fromLayer === "std" && toLayer === "entry") bad.push([from, i + 1, spec, "std 依赖入口面（违反规则 2）"]);
    }
  });
}

// ── 规则 7/8：共享单例只有一处声明（平台），且该声明必须**真的产出入口** ─────────────
// 背景（定案）：提供者是**基础框架一处** = 平台 `esmx.provides`；esmx 会**按 provides 自动生成
// 导出模块**（实测：平台 `esmx` 段只有 entry+provides，manifest 里却有 react/react-dom/… 全部入口）。
// 宿主的 8 个 shim 导出已**清空**——它们曾是"第二份实现"，是生产里两份 React 的根因。
// 规则 7：shim 源里引用的裸包必须在 provides 内（shim 仍作为适配器素材存在，但宿主不再路由它们）。
// 规则 8：provides 里**每个**名字，都必须在平台自己的构建产物 manifest 里有入口（= 消费者真能拿到它）。
const DEMO = path.resolve(SRC, "..");
try {
  // 提供者 = mdio_ui（提供者必须是一个**第三方可依赖的包名** ⇒ 不能搬到宿主）。
  const providerPkg = JSON.parse(readFileSync(path.resolve(DEMO, "../lib/mods/ui_base/package.json"), "utf8"));
  const provides = new Set((providerPkg.esmx && providerPkg.esmx.provides) || []);
  const pkgOf = (spec) => (spec.startsWith("@") ? spec.split("/").slice(0, 2).join("/") : spec.split("/")[0]);
  // 规则 7：扫 shim 源（与是否被 exports 路由无关）
  try {
    const shimDir = path.resolve(SRC, "std/shims");
    for (const f of readdirSync(shimDir)) {
      if (!f.endsWith(".ts")) continue;
      const txt = readFileSync(path.resolve(shimDir, f), "utf8");
      for (const m of txt.matchAll(/from\s+"([^".\/][^"]*)"/g)) {
        const p = pkgOf(m[1]);
        if (!provides.has(p)) bad.push(["src/std/shims/" + f, 0, m[1], "共享单例越出提供者声明（规则 7：提供者 = 宿主 demo）"]);
      }
    }
  } catch (e) { /* 没有 shim 目录则跳过 */ }
  // 规则 8：provides ⇒ **提供者产物**里必须有入口（缺一个就是"消费者拿不到"那类事故）
  const hostManifest = path.resolve(DEMO, "../lib/mods/ui_base/dist/client/manifest.json");
  if (existsSync(hostManifest)) {
    const hm = JSON.parse(readFileSync(hostManifest, "utf8"));
    const exp = new Set(Object.keys(hm.exports || {}));
    const distClient = path.resolve(DEMO, "dist/client");
    for (const p of provides) {
      if (exp.has(p)) continue;
      // 抗抖动：产物 manifest 可能被某条演练路径重建成只有 entry 的旧态（实测 provides:{} 的空壳），
      // 所以退一步直接看产物文件在不在（react.final.mjs / react-dom/client.*.final.mjs）。
      const flat = p.replace(new RegExp("/", "g"), path.sep);
      const hit = existsSync(distClient) && readdirSync(distClient, { recursive: true })
        .some((f) => { const s = String(f); return s === flat || s.startsWith(flat + ".") || s.startsWith(flat + path.sep); });
      if (!hit) bad.push(["demo dist", 0, p, "provides 声明了但宿主产物没有入口（规则 8）"]);
    }
  } else {
    console.log("  ⚠ 宿主未构建 ⇒ 规则 8 跳过（先 node_modules/.bin/esmx build）");
  }
} catch (e) {
  bad.push(["check-layers", 0, "provides/exports 检查", "读取失败：" + String(e.message).slice(0, 60)]);
}

// ── 规则 9：工作树 ui/ 的形状统一（有件就必须是一个可构建的模块包）─────────────
// 约定：<wt 根>/ui 里只要有源码件（.js/.jsx/.mjs），就必须有 package.json 且 name = wtui_<uid>
//       （esmx 靠它把该目录挂成模块）；没有 dist/ 只是"未构建"，打印提示但不判红。
try {
  const platformRoot = path.resolve(DEMO, "../lib/mods/ui_base");
  const repoRoot = path.resolve(DEMO, "../..");
  const roots = [path.join(repoRoot, "worktrees", "genesis"), path.join(repoRoot, "..")].slice(0, 0); // 空列表，实际用注册表
  const registry = path.join(repoRoot, "worktrees", "genesis", "wt", ".mds", "worktrees.md");
  const list = [path.join(repoRoot, "worktrees", "genesis")];
  if (existsSync(registry)) {
    const text = readFileSync(registry, "utf8");
    for (const m of text.matchAll(/^-\s*path:\s*(.+)$/gm)) list.push(m[1].trim());
  }
  for (const root of list) {
    const ui = path.join(root, "ui");
    if (!existsSync(ui)) continue;
    const entries = readdirSync(ui).filter((n) => !n.startsWith(".") && n !== "node_modules" && n !== "dist");
    // **放宽**：顶层**单文件 mod**（第三方"不大而全"的形态）**合法** ——
    // 只要它**被本树的声明引用**（`.ui::<文件名>` / `ui:<文件名>`）。未被声明的散件才算形状不对。
    const decl9 = path.join(root, "wt/.mds/ui.md");
    const declTxt9 = existsSync(decl9) ? readFileSync(decl9, "utf8") : "";
    const undeclared = entries.filter((n) => /\.(js|jsx|mjs)$/.test(n) && declTxt9.indexOf(n) < 0);
    const pkgPath = path.join(ui, "package.json");
    if (undeclared.length && !existsSync(pkgPath)) bad.push([path.relative(repoRoot, ui) + "/", 0, "(无 package.json)", "工作树 ui 有**未被声明引用**的源码件（规则 9）：批量件请做成 mod 包，单文件请写进 ui.md 的 `## mods`"]);
    else if (existsSync(pkgPath)) {
      try {
        const name = JSON.parse(readFileSync(pkgPath, "utf8")).name;
        if (typeof name !== "string" || !name.startsWith("wtui_")) bad.push([path.relative(repoRoot, pkgPath), 0, String(name), "模块名不是 wtui_<uid>（规则 9）"]);
        if (!existsSync(path.join(ui, "dist"))) console.log("  · 提示：" + path.relative(repoRoot, ui) + " 尚无 dist（未构建）");
      } catch (e) { bad.push([path.relative(repoRoot, pkgPath), 0, "parse", "package.json 读不出来（规则 9）"]); }
    }
  }
} catch (e) {
  bad.push(["check-layers", 0, "工作树 ui 形状检查", "读取失败：" + String(e.message).slice(0, 60)]);
}

// ── 规则 10：测试里硬编码的 src 路径必须仍然存在（搬目录时测试面也是搬迁面）────────
// 由来：三次真实踩坑 —— S1/S2 打断 test/error-boundary.mjs 的硬编码 import；S2 又打断 e2e ⑭b 的
//       /demo/src/devview/DevView.jsx（视图件搬到 app/ 后该 URL 404，只有真机跑才暴露）。
// 认两种写法：① URL 形 "/demo/src/<p>"（动态 import/ fetch 都可能用）② 相对形 "../src/<p>"。
const TEST_DIR = path.resolve(DEMO, "test");
if (existsSync(TEST_DIR)) {
  for (const f of walk(TEST_DIR)) {
    const txt = readFileSync(f, "utf8");
    for (const m of txt.matchAll(/["'`](\/demo\/src\/[^"'`\s]+|(\.\.\/)+src\/[^"'`\s]+)["'`]/g)) {
      const spec = m[1];
      const bare = spec.replace(/\?.*$/, "");   // 剥掉 ?b1 这类查询串（那是 import 的 cache-bust，不是路径的一部分）
      const abs = bare.startsWith("/demo/") ? path.join(DEMO, bare.slice("/demo/".length)) : path.resolve(path.dirname(f), bare);
      const cands = [abs, abs + ".js", abs + ".jsx", abs + ".ts", abs + ".tsx", abs + ".mjs", abs + ".css"];
      if (!cands.some((c) => existsSync(c))) {
        bad.push([path.relative(DEMO, f), 0, spec, "测试里硬编码的 src 路径已失效（规则 10）"]);
      }
    }
  }
}

// ── 规则 11：样式里的 @source glob 必须真的扫到文件（否则 tailwind 零工具类）──────
// 由来：S2 把 ui.css 从 src/ 搬到 src/app/ 后，五条 @source glob 全部指空 ⇒ tailwind 一条工具类都不生成
// ⇒ 全靠 tailwind 写样式的页（stats / swap）静默塌掉，而所有判据与真机套件都还是绿的
//（它们查结构/文本，不查计算样式）——由用户指出。
try {
  const CSS_ENTRY = path.resolve(DEMO, "src/app/ui.css");
  if (existsSync(CSS_ENTRY)) {
    const css = readFileSync(CSS_ENTRY, "utf8");
    const dir = path.dirname(CSS_ENTRY);
    const matched = [];
    for (const m of css.matchAll(/@source\s+"([^"]+)"/g)) {
      const g = m[1];
      let files = [];
      try { files = fsGlobSync ? fsGlobSync(g, { cwd: dir }) : []; } catch (e) { files = []; }
      if (files.length === 0) bad.push(["src/app/ui.css", 0, g, "@source glob 扫不到任何文件 ⇒ tailwind 不会产出工具类（规则 11）"]);
      for (const f of files) matched.push(path.resolve(dir, f));
    }
    // 规则 11b（G11）：每个 **mod 包目录**都要被至少一条 @source 覆盖——S2 的教训是"非空即绿"不够：
    // 搬件后旧 glob 仍非空（别处有文件），但新位置一个类都不生成 ⇒ 页面静默塌掉而判据全绿。
    const pkgDirs = [];
    const libMods = path.resolve(DEMO, "../lib/mods");
    const collectPkg = (base) => {
      if (!existsSync(base)) return;
      try { if (existsSync(path.join(base, "package.json"))) pkgDirs.push(base); } catch (e) { /* 跳过 */ }
      try {
        for (const n of readdirSync(base)) {
          if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.") || n === "src") continue;   // dist.gen* = 原子发布的代际目录（不是包）
          const d = path.join(base, n);
          try { if (statSync(d).isDirectory() && existsSync(path.join(d, "package.json"))) pkgDirs.push(d); } catch (e) { /* 跳过 */ }
        }
      } catch (e) { /* 跳过 */ }
    };
    collectPkg(libMods);
    const repoRoot11 = path.resolve(DEMO, "../..");
    const reg11 = path.join(repoRoot11, "worktrees", "genesis", "wt", ".mds", "worktrees.md");
    const wtList11 = [path.join(repoRoot11, "worktrees", "genesis")];
    if (existsSync(reg11)) for (const m of readFileSync(reg11, "utf8").matchAll(/^-\s*path:\s*(.+)$/gm)) wtList11.push(m[1].trim());
    for (const root of wtList11) collectPkg(path.join(root, "ui"));
    for (const d of pkgDirs) {
      if (!matched.some((f) => f.startsWith(d + path.sep))) {
        bad.push([path.relative(repoRoot11, d), 0, "@source", "该 mod 包目录没有任何 @source 覆盖 ⇒ 其工具类会静默清零（规则 11b）"]);
      }
    }
  }
} catch (e) { /* 无 fs.globSync 时跳过 */ }

if (bad.length === 0) {
  console.log("✓ 分层方向检查通过（std 不依赖 app / 入口面；无越界相对路径）");
  process.exit(0);
}
console.log("✗ 分层方向违规 " + bad.length + " 处：");
for (const [f, ln, spec, why] of bad) console.log("   " + f + ":" + ln + "  ->  " + spec + "   [" + why + "]");
process.exit(1);
