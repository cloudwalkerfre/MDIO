// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-lock");
armWatchdog("check-lock", 10000);   /* = 10s；实测 2.1s（扫描 1 跑）⇒ ×2 */
/**
 * check-lock —— **按包构建锁的行为判据**（把 §60/§64 的手工实测固化成检查；审查 B1）。
 *
 * 四条断言（都用**可观测行为**，不看实现细节）：
 *   ① 活锁（有效 owner）⇒ **拿不到**（等到超时）；
 *   ② 锁文件**内容为空**（"删掉内容就想抢"的形态）⇒ 仍然**拿不到**（新锁按 mtime 判龄，一律尊重）；
 *   ③ **陈旧**锁（mtime 早于 staleMs 且 owner 是死 pid）⇒ **能抢占**并跑完；
 *   ④ 正常单跑后**锁没有残留**。
 *
 * 用法：node scripts/check-lock.mjs
 */
import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { acquireLockSync, releaseLockSync } from "./lib/flock.mjs";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const PKG = path.join(REPO, "exp_swap/ui/swap");
const LOCK = path.join(PKG, ".staging.lock");
const bail = (msg) => { console.log("  ✗ " + msg); process.exit(1); };

fs.rmSync(LOCK, { force: true });
console.log("check-lock: 目标包 " + path.relative(REPO, PKG));

// ① 活锁 ⇒ 拿不到
fs.writeFileSync(LOCK, JSON.stringify({ pid: process.pid, at: new Date().toISOString() }));
let ok = false;
try { acquireLockSync(LOCK, { waitMs: 800 }); } catch (e) { ok = true; }
fs.rmSync(LOCK, { force: true });
if (!ok) bail("① 活锁居然被拿到了");
console.log("  ✓ ① 活锁拿不到（等到超时）");

// ② 空内容的新锁 ⇒ 仍然拿不到（修复 v2 的关键性质）
fs.writeFileSync(LOCK, "");
ok = false;
try { acquireLockSync(LOCK, { waitMs: 800 }); } catch (e) { ok = true; }
fs.rmSync(LOCK, { force: true });
if (!ok) bail("② 空内容的锁被抢走了（等于删掉内容即可夺活锁）");
console.log("  ✓ ② 空内容的锁仍拿不到（按 mtime 判龄）");

// ③ 陈旧锁 ⇒ 能抢占（死 pid + 旧的 mtime）
fs.writeFileSync(LOCK, JSON.stringify({ pid: 999999, at: "2020-01-01T00:00:00Z" }));
const old = Date.now() / 1000 - 3600;
try { fs.utimesSync(LOCK, old, old); } catch (e) { /* 忽略 */ }
ok = false;
try { acquireLockSync(LOCK, { waitMs: 1500, staleMs: 600000 }); ok = true; } catch (e) { /* 没抢到 */ }
releaseLockSync(LOCK);
if (!ok) bail("③ 陈旧锁没能被抢占（会永久卡住）");
console.log("  ✓ ③ 陈旧锁被抢占并可释放");

// ④ 真跑一次构建 ⇒ 锁不残留
const b = spawnSync(process.execPath, ["scripts/build-wt-ui.mjs", PKG], { cwd: DEMO, encoding: "utf8" });
const left = fs.existsSync(LOCK);
if (b.status !== 0) bail("④ 构建失败：" + String(b.stdout || "").split(String.fromCharCode(10)).filter((l) => l.indexOf("[build-wt-ui]") >= 0).pop());
if (left) bail("④ 构建结束后锁仍有残留：" + LOCK);
console.log("  ✓ ④ 正常构建后锁无残留");
console.log("✓ check-lock 全绿：活锁拿不到 · 空内容锁拿不到 · 陈旧锁可抢占 · 正常路径不留锁");

// 收尾退出：老的测试脚本跑完不收 CDP socket ⇒ 不显式退出就会被自己的 deadline 判成卡死。
// 退出码沿用脚本自己设的 process.exitCode（没设 = 0）；失败路径若已 process.exit(1) 则不受影响。
process.exit(process.exitCode === undefined ? 0 : process.exitCode);
