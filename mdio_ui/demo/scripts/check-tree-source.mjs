// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-tree-source");
armWatchdog("check-tree-source", 10000);   /* = 10s；实测 0.2s（扫描 1 跑）⇒ ×2 */
/**
 * check-tree-source —— **树的来源判据**：接口层（HTTP `/fs/tree`）与 WS 两条树帧的关系。
 * 事实与 file:line 全在 `scripts/README.md` 的「树的两条来源（已核实事实）」一节（本判据是那节的机器形式）。
 *
 * 已核实：一条 WS 连接（`/ws/gateway/<wt>`）发一次 `UpTree(0x01)`（`lib/wasm/src/lib.rs:1020` 连接即发）
 * 会收到**两条** `DnTree(0x81)` 帧：
 *   ① 网关补发（`gateway/handlers/crdt.py:256-260`，`store.get_vfs_tree(conn_wt, True)`）⇒ 与 HTTP
 *      `/fs/tree?hidden=true&wt_uid=<连接 wt>`（`gateway/api/fs.py:11-20`，同一个 `refresh_tree_json`）**逐项等价**；
 *   ② 引擎答复（`mdio_core/src/store.rs:1583-1586`，`refresh_tree_json("genesis", false)` 写死）⇒ 与连接 wt 无关。
 * 应用当前**两条都丢弃**（`src/entry.client.ts:110` 的 `onTree` 是空实现），UI 树来自 HTTP（`:120` → `client.js:688`）。
 *
 * 断言（**全部结构性**：按到达顺序定位帧；**不做内容嗅探**——业主已否「按含不含 ui.md 挑帧」那类规则）：
 *   ① 接口层的树含 `/.mds/ui.md`
 *   ② 第 1 条树帧 == 同一 wt 的 HTTP 树（条数 + id 集 + path 集逐项相同）⇒ 补发在、且真的按连接 wt 取
 *   ③ 一次 `UpTree` 恰好 2 条树帧（帧数变了 = 协议形状变了，必须显式改本判据与 README）
 *      · 第 2 条帧与 `HTTP(genesis, hidden=false)` 的关系**只播报不判红**：引擎按连接 wt/hidden 出树后，
 *        这条读数自然变化（切换的前提在引擎侧，见 README 同节）。
 *   ④ `/.mds/mounts/**` 过滤仍生效：HTTP 树里确实有 mounts 条目（过滤非空转）+ 装配点调用处仍带该前缀过滤（源断言）
 *   ⑤ 换一棵**非 genesis** 的已登记 wt 复跑 ②（防「补发写死 genesis」这类回归）
 *
 * 用法：node scripts/check-tree-source.mjs [--wt <uid>]
 * 依赖：网关（默认 8011，可用 MDIO_GATEWAY 覆盖）活着；取不到就 `⊘ 跳过` 并退 0（verify-all 视为跳过）。
 * 边界：本判据**不碰浏览器**（守的是"树从哪来"的接口/协议面）；页面 DOM 上的树渲染由 pages-live / doc-render 管。
 */
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const GW = (process.env.MDIO_GATEWAY || "http://127.0.0.1:8011").replace(/\/+$/, "");
const argWt = process.argv.indexOf("--wt");
const WT = argWt >= 0 ? String(process.argv[argWt + 1] || "genesis") : "genesis";
const MOUNTS = "/.mds/mounts/";   // 装配点排除的前缀（entry.client.ts:121）
const DECL = "/.mds/ui.md";       // 声明档（boot 的 uiMd 步骤按它取 id：entry.client.ts:256-261）

const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };
const note = (s) => console.log("      · " + s);
const idsOf = (t) => t.map((e) => Number(e.id)).sort((a, b) => a - b);
const pathsOf = (t) => t.map((e) => String(e.path)).sort();
const same = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
const eqTree = (a, b) => !!a && !!b && same(idsOf(a), idsOf(b)) && same(pathsOf(a), pathsOf(b));

/** 接口层的树 = 页面唯一树源（client.js:688 的 fullTree 就是这一条 URL）。 */
async function treeOf(wt, hidden) {
  const url = GW + "/fs/tree?hidden=" + (hidden ? "true" : "false") + "&wt_uid=" + encodeURIComponent(wt);
  const r = await fetch(url, { signal: AbortSignal.timeout(8000) });
  if (!r.ok) throw new Error("HTTP " + r.status + " " + url);
  const j = await r.json();
  if (!Array.isArray(j)) throw new Error("响应不是数组：" + url);
  return j;
}

