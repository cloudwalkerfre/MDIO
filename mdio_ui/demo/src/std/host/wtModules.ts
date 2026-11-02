// @ts-check
/**
 * wtModules —— 工作树 UI 模块的**发现**（唯一真源 = 工作树注册表）。
 *
 * 规矩（原则一：一处定义）：**挂载表不写在应用里**，由这里按注册表读出来；
 * 两个消费者共用同一个函数：
 *   ① src/entry.node.ts 的 esmx modules.links（给出 artifactDir）
 *   ② scripts/sync-wt-uses.mjs 生成 package.json 的 esmx.uses（模块名清单）
 *
 * **真源 = 运行时的工作树注册表**（根工作树文档区的 /.mds/worktrees.md：每条 - uid + - path），
 * 与运行时侧的挂载登记同一份（§25.7 收口：不再"前端扫盘、后端读表"两套真源）。
 * 根 wt 自己不在表里（worktree_paths.py:109），故按运行时根补上。
 * 注册表读不到时**退回扫盘并告警**（静默的兜底等于第二套真源）。
 */
import { existsSync, lstatSync, readFileSync, readdirSync, realpathSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const PREFIX = "wtui_";

/** 运行时根的候选（顺序即优先级）：环境变量 → 源码态 → 装态。 */
function runtimeRootCandidates(repoRoot) {
  const out = [];
  if (process.env.MDIO_WORKSPACE_ROOT) out.push(String(process.env.MDIO_WORKSPACE_ROOT));
  out.push(path.join(repoRoot, "worktrees", "genesis"));
  out.push(path.join(os.homedir(), ".mdio", "genesis"));
  return out;
}

/**
 * 戳即开关（G4 + I1）：读 `<ui>/.esmx-build.json`，给出"这个产物可不可信"。
 *
 * 规则（单一真源，P1）：
 *  · 无戳 ⇒ `usable = built`（老规矩：看 manifest），`reason = "no-stamp"`；
 *  · 有戳且 `ok:false` ⇒ **`usable = false`**（构建失败留下的半成品/旧产物一律不可信）；
 *  · 有戳且 `source !== "build"`（如 dev 产物）⇒ **`usable = false`**（F4 第二个坑：dev 与生产写同一份 manifest）；
 *  · 有戳、`ok:true`、`source:build` ⇒ `usable = built`。
 */
/**
 * **单个包的状态**（供宿主之外的脚本复用 —— 例如 `scripts/ensure-built.mjs` 的"启动前预构建"）。
 * 与宿主挂载用的是**同一份**判据（`stampVerdict` + `dist/client/manifest.json`），避免两处口径漂移。
 * @param {string} pkgDir 包目录（含 package.json 的那层）
 */
export function packageState(pkgDir) {
  const built = existsSync(path.join(pkgDir, "dist", "client", "manifest.json"));
  const v = stampVerdict(pkgDir);
  const usable = v.usable === null ? built : v.usable;
  return { built, usable, reason: usable ? "usable" : (v.usable === null ? "not-built" : v.reason) };
}

function stampVerdict(ui) {
  const stampPath = path.join(ui, ".esmx-build.json");
  if (!existsSync(stampPath)) return { usable: null, reason: "no-stamp" };
  try {
    const s = JSON.parse(readFileSync(stampPath, "utf8"));
    if (s && s.ok === false) return { usable: false, reason: "stamp-not-ok" };
    if (s && s.source && s.source !== "build") return { usable: false, reason: "stamp-source-" + s.source };
    // C2（审查）：`--stage-only` 构建成功但**尚未发布**时戳里 `published:false` ⇒ **不可挂载**。
    // 否则 publish 掉队会出现"戳 ok + dist 仍是旧代"⇒ 把旧内容当新内容挂载，绕过 I1「戳即开关」。
    if (s && s.published === false) return { usable: false, reason: "stamp-unpublished" };
    return { usable: null, reason: "stamp-ok" };
  } catch (e) {
    return { usable: false, reason: "stamp-unreadable" };
  }
}

/**
 * 沿**注册表分层**递归收集项目根（G10）：每级 `<项目根>/wt/.mds/worktrees.md`（或 `<项目根>/.mds/worktrees.md`）
 * 登记**它自己的子**；深度上限 8、按 realpath 去环——与 Python 侧 `worktree_paths.py:98-133` 同一拓扑。
 * 注意：注册面（挂载/uses）可以是超集，**解析面必须按父链、兄弟不串**（客户端 resolveUiSpec 已如此）。
 */
function collectProjectRoots(seedRoots) {
  const out = [];
  const seenRoots = new Set();
  const seenMd = new Set();
  const queue = seedRoots.map((r) => ({ root: path.resolve(r), depth: 0 }));
  while (queue.length) {
    const cur = queue.shift();
    if (cur.depth > 8) continue;
    let real = cur.root;
    try { real = realpathSync(cur.root); } catch (e) { continue; }
    if (seenRoots.has(real)) continue;
    seenRoots.add(real);
    out.push(real);
    for (const md of [path.join(real, "wt", ".mds", "worktrees.md"), path.join(real, ".mds", "worktrees.md")]) {
      if (!existsSync(md) || seenMd.has(md)) continue;
      seenMd.add(md);
      for (const m of readFileSync(md, "utf8").matchAll(/^-\s*path:\s*(.+)$/gm)) {
        const p = m[1].trim();
        if (p) queue.push({ root: p, depth: cur.depth + 1 });
      }
      break;
    }
  }
  return out;
}

/**
 * @returns {Record<string, { dist: string, ui: string, root: string, uid: string, built: boolean }>}
 *   模块名 → { dist, ui, root, uid, built }。**发现门（G1）**：有 `ui/package.json` 即登记；
 *   是否已构建由 `built`（dist/client/manifest.json 存在）标记——由消费方决定挂不挂、并打告警。
 */
export function discoverWtModules(repoRoot) {
  /** @type {string[]} */
  const roots = [];
  let registry = null;
  for (const rc of runtimeRootCandidates(repoRoot)) {
    for (const md of [path.join(rc, "wt", ".mds", "worktrees.md"), path.join(rc, ".mds", "worktrees.md")]) {
      if (!existsSync(md)) continue;
      registry = md;
      roots.push(rc);                                   // 根 wt（表里没有它自己）
      const text = readFileSync(md, "utf8");
      for (const m of text.matchAll(/^-\s*path:\s*(.+)$/gm)) {
        const p = m[1].trim();
        if (p) roots.push(p);                           // 各已登记工作树的项目根
      }
      break;
    }
    if (registry) break;
  }
  if (!registry) {
    console.warn("[wtModules] 工作树注册表不可读 ⇒ 退回扫盘发现（兜底，不是真源）");
    const wtDir = path.join(repoRoot, "worktrees");
    if (existsSync(wtDir)) for (const n of readdirSync(wtDir)) roots.push(path.join(wtDir, n));
    for (const n of readdirSync(repoRoot)) {
      if (n.startsWith(".") || n === "node_modules" || n === "worktrees" || n === "target") continue;
      roots.push(path.join(repoRoot, n));
    }
  }
  const allRoots = collectProjectRoots(roots);
  /** @type {Record<string, { dist: string, ui: string, root: string, uid: string, built: boolean }>} */
  const out = {};
  for (const root of allRoots) {
    const ui = path.join(root, "ui");
    if (!existsSync(ui)) continue;
    // 形状一（过渡，今天的 genesis/exp_app/exp_swap）：**wt 级单包** = <root>/ui，包名必须 wtui_<uid>。
    // 形状二（目标，§3.3「一 mod 一包」）：<root>/ui/<mod>/，**包名由开发者定**（前缀是宿主内部标识，不是命名约束）。
    const candidates = [ui];
    for (const n of readdirSync(ui)) {
      if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.")) continue;   // dist.gen* = 代际目录（不是包）
      const sub = path.join(ui, n);
      try { if (lstatSync(sub).isDirectory() && existsSync(path.join(sub, "package.json"))) candidates.push(sub); } catch (e) { /* 跳过 */ }
    }
    if (process.env.MDIO_WT_DEBUG) console.log("[wtModules] " + root + " ⇒ candidates: " + candidates.map((c) => path.relative(root, c) || ".").join(", "));
    for (const pkgDir of candidates) {
      const pkg = path.join(pkgDir, "package.json");
      if (!existsSync(pkg)) continue;                      // G1：只要求「有包」，不再要求「有 dist」
      try {
        const pkgJson = JSON.parse(readFileSync(pkg, "utf8"));
        const name = pkgJson.name;
        if (typeof name !== "string" || !name) continue;
        const isWtLevel = pkgDir === ui;
        if (isWtLevel && !name.startsWith(PREFIX)) continue;   // wt 级包仍按 wtui_<uid> 认（宿主内部标识）
        // **无入口的 wt 级包不算 ui 包**（B7 清账）：`<root>/ui/` 这层本该是「这棵树的 UI 应用」，
        // 没有 `src/entry.{client,server,node}.ts` 就既挂不上、也不该被「预构建」—— 以前它按戳判成 `usable:false`
        // ⇒ `ensure-built` 去构建 ⇒ 构建失败 ⇒ 宿主走兜底 ⇒ 控制台留下旧 chunk 的 MIME 报错（实测）。
        // 这里**大声跳过**（不静默）：**只对 wt 级包生效**；mod 级包（`<root>/ui/<mod>/`，如 `region`）本来就没有入口，不受影响。
        if (isWtLevel) {
          const hasEntry = ["client", "server", "node"].some((k) => existsSync(path.join(pkgDir, "src", "entry." + k + ".ts")));
          if (!hasEntry) { console.warn("[wtModules] 跳过 " + name + "：wt 级包没有入口（src/entry.{client,server,node}.ts）⇒ 不作为可挂载的 ui 包（" + path.relative(root, pkgDir) + "）"); continue; }
        }
        if (out[name]) continue;                                // 同名以先到为准（wt 级优先）
        if (process.env.MDIO_WT_DEBUG) console.log("[wtModules]   登记 " + name + "  kind=" + (isWtLevel ? "wt" : "mod") + "  pkgDir=" + path.relative(root, pkgDir));
        const dist = path.join(pkgDir, "dist");
        out[name] = {
          dist,
          ui,
          pkgDir,
          kind: isWtLevel ? "wt" : "mod",
          uid: isWtLevel ? name.slice(PREFIX.length) : path.basename(pkgDir),
          // 导出清单（来自 package.json 的 esmx.exports）：键形如 "./std.js" ⇒ 逻辑名 "<包名>/std.js"。
          // G6 要用它把声明里的逻辑名解析成 import map 的键；G7 要用它核对官方件的迁移是否完整。
          exports: (pkgJson.esmx && pkgJson.esmx.exports) || {},
          root,
          // 过渡形态：包名即 wtui_<uid>；一 mod 一包：uid 取包目录名（直接取，避免误引外层循环变量 n）。
          built: existsSync(path.join(dist, "client", "manifest.json")),
          ...stampVerdict(pkgDir),
        };
      } catch (e) { /* 坏包跳过，不拖垮启动 */ }
    }
  }
  return out;
}
