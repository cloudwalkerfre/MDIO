# MDIO Telemetry 用户手册

> 版本对应: 0.8.4 (2026-08-01)。dev-only 物理采样层——与 VFS/文档体系（/.mdio）无关。

## 1. 它是什么

三层全栈性能可观测框架，目标 sub-microsecond 开销（span 全开销 ~1.4ns），敢埋热路径：

```
Rust 内核 (25+ 预埋宏探针)  ─┐
Python 用户态 (@trace/span)   ├─→ 同一 session → 三轨存储 → 报告
JS/WASM 显示层 (钩子+PROBE)  ─┘     (trace/bench/layer)
```

- **跨语言单事件树**：Python span 内调 Rust FFI，自动嵌套进同一棵树（语言边界不是树边界）
- **零侵入**：feature OFF 时宏展开为 `()`；无激活 session 时 Python/JS 探针零开销
- **双模式**：trace（完整事件流）/ bench（时长分布统计）
- **可插拔报告**：六个槽函数组成分析管道，polars DataFrame 是数据单元

## 2. 快速开始

### 场景 A：脚本性能分析（显式会话）

```python
from mdio_py import MdioRuntime
from mdio_py.telemetry import TelemetrySession, span
import asyncio

async def main():
    rt = MdioRuntime('/mem')                      # 需要 VFS 时用 MdioRuntime
    sess = TelemetrySession(rt.store, "my_bench", mode="bench")  # 看耗时分布 → bench（§4 mode 即契约）
    with sess:                                    # 激活（thread-local push）
        with span("my_work"):                     # 手动 span
            ...                                   # 你的代码
        ...                                       # Rust 预埋探针自动记录
    df = sess.to_polars()                         # polars DataFrame
    print(df)                                     # name/dur_ns 原始行
    sess.report()                                 # 完整报告（终端+四产物）
    sess.close()
```

> 纯分析场景（不需要 VFS 操作）可用 `mrc.ProjectStore("/mem")` 直接作为 store——TelemetrySession 只依赖 store。

### 场景 B：零配置自动报告

```python
async def main():
    rt = MdioRuntime('/mem')
    with rt.telemetry("my_bench"):                # 进入即创建 bench session
        ...                                       # 代码段
    # 退出自动: 终端报告 + .md/.html/.json/.ipynb 四产物 + notebook kit 数据
```

### 场景 C：gateway 部署层

```python
# 浏览器 UI 接入后（CrdtClient 钩子 + flush）:
#   网关侧自动采集 RS_proc/JS/WASM layer 数据
# 查看:
curl http://127.0.0.1:8011/telemetry/layer-stats   # layer 样本
curl http://127.0.0.1:8011/telemetry/report        # 完整 Python 报告（四产物）
# gateway 启动时自动创建 trace session（telemetry="auto"）
```

## 2.1 完整示例：从插桩到报告

> 场景：分析 20 次批量写入的耗时分布。以下为实测输出（2026-08-01）。

**第 1 步 · 插桩**：Rust 预埋探针（vfs_resolve/execute_txn/plan_write 等）自动覆盖写入路径——不需要额外代码；场景级手动 span 包住业务逻辑；想加自己的 Rust 探针见第 3 章。

**第 2 步 · 分析脚本**：

```python
from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder
from mdio_py.telemetry import span

async def main():
    rt = MdioRuntime('/mem')
    rt.store.mk('genesis', '/work.md', parents=True, verbose=False)
    rt.store.write_locator('genesis', '/work.md', SelectorBuilder().build(), '/>',
                           '# Work\n- base: 1', auto_scaffold=True, god=True)
    with rt.telemetry('tutorial_write'):            # 自动创建 bench session
        for i in range(20):
            with span('batch_write'):               # 场景级 span
                rt.store.write_locator('genesis', '/work.md', SelectorBuilder().build(),
                                       '+>', f'- item: {i}')
    # with 退出即自动: 终端报告 + .md/.html/.json/.ipynb 四产物 + kit 数据
```

**第 3 步 · 运行并读报告**（`with` 退出自动打印）：

```
============================================================
  bench · 13 phases · total 100.8ms
  Hotspots: batch_write 2514µs  ·  wl_total 2511µs  ·  execute_txn 6µs
============================================================
shape: (13, 4)
┌──────────────────┬─────┬──────────┬────────┐
│ name             ┆ cnt ┆ total_ms ┆ avg_us │
│ batch_write      ┆ 20  ┆ 50.29    ┆ 2514.7 │   ← 场景 span: 2.5ms/次
│ wl_total         ┆ 20  ┆ 50.22    ┆ 2511.1 │   ← 写路径几乎全在这
│ execute_txn      ┆ 20  ┆ 0.11     ┆ 5.3    │
│ apply_crdt       ┆ 20  ┆ 0.03     ┆ 1.5    │
│ …                ┆ …   ┆ …        ┆ …      │
└──────────────────┴─────┴──────────┴────────┘
  13 rows · wall: 50.4ms · arm64 · 10 cores · release
```

解读：每次写入 2.5ms，`wl_total`（Python→Rust 桥）几乎占满——瓶颈在桥层而非 CRDT 内核（execute_txn 仅 5.3µs）。

