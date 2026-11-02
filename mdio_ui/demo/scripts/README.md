# scripts/ —— 操作手册（最短版）

> 完整设计与依据在**仓库根**：`MODS_LAYER_PLAN.md`（主线）与 `WHEELS_SURVEY.md`（研究与上游）。
> 它们是**工作/交接文档**（AGENTS.md 第 8 条：进度放仓库根），不是 mdio 文档、不进任何 wt。
> 纪律：md 的结构/内容只有一个读取来源 = **引擎**（exec → 交付树/FJ）；工作文档是散文，不做机械结构检查，也不许自写解析器。
> 本文件只回答两件事：**怎么起**、**怎么验**。

## 前提

- 仓库根 `<repo-root>`；Node（本机 v24）；`.venv`（Python 网关）；Chrome + CDP `9222`；端口 `8011`（网关）/ `5178`（宿主）。
- 在范围内的工作树：`genesis` · `exp_swap` · `exp_pages`（**其余 wt 未对齐新版，不管**）。

## 两条命令

```bash
cd mdio_ui/demo

node scripts/up-all.mjs            # 起全套：协议同步 → 网关（已在则复用）→ 宿主（平滑重启）→ 就绪检查
node scripts/up-all.mjs --check    # 只体检，不动任何进程（退出码即健康判定）

node scripts/verify-all.mjs               # 判据体检（dev）：judge · check-layers · pages-live · swap-live
node scripts/verify-specials.mjs [--only a,b]       # **四条专项**一条命令（生产链 / 冷启动 / 两种接入形态；约 3–5 分钟，自己持编排锁）
node scripts/verify-all.mjs --with-e2e    # 再加 e2e（完整基线）
node scripts/verify-prod.mjs              # **生产链路体检**：build → preview → CDP 断言（客户端启动/CSS mod 采纳≥1/无异常/无自注入）→ 自动恢复 dev
```

两者的判定口径都是**各套件自己的总结行**；`verify-all` 末行是 `✓ 全部通过` 或 `✗ 失败 N 项`，退出码 0/1。

## 其余常用

```bash
node scripts/build-wt-ui.mjs <wtRoot 或 包目录>   # 构建（默认**原子**：旁路 → 搬代 → 原子改指；留两代）
node scripts/build-wt-ui.mjs <路径> --stage-only  # 只旁路构建；随后 node scripts/publish-wt-ui.mjs <同一路径> 发布
node scripts/watch-wt-ui.mjs                      # 源码级热替换：改源码 → 自动重建所属包 → 刷新页面即可
node scripts/restart-host.mjs                     # 重启宿主（等端口释放 + 就绪轮询；--check 只探就绪）
node scripts/restart-gateway.mjs [--check]          # 重启引擎/网关（带完整 env + 挂载根校验；--check 只看状态）
node scripts/sync-wt-uses.mjs                     # 一处生成 uses/links/符号链接 + 断链清理
node scripts/build-monaco.mjs                     # 把 monaco 打成自包含 ESM 进 public/（vendor 大件不进 Vite 转换链）
node scripts/check-protocol.mjs                   # 判据：每个可挂载的包在 uses / dependencies.link / node_modules 三处都有条目
node scripts/check-externals.mjs                  # 判据：mod 产物不得内联共享依赖（react/mobx 系），且裸名引入对齐源码用法
node scripts/check-tree-form.mjs                  # 判据：在范围内的树 —— 声明只用逻辑名 / ui 只放 mod 包 / 无官方件副本
node scripts/check-tree-source.mjs                # 判据：树从哪来 —— 接口层含 /.mds/ui.md · 第 1 条 WS 树帧 == 同 wt 的 HTTP 树 · mounts 过滤仍在（只连网关；见下文「树的两条来源」）
node scripts/check-no-residue.mjs                 # 判据：探针/fixture 不留残渣（astprobe 0 字节、无临时文档、无 __*probe 件）
node scripts/check-dev-provider.mjs                  # 判据：开发侧裸名必须由宿主 shim 承重（平台映射存在但不能顶替；见 PLAN §58）
node scripts/check-css-mods.mjs                       # 判据：dev 路径上 CSS mod 真被采纳（带真实规则，非自注入兜底）
node scripts/check-doc-render.mjs                   # 判据：文档面渲染完整（标题数/列表项数 = 交付树里的节数/bullet 数；防"命中≠能画"静默空白，见 PLAN §71）
node scripts/check-lock.mjs                          # 判据：按包构建锁（活锁拿不到 / 空内容拿不到 / 陈旧可抢占 / 正常不留锁）
node scripts/prepare-pending.mjs --item 1|2 [--apply]   # 待批两项的可执行形式（默认 dry-run 只打印；--verify 1|2 跑验收）
node scripts/ensure-built.mjs [--dry]             # 启动前预构建（restart-host/up-all 已自动调用；--dry 只看清单）
node scripts/ensure-tree-deps.mjs [--check]       # 每棵树的 ui/ 依赖落点：补 mdio_ui 链接 + 报告缺的运行时依赖
node scripts/rehearse-new-tree.mjs                # **新树从零接入演练**：建树 → 登记 → 依赖落点 → 构建 → sync → 边界实证 → 清场
node scripts/rehearse-css-unadopt.mjs               # **CSS mod 可取可卸**演练（加声明→采纳；删声明→卸下；跑完自清）
node scripts/check-vp-cache.mjs                   # Vite+ 缓存三态回归（自建临时 lab；缺 vpr 会"⊘ 跳过"）
node scripts/rehearse-coldstart.mjs               # 冷启动自愈演练（移产物 → 预构建 → 自愈 → 判据）
node scripts/rehearse-onboarding.mjs              # **第三方接入演练**：造一个单文件 mod → 声明 → 重启 → 验证装载 → 清场
```

