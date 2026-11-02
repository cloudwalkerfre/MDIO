// @ts-check
import { Component, memo } from "react";
import { report } from "../services/monitor.js";
import "./ErrorBoundary.css";   // 样式随件走（框架件在 core，壳包复用其 .dv-mini）

/**
 * ErrorBoundary —— 渲染区崩溃隔离（chrome 层共用件）。
 *
 * 功能照旧壳 components/demo/ErrorBoundary.jsx：任一渲染区崩溃只降级该区域，
 * 其余（Monaco / 文件树 / 日志 / 输入条）保持可用——避免整树卸载成白屏。
 *
 * 新架构下的两点差异：
 *   ① 错误同时走框架出口 `report`（装配点把它接到 devview 日志面板 ⇒ 面板可见、可被 CDP 断言），不止 console.warn；
 *   ② 样式落 ui.css（不再内联），与壳件一致。
 *
 * resetKey：内容版本（NodeView 传子树快照身份的 resetKey）。内容一变就自动清错 ⇒
 * 修好数据后不需要手点"重试"；不带 resetKey 时保持旧壳行为（只能手点重试）。
 */
class ErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { error: null, resetKey: props.resetKey };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  static getDerivedStateFromProps(props, state) {
    if (props.resetKey !== state.resetKey) return { error: null, resetKey: props.resetKey };
    return null;
  }

  componentDidCatch(error, info) {
    const name = this.props.name || "区域";
    try { report("EB✗: " + name + " 渲染崩溃: " + String((error && error.message) || error).slice(0, 160)); } catch (e) { /* 报告不可用也不能再崩 */ }
    if (typeof console !== "undefined" && console.warn) {
      console.warn("[ErrorBoundary] " + name + " 崩溃:", error, info && info.componentStack ? String(info.componentStack).slice(0, 300) : "");
    }
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="dv-eb" role="alert">
        <div className="dv-eb-ttl">{(this.props.name || "渲染区域") + " 渲染出错（已隔离，不影响其他区域）"}</div>
        <div className="dv-eb-msg">{String((this.state.error && this.state.error.message) || this.state.error)}</div>
        <button className="dv-mini" onClick={() => this.setState({ error: null })}>重试</button>
      </div>
    );
  }
}

/**
 * 业主：包一层 memo。它本身是普通 class 组件（没有 shouldComponentUpdate）⇒ 父级一渲染它必渲染。
 * 据实说明：children 是父级每次渲染新造的 element ⇒ 只有当父级重渲染而该区域 children 身份未变时才真省下这一层；
 * 真正的大头是让父级别每帧重渲染（见 NodeView 的 A2 扇出修复）。
 */
export default memo(ErrorBoundary);
