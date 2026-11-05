// @ts-check
/**
 * verify-all —— **一条命令回答"现在是否健康"**（与 `up-all.mjs` 配对：一个起、一个验）。
 *
 * 串跑判据并汇总成一张表 + 退出码：
 *   judge · check-layers · pages-live · swap-live（默认）· e2e（需 --with-e2e，约 3–5 分钟）
 *
 * 用法：node scripts/verify-all.mjs [--with-e2e] [--only a,b]
 */
import { spawn } from "node:child_process";
import path from "node:path";
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";

// **自动化运行标记**：子进程继承 ⇒ 宿主据此**关掉** dev 期"改源码 ⇒ 自动重编 ⇒ 整页重载"
// 的监视器（`MDIO_WATCH_MODS`）。为什么必须关：本套里的 rehearsal 会**写 mod 源码**（换 CSS 件、接新树…），
// 监视器会在测试中途触发重编 + 整页重载 ⇒ 断言被打断（实测：swap-live 6 项、rehearse-css-unadopt 1 项抖动）。
// 手跑 dev 时没有这个标记 ⇒ 监视器默认开（那才是开发者要的行为）。
process.env.MDIO_TESTING = "1";

const DEMO = path.resolve(import.meta.dirname, "..");
// **C3（第三份审查）**：`verify-all` 与"会停/重启宿主或网关"的工具并发时，双方会得到无意义的红
//（`dev-provider` 断言裸名指向 shim、活体项共用同一宿主与 CDP）⇒ 这里也持编排锁：拿不到就快速失败退 2。
// 顺序有讲究：`requireOrchestrationLock` 在**拿到锁后会把 `MDIO_ORCH_LOCK_HELD=1` 写进自己的 env**（给子进程继承用）
//   ⇒ 必须**先读**嵌套标记、再取锁；否则本进程永远被自己判成 NESTED（实测踩到：
//   宿主重启被整段跳过 ⇒ externals 重建产物后所有浏览器套件拿到过期 integrity，整片红）。
const NESTED = process.env.MDIO_ORCH_LOCK_HELD === "1";   // 嵌套（rehearse-coldstart 会调本脚本）⇒ 不重起宿主
requireOrchestrationLock(DEMO, "verify-all");
process.env.MDIO_ORCH_LOCK_HELD = "1";   // 让子套件（如 --with-tree 的演练）不再重复取锁
const args = process.argv.slice(2);
const withE2e = args.includes("--with-e2e");
const withVp = args.includes("--with-vp");   // Vite+ 缓存三态回归（需要 vpr；没有则该项自行"⊘ 跳过"并退 0）
const withTree = args.includes("--with-tree");
const withShell = args.includes("--with-shell");   // J1 缺件降级演练（会**拿走某件产物**再还原 + 重启宿主 ⇒ 默认不跑）   // 新树从零接入演练（会建树/登记/构建，跑完自清；默认不跑）
const withCss = args.includes("--with-css");     // CSS mod 可取可卸演练（会改声明 + 两次重启网关；默认不跑）
const onlyIdx = args.indexOf("--only");
const only = onlyIdx >= 0 ? String(args[onlyIdx + 1] || "").split(",") : null;

