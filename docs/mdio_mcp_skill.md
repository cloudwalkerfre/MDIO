---
name: mdio
description: Enable you to CRUD over Markdown documents. Use whenever the task involves Markdown — reading, searching, writing, or reorganizing .md content.
---

# MDIO 基础

## MDIO 在 VFS 上，不在磁盘上

MDIO 把 Markdown 文档放在自己的 **VFS（worktree 文档库）**里。选择器里的 `/dir/file.md` 是 **MDIO 虚拟路径**，不是本机文件路径；所有读写都走 `mcp__mdio__exec`，落盘由 MDIO 自己负责。

```mermaid
flowchart LR
  A["LLM / agent"] -->|mcp__mdio__exec · cwd=项目根| B["MDIO runtime / gateway"]
  B --> C["VFS 文档库<br/>选择器路径 /dir/file.md::..."]
  C -.->|自动落盘 · 快照| D["磁盘：项目根/.mdio/"]
  A -.->|不直接读写磁盘| D
  classDef sys fill:#0f1b2d,stroke:#1f4e79,color:#bfdbfe
  classDef vfs fill:#1b1430,stroke:#7c3aed,color:#ede9fe
  classDef disk fill:#111827,stroke:#4b5563,color:#d1d5db
  class B sys
  class C vfs
  class D disk
```

**接口要点**

- 调用 `mcp__mdio__exec` 必须带 `cwd`，值为**你所在项目的根**；
- mdio 的 wt 在项目下的 `.mdio/` 里，文档在库内；VFS 路径与工作区路径不是一回事；
- **不要**直接改 `.mdio/` 下的文件，也不要拿普通 fs 工具去读写它们（会绕过 VFS 会话/索引）；
- 读、写、删都通过 `mcp__mdio__exec`；写入后读回验证。

## 速查

- 不知道目录里有什么
  > ls -l /dir/
  > ls -R /dir
- 想一次看所有标题
  > $ /dir/file.md::..*[h] -pj tag
- 想读整篇原文
  > $ /dir/file.md::
- 想读某个值
  > $ /dir/file.md::节.子节:键
- 想给任务节追加一条待办
  > "- 待办: 补测试\n" | +> /dir/project.md::任务 -pj raw
- 想跨文档搜索内容
  > arrow -duckdb "select path, node, content from blocks where content like '%TODO%' limit 5" -rows
- 想看某个目录的章节树
  > arrow -duckdb "select path, section, parent_section, level, heading from sections where path like '/dir/%' order by path, start_line" -rows
- 想按标题找文档
  > arrow -duckdb "select path, section, heading, node from sections where heading like '%配置%' limit 5" -rows

## §0 概念框架

MDIO 将 Markdown 投影为 `HS + proj`。

