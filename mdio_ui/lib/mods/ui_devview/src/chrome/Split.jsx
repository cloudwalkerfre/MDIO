// @ts-check
import { useCallback, useEffect, useRef } from "react";
import "./Split.css";   // 样式随件走

/**
 * Split —— 可拖分隔条（chrome 层，纯视图件）：devview 的栏宽可拉伸。
 * 栏宽可拉伸（30/40/30、最小宽度夹取）；底层自写 pointer 事件 + 宽度回传，
 * 不引 UI 库。宽度由父组件持有并落 sessionStorage。
 */
export default function Split({ onDelta, min = 160, max = 900, width }) {
  const dragging = useRef(null);
  const onDown = useCallback((e) => {
    e.preventDefault();
    dragging.current = { x: e.clientX, w: width };
    e.currentTarget.setPointerCapture(e.pointerId);
  }, [width]);
  const onMove = useCallback((e) => {
    const d = dragging.current;
    if (!d) return;
    const next = Math.min(max, Math.max(min, d.w + (e.clientX - d.x)));
    onDelta(next);
  }, [max, min, onDelta]);
  const onUp = useCallback((e) => {
    dragging.current = null;
    try { e.currentTarget.releasePointerCapture(e.pointerId); } catch (err) { /* ignore */ }
  }, []);
  useEffect(() => () => { dragging.current = null; }, []);
  return (
    <div className="dv-split" title="拖动调整栏宽"
      onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerCancel={onUp} />
  );
}
