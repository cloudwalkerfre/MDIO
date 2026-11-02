import asyncio
import json
import time
from mdio_py.core.interpreter import RuntimeContext

# 生命周期终结时的占位回调：可调用、无捕获、不保活任何对象。
# close() 用它替换 Rust 侧持有的 bound method（触发路径 Some(cb) → cb.call1），
# 被替换的 bound method 随引用消失而释放。
_NOOP_CALLBACK = lambda *a, **k: None


class HookManager:
    def __init__(self, workspace):
        self.workspace = workspace
        self.loop = asyncio.get_event_loop()
        self.queue = asyncio.Queue()

        # 熔断器：跟踪每个指令签名的触发时间线
        self.fuse_box = {}

        self.workspace.store.register_hook_dispatcher(self._on_rust_trigger)
        self.loop.create_task(self._hook_consumer())

    def _on_rust_trigger(self, cb_code: str, ctx_dict: dict):
        self.loop.call_soon_threadsafe(self.queue.put_nowait, (cb_code, ctx_dict))

    async def _hook_consumer(self):
        while True:
            cb_code, ctx_dict = await self.queue.get()
            try:

                # 1. 物理保险丝：1秒内的滑动时间窗口检测

                # now = time.time()
                # timestamps = self.fuse_box.get(cb_code, [])

                # # 清除沾满灰尘的记录，留下最近 1 秒的弹道轨迹
                # timestamps = [t for t in timestamps if now - t < 1.0]
                # timestamps.append(now)
                # self.fuse_box[cb_code] = timestamps

                # # 💣 如果哪怕是最微小的改变累加，在 1 秒内被极高频激发超过 3 次，拉闸断电！
                # if len(timestamps) >= 3:
                #     print(f"🚨 [MDIO Fuse] 熔断！检测到过载级风暴递归，切断引信: {cb_code}")
                #     err_msg = f"\n- error: [MDIO Hook] 循环递归风暴熔断 代码: {cb_code}"

                #     self.fuse_box[cb_code] = [] # 冷却清空，等待系统降温
                #     continue


                # 2. 正常还原造物主坐标并排入微任务

                ctx = RuntimeContext(**ctx_dict)
                asyncio.create_task(self.workspace.runtime.exec(cb_code, ctx=ctx))
            except Exception as e:
                print(f"🚨 [Hook Dispatch Error] {e}")
            finally:
                self.queue.task_done()

    def close(self):
        """显式生命周期终结（幂等）——业务分发语义零改动，仅切断保活链。

        背景：_hook_consumer 常驻 Task 与 Rust 侧 dispatcher 的 Py 引用
        构成 store 保活链（Task→coro帧→self→workspace→store），使 store
        释放后 6 个 worker 线程永不退出。close() 三动作：
          ① cancel 常驻消费协程（按 coro 帧 self 精确匹配——多实例安全）
          ② 占位回调替换 Rust 侧 bound method（可调用、无捕获、不保活）
          ③ 断自身引用（workspace/loop/queue）
        调用后：store 随引用计数回收，线程随 channel 关闭退出。
        不调用 close() 时，hook 分发按原路径工作。
        """
        # ① cancel 常驻消费协程（帧 self 匹配——不误杀其他 HookManager 实例）
        for t in asyncio.all_tasks():
            try:
                f = t.get_coro().cr_frame
                if f is not None and f.f_locals.get("self") is self:
                    t.cancel()
            except Exception:
                pass
        # ② 占位回调替换（Rust 侧 Option<Py> 换为无操作 lambda；被替换的 bound
        #    method 引用随之释放，触发路径与替换前一致）
        store = getattr(self, "workspace", None)
        if store is not None:
            store = getattr(store, "store", None)
        if store is not None:
            try:
                store.register_hook_dispatcher(_NOOP_CALLBACK)
            except Exception:
                pass
            try:
                store.register_ast_callback(_NOOP_CALLBACK)
            except Exception:
                pass
        # ③ 断链（幂等：重复 close 安全）
        self.workspace = None
        self.loop = None
        self.queue = None