- **HS：header section 拓扑**
  - header 是节；非 header block 归它前面最近的 header；header 可嵌套。
  > ```text
  > # 项目                         [head h1]
  > ├─ ## 配置                     [head h2]
  > │  ├─ - 端口: 8080             [bullet]
  > │  ├─ > 本地配置               [bq]
  > │  ├─ 1. 先启动服务            [ol]
  > │  ├─ 普通段落文本              [p]
  > │  ├─ | 列 | 值 |              [tb]
  > │  └─ <!-- 备注 -->            [comment]
  > ├─ ## 任务                     [head h2]
  > │  ├─ - 待办: 写文档           [bullet]
  > │  ├─ ```python                [code]
  > │  └─ ### 子任务               [head h3]
  > │     └─ - 子项: 测试          [bullet]
  > └─ ## 日志                     [head h2]
  >    └─ > 记录一                 [bq]
  > ```
  - 演示
    > $ /demo.md::..*[h] -pj tag
    > arrow -duckdb "select path, section, parent_section, level, heading from sections where path = '/demo.md' order by start_line" -rows
- **proj：每个 block 的五面**
  > raw = container[ mark + tag ] + content
  >
  > 以 `- 端口: 8080` 为例：
  > raw = "- 端口: 8080"
  >  ├─ container = "- 端口: "
  >  │    ├─ mark = "-"
  >  │    └─ tag  = "端口"
  >  └─ content = "8080"
  >
  > container 是 raw 的前缀段，包住 mark+tag；content 紧接在 container 后，不是左右并列。
  - 五种投影（同一条 bullet）
    > $ /demo.md::项目.配置.*[b.t=="端口"] -pj raw  → - 端口: 8080
    > $ /demo.md::项目.配置.*[b.t=="端口"] -pj ctn  → - 端口: 
    > $ /demo.md::项目.配置.*[b.t=="端口"] -pj m    → -
    > $ /demo.md::项目.配置.*[b.t=="端口"] -pj tag  → 端口
    > $ /demo.md::项目.配置.*[b.t=="端口"] -pj ct   → 8080
- 别名：`r` raw、`ctn` container、`m` mark、`t` tag、`ct` content。

## 选择器

选择器 = **路径 + block_type + 投影（-pj）**

- **路径**
  - `.NAME`：钻取，取当前层 tag=NAME 的子节点（例 `项目.配置`）
  - `.*`：当前层全部子节点
  - `..NAME`：递归钻取，全部后代中 tag=NAME 的节点
  - `..*`：递归通配，全部后代
  - `:tag`：标识匹配，返回匹配节点的 content/value
  - `[filter]`：过滤 / 切片，作用在当前集合
- 路径演示
  > $ /demo.md::项目.配置 -pj raw
  > $ /demo.md::项目.配置.* -pj tag
  > $ /demo.md::..*[h] -pj tag
  > $ /demo.md::项目.配置:端口
- **跨文件寻址**：两段式 `文件地址::文档内选择器`。
  > $ /demo.md::项目.配置:端口
  > $ /demo.md::
- **类型（全写 / 简写）× 投影**（`raw = container + content`）

  | 类型 | 全写 | 简写 / 别名 | raw | container | mark | tag | content | 可写层 |
  |---|---|---|---|---|---|---|---|---|
  | head | `[head]` | `[h]`、`[h1]`–`[h6]`、`[head1]`–`[head6]` | 标题 + 节体 | `## 标题` | `#`–`######` | 标题文本 | 节体 | tag / content |
  | bullet | `[bullet]` | `[b]`、`[blt]` | `- KEY: value` | `- KEY: ` | `-` | KEY | value | tag / content |
  | code | `[code]` | `[c]`、`[codeblock]` | fence + 代码体 | fence 行 | 三个反引号 | 语言 | 代码体 | tag / content |
  | bq | `[bq]` | `[blockquote]` | `> 文本` | `> ` | `>` | 空 | 引用文本 | content |
  | ol | `[ol]` | `[o]`、`[orderlist]` | `N. item` | `N. ` | `N.` | 编号 | item | tag / content |
  | tb | `[tb]` | `[table]` | 表格行 | 竖线 | 竖线 | 空 | 单元格文本 | content |
  | comment | `[comment]` | 无 | `<!-- 注释 -->` | `<!--` | `<!--` | 空 | 注释体 | content |
  | p | `[p]` | `[paragraph]` | 正文 | 无 | 无 | 空 | 正文 | content |

- 类型演示
  > $ /demo.md::..*[h1] -pj tag
  > $ /demo.md::..*[bq] -pj raw
  > $ /demo.md::..*[ol] -pj raw
  > $ /demo.md::..*[comment] -pj raw
  > $ /demo.md::说明.*[p] -pj ct
  > $ /demo.md::..*[blt] -pj raw
  > $ /demo.md::..*[codeblock] -pj tag
  > $ /demo.md::..*[orderlist] -pj raw
  > $ /demo.md::..*[table] -pj ct

