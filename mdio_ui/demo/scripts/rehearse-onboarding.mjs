// @ts-check
/**
 * rehearse-onboarding —— **第三方"单 js 文件"形态的接入演练**。
 *
 * 它自己造件、自己声明、自己重启（网关+宿主）、自己验证、自己清场：
 *   ① 在 A 树（默认 genesis）的 `ui/` 放一个**不打包、无 package.json 的 ESM 文件**；
 *   ② 在该树 `wt/.mds/ui.md` 的 `## mods` 加一行 `- <NAME>: .ui::<文件>`；
 *   ③ 重启网关（读声明）+ 宿主（客户端代码）—— 用 `up-all`（它带正确 env）；
 *   ④ CDP 断言：**控制台出现 mod 里的标记**（= 客户端经资产路由把它装载进页面）；
 *   ⑤ 清场：删文件与声明行 ⇒ 重启网关+宿主 ⇒ 三套形态判据回绿。
 *
 * 用法：node scripts/rehearse-onboarding.mjs [--tree worktrees/genesis] [--name EXT_SINGLE] [--keep]
 */
import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";
const REPO = path.resolve(DEMO, "../..");
const args = process.argv.slice(2);
const argOf = (n, d) => { const i = args.indexOf(n); return i >= 0 ? String(args[i + 1]) : d; };
const TREE = argOf("--tree", "worktrees/genesis");
const NAME = argOf("--name", "EXT_SINGLE");
const keep = args.includes("--keep");
const FORM = argOf("--form", "single");            // single | package
const PKG = "ext_pkg_rehearsal";                  // 包形态的目录名（兼包名）
const FILE = "ext_single_rehearsal.js";
const ui = path.join(REPO, TREE, "ui");
const decl = path.join(REPO, TREE, "wt/.mds/ui.md");
const PORT = Number(process.env.PORT || 5178);
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };
// **用 restart-gateway 而不是手搓 `pkill`**：它带**完整 env** 起网关并校验挂载根，
// 避免本 session 踩过的"裸 pkill + 裸 nohup 丢 env ⇒ 页面起得来但声明装载不了"。
const killGateway = () => { spawnSync(process.execPath, ["scripts/restart-gateway.mjs"], { cwd: DEMO, encoding: "utf8" }); };

// **C2（审查）修正**：清场必须覆盖"异常/被中断"路径 —— §53 那次事故正是"中断留下半完成状态"。
// 做法：把清场做成**幂等函数**，并挂在 exit / SIGINT / SIGTERM 上（正常路径也调它，重复调用无副作用）。
// **C3（第三份审查）**：取锁必须在**注册清场之后** —— `exit` 处理器按**注册序**执行，
// 若先取锁（release 先注册）就会"**先放锁、后清场**"，清场那段（改声明 + 重启网关与宿主）会暴露在锁外。
let __cleaned = false;
let __cleanup = () => {};
requireOrchestrationLock(DEMO, path.basename(process.argv[1] || "?"));
const onExit = () => { try { __cleanup(); } catch (e) { /* 清场失败不能掩盖原错误 */ } };

// **清场必须在"任何改动之前"装好**（中断测试教我的，同 `rehearse-new-tree`）：第一版把赋值放在脚本后段，
// 于是"早于赋值的中断"调到空函数 ⇒ 声明行/文件/协议条目全留。
const spec = FORM === "package" ? (PKG + "/mod.js") : (".ui::" + FILE);
__cleanup = () => {
  if (__cleaned) return; __cleaned = true;
  try {
    if (FORM === "package") { fs.rmSync(path.join(ui, PKG), { recursive: true, force: true }); spawnSync(process.execPath, ["scripts/sync-wt-uses.mjs"], { cwd: DEMO, encoding: "utf8" }); }
    else fs.rmSync(path.join(ui, FILE), { force: true });
    if (fs.existsSync(decl)) fs.writeFileSync(decl, fs.readFileSync(decl, "utf8").split(String.fromCharCode(10)).filter((l) => l.indexOf(spec) < 0).join(String.fromCharCode(10)));
    spawnSync(process.execPath, ["scripts/restart-gateway.mjs"], { cwd: DEMO, encoding: "utf8" });
    spawnSync(process.execPath, ["scripts/up-all.mjs"], { cwd: DEMO, encoding: "utf8" });
  } catch (e) { /* 忽略 */ }
};
process.once("exit", onExit);
for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => { onExit(); process.exit(130); });
const gwPortFree = () => { try { return execSync("lsof -nP -iTCP:8011 -sTCP:LISTEN -t", { encoding: "utf8" }).trim() === ""; } catch (e) { return true; } };
/** 起全套；**失败重试一次**（实测：上一次的网关刚被 kill 时，偶发"端口未释放/就绪超时" ⇒ 演练误红）。 */
const upAll = () => {
  for (let i = 0; i < 6 && !gwPortFree(); i += 1) execSync("sleep 1");
  let r = spawnSync(process.execPath, ["scripts/up-all.mjs"], { cwd: DEMO, encoding: "utf8" });
  if (r.status !== 0) { execSync("sleep 3"); r = spawnSync(process.execPath, ["scripts/up-all.mjs"], { cwd: DEMO, encoding: "utf8" }); }
  return r;
};