**第 4 步 · 产物与可视化**（`.telemetry/tutorial_write/`）：

| 产物 | 内容 | 打开方式 |
|---|---|---|
| `report_<ts>.md` | 上面的终端报告 | 直接看 |
| `report_<ts>.html` | HTML 表格（深色主题） | 浏览器 |
| `data_<ts>.json` | 聚合数据（df_out） | 任意工具 |
| `report_<ts>.ipynb` | df_raw 嵌入 + 分析模板 | `sess.report(open_notebook=True)` → Try Jupyter |
| notebook kit | treemap 动画 + sunburst + Pipeline Latency | `sess.report(open_kit=True)` 或 kit URL |

**报告查看链路**（2026-08-01 梳理）：

```
ipynb:  本地 server (18766, CORS *) serve .telemetry/<session>/report_<ts>.ipynb
         → Try Jupyter (jupyter.org/try-jupyter — fromURL 扩展 + pyodide kernel)
         → ?fromURL=<本地 ipynb> 自动打开 → Run cell 执行 (数据已嵌入, Pyodide 浏览器内跑)
kit:    本地 server → /notebook/.observable/dist/index.html
         ?session=<名>&ts=<时间戳> 绑定某次报告数据 (否则最新)
         折叠: 代码块默认收起 (serve 时注入, 不动产物)
```

- **Try Jupyter 是唯一完整官方站**（fromURL 扩展 + kernel 都有；`jupyterlite.github.io/demo` 与 `jupyterlite.readthedocs.io/_static/lab` 有 kernel 无扩展；`jupyterlab-open-url-param.readthedocs.io` 有扩展无 kernel——均实测）
- **kit 报告绑定**：`notebook_kit_url("session", "ts")` 或 URL `?session=&ts=`——每次报告数据独立回看（treemap 动画仍全局）
- **kit 模板改动后重建**：`just kit-build`（Observable notebook-kit CLI → `.observable/dist`）
- **server 生命周期**：daemon 线程随进程退出——gateway/长驻进程内常驻；一次性脚本里 URL 仅在进程存活期有效

**变体**：
- 看调用树/时序：`mode="trace"` → `sess.report()` 自动额外产出 Chrome trace JSON（`report_<ts>.json`）拖进 speedscope（speedscope.app 打开文件即可）
- 看跨栈延迟：浏览器连上后（JS 采集自动），报告出现 Pipeline Latency 段（RS_proc/JS_*/WASM_*）
- 对比两次运行：kit 的 treemap 动画按 session 滑动对比（history.json 自动聚合）

## 2.2 自定义分析 + 自定义报告

延续 2.1 的场景（20 次批量写入）。可插拔管道：换 `data_fn`（分析）与 `render_fn`（展示）——同样数据，不同视角。

> **价值一句话**：埋点只负责采集，**分析视角是消费方的**——同一个 df_raw 可以按层、按前缀、按分布任意切（p50/p99/占比），换 `data_fn` 即换视角，采集层零改动。完整价值与用法见 §5。

**目标**：按层分类（场景层/桥层/内核层）看耗时占比。

```python
import polars as pl
import mdio_py.telemetry as tele

# ── 自定义分析: df_raw → 层聚合 (输入永远是 df_raw: name/dur_ns) ──
def layer_analysis(df):
    return (df.with_columns(
                pl.when(pl.col("name").is_in(["batch_write"])).then(pl.lit("1_scene"))
                  .when(pl.col("name").str.contains("wl_|ffi_")).then(pl.lit("2_bridge"))
                  .otherwise(pl.lit("3_kernel"))
                  .alias("layer"))
            .group_by("layer")
            .agg([(pl.col("dur_ns").sum()/1_000_000).round(2).alias("total_ms"),
                  pl.col("dur_ns").count().alias("cnt")])
            .with_columns((pl.col("total_ms")/pl.col("total_ms").sum()*100).round(1).alias("pct"))
            .sort("total_ms", descending=True))
tele.data_fn = layer_analysis

# ── 自定义渲染: 终端输出任意格式 ──
def pct_render(df):
    return "\n".join(f"  {r['layer']:<10} {r['total_ms']:>8.2f}ms  {r['pct']:>5.1f}%  ({r['cnt']} samples)"
                     for r in df.to_dicts())
tele.render_fn = pct_render

with rt.telemetry("tutorial_custom"):      # 同一入口, 退出自动用定制管道报告
    ...  # 20 次写入
```

**实测输出**：

```
============================================================
  bench · 3 phases · total 101.1ms
  Hotspots: (自定义聚合)          ← 无 name 列时 header 自动降级
============================================================
  1_scene       50.44ms   49.9%  (20 samples)   ← 场景 span
  2_bridge      50.34ms   49.8%  (20 samples)   ← Python→Rust 桥
  3_kernel       0.31ms    0.3%  (220 samples)  ← CRDT 内核
  3 rows · wall: 50.5ms · arm64 · 10 cores · release
```

> 注意：`data_fn` 的输入永远是 df_raw（`name_hash/name/dur_ns`）——聚合从 `dur_ns` 算起，没有现成的 `total_ms` 列。定制 `data_fn` 无需同时定制 `header_fn`（自动降级），history 聚合也自动跳过缺列文件。