- **`_nt`：路径类型标记（选择器语法的一部分）**
  - 约定：`_` + 节点类型简写，**挂在路径段后面**；它不是一个独立的命令，也不是过滤条件，而是“路径本身只匹配这种节点”。
  - 常用简写：`_b` bullet、`_h` 标题（任意级别）、`_h1`–`_h6` 指定级别、`_c` code、`_bq` 引用、`_ol` 有序列表、`_tb` 表格、`_comment` 注释、`_p` 段落。
  - 位置规则：
    - `HEAD._b`：HEAD 的**直接** bullet 子节点；
    - `HEAD.._b`：HEAD 下**递归**所有 bullet；
    - `HEAD.._b:port`：递归 bullet 中 tag=port 的节点，读它的 value；
    - `.._b` 与 `..*_b` 等价；`.` 是直接子节点，`..` 是递归后代。
  - 为什么需要它：同名 tag 可能同时是标题、bullet、代码块；只用 `HEAD.NAME` 会一起命中，`_nt` 在**路径层**就把类型定死。
  - 和 filter 的关系：`_nt` 管“节点类型”，`[filter]` 管“节点内容”；可叠加，如 `.._b[b.ct.has("文档")]`。
  - 演示
    > $ /demo.md::项目._h -pj tag
    > $ /demo.md::项目.._b -pj raw
    > $ /demo.md::项目.._b:端口
    > $ /demo.md::..*_c -pj tag

- **过滤器**
  - 字段：`.tag` / `.content` / `.raw` / `.container` / `.mark`
  - 方法：`.has("text")` / `.re("regex")` / `.eq("value")` / `.empty`
  - 证据过滤：`.by[...]` 选“内容里包含某类子节点”的节点；必须挂在类型/字段后，裸 `.by[..]` 会报 P001。
    > $ /demo.md::..*[h2.by[tb]] -pj tag
    > $ /demo.md::..*[h2.by[blt]] -pj tag
  - 逻辑：`&&` / `||` / `!` / `()`
  > $ /demo.md::..*[c.t=="python"]
  > $ /demo.md::..*_b[b.ct.has("文档")] -pj raw
  > $ /demo.md::..*_b[b.tag.re("端口|环境")] -pj raw
  > $ /demo.md::..*_b[b.r.has("端口")] -pj raw
  > $ /demo.md::..*_b[b.ctn.has("-")] -pj raw
  > $ /demo.md::..*_b[b.m=="-"] -pj raw
  > $ /demo.md::..*_b[b.ct.eq("dev")] -pj raw
  > $ /demo.md::..*_b[b.ct.empty] -pj raw
  > $ /demo.md::..*_b[b.tag=="端口" || b.tag=="环境"]
  > $ /demo.md::..*_b[!(b.ct.empty)] -pj raw
  > $ /demo.md::..*_b[(b.tag=="端口" || b.tag=="环境") && !(b.ct.empty)]
  > $ /demo.md::..*_b[0] -pj raw
  > $ /demo.md::..*_b[0:2] -pj raw
  > $ /demo.md::..*_b[-1] -pj raw
  > $ /demo.md::..*_b[0:6:2] -pj raw
  > $ /demo.md::..*_b[1,3,5] -pj raw
  > $ /demo.md::..*_b[0:3, ::-1] -pj raw
  - 过滤器必须挂在集合后（`.*` / `..*`），不能裸写 `[filter]`。
  - 字段方法要带类型或字段：`[b.ct.has(...)]` ✅，`[.content.has(...)]` ❌（P001）；裸字段只支持 `.re`。
  - **过滤器后面不能再接 `..*`**：要再过滤就换起点（先钻到目标节，再 `.*[filter]`）。
  - `:tag` 是懒搜索（只取第一个命中）；要全部命中用 `..*_b[b.tag=="KEY"]`；`:tag` 永远最后。
- **组合**：`文件::路径 + 类型 + 过滤 + 投影`。

## 操作符

- `$` 读；无 `:tag` 时 raw，有 `:tag` 时 content。
- `$` 和 `$$` 等同：`-pj` 行为完全一致；`$$` 只对管道文本做内存级 Markdown 解析，不触 VFS。
- 管道读 `$$`
  > $VFS_FILE | $$ ..*[c.ct.re("fn main")]
  > $ /docs/log.md::ERRORS | $$ ..*[.ct.re("FATAL")]
