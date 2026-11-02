// @ts-check
import { useEffect, useState, useSyncExternalStore } from "react";
import { perfSubscribe, perfSnapshot, perfSetFps, perfLongSnapshot } from "./perfBus.js";
import "./PerfBox.css";   // 样式随件走

/**
 * PerfBox —— 管线性能小卡片（**放在右列顶部，不悬浮**）：[Perf] 在上、[log] 在下。
 *
 *   120 fps   19.39ms        ← fps（颜色分档） + 总时长（写发出 → 渲染完成）
 *   incr 5.2 › render 14.2   ← 两段：incr(写→补丁) + render(补丁→渲染)
 *   F 12 / P 40              ← 帧计数（全量 / 补丁）
 *
 * fps 的唯一采样点就在这里（rAF 计数/秒 → perfBus）；5s 新鲜窗口外不显示陈旧分段；点击折叠。
 */
function fpsColor(fps) {
  if (fps >= 55) return "#4ade80";
  if (fps >= 30) return "#facc15";
  return "#f87171";
}
const r2 = (n) => Math.round(n * 100) / 100;

export default function PerfBox({ visible = true }) {
  // 开发者模式关闭（或面板被关）⇒ **不渲染**：这里省的是它自己的 rAF 采样循环与每次 perfBus 变更的重渲染。
  // 数据源（perfBus）不受影响 ⇒ 再打开是瞬时重建，不重载、不丢会话。
  const s = useSyncExternalStore(perfSubscribe, perfSnapshot);
  const [collapsed, setCollapsed] = useState(() => { try { return sessionStorage.getItem("dv.perf.collapsed") === "1"; } catch (e) { return false; } });
  useEffect(() => { try { sessionStorage.setItem("dv.perf.collapsed", collapsed ? "1" : "0"); } catch (e) { /* ignore */ } }, [collapsed]);

  // fps：单一采样点（rAF 计数 / 秒）→ perfBus，LogPanel 的徽章读同一份
  useEffect(() => {
    if (!visible) return undefined;   // 不可见 ⇒ 连 fps 采样循环也不跑（切回来瞬时重启）
    let frames = 0;
    let last = performance.now();
    let live = true;
    const tick = () => {
      if (!live) return;
      frames += 1;
      const now = performance.now();
      if (now - last >= 1000) { perfSetFps(Math.round((frames * 1000) / (now - last))); frames = 0; last = now; }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return () => { live = false; };
  }, [visible]);

  // 展示会话：优先**最近一次写触发的会话**（一次写会有多个补丁帧，纯 last 会把 incr 覆盖掉）；
  // 没有写会话时退回最近一次补丁帧（此时 incr 自然为 —）。
  const pickFresh = (x) => (x && x.patchAt != null && performance.now() - x.patchAt < 5000 ? x : null);
  const last = pickFresh(s.lastWrite) || pickFresh(s.last);
  const fresh = !!last;
  // incr：进 wasm → PJ 出 wasm（client 带出的 ev.wasmMs；老会话退化为写锚）
  const incrMs = fresh && last.incrMs != null ? r2(last.incrMs) : null;
  const renderMs = fresh && last.renderAt != null ? r2(last.renderAt - last.patchAt) : null;
  const totalMs = incrMs != null && renderMs != null ? r2(incrMs + renderMs) : null;
  // 所有 hook 已调用完 ⇒ 这里早退是合法的；数据源(perfBus)照常，重开即瞬时重建。
  if (!visible) return null;

  return (
    <div className={"dv-perf" + (collapsed ? " is-collapsed" : "")} title="管线性能（点击折叠）" onClick={() => setCollapsed((c) => !c)}>
      <div className="dv-perf-row">
        <span className="ttl">Perf</span>
        <span className="fps" style={{ color: fpsColor(s.fps) }}>{s.fps} fps</span>
        <span className="total">{totalMs != null ? totalMs + "ms" : "—"}</span>
      </div>
      {!collapsed && (
        <>
          <div className="dv-perf-row sub">
            <span className="seg">incr {incrMs != null ? incrMs : "—"}</span>
            <span className="sep">›</span>
            <span className="seg">render {renderMs != null ? renderMs : "—"}</span>
          </div>
          <div className="dv-perf-row sub">
            <span className="seg">long {perfLongSnapshot().n}</span>
            <span className="sep">·</span>
            <span className="seg">max {r2(perfLongSnapshot().maxMs)}ms</span>
            <span className="seg muted">{""}</span>
          </div>
          <div className="dv-perf-row sub">
            <span className="seg">F {s.frames.f}</span>
            <span className="sep">/</span>
            <span className="seg">P {s.frames.p}</span>
            <span className="seg muted">{last && fresh && last.ops ? last.ops + " ops" : ""}</span>
          </div>
        </>
      )}
    </div>
  );
}