// **先把宿主以"测试标记"重起一次**：本套里有的套件会写 mod 源码，若宿主还开着 dev 监视器
// （改源码 ⇒ 自动重编 ⇒ 整页重载），那记重载会打断**后续**套件的断言（实测抖动：swap-live 6 项 / css-unadopt 1 项）。
// 重起把 `MDIO_TESTING=1` 带进宿主 ⇒ `WATCH_MODS=0`（显式 `MDIO_WATCH_MODS` 仍优先）。
// 嵌套调用（rehearse-coldstart）跳过：它自己编排宿主。
const restartHostAndWarm = async (why) => {
  const rh = spawn(process.execPath, ["scripts/restart-host.mjs"], { cwd: DEMO, stdio: "inherit" });
  await new Promise((r) => rh.on("close", r));
  console.log("· 宿主已重启（" + why + "）");
  // **预热（照 UI_TEST_SOP §2.3 的处方）**：宿主刚重启时首个页面要付冷启动代价，
  // SOP 记的症状是「8s 连接超时被冷启动吃掉」⇒ 页面 boot 失败 ⇒ 依赖页面的套件**整片假红**
  //（实测：套件内 css-mods adopted=0 / doc-render 180s 超时 / pages-live / swap-live 全红，而套件外手动跑全绿）。
  // 处方 = **先预热再 new_page**：取一次页面 ⇒ 按 import map 把模块各取一遍 ⇒ 再取客户端入口（最大的一块转换）。
  try {
    const t0 = Date.now();
    const base = "http://127.0.0.1:5178";
    const r = await fetch(base + "/?wt=genesis", { signal: AbortSignal.timeout(8000) });
    const html = await r.text();
    const m = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
    const urls = m ? Object.values(JSON.parse(m[1]).imports || {}).filter((u) => typeof u === "string" && u.startsWith("/")).slice(0, 24) : [];
    await Promise.all(urls.map((u) => fetch(base + u, { signal: AbortSignal.timeout(8000) }).catch(() => null)));
    await fetch(base + "/demo/src/entry.client.ts", { signal: AbortSignal.timeout(15000) }).catch(() => null);
    console.log("· 预热完成（" + urls.length + " 个模块 + 客户端入口，" + (Date.now() - t0) + "ms）");
  } catch (e) { console.log("· 预热失败（不阻塞套件）：" + String((e && e.message) || e).slice(0, 70)); }
};
/** 会重建产物、让 dev 宿主 manifest 换代的一组：跑完后新页面会拿到过期 integrity（实测卡「正在装载…」）。 */
const BUILD_SUITES = new Set(["externals"]);
/** 依赖浏览器/dev 页面的套件：宿主被构建套件弄脏时，开跑前先恢复。 */
const BROWSER_SUITES = new Set(["module-graph", "css-mods", "doc-render", "cdp-cursor", "pages-live", "swap-live", "e2e", "check-vp-cache", "rehearse-new-tree", "rehearse-shell-missing", "rehearse-css-unadopt"]);
let hostDirty = false;
if (!NESTED) await restartHostAndWarm("测试标记 + 冷启动预热");