**HTML 定制同理**：`tele.html_render_fn = lambda df: ...`（返回 HTML 字符串）——`report_<ts>.html` 产物即用你的渲染（见第 5 章）。

## 3. 探针：三层注入

### Rust 宏：给内核加探针（5 步上手）

```rust
// 1. 文件顶部导入宏（lib.rs 定义，#[macro_export]）
use crate::telemetry_guard;          // guard 宏
use crate::telemetry_span;           // span 宏

// 2. 函数第一行埋点
pub fn resolve_path(&self, ...) {
    let _guard = telemetry_guard!(self);                    // ← 仅 Store 入口方法
    let _span = telemetry_span!("vfs_resolve", "wt_uid" => wt_uid, "path" => vfs_path);
    ...
}
// 3. 编译: just build-core（feature 已在 pyproject 开启）
// 4. 运行: 任意 bench/trace session 自动记录
// 5. 查看: 报告/Chrome trace 里出现 "vfs_resolve"
```

**何时用哪个**（决策表）：

| 场景 | 写法 | 原因 |
|---|---|---|
| Store 入口方法（resolve_path/open_session/doc_flush/hook_register 等 FFI 层） | `telemetry_guard!(self)` + `telemetry_span!` | guard 保证无 Python 调用上下文里 session 也激活 |
| 普通函数/内部函数 | 只 `telemetry_span!("name")` | 嵌套自动（span_stack），内部调用者已激活 |
| 需要定位信息 | span 加 kv：`"doc_id" => doc_id` | 进报告的 args（trace 模式） |
| 高频小函数 | 不埋 | 嵌套的父 span 已覆盖耗时归属 |

**语法**：`telemetry_span!("name", "key" => value, ...)`——value 任意 `ToString`。命名用短名（如 `"vfs_resolve"`、`"execute_txn"`），与预埋点风格一致（Python 侧用层级名如 `"1_Compiler/1.Lark"`）。

**零开销保证**：feature OFF 时两个宏都展开为 `()`（0 字节）；feature ON 但 `telemetry="off"` 时 guard 走 AtomicBool 检查（~20ns）。预埋点分布：store.rs（vfs_resolve/session_open/doc_flush/hook_register）、session.rs（execute_txn/apply_crdt/crdt_apply_update/txn_acquire_lock）、selector.rs（select_nodes/select_for_write/eval_filter/step_traversal_filter）、op.rs（plan_write）、projection.rs（make_snapshot）、lib.rs FFI 入口（rl_total/wl_total/query_memory/ast_parse 等）。

**span 生命周期（通用规则）**：`telemetry_span!` 返回 RAII guard——**生命周期 = 绑定变量的作用域**。
- **必须 let 绑定**：`let _s = telemetry_span!("name");`——裸语句立即结束（dur≈0——测量失效）
- **drop 截断边界**：`drop(_s)` 提前结束 span——函数中途分段测量（一段一个 span 绑定）
- **嵌套自动 parent**（span 栈）——树结构只在 Trace 模式落盘（§4）
- **Rust 侧消费闭环**（建会话/激活/导出——测试与工具的使用方式）见 §10

### Python 惰性探针（不改业务代码）

```python
from mdio_py.telemetry import trace, span

@trace("1_Compiler/1.Lark", kind="parse")             # 装饰器（sync/async 都支持）
def lark_parse(s): ...

with span("write", doc_id=42, path="/x.md"):          # 上下文管理器（可带元数据）
    ...
```

无激活 session 时直接执行原函数/空操作——零开销。**元数据**（str/int/float/bool 全类型）进入报告的 args（trace 模式），bench 模式按语义丢弃。

### JS/WASM（显示层，已生产接入）

- `CrdtClient.onBeforeSend/onAfterRecv` 钩子自动采集 send_gap / recv_to_render
- `DocRenderer` 渲染完成自动 mark `render_commit`
- 页面卸载/手动 flush → POST `/telemetry/js-flush` → 汇入网关 session
- PROBE 帧（0x30/0xB0）测端到端 RTT（`window.__mdioTel.probe()`，DEV 模式）

## 4. 会话与模式：mode 即契约

**mode 由你指定，行为如本表所述——实现没有参数之外的默认行为，选错后果自负**。写代码前先定目标，再选 mode：

| 你的目标 | 入口 | mode |
|---|---|---|
| 看耗时分布/热点统计 | `runtime.telemetry(name)`（自动 bench）或 `TelemetrySession(store, name)` | `"bench"` |
| 看单次操作的调用树/时序 | `TelemetrySession(store, name)` | `"trace"` |
| 跨栈延迟（浏览器接入后） | 任意 mode——layer 段与 mode 无关 | — |

**mode 行为与后果**（参数即契约，无智能默认）：

| | `"trace"`（默认） | `"bench"` |
|---|---|---|
| 存什么 | 完整事件流（32B×4096 cap） | 每 phase 时长分布（100k/phase cap） |
| 时间/树 | 相对 epoch + parent 树 | 纯统计 |
| 报告 | Python 报告 + Chrome trace JSON（speedscope） | Python 报告（p50/p99） |
| 适合 | 单次流程的调用树/时序 | 循环/基准/任何分布统计 |
| 后果 | **循环/高频场景：事件环溢出静默丢弃——统计仅覆盖前 4096 条，无提示** | **无事件流：不产调用树与 Chrome trace JSON** |