## 五条时序坑（都踩过，务必照做）

1. 改**声明** / 改 `esmx.exports`、`uses` / **新包首次可用** ⇒ **要重启宿主**（挂载表启动期解析）。
2. 改**盘上文档或声明后立刻要验** ⇒ **要重启网关**（引擎持快照）。
3. **重建 mod 包** ⇒ **不用**重启宿主（import map 每请求生成，hash 自动更新）。
4. 重启宿主一律用 `restart-host.mjs`（否则 bind 失败会伪装成"无故退出"）。
5. CDP 探针前**先关掉所有旧页签**；起 watcher 后**先确认它打印了启动行**再改文件。

## 树的两条来源（已核实事实 · 2026-10-01）

一条 WS 连接（`/ws/gateway/<wt>`，`mdio_py/src/mdio_py/gateway/handlers/multiplexer.py:42`）在客户端连接时自动发出 `UpTree(0x01)`
（`mdio_ui/lib/wasm/src/lib.rs:1020`）后，**收到两条树帧**（`DnTree = 0x81`，`mdio_ui/lib/mods/ui_core/src/services/client.js:49`）：

| 帧 | 谁发 | 内容 | 与谁等价 |
|---|---|---|---|
| #1 | 网关补发 | **连接 wt**、含隐藏 | `HTTP /fs/tree?hidden=true&wt_uid=<连接 wt>` |
| #2 | 引擎答复 | **genesis**、不含隐藏 | `HTTP /fs/tree?hidden=false&wt_uid=genesis` |

