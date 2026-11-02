// @ts-check
/**
 * verify-specials —— **四条"专项"判据的一条命令**（与 `verify-all` 分工：`verify-all` 跑"默认面 + 可选面"，本脚本跑"会重启服务的专项"）。
 *
 * 为什么单独成命令：`rehearse-coldstart` 第 ⑤ 步会嵌套调 `verify-all`，这类**嵌套编排**只有"把专项也跑一遍"才会暴露；
 * 而它们各自都会停/重启宿主或网关（耗时数分钟）⇒ 不适合塞进 `verify-all` 的默认面。
 *
 * 它**自己持编排锁**（子进程靠 `MDIO_ORCH_LOCK_HELD` 继承 ⇒ 不会自我拒绝）。
 * 用法：node scripts/verify-specials.mjs [--only a,b]
 */
import { spawn } from "node:child_process";
import path from "node:path";

// 同 verify-all：自动化运行 ⇒ 宿主关掉 dev 监视器（见 verify-all.mjs 的说明）。
process.env.MDIO_TESTING = "1";
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";

const DEMO = path.resolve(import.meta.dirname, "..");
requireOrchestrationLock(DEMO, "verify-specials");
const args = process.argv.slice(2);
const onlyIdx = args.indexOf("--only");
const only = onlyIdx >= 0 ? String(args[onlyIdx + 1] || "").split(",") : null;

const SUITES = [
  { name: "verify-prod", cmd: ["node", "scripts/verify-prod.mjs"], ok: /verify-prod 全绿/, timeoutMs: 300000 },
  { name: "rehearse-coldstart", cmd: ["node", "scripts/rehearse-coldstart.mjs"], ok: /rehearse-coldstart 全绿/, timeoutMs: 300000 },
  { name: "onboarding-single", cmd: ["node", "scripts/rehearse-onboarding.mjs", "--form", "single"], ok: /rehearse-onboarding 全绿/, timeoutMs: 300000 },
  { name: "onboarding-package", cmd: ["node", "scripts/rehearse-onboarding.mjs", "--form", "package"], ok: /rehearse-onboarding 全绿/, timeoutMs: 300000 },
];

const run = (s) => new Promise((res) => {
  const t0 = Date.now();
  const p = spawn(s.cmd[0], s.cmd.slice(1), { cwd: DEMO, stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  const timer = setTimeout(() => { try { p.kill("SIGKILL"); } catch (e) { /* 忽略 */ } }, s.timeoutMs || 300000);
  p.stdout.on("data", (b) => { out += String(b); });
  p.stderr.on("data", (b) => { out += String(b); });
  p.on("close", (code) => { clearTimeout(timer); res({ code, out, ms: Date.now() - t0 }); });
});

console.log("== verify-specials（四条专项；约 3–5 分钟）==");
const rows = [];
for (const s of SUITES) {
  if (only && !only.includes(s.name)) continue;
  process.stdout.write("· 跑 " + s.name + " … ");
  const r = await run(s);
  const ok = r.code === 0 && s.ok.test(r.out);
  const summary = (r.out.split(String.fromCharCode(10)).filter((l) => /全绿|失败|✗/.test(l)).pop() || "").trim().slice(0, 80);
  console.log(ok ? "✓（" + r.ms + "ms）" : "✗（" + r.ms + "ms）");
  rows.push({ name: s.name, ok, ms: r.ms, summary });
}
console.log(String.fromCharCode(10) + "== 状态表 ==");
for (const r of rows) console.log("  " + (r.ok ? "✓" : "✗") + " " + r.name.padEnd(20) + " " + String(r.ms + "ms").padStart(8) + "  " + r.summary);
const failed = rows.filter((r) => !r.ok).length;
console.log(failed ? "✗ 失败 " + failed + " 项" : "✓ 全部通过（专项 " + rows.length + " 项）");
process.exit(failed ? 1 : 0);
