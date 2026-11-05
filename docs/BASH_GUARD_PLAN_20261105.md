# BASH_GUARD_PLAN_20261105

> 独立专项（本 worktree 只留计划，不实施）。触发：技术评估报告 D10「nah 守卫 fail-open + `spawn -sess` 可绕」。
> 基线：v0.8.5 / c26383b0（本 worktree 分支 `fix/fixes-20261105` 基于 28e54c35）。

## 1. 已确认的事实（只读核验 + 运行时探针）

- `@BASH` 是 agent 直连宿主 shell 的唯一通道；`nah` 分类器是它与宿主之间的闸门。
- 守卫实现 `mdio_py/src/mdio_py/plugins/bash/service.py:53-63` 有三处放行：
  1. `-nah false` 或 `_NAH_AVAILABLE` 为假 ⇒ 直接返回 None（依赖缺失静默降级）；
  2. `classify_command` 抛异常 ⇒ 返回 None；
  3. 只拦 `final_decision == "block"`，`ask` 档一律放行。
- 调用点只有 4 处：`service.py:367`（stream_oneshot）、`:404`（spawn_oneshot）、`:1139`（context_run）、`:1167`（context_stream）；`spawn_session`（`:737-756`）→ `_spawn_session_runner`（`:761`）→ `backend.context_stream` **没有守卫**。
- 运行时判别性探针（release 0.8.5 `.so`，临时 `MDIO_HOME`）：
  - `"curl … | sh" | @BASH` → `blocked by nah`；
  - `"curl … | sh" | @BASH.spawn` → `blocked by nah`；
  - `"curl … | sh" | @BASH.spawn -sess dev` → **未触发 nah**（进入 pty 创建，因环境 pty 耗尽返回 C500）；
  - `nah.bash.classify_command("rm -rf /")` → `ask`（`outside project: /`），按当前代码会被放行；
  - `nah.bash.classify_command("curl … | sh")` → `block`；`rm -rf /tmp/x` → `allow`。
- `nah` 在 `mdio_py/pyproject.toml` 是硬依赖（`nah>=0.7.0`），但代码仍保留 ImportError fail-open 分支。

## 2. 修复方向（实施时再定稿）

1. **单一收口**：把守卫移进所有执行路径共用的位置（`_build_execution_spec`，或 `LocalBashBackend.stream/context_stream` 入口），删除 4 个调用点的手工守卫；`spawn -sess` 自动被覆盖。
2. **策略明确**：fail-open → fail-closed（依赖缺失/分类器异常时拒绝执行并报可诊断错误），或至少把状态暴露到 status/文档；`ask` 档对无人值守 agent 的处理需明确（拒绝并提示，还是白名单放行）。
3. **状态可观测**：在 `/.mds/bash.md` / `@BASH status` / 日志中暴露 `guard: unavailable | active | disabled`，`-nah false` 要留审计痕迹。
4. **测试**：把上述 6 条探针固化成用例（四条调用形式 + ask/block + nah 不可用模拟 + spawn -sess），并纳入 CI 门；判别性要求：先证哨兵命令被 `nah` 判 block，再看路径是否拦截。
5. **威胁模型写清**：`@BASH` 的信任边界、`-nah false` 的运维责任、沙箱外命令的影响面。

## 3. 边界

- 本 worktree（fix/fixes-20261105）不做上述实现；只记录事实与方案，避免与 D23/ls/unmount 改动混在一起。
- 实施时建议单独 worktree/branch：`fix/bash-guard-<date>`，并保持 `@BASH` 行为兼容（默认更严格需在 CHANGELOG 记 breaking 变更）。
