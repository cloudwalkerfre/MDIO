// @ts-check
/**
 * ui_appview —— **mdio_ui 自带的 appview 容器件**（独立 mod，不是 wt 声明的 app）。
 *
 * 它就是 `#app` 那一格的**本体**：被 devview 包围，但不属于 devview（业主模型）。
 * 契约（与壳/宿主的握手上）：
 *   · 连接：建/复用影子根（B 路线）→ 挂载点 → 派发 `mdio:app-ready`（detail: { shadowRoot, mountPoint }）；
 *   · 断开：派发 `mdio:app-unmount`；
 *   · 属性变更：派发 `mdio:app-change`。
 *    事件一律 **bubbles + composed**：实测只写 composed 到不了 document（拿不到就没法驱动）。
 * 守卫（学 micro-app 的语义）：重复连接只处理一次（瞬时插拔两次不重复挂载）；已建影子根则复用。
 * 不 import React、不写 JSX —— 它只管容器与生命周期，内容由宿主/壳挂进来。
 */
import { ensureIsolatedRoot, adoptInto } from "ui_core/domScope";

export const APP_VIEW_TAG = "mdio-appview";
export const APP_READY_EVENT = "mdio:app-ready";
export const APP_UNMOUNT_EVENT = "mdio:app-unmount";
export const APP_CHANGE_EVENT = "mdio:app-change";

/** **影子硬边界开关**：默认关。关着时挂载点 = 元素自身（light DOM），不建影子根。
 *  开：元素自建影子根 + `#mount`，挂载点变成影子树里的那个 div。业主拍板后改这一行。 */
const APP_SHADOW = true;

//  **高度必须是确定值**（实测）：`min-height:100%` 只给最小高度 ⇒ 里面 `height:100%` 的 app 解析成 auto，
//    表现是「app 只占内容高度、下面露出壳的底色」。所以 host 与 #mount 都给 `height:100%`（host 的父是 flex 列，高度确定）。
const HOST_CSS = ":host{display:block;height:100%;font:inherit;color:inherit;contain:layout paint style;"
  + "container-type:inline-size;container-name:appview} #mount{display:block;height:100%}";

function fire(el, type, detail) {
  try { el.dispatchEvent(new CustomEvent(type, { detail: detail, bubbles: true, composed: true })); } catch (e) { /* 老环境无 CustomEvent：忽略 */ }
}

export class AppViewElement extends HTMLElement {
  static get observedAttributes() { return ["app-name", "app-doc"]; }

  constructor() {
    super();
    this._connectCount = 0;
    this._mount = null;
  }

  /** 挂载点：影子模式下 = 影子树里的 `#mount`；否则 = 元素自身（light DOM）。 */
  get mountPoint() { return this._mount || this; }

  connectedCallback() {
    this._connectCount += 1;
    if (this._connectCount > 1) return;   // 守卫：重复连接只处理一次
    this.setAttribute("data-esmap-scope", "mdio-app");
    // 布局/绘制容器（写在元素上，两种模式都成立；`:host` 那条是给影子模式兜底的）
    this.style.contain = "layout paint style";
    this.style.containerType = "inline-size";
    this.style.containerName = "appview";
    if (APP_SHADOW) {
      const sr = ensureIsolatedRoot(this);
      if (sr) {
        if (!this._mount) { this._mount = document.createElement("div"); this._mount.id = "mount"; }
        if (this._mount.parentNode !== sr) sr.appendChild(this._mount);
        try { const s = new CSSStyleSheet(); s.replaceSync(HOST_CSS); adoptInto(sr, s); } catch (e) { /* 老浏览器跳过 */ }
      }
    }
    fire(this, APP_READY_EVENT, { shadowRoot: APP_SHADOW ? this.shadowRoot : null, mountPoint: this.mountPoint });
  }

  disconnectedCallback() {
    this._connectCount = Math.max(0, this._connectCount - 1);
    fire(this, APP_UNMOUNT_EVENT, {});
  }

  attributeChangedCallback(name, oldValue, value) {
    if (oldValue !== value) fire(this, APP_CHANGE_EVENT, { name: name, oldValue: oldValue, value: value });
  }
}

/** 幂等注册（重复调用安全）。宿主装配点调用一次即可。 */
export function defineAppView() {
  if (typeof customElements === "undefined") return false;
  if (customElements.get(APP_VIEW_TAG)) return true;
  customElements.define(APP_VIEW_TAG, AppViewElement);
  return true;
}
