// KaTeX 样式的 **CSS mod 形态**：导出浏览器原生 CSSStyleSheet，
// 由公式组件在**用到时**递给宿主采纳（`globalThis.__mdioAdoptSheet`）⇒ 可卸载，不需要写 <head>。
// 为什么仍然惰性：katex-css.js 是 369KB 的字符串（字体 base64 内联）——不该进首屏。
import { KATEX_CSS } from "./katex-css.js";

/** 浏览器原生样式表；环境不支持构造时为 null（调用方回落到自注入）。 */
export const sheet = (() => {
  try {
    const s = new CSSStyleSheet();
    s.replaceSync(KATEX_CSS);
    return s;
  } catch (e) { return null; }
})();

/** 原文本（回落路径用：宿主没提供采纳入口时仍走自注入）。 */
export const css = KATEX_CSS;
