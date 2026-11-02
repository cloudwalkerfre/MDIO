// @ts-check
import { useEffect, useRef, useState } from "react";
import { observer } from "mobx-react-lite";
import "./Header.css";   // 样式随件走（与组件同目录）

/**
 * Header —— 三行布局的第一行，**横贯整页宽**（两种模式都常驻）。
 * 内容（左→右）：**wt 切换（当前 wt，点击展开树形清单）** → 文件全路径 → 错误 Tag → **一组开关 `[Monaco][Log][开发者模式]`** → 连接徽章（connected · rev）→ 主题。
 *
 * wt 菜单（参考 animate-ui / Base UI DropdownMenu 的动效语言，纯 CSS 实现，不引第三方动效库）：
 *   · 触发钮 → popup：从触发器顶部 origin 淡入 + 0.9→1 缩放（220ms，soft ease-out）；关闭先播 160ms 退出再卸载；
 *   · 顶部筛选框（名称 / uid / tag / repo）→ 树形行（可折叠，默认全展开，筛选时自动展开命中路径）；
 *   · 行进入 stagger（33ms/行，>10 行按 √ 收窄，8px 右侧滑入 + 4px 模糊淡入，300ms）；
 *   · 底部「从 GitHub 加载」输入框（研究项：见 onLoadSource；网关侧接口未接前只给研究结论）。
 * 读法：只收 `store`（DevStore）与可选 `onLoadSource`；点选直接调 store.switchWT。
 */
const GH_RE = /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/?$/i;

/** 树形行：默认过滤（tag app + 根树 + 当前 wt）→ 按 parent 建树 → 筛选时保留命中+祖先。 */
function treeRows(list, filter, collapsed, wtAll, currentWT) {
  const all = Array.isArray(list) ? list : [];
  const rows = wtAll ? all : all.filter((w) => w.tagApp || w.uid === currentWT || !w.parent);
  const uidRows = new Map(rows.map((r) => [r.uid, r]));
  const byParent = new Map();
  for (const r of rows) {
    const p = r.parent && uidRows.has(r.parent) ? r.parent : "";
    if (!byParent.has(p)) byParent.set(p, []);
    byParent.get(p).push(r);
  }
  const q = String(filter || "").trim().toLowerCase();
  const hitOf = (r) => !q || [r.title, r.uid, r.raw, r.appName, r.repo, (r.tags || []).join(" ")]
    .join(" ").toLowerCase().includes(q);
  const visible = new Set();
  const visit = (r) => {
    let hit = hitOf(r);
    for (const k of byParent.get(r.uid) || []) if (visit(k)) hit = true;
    if (hit) visible.add(r.uid);
    return hit;
  };
  for (const r of byParent.get("") || []) visit(r);
  const out = [];
  const walk = (r, lv) => {
    if (!visible.has(r.uid)) return;
    const kids = (byParent.get(r.uid) || []).filter((k) => visible.has(k.uid));
    out.push({ row: r, lv, hasKids: kids.length > 0 });
    const open = q ? true : !collapsed.has(r.uid);
    if (open) for (const k of kids) walk(k, lv + 1);
  };
  for (const r of byParent.get("") || []) walk(r, 0);
  return out;
}