- `+>` 追加；默认 content，追加到目标（节/条目）末尾。
- `/>` 覆盖；默认替换 content、保留 container；`-pj raw` 整段替换。
- `->` 删除；整节点删；`:key` 只清值、保留容器。
- `~>` 补丁；JSON 数组 `[{"old":"...","new":"..."}]`。
- 写标志：`-f` 自动接骨（默认 true）；`-l` 自动层级（仅 head）；`-pt` 流透传；`-god` 全自动创建；`-pj r` 写 raw。
- 写文件不存在先 `mk -f`；`+>` 默认落到节末尾，要插到锚后用 `-pj raw` + 锚点。
  > "9090" | /> /demo.md::项目.配置:端口
  > "- 新增: 测试\n" | +> /demo.md::项目.任务
  > "- 插入: 锚后\n" | +> /demo.md::项目.任务.*[b.t=="待办"] -pj raw
  > "\n## 新节\n- 新键: 新值" | +> /demo.md:: -pj raw
  > -> /demo.md::项目.配置:模式
  > [{"old":"9090","new":"7070"}] | ~> /demo.md::项目.配置:端口
- 标志 demo
  > "正文" | /> /demo.md::项目.新节
  > "正文" | /> /demo.md::项目.另一节 -f false
  > "\n## 新章\n- k: v" | +> /demo.md:: -l true
  > "\n# NewSection\n- item: 1" | +> /demo.md::项目.配置 -l  → ### NewSection
  > "# G" | +> /tmp/god.md:: -god
  > mk -f /tmp/pt_a.md ; mk -f /tmp/pt_b.md
  > "# P\n## 节\n- 键: old" | /> /tmp/pt_a.md:: -pj r
  > "# P\n## 节\n- 键: old" | /> /tmp/pt_b.md:: -pj r
  > "NEW" | /> /tmp/pt_a.md::P.节:键 -pt true | +> /tmp/pt_b.md::P.节:键

## 管道

- 概念：管道 `|` 是 **str in → str out**，左边输出作为右边输入。
- 基本 std：`rm` 删除、`mk` 创建、`ls` 列出、`fmt` 套格式。
- JSON：`tojson` 把 md 文本/选择器结果转 JSON，管道形式为 `$ ... | tojson`。
- 字面量：`"..."` / `'...'` 单行字符串，`"""..."""` 多行字符串，`r"""..."""` 原始多行字符串，数字。
- 占位符：`_` 用于 `calc` 等 std 函数引用管道数据；`fmt` 自己的占位符是 `{}`。
- 布尔：只能用于参数/过滤条件（如 `-f false`），不能单独当管道输入（会报 I404）。
- 串联：`;` 分隔多个独立单元，顺序执行；**任一单元失败即停止后续**，只返回最后一个单元的结果。
- 返回值：读是文本；写返回 `- mdio_success: ...`；失败 `- mdio_error: ...`。
- 示例
  > ls -l /demo.md | fmt "- {}"
  > $ /demo.md::项目.配置.* -pj tag | fmt "- {}"
  > "50" | calc -e "_ + 1"
  > "x" | fmt "prefix: {}"
  > 'single' | fmt "- {}"
  > """multi\nline""" | fmt "- {}"
  > r"""raw\n""" | fmt "- {}"
  > 42 | fmt "- {}"
  > $ /demo.md::项目.配置 -pj raw | tojson
  > mk -f /tmp/demo_copy.md ; $ /demo.md:: | /> /tmp/demo_copy.md:: -pj r
  > rm -f /tmp/demo_copy.md
  > $ /demo.md::项目.配置:端口 ; $ /demo.md::项目.任务:待办
- 长 exec：备份 → 修改 → 验证
  > $ /note.md::读库.概念:status | +> /backup/status.md:: ;
  > "refactor" | /> /note.md::读库.概念:status ;
  > $ /note.md::读库.概念:status
- 写后一发验证
  > "beta" | /> /d.md::读库.概念:status ; $ /d.md::读库.概念:status

## 示例

先 `mk -f /demo.md`，写入下面内容；以下读写命令会修改它，按顺序跑或先复制一份。

````md
# 项目

## 配置

