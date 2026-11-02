// check-app-router-roundtrip —— app 路由往返判据（真鼠标 · 自建无头 9223）
// 断言：① 读→返回→刷新 停在画廊（P1 反向清指针）；② 三态一致（hash / ?app= / 显示）；③ 后退/前进 与 URL 一致。
const CDP = process.env.CDP || 'http://127.0.0.1:9223';
const BASE = process.env.BASE || 'http://127.0.0.1:5178/?wt=czUR8zxu&app=%2FREADME.md';
const t = await (await fetch(CDP + '/json/new?' + encodeURIComponent(BASE), { method: 'PUT' })).json();
const ws = new WebSocket(t.webSocketDebuggerUrl); let id = 0; const pend = new Map();
ws.addEventListener('message', (e) => { const m = JSON.parse(e.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } });
await new Promise((r) => ws.addEventListener('open', r));
const send = (method, params) => new Promise((r) => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params: params || {} })); });
const ev = async (x) => { const r = await send('Runtime.evaluate', { expression: x, returnByValue: true }); return r.result ? r.result.value : null; };
const snap = "(()=>{const a=document.getElementById('app');const R=a&&a.shadowRoot;return JSON.stringify({hash:location.hash,app:new URLSearchParams(location.search).get('app'),shell:(R?(R.querySelector('[data-shell=view]')||{getAttribute:()=>null}):{getAttribute:()=>null}).getAttribute('data-shell-path'),reader:!!(R&&R.querySelector('.app-scripts-reader')),cards:R?R.querySelectorAll('[data-cover]').length:-1,err:R?R.querySelectorAll('.dv-eb').length:-1})})()";
const click = async (sel, dx, dy) => { const pt = await ev("(()=>{const R=document.getElementById('app').shadowRoot;const e=R.querySelector('" + sel + "');if(!e)return null;const r=e.getBoundingClientRect();return JSON.stringify([Math.round(r.left+" + dx + "),Math.round(r.top+" + dy + ")])})()"); if (!pt) return false; const p = JSON.parse(pt); await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: p[0], y: p[1], button: 'left', clickCount: 1 }); await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: p[0], y: p[1], button: 'left', clickCount: 1 }); return true; };
const results = []; const ok = (n, cond, detail) => results.push({ n, pass: !!cond, detail });
await send('Runtime.enable'); await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
await send('Page.navigate', { url: BASE }); await new Promise((r) => setTimeout(r, 10000));
const A = JSON.parse(await ev(snap)); ok('① 画廊：cards=2 且 reader=false', A.cards === 2 && A.reader === false, JSON.stringify(A));
await click('[data-cover]', 30, 30); await new Promise((r) => setTimeout(r, 2500));
const B = JSON.parse(await ev(snap)); ok('② 点卡进阅读页且 hash 与 shell 一致', B.reader === true && B.hash.replace(/^#/, '') === B.shell && String(B.app || '').indexOf('/scripts/') === 0, JSON.stringify(B));
await ev('history.back()'); await new Promise((r) => setTimeout(r, 2000));
const C = JSON.parse(await ev(snap)); ok('③ 后退回画廊（hash 空、reader=false）', C.reader === false && (C.hash === '' || C.hash === '#/'), JSON.stringify(C));
await ev('history.forward()'); await new Promise((r) => setTimeout(r, 2000));
const D = JSON.parse(await ev(snap)); ok('④ 前进回阅读页（hash/shell 一致）', D.reader === true && D.hash.replace(/^#/, '') === D.shell, JSON.stringify(D));
await click('.app-scripts-back', 10, 10); await new Promise((r) => setTimeout(r, 2000));
const E = JSON.parse(await ev(snap)); ok('⑤ 点返回 ⇒ 壳指针清空但字段保留（产品面不丢）', E.reader === false && E.app === '' , JSON.stringify(E));
await send('Page.reload', {}); await new Promise((r) => setTimeout(r, 10000));
const F = JSON.parse(await ev(snap)); ok('⑥ 刷新仍停在画廊（缺陷闭环）', F.reader === false && F.cards === 2 && F.err === 0, JSON.stringify(F));
// ⑦ `?app=` **空值仍是产品面**（字段可空、不可无）：空值 ⇒ 无入口文档 ⇒ app 走自己的首页
await send('Page.navigate', { url: 'http://127.0.0.1:5178/?wt=czUR8zxu&app=' }); await new Promise((r) => setTimeout(r, 9000));
const G = JSON.parse(await ev(snap)); ok('⑦ ?app= 空值仍是产品面且落首页', G.cards === 2 && G.reader === false && G.err === 0, JSON.stringify(G));
// ⑧ **hash 是 app 的权威**：hash 指名某篇时，即便 `?app=` 指向别处，也应由 hash 决定落哪页
await send('Page.navigate', { url: 'http://127.0.0.1:5178/?wt=czUR8zxu&app=%2FREADME.md#/doc/%2Fscripts%2F%E5%89%A7%E6%9C%AC.md' }); await new Promise((r) => setTimeout(r, 9000));
const H = JSON.parse(await ev(snap)); ok('⑧ hash 优先（app 路由权威）', H.reader === true && H.hash.replace(/^#/, '') === '/doc/%2Fscripts%2F%E5%89%A7%E6%9C%AC.md', JSON.stringify(H));
const bad = results.filter((r) => !r.pass);
for (const r of results) console.log((r.pass ? 'PASS ' : 'FAIL ') + r.n + (r.pass ? '' : '  → ' + r.detail));
console.log('check-app-router-roundtrip: ' + (results.length - bad.length) + '/' + results.length);
process.exit(bad.length ? 1 : 0);