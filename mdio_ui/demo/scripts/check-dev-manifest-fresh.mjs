#!/usr/bin/env node
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-dev-manifest-fresh");
armWatchdog("check-dev-manifest-fresh", 10000);   /* = 10s；实测 0.2s（扫描 1 跑）⇒ ×2 */
// @ts-check
/**
 * check-dev-manifest-fresh —— **dev 页引用的产物必须都在、且按 JS 发**（清单陈旧 = 页面卡死的真凶）。
 *
 * 为什么要有这条（实测两次踩到）：构建产物（mod 的 `.final.mjs` chunk）重建后，**正在跑的宿主**
 * 可能还在发**旧清单** ⇒ 页面去取一个已不存在的 chunk ⇒ SPA 回退把 HTML 当 JS 发 ⇒
 * `Failed to load module script: … MIME type of "text/html"` ⇒ 页面卡在「正在装载…」，
 * 症状是「套件内全红、套件外手动全绿」。
 *
 * 断言：
 *   ① 页面是真页面（有 importmap）；
 *   ② 页面里每个 `.final.mjs` URL 都能取到，**且 content-type 不是 text/html**（即真产物、非回退页）；
 *   ③ chunk 标签**带** integrity（去掉它 modulepreload 会失配 ⇒ 同一模块加载两份，见 check-module-graph）。
 *
 * 用法：node scripts/check-dev-manifest-fresh.mjs [--url http://127.0.0.1:5178/?wt=genesis]
 */
const idx = process.argv.indexOf("--url");
const URL = idx >= 0 ? String(process.argv[idx + 1]) : "http://127.0.0.1:5178/?wt=genesis";
let pass = 0, fail = 0;
const chk = (n, ok, d) => { ok ? pass++ : fail++; console.log((ok ? "OK   " : "FAIL ") + n + (d ? "  " + d : "")); };
try {
  const r = await fetch(URL, { signal: AbortSignal.timeout(15000) });
  const html = await r.text();
  chk("dev 页 HTTP 200", r.status === 200, "status=" + r.status);
  chk("页面是真页面（有 importmap）", /<script type="importmap">/.test(html), html.length + " 字符");
  const urls = [...new Set((html.match(/["\'(](\/[A-Za-z0-9_./-]+\.final\.mjs)["\')]/g) || []).map((s) => s.slice(1, -1)))];
  chk("页面引用了产物 chunk", urls.length > 0, urls.length + " 个唯一 chunk");
  const bad = [];
  for (const u of urls) {
    try {
      const res = await fetch("http://127.0.0.1:5178" + u, { signal: AbortSignal.timeout(8000) });
      const ct = String(res.headers.get("content-type") || "");
      if (!res.ok || ct.indexOf("text/html") >= 0) bad.push(u.slice(-42) + " → " + res.status + " " + ct.split(";")[0]);
    } catch (e) { bad.push(u.slice(-42) + " → " + String((e && e.message) || e).slice(0, 40)); }
  }
  chk("② 每个 chunk 都取得到且按 JS 发（清单不陈旧）", bad.length === 0, bad.length ? bad.slice(0, 3).join(" · ") : urls.length + "/" + urls.length + " 全 OK");
  const tags = (html.match(/<(?:script|link)\b[^>]*>/g) || []).filter((t) => /\.final\.mjs/.test(t));
  const sri = tags.filter((t) => /integrity="/.test(t));
  chk("③ chunk 标签带 integrity（preload 才不失配）", tags.length > 0 && sri.length === tags.length, sri.length + "/" + tags.length + " 个带");
} catch (e) { chk("取 dev 页", false, String((e && e.message) || e).slice(0, 100)); }
console.log("---- dev 清单新鲜度判据 " + pass + " passed / " + fail + " failed ----");
process.exit(fail ? 1 : 0);
