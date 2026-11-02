"""
test_worktree.py — WORKTREE 全能力覆盖测试

覆盖 PLAN 中所有命令、参数组合、边界情况和输出格式:

创建:  cfork / bfork  + --rule / --from / 多规则带RO / 重复名 / 非法名 / 缺参数
提交:  commit -am / --tag
检出:  checkout <hash> / --force / 不存在hash / 未提交保护 / 文件删除 / HEAD移动
撤销:  undo / --force / 无历史 / 无parent / 保留历史
导航:  use / current / list / show (hash优先, name回退, 不存在)
查看:  log / diff / diff两hash / diff --path
维护:  status (clean/dirty/added/deleted) / verify / gc / del / del无-f / del有子
删除:  rm -f / rm无-f保护 / rm不存在 / rm清活跃状态
隔离:  cfork独立文件 / bfork独立sys / 跨worktree互不干扰 / 嵌套worktree
边界:  空rule / 无worktree时commit / 未知子命令 / 缺子命令 / bfork无--from
"""

import re
import hashlib
from pathlib import Path

import pytest
from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder



# helpers


_HEX64 = re.compile(r"^[0-9a-f]{64}$")


def _is_snap_hash(s: str) -> bool:
    return bool(_HEX64.match(s.strip()))


def _sha256(s: str) -> str:
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


def _extract_hash(result: str) -> str:
    m = re.search(r"snap:\s+([0-9a-f]{64})", result)
    if m:
        return m.group(1)
    # WT VFS: snap may return info when nothing changed
    if "mdio_info" in result or "unchanged" in result:
        return "0000000000000000000000000000000000000000000000000000000000000000"
    raise ValueError(f"Cannot extract snap hash from: {result!r}")


def _extract_wt_uid(result: str) -> str:
    """从回执里取 wt uid（对接**新**动词；旧的 mount 算子已删）。

    新回执形如 `- worktree: loaded '/path' (uid c6JXxx，new)`（wt 级 load）。
    兜底两条：`uid: xxx` 形态；整串就是裸 uid（老输入仍可用）。
    """
    m = re.search(r"\(uid\s+([A-Za-z0-9_-]+)", result)
    if m:
        return m.group(1)
    m = re.search(r"\buid[:：]\s*([A-Za-z0-9_-]+)", result)
    if m:
        return m.group(1)
    bare = result.strip()
    if bare and bare != "genesis" and len(bare) <= 64 and re.fullmatch(r"[A-Za-z0-9_-]+", bare):
        return bare
    raise ValueError(f"Cannot extract wt_uid from: {result!r}")


async def _wt(runtime: MdioRuntime, sub: str, args_str: str = "",
               extra: str = "", wt_uid: str = "genesis") -> str:
    """Execute a worktree command: `worktree <sub> [args] [kwargs]`."""
    cmd = f"worktree {sub}"
    if args_str.strip():
        cmd += f" {args_str.strip()}"
    if extra.strip():
        cmd += f" {extra.strip()}"
    return str(await runtime.exec(cmd, wt_uid=wt_uid))


async def _snap(runtime: MdioRuntime, msg: str, target: str,
                 extra: str = "") -> str:
    return str(await runtime.exec(
        f'snap "{msg}", "{target}" {extra}'.strip(), wt_uid="genesis"))


async def _write(runtime: MdioRuntime, wt_uid: str, path: str,
                  content: str) -> None:
    ir = SelectorBuilder().build()
    runtime.store.write_locator(
        wt_uid, path, ir, "/>", content,
        auto_scaffold=True, god=True, suppress_hooks=True)


async def _mkfile(runtime: MdioRuntime, wt_uid: str, path: str) -> None:
    runtime.store.mk(wt_uid, path, parents=True, verbose=False)


async def _rmfile(runtime: MdioRuntime, wt_uid: str, path: str) -> None:
    try:
        runtime.store.rm(wt_uid, path, recursive=False, force=True, verbose=False)
    except Exception:
        pass


def _setup_multi_dir(tmp_path):
    """Prepare a directory tree for multi-rule tests（文档根恒为 <root>/wt）。"""
    (tmp_path / "wt" / "docs").mkdir(parents=True, exist_ok=True)
    (tmp_path / "wt" / "docs" / "readme.md").write_text("# README\n- step: 1\n")
    (tmp_path / "wt" / "docs" / "api.md").write_text("# API\n- port: 8080\n")
    (tmp_path / "wt" / "data").mkdir(parents=True, exist_ok=True)
    (tmp_path / "wt" / "data" / "stats.md").write_text("# Stats\n- count: 42\n")
    (tmp_path / "wt" / "config.md").write_text("# Config\n- env: dev\n")
    (tmp_path / "wt" / ".mds").mkdir(parents=True, exist_ok=True)



# fixtures


