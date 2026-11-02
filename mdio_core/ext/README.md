# mdio_core/ext

外部宿主适配层。每个子目录是一个独立 Cargo package，不是 mdio_core/src 模块。

规则：

- 依赖方向：mdio_core <- ext/<target>；禁止反向依赖。
- 每个 target 自己声明 crate-type、构建脚本、平台产物与 CI。
- 平台相关产物（.duckdb_extension、.so、.dylib、target/）不提交 git。
- 根 workspace 统一 members/default-members；重依赖 target 不进入默认构建。
- 新增 target 先写接口、README、测试，再接构建链。

当前 target：

- arrow: DuckDB v2 extension adapter（package mdio_arrow，lib mdio_frame）。
  - 校准框架：`arrow/src/calibration.rs`（feature `calibration`，μ/trinity 驱动）。
  - 补齐计划：mdio VFS `/DEV/calibration_framework_fill_plan_20261024.md`。
  - 校准用法：
    ```bash
    cargo test -p mdio_arrow --features calibration                 # 全套（含 depth6 采样）
    cargo test -p mdio_arrow --features calibration depth5_full_report -- --ignored --nocapture
    cargo test -p mdio_arrow --features calibration depth5_baseline_dump -- --ignored --nocapture
    ```
  - 报告字段：`Report`（cells / signature_counts / exemplars）+ 采样块
    （full_lines / sampled_lines / sampled_docs / estimated_total /
    buckets A/B/C / sample_cells）。基线：
    `arrow/tests/baseline_depth5.json`。