/** 每一项：名字 · 命令 · 命中"成功"的判据（正则，取自各套件自己的总结行）。 */
const SUITES = [
  { name: "judge", cmd: ["node", "scripts/judge.mjs"], ok: /判官绿/ },
  { name: "check-layers", cmd: ["node", "scripts/check-layers.mjs"], ok: /分层方向检查通过/ },
  // （docs-sync 不存在：它的存在只是"管文档"——要求每个脚本在 README 里被提到。文档不挂工具。）
  { name: "protocol", cmd: ["node", "scripts/check-protocol.mjs"], ok: /check-protocol 全绿/ },        // §17：挂载三处条目
  { name: "externals", cmd: ["node", "scripts/check-externals.mjs"], ok: /check-externals 全绿/ },      // §21.1：产物不得内联共享依赖
  { name: "tree-form", cmd: ["node", "scripts/check-tree-form.mjs"], ok: /check-tree-form 全绿/ },     // 在范围内的树：声明只用逻辑名 / ui 只放 mod 包 / 无官方件副本
  // 树从哪来：接口层含 /.mds/ui.md · 第 1 条 WS 树帧 == 同 wt 的 HTTP 树（id/path 逐项）· mounts 过滤仍在。
  // 只连网关（不碰浏览器，约 3 秒）；网关不在时它自己打印"⊘ 跳过"并退 0 ⇒ 本表按跳过记（见 README「树的两条来源」）。
  { name: "tree-source", cmd: ["node", "scripts/check-tree-source.mjs"], ok: /check-tree-source 全绿/ },
  { name: "no-residue", cmd: ["node", "scripts/check-no-residue.mjs"], ok: /check-no-residue 全绿/ },   // 探针/fixture 不留残渣
  // link 目标真实性：改名按名字的规则抓不到 `"x": "link:../core"` 这类**相对路径值**，
  // 结果 ui_devview→ui_core 与 ui_std→ui_base 两条 link 一直指向不存在的目录而**所有判据全绿**。
  // 该判据解析每个 link: 目标的真实路径 + 校验名实相符 + 查 node_modules 悬空链接，并自报扫描量（防空跑）。
  { name: "links", cmd: ["node", "scripts/check-links.mjs"], ok: /check-links 全绿/ },
  // 原判据钉的是 §58「开发必须用宿主 shim」——shim 已按官方模型删除 ⇒ 现在钉官方不变量
  //（每个共享包只有一个所有者、闭包全部接到它）。浏览器侧的资源图不变量见下一条 module-graph。
  { name: "dev-provider", cmd: ["node", "scripts/check-dev-provider.mjs"], ok: /check-dev-provider 全绿/ },
  { name: "module-graph", cmd: ["node", "scripts/check-module-graph.mjs"], ok: /check-module-graph 全绿/, timeoutMs: 120000 },
  { name: "esmx-validate", cmd: ["node", "scripts/check-esmx-validate.mjs"], ok: /check-esmx-validate 全绿/, timeoutMs: 120000 },
  { name: "css-mods", cmd: ["node", "scripts/check-css-mods.mjs"], ok: /check-css-mods 全绿/ },          // §63：dev 路径上 CSS mod 真被采纳
  { name: "doc-render", cmd: ["node", "scripts/check-doc-render.mjs"], ok: /check-doc-render 全绿/, env: { SHOT_DIR: ".tmp/steps-gui/verify" } },   // §71：文档面 DOM 与交付树逐项对齐（"命中≠能画"那类静默空白）
  { name: "lock", cmd: ["node", "scripts/check-lock.mjs"], ok: /check-lock 全绿/ },                   // §64：按包构建锁的行为（活锁/空内容/陈旧/无残留）
  // 声明槽位（appview）解析与取值校验：纯 Node 合成声明树，不需要宿主/浏览器（套件自带 10s 看门狗 ⇒ timeoutMs ×2）。
  { name: "check-slot", cmd: ["node", "scripts/check-slot.mjs"], ok: /槽位解析判据全绿/, timeoutMs: 20000 },
  // （plan-structure / survey-structure 两条不存在：它们靠正则自己解析 md 结构，违反「md 的解析只有一个来源 = 引擎」）
  { name: "pages-live", cmd: ["node", "test/pages-live.mjs"], ok: /pages-live 全绿/, env: { SHOT_DIR: ".tmp/steps-gui/verify" } },
  { name: "swap-live", cmd: ["node", "test/swap-live.mjs"], ok: /swap-live 全绿/ },
  { name: "e2e", cmd: ["node", "test/e2e.mjs"], ok: /e2e 全部通过/, needsFlag: true, timeoutMs: 420000 },
  // 可选：Vite+ 缓存三态回归（自建临时 lab；找不到 vpr 会打印"⊘ 跳过"并退 0 ⇒ 视为通过）
  { name: "check-vp-cache", cmd: ["node", "scripts/check-vp-cache.mjs"], ok: /check-vp-cache 全绿|⊘ 跳过/, vpFlag: true, timeoutMs: 240000 },
  // 可选：新树从零接入演练（建树 → 登记 → 依赖落点 → 构建 → sync → 边界实证 → 清场）
  { name: "rehearse-new-tree", cmd: ["node", "scripts/rehearse-new-tree.mjs"], ok: /rehearse-new-tree 全绿/, treeFlag: true, timeoutMs: 600000 },
  // J1（业主要求）：挂载表里有产物被删时，页面必须 200 且日志出现 shell 缺件告警。自持宿主（告警按进程去重）。
  { name: "rehearse-shell-missing", cmd: ["node", "scripts/rehearse-shell-missing.mjs"], ok: /rehearse-shell-missing 全绿/, shellFlag: true, timeoutMs: 300000 },
  // J2（业主要求）：__MDIO_CURSOR_DIFF__ 升成真判据，覆盖切档往返（浏览器侧，CDP）。
  { name: "cdp-cursor", cmd: ["node", "test/cdp-cursor.mjs"], ok: /cdp-cursor 全绿/, timeoutMs: 120000 },
  // 准入门（B5 反演判据 · 修复后固化）：挂载连接必须能开自己的 doc、他 wt 必须拒、
  // 未知 conn_wt 必须拒、boot 声明必须放行、A2 视图归属与 A4 顺序不变量 —— 免得上一次「挂载树全拒」
  // 只靠人肉发现（当时 ⑤⑦ 两条被业主先撞上）。⑨/⑪ 两条 SKIP 在脚本内部标注理由、不算红。
  { name: "admit-gate", cmd: ["node", "scripts/check-admit-gate.mjs"], ok: /check-admit-gate 全绿/, timeoutMs: 150000 },
  // 孪生对照（业主口径）：同一内容 ± 顶头 `# 标题` ⇒ 渲染必须一致（计算样式逐项相等）。
  // 专治「文档体丢了 .mdio-std 排版作用域 / 兜底没走 std」这类**只验计数抓不到**的排版回归。
  { name: "render-parity", cmd: ["node", "scripts/check-render-parity.mjs"], ok: /check-render-parity 全绿/, timeoutMs: 240000 },
  // CSS 类面（业主报「[清空 底部] 的样式怎么没了」换来的）：`className` 用到但任何 .css 里
  // 都没定义的 token ⇒ 清死类清错、类名拼错、样式文件漏 import 都能当场抓住。
  { name: "css-classes", cmd: ["node", "scripts/check-css-classes.mjs"], ok: /check-css-classes 全绿/, timeoutMs: 60000 },
  // 可选：CSS mod"可取可卸"演练（§67：补上 Z.3 第 4 条的缺口）
  { name: "rehearse-css-unadopt", cmd: ["node", "scripts/rehearse-css-unadopt.mjs"], ok: /rehearse-css-unadopt 全绿/, cssFlag: true, timeoutMs: 600000 },
];

