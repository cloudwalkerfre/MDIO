// watchdog.mjs —— 判据套件的**超时 / 卡死检测**。
//
// 为什么要有：CDP 目标没了、页面白屏、`await ev()` 永不回 ⇒ 套件会**静默挂着**，人只看到「跑不完」；
// 更坏的情况是打错端口连到业主的 Chrome（9222）上去操作别人的页面。三条机制都放在这里：
//   ① `armWatchdog(label, ms)` —— 进程级看门狗：到点打印卡死位置并 `exit 2`（默认 180s，可用 `JUDGE_DEADLINE_MS` 覆盖）；
//   ② `withCallTimeout(label, send)` —— 单次 CDP 调用超时（默认 20s，`JUDGE_CALL_MS` 覆盖）⇒ 把「永远挂着」变成「抛异常」；
//   ③ `guardTarget(cdp, label)` —— 目标守卫：默认**拒绝** 9222（业主 Chrome），要连必须显式 `ALLOW_OWNER_CDP=1`。

export const DEADLINE_MS = Number(process.env.JUDGE_DEADLINE_MS || 180000);
export const CALL_MS = Number(process.env.JUDGE_CALL_MS || 20000);

let armed = null;
/** 装一个进程级看门狗；返回「解除」函数（正常跑完时调用，避免误杀）。 */
export function armWatchdog(label, ms = DEADLINE_MS) {
  // 每套传自己的 ms（= 实测 ×2）；**环境变量优先级最高**，方便临时逼出「卡死路径」（JUDGE_DEADLINE_MS=200 ⇒ 必杀）。
  const envMs = Number(process.env.JUDGE_DEADLINE_MS || 0);
  if (envMs > 0) ms = envMs;
  const t0 = Date.now();
  const timer = setTimeout(() => {
    console.error("[watchdog] " + label + " 卡死：超过 " + Math.round(ms / 1000) + "s 没有结束（已跑 " + Math.round((Date.now() - t0) / 1000) + "s）⇒ 判定挂起，退出 2。");
    process.exit(2);
  }, ms);
  // **unref**（全量扫描后的结论）：deadline 抓的是「**还在干活**的挂起」；
  //   老的测试脚本大多没有显式 `process.exit`，跑完只剩这个计时器一个句柄 ⇒ 不 unref 会把「跑完不退」误判成卡死
  //   （扫描里 30 套全绿却 exit 2）。unref 后：干完了 ⇒ 事件循环自然空 ⇒ 正常退出；真挂起（还有 pending 的 await / 开着的 socket）
  //   ⇒ 事件循环还活着 ⇒ 计时器照常到点杀。
  armed = () => { clearTimeout(timer); armed = null; };
  return armed;
}
export function disarmWatchdog() { if (armed) armed(); }

/** 给任意 promise 加超时：超时即拒绝（这样 await 会抛，而不是永远等）。 */
export function withTimeout(promise, ms, what) {
  let timer = null;
  // 同样刻意不 unref：计时器必须保活到它 fire（否则事件循环空了会以 13 退出，看着像「静默失败」）。
  const t = new Promise((_, rej) => { timer = setTimeout(() => rej(new Error("[timeout] " + what + " 超过 " + ms + "ms 无响应")), ms); });
  return Promise.race([promise, t]).finally(() => { if (timer) clearTimeout(timer); });
}

/** 包一层 CDP send：每次调用带超时 + 计数（出错时能说出是第几次哪条命令）。 */
export function withCallTimeout(label, send, ms = CALL_MS) {
  let n = 0;
  return (method, params) => { n += 1; return withTimeout(send(method, params), ms, label + " · " + method + " #" + n); };
}

/** 目标守卫：拒绝 9222（业主 Chrome）；返回原 cdp 方便 `const CDP = guardTarget(...)` 这种写法。 */
export function guardTarget(cdp, label = "suite") {
  try {
    const u = new URL(String(cdp));
    if (u.port === "9222" && process.env.ALLOW_OWNER_CDP !== "1") {
      console.error("[guard] " + label + "：拒绝连 " + cdp + "（9222 = 业主的 Chrome）。用自建无头（如 9223），确需连它请显式 ALLOW_OWNER_CDP=1。");
      process.exit(3);
    }
  } catch (e) { /* URL 不合法：交给上层报错 */ }
  return cdp;
}
/** **异常守卫**：套件里任何未捕获异常 / 未处理的 Promise 拒绝 ⇒ 打印清楚 + exit 4（不要让它变成「跑不完」）。
 *  必须在套件最前面调用一次。 */
export function installGuards(label) {
  const die = (what) => (err) => {
    console.error("[guard] " + label + " " + what + "：" + String((err && (err.stack || err.message)) || err).slice(0, 500));
    console.error("[guard] " + label + " ⇒ 判定套件失败（不是挂起），退出 4。");
    process.exit(4);
  };
  process.on("uncaughtException", die("未捕获异常"));
  process.on("unhandledRejection", die("未处理的 Promise 拒绝"));
}

/** **页面异常检测**：CDP 的 `Runtime.evaluate` 返回 `exceptionDetails` 时，以前我们只看 `r.result.value`
 *  ⇒ 页面里的 TypeError 会变成 `undefined`，断言只是「假红」而看不到原因。这里把它变成显式的错误记录。
 *  `errs` 由调用方收集，套件末尾用 `chk("无页面异常", errs.length === 0, …)` 断言。
 *  `JUDGE_STRICT_PAGE=0` 可降级为只记录不打印。 */
export function pageExceptionText(r) {
  if (!r || !r.exceptionDetails) return "";
  const d = r.exceptionDetails;
  const desc = (d.exception && (d.exception.description || d.exception.value)) || d.text || "";
  return "[page-exception] " + String(desc).slice(0, 240);
}
export function makeEv(send, label, errs, ms, wrap) {
  return async (expr) => {
    // `wrap`（可选）= 表达式包装器（例如 check-router 的「按根门面」：影子根里先找、回退文档）。
    const text = typeof wrap === "function" ? wrap(expr) : expr;
    const r = await withTimeout(send("Runtime.evaluate", { expression: text, returnByValue: true, awaitPromise: true }), ms || CALL_MS, label + " · evaluate");
    const t = pageExceptionText(r);
    if (t) { (errs || []).push(t); if (process.env.JUDGE_STRICT_PAGE !== "0") console.error("  " + t); }
    return r && r.result ? r.result.value : undefined;
  };
}
