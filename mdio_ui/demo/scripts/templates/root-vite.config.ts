// 由 `node mdio_ui/demo/scripts/prepare-pending.mjs --item 2 --apply` 拷到仓库根。
// 形态：**正项与负项一律用裸 glob**（对象形式 {pattern, base} 作正项已实测无效），
// 且负项必须覆盖**所有任务自己的产物** —— 漏一个就会出现"无改动也永不命中"。
import { defineConfig } from "vite-plus";

const PKGS = {
  mdio_mods: "mdio_ui/lib/mods",
  ui_std: "mdio_ui/lib/mods/ui_std",
  genesis_region: "worktrees/genesis/ui/region",
  exp_swap_swap: "exp_swap/ui/swap",
};

const excl = (d) => [
  "!" + d + "/dist*/**",
  "!" + d + "/.staging/**",
  "!" + d + "/.staging.lock/**",
  "!" + d + "/node_modules/**",
  "!" + d + "/.vite-cache/**",
  "!" + d + "/.esmx-build.json",
];

const tasks = {};
for (const [uid, dir] of Object.entries(PKGS)) {
  tasks["build:" + uid] = {
    command: "node mdio_ui/demo/scripts/build-wt-ui.mjs " + dir + " --stage-only",
    cache: { input: [dir + "/**", ...excl(dir)], output: [dir + "/.staging/**"] },
  };
  tasks["publish:" + uid] = {
    command: "node mdio_ui/demo/scripts/publish-wt-ui.mjs " + dir,
    cache: false,
    dependsOn: ["build:" + uid],
  };
}

export default defineConfig({ run: { tasks } });