- #1 = `mdio_py/src/mdio_py/gateway/handlers/crdt.py:256-260`（`store.get_vfs_tree(conn_wt, True)` 后直接 `send_bytes`）。它排在同文件 `:273-274` 的 Direct 发送**之前** ⇒ 先到。
- #2 = 同文件 `:251` 的 `store.handle_packet(data)` → `mdio_core/src/store.rs:1583-1586`（`refresh_tree_json("genesis", false)` **写死**：与连接 wt 无关）。
- HTTP 端点 = `mdio_py/src/mdio_py/gateway/api/fs.py:11-20`，与 #1 走的是**同一个** `refresh_tree_json`（`mdio_core/src/lib.rs:833`）⇒ 两条来源逐条等价（判据比对 id 集 + path 集）。
- 网关补发是**有意补偿**：出自提交 `3280f9d4`（2026-09-29「切档 ≠ 退订 + 投递通道化 + 每连接 Subscriber」——该提交的 crdt.py 改动块里就有这段 0x01 补发），动机即 crdt.py:252-255 的注释（Rust 的 0x01 分支回的是写死 genesis 的树）；同轮计划记着这条连接级整棵树订阅：`GATEWAY_UNIFIED_PLAN.md:53`（UpTree → sys_events）。
- 实测读数（2026-10-01 · genesis）：HTTP `hidden=true` = **219 条**（`/.mds/` 110 · `/.mds/mounts/` **99** · `/.mds/ui.md` id=111）；HTTP `hidden=false` = **109 条**；一次 UpTree 的两条帧 = **[219, 109]**。换一棵已登记 wt（`c8Page01`，HTTP 9 条）：帧 = **[9, 109]** ⇒ #1 跟连接 wt 走，#2 是常量。
- 应用**两条都丢弃**：`mdio_ui/demo/src/entry.client.ts:110` 的 `onTree` 是空实现（帧到达只由 monitor 播报计数：`mdio_ui/lib/mods/ui_core/src/services/monitor.js:48-51`）。
- UI 树来自 HTTP：唯一调用点 `mdio_ui/demo/src/entry.client.ts:120`（`client.fullTree(WT)`，定义在 `client.js:688`）。它的消费点：`:121` 过滤 `/.mds/mounts/**` · `:122` entries/allEntries · `:256-261` boot 找 `/.mds/ui.md` 的 id · `:89` docPathOfId · `:245`/`:329` 产品面可见集 · `:337` 交 FileTree。
- `/fs/tree` 的其它消费者：宿主同源转发（`mdio_ui/demo/src/entry.node.ts:287-290`；同源取件的 wt 来源 = Referer 的 `?wt=`，`:150-158`）与 `client.js:688`。跨 wt 读（`readDoc(wt, path)`，`client.js:701`）**必须**继续走 HTTP —— 浏览器里的引擎只挂自己那棵 wt，属固有。
- **判据** `node scripts/check-tree-source.mjs`（`verify-all` 默认跑；网关不在时 `⊘ 跳过` + 退 0）。断言 = ①接口层树含 `/.mds/ui.md` ②第 1 条树帧 == 同 wt 的 HTTP 树（条数 + id/path 逐项）③一次 UpTree 恰好 2 条树帧 ④`/.mds/mounts/**` 过滤非空转 + 调用点源断言 ⑤换一棵非 genesis 的 wt 复跑 ②。帧的定位**只按到达顺序**，不按内容挑帧（「含不含 ui.md 就选哪条」那类内容嗅探规则业主已否决）。
- **切换（应用改吃树帧）的前提在引擎侧**：`mdio_core/src/store.rs:1584` 要改成按连接 wt + hidden 出树。责任方 = 引擎改动者；届时 #2 与 #1 同为连接 wt 的树（判据 ③ 的那条读数会跟着变，它只播报、不判红）。若同时把「树来自 HTTP」也改掉，`entry.client.ts:120-121` 与本判据 ④ 项要一起显式改 —— 树源变更必须由改的人更新本节与判据，不能自动通过。

## 第三方接入（两种形态）

1. **包形态**（大而全）：`<wt>/ui/<mod>/package.json`（含 `esmx.exports` + `uses:["ui_base"]`）⇒ 在树里跑一次构建 ⇒ 宿主 `sync-wt-uses` 建条目；声明写**逻辑名** `- NAME: <包名>/<导出>`。
2. **单 js 文件**（不大而全）：把**一个可直接执行的 ESM 文件**放进 `<wt>/ui/`（无 `package.json`）⇒ 声明写 `- NAME: .ui::<文件名>`。客户端按**资产路由**（`/ui-assets/<wt>/<文件>`）装载它，里面的 `react`/`mobx` 由页面的 **import map** 解析 ⇒ **零配置**。
   边界：该文件必须是**浏览器可直接执行**的 ESM（不含 JSX/TS 语法）。
   演练：`node scripts/rehearse-onboarding.mjs`（自建 → 验证 → 自清）。
