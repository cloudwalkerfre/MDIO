"""Tests for the `hash` std: file hash, folder tree hash, list mode, exclusions."""

import hashlib
import re


def _sha(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _tree(rows) -> str:
    body = "\0".join(f"{path}:{digest}" for path, digest in sorted(rows))
    return hashlib.sha256(body.encode("utf-8")).hexdigest()


async def test_hash_single_file(runtime_ls, wt_uid, tmp_path):
    expected = _sha((tmp_path / "wt" / "a.md").read_text())
    res = await runtime_ls.exec("hash /a.md", wt_uid=wt_uid)
    assert "## /a.md" in res
    assert "- kind: file" in res
    assert expected in res
    assert "- mdio_error:" not in res


async def test_hash_single_file_list(runtime_ls, wt_uid, tmp_path):
    expected = _sha((tmp_path / "wt" / "a.md").read_text())
    res = await runtime_ls.exec("hash -l /a.md", wt_uid=wt_uid)
    assert res.count("## ") == 1
    assert "## /a.md" in res
    assert expected in res


async def test_hash_single_file_tojson(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash /a.md | tojson", wt_uid=wt_uid)
    assert '"nt":"h"' in res
    assert '"/a.md"' in res
    assert '"nt":"tb"' not in res


async def test_hash_folder_tree(runtime_ls, wt_uid, tmp_path):
    root = tmp_path / "wt"
    rows = []
    for rel in ("sub/c.md", "sub/deep/d.md"):
        rows.append(("/" + rel, _sha((root / rel).read_text())))
    expected = _tree(rows)

    res = await runtime_ls.exec("hash /sub", wt_uid=wt_uid)
    assert "## /sub" in res
    assert "- kind: dir" in res
    assert expected in res


async def test_hash_folder_list(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash -l /sub", wt_uid=wt_uid)
    assert "## /sub/c.md" in res
    assert "## /sub/deep/d.md" in res
    # default excludes hidden docs
    assert "hidden_in_sub" not in res
    assert res.count("## ") == 2


async def test_hash_folder_list_all_includes_hidden(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash -a -l /sub", wt_uid=wt_uid)
    assert "## /sub/.hidden_in_sub.md" in res
    assert "## /sub/c.md" in res


async def test_hash_empty_dir(runtime_ls, wt_uid):
    await runtime_ls.exec("mk -f /emptydir/", wt_uid=wt_uid)
    res = await runtime_ls.exec("hash /emptydir", wt_uid=wt_uid)
    assert "## /emptydir" in res
    assert "- kind: dir" in res
    assert "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" in res


async def test_hash_non_markdown(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash /ignore.txt", wt_uid=wt_uid)
    assert "mdio_info" in res
    assert "not a markdown doc" in res or "no matches" in res


async def test_hash_missing(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash /nonexistent", wt_uid=wt_uid)
    assert "mdio_info" in res
    assert "no matches" in res


async def test_hash_list_excludes_system_and_hidden_by_default(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash -l /", wt_uid=wt_uid)
    assert "## /a.md" in res
    assert "## /.hidden.md" not in res
    assert "## /.mds" not in res


async def test_hash_list_all_still_excludes_mds(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash -a -l /", wt_uid=wt_uid)
    assert "## /.hidden.md" in res
    assert "## /.mds" not in res


async def test_hash_mds_explicit(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash /.mds", wt_uid=wt_uid)
    assert "mdio_info" in res
    assert "system path excluded" in res


async def test_hash_root_tree(runtime_ls, wt_uid):
    res = await runtime_ls.exec("hash /", wt_uid=wt_uid)
    assert "## /" in res
    assert "- kind: dir" in res

    listed = await runtime_ls.exec("hash -l /", wt_uid=wt_uid)
    rows = [
        (m.group(1), m.group(2))
        for m in re.finditer(r"^## (.+)\n- kind: file\n- hash: ([0-9a-f]{64})$", listed, re.M)
    ]
    assert rows, listed
    assert _tree(rows) in res
