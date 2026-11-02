from litestar import Controller, get, post
from litestar.di import Provide
from litestar.response import Response
from mdio_py.gateway.state import provide_runtime
from mdio_py import MdioRuntime

class FileSystemController(Controller):
    path = "/fs"
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @get("/tree")
    async def get_file_tree(self, runtime: MdioRuntime, wt_uid: str = "genesis", hidden: bool = False) -> Response:
        """
        获取 VFS 文件资源树
        :param wt_uid: worktree uid 字符串，默认为 "genesis"
        :param hidden: 是否包含隐藏文件（.mds 等点前缀路径）——开发者模式文件树用
        """
        try:
            tree_bytes = runtime.store.get_vfs_tree(wt_uid, hidden)
            return Response(tree_bytes, media_type="application/json")
        except Exception as e:
            return Response({"error": str(e)}, status_code=500)

    @get("/dirs")
    async def list_dirs(self, path: str = "") -> Response:
        """
        列**宿主**目录下的子目录——供「从 fs 选项目文件夹」用（worktree load 要的是本地绝对路径，
        而浏览器出于安全不给本地路径，没有这个端点就只能手打路径，做不成真选择）。
        只读、只列目录、忽略点开头的项；path 缺省取 ~。
        """
        import os
        base = os.path.realpath(os.path.expanduser(path or "~"))
        if not os.path.isdir(base):
            return Response({"error": f"not a directory: {base}"}, status_code=400)
        try:
            names = sorted(os.listdir(base))
        except Exception as e:
            return Response({"error": str(e)}, status_code=400)
        dirs = [
            {"name": n, "path": os.path.join(base, n)}
            for n in names
            if not n.startswith(".") and os.path.isdir(os.path.join(base, n))
        ]
        parent = os.path.dirname(base.rstrip("/")) or "/"
        return Response({"path": base, "parent": parent, "dirs": dirs})

    @post("/create")
    async def create_file(
        self, 
        runtime: MdioRuntime, 
        path: str, 
        wt_uid: str = "genesis"
    ) -> Response:
        """
        新建文件
        :param path: 绝对虚拟路径，例如 '/notes/todo.md'
        :param wt_uid: worktree uid 字符串，默认为 "genesis"
        """
        safe_path = path if path.startswith("/") else f"/{path}"
        try:
            if safe_path.endswith("/"):
                return Response(
                    {"status": "ignored", "msg": "VFS dir is implicit"}, 
                    media_type="application/json"
                )
            # 利用空写入触发文件创建
            await runtime.exec(
                f'"" | +> {safe_path}::',
                active_doc="/", 
                wt_uid=wt_uid
            )
            return Response({"status": "ok", "path": safe_path}, media_type="application/json")
        except Exception as e:
            return Response({"error": str(e)}, status_code=500)