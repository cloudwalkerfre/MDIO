"""UI 资产路由 —— 按 wt 从 **ui 作用域链**（`[自己, 父, …]` 各份 ui 目录的并集）发实现文件。

设计要点（来自 exp1 实测）：
- 资产**不进 VFS**（`FsBackend::load` 用 `is_md_file` 过滤），所以直接从 external_root 读盘；
- 客户端必须绕开打包器（Vite 会给动态 import 加 `?import` 并 500）⇒ 本路由只发**裸源码**；
- 除源码外也发静态资源（字体、图片）：扩展名白名单放行，content-type 由 `mimetypes` 决定；
- 安全边界：禁止 `..` 与点开头段、扩展名白名单、大小上限、路径必须落在链上某一根之下；
- 查找面 = `ui_dirs_for_wt` 给的根链（子覆父）：本根缺件才落下一根，全链都没有 → 404。
"""
from __future__ import annotations

import hashlib
import mimetypes
from pathlib import Path

from litestar import Controller, get
from litestar.di import Provide
from litestar.response import Response

from mdio_py import MdioRuntime
from mdio_py.gateway.state import provide_runtime

from mdio_py.runtime.worktree_paths import ui_dirs_for_wt, _local_scope_roots
from mdio_py.gateway.config import RUNTIME_ROOT

ALLOWED_EXT = {".js", ".mjs", ".css", ".json", ".map", ".svg", ".txt", ".wasm",
               # 字体（wf 资产的一部分）：`exp_scripts` 这个 wt app 要把剧本字体（Courier Prime）
               # 作为 wt 资产供出 —— 白名单里没有字体扩展名时取件回 415/`bad_ext`。
               ".ttf", ".otf", ".woff", ".woff2",
               # 图片（文档插图）：图放在 `<项目根>/ui/` 下即可经本路由取件，content-type 由
               # `mimetypes` 给出；注意查找面只有 ui 树，**文档同目录**的图（`<项目根>/wt/…`）
               # 不在这里，需要另做文档资产路由。
               ".png", ".jpg", ".jpeg", ".webp", ".gif", ".avif"}
MAX_BYTES = 4 * 1024 * 1024


class UiAssetsController(Controller):
    path = "/ui-assets"
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @get("/{wt_uid:str}/{path:path}")
    async def asset(self, runtime: MdioRuntime, wt_uid: str, path: str) -> Response:
        # list_worktrees() 元组 = (uid, name, storage_root(VFS 挂载别名), project_root)：
        # **具名解包**，避免加字段时下标错位。
        projects = {}
        for uid, _name, _vfs_alias, project_root in runtime.store.list_worktrees():
            if project_root:
                projects[uid] = project_root
        project_root = projects.get(wt_uid)
        if not project_root:
            # 快路径（store 里有挂载过的 wt）没命中 ⇒ 盘上顺着 worktrees.md 逐级找。
            # 为什么必须有这条：project_root 只在 wt 被挂载过之后才有值，而取件发生在挂载**之前**
            # （客户端切 wt：先装载声明与资产，再开会话）。`..某个wt.ui::` 的解析也依赖这条。
            project_root = _local_scope_roots(RUNTIME_ROOT).get(wt_uid)
        if not project_root:
            return Response({"error": "unknown_wt", "wt_uid": wt_uid}, status_code=404)
        # 查找面 = **ui 作用域根链**（[自己, 父, 祖父, …]；子覆父，见 ui_dirs_for_wt）。
        # 取件位置 `.ui::<作用域内路径>` 里的路径逐根解析；作用域内是一棵平铺的树（可带目录）。
        roots = ui_dirs_for_wt(wt_uid, project_root, runtime.store.parent_uid)
        rel = path.lstrip("/")            # Litestar 的 {path:path} 会带前导斜杠
        if not rel:
            return Response({"error": "no_path"}, status_code=404)
        # 形状检查与「哪一根」无关，先行一次：点段（含 `..`）与白名单外扩展名在任何一根上都一样非法，
        # 所以 403/415 语义收在这里统一判定。
        rel_parts = [p for p in rel.split("/") if p]
        if not rel_parts or any(p.startswith(".") for p in rel_parts):
            return Response({"error": "forbidden"}, status_code=403)
        suffix = Path(rel_parts[-1]).suffix
        if suffix.lower() not in ALLOWED_EXT:
            return Response({"error": "bad_ext", "suffix": suffix}, status_code=415)
        for base in roots:
            root = Path(base).resolve()
            target = (root / rel).resolve()
            if root != target and root not in target.parents:
                # 越界（软链接逃逸等）是非法路径，不是「本根缺件」⇒ 403，不靠下一根洗白。
                return Response({"error": "forbidden"}, status_code=403)
            if not target.is_file():
                continue                  # 本根下确实没有这件 ⇒ 落下一根（并集/继承即这一步）
            data = target.read_bytes()
            if len(data) > MAX_BYTES:
                return Response({"error": "too_large"}, status_code=413)
            ctype = mimetypes.guess_type(str(target))[0] or "application/octet-stream"
            etag = hashlib.sha256(data).hexdigest()[:16]
            return Response(
                content=data,
                media_type=ctype,
                headers={
                    "ETag": '"%s"' % etag,
                    "Cache-Control": "no-cache",
                    "Cross-Origin-Resource-Policy": "cross-origin",
                },
            )
        return Response({"error": "not_found", "path": path}, status_code=404)
