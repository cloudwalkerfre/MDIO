// @ts-check
/**
 * vp-build —— `ui:build` 的包装：跑 Vite+ 任务，且**拒绝静默成功**、**拒绝 root 之外的目标**。
 *
 * 两个实测事实:
 *   1. `vpr <错包名>` 与「包未列进 workspace」都是 **exit 0、零输出** ⇒ 裸跑会假绿；
 *   2. vp 的缓存只跟踪 **workspace root 之内**的文件，root 之外**静默忽略** ⇒ 会假命中。
 * 所以：① 断言有输出且含预期标记；② 断言目标路径落在 workspace root 内。
 *
 * workspace 边界在 `mdio_ui/pnpm-workspace.yaml`（**不在仓库根**）⇒ 本脚本一律以 `mdio_ui/` 为 cwd 调用 vpr。
 * 树内 mod（worktrees/**）**不走这里**：用 `node scripts/build-wt-ui.mjs <树内包根>`。
 */
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const WS = path.resolve(HERE, "../..");                 // mdio_ui/demo/scripts → mdio_ui（workspace root）
const VPR = path.join(WS, "demo/node_modules/.bin/vpr");
const TASK = process.env.VP_TASK || "demo#build:ui";
const args = process.argv.slice(2);

// 目标参数（可选）按**任务 cwd = 包根**解析，然后断言它没越出 workspace root
if (args.length) {
  const target = path.resolve(WS, "demo", args[0]);
  const rel = path.relative(WS, target);
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    console.error("[vp-build] ✗ 目标在 workspace root（" + WS + "）之外：" + target);
    console.error("           vp 的缓存**不跟踪 root 之外**的文件 ⇒ 会静默假命中。树内 mod 请用：node scripts/build-wt-ui.mjs <包根>");
    process.exit(2);
  }
}

const r = spawnSync(VPR, [TASK, ...args], { cwd: WS, encoding: "utf8" });
const out = (r.stdout || "") + (r.stderr || "");
if (r.stdout) process.stdout.write(r.stdout);
if (r.stderr) process.stderr.write(r.stderr);

const code = r.status == null ? 1 : r.status;
if (code !== 0) { console.error("[vp-build] ✗ vpr 退出码 " + code + "（任务 " + TASK + "）"); process.exit(code); }
if (!out.trim().length) {
  console.error("[vp-build] ✗ vpr 退出 0 但**零输出** ⇒ 任务没真跑（包名写错 / demo 未列进 workspace 都是这种表现）。");
  console.error("           排查：mdio_ui/pnpm-workspace.yaml 的 packages 是否含 demo；包名/任务名是否写对。");
  process.exit(1);
}
if (!/build-wt-ui|cache hit|Build completed|vp run/.test(out)) {
  console.error("[vp-build] ✗ 输出里没有预期标记 ⇒ 拒绝当成功。");
  process.exit(1);
}
console.log("[vp-build] ✓ " + TASK + " " + (args.join(" ") || "（无参数）"));