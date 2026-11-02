"""MDIO CLI — Click 入口。CLI 是运行中 runtime 的客户端。"""

import click


@click.group()
@click.version_option(package_name="mdio-py")
@click.option("--wt", "-w", type=str, default="genesis", help="默认 worktree（名或 uid）")
@click.pass_context
def main(ctx, wt):
    """MDIO — Markdown document shell & workbench.

    \b
    Usage:
      mdio serve                    # gateway + UI，前台常驻
      mdio web                      # 确保 gateway/UI 运行并打开浏览器
      mdio exec '$ /docs/a.md::HEAD._b:KEY | /> /docs/b.md::TARGET'
      mdio load ~/project1               # wt 级：本地目录或 repo → worktree
      mdio offload project1              # wt 级：按名称卸载 worktree
      mdio mount --ticket iroh://xxx     # folder 级：挂载远程 folder
      mdio unmount --path /share/xxx     # folder 级：断开远程 folder
      mdio mcp                           # stdio MCP 桥（默认连 8011）
      mdio status
      mdio stop [--force]
    """
    ctx.ensure_object(dict)
    ctx.obj["wt"] = wt


# 注册命令
from mdio_py.cli.commands import (
    exec_cmd,
    load_cmd,
    mcp_cmd,
    mount_cmd,
    unmount_cmd,
    offload_cmd,
    repl,
    serve,
    share_cmd,
    web_cmd,
    status_cmd,
    stop_cmd,
)

main.add_command(serve)
main.add_command(web_cmd)
main.add_command(mcp_cmd)
main.add_command(exec_cmd)
main.add_command(load_cmd)
main.add_command(mount_cmd)
main.add_command(unmount_cmd)
main.add_command(offload_cmd)
main.add_command(share_cmd)
main.add_command(status_cmd)
main.add_command(stop_cmd)
main.add_command(repl)

from mdio_py.cli.llm_config import llm_group
main.add_command(llm_group)