```python
sess = TelemetrySession(store, name, mode="trace"|"bench")
sess.close()          # 真关闭: 释放 store 的 Rust session（幂等；后续新 session 自动重开）
# 覆盖警告: 显式创建时若已有 session（如 gateway auto）会打印提示（旧数据丢弃）
```

报告头部显示会话 mode（`trace · N phases` / `bench · N phases`）——对照 §4 表验证你的选择。

### 4.1 导出数据形态：桶序与场景切分（实战补充——2026-08-10）

**bench 模式的 `export_phase_json` 是"桶序"，不是时间线**——这是最容易踩的坑：

- accumulator 按 phase 分桶存储（每 phase 一个 Vec），导出按桶遍历——**同 phase 事件连续出现**（如 `ripple_total` × 86 条全部连着，然后 `update_total` × 25 条），跨 phase 事件**无法按位置对齐**
- 后果：**"按行号切场景"是错误假设**——从事件流位置推断"这是第几次编辑的第几个 phase"会得出错误结论
- 正确做法：**场景切分在 session 层做——session 名即场景维度**——探针/测试按场景建独立 session（`TelemetrySession` 或 Rust 侧 `TelemetryInner` 每场景一个），导出天然分离，不需要事后猜测

**桶内对齐配对技巧**（分析能力的关键）：

- 同一 session 内，每个 phase 的桶**按产生顺序排列**（push 追加）
- 若场景步骤（如每次编辑）恰好产生一次某 phase → **桶内序号 = 步骤序号**
- 跨 phase 桶内对齐可做配对分析：如结构编辑场景里 `update_total[k]` 与 `patch_*[k]` 是**同一次编辑的两条路径**——逐次配对对比（update vs patch 同次对照）
- 每次编辑恰好一次 vs 多次的 phase 可区分：计数 != 编辑数 ⟹ 该 phase 每次编辑多次（如 ripple_conv 每编辑 ~3 次——收敛扫描多行）

## 5. 报告：可插拔分析管道

数据流：`采集 → JSON 线协议 → df_raw → 槽函数 → 多形态消费`

```
df_raw (name/dur_ns 契约)
  ├─ data_fn      聚合（df_raw → df_out）
  ├─ header_fn    报告头部（默认固定格式）
  ├─ footer_fn    报告尾部（wall 时间/平台）
  ├─ render_fn    终端表格
  ├─ html_render_fn  HTML 渲染（默认简单表格）
  └─ layer_fn     layer 样本 → Pipeline Latency 段（avg/p50/p99）
```

槽是**模块级单例**：`import mdio_py.telemetry as tele; tele.data_fn = my_fn` 设一次全局生效。

### 为什么可插拔是有价值的（设计意图）

- **采集与分析解耦**：埋点（§3）只负责把 span 时长送进 session；**怎么分析是消费方的事**——同一个 `df_raw`（`name_hash/name/dur_ns` 契约）可以喂给任意视角：按前缀分层、按命名规则归组、算 p50/p99 长尾、按占比排序——**换 `data_fn` 即换视角，零改动采集层**
- **数据单元稳定**：`df_raw` 永远是 `{name_hash, name, dur_ns}`——polars DataFrame——所有槽函数的输入契约不变——自定义分析不依赖报告格式，报告格式不依赖分析逻辑
- **全局生效、一次设置**：模块级单例——设一次 `tele.data_fn` 后续所有报告出口（`runtime.telemetry` 作用域、`report_session`、gateway `/telemetry/report`）自动用你的分析——**不改业务代码**
- **分析结果本身就是数据**：`data_fn` 的输出（df_out）可再消费（HTML/JSON/kit 绑定）——分析不是终点，是管道的一环

### 使用方法（三个槽对应三类定制）

| 想定制什么 | 改哪个槽 | 签名 |
|---|---|---|
| 聚合/统计口径（怎么算） | `data_fn` | `df_raw → df_out`（polars 计算） |
| 终端展示（怎么看） | `render_fn` | `df_out → str` |
| HTML 展示（网页怎么看） | `html_render_fn` | `df_out → str(HTML)` |
| 报告头部/尾部 | `header_fn` / `footer_fn` | `df_out → str` |
| 跨层延迟聚合 | `layer_fn` | `layer 样本 → df` |

通用写法（输入永远是 df_raw——聚合从 `dur_ns` 算起）：

```python
import mdio_py.telemetry as tele
import polars as pl

def my_analysis(df):
    return (df.group_by("name")
              .agg([pl.col("dur_ns").count().alias("cnt"),
                    (pl.col("dur_ns")/1000).median().round(2).alias("p50_us"),
                    (pl.col("dur_ns")/1000).quantile(0.99).round(2).alias("p99_us")])
              .sort("p50_us", descending=True))
tele.data_fn = my_analysis      # 设一次——全局生效
```