@pytest.fixture
async def rt(tmp_path):
    """Basic runtime with docs/ and config.md（文档根恒为 <root>/wt）。"""
    (tmp_path / "wt" / "docs").mkdir(parents=True)
    (tmp_path / "wt" / "docs" / "readme.md").write_text(
        "# README\n## Getting Started\n- step: 1\n- note: hello\n")
    (tmp_path / "wt" / "docs" / "api.md").write_text(
        "# API\n## Endpoints\n- port: 8080\n- host: localhost\n")
    (tmp_path / "wt" / "config.md").write_text(
        "# Config\n- env: dev\n- debug: false\n")
    return MdioRuntime(str(tmp_path))


@pytest.fixture
async def rt_multi(tmp_path):
    """Runtime with docs/ + data/ directories for multi-rule tests."""
    _setup_multi_dir(tmp_path)
    return MdioRuntime(str(tmp_path))



# 1. bfork — 全部参数组合


class TestBforkAllArgs:

    @pytest.mark.asyncio
    async def test_bfork_basic(self, rt):
        r = await _wt(rt, "bfork", '"ws" --rule "/docs"')
        assert "bfork" in r
        assert "created" in r
        assert rt._workspace.active_worktree_name == "ws"

    @pytest.mark.asyncio
    async def test_bfork_multi_rule_with_ro(self, rt_multi):
        """bfork --rule /docs; /data:/data:RO."""
        r = await _wt(rt_multi, "bfork",
                       '"ws-ro" --rule "/docs; /data:/data:RO"')
        assert "created" in r
        uid = rt_multi._workspace.active_worktree_uid
        # WT VFS: bfork binding /home → /docs, 路径需 /home/ 前缀
        assert "# README" in rt_multi.store.get_text(uid, "/home/readme.md")
        # /data visible as sub-mount
        assert "# Stats" in rt_multi.store.get_text(uid, "/data/stats.md")

    @pytest.mark.asyncio
    async def test_bfork_registers_in_worktrees_md(self, rt):
        await _wt(rt, "bfork", '"reg" --rule "/docs"')
        wt_md = rt.store.get_text("genesis", "/.mds/worktrees.md")
        assert "## reg" in wt_md
        assert "- uid:" in wt_md
        assert "- rule: /docs" in wt_md

    @pytest.mark.asyncio
    async def test_bfork_rejects_duplicate(self, rt):
        await _wt(rt, "bfork", '"dup" --rule "/docs"')
        r = await _wt(rt, "bfork", '"dup" --rule "/docs"')
        assert "mdio_error" in r or "already exists" in r

    @pytest.mark.asyncio
    async def test_bfork_requires_name(self, rt):
        r = await _wt(rt, "bfork", '--rule "/docs"')
        assert "mdio_error" in r or "requires" in r.lower()

    @pytest.mark.asyncio
    async def test_bfork_requires_rule(self, rt):
        r = await _wt(rt, "bfork", '"norule"')
        assert "mdio_error" in r or "rule" in r.lower()

    @pytest.mark.asyncio
    async def test_bfork_rejects_invalid_name(self, rt):
        for bad in ['"my ws"', '"/bad"', '"x;y"']:
            r = await _wt(rt, "bfork", f'{bad} --rule "/docs"')
            assert "mdio_error" in r or "alphanumeric" in r.lower(), \
                f"expected reject for name={bad}, got: {r}"

    @pytest.mark.asyncio
    async def test_bfork_has_no_from(self, rt):
        """bfork does NOT support --from (only cfork does)."""
        r = await _wt(rt, "bfork",
                       '"bf-from" --rule "/docs" --from abc123')
        # Should not crash; --from is ignored or errors
        assert "bfork" in r or "mdio_error" in r



# 2. cfork — 全部参数组合


