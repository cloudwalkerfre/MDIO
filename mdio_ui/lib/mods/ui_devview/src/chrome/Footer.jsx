// @ts-check
import { createPortal } from "react-dom";
import { useEffect, useRef, useState } from "react";
import "./Footer.css";   // 样式随件走

/**
 * Footer —— 视图层 **chrome**（平齐第三行：header / 工作区 / footer）。
 * footer 槽只负责高度/拖拽/挂载点/占位；console 业务在外部 wt app exp_console 内。
 * 本件只负责：
 *   · 高度（默认 30 = 只露一行；顶边可拖，上限 40% 视口）；
 *   · 容器：mdio-appview#app-foot + 把宿主注入的 footer app 投进它的挂载点；
 *   · 未装载时占位；
 *   · 把 app 的“请求高度”（onRequestHeight）夹到合法范围后交 onHeight。
 */
const BAR_H = 30;
const capH = () => Math.round((typeof window !== "undefined" ? window.innerHeight : 800) * 0.4);
const clampH = (v) => Math.max(BAR_H, Math.min(capH(), Number(v) || BAR_H));

export default function Footer({
  height, onHeight,
  appTag: AppTag = null,
  app: App = null,      // footer app 组件（大写变量）
  store = null,         // footer app 店（宿主 create，env=tools）
  placeholder = "",
}) {
  const hostRef = useRef(null);
  const [mount, setMount] = useState(null);
  const drag = useRef(null);
  const h = clampH(height);

  // 挂载点：appview 的 mountPoint（影子根里的 #mount）；重建/重挂时靠 app-ready 事件补齐。
  useEffect(() => {
    const el = hostRef.current;
    if (!el) return undefined;
    const pick = () => setMount(el.mountPoint || el);
    pick();
    el.addEventListener("mdio:app-ready", pick);
    return () => el.removeEventListener("mdio:app-ready", pick);
  }, [AppTag]);

  const onDown = (e) => { e.preventDefault(); drag.current = { y: e.clientY, h }; try { e.currentTarget.setPointerCapture(e.pointerId); } catch (err) { /* ignore */ } };
  const onMove = (e) => {
    const d = drag.current;
    if (!d) return;
    const next = clampH(d.h + (d.y - e.clientY));   // 往上拖 = 变高
    if (typeof onHeight === "function") onHeight(next);
  };
  const onUp = (e) => { drag.current = null; try { e.currentTarget.releasePointerCapture(e.pointerId); } catch (err) { /* ignore */ } };
  const requestHeight = (v) => { if (typeof onHeight === "function") onHeight(clampH(v)); };

  return (
    <div className="dv-footer" style={{ height: h }}>
      <div className="dv-grip" title="拖动调整 footer 高度（上限 40% 窗口高）"
        onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerCancel={onUp} />
      {AppTag ? <AppTag className="dv-foot-app" id="app-foot" ref={hostRef} /> : null}
      {!App && placeholder ? <div className="dv-foot-empty">{placeholder}</div> : null}
      {App && mount ? createPortal(
        <App app={store} footer onRequestHeight={requestHeight} />, mount) : null}
    </div>
  );
}
