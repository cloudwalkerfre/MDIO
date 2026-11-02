// CSS 作为 mod 的**参考实现**：导出 sheet（CSSStyleSheet）⇒ 客户端装载器 adopt。
// 形态约束：平台原生 `with { type: "css" }` 被 Vite 8/Rolldown 拒绝，所以走 `?inline` + 构造。
import css from "./std/appearance.css?inline";

export const sheet = (() => {
  const s = new CSSStyleSheet();
  s.replaceSync(css);
  return s;
})();
export const cssText = css;
// CSS mod 的身份由装载器按 `sheet`/`cssText` 判定，不靠自报。