要点：
- 没设的槽用默认（自动降级——如无 `header_fn` 用默认头部）——**只定你要的**
- 纯分析脚本可直接对导出 JSON 做同样操作（`json.loads(export_phase_json(...)) → pl.DataFrame`）——同一套 polars 分析技能

### 定制分析示例（profiler 迁移模板）

```python
import mdio_py.telemetry as tele
import polars as pl

# 注入探针
Lark.parse = tele.trace("1_Compiler/1.Lark")(Lark.parse)

# 定制聚合: 按 category 前缀
def cat_data(df):
    return (df.with_columns(pl.col("name").str.split("/").list.first().alias("category"))
              .group_by("category")
              .agg([pl.col("dur_ns").count().alias("cnt"),
                    (pl.col("dur_ns").sum()/1_000_000).round(2).alias("total_ms")])
              .sort("total_ms", descending=True))
tele.data_fn = cat_data

# 定制 HTML（如 plotly/自包含图表）
tele.html_render_fn = lambda df: f"<table>...</table>"

with runtime.telemetry("my_bench"): ...   # 退出即用定制管道出报告
```

### 统一出口 report_session

```python
from mdio_py.telemetry import report_session
out = report_session(rt.store, "name")    # 终端报告 + 四产物 + Pipeline Latency + 消费性 clear
```

所有报告路径都汇入它——单一管线：gateway `/telemetry/report` 端点、`runtime.telemetry` 作用域、`MdioRuntime.telemetry_report()`（CLI/脚本）与手动调用。报告头自动带 session 真实 mode（`trace · N phases` / `bench · N phases`）。

```python
rt.telemetry_report("my_run")    # MdioRuntime 方法 — 同上完整报告（默认名 cli_report）
```

### 产物与查看

- `.telemetry/<session>/`：report_*.md（终端）/ report_*.html / data_*.json（df_out）/ report_*.ipynb（df_raw 嵌入）
- **JupyterLite**：`sess.report(open_notebook=True)` → 浏览器内 Pyodide 分析（数据已嵌入）
- **Observable kit**（treemap 动画 + sunburst 钻取）：`sess.report(open_kit=True)` 或手动开 `http://127.0.0.1:18766/notebook/index.html`
- **speedscope**：trace 模式的 Chrome trace JSON（`report_<ts>.json`，`sess.report()` 自动产出）

### 数据分析实战：拿到 df_raw 之后（pandas 视角——2026-08-10）

`df_raw`（`{name_hash, name, dur_ns}`——每事件一行）就是你的 DataFrame——**从拿到它的那一刻起，你是数据科学家，不是看日志的**。数据都在，关键是怎么变换分解组合。标准工作流：

```python
import json, polars as pl

df = pl.DataFrame(json.load(open('export.json')))   # 数据落盘 → 文件读，不要打终端
```

**① 探索先行（数据形态先于分析设计）**

```python
df.group_by('name').agg([pl.len(), pl.col('dur_ns').median(), pl.col('dur_ns').max()])
```
先看清：有哪些 phase、各多少条、**导出顺序语义（§4.1 桶序！）**——形态没摸清前不要写大分析脚本。

**② 场景标记（行号/计数 → 列）**

```python
df = df.with_row_index('rid')
df.with_columns(pl.col('rid').map_elements(f, return_dtype=pl.Int64).alias('scene'))
```
步骤边界已知时，把每事件归到场景列（打字/结构/嵌套 × 文档）——后续全部按列过滤。

**③ 分布而非均值（长尾暴露）**

```python
pl.col('dur_ns').median()          # p50——稳健中心
pl.col('dur_ns').quantile(0.99)    # p99——长尾
pl.col('dur_ns').max()             # 极端值
```
均值掩盖长尾（实测 p50 0.88µs vs max 145.6µs——25 倍）。

**④ 宽表 pivot（编辑 × phase 矩阵）**

```python
df.pivot(on='name', index='edit', values='dur_ns', aggregate_function='sum')
```
每行一次编辑、每列一个 phase——合计/占比/配对一目了然。

**⑤ 配对 join（两条路径同次对照）**

```python
upd.join(patch, on='edit', how='inner')   # update 侧与 patch 侧逐次比
```
同一次编辑的两条路（如 update vs patch、插桩前 vs 后）——配对才看出一致性，均值比会掩盖波动。

**⑥ 规模趋势（小/大文档对比——O(n) 确认）**

```python
# 同 phase 两种规模均值比——未覆盖段 1.3→54.9µs（文档 44x）→ 线性确认
```
结论"值不值"必须用规模趋势回答：占比随规模上升 ⟹ 优化 ROI 高。

**⑦ 结论必须对照**——插桩前后 / L2 前后 / 全量 vs 增量 / 小 vs 大文档——**单次数字无意义**。

## 6. 跨栈延迟分析

layer 数据（网关+浏览器+WASM 汇聚，按 layer 名区分）：

| layer | 来源 | bucket |
|---|---|---|
| RS_proc | gateway 每包处理 | edit/probe/ctrl（按 tag） |
| JS_send_gap | 浏览器发送间隔 | edit |
| JS_recv_to_render | 接收→渲染延迟 | edit |
| WASM_* | worker 内处理 | edit |