export default observer(function Header({ store, rev, conn, bootError, onToggleDev = null, onLoadSource = null }) {
  // 菜单三态：closed / open / closing（closing 期间保留 DOM 播退出动画）
  const [menu, setMenu] = useState("closed");
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState(() => new Set());
  const [ghUrl, setGhUrl] = useState("");
  const [ghMsg, setGhMsg] = useState("");
  const [ghErr, setGhErr] = useState(false);
  const [ghBusy, setGhBusy] = useState(false);
  const box = useRef(null);
  const closeTimer = useRef(0);

  const closeMenu = () => {
    if (menu !== "open") return;
    setMenu("closing");
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setMenu("closed"), 160);
  };
  const openMenu = () => {
    window.clearTimeout(closeTimer.current);
    setFilter("");
    setCollapsed(new Set());
    setGhMsg("");
    setGhErr(false);
    setMenu("open");
  };
  const toggleMenu = () => (menu === "open" ? closeMenu() : openMenu());

  useEffect(() => {
    if (menu === "closed") return undefined;
    const onDoc = (e) => { if (box.current && !box.current.contains(e.target)) closeMenu(); };
    const onKey = (e) => { if (e.key === "Escape") closeMenu(); };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [menu]);
  useEffect(() => () => window.clearTimeout(closeTimer.current), []);

  if (!store) return <div className="dv-header" />;
  const wt = store.currentWT;
  const entryPath = store.entry ? store.entry.path : "";
  const model = store.model;
  const list = Array.isArray(store.wtList) ? store.wtList : [];
  const cur = list.find((w) => w.uid === wt);
  const wtAll = !!store.wtAll;
  const rows = treeRows(list, filter, collapsed, wtAll, wt);
  // 行 stagger：>10 行时按 √(10/rows) 收窄步距（来自 animate-ui 的动效规格）
  const step = rows.length > 10 ? 33 * Math.sqrt(10 / rows.length) : 33;
  const toggleCollapse = (uid) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(uid)) next.delete(uid); else next.add(uid);
      return next;
    });
  };
  const submitLoad = async (e) => {
    e.preventDefault();
    const v = ghUrl.trim();
    if (!GH_RE.test(v)) { setGhErr(true); setGhMsg("请输入 https://github.com/owner/repo"); return; }
    setGhErr(false);
    if (!onLoadSource) {
      setGhMsg("研究结论：可行（复用 CLI 的 archive 下载 + install.md 解析），但网关目前只提供 worktree.load(path)；需新增 load-source 接口（方案见本轮答复）。");
      return;
    }
    try {
      setGhBusy(true);
      setGhMsg("正在加载…");
      const r = await onLoadSource(v);
      setGhMsg((r && (r.message || r.error)) || "已提交加载");
    } catch (err) {
      setGhErr(true);
      setGhMsg(String((err && err.message) || err));
    } finally {
      setGhBusy(false);
    }
  };
  return (
    <div className="dv-header">
      <span className="dv-wtswitch" ref={box}>
        <button className={"dv-wtbtn" + (menu === "open" ? " open" : "")} title="切换 wt（工作树）" aria-haspopup="menu" aria-expanded={menu === "open"} onClick={toggleMenu}>
          <span className="ico" aria-hidden="true">▾</span>
          <span className="nm">{(cur && cur.title) || wt}</span>
        </button>
        {menu !== "closed" && (
          <div className={"dv-wtmenu " + menu} role="menu">
            {/* 临时隐藏 search bar（保留代码，恢复时取消注释）
            <div className="dv-wtfilter">
              <span className="dv-wtfilter-ico" aria-hidden="true">⌕</span>
              <input
                autoFocus
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                placeholder="筛选 wt（名称 / uid / tag / repo）"
                aria-label="筛选工作树"
              />
              {filter ? <button type="button" className="dv-wtfilter-clear" onClick={() => setFilter("")} aria-label="清空筛选">×</button> : null}
            </div>
            */}
            <div className="dv-wtseg" role="group" aria-label="wt 过滤">
              <button className={"dv-wtsegbtn" + (wtAll ? "" : " on")} onClick={() => store.setWtAll(false)}>默认</button>
              <button className={"dv-wtsegbtn" + (wtAll ? " on" : "")} onClick={() => store.setWtAll(true)}>全部</button>
            </div>
            <div className="dv-wttree" role="group">
              {rows.map(({ row, lv, hasKids }, i) => {
                const open = !!filter.trim() || !collapsed.has(row.uid);
                return (
                  <div
                    key={row.uid}
                    className={"dv-wtrow" + (row.uid === wt ? " on" : "")}
                    style={{ marginLeft: lv * 14, animationDelay: i * step + "ms" }}
                    role="menuitem"
                    tabIndex={0}
                    title={row.repo || row.uid}
                    onClick={() => { if (row.uid !== wt) store.switchWT(row.uid); closeMenu(); }}
                    onKeyDown={(e) => { if (e.key === "Enter") { if (row.uid !== wt) store.switchWT(row.uid); closeMenu(); } }}
                  >
                    {hasKids ? (
                      <button type="button" className={"dv-wtcaret" + (open ? " open" : "")} aria-label={open ? "收起" : "展开"}
                        onClick={(e) => { e.stopPropagation(); toggleCollapse(row.uid); }}>▸</button>
                    ) : <span className="dv-wtcaret ghost" aria-hidden="true" />}
                    <span className="dv-wtrow-name">{row.title || row.uid}</span>
                    {row.tagApp ? <span className="dv-wttag">app</span> : null}
                    {row.version ? <span className="dv-wtver">v{row.version}</span> : null}
                    <span className="dv-wtuid">{row.uid}</span>
                  </div>
                );
              })}
              {rows.length === 0 && <div className="dv-wtempty">（没有可显示的 wt）</div>}
            </div>
            {/* 临时隐藏 GitHub load bar（保留代码，恢复时取消注释）
            <div className="dv-wtload">
              <div className="dv-wtload-label">从 GitHub 加载</div>
              <form className="dv-wtload-form" onSubmit={submitLoad}>
                <input
                  value={ghUrl}
                  onChange={(e) => setGhUrl(e.target.value)}
                  placeholder="https://github.com/owner/repo"
                  aria-label="GitHub 仓库地址"
                  spellCheck={false}
                  disabled={ghBusy}
                />
                <button type="submit" disabled={ghBusy} title={onLoadSource ? "加载这个 WTS" : "研究项：远程 load 接口待接入"}>{ghBusy ? "…" : "Load"}</button>
              </form>
              {ghMsg ? <div className={"dv-wtload-msg" + (ghErr ? " err" : "")}>{ghMsg}</div> : null}
            </div>
            */}
          </div>
        )}
      </span>
      {entryPath && <span className="dv-filetag"><span className="p">{entryPath}</span></span>}
      <span className="dv-spacer" />
      <span className="dv-seg">
        {store.devMode && <button className={"dv-toggle" + (store.mona ? " on" : "")} onClick={store.toggleMona}>Monaco</button>}
        {store.devMode && <button className={"dv-toggle" + (store.log ? " on" : "")} onClick={store.toggleLog}>Log</button>}
        <button className={"dv-toggle wide" + (store.devMode ? " on" : "")} title="开发者模式（关掉 ⇒ 只留 header + app）" onClick={onToggleDev || store.toggleDev}>开发者模式</button>
      </span>
      {bootError && <span className="dv-errtag" title={String(bootError)}>✗ {String(bootError).slice(0, 40)}</span>}
      <span className={"dv-conn" + (model ? " ok" : "")} title={conn ? "连接状态: " + conn : undefined}>{model ? "connected · rev " + rev : "等待首帧"}</span>
      <button className="dv-switch" title="主题切换" onClick={store.toggleDark}>{store.dark ? "☾" : "☀"}</button>
    </div>
  );
});