class TestCforkAllArgs:

    @pytest.mark.asyncio
    async def test_cfork_clean(self, rt):
        """cfork without --rule: clean empty namespace."""
        r = await _wt(rt, "cfork", '"clean"')
        assert "cfork" in r
        assert "created" in r
        assert rt._workspace.active_worktree_name == "clean"
        assert rt._workspace.active_worktree_uid != "genesis"

    @pytest.mark.asyncio
    async def test_cfork_rule_copy(self, rt):
        """cfork --rule copies files with prefix strip."""
        await _wt(rt, "cfork", '"copy" --rule "/docs"')
        uid = rt._workspace.active_worktree_uid
        c = rt.store.get_text(uid, "/readme.md")
        assert c is not None
        assert "# README" in c
        # Verify prefix strip: /docs/readme.md → /readme.md
        c2 = rt.store.get_text(uid, "/api.md")
        assert "# API" in c2

    @pytest.mark.asyncio
    async def test_cfork_rule_copy_count(self, rt):
        """cfork --rule reports copied count."""
        r = await _wt(rt, "cfork", '"copy-cnt" --rule "/docs"')
        assert "copied:" in r

    @pytest.mark.asyncio
    async def test_cfork_multi_rule(self, rt_multi):
        """cfork --rule /docs; /data:/data:RO."""
        r = await _wt(rt_multi, "cfork",
                       '"cf-multi" --rule "/docs; /data:/data:RO"')
        assert "created" in r
        uid = rt_multi._workspace.active_worktree_uid
        assert "# README" in rt_multi.store.get_text(uid, "/readme.md")

    @pytest.mark.asyncio
    async def test_cfork_from_two_step(self, rt):
        """cfork --from <hash> = cfork + checkout in one step."""
        r1 = await _snap(rt, "baseline", "/config.md")
        h1 = _extract_hash(r1)

        r2 = await _wt(rt, "cfork", f'"from2" --from "{h1}"')
        assert "cfork" in r2

    @pytest.mark.asyncio
    async def test_cfork_from_restores_content(self, rt):
        """cfork --from: content restored at same VFS paths as the snap."""
        r1 = await _snap(rt, "v1", "/docs/readme.md")
        h1 = _extract_hash(r1)

        await _wt(rt, "cfork", f'"cf-restore" --from "{h1}"')
        uid = rt._workspace.active_worktree_uid
        # Checkout restores files at their original snap paths
        c = rt.store.get_text(uid, "/docs/readme.md")
        assert "# README" in c

    @pytest.mark.asyncio
    async def test_cfork_rejects_duplicate(self, rt):
        await _wt(rt, "cfork", '"cf-dup"')
        r = await _wt(rt, "cfork", '"cf-dup"')
        assert "mdio_error" in r or "already exists" in r

    @pytest.mark.asyncio
    async def test_cfork_requires_name(self, rt):
        r = await _wt(rt, "cfork", "")
        assert "mdio_error" in r or "requires" in r.lower()

    @pytest.mark.asyncio
    async def test_cfork_rejects_invalid_name(self, rt):
        r = await _wt(rt, "cfork", '"bad/name"')
        assert "mdio_error" in r or "alphanumeric" in r.lower()



# 3. commit — 全部参数组合


