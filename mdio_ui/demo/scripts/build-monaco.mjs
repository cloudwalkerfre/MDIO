// build-monaco.mjs —— 把 monaco 打成一个自包含 ESM 文件进 public/（vendor 大件不进 vite 的转换/预打包链路）。
// 用法：node scripts/build-monaco.mjs   （产物：public/monaco.api.mjs）
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const require = createRequire(import.meta.url);
const entry = join(root, "node_modules/monaco-editor/esm/vs/editor/editor.api.js");

await build({
  entryPoints: [entry],
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2022",
  outfile: join(root, "vendor/monaco.api.mjs"),
  minify: true,          // 4.5MB 未压缩 → 约 1.8MB；无头环境里解析成本直接决定挂载成败
  legalComments: "none",
  logLevel: "info",
});
console.log("monaco -> vendor/monaco.api.mjs（由壳宿主 /vendor/ 路由直出；public 不能从源码 import）");
