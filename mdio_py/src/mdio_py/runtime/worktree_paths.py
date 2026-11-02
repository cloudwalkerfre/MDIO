"""worktree 外部坐标换算的**唯一入口** —— mdio 的「坐标词典」也在这里。

四套空间（别混用）

  ① OS 路径空间（盘上）
       mount_config（启动配置 / MDIO_VFS_MOUNTS）  "path:alias:mode[,path:alias:mode…]"
         └ 首段 path = genesis 的盘上根
       project_root（参数；= worktrees.md 的 path 字段）  ← **用户项目根 = 身份坐标**
         （最长前缀匹配用它：Workspace.match_worktree）
       external_root（core 侧）  ← **该 wt 的盘上根**（core 建 fs_op / offload 回邮）
         = project_root/wt
       state_root（core 侧）  ← **运行态根**（WAL 等）
         未设置时 WAL 在 external_root 下
  ② VFS 地址空间（文档）
       storage_root（core 的挂载别名）  genesis "/" · 项目 "/.mds/mounts/mnt_<uid>/"
       地址（/weather.md、/.mds/ui.md）都以它为根 —— **与盘上层级无关**
  ③ spec 空间（实现）  .ui::<作用域内路径> · npm:… · builtin:… · doc:… · md:…  —— 由「ui 作用域根」解析
  ④ HTTP 资产前缀      /ui-assets/<wt_uid>/<作用域内路径>  —— 由 ui 作用域根换算

命名纪律（可 grep 守）
  · 运行时自己的家叫 runtime_root()（mdio_py._env），**不要**再叫 project_root —— 那个名字留给「用户项目」
  · "wt" / "plugin" / ".wal" 这些层级名只准出现在本文件；消费方拿「根」或换算结果，不准自己拼串
  · 布局**恒定**：文档 `<根>/wt`、实现 `<根>/plugin`、运行态 `<根>/.wal`；**没有探测、没有扁平兜底**——
    缺 `wt/` 时由 core 报错，不静默退回扁平

"""
import os

from mdio_py import _env


def is_anchor_dir(path: str) -> bool:
    """锚点判定：path 属于**mdio 自己的空间**（⇒ 不当作可 load 的项目）。

    锚点判定规则：
      · 地盘（home，如 ~/.mdio）本身、以及它的父目录  → 自家
      · **默认 genesis**（runtime_root()）及其子目录（~/​.mdio/genesis/wt/…）→ 自家（它已是根挂载点，别重复 load）
      · 地盘下**其它**目录（如 ~/.mdio/demo、~/.mdio/mywts/foo）→ **不是自家，是可加载的项目**
    """
    home = os.path.realpath(str(_env.mdio_home()))
    gen = os.path.realpath(str(_env.runtime_root()))
    rp = os.path.realpath(path)
    if rp == home or rp == os.path.dirname(home):
        return True
    return rp == gen or rp.startswith(gen.rstrip("/") + "/")


# ── 三根换算（唯一布局：文档 `<根>/wt`、实现 `<根>/plugin`、运行态 `<根>/.wal`）──────
# 没有探测、没有兜底：根由调用方给的路径决定，布局恒定。缺 `wt/` 即"未迁移"，不静默降级。


def doc_root_for(project_root: str) -> str:
    """项目根 → **文档根**（= core 的挂载根 / VFS 泵入源）：恒 `<项目根>/wt`。"""
    return os.path.join(os.path.realpath(project_root), "wt")


def plugin_root_for(project_root: str) -> str:
    """项目根 → **实现根**（spec `./ui/x.js` 的解析根；`/ui-assets/` 的 base）：恒 `<项目根>/plugin`。"""
    return os.path.join(os.path.realpath(project_root), "plugin")


def ui_scope_root_for(project_root: str) -> str:
    """项目根 → **ui 作用域根**（取件位置 `.ui::<作用域内路径>` 的解析根；`/ui-assets/` 的 base）。

    **ui 与 wt 并列在项目根下**：`<项目根>/ui`——不分层/分层同形；
    分层时文档在 `<项目根>/wt`、实现在 `<项目根>/ui`，ui 不是 wt 的子目录。
    **根改无点**：实现树是普通一层，声明里的 `.ui::` 是**作用域名**，与盘上目录名是否带点无关。
    单个 wt 的取件面 = **父链并集**，见 `ui_dirs_for_wt`（子覆父）。
    """
    return os.path.join(os.path.realpath(project_root), "ui")


