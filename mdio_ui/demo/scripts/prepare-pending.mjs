// @ts-check
/**
 * prepare-pending —— 把两项"待批"变成可执行动作（默认 dry-run：不写盘、不安装）。
 *
 *   ① `--item 1`：`mdio_ui/lib/mods/ui_std` 的局部 `pnpm install`（同时补"新树接入清单第 2 步"——唯一未被演练验证的一步）。
 *   ② `--item 2`：把 `scripts/templates/root-vite.config.ts` 拷到仓库根（Vite+ 编排；形态已实测）。
 * `--verify 1|2`：落地后立即跑各自的验收判据。
 *
 * 配置文本**存在模板文件里**，本脚本只读/打印/拷贝 —— 不在代码里内嵌文本（前一版因此连踩三次转义坑）。
 * 用法：node scripts/prepare-pending.mjs --item 1 [--apply] | --item 2 [--apply] | --verify 1|2
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const args = process.argv.slice(2);
const item = (() => { const i = args.indexOf("--item"); if (i >= 0) return String(args[i + 1]); const v = args.indexOf("--verify"); return v >= 0 ? String(args[v + 1]) : null; })();
const apply = args.includes("--apply");
const verifyOnly = args.includes("--verify");
const TEMPLATE = path.join(DEMO, "scripts/templates/root-vite.config.ts");

if (verifyOnly) {
  const cmds = item === "1"
    ? [["check-protocol", "scripts/check-protocol.mjs"], ["ensure-built", "scripts/ensure-built.mjs"], ["externals", "scripts/check-externals.mjs"]]
    : [["check-vp-cache", "scripts/check-vp-cache.mjs"]];
  let bad = 0;
  for (const [name, file] of cmds) { const r = spawnSync(process.execPath, [file], { cwd: DEMO, encoding: "utf8" }); const ok = r.status === 0; if (!ok) bad += 1; console.log("  " + (ok ? "✓" : "✗") + " " + name); }
  if (item === "2") console.log("  提示：② 还需人工确认「连跑两次第二次命中」（§31.3 的验收点之一）。");
  console.log(bad ? "✗ 验收有 " + bad + " 项未过" : "✓ 验收全绿");
  process.exit(bad ? 1 : 0);
}

if (item === "1") {
  console.log("== 待批 ①：mdio_ui/lib/mods/ui_std 的局部安装 ==");
  console.log("  命令：cd mdio_ui/lib/mods/ui_std && pnpm install");
  console.log("  作用：替换该包的临时依赖桥；同时补上「新树接入清单第 2 步」（唯一从未被演练验证的一步）。");
  if (!apply) { console.log("  （dry-run：未执行。加 --apply 才真跑。）"); process.exit(0); }
  const r = spawnSync("pnpm", ["install"], { cwd: path.join(REPO, "mdio_ui/lib/mods/ui_std"), stdio: "inherit" });
  console.log(r.status === 0 ? "✓ 安装完成；接着跑 --verify 1" : "✗ 安装失败（exit=" + r.status + "）");
  process.exit(r.status === 0 ? 0 : 1);
}

if (item === "2") {
  const target = path.join(REPO, "vite.config.ts");
  if (!fs.existsSync(TEMPLATE)) { console.error("✗ 模板不存在：" + TEMPLATE); process.exit(1); }
  const text = fs.readFileSync(TEMPLATE, "utf8");
  console.log("== 待批 ②：仓库根 vite.config.ts（Vite+ 编排）==");
  console.log("  模板：" + path.relative(DEMO, TEMPLATE));
  console.log("  目标：" + target + (fs.existsSync(target) ? "（**已存在**，--apply 会覆盖）" : "（尚不存在）"));
  console.log("  ---- 文本 ----");
  console.log(text);
  console.log("  ---- 结束 ----");
  if (!apply) { console.log("  （dry-run：未写盘。加 --apply 才写。）"); process.exit(0); }
  fs.writeFileSync(target, text);
  console.log("✓ 已写入；下一步：node scripts/prepare-pending.mjs --verify 2");
  process.exit(0);
}

console.error("用法：--item 1|2 [--apply] 或 --verify 1|2");
process.exit(2);
