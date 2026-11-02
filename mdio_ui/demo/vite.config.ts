// Vite+ 任务定义（**放在包内**，所以仓库根不需要 vite.config.ts）。以下每条都是实测，不是推测：
//   ① 顶层 `run.cache` / `run.enablePrePostScripts` **只能**写在 workspace 根配置 ⇒ 这里不写；任务级 cache 合法。
//   ② 调用必须在**仓库根**跑：vp 从 cwd 向上取**最近**的 pnpm-workspace.yaml 当 workspace root。
//      站在 demo/ 里跑（demo 自带这份文件）⇒
//      workspace 变成 demo，目标包的源码**不进指纹**：同一处改动在仓库根跑报 `cache miss: '<path>/src/...'
//      modified`，在 demo/ 里跑却报 `cache hit` 且重放的戳里 sourceHash 还是旧的（实测）。
//   ③ 参数**不要**用 `--` 分隔：`vpr 'demo#build:ui' -- x` 会把 `--` 原样喂给脚本（argv[2]='--' ⇒ 退 3，
//      并在 demo/ 留下一个名为 `--` 的空目录）。
//   ④ `cache.input` 里每一条都承重，去掉任一条都会坏（用一个仓库内探针包连跑两次实测）：
//      · `{ auto: true }`  —— 它扛着**全部正项**。去掉后输入集为空 ⇒ 改源也 `cache hit`（假命中）。
//      · 两个负项都要 **对象形 `{ pattern, base: 'workspace' }`** —— 目标包在 demo **之外**，包相对的裸串
//        `'!**/.staging/**'` 打不到它（实测：用裸串连跑两次都 `cache miss: input configuration changed`）。
//      · `.esmx-build.json` —— 脚本先读旧戳再写新戳 ⇒ 不排就是"改了自己的输入"，缓存永不成立。
//      · `.staging/**` —— 构建源从它读、构建完又被删/改名 ⇒ 不排则输入集每跑都变，同样永不成立。
//      `!**/dist/**` **不**承重，故不写：产物只被写不被读，且 dist 是符号链接；实测原子/非原子两模式 ×
//      三个目标（含改 chunk 名的语义改动）全部照常命中。
// 日常入口（仓库根）：`pnpm --dir mdio_ui/demo ui:build <目标，相对 mdio_ui/demo>`
import { defineConfig } from 'vite-plus';

export default defineConfig({
  run: {
    tasks: {
      'build:ui': {
        command: 'node scripts/build-wt-ui.mjs',
        // 输入 = 自动跟踪（脚本/包的读取，含 cp 源文件的读）减去**自己写的**旁路件与戳。
        cache: {
          input: [
            { auto: true },
            { pattern: '!**/.staging/**', base: 'workspace' },
            { pattern: '!**/.esmx-build.json', base: 'workspace' },
          ],
        },
      },
    },
  },
});
