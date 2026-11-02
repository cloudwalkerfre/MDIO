// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-vp-cache");
armWatchdog("check-vp-cache", 10000);   /* = 10s；实测 0.1s（扫描 1 跑）⇒ ×2 */
/**
 * check-vp-cache —— **Vite+ 缓存三态判据的可复跑回归**。
 *
 * 自建临时 lab（"包 = 根"的同形形态），用**裸 glob 正项 + 裸负项**，断言三态：
 *   ② 无改动连跑 ⇒ **命中**；③ 改**被排除**的文件 ⇒ **仍命中**；④ 改**被包含**的文件 ⇒ **失效**。
 * 另有第 ① 态（冷跑不命中）作为前置。
 *
 * 为什么值得固化成脚本：这条判据专门拦两类坑 ——
 *   · 正项形态写错（对象 `{pattern, base}` 作正项实测**不纳入文件** ⇒ 输入集为空 ⇒ 永远"命中"假象）；
 *   · 负项漏掉**任务自己的产物**（如发布步写的文件）⇒ 无改动也永不命中。
 *
 * 依赖 `vpr`：按 `$MDIO_VP_BIN` → `/tmp/vp-exp/node_modules/.bin/vpr` → `vpr`（PATH）依次找；
 * 找不到则打印 `⊘ 跳过` 并**退 0**（不把"环境没装"当失败）。
 */
import { execSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const VP_CANDIDATES = [process.env.MDIO_VP_BIN, "/tmp/vp-exp/node_modules/.bin/vpr", "vpr"].filter(Boolean);
let VPR = null;
for (const c of VP_CANDIDATES) {
  if (c.indexOf("/") >= 0) { if (fs.existsSync(c)) { VPR = c; break; } continue; }
  try { execSync("command -v " + c, { stdio: "ignore" }); VPR = c; break; } catch (e) { /* 试下一个 */ }
}
if (!VPR) { console.log("⊘ 跳过：找不到 vpr（设 MDIO_VP_BIN 指向它即可跑这条回归）"); process.exit(0); }
const VP_PKG = path.join(path.dirname(path.dirname(VPR)), "vite-plus");

const lab = fs.mkdtempSync(path.join(os.tmpdir(), "vp-cache-lab-"));
const w = (p, c) => { fs.mkdirSync(path.dirname(path.join(lab, p)), { recursive: true }); fs.writeFileSync(path.join(lab, p), c); };
w("package.json", JSON.stringify({ name: "vp-cache-lab", private: true, type: "module" }, null, 2) + "\n");
w("scripts/stage.mjs", [
  'import fs from "node:fs";',
  'const t = fs.readFileSync("pkgs/x/src/data.txt", "utf8").trim();',
  'fs.mkdirSync("pkgs/x/.staging", { recursive: true });',
  'fs.writeFileSync("pkgs/x/.staging/out.txt", "STAGED:" + t);',
  'console.log("RAN stage");',
].join("\n"));
w("scripts/publish.mjs", 'import fs from "node:fs"; fs.writeFileSync("pkgs/x/published.txt", fs.readFileSync("pkgs/x/.staging/out.txt", "utf8")); console.log("RAN publish");');
w("pkgs/x/src/data.txt", "v1");
w("pkgs/x/dist.gen1/ignored.mjs", "// 被负项排除\n");
w("vite.config.ts", [
  'import { defineConfig } from "vite-plus";',
  'const dir = "pkgs/x";',
  'export default defineConfig({',
  '  run: { tasks: {',
  '    "build:x": { command: "node scripts/stage.mjs",',
  '      cache: { input: [dir + "/**", "!" + dir + "/dist*/**", "!" + dir + "/.staging/**", "!" + dir + "/published.txt"],',
  '               output: [dir + "/.staging/**"] } },',
  '    "publish:x": { command: "node scripts/publish.mjs", cache: false, dependsOn: ["build:x"] },',
  '  } },',
  '});',
].join("\n"));
fs.mkdirSync(path.join(lab, "node_modules"), { recursive: true });
try { fs.symlinkSync(VP_PKG, path.join(lab, "node_modules/vite-plus"), "dir"); } catch (e) { /* 已存在 */ }

const run = () => { try { return execSync(VPR + " publish:x 2>&1", { cwd: lab, encoding: "utf8" }); } catch (e) { return String(e.stdout || "") + String(e.stderr || ""); } };
// 必须**解析分数**：`0/2 cache hit (0%)` 也含子串"cache hit"（第一版就因此把 miss 判成 hit ⇒ 假红）。
const hits = (out) => { const m = /(\d+)\/(\d+) cache hit/.exec(out); return m ? Number(m[1]) : 0; };
const hit = (out) => hits(out) > 0;
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };

console.log("== check-vp-cache（lab: " + lab + "）==");
const o1 = run();
step("① 冷跑：不命中", !hit(o1), (o1.match(/\d+\/\d+ cache hit[^\n]*/) || [""])[0].slice(0, 40));
const o2 = run();
step("② 无改动连跑：命中", hit(o2), (o2.match(/\d+\/\d+ cache hit[^\n]*/) || [""])[0].slice(0, 40));
fs.appendFileSync(path.join(lab, "pkgs/x/dist.gen1/ignored.mjs"), "// changed\n");
const o3 = run();
step("③ 改**被排除**文件：仍命中", hit(o3), (o3.match(/\d+\/\d+ cache hit[^\n]*/) || [""])[0].slice(0, 40));
fs.writeFileSync(path.join(lab, "pkgs/x/src/data.txt"), "v2");
const o4 = run();
step("④ 改**被包含**文件：失效", !hit(o4), (o4.match(/\d+\/\d+ cache hit[^\n]*/) || [""])[0].slice(0, 40));
const published = fs.readFileSync(path.join(lab, "pkgs/x/published.txt"), "utf8");
step("⑤ 发布步每次都跑（产物为新值）", published === "STAGED:v2", "published=" + published);
const bad = rows.filter((r) => !r.ok).length;
// **自清**（默认）：跑完就删掉自建 lab —— 免得 /tmp 里越积越多（本轮就发现积了 4 个）。
// `--keep-lab` 可保留（排查用），届时会打印路径。
if (process.argv.includes("--keep-lab")) console.log("  · lab 保留在 " + lab);
else { try { fs.rmSync(lab, { recursive: true, force: true }); } catch (e) { /* 删不掉就算了 */ } }
console.log(bad ? "✗ check-vp-cache 失败 " + bad + " 项" : "✓ check-vp-cache 全绿");
process.exit(bad ? 1 : 0);
