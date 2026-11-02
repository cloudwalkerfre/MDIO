// @ts-check
/**
 * rehearse-css-unadopt —— **CSS mod"可取**且**可卸"的演练**（补齐缺口）。
 *
 * 采纳两侧都验过（`check-css-mods` 查 dev 采纳、`verify-prod` 查生产采纳），**"卸下"（`pruneCssMods`）一直没有网** ✓。
 * 本演练用**单文件 mod**（无需构建）走完整时序：
 *   ① 在树里放一个导出 `sheet` 的 ESM 文件，并加声明 ⇒ ② 重启网关（读新声明）⇒ ③ 重载页面 ⇒
 *   ④ 断言"该表被采纳"（按规则里的标记认领）⇒ ⑤ **删掉声明** + 重启网关 + 重载 ⇒
 *   ⑥ 断言"该表被卸下"（`document.adoptedStyleSheets` 里不再有它）⇒ ⑦ 清场并复查判据。
 *
 * 用法：node scripts/rehearse-css-unadopt.mjs [--keep-decl]
 */
import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const TREE = "worktrees/genesis";
const NAME = "CSS_PROBE";
const FILE = "css_probe_unadopt.js";
const MARK = "css-probe-unadopt-marker";
const ui = path.join(REPO, TREE, "ui");
const decl = path.join(REPO, TREE, "wt/.mds/ui.md");
const PORT = Number(process.env.PORT || 5178);
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const spec = ".ui::" + FILE;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };

// 清场必须**在任何改动之前**装好（§64/§66 的纪律）
let __cleaned = false;
let __cleanup = () => {};
const onExit = () => { try { __cleanup(); } catch (e) { /* 清场失败不掩盖原错误 */ } };
process.once("exit", onExit);
for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => { onExit(); process.exit(130); });
requireOrchestrationLock(DEMO, path.basename(process.argv[1] || "?"));
__cleanup = () => {
  if (__cleaned) return; __cleaned = true;
  try {
    fs.rmSync(path.join(ui, FILE), { force: true });
    if (fs.existsSync(decl)) fs.writeFileSync(decl, fs.readFileSync(decl, "utf8").split(String.fromCharCode(10)).filter((l) => l.indexOf(FILE) < 0).join(String.fromCharCode(10)));
    spawnSync(process.execPath, ["scripts/restart-gateway.mjs"], { cwd: DEMO, encoding: "utf8" });
    spawnSync(process.execPath, ["scripts/up-all.mjs"], { cwd: DEMO, encoding: "utf8" });
  } catch (e) { /* 忽略 */ }
};

console.log("== rehearse-css-unadopt（单文件 CSS mod）==");
fs.writeFileSync(path.join(ui, FILE), [
  "// CSS mod（演练件）：导出原生 CSSStyleSheet ⇒ 客户端 adoptCssMod 会采纳它。",
  "const sheet = new CSSStyleSheet();",
  "sheet.replaceSync('." + MARK + " { color: rgb(1, 2, 3); }');",
  "export { sheet };",
].join(String.fromCharCode(10)) + String.fromCharCode(10));
step("① 造件（导出 sheet 的 ESM）", fs.existsSync(path.join(ui, FILE)));
const addDecl = () => {
  const t = fs.readFileSync(decl, "utf8");
  if (t.indexOf(spec) >= 0) return;
  fs.writeFileSync(decl, t.replace("## mods" + String.fromCharCode(10), "## mods" + String.fromCharCode(10) + "- " + NAME + ": " + spec + String.fromCharCode(10)));
};
const delDecl = () => {
  const t = fs.readFileSync(decl, "utf8");
  fs.writeFileSync(decl, t.split(String.fromCharCode(10)).filter((l) => l.indexOf(spec) < 0 && l.indexOf(FILE) < 0).join(String.fromCharCode(10)));
};
addDecl();
step("② 声明加一行（" + spec + "）", fs.readFileSync(decl, "utf8").indexOf(spec) >= 0);

// 载入页面并用 CDP 查"是否采纳了带标记的那张表"
const probe = async () => {
  const list = await (await fetch(CDP + "/json/list")).json();
  for (const t of list.filter((x) => x.type === "page")) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* 忽略 */ } }
  await sleep(400);
  await fetch(CDP + "/json/new?" + encodeURIComponent("about:blank"), { method: "PUT" });
  await sleep(400);
  const tab = (await (await fetch(CDP + "/json/list")).json()).filter((x) => x.type === "page").pop();
  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0; const pend = new Map();
  const send = (m, p) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); if (m.id) { const p = pend.get(m.id); if (p) { pend.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); } } });
  await new Promise((r) => ws.addEventListener("open", r));
  await send("Runtime.enable"); await send("Page.enable");
  await send("Page.navigate", { url: "http://127.0.0.1:" + PORT + "/demo/?wt=genesis" });
  await sleep(12000);
  const ev = (expr) => send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }).then((r) => (r.result ? r.result.value : undefined));
  const has = await ev("document.adoptedStyleSheets.some((s) => { try { return [...s.cssRules].some((r) => String(r.cssText).indexOf('" + MARK + "') >= 0); } catch (e) { return false; } })");
  ws.close();
  return !!has;
};

execSync("node scripts/restart-gateway.mjs", { cwd: DEMO, stdio: "ignore" });
const adopted = await probe();
step("③④ 重启网关后重载页面：该 CSS mod **被采纳**", adopted, adopted ? "adoptedStyleSheets 里找到 " + MARK : "没找到");

delDecl();
step("⑤ 删掉声明", fs.readFileSync(decl, "utf8").indexOf(spec) < 0);
execSync("node scripts/restart-gateway.mjs", { cwd: DEMO, stdio: "ignore" });
const stillThere = await probe();
step("⑥ 重载页面：该表**被卸下**（pruneCssMods 生效）", !stillThere, stillThere ? "仍在（卸下没生效）" : "已不在 adoptedStyleSheets 里");

if (!process.argv.includes("--keep-decl")) {
  __cleanup();
  const cl = spawnSync(process.execPath, ["scripts/check-css-mods.mjs"], { cwd: DEMO, encoding: "utf8" });
  step("⑦ 清场后判据回绿（check-css-mods）", cl.status === 0, "exit=" + cl.status);
}
const bad = rows.filter((r) => !r.ok).length;
console.log(bad ? "✗ rehearse-css-unadopt 失败 " + bad + " 项" : "✓ rehearse-css-unadopt 全绿：CSS mod 可取**且**可卸");
process.exit(bad ? 1 : 0);
