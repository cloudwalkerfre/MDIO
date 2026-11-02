// check-slot.mjs —— router 槽位的**解析与取值校验**判据（合成声明树，不碰真声明、不需要浏览器）。
import { armWatchdog, installGuards } from "../test/lib/watchdog.mjs";
installGuards("check-slot");
armWatchdog("check-slot", 10000)   /* = 10s */;   // 实测 max=0.1s（本机 2 跑）⇒ ×2=0.2s，取下限 10s ⇒ 10s
// 判据来源：负例清单 N1–N7。跑法：node mdio_ui/demo/scripts/check-slot.mjs
import { extractDecl, loadDecl } from "../../lib/mods/ui_core/src/services/uiMd.js";

let pass = 0, total = 0;
const chk = (n, ok, d) => { total += 1; if (ok) pass += 1; console.log((ok ? "  ok   " : "  FAIL ") + n + (d ? "  — " + d : "")); };
const b = (name, spec) => ({ nt: "b", t: name, ct: [{ nt: "p", ct: " " + spec }] });
const bl = (...items) => ({ nt: "bl", ct: items });
const sec = (name, ...items) => ({ nt: "h", lv: 2, t: name, ct: [bl(...items)] });
const tree = (...secs) => ({ nt: "root", ct: secs });
const MODS = sec("mods", b("std", "ui_std/std"), b("SMOKE", ".md::/smoke.md::..smoke"), b("CSSX", "ui_std/theme"));
const load = async (name) => ({ name, ns: name === "CSSX" ? { sheet: {} } : { default: function C() {} } });

{ // N0 正例
  const t = tree(MODS, sec("router", b("appview", "SMOKE")));
  const d = extractDecl(t);
  const bag = await loadDecl({ declTree: t, load });
  chk("N0 正例：router.appview 解析出来", d.router && d.router.appview === "SMOKE", JSON.stringify(d.router));
  chk("N0 正例：无诊断", d.errors.length === 0 && bag.errors.length === 0, JSON.stringify(d.errors.concat(bag.errors).slice(0, 2)));
}
{ // N1 节名写错 ⇒ 静默回退
  const d = extractDecl(tree(MODS, sec("routers", b("appview", "SMOKE"))));
  chk("N1 节名写错 ⇒ 静默回退（appview 空、无诊断）", d.router.appview === "" && d.errors.length === 0, JSON.stringify({ av: d.router.appview, errs: d.errors.length }));
}
{ // N2 键名写错 ⇒ 红灯
  const d = extractDecl(tree(MODS, sec("router", b("appveiw", "SMOKE"))));
  chk("N2 键名写错 ⇒ errors 非空", d.errors.length > 0 && /不认的键/.test(d.errors[0].msg), JSON.stringify(d.errors[0]));
}
{ // N3 空值 ⇒ 红灯
  const d = extractDecl(tree(MODS, sec("router", b("appview", ""))));
  chk("N3 空值 ⇒ errors 非空", d.errors.length > 0 && /值为空/.test(d.errors[0].msg), JSON.stringify(d.errors[0]));
}
{ // N4 子节 ⇒ 既有形态红灯
  const t = tree(MODS, { nt: "h", lv: 2, t: "router", ct: [{ nt: "h", lv: 3, t: "appview", ct: [] }] });
  const d = extractDecl(t);
  chk("N4 子节 ⇒ errors 非空（桶已删的既有行为）", d.errors.length > 0, JSON.stringify(d.errors[0] && d.errors[0].msg));
}
{ // N5 值不在 mods ⇒ 红灯
  const bag = await loadDecl({ declTree: tree(MODS, sec("router", b("appview", "NOPE"))), load });
  chk("N5 值不在 mods ⇒ 红灯", bag.errors.some((e) => /值不在 ## mods 里/.test(e.msg)), JSON.stringify(bag.errors.map((e) => e.msg).slice(0, 2)));
}
{ // N6 非渲染件 ⇒ 红灯
  const bag = await loadDecl({ declTree: tree(MODS, sec("router", b("appview", "CSSX"))), load });
  chk("N6 非渲染件 ⇒ 红灯", bag.errors.some((e) => /不能画/.test(e.msg)), JSON.stringify(bag.errors.map((e) => e.msg).slice(0, 2)));
}
{ // N7 无 router 节 ⇒ 缺省
  const bag = await loadDecl({ declTree: tree(MODS), load });
  chk("N7 无 router 节 ⇒ appview 空 + 无诊断", bag.decl.router.appview === "" && bag.errors.length === 0, JSON.stringify(bag.decl.router));
}
console.log((pass === total ? "✓ router 槽位解析判据全绿（" : "✗ 失败 ") + pass + "/" + total + (pass === total ? "）" : ""));
process.exit(pass === total ? 0 : 1);
