// @ts-check
/**
 * flock —— **按资源的文件锁**（审查 C1 修复：同一包的 `.staging` 不能被两个构建同时删改）。
 *
 * 为什么需要：`build-wt-ui.mjs` 的 staging 是固定路径 `<pkg>/.staging`，`prepareStaging()` 先 `rmSync` 再拷贝。
 * 触发源至少有四个：`ensure-built` 预构建（`up-all`/`restart-host`）· 宿主 `[autoBuild]` · 手工构建 · （将来）Vite+ 任务。
 * 无锁时互相删改 + `.ok.json` 先落者放行 ⇒ `publish-wt-ui` 可能发布**混合代际**。
 *
 * 语义：`O_EXCL` 建锁文件（内容含 pid/时间）→ 拿不到就轮询等待 → 超时按"陈旧锁"抢占（进程已死或太久）。
 * 用法：`await withLock(lockPath, async () => { ... })`；异常路径自动释放（`finally`）。
 */
import fs from "node:fs";
import path from "node:path";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function withLock(lockPath, fn, opts = {}) {
  const waitMs = Number(opts.waitMs || 120000);
  const staleMs = Number(opts.staleMs || 300000);
  const t0 = Date.now();
  fs.mkdirSync(path.dirname(lockPath), { recursive: true });
  let held = false;
  while (!held) {
    try {
      const fd = fs.openSync(lockPath, "wx");
      fs.writeSync(fd, JSON.stringify({ pid: process.pid, at: new Date().toISOString() }));
      fs.closeSync(fd);
      held = true;
    } catch (e) {
      let stale = false;
      try {
        const info = JSON.parse(fs.readFileSync(lockPath, "utf8"));
        const age = Date.now() - Date.parse(info.at || 0);
        let alive = false;
        try { process.kill(Number(info.pid), 0); alive = true; } catch (e2) { alive = false; }
        stale = !alive || age > staleMs;
      } catch (e2) { stale = true; }
      if (stale) { try { fs.unlinkSync(lockPath); } catch (e2) { /* 已被别人清掉 */ } continue; }
      if (Date.now() - t0 > waitMs) throw new Error("等锁超时（" + lockPath + "）");
      await sleep(120);
    }
  }
  try { return await fn(); } finally { try { fs.unlinkSync(lockPath); } catch (e) { /* 已被清 */ } }
}
/** **同步**获取锁（供直线脚本用：不必把整段逻辑改成 async）。拿到就返回，拿不到就等；超时或陈旧则抢占。 */
export function acquireLockSync(lockPath, opts = {}) {
  const waitMs = Number(opts.waitMs || 120000);
  const staleMs = Number(opts.staleMs || 300000);
  const t0 = Date.now();
  fs.mkdirSync(path.dirname(lockPath), { recursive: true });
  const sleeper = new Int32Array(new SharedArrayBuffer(4));
  for (;;) {
    try {
      // **原子建立（第三版，前两版都被实测否掉）**：
      //   第一版 `mkdir` + 写 owner 两步 ⇒ 缝里读不到 owner 就误判陈旧、**抢活锁**（审查 C2 实测）；
      //   第二版"写临时目录再 `rename` 过去" ⇒ 仍会被抢：POSIX 下 `rename` 到**空目录**是**成功**的，
      //     而"删掉 owner.json"恰好把锁目录留空（我自己实测复现 ✗）。
      //   现在：锁是**一个文件**，用 `link(2)` 建立 —— 它**与目标内容无关**，只要目标存在就 EEXIST ⇒ 真原子。
      const tmp = lockPath + ".tmp." + process.pid;
      fs.writeFileSync(tmp, JSON.stringify({ pid: process.pid, at: new Date().toISOString() }));
      try { fs.linkSync(tmp, lockPath); } finally { try { fs.unlinkSync(tmp); } catch (e) { /* 忽略 */ } }
      // 兜底释放：任何 exit 路径（含 process.exit / 未捕获异常）都会清掉锁文件
      process.once("exit", () => { try { fs.rmSync(lockPath, { force: true }); } catch (e) { /* 忽略 */ } });
      return lockPath;
    } catch (e) {
      let stale = false;
      try {
        const info = JSON.parse(fs.readFileSync(lockPath, "utf8"));   // 锁是一个文件（v3）
        const age = Date.now() - Date.parse(info.at || 0);
        let alive = false;
        try { process.kill(Number(info.pid), 0); alive = true; } catch (e2) { alive = false; }
        stale = !alive || age > staleMs;
      } catch (e2) {
        // **锁文件不可读/无内容**：不能立刻判陈旧（那等于"删掉内容就能抢活锁"）。
        // 退回按锁文件的 **mtime** 判龄：新锁一律尊重，只有"老到超过 staleMs"才抢占。
        try { const st = fs.statSync(lockPath); stale = Date.now() - st.mtimeMs > staleMs; } catch (e3) { stale = false; }
      }
      if (stale) { try { fs.rmSync(lockPath, { recursive: true, force: true }); } catch (e2) { /* 已被清 */ } continue; }
      if (Date.now() - t0 > waitMs) throw new Error("等锁超时：" + lockPath);
      Atomics.wait(sleeper, 0, 0, 120);                        // 真睡眠（不烧 CPU）
    }
  }
}

/** 显式释放（正常路径调用；异常路径由 exit 兜底）。 */
export function releaseLockSync(lockPath) { try { fs.rmSync(lockPath, { recursive: true, force: true }); } catch (e) { /* 忽略 */ } }
