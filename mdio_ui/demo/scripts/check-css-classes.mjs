// check-css-classes —— **用到却没定义的类**判据（业主报「[清空 底部] 样式没了」换来的）。
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-css-classes");
armWatchdog("check-css-classes", 10000);   /* = 10s；实测 0.1s（扫描 1 跑）⇒ ×2 */
//
// 现场：`.dv-mini` 的定义在 ui_core/src/render/ErrorBoundary.css ✓、`ErrorBoundary.jsx` 也 import 了它 ✓、
// 构建**也产出了** ErrorBoundary.<hash>.final.css ✓ —— 但宿主注入 mod CSS 时只信 manifest 的
// `chunks[*].css`，而 ui_core 的 17 个 chunks **没有一个登记这张 CSS** ⇒ 页面里没有那条 <link>
// ⇒ 按钮掉回浏览器默认样式。教训：**「CSS 文件存在」不等于「CSS 到了页面」**，也不等于「类名有定义」。
//
// 本判据只查最基础的一层：各 mod 的 `src/**` 里用到的 `className` token，是否在任何 `.css` 里出现过。
// 它抓不到「文件存在但没被 link」（那要靠宿主侧/活体），但能立刻抓住「清死类清错了」这类回归——
// 本仓的 CSS 拆分就顺手把 `.dv-mini` 当死类删过一次。
import fs from 'node:fs';
import path from 'node:path';
const HERE = path.dirname(new URL(import.meta.url).pathname);
const MODS = path.resolve(HERE, '..', '..', 'lib', 'mods');
// 允许例外的 token：① 非 CSS 语义（模板里的变量名等）；② 已知的装饰性缺口（记在这里比默默放过强）。
const ALLOW = new Set(['uid', 'dv-count']);
const walk = (d, out = []) => { for (const e of fs.readdirSync(d, { withFileTypes: true })) { const p = path.join(d, e.name); if (e.isDirectory()) walk(p, out); else out.push(p); } return out; };
// **类定义按全仓查**（不是逐 mod 查）：本仓有跨 mod 复用（`.dv-mini` 定义在 ui_core 的 ErrorBoundary.css、
// 由 ui_devview 的 LogPanel 复用 —— 原注释就写着「低层定义、上层继承」）⇒ 逐 mod 查会误报。
const definedAll = new Set();
for (const mod of fs.readdirSync(MODS)) {
  const src0 = path.join(MODS, mod, 'src');
  if (!fs.existsSync(src0)) continue;
  for (const f of walk(src0).filter((f) => /\.css$/.test(f))) {
    const t = fs.readFileSync(f, 'utf8');
    for (const m of t.matchAll(/\.([A-Za-z][A-Za-z0-9_-]*)/g)) definedAll.add(m[1]);
  }
}
let bad = 0, checked = 0;
for (const mod of fs.readdirSync(MODS)) {
  const src = path.join(MODS, mod, 'src');
  if (!fs.existsSync(src)) continue;
  const files = walk(src);
  const used = new Set();
  for (const f of files.filter((f) => /\.(jsx?|tsx?)$/.test(f))) {
    const t = fs.readFileSync(f, 'utf8');
    for (const m of t.matchAll(/className\s*=\s*"([^"]+)"/g)) m[1].split(/\s+/).forEach((c) => c && used.add(c));
    for (const m of t.matchAll(/className\s*=\s*\{`([^`]+)`\}/g)) m[1].split(/[$\s{}]+/).forEach((c) => c && used.add(c));
    for (const m of t.matchAll(/className\s*=\s*\{["']([^"']+)["']/g)) m[1].split(/\s+/).forEach((c) => c && used.add(c));
  }
  const defined = new Set();
  for (const f of files.filter((f) => /\.css$/.test(f))) {
    const t = fs.readFileSync(f, 'utf8');
    for (const m of t.matchAll(/\.([A-Za-z][A-Za-z0-9_-]*)/g)) defined.add(m[1]);
  }
  checked += used.size;
  const miss = [...used].filter((c) => !defined.has(c) && !definedAll.has(c) && !ALLOW.has(c) && !c.startsWith('md-') && !['mdio-doc', 'mdio-std'].includes(c)).sort();
  if (miss.length) { bad += miss.length; console.log(' ✗ ' + mod + '：用到的类没有定义 → ' + miss.join(' ')); }
  else console.log(' ✓ ' + mod + '（' + used.size + ' 个类都有定义）');
}
console.log(bad ? '✗ check-css-classes 失败 ' + bad + ' 项（查了 ' + checked + ' 个类）' : '✓ check-css-classes 全绿（查了 ' + checked + ' 个类）');
process.exit(bad ? 1 : 0);