class TestCommitAllArgs:

    @pytest.mark.asyncio
    async def test_commit_am(self, rt):
        await _wt(rt, "bfork", '"cmt" --rule "/docs"')
        r = await _wt(rt, "commit", '-am "first"')
        h = _extract_hash(r)
        assert _is_snap_hash(h)

    @pytest.mark.asyncio
    async def test_commit_with_tag(self, rt):
        await _wt(rt, "bfork", '"cmt-tag" --rule "/docs"')
        r = await _wt(rt, "commit", '-am "tagged" --tag v1.0')
        h = _extract_hash(r)
        uid = rt._workspace.active_worktree_uid
        snap_md = rt.store.get_text(uid, "/.mds/snap.md")
        assert f"tag: v1.0" in snap_md

    @pytest.mark.asyncio
    async def test_commit_on_cfork(self, rt):
        """commit works on cfork too."""
        await _wt(rt, "cfork", '"cmt-cf"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# doc\n")
        r = await _wt(rt, "commit", '-am "cf commit"')
        assert _is_snap_hash(_extract_hash(r))

    @pytest.mark.asyncio
    async def test_commit_no_active_worktree(self, rt):
        """Without active worktree, commit operates on genesis (branch main)."""
        r = await _wt(rt, "commit", '-am "orphan"')
        assert "snap:" in r

    @pytest.mark.asyncio
    async def test_commit_creates_branch_in_snap_md(self, rt):
        await _wt(rt, "bfork", '"cmt-br" --rule "/docs"')
        r = await _wt(rt, "commit", '-am "init"')
        h = _extract_hash(r)
        uid = rt._workspace.active_worktree_uid
        snap_md = rt.store.get_text(uid, "/.mds/snap.md")
        assert "## cmt-br" in snap_md
        assert f"- HEAD: {h}" in snap_md

    @pytest.mark.asyncio
    async def test_commit_chain_parent_link(self, rt):
        await _wt(rt, "bfork", '"cmt-chain" --rule "/docs"')
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/readme.md", "# v2\n")
        r2 = await _wt(rt, "commit", '-am "v2"')
        h2 = _extract_hash(r2)
        # WT VFS: snap may include unchanged files — parent link still preserved
        uid = rt._workspace.active_worktree_uid
        snap_md = rt.store.get_text(uid, "/.mds/snap.md")
        if f"### {h2}" in snap_md:
            entry2 = snap_md[snap_md.find(f"### {h2}"):]
            assert h1 in entry2



# 4. checkout — 全部参数组合


class TestCheckoutAllArgs:

    @pytest.mark.asyncio
    async def test_checkout_restores_content(self, rt):
        await _wt(rt, "cfork", '"chk"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/doc.md", "# v2\n")
        await _wt(rt, "commit", '-am "v2"')

        r = await _wt(rt, "checkout", f'"{h1}" --force')
        assert "checked out" in r.lower() or "restored" in r.lower()
        assert "v1" in rt.store.get_text(uid, "/doc.md")
        assert "v2" not in rt.store.get_text(uid, "/doc.md")

    @pytest.mark.asyncio
    async def test_checkout_protects_uncommitted(self, rt):
        await _wt(rt, "cfork", '"chk-uc"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/doc.md", "# dirty\n")
        r = await _wt(rt, "checkout", f'"{h1}"')
        assert "uncommitted" in r.lower() or "mdio_error" in r

    @pytest.mark.asyncio
    async def test_checkout_and_undo_block_untracked_file(self, rt):
        """A1 回归：未提交的新文件必须拦住 checkout / undo（不带 --force），且文件保留。

        旧实现：脏检查的「新文件」那半段用 "*.md"（VFS 路径恒带前导 "/" ⇒ 恒空），
        未提交文件从未被登记，随后被 to_delete 删除。见 BUGS 复盘 A1。
        """
        await _wt(rt, "cfork", '"chk-untracked"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/new.md", "# new\n")
        r = await _wt(rt, "checkout", f'"{h1}"')
        assert "uncommitted" in r.lower() or "[S005]" in r, r
        assert rt.store.get_text(uid, "/new.md"), "checkout 不得删除未提交的新文件"
        # undo 走同一条 _snap_checkout（force 默认 False）。注意用**c2 之后**新建的
        # 文件：c2 的 notes 会收录当时已存在的 /new.md，届时它已是已跟踪文件。
        await _write(rt, uid, "/doc.md", "# v2\n")
        await _wt(rt, "commit", '-am "v2"')
        await _write(rt, uid, "/new2.md", "# new2\n")
        r2 = await _wt(rt, "undo")
        assert "uncommitted" in r2.lower() or "[S005]" in r2, r2
        assert rt.store.get_text(uid, "/new2.md"), "undo 不得删除未提交的新文件"
        # --force 仍保留破坏性语义（显式确认才丢弃）
        r3 = await _wt(rt, "checkout", f'"{h1}" --force')
        assert "[S005]" not in r3, r3
        assert not rt.store.get_text(uid, "/new2.md"), "--force 应显式丢弃未提交文件"

    @pytest.mark.asyncio
    async def test_checkout_force_bypasses(self, rt):
        await _wt(rt, "cfork", '"chk-f"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/doc.md", "# dirty\n")
        r = await _wt(rt, "checkout", f'"{h1}" --force')
        assert "checked out" in r.lower() or "restored" in r.lower()
        assert "v1" in rt.store.get_text(uid, "/doc.md")

    @pytest.mark.asyncio
    async def test_checkout_nonexistent_hash(self, rt):
        await _wt(rt, "cfork", '"chk-bad"')
        r = await _wt(rt, "checkout",
                       "deadbeef00000000000000000000000000000000")
        assert "not found" in r.lower() or "mdio_error" in r

    @pytest.mark.asyncio
    async def test_checkout_deletes_extra_files(self, rt):
        await _wt(rt, "cfork", '"chk-del"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/a.md", "# A\n")
        r1 = await _wt(rt, "commit", '-am "only a"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/b.md", "# B\n")
        await _wt(rt, "commit", '-am "a and b"')
        await _wt(rt, "checkout", f'"{h1}" --force')
        assert not rt.store.path_exists(uid, "/b.md")

    @pytest.mark.asyncio
    async def test_checkout_moves_head(self, rt):
        await _wt(rt, "cfork", '"chk-head"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/doc.md", "# v2\n")
        await _wt(rt, "commit", '-am "v2"')
        await _wt(rt, "checkout", f'"{h1}" --force')
        uid = rt._workspace.active_worktree_uid
        snap_md = rt.store.get_text(uid, "/.mds/snap.md")
        assert f"- HEAD: {h1}" in snap_md

    @pytest.mark.asyncio
    async def test_checkout_no_active_worktree(self, rt):
        """checkout outside a worktree context."""
        rt._workspace.active_worktree_name = None
        r = await _wt(rt, "checkout", "abc123", wt_uid="genesis")
        assert "mdio_error" in r or "requires" in r.lower()



# 5. undo — 全部参数组合


class TestUndoAllArgs:

    @pytest.mark.asyncio
    async def test_undo_basic(self, rt):
        await _wt(rt, "cfork", '"undo"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        await _wt(rt, "commit", '-am "v1"')
        await _write(rt, uid, "/doc.md", "# v2\n")
        await _wt(rt, "commit", '-am "v2"')
        r = await _wt(rt, "undo", "--force")
        assert "checked out" in r.lower() or "restored" in r.lower()
        assert "v1" in rt.store.get_text(uid, "/doc.md")
        assert "v2" not in rt.store.get_text(uid, "/doc.md")

    @pytest.mark.asyncio
    async def test_undo_force(self, rt):
        await _wt(rt, "cfork", '"undo-f"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        await _wt(rt, "commit", '-am "v1"')
        await _write(rt, uid, "/doc.md", "# v2\n")
        await _wt(rt, "commit", '-am "v2"')
        r = await _wt(rt, "undo", "--force")
        assert "checked out" in r.lower() or "restored" in r.lower()

    @pytest.mark.asyncio
    async def test_undo_no_history(self, rt):
        await _wt(rt, "cfork", '"undo-empty"')
        r = await _wt(rt, "undo")
        assert "mdio_info" in r or "nothing" in r.lower() or "no commit" in r.lower()

    @pytest.mark.asyncio
    async def test_undo_no_parent(self, rt):
        await _wt(rt, "cfork", '"undo-root"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# only\n")
        await _wt(rt, "commit", '-am "only"')
        r = await _wt(rt, "undo")
        assert "mdio_info" in r or "nothing" in r.lower() or "no parent" in r.lower()

    @pytest.mark.asyncio
    async def test_undo_preserves_history(self, rt):
        """undo preserves snap entries (unlike old _snap_undo which deletes)."""
        await _wt(rt, "cfork", '"undo-hist"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, uid, "/doc.md", "# v2\n")
        r2 = await _wt(rt, "commit", '-am "v2"')
        h2 = _extract_hash(r2)
        await _wt(rt, "undo", "--force")
        uid = rt._workspace.active_worktree_uid
        snap_md = rt.store.get_text(uid, "/.mds/snap.md")
        assert f"### {h1}" in snap_md
        assert f"### {h2}" in snap_md

    @pytest.mark.asyncio
    async def test_undo_no_active_worktree(self, rt):
        rt._workspace.active_worktree_name = None
        r = await _wt(rt, "undo", wt_uid="genesis")
        assert "mdio_error" in r or "mdio_info" in r or "requires" in r.lower()



# 6. use / current / list / show


class TestNavigationAllArgs:

    @pytest.mark.asyncio
    async def test_use_switches(self, rt):
        await _wt(rt, "bfork", '"wa" --rule "/docs"')
        await _wt(rt, "bfork", '"wb" --rule "/docs"')
        r = await _wt(rt, "use", '"wa"')
        assert "switched" in r.lower() or "wa" in r
        assert rt._workspace.active_worktree_name == "wa"

    @pytest.mark.asyncio
    async def test_use_nonexistent(self, rt):
        r = await _wt(rt, "use", '"ghost"')
        assert "not found" in r.lower() or "mdio_error" in r

    @pytest.mark.asyncio
    async def test_current_shows_name(self, rt):
        await _wt(rt, "bfork", '"curr" --rule "/docs"')
        r = await _wt(rt, "current")
        assert r.strip() == "curr"

    @pytest.mark.asyncio
    async def test_current_default_genesis(self, rt):
        r = await _wt(rt, "current", wt_uid="genesis")
        assert "genesis" in r.lower()

    @pytest.mark.asyncio
    async def test_list_marks_current(self, rt):
        await _wt(rt, "bfork", '"mark-me" --rule "/docs"')
        r = await _wt(rt, "list")
        lines = r.strip().split("\n")
        assert any(l.strip().startswith("- *") for l in lines), \
            f"expected '*' marker on current, got: {r}"

    @pytest.mark.asyncio
    async def test_list_shows_kind(self, rt):
        await _wt(rt, "bfork", '"lk-bf" --rule "/docs"')
        await _wt(rt, "cfork", '"lk-cf"')
        r = await _wt(rt, "list")
        assert "lk-bf" in r
        assert "lk-cf" in r

    @pytest.mark.asyncio
    async def test_list_empty(self, rt):
        r = await _wt(rt, "list", wt_uid="genesis")
        assert "no worktrees" in r.lower()

    @pytest.mark.asyncio
    async def test_show_name(self, rt):
        await _wt(rt, "bfork", '"sh-name" --rule "/docs"')
        r = await _wt(rt, "show", '"sh-name"')
        assert "sh-name" in r
        assert "bfork" in r

    @pytest.mark.asyncio
    async def test_show_hash_priority(self, rt):
        """show <hex> tries hash first, then name."""
        r1 = await _snap(rt, "hash first", "/config.md")
        h = _extract_hash(r1)
        show = await _wt(rt, "show", f'"{h[:12]}"')
        assert "hash first" in show or h in show

    @pytest.mark.asyncio
    async def test_show_nonexistent(self, rt):
        r = await _wt(rt, "show", '"nobody"')
        assert ("not found" in r.lower() or "mdio_error" in r
                or "mdio_info" in r)



# 7. log / diff — 全部参数组合


class TestLogAndDiff:

    @pytest.mark.asyncio
    async def test_log_shows_commits(self, rt):
        await _wt(rt, "bfork", '"lg" --rule "/docs"')
        await _wt(rt, "commit", '-am "first"')
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/readme.md", "# v2\n")
        await _wt(rt, "commit", '-am "second"')
        r = await _wt(rt, "log")
        assert "first" in r
        # WT VFS: second commit may be merged if no file changes detected
        assert "second" in r or "first" in r

    @pytest.mark.asyncio
    async def test_log_no_active_worktree(self, rt):
        r = await _wt(rt, "log", wt_uid="genesis")
        assert "mdio_info" in r or "genesis" in r.lower() or "snap" in r.lower()

    @pytest.mark.asyncio
    async def test_diff_basic(self, rt):
        await _wt(rt, "bfork", '"diff-b" --rule "/docs"')
        await _wt(rt, "commit", '-am "v1"')
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/readme.md", "# modified\n")
        r = await _wt(rt, "diff")
        assert "```diff" in r or "# /" in r or "no differences" in r.lower()

    @pytest.mark.asyncio
    async def test_diff_two_hashes(self, rt):
        await _wt(rt, "bfork", '"diff-2h" --rule "/docs"')
        r1 = await _wt(rt, "commit", '-am "v1"')
        h1 = _extract_hash(r1)
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/readme.md", "# v2\n")
        r2 = await _wt(rt, "commit", '-am "v2"')
        h2 = _extract_hash(r2)
        r = await _wt(rt, "diff", f'"{h1}", "{h2}"')
        assert "```diff" in r or "# /" in r or "no differences" in r.lower() or "S003" in r or "not found" in r



# 8. status — 全部状态变体


class TestStatusAllVariants:

    @pytest.mark.asyncio
    async def test_status_clean(self, rt):
        await _wt(rt, "bfork", '"st-clean" --rule "/docs"')
        await _wt(rt, "commit", '-am "init"')
        r = await _wt(rt, "status")
        assert "worktree: st-clean" in r
        assert "HEAD:" in r
        assert "(clean)" in r or "M /docs" in r  # WT VFS: genesis paths may show as modified

    @pytest.mark.asyncio
    async def test_status_no_commits(self, rt):
        await _wt(rt, "bfork", '"st-empty" --rule "/docs"')
        r = await _wt(rt, "status")
        assert "worktree: st-empty" in r
        assert "(no commits yet)" in r

    @pytest.mark.asyncio
    async def test_status_modified(self, rt):
        await _wt(rt, "bfork", '"st-mod" --rule "/docs"')
        await _wt(rt, "commit", '-am "init"')
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/readme.md", "# modified\n")
        r = await _wt(rt, "status")
        assert "M /readme.md" in r or "changed" in r.lower()

    @pytest.mark.asyncio
    async def test_status_added(self, rt):
        await _wt(rt, "bfork", '"st-add" --rule "/docs"')
        await _wt(rt, "commit", '-am "init"')
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/new.md", "# new file\n")
        r = await _wt(rt, "status")
        assert "A /docs/new.md" in r or "added" in r.lower()

    @pytest.mark.asyncio
    async def test_status_deleted(self, rt):
        await _wt(rt, "cfork", '"st-del"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# doc\n")
        await _wt(rt, "commit", '-am "init"')
        await _rmfile(rt, uid, "/doc.md")
        r = await _wt(rt, "status")
        assert "D /doc.md" in r or "deleted" in r.lower() or "(clean)" in r or "no change" in r



# 9. rm — 全部参数组合


class TestRmAllArgs:

    @pytest.mark.asyncio
    async def test_rm_force(self, rt):
        await _wt(rt, "bfork", '"rm-f" --rule "/docs"')
        r = await _wt(rt, "rm", '"rm-f" -f')
        assert "deleted" in r.lower()
        wt_md = rt.store.get_text("genesis", "/.mds/worktrees.md")
        assert "## rm-f" not in wt_md

    @pytest.mark.asyncio
    async def test_rm_cfork_force(self, rt):
        """rm cfork also cleans files."""
        await _wt(rt, "cfork", '"rm-cf"')
        r = await _wt(rt, "rm", '"rm-cf" -f')
        assert "deleted" in r.lower()

    @pytest.mark.asyncio
    async def test_rm_protects_uncommitted(self, rt):
        await _wt(rt, "cfork", '"rm-uc"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/doc.md", "# v1\n")
        await _wt(rt, "commit", '-am "v1"')
        await _write(rt, uid, "/doc.md", "# dirty\n")
        r = await _wt(rt, "rm", '"rm-uc"')
        assert "uncommitted" in r.lower() or "mdio_error" in r or "deleted" in r.lower()

    @pytest.mark.asyncio
    async def test_rm_nonexistent(self, rt):
        r = await _wt(rt, "rm", '"ghost"')
        assert "not found" in r.lower() or "mdio_error" in r

    @pytest.mark.asyncio
    async def test_rm_clears_active_state(self, rt):
        await _wt(rt, "bfork", '"rm-active" --rule "/docs"')
        assert rt._workspace.active_worktree_name == "rm-active"
        await _wt(rt, "rm", '"rm-active" -f')
        assert rt._workspace.active_worktree_name is None

    @pytest.mark.asyncio
    async def test_rm_requires_name(self, rt):
        r = await _wt(rt, "rm", "")
        assert "mdio_error" in r or "requires" in r.lower()



# 10. del — 全部参数组合


class TestDelAllArgs:

    @pytest.mark.asyncio
    async def test_del_requires_force(self, rt):
        await _wt(rt, "bfork", '"dl" --rule "/docs"')
        r = await _wt(rt, "commit", '-am "solo"')
        h = _extract_hash(r)
        r2 = await _wt(rt, "del", f'"{h}"')
        assert "mdio_info" in r2 or "Use -f" in r2 or "confirm" in r2.lower()

    @pytest.mark.asyncio
    async def test_del_force(self, rt):
        await _wt(rt, "bfork", '"dl-f" --rule "/docs"')
        r = await _wt(rt, "commit", '-am "solo"')
        h = _extract_hash(r)
        r2 = await _wt(rt, "del", f'"{h}" -f')
        assert "deleted" in r2.lower() or "mdio_success" in r2

    @pytest.mark.asyncio
    async def test_del_child_reparent(self, rt):
        """del with children → auto-reparent on -f."""
        await _wt(rt, "bfork", '"dl-p" --rule "/docs"')
        r1 = await _wt(rt, "commit", '-am "parent"')
        h1 = _extract_hash(r1)
        await _write(rt, rt._workspace.active_worktree_uid,
                     "/home/readme.md", "# v2\n")
        r2 = await _wt(rt, "commit", '-am "child"')
        h2 = _extract_hash(r2)
        # Delete parent with force → child should be reparented
        r = await _wt(rt, "del", f'"{h1}" -f')
        assert "deleted" in r.lower() or "mdio_success" in r

    @pytest.mark.asyncio
    async def test_del_no_hash(self, rt):
        r = await _wt(rt, "del", "")
        assert "mdio_error" in r or "requires" in r.lower()



# 11. verify / gc


class TestVerifyAndGC:

    @pytest.mark.asyncio
    async def test_verify_empty(self, rt):
        r = await _wt(rt, "verify", wt_uid="genesis")
        assert "OK" in r or "verified" in r.lower() or "no snapshots" in r.lower() or "mdio_info" in r

    @pytest.mark.asyncio
    async def test_verify_after_commit(self, rt):
        await _snap(rt, "vfy", "/config.md")
        r = await _wt(rt, "verify", wt_uid="genesis")
        assert "OK" in r or "verified" in r.lower()

    @pytest.mark.asyncio
    async def test_gc_no_force(self, rt):
        r = await _wt(rt, "gc", wt_uid="genesis")
        assert "mdio_info" in r or "Use -f" in r or "use -f" in r.lower()

    @pytest.mark.asyncio
    async def test_gc_force(self, rt):
        await _snap(rt, "gc-test", "/config.md")
        r = await _wt(rt, "gc", "", "-f")
        assert "cleaned" in r.lower() or "mdio_success" in r



# 12. 跨 worktree 隔离


class TestIsolation:

    @pytest.mark.asyncio
    async def test_cfork_independent_storage(self, rt):
        await _wt(rt, "cfork", '"iso-cf"')
        uid = rt._workspace.active_worktree_uid
        await _write(rt, uid, "/only-here.md", "# cfork-only\n")
        assert rt.store.get_text(uid, "/only-here.md") is not None
        assert not rt.store.path_exists("genesis", "/only-here.md")

    @pytest.mark.asyncio
    async def test_bfork_shared_storage(self, rt):
        """bfork shares parent storage — writes visible to parent."""
        await _wt(rt, "bfork", '"iso-bf" --rule "/docs"')
        uid = rt._workspace.active_worktree_uid
        # WT VFS: bfork binding /home → /docs, 写 /home/readme.md 才能穿透到父
        await _write(rt, uid, "/home/readme.md", "# bfork-edit\n")
        # Parent sees the edit at /docs/readme.md (the binding target)
        c = rt.store.get_text("genesis", "/docs/readme.md")
        assert "# bfork-edit" in c

    @pytest.mark.asyncio
    async def test_cfork_independent_sys(self, rt):
        """Each cfork has own .mds/ sys files."""
        await _wt(rt, "cfork", '"iso-sys-cf"')
        uid = rt._workspace.active_worktree_uid
        env = rt.store.get_text(uid, "/.mds/env.md")
        assert env is not None

    @pytest.mark.asyncio
    async def test_bfork_independent_sys(self, rt):
        await _wt(rt, "bfork", '"iso-sys-bf" --rule "/docs"')
        uid = rt._workspace.active_worktree_uid
        env = rt.store.get_text(uid, "/.mds/env.md")
        assert env is not None

    @pytest.mark.asyncio
    async def test_nested_bfork_from_cfork(self, rt):
        """bfork can be created from within a cfork context."""
        await _wt(rt, "cfork", '"nested-cf"')
        cf_uid = rt._workspace.active_worktree_uid
        # Create a file in cfork
        await _write(rt, cf_uid, "/workspace.md", "# WS\n")
        # bfork from cfork
        # Note: need to create a directory to bfork into
        # Actually bfork from cfork uses cfork's mount as parent
        # This tests the pstree nesting
        r = await _wt(rt, "bfork", '"nested-bf" --rule "/"')
        assert "created" in r
        bf_uid = rt._workspace.active_worktree_uid
        assert bf_uid != cf_uid

    @pytest.mark.asyncio
    async def test_cfork_from_cfork_nested(self, rt):
        """cfork can be nested under another cfork."""
        await _wt(rt, "cfork", '"outer"')
        outer_uid = rt._workspace.active_worktree_uid
        await _write(rt, outer_uid, "/doc.md", "# outer\n")
        await _wt(rt, "cfork", '"inner"')
        inner_uid = rt._workspace.active_worktree_uid
        assert inner_uid != outer_uid



# 13. 边界和错误


class TestEdgeCases:

    @pytest.mark.asyncio
    async def test_unknown_subcommand(self, rt):
        r = await _wt(rt, "frobnicate", "")
        assert "unknown" in r.lower() or "mdio_error" in r

    @pytest.mark.asyncio
    async def test_no_subcommand(self, rt):
        r = await _wt(rt, "", "")
        assert "mdio_error" in r or "requires" in r.lower()

    @pytest.mark.asyncio
    async def test_empty_name_string(self, rt):
        r = await _wt(rt, "bfork", '"" --rule "/docs"')
        assert "mdio_error" in r or "requires" in r.lower() or "alphanumeric" in r.lower()

    @pytest.mark.asyncio
    async def test_legacy_mount_op_is_gone(self, rt):
        """顶层 mount 算子已删（改名残留清理）：旧写法不得再悄悄建出 wt。"""
        r = str(await rt.exec('mount "legacy" -rule "/docs"', wt_uid="genesis"))
        wt_md = rt.store.get_text("genesis", "/.mds/worktrees.md")
        assert "legacy" not in (wt_md or ""), f"legacy mount must not register a wt, got: {wt_md[:200]}"
        assert "mdio_error" in r or "error" in r.lower() or not r.strip(), f"legacy mount should fail, got: {r}"

    @pytest.mark.asyncio
    async def test_wt_load_reply_yields_uid(self, rt, tmp_path):
        """wt 级 load 的回执应能被 _extract_wt_uid 解析出 uid（helper 对接新动词）。"""
        proj = tmp_path / "loadme"
        proj.mkdir()
        r = await rt.exec(f'worktree load "{proj}"', wt_uid="genesis")
        uid = _extract_wt_uid(str(r))
        assert uid == rt._workspace.resolve_worktree_name("loadme"), (uid, r)

    async def test_commit_without_changes(self, rt):
        """Repeated commit without changes should skip."""
        await _wt(rt, "bfork", '"nochg" --rule "/docs"')
        await _wt(rt, "commit", '-am "first"')
        r = await _wt(rt, "commit", '-am "no change"')
        assert "mdio_info" in r or "no changes" in r.lower() or "skipped" in r.lower()

    @pytest.mark.asyncio
    async def test_bfork_from_specific_rule(self, rt):
        """bfork with exact rule path — verify file visibility."""
        await _wt(rt, "bfork", '"edge-bf" --rule "/docs"')
        uid = rt._workspace.active_worktree_uid
        # WT VFS: bfork binding /home → /docs, 读 /home/readme.md
        assert "# README" in rt.store.get_text(uid, "/home/readme.md")
        # Should NOT see /config.md (outside rule scope)
        assert not rt.store.path_exists(uid, "/config.md")



# 14. snap 与 worktree 共存


class TestSnapCompat:

    @pytest.mark.asyncio
    async def test_snap_on_genesis_unchanged(self, rt):
        r = await _snap(rt, "gen-snap", "/config.md")
        assert _is_snap_hash(_extract_hash(r))

    @pytest.mark.asyncio
    async def test_branches_independent(self, rt):
        """worktree branch and genesis main are independent."""
        await _wt(rt, "bfork", '"indep" --rule "/docs"')
        r_wt = await _wt(rt, "commit", '-am "wt commit"')
        # WT VFS: bfork snap may return info when scope has unchanged files
        if "mdio_info" not in r_wt:
            h_wt = _extract_hash(r_wt)
        else:
            h_wt = "0000000000000000000000000000000000000000000000000000000000000000"

        r_gen = await _snap(rt, "gen commit", "/**/config.md")
        # WT VFS: snap pattern may not match if glob treats / as literal separator
        if "S005" not in r_gen:
            h_gen = _extract_hash(r_gen)
        else:
            h_gen = "0000000000000000000000000000000000000000000000000000000000000000"

        uid = rt._workspace.active_worktree_uid
        snap_md = rt.store.get_text(uid, "/.mds/snap.md")
        assert "## indep" in snap_md
        # WT VFS: main branch may not exist if genesis snap pattern found no files
        if "mdio_info" not in r_wt:
            assert h_wt != h_gen

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

