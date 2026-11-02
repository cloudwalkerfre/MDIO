// @ts-check
/**
 * orchestrate —— **编排互斥**（审查 C3）：会"停/重启宿主或网关"的工具之间必须串行。
 *
 * 涉及：`verify-prod`（停 dev 宿主跑 preview，再恢复）· 三个 `rehearse-*`（重启网关/宿主）。
 * 它们与 `verify-all` 的活体项（`pages-live`/`swap-live` 共用同一宿主与 CDP）并发时，**双方都会得到无意义的红**。
 *
 * 语义：**快速失败**（不排队）——拿不到锁就打印占用者并退 2，让调用者显式决定"等一下再跑"。
 * （排队会更糟：一次演练可能几十秒，串起来会让人以为卡死。）
 */
import fs from "node:fs";
import path from "node:path";
import { acquireLockSync, releaseLockSync } from "./flock.mjs";

export function requireOrchestrationLock(demoDir, who) {
  // **继承标记**（C3）：`verify-all` 自己持锁后，它派生的子套件（例如 `--with-tree` 的演练）会继承
  // `MDIO_ORCH_LOCK_HELD=1` ⇒ 它们**不再取锁**（否则同一个编排会被自己的子进程拒之门外）。
  if (String(process.env.MDIO_ORCH_LOCK_HELD || "") === "1") return () => {};
  const lockPath = path.join(demoDir, ".tmp/orchestrate.lock");
  try {
    acquireLockSync(lockPath, { waitMs: 0, staleMs: 600000 });   // waitMs:0 ⇒ 第一次拿不到就抛（快速失败）
  } catch (e) {
    let owner = "（读不到占用者信息）";
    try { owner = fs.readFileSync(path.join(lockPath, "owner.json"), "utf8"); } catch (e2) { /* 忽略 */ }
    console.error("✗ 有别的编排在跑（编排锁 " + path.relative(demoDir, lockPath) + " 被占用）：" + owner);
    console.error("  ⇒ " + who + " 会停/重启宿主或网关，与它并发只会得到无意义的红。等它跑完再试，或确认无残留进程后删掉该锁目录。");
    process.exit(2);
  }
  // **取锁者给自己的子进程打继承标记**（否则"演练里再跑 verify-all"会自我拒绝：§69 实测到 rehearse-coldstart 第 ⑤ 步）。
  process.env.MDIO_ORCH_LOCK_HELD = "1";
  const release = () => releaseLockSync(lockPath);
  process.once("exit", release);
  return release;
}