```python
sess.layer_stats()          # 原始样本 list[dict]
# 报告里自动出现 Pipeline Latency 段（layer_fn 聚合 avg/p50/p99）
```

layer 段末尾附带各端上报状态行（原始设计语义）：

```
  PY + RS: ✓  |  JS + WASM: pending
```

✓ = 该端已有样本上报；`pending` = 尚未收到（浏览器未连接/未 flush）。浏览器接入并 flush 后自动变 ✓——报告前先看这行即可确认 JS/WASM 数据是否汇入。

## 7. 性能预算与上限

| 项 | 值 |
|---|---|
| span 全开销 | ~1.4ns（cputicks CNTVCT_EL0 0.7ns 时钟） |
| events（trace） | 4096 条 cap，溢出静默丢弃（events_dropped 计数） |
| meta 池 | 256KB 硬上限，超限丢弃元数据（事件仍记录） |
| accumulator（bench） | 100k 样本/phase cap（p99 精度足够） |
| layer_stats | 100k 样本 cap（RS_proc 每包一条，长跑防累积） |
| 报告 | `report_session` 消费性（报告后 session 清空）；`sess.report()` 非消费——可重复报告（数据保留） |

## 8. 陷阱与 FAQ

1. **报告没数据/数据不全？** 先对 §4 决策表——mode 是否匹配目标（循环统计用 bench，trace 溢出会静默丢弃）；polars/tabulate 是否安装（mdio_py dependencies 已声明，`uv sync` 即装）
2. **to_polars 只有 name/dur_ns？** 对——phase 数据；layer 数据走 `layer_stats()`
3. **JS flush 404？** vite dev 需要 `/telemetry` proxy（`MDIO_TELEMETRY_TARGET` env 指向 gateway）
4. **打开报告 URL 失败？** Micro HTTP server 是 daemon 线程——一次性脚本退出后 URL 失效；gateway/长驻进程内常驻
5. **报告后数据没了？** `report_session`（及 gateway/CLI 出口）消费性 clear——需要原始数据先 `to_polars()`/`dump_json()`；`sess.report()` 本身非消费，可重复调用
6. **如何加自己的 Rust 探针？** 函数入口 `telemetry_span!("name")`；Store 入口加 `telemetry_guard!`
7. **PROBE 不工作？** DEV 模式 `window.__mdioTel.probe()` 手动测 RTT；生产 UI 默认被动采集（无 probe 帧）
8. **polars 报错？** 报告层用新版 API（group_by/quantile）——需要 polars ≥1.0
9. **ipynb 打不开/无 kernel？** 检查 `JUPYTERLITE_LAB`（应指向 `jupyter.org/try-jupyter`——唯一同时有 fromURL 扩展 + kernel 的官方站）；本地 server 是否存活（18766）
10. **ipynb 报 ColumnNotFoundError？** 打开了旧产物（旧版嵌入聚合数据）——重新生成报告；当前版本嵌入原始 dur_ns
11. **kit 白屏？** 需 `just kit-build` 重建（模板改动后）；确认 server 存活（`_start_telemetry_server`）
12. **同名 session 多次报告？** 目录自动保留最近 5 份（`_prune_reports`）——旧产物自动清理，不会混淆；kit 绑定可精确回看某次（`notebook_kit_url("session", "ts")`）
13. **span 埋了但 dur≈0？** 裸语句调用 `telemetry_span!("x");` 立即释放 guard——必须 `let _s = telemetry_span!("x");` 绑定保活（§3 生命周期）
14. **导出 JSON 没有 kv/维度？** 部分有——`export_phase_json` 每行 `{name_hash, parent_hash, name, dur_ns, src, ...kv}`：`src="acc"` 行是时间口径（全样本、无 kv）；**带 kv 的 span 双写**（acc 行 + `src="events"` 行携带 kv，受 4096 cap 截断）。聚合前先按 `src` 过滤，否则同一样本计两次。kv 在 Trace 的 `report_json_trace` args 同样可见
15. **分析数据与外部计时器读数矛盾？** span 时长 = CNTVCT_EL0（**墙钟**）差值换算纳秒——**包含线程被抢占与调度等待**；与同进程内的 `Instant` 同口径、可比（实测双钟对照一致）。真正不可比的是 **CPU 时间**口径与进程外时钟——报告必须注明用的是哪个口径（§10 时钟纪律）
16. **按行号切场景失败/事件顺序对不上？** bench 导出的 `export_phase_json` 是**桶序非时间线**——同 phase 事件连续出现，跨 phase 无法按位置对齐。场景切分在 session 层做（session 名即场景，§4.1）；同 session 内桶内序号 = 产生序——步骤恰好各产一次时可跨 phase 对齐配对
17. **进程内首个 span 慢 ~50ms，之后永不复发？** 不是挂起也不是缓存冷启动——首个 `ticks_to_ns` 调用触发 `cputicks` 频率校准（5 次 × 每次 spin 10ms，OnceLock 每进程一次）。**谁先录第一个 span 谁付**（主线程/worker 都可能）；不开会话不出现（noop guard 不换算）。规避：warmup 轮剔除（分析纪律 k≥2/3）；生产进程启动时先调一次 `ticks_ns()`（Rust 侧 pub）即付清——实测预热一个空 span 可把税精确转移
18. **bench 想按入口/场景分组却分不开？** bench 导出无 ts（时间轴）——不能按时间窗归因。归组三条路：每入口独立 session（session 名即分组）、kv 键（注意 kv 行受 4096 cap 截断）、`parent_hash`（调用者归属——见下条）。
19. **trace 里 worker 的 span 没有父节点？** Detect/Path/FJ 的 span 是根（线程局部栈，parent=None）——归因靠"启动点落入 op 窗口 + ~100µs 尾容差"；worker 在窗口缝隙启动属正常调度形态。这是分析脚本约定（`fdc_decomp.py`/`prod_loop.py` 同律实现），不是框架行为
20. **多个 session 的 ts 拼在一起后时序全乱？** trace 的 ts 每 session 从 ~0 起（epoch 在会话创建时定）——跨 session 不可合并，逐 session 建树再聚合
21. **对照组结果和预期相反？** 检查顺序混淆——一次性成分（校准税、pyo3 桥冷启动）使"对照组放后面"的实验全假，对照必须同序位（或同进程内先跑对照组）
22. **分析结论自己都不敢信？** 用 `telemetry-check <trace.json>`（链接缺失/父子记账违例/歧义三判据）把关；因果树归因逻辑以 `fdc_decomp.py`/`prod_loop.py` 为参考实现——复用而非重写
23. **怎么知道"谁在调这个 span"（跨线程归属）？** 用导出里的 `parent_hash`（2026-09 补入 `export_phase_json`）：它是父 span **名字**的哈希，配合同批导出行的 `name_hash` 建 `name_hash → name` 表，即可把任意 span 归到它的调用者名下——例如把 `seg_build` 归到 `adapt_parts` / `patch_zone` / `mz_mid`。
    注意三条边界：① 归属是**名字级**（同名父的多个实例不区分）；② 祖先链要逐级查表，导出里没有现成的树结构；③ worker 线程的根 span（Detect/Path/FJ）`parent_hash` 指向会话根——跨线程因果仍靠"启动点落入 op 窗口 + ~100µs 尾容差"（§8.19）。