/** 裸 WS 探针：连上 → 发一次 UpTree(0x01) → 收 DnTree(0x81) 帧（条件轮询到 want 条或超时）。 */
async function probeFrames(wt, want = 2, ms = 4000) {
  const url = GW.replace(/^http/, "ws") + "/ws/gateway/" + encodeURIComponent(wt);
  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  const opened = await new Promise((res) => {
    const t = setTimeout(() => res(false), 5000);
    ws.addEventListener("open", () => { clearTimeout(t); res(true); });
    ws.addEventListener("error", () => { clearTimeout(t); res(false); });
  });
  if (!opened) throw new Error("WS 连不上 " + url);
  const frames = [];
  const done = new Promise((res) => {
    const t = setTimeout(() => res("timeout"), ms);
    ws.addEventListener("message", (ev) => {
      const b = new Uint8Array(ev.data);
      if (b[0] !== 0x81) return;   // 只收树帧（0x81 = DnTree；client.js:49 / crdt.py:187）
      const len = new DataView(b.buffer, b.byteOffset, b.byteLength).getUint32(1, true);
      let arr = null;
      try { arr = JSON.parse(new TextDecoder().decode(b.subarray(13, 13 + len))); } catch (e) { arr = null; }
      frames.push({ n: Array.isArray(arr) ? arr.length : -1, rows: Array.isArray(arr) ? arr : [], parsed: Array.isArray(arr) });
      if (frames.length >= want) { clearTimeout(t); res("done"); }
    });
  });
  ws.send(new Uint8Array([0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));   // UpTree：13 字节头，peer=0 / doc=0 / 空载荷
  const how = await done;
  try { ws.close(); } catch (e) { /* 忽略 */ }
  return { frames, how };
}

console.log("check-tree-source: 探针 wt=" + WT + "（网关 " + GW + "）");

// ── 闸门：网关不在 ⇒ 本判据无从谈起（照 check-vp-cache 的约定：⊘ 跳过 + 退 0）
let genesisTree = null;
try { genesisTree = await treeOf("genesis", true); }
catch (e) {
  console.log("⊘ 跳过：网关取不到树（" + GW + "/fs/tree）——" + String((e && e.message) || e).slice(0, 90));
  process.exit(0);
}

// ── ① 接口层的树含 /.mds/ui.md
let tree = null;
try { tree = WT === "genesis" ? genesisTree : await treeOf(WT, true); }
catch (e) { tree = null; }
if (!tree) {
  step("① 接口层树可取得（wt=" + WT + "）", false, String("HTTP 取树失败"));
} else {
  const uiMd = tree.find((e) => String(e.path) === DECL);
  const mdN = tree.filter((e) => String(e.path).startsWith("/.mds/")).length;
  step("① 接口层树含 " + DECL + "（wt=" + WT + "）", !!uiMd,
    "树 " + tree.length + " 条 · .mds " + mdN + " 条" + (uiMd ? " · ui.md id=" + uiMd.id : " · **缺失**"));
}

// ── ②③ 两条树帧：第 1 条必须 == 同 wt 的 HTTP 树；帧数必须恰好 2
let probe = { frames: [], how: "skip" };
if (tree) {
  try { probe = await probeFrames(WT, 2); } catch (e) { probe = { frames: [], how: "error: " + String((e && e.message) || e).slice(0, 80) }; }
}
const f1 = probe.frames[0] || null;
step("② 第 1 条树帧 == 同 wt 的 HTTP 树（条数 + id/path 逐项）", !!f1 && eqTree(f1.rows, tree),
  f1 ? ("帧 " + f1.n + " 条 vs HTTP " + (tree ? tree.length : "?") + " 条 · 收帧 " + probe.frames.length + " 条（" + probe.how + "）")
     : ("没收到树帧（" + probe.how + "）"));
step("③ 一次 UpTree 恰好 2 条树帧", probe.frames.length === 2,
  "帧数 " + probe.frames.length + (probe.frames.length ? " · 条数 [" + probe.frames.map((f) => f.n).join(", ") + "]" : ""));
if (probe.frames[1]) {
  let gn = null;
  try { gn = await treeOf("genesis", false); } catch (e) { gn = null; }
  const isGene = gn ? eqTree(probe.frames[1].rows, gn) : null;
  note("第 2 条帧 " + probe.frames[1].n + " 条 · == HTTP(genesis, hidden=false)"
    + (isGene === null ? "：判不了（genesis 树取不到）"
      : isGene ? "：是（引擎现状 store.rs:1584 写死 genesis+false，与连接 wt 无关；本项只播报）"
        : "：否（引擎已按连接 wt/hidden 出树？两帧语义趋同；本项只播报，不改红绿）"));
}

// ── ④ mounts 过滤仍生效：非空转（HTTP 树里真有 mounts）+ 调用点源断言
const mounts = genesisTree.filter((e) => String(e.path).startsWith(MOUNTS));
const srcPath = path.join(DEMO, "src", "entry.client.ts");
const srcLines = fs.readFileSync(srcPath, "utf8").split(String.fromCharCode(10));
const callAt = srcLines.findIndex((l) => /client\.fullTree\(/.test(l));
let filtAt = -1;
if (callAt >= 0) for (let i = callAt; i <= Math.min(callAt + 6, srcLines.length - 1); i++) {
  if (/\.filter\(/.test(srcLines[i]) && srcLines[i].indexOf(MOUNTS) >= 0) { filtAt = i; break; }
}
const scoped = callAt >= 0 && /client\.fullTree\(\s*WT\s*\)/.test(srcLines[callAt]);
step("④ " + MOUNTS + "** 过滤仍生效（非空转 + 调用点源断言）", mounts.length > 0 && filtAt >= 0 && scoped,
  "HTTP 树含 " + mounts.length + " 条 mounts；装配点 entry.client.ts:"
  + (callAt >= 0 ? (callAt + 1) : "?") + (filtAt >= 0 ? "-" + (filtAt + 1) : "") + " 按该前缀过滤"
  + (scoped ? "（按当前 wt 取树）" : "（⚠ 调用未带 WT ⇒ 会拿到整棵 VFS）"));

// ── ⑤ 换一棵非 genesis 的已登记 wt 复跑 ②：补发写死 genesis 时这里必红
let second = null;
try {
  const st = await (await fetch(GW + "/control/status", { signal: AbortSignal.timeout(5000) })).json();
  const cands = (Array.isArray(st && st.wts) ? st.wts : []).filter((u) => u && u !== WT);
  for (const u of cands) {
    const t = await treeOf(u, true);
    if (t.length && t.length !== genesisTree.length) { second = { wt: u, tree: t }; break; }   // 条数要与 genesis 不同，否则这条断言没有鉴别力
  }
} catch (e) { second = null; }
if (!second) {
  note("⑤ 跳过：没有第二棵「非 " + WT + " 且树非空且条数 ≠ genesis」的已登记 wt（本项专防「补发写死 genesis」的回归）");
} else {
  let p2 = { frames: [], how: "error" };
  try { p2 = await probeFrames(second.wt, 2); } catch (e) { p2 = { frames: [], how: String((e && e.message) || e).slice(0, 60) }; }
  const g1 = p2.frames[0] || null;
  step("⑤ 换 wt（" + second.wt + "）后第 1 条帧仍 == 该 wt 的 HTTP 树", !!g1 && eqTree(g1.rows, second.tree),
    "帧 " + (g1 ? g1.n : "?") + " 条 vs HTTP " + second.tree.length + " 条 · genesis 是 " + genesisTree.length + " 条（不同 ⇒ 有鉴别力）");
}

const bad = rows.filter((r) => !r.ok);
if (bad.length) {
  console.log("✗ check-tree-source 失败 " + bad.length + " 项：" + bad.map((b) => b.name).join(" · "));
  process.exit(1);
}
console.log("✓ check-tree-source 全绿：接口层树含 " + DECL + " · 第 1 条树帧 == 同 wt 的 HTTP 树（id/path 逐项）· 一次 UpTree " + probe.frames.length + " 条帧 · mounts 过滤非空转且调用点仍在");
// 收尾显式退出（本脚本跑完仍有未释放句柄 ⇒ 会被自己的 deadline 判成「挂起」；红路径上面已 exit 1）
process.exit(0);