- 端口: 8080
- 环境: dev
- 模式: local
- 空值:

## 任务

- 待办: 写文档
- 完成: false

### 子任务

- 子项: 测试

## 说明

1. 先启动服务
<!-- 备注 -->
普通段落文本

## 日志

> 记录一
> 记录二

```python
print("hi")
```

| 列A | 列B |
| --- | --- |
| a | 1 |
| b | 2 |
````

- 读
  - 所有标题
    > $ /demo.md::..*[h] -pj tag
  - 一个值
    > $ /demo.md::项目.配置:端口
  - 配置节原文
    > $ /demo.md::项目.配置 -pj raw
  - 过滤 bullet
    > $ /demo.md::..*_b[b.tag.re("端口|环境")] -pj raw
  - 代码 / 表格
    > $ /demo.md::..*[c] -pj tag
    > $ /demo.md::..*[tb] -pj ct
  - 全文
    > $ /demo.md::
- 写
  - 覆盖值
    > "9090" | /> /demo.md::项目.配置:端口
  - 末尾追加
    > "- 新增: 测试\n" | +> /demo.md::项目.任务
  - 锚后插入
    > "- 插入: 锚后\n" | +> /demo.md::项目.任务.*[b.t=="待办"] -pj raw
  - 根末尾新增一节
    > "\n## 新节\n- 新键: 新值" | +> /demo.md:: -pj raw
  - 删除 / 补丁
    > -> /demo.md::项目.配置:模式
    > [{"old":"9090","new":"7070"}] | ~> /demo.md::项目.配置:端口
- 管道
  - 格式化标签
    > $ /demo.md::项目.配置.* -pj tag | fmt "- {}"
  - 复制整篇
    > mk -f /tmp/demo_copy.md ; $ /demo.md:: | /> /tmp/demo_copy.md:: -pj r

## 注意事项

- 文件名含 `-`、空格等特殊字符要加引号：`/tmp/name-with-dash.md` 不加引号会被截成 `/tmp/name.md`；`$ "/tmp/name with space.md"::` 正常。
- 路径中的 `.md` 扩展名可省略：`/path/file::` 等价于 `/path/file.md::`。
- 过滤器和切片可链式组合：`..*[o][0:2]` 表示先取有序列表，再取前两项。
- 写操作不建文件：先 `mk -f`；`-f` 只接骨，不创建不存在的文件。
- 过滤器后面不能再接 `..*`；`:tag` 永远最后；`:tag` 读值忽略 `-pj`。
- 裸字段方法不合法：`[b.ct.has(...)]` ✅，`[.content.has(...)]` ❌。
- 读取输出会对**完全相同的投影行去重**；`raw` 不去重，看命中数用 `raw`。
- `nodes` 一次返回多个投影面会报 `ARROW_INVALID_RESULT`；多面用 `pj` 或拆查询。
- FTS/search 只索引内容 value，不索引 tag/key；按 tag 找用 `nodes` / `pj`。
- 错误看 `- mdio_error:` 前缀（P001 / I500 / R201）；读到不存在的内容可能返回空，不一定是错误。
- 写后必须 `$` 读回：`mdio_success` 只表示管道跑完。

## arrow

arrow 是 MDIO 的 SQL 阶段：把已选文档变成关系，交给 DuckDB 查询，默认把命中的 node 渲染回 md。

- 单文档
  > $ /demo.md:: -pj raw | arrow -duckdb "select path, node, section, type from blocks where type = 'bullet' limit 3"
- 多文档（docs 关系选文档）
  > arrow -duckdb "
  >   with doc as (select path from docs where path = '/demo.md')
  >   select b.path, b.node, b.section
  >   from blocks b join doc on doc.path = b.path
  >   where b.type = 'bullet'
  >   limit 3
  > "
- 视图：`docs` 文档、`sections` HS、`blocks` 语义块、`nodes` 全投影、`pj` 五面、`children` 父子、`span` 区间、`outline` 轻量章节、`section_member` 节点归属、`meta` 元信息。
- 规则：结果必须包含 `node`（多文档还要 `path`）；默认 md 输出选中的节点；聚合也要带 node/path。
- 输出开关：`-rows` 行表，`-json` JSON；默认 md。
  > arrow -duckdb "select path, section, heading, node from sections where path = '/demo.md' limit 3" -json