## 新树接入（从零建一棵树；第 7 步是**唯一的非零配置项**）

| # | 步骤 | 谁做 |
|---|---|---|
| 1 | 写 `<tree>/wt/.mds/ui.md`（含 `## mods` 声明） | 人手一次 |
| 2 | `<tree>/ui/package.json` + 在**该树 `ui/`** 下 `pnpm install`（得到 react/react-dom/katex/…） | 人手一次（**这一步尚未被演练验证**：`rehearse-new-tree` 暂用「从 genesis 借 react/react-dom」代替，见 PLAN §60.2-B2） |
| 3 | `<tree>/ui/node_modules/mdio_ui` → `mdio_ui/lib/mods/ui_base` | **自动**：`ensure-tree-deps.mjs`（`up-all` 第 ①b 步） |
| 4 | 放 mod 包（`<tree>/ui/<mod>/`：`package.json` 含 `esmx.exports` + `uses:["ui_base"]`） | 人手 |
| 5 | **登记**：`<rc>/wt/.mds/worktrees.md` 加一行 `- path: <tree>`（不登记 ⇒ 宿主看不见） | 人手一行 |
| 6 | `build-wt-ui`（先）+ `sync-wt-uses`（后） | **自动**：`up-all` / `restart-host` 已带预构建与同步（A1 修正后 `restart-host` 真的调了 sync） |
| 7 | **把树纳入网关挂载**（或在挂载根之内） | **一行配置**（必要）：未挂载 ⇒ 网关 `roots` 为空、`/ui-assets/<tree>/…` 404 ⇒ 浏览器装载不了它的 ui 件 |

演练（建树 → 登记 → 依赖落点 → 构建 → sync → 边界实证 → 清场）：`node scripts/rehearse-new-tree.mjs`，或 `verify-all --with-tree`。

## lab 探针（非操作入口，留作复现实验）

- `vite-plugin-mod-watch.mjs` + `mod-watch-probe.mjs` —— 早期"给 esmx 构建补 watch/增量"的探针（结论见 PLAN §22 与 SURVEY 第二十三～二十五轮）；日常不需要跑。

## 三个观测点

- `.tmp/host.log`（宿主日志：`[autoBuild]` · `[wtModules]` · `[W_NO_RANGE]` 等诊断）· `.tmp/gateway.log`（网关，若由 up-all 起）
- `<pkg>/.esmx-build.json`（戳：`ok` / `source` / `sourceHash` / `lockHash`）
- `<pkg>/dist.gen*`（代际目录；`dist` 是指向最新代的**符号链接**）

## 生产侧

```bash
node_modules/.bin/esmx build && node_modules/.bin/esmx preview
# 页面地址：http://127.0.0.1:5178/demo/?wt=<宿主挂了哪棵树>
# 入口路径无所谓（2026-10-01 实测：/ 、/demo/ 、/demo 三者都正常 boot 到 boot: ready，文件 120 条，无报错）
# —— 此前记的「根路径不启动、健康检查必须用 /demo/」**未复现**，按实测改写。
# **真正决定能不能起来的是 ?wt=**：必须与**宿主挂载的那棵树**一致（当前 = genesis）。
#   实测：?wt=exp_pages / ?wt=exp_swap / ?wt=nosuchwt ⇒ 都不 boot，页面显示「换树」说明（看起来像一直装载不出来）；
#         缺省 wt 或 ?wt= 空 ⇒ 回落到 genesis，正常 boot。
#   换树：MDIO_VFS_MOUNTS=<仓库>/worktrees/<wt>（exp_* 树为 <仓库>/exp_*）后跑 node mdio_ui/demo/scripts/up-all.mjs，再刷新。
```