console.log("== rehearse-onboarding（tree=" + TREE + " · form=" + FORM + " · " + (FORM === "package" ? "pkg=" + PKG : "file=" + FILE) + "）==");
const marker = FORM === "package" ? "EXT_PKG_LOADED" : "EXT_SINGLE_LOADED";
const body = [
  "// 第三方单文件 mod（演练件）：无 package.json，直接是树里的 ESM 文件。",
  "import { createElement as h } from \"react\";",
  "console.log(" + JSON.stringify(marker) + ", typeof h);",   // 必须**拼接**：写成数组元素会变成多行未闭合字符串
  "export default function P() { return h(\"span\", null, \"probe\"); }",
].join(String.fromCharCode(10)) + String.fromCharCode(10);
if (FORM === "package") {
  const dir = path.join(ui, PKG);
  fs.mkdirSync(path.join(dir, "src"), { recursive: true });
  // **零配置形态**（业主口径 · 用户复述）：第三方只写"我是谁 + 我导出什么 + 我用宿主哪份基础件"。
  // 实测三条（人工隔离检查，仓库里没有同名脚本）：
  //   · 不写 `dependencies` / `devDependencies` ⇒ **构建照样成功**（esmx/vite 由宿主出，`build-wt-ui` 从 demo 解析）；
  //   · 不写 `uses` ⇒ **能构建、能装载，但 React 被内联进产物**（1,571 B 里是整个 react 实现）
  //     ⇒ 与宿主各一份 ⇒ 单例破。**`uses` 是承重件**，不是配置噪音；
  //   · 写 `uses: ["ui_base"]` ⇒ 共享件走宿主的供给表（下面 ①d 用判据钉住，不再靠"跑起来就行"）。
  fs.writeFileSync(path.join(dir, "package.json"), JSON.stringify({
    name: PKG, version: "0.0.0", private: true, type: "module",
    esmx: { uses: ["ui_base"], exports: { "./mod.js": "./src/mod.js" } },
  }, null, 2) + String.fromCharCode(10));
  fs.writeFileSync(path.join(dir, "src/mod.js"), [
    "// 第三方包形态 mod（演练件）：package.json + src/，构建后由 import map 以逻辑名装载。",
    "import { createElement as h } from \"react\";",
    "console.log(\"" + marker + "\", typeof h);",
    "export default function P() { return h(\"span\", null, \"pkg-probe\"); }",
  ].join(String.fromCharCode(10)) + String.fromCharCode(10));
  step("① 造包（package.json + src/）", fs.existsSync(path.join(dir, "package.json")));
  const b = spawnSync(process.execPath, ["scripts/build-wt-ui.mjs", dir], { cwd: DEMO, encoding: "utf8" });
  step("①b 构建该包", b.status === 0, (String(b.stdout || "").split(String.fromCharCode(10)).filter((l) => l.indexOf("[build-wt-ui]") >= 0).pop() || "").trim().slice(0, 70));
  // ①d **共享件不得内联**（判据）：产物里出现 react 实现标记 ⇒ 该 mod 自带了第二份 react。
  // 由来：实测"零配置"时把 `uses` 也一起去掉 ⇒ 构建绿、装载绿、**产物里内联了整个 react**；
  // 只靠"控制台出现标记"根本发现不了。这条判据与 `check-externals` 同源，但**在演练里当场判**。
  let inlined = null;
  try {
    const cli = path.join(dir, "dist", "client");
    const prod = fs.readdirSync(cli).filter((f) => f.endsWith(".final.mjs"));
    const txt = prod.map((f) => fs.readFileSync(path.join(cli, f), "utf8")).join(String.fromCharCode(10));
    const hit = /react\.transitional\.element|react\.production\.min|Invalid hook call/.test(txt);
    inlined = hit ? "内联了 react（共享件未外部化）" : null;
  } catch (e) { inlined = "读产物失败：" + String(e.message).slice(0, 60); }
  step("①d 产物不内联共享件（uses 生效）", inlined === null, inlined || "externalized");
  const s = spawnSync(process.execPath, ["scripts/sync-wt-uses.mjs"], { cwd: DEMO, encoding: "utf8" });
  step("①c sync（建 uses/link/node_modules 条目）", s.status === 0 && String(s.stdout || "").indexOf(PKG) >= 0);
} else {
  fs.writeFileSync(path.join(ui, FILE), body);
  step("① 造件（无 package.json 的 ESM）", fs.existsSync(path.join(ui, FILE)));
}
const declTxt = fs.readFileSync(decl, "utf8");
// （`spec` 已在脚本早期定义 —— 清场需要它。）
if (declTxt.indexOf(spec) < 0) fs.writeFileSync(decl, declTxt.replace("## mods" + String.fromCharCode(10), "## mods" + String.fromCharCode(10) + "- " + NAME + ": " + spec + String.fromCharCode(10)));
step("② 声明加一行（" + spec + "）", fs.readFileSync(decl, "utf8").indexOf(spec) >= 0);