- 其余视图 demo
  > arrow -duckdb "select path, section, level, heading from outline where path = '/demo.md' order by start_line" -rows
  > arrow -duckdb "select path, node, section from section_member where path = '/demo.md' order by node limit 6" -rows
  > arrow -duckdb "select path, node, start_line, end_line from span where path = '/demo.md' limit 3" -rows
  > arrow -duckdb "select * from meta" -rows
  > arrow -duckdb "select path, node, type, text from search.blocks where path = '/demo.md' limit 5" -rows
- find / tree 平替
  - 文档清单
    > arrow -duckdb "select path, folder, name, stem from docs where path like '/dir/%' order by path" -rows
  - 章节树
    > arrow -duckdb "select path, section, parent_section, level, heading, start_line, end_line, node from sections where path like '/dir/%' order by path, start_line" -rows
  - 直接父子边
    > arrow -duckdb "select path, parent, child, pos from children where path like '/dir/%' order by path, parent, pos" -rows
  - 按内容找 block
    > arrow -duckdb "select path, node, section, content from blocks where path like '/dir/%' and content like '%TODO%'" -rows
  - 按标题找节
    > arrow -duckdb "select path, section, level, heading, node from sections where path like '/dir/%' and heading like '%配置%'" -rows
  - 按 tag 找节点
    > arrow -duckdb "select path, node from nodes where path like '/dir/%' and tag = '端口'" -rows
    > arrow -duckdb "select path, node, value from pj where path like '/dir/%' and pj = 'tag' and value = '端口'" -rows
  - 叶子 section
    > arrow -duckdb "select s.path, s.section, s.heading, s.node from sections s where s.path like '/dir/%' and not exists (select 1 from sections c where c.path = s.path and c.parent_section = s.section)" -rows
  - 统计 / 排名
    > arrow -duckdb "select section, any_value(node) as node, count(*) as n from blocks where path = '/demo.md' group by section order by n desc" -rows
  - 全文搜索
    > arrow -duckdb "select path, node, score from search.hits('写文档') where path like '/dir/%'" -rows
    > arrow -duckdb "select path, section, node, score, matched from search.results('写文档') where path like '/dir/%'" -rows
- 边界：FTS 只索引内容 value，不索引 tag/key；按 tag 用 `nodes` / `pj`；pipe 模式搜索 path 是 `input`；`nodes` 一次返回多个投影面会报 `ARROW_INVALID_RESULT`。

## 高级用法

- 问题：在多级节里找配置，不想手写完整路径
  - 讲解：`..NAME` 递归钻取，再按类型/过滤收窄
  - 解决
    > $ /demo.md::..配置 -pj raw
    > $ /demo.md::..*[h2] -pj tag
    > $ /demo.md::..*_b[b.ct.has("测试")] -pj raw
- 问题：把一组读出的标签加工后写回
  - 讲解：管道 `fmt` 改文本，`/>` 写回；`;` 串多个单元
  - 解决
    > mk -f /tmp/labels.md ; $ /demo.md::项目.配置.* -pj tag | fmt "- {}" | /> /tmp/labels.md:: -pj r ; $ /tmp/labels.md::
- 问题：跨文档统计 block 最多的 section
  - 讲解：`blocks` 分组，结果保留 `node`
  - 解决
    > arrow -duckdb "select section, any_value(node) as node, count(*) as n from blocks group by section order by n desc limit 3" -rows

## 常见错误