## 9. 架构附录

```
采集（Rust 宏/Python span/JS 钩子）
    → thread_local ACTIVE_SESSION 传播（py.detach 同线程不变量）
    → TelemetryInner 三轨（events/accumulator/layer_stats）
    → JSON 线协议（export_phase_json: {name_hash, parent_hash, name, dur_ns, src, ...kv}）
    → df_raw → 六槽管道 → 多形态消费（终端/HTML/JSON/ipynb/kit/speedscope）
```

- 开关：feature OFF → 宏展开 `()` 零字节；`telemetry="off"/"auto"/"full"`（MdioRuntime 参数）
- pytest：`MDIO_TELEMETRY=0` 跳过 / `=bench|trace` 切换模式
- 网关：`telemetry="auto"` 启动即建 trace session；`/telemetry/*` 四端点

## 10. Rust 侧闭环与数据形态（使用者补全——2026-08-09）

> 面向"用 telemetry 做测量与分析"的使用者——建会话/激活/埋点生命周期/数据理解。不含具体业务场景（探针注入见 §3，报告管道见 §5）。

### 10.1 Rust 侧会话三件套（建会话 → 激活 → 导出）

任何 Rust 代码（测试/工具/bin）采集埋点数据的固定三步：

```rust
use std::sync::Arc;
use mdio_core::telemetry::{TelemetryInner, TelemetryMode, set_session, export_phase_json};

// ① 建会话（mode 二选一——见 §4 选型）
let sess = Arc::new(TelemetryInner::new("会话名", TelemetryMode::Bench));

// ② 激活（guard 必须保活——Drop 时还原；作用域 = 采集范围）
let _guard = set_session(&sess);

// ③ 被测代码——已埋点（telemetry_span! 等）自动记录到 sess

// ④ 导出（任意时刻——guard 存活期内可多次）
let json = export_phase_json(&sess);   // [{name_hash, parent_hash, name, dur_ns, src, ...kv}]
```

- **guard 保活决定采集窗口**——函数级/块级绑定；drop 后探针自动 noop（零开销）
- 一个进程可多会话串行（先 drop 旧 guard 再 set 新）；并发不混（thread_local）
- 导出 JSON 可多次取；`report_all` 才是消费性 clear

### 10.2 span 生命周期（绑定 → 保活 → 截断）

`telemetry_span!` 返回 RAII guard——**生命周期 = 绑定变量的作用域**：

```rust
let _s = telemetry_span!("名", "k" => v);   // 必须绑定——裸语句立即结束（dur≈0）
// ...被测段...
drop(_s);                                    // 显式截断——函数中途分段测量
```

- **必须 let 绑定**（裸语句 = dur≈0——测量失效）
- **drop 截断边界**——想测哪段就把 guard 绑定/释放到哪段
- **嵌套自动 parent**（span 栈）——树结构只在 Trace 模式落盘（见 10.3）

### 10.3 双模式：选型与数据形态