def ui_dirs_for_wt(wt_uid: str, project_root: str, parent_of) -> list:
    """wt uid → **ui 作用域根链** `[自己, 父, 祖父, …]`（子覆父的查找顺序）。

    ui 继承 = 沿 parent 链取各 wt 的 ui 目录做**并集**（不做 md 的 bind/local/remote 拓扑）：链首是
    本 wt 自己的根，往后是父、祖父……取件语义「本根没有才落下一根」，所以顺序 = 优先级（子覆父）。
    链来自 WtTree（`parent_of` = store.parent_uid，既有 py 绑定）；祖先的项目根来自盘面扫描
    `_local_scope_roots`（worktrees.md 全树 uid→项目根）。链的终点 = 无父的那个 wt = 根 wt，
    它的根 = `MDIO_WORKSPACE_ROOT`（运行时根的覆盖口）。唯一解析入口是
    `_env.runtime_root()`：它读环境变量、展开 ~、把相对值锚定到地盘；
    不要在消费方直接读 os.environ，否则相对路径会随进程 cwd 漂移。
    盘面扫描只收「子」条，根 wt 自己不在 worktrees.md 里。
    """
    roots = [ui_scope_root_for(project_root)]
    seen = {wt_uid}
    # 唯一权威：_env.runtime_root() 已读 MDIO_WORKSPACE_ROOT，并把相对值锚定到地盘。
    gen = str(_env.runtime_root())
    disk = _local_scope_roots(gen)
    cur = parent_of(wt_uid)
    while cur and cur not in seen and len(roots) < 8:
        seen.add(cur)
        nxt = parent_of(cur)
        proot = disk.get(cur) or (gen if not nxt else None)
        if proot:
            roots.append(ui_scope_root_for(proot))
        cur = nxt
    return roots


def _local_scope_roots(start_project_root: str) -> dict:
    """盘上顺着 worktrees.md 逐级收 uid → 项目根（本地 wt 才有；远端挂载仍走 store 那条路）。

    为什么要有它：`store.list_worktrees()` 的 project_root 只在 wt 被挂载过之后才有值，
    而资产路由要能在**任何**已登记 wt 上发件（客户端切过去时先取件、后挂载）。这条路只读盘：
    根 wt 的根 = runtime_root()（由调用方给），每级 `<文档根>/.mds/worktrees.md` 里 `- uid:` + `- path:`。
    本文件只做坐标换算（uid → 项目根），不是路由逻辑。
    """
    out: dict = {}

    def visit(project_root: str, depth: int) -> None:
        if depth > 8 or not project_root:
            return
        md = os.path.join(doc_root_for(project_root), ".mds", "worktrees.md")
        try:
            with open(md, encoding="utf-8") as f:
                text = f.read()
        except OSError:
            return
        uid = None
        path = None
        for line in text.split("\n"):
            if line.startswith("## "):
                if uid and path:
                    out[uid] = path
                    visit(path, depth + 1)
                uid = None
                path = None
                continue
            if line.startswith("- uid:"):
                uid = line.split(":", 1)[1].strip()
            elif line.startswith("- path:"):
                path = line.split(":", 1)[1].strip()
        if uid and path:
            out[uid] = path
            visit(path, depth + 1)

    visit(start_project_root, 0)
    return out


def doc_subdir_for(project_root: str):
    """项目根 → **文档子目录名**（core 把 `fs_op` 挂到 `<项目根>/<这个>`）：恒 `"wt"`。"""
    return "wt"


def wal_rel_for(project_root: str):
    """项目根 → **WAL 相对目录**（相对项目根）：恒 `".wal"`（与 wt/、plugin/ 并列）。"""
    return ".wal"


def project_root_for_wt(store, wt_uid: str):
    """wt → 用户项目根（external_root 的最近祖先；genesis 回 runtime_root）。"""
    if store is None:
        return None
    try:
        entries = list(store.list_worktrees())
    except Exception:
        entries = []
    by_uid = {uid: external_root for uid, _name, _alias, external_root in entries}
    root = by_uid.get(str(wt_uid))
    if root:
        return os.path.realpath(str(root))
    cur = str(wt_uid)
    seen = set()
    while cur and cur not in seen:
        seen.add(cur)
        try:
            cur = store.parent_uid(cur) or ""
        except Exception:
            cur = ""
        if not cur:
            break
        root = by_uid.get(cur)
        if root:
            return os.path.realpath(str(root))
    fallback = os.path.realpath(str(_env.runtime_root()))
    return fallback if os.path.isdir(fallback) else None


def state_root_for(project_root: str):
    """项目根 → **运行态根**（WAL 等的基）；返回 None = WAL 由 core 按 wal_rel 计算。

    返回 None 时 WAL 在 external_root 下，由 core 按 wal_rel 计算 `.wal`。
    """
    return None
