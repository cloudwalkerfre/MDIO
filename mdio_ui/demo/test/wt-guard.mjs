// test/wt-guard.mjs —— 活体用例的「目标 wt 未挂载」处理
//
// 背景：活体用例都钉在某个 wt（exp_pages=c8Page01 / exp_swap=c7Sw4p00 / e2e ⑲ 组用 exp_app=c6Oe1Gp1）。
// 装载是可收敛的（genesis 只保留 swap + pages），所以「目标 wt 没装」是**正常状态**而不是失败。
// 约定（沿仓库既有做法）：
//   · 整包依赖一个 wt 的套件（pages-live / swap-live）⇒ 打印 "⊘ 跳过：…" 且**退 0**；
//     verify-all 见到 "⊘ 跳过" + 退 0 会归入「跳过」（不计绿、不计红）；加 --strict 时退 1（防止把"没跑到"当"全绿"）。
//   · 只有一组依赖某 wt 的大套件（e2e ⑲）⇒ 组内 SKIP 并标注（与 check-admit-gate 的 SKIP 同约定），汇总行如实写明跳过了哪组。

/** 取网关认得的 wt 列表；取不到返回 null（网关不在）。 */
export async function mountedWts(gw) {
  try {
    const r = await fetch(gw + "/control/status");
    const j = await r.json();
    return Array.isArray(j.wts) ? j.wts : [];
  } catch (e) { return null; }
}

/** 目标 wt 未挂载 ⇒ 打印 ⊘ 跳过 并退出（--strict ⇒ 退 1）。返回 true 表示已挂载、可继续。 */
export async function requireMountedWt(gw, uid, what) {
  const wts = await mountedWts(gw);
  if (wts && wts.indexOf(uid) >= 0) return true;
  const why = wts ? "当前 /control/status 的 wts = [" + wts.join(",") + "]" : "网关取不到 /control/status";
  console.log("⊘ 跳过：" + (what || "本用例") + " 依赖 wt " + uid + " 已挂载；" + why);
  process.exit(process.argv.indexOf("--strict") >= 0 ? 1 : 0);
}