- `:tag` 后接 `[0]` → `[P001]`；正确：`项目.配置.*[b.t=="端口"][0]` 或先取集合。
- filter 后接 `..*` → `[P001]`；正确：先钻到目标节，再 `.*[filter]`。
- `[.content.has("x")]` → `[P001]`；正确：`[b.ct.has("x")]`。
- `+> /no_such.md::` → `[I500]`；正确：先 `mk -f /no_such.md`。
- 读不存在的节可能是空串，不是报错；先 `$ /demo.md::..*[h]` 确认节名。
- 文件名不加引号导致 `-` 被截断；正确：`"/tmp/name-with-dash.md"`。
- 用 `:端口 -pj raw` 想拿原文 → `:tag` 是 value-target；正确：`.*[b.t=="端口"] -pj raw`。
- 看到 `mdio_success` 就结束 → 必须再 `$` 读回目标。
- `nodes` 一次返回多个投影面 → `ARROW_INVALID_RESULT`；改用 `pj` 或分开查。

## 最佳实践

- 问题：改一个值，不想动容器
  - 讲解：`/>` 默认只换 content；`-pj raw` 是整段替换
  - 解决
    > "9090" | /> /demo.md::项目.配置:端口
    > "9090" | /> /demo.md::项目.配置:端口 -pj raw
- 问题：在某个 bullet 后面插一行
  - 讲解：`+>` 默认落到节末尾；要插锚后必须用 `-pj raw` + 锚点
  - 解决
    > "- 插入: 锚后\n" | +> /demo.md::项目.任务.*[b.t=="待办"] -pj raw
- 问题：目标节不存在
  - 讲解：默认 `-f` 接骨；要明确失败用 `-f false`；文件不存在先 `mk -f`
  - 解决
    > "正文" | /> /demo.md::项目.新节
    > "正文" | /> /demo.md::项目.另一节 -f false
- 问题：写完怎么确认
  - 讲解：`mdio_success` 只代表管道完成；读回目标才算验收
  - 解决
    > "9090" | /> /demo.md::项目.配置:端口 ; $ /demo.md::项目.配置:端口
- 什么时候用 MDIO：文档即数据结构，读/写/检索/统计都在一棵树上完成。
- 什么时候用 arrow：选择器表达不了跨文档聚合、join、排名时。
- 什么时候别用：把 md 当 shell/数据库万能工具，或绕过 VFS 直接改磁盘。

---

# VFS

本章节为 mdio vfs 的文件夹约定：说明每个目录放什么、文件如何命名、文件开头怎么写，方便查阅和复用。

## ABOUT_<文件名>

- 每篇 md 开头放 `# ABOUT_<文件名>`。
- about 写清：本文件是什么、解决什么问题、where & when；计划类要写清任务上下文。
- 作用：打开文件先看到名片；也是 selector 查询的稳定入口。

## /DOC/

- 只放外部成熟包的文档（工具书）；一个包一篇 md。
- 只放成熟稳定的轮子，不放开发中的、随方案变化的内容（那属于 `/note/`）。
- 开头按 `# ABOUT_<文件名>` 写清轮子名称、版本、成文时间。
- 章节名按实际查询需求设计，便于 `$ /DOC/xxx.md::..*[h] -pj tag` 当目录用。
- 章节名含空格、dot 等特殊字符时用引号或 `[h.t=="..."]`。

## /DEV/

- 只放 plan md：动手前的 plan & spec。
- 文件名：`<主题>_plan_YYYYMMDD.md`，日期放最后，用 `_` 替代 `-` 和空格。
- 同一 task 内可迭代更新；跨 task 不回改旧 plan，保留当时状态（即使过时/错漏）。
- 开头 `# ABOUT_<文件名>`，写清 where & when、内容与章节组织，方便查阅。
- 知识在 `/DOC/`，研究在 `/research/`，进度在 `/note/`，计划在 `/DEV/`。

## /guide/

- 放 mdio 项目开发相关的上手 / 契约 / 操作指南。
- 开头 `# ABOUT_<文件名>`，about 写「本 md 主题、解决什么问题」。
- 章节名按 `/DOC/` 同样的原则设计，方便查询。

## /note/

- 只放 note md：重要记忆、进度记录；可流水账，不强求格式。
- 文件名：`<主题>_note_YYYYMMDD.md`，日期放最后。
- 开头 `# ABOUT_<文件名>`，写清本 note 是哪个 task 的要项记录。
- 建议做好章节标题，方便 selector 查阅。