| | Trace | Bench |
|---|---|---|
| 存什么 | 事件流（32B/条——4096 cap 溢出静默丢 + events_dropped 计数） | 每 span 时长分布（100k/phase cap）；**kv span 双写**：acc 行（时间口径）+ events 行（kv 口径，共用 4096 cap） |
| 树/层级 | parent_hash 链（树可重建） | **不记录——名字级统计；导出无 ts** |
| kv（meta） | 进事件 TLV 池——`report_json_trace` 的 args 可见 | **进 events 行（`src="events"` 携带 kv；acc 行无 kv）** |
| 导出 | export_phase_json（平铺事件）/ report_json_trace（Chrome trace） | export_phase_json（acc 分布行 + kv 事件行——按 `src` 区分，聚合前先过滤） |
| 用途 | 单次流程深挖（调用树/时序） | 循环/基准/分布统计（p50/p99） |

**数据形态（通用）**：
- 导出 JSON = **每事件一行** `{name_hash, name, dur_ns}`——**不聚合**——分布/占比是消费方的事（polars/脚本）
- **kv 维度只在 Trace 的 JSON args 可见；Bench 导出无 meta**——带维度分析要么 Trace 读 args、要么扩展导出层
- 同名字 span 的多次调用是独立行（可算中位/长尾）

### 10.4 会话生命周期

- **单实例**：新会话覆盖旧（旧数据丢）——Python 侧有覆盖警告；Rust 侧同理（换会话前先导出）
- Python `__exit__` 快照缓存（覆盖后仍可读）；Rust 侧导出即取（无缓存——覆盖前导出）
- `report_all`/`report_session` 消费性 clear（报告后数据清空）——先导出再报告

### 10.5 时钟纪律（单一事实源）

- span 时长 = 裸读 CNTVCT_EL0（**墙钟**——线程被抢占、调度等待期间照走）的差值，经 `cputicks::Ticks::frequency()` 换算纳秒——该频率的校准源就是系统时钟，所以 **dur_ns 就是墙钟纳秒**
- **span dur 如实包含抢占与等待**——它不是 CPU 时间；"计算成本"要另用 CPU 时间口径，报告必须注明口径
- **与同进程内的 `Instant` 同口径、可比**（实测双钟对照一致，可互证）；真正不可比的是 CPU 时间与进程外时钟
- **每进程一次性 ~50ms 校准税**：首个 `ticks_to_ns` 调用触发频率校准（5 次 × 每次 spin 10ms，OnceLock 缓存）——谁先录第一个 span 谁付；warmup 轮剔除或启动时先调一次 `ticks_ns()` 规避（§8-17）

### 10.6 通用陷阱清单

| 陷阱 | 现象 | 规避 |
|---|---|---|
| span 裸语句 | dur≈0 | let 绑定 |
| 高频循环用 Trace | 4096 cap 溢出静默丢 | 循环/基准用 Bench |
| Bench 测调用树 | 无事件流 | 树分析用 Trace |
| 新会话覆盖 | 旧数据丢 | 导出后再换 |
| 报告后没数据 | report 消费性 clear | 先 export 再 report |
| kv 在 Bench 不可见 | 维度分析缺数据 | 读 `src="events"` 行（双写；受 4096 cap） |
| Python 3.14 无 event loop | 构造报错 | asyncio 内使用 |
| 混外部计时 | 读数矛盾 | 墙钟口径可比；CPU 时间与进程外时钟不可比 |
| 首 span 慢 50ms | p50 被首轮污染 | warmup 剔除 / 启动时先调 `ticks_ns()` |
| bench 归组 | 无 ts 无树分不开入口 | 每入口独立 session 或 kv 键 |
| worker span 无父 | 因果树挂不上 | 启动点归因 + 尾容差（脚本约定） |
| 跨 session 拼 ts | 时序全乱 | 逐 session 建树再聚合 |
| 对照组反直觉 | 一次性成分顺序混淆 | 对照同序位 |
| 结论没把握 | 归因/记账存疑 | `telemetry-check` 把关 |

### 10.7 分析方法论：插桩分层与测量纪律（实战补充——2026-08-10）

**插桩分层下钻**（数据引导下一层）：

```
函数级 span（update_tree——总成本）
  → 段级 span（tree_map_build/tree_tail_clone/...——内部七段分解）
  → 计数器 span（每节点/每 children 一次——次数 × 单次成本）
```

先粗后细——每一层的数据决定下一层插在哪：总成本大 ⟹ 拆段；段大 ⟹ 拆计数。**计数器 span 是高频循环内插桩的形态**（每节点一次——bench 分布给出次数与单次成本）。

**纯插桩 = 测量资产**：`telemetry_span!` feature OFF 展开 `()`——分析插桩零成本且不改行为——**可长期保留提交**（L2 提交即保留测量插桩）。区别于诊断代码（Instant 计时/print——会污染生产路径——禁止）。

**数据落盘不走终端**：大数据量导出到文件（`export_phase_json → 文件`），再 `pl.DataFrame` 读——33.6MB stdout 再 grep 是反面教材（§5 实战工作流 ①）。

**测量对照三原则**：
1. 插桩前后（插桩本身开销）
2. 优化前后（L2 前 vs 后——收益归因）
3. 全量 vs 增量 / 小 vs 大文档（规模趋势——O(n) 确认）