if (!keep) { /* 中间不重启，最后统一 */ }
killGateway();
await sleep(2000);
const up1 = upAll();
step("③ 重启网关+宿主（exit=" + up1.status + "）", up1.status === 0, (String(up1.stdout || "").split(String.fromCharCode(10)).filter((l) => l.trim()).slice(-2).join(" · ") || "").trim().slice(0, 90));

// ④ CDP：抓控制台标记
let logs = [];
try {
  const list = await (await fetch(CDP + "/json/list")).json();
  for (const t of list.filter((x) => x.type === "page")) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* 忽略 */ } }
  await sleep(500);
  await fetch(CDP + "/json/new?" + encodeURIComponent("about:blank"), { method: "PUT" });
  await sleep(500);
  const tab = (await (await fetch(CDP + "/json/list")).json()).filter((x) => x.type === "page").pop();
  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0; const pend = new Map();
  const send = (m, p) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); if (m.id) { const p = pend.get(m.id); if (p) { pend.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); } return; } if (m.method === "Runtime.consoleAPICalled") logs.push((m.params.args || []).map((a) => String(a.value === undefined ? a.description || a.type : a.value)).join(" ")); });
  await new Promise((r) => ws.addEventListener("open", r));
  await send("Runtime.enable"); await send("Page.enable");
  await send("Page.navigate", { url: "http://127.0.0.1:" + PORT + "/demo/?wt=genesis" });
  await sleep(13000);
  ws.close();
} catch (e) { /* CDP 不可用时下面会判红 */ }
const hit = logs.filter((l) => l.indexOf(marker) >= 0);
step("④ 客户端装载了该 mod（控制台标记）· 形态=" + FORM, hit.length > 0, hit[0] || "未出现");

// ⑤ 清场 + 判据回绿（清场函数已在脚本早期定义并挂上 exit/SIGINT/SIGTERM 兜底）。
__cleanup();
const cl = spawnSync(process.execPath, ["scripts/check-layers.mjs"], { cwd: DEMO, encoding: "utf8" });
const tf = spawnSync(process.execPath, ["scripts/check-tree-form.mjs"], { cwd: DEMO, encoding: "utf8" });
step("⑤ 清场后判据回绿（check-layers / check-tree-form）", cl.status === 0 && tf.status === 0, "exit=" + cl.status + "/" + tf.status);
const bad = rows.filter((r) => !r.ok).length;
console.log(bad ? "✗ rehearse-onboarding 失败 " + bad + " 项" : "✓ rehearse-onboarding 全绿：单文件形态可接入、且能干净收场");
process.exit(bad ? 1 : 0);