const run = (s) => new Promise((res) => {
  const t0 = Date.now();
  const p = spawn(s.cmd[0], s.cmd.slice(1), { cwd: DEMO, env: Object.assign({}, process.env, s.env || {}), stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  const timer = setTimeout(() => { try { p.kill("SIGKILL"); } catch (e) { /* 忽略 */ } }, s.timeoutMs || 180000);
  p.stdout.on("data", (b) => { out += String(b); });
  p.stderr.on("data", (b) => { out += String(b); });
  p.on("close", (code) => { clearTimeout(timer); res({ code, out, ms: Date.now() - t0 }); });
});

const rows = [];
for (const s of SUITES) {
  if (only && !only.includes(s.name)) continue;
  if (s.needsFlag && !withE2e) { rows.push({ name: s.name, skipped: true, why: "未加 --with-e2e" }); continue; }
  if (s.vpFlag && !withVp) { rows.push({ name: s.name, skipped: true, why: "未加 --with-vp" }); continue; }
  if (s.treeFlag && !withTree) { rows.push({ name: s.name, skipped: true, why: "未加 --with-tree" }); continue; }
  if (s.shellFlag && !withShell) { rows.push({ name: s.name, skipped: true, why: "未加 --with-shell" }); continue; }
  if (s.cssFlag && !withCss) { rows.push({ name: s.name, skipped: true, why: "未加 --with-css" }); continue; }
  // 构建类套件会覆盖 dev 宿主正在用的产物/manifest ⇒ 之后的浏览器套件会拿到过期 integrity（实测页面卡「正在装载…」）。
  // 恢复一次宿主 + 预热再跑浏览器套件；单跑这些套件本来是全绿的。
  if (!NESTED && hostDirty && BROWSER_SUITES.has(s.name)) { await restartHostAndWarm("构建类套件之后恢复 dev 宿主"); hostDirty = false; }
  process.stdout.write("· 跑 " + s.name + " … ");
  const r = await run(s);
  if (BUILD_SUITES.has(s.name)) hostDirty = true;
  // **B3（审查）修正**：`⊘ 跳过` 不等于「通过」—— 例如 `check-vp-cache` 在没有 `vpr` 的机器上会打印「⊘ 跳过」并退 0，
  // 若照旧判 ✓，§49.1 里「该判据已可证伪」的说法就不成立（表格会给出虚假的安心）。⇒ 归入**跳过**并写明原因。
  if (r.code === 0 && /⊘ 跳过/.test(r.out)) {   // C1（第三份审查）：必须同时要求退出码 0，否则"打印跳过但其实失败"会被吞掉
    const why = (r.out.split(String.fromCharCode(10)).filter((l) => l.indexOf("⊘") >= 0).pop() || "自身跳过").trim().slice(0, 60);
    console.log("⊘（跳过：" + why + "）");
    rows.push({ name: s.name, skipped: true, why: why });
    continue;
  }
  const ok = r.code === 0 && s.ok.test(r.out);   // 退出码也要为 0（不只看总结行）
  const summary = (r.out.split(String.fromCharCode(10)).filter((l) => /全绿|全部通过|判官绿|检查通过|失败|✗/.test(l)).pop() || "").trim().slice(0, 90);
  console.log(ok ? "✓（" + r.ms + "ms）" : "✗（" + r.ms + "ms）");
  rows.push({ name: s.name, ok, ms: r.ms, summary });
}

console.log(String.fromCharCode(10) + "== 状态表 ==");
for (const r of rows) {
  if (r.skipped) { console.log("  ⊘ " + r.name + "  （跳过：" + (r.why || "") + "）"); continue; }
  console.log("  " + (r.ok ? "✓" : "✗") + " " + r.name.padEnd(14) + " " + String(r.ms + "ms").padStart(8) + "  " + r.summary);
}
const failed = rows.filter((r) => !r.skipped && !r.ok).length;
console.log(failed ? "✗ 失败 " + failed + " 项" : "✓ 全部通过" + (withE2e ? "" : "（e2e 未跑：加 --with-e2e）") + (withVp ? "" : "（Vite+ 缓存回归未跑：加 --with-vp）") + (withTree ? "" : "（新树接入演练未跑：加 --with-tree）") + (withShell ? "" : "（缺件降级演练未跑：加 --with-shell）") + (withCss ? "" : "（CSS 卸下演练未跑：加 --with-css）"));
process.exit(failed ? 1 : 0);
