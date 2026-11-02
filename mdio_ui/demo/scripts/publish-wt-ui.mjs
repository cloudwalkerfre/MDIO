// @ts-check
/**
 * publish-wt-ui —— **原子上架**的发布半步：把 `<uiDir>/.staging/dist` 搬成新代际
 * `dist.gen<ts>`，再用**原子 rename** 把符号链接 `dist` 改指过去，最后清理旧代（保留两代）。
 *
 * 为什么独立成脚本：与 Vite+ 的任务对配套 —— `build:<uid>` 缓存的是 `.staging/**`，命中时**重放**它；
 * 而本脚本所在的任务 `cache: false`（**每次必跑**）⇒ 于是"重放永不覆盖 dist"与"任何情况下都原子发布"同时成立。
 *
 * 用法：node scripts/publish-wt-ui.mjs <wtRoot|uiDir>
 * 退出码：0 = 已发布；1 = 没有可发布的旁路产物（或发布失败）。
 */
import fs from "node:fs";
import path from "node:path";

const arg = process.argv[2];
if (!arg) { console.error("用法: node scripts/publish-wt-ui.mjs <wtRoot|uiDir>"); process.exit(2); }
const resolved = path.resolve(arg);
const uiDir = fs.existsSync(path.join(resolved, "ui", "package.json")) ? path.join(resolved, "ui") : resolved;
const stagingDir = path.join(uiDir, ".staging");
const stagedDist = path.join(stagingDir, "dist");

// **与 `build-wt-ui` 共用同一把按包锁**（审查 C1）：构建与发布对**同一个包**永不交错；
// 否则并发构建删改 staging 时，发布可能把**混合代际**搬成新代。
import { acquireLockSync, releaseLockSync } from "./lib/flock.mjs";
const LOCK = path.join(uiDir, ".staging.lock");
acquireLockSync(LOCK, { waitMs: 180000 });

// **闸门**：既要有产物，也要有构建侧写的**成功标记** —— 否则可能把"失败构建留下的半成品"发布出去
//（只验 manifest 存在不够：失败可能发生在 client 之后）。
if (!fs.existsSync(path.join(stagedDist, "client", "manifest.json"))) {
  console.error("[publish-wt-ui] 没有可发布的旁路产物（缺 .staging/dist/client/manifest.json）：", uiDir);
  process.exit(1);
}
if (!fs.existsSync(path.join(stagingDir, ".ok.json"))) {
  console.error("[publish-wt-ui] 旁路产物**缺少成功标记**（.staging/.ok.json）⇒ 拒发（可能是失败/中断的构建残留）：", uiDir);
  process.exit(1);
}

const gen = "dist.gen" + Date.now();
fs.renameSync(stagedDist, path.join(uiDir, gen));
const cur = path.join(uiDir, "dist");
const linkTmp = path.join(uiDir, ".dist-link.tmp");
try { fs.unlinkSync(linkTmp); } catch (e) { /* 不存在 */ }
fs.symlinkSync(gen, linkTmp);
let st = null;
try { st = fs.lstatSync(cur); } catch (e) { /* 还没有 dist */ }
if (st && !st.isSymbolicLink()) fs.renameSync(cur, path.join(uiDir, "dist.gen-old-" + Date.now()));   // 首次：实体目录转代
fs.renameSync(linkTmp, cur);   // ← 原子改指：读方要么旧代要么新代

const gens = fs.readdirSync(uiDir)
  .filter((n) => n.indexOf("dist.gen") === 0)
  .map((n) => ({ n, t: fs.statSync(path.join(uiDir, n)).mtimeMs }))
  .sort((a, b) => b.t - a.t);
let pruned = 0;
for (const g of gens.slice(2)) { try { fs.rmSync(path.join(uiDir, g.n), { recursive: true, force: true }); pruned += 1; } catch (e) { /* 删不掉就算了 */ } }
fs.rmSync(stagingDir, { recursive: true, force: true });
// **发布成功才把 `published` 置真**（C2）：与 discovery 的 `stamp-unpublished` 判定配对，
// 保证"戳说 ok"与"dist 已是这一代"两件事同时成立。
try {
  const sp = path.join(uiDir, ".esmx-build.json");
  const s = JSON.parse(fs.readFileSync(sp, "utf8"));
  s.published = true;
  s.publishedAt = new Date().toISOString();
  fs.writeFileSync(sp, JSON.stringify(s, null, 2) + "\n");
} catch (e) { console.warn("[publish-wt-ui] 更新戳失败（published 未置真）：" + String(e.message)); }
releaseLockSync(LOCK);
console.log("[publish-wt-ui] ✓ " + path.basename(uiDir) + " ⇒ " + gen + "（清理旧代 " + pruned + " 个）");
