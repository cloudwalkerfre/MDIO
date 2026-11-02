"""MCP 端点（/mcp）集成测试 —— 定则：`cwd` 是 harness 项目根，mdio 根恒为 `<cwd>/.mdio`，文档根 `<cwd>/.mdio/wt`。

覆盖：
  · 双时代握手(initialize / server/discover)与 tools/list
  · 无 cwd ⇒ 落 genesis，不注册 wt
  · cwd=<项目根> ⇒ 注册 mdio 根 `<cwd>/.mdio`：条目 path == `<cwd>/.mdio`、doc_subdir=wt、wal_rel=.wal；文档落在 `<cwd>/.mdio/wt`
  · cwd 已以 `.mdio` 结尾 ⇒ 它就是 mdio 根，条目标题取父目录名；文档在 `<cwd>/wt`
  · cwd 不存在 ⇒ -32602（不凭空建项目树）
  · 锚点 cwd（自家地盘）⇒ 落 genesis，不建任何目录
  · rule：默认两段串（首注自动带）；MDIO_LOAD_RULE 覆盖；显式 ="" ⇒ 不带 rule
  · 共享文档只读：项目 wt 内可读 genesis 的 /docs/skills/manual.md，写它被拒（RO/R201）

注：登记表断言一律走 VFS（`$ /.mds/worktrees.md::`，genesis），不读磁盘文件——
条目落盘要等 flush，直接读文件会假失败。
"""

import pytest
from litestar.testing import TestClient

DEFAULT_RULE = "/docs/skills:docs/skills:RO; /docs/skills:docs/skills:RO"
MANUAL = "# 共享手册\n- x: 1\n"

# 持久化面（conftest 的 autouse fixture 据此设 MDIO_PERSIST=1）
pytestmark = pytest.mark.persist


@pytest.fixture
def mcp_app(tmp_path, monkeypatch):
    """隔离的 mdio 家目录（内含预置共享手册）+ 不在自家子树下的项目根。

    家目录同样是"项目根"语义：布局恒定 ⇒ 它的文档根是 `<home>/wt`。
    """
    home = tmp_path / "mdio_home"
    (home / "wt" / "docs" / "skills").mkdir(parents=True)
    (home / "wt" / "docs" / "skills" / "manual.md").write_text(MANUAL, encoding="utf-8")
    monkeypatch.setenv("MDIO_HOME", str(home))
    monkeypatch.setenv("MDIO_VFS_MOUNTS", str(home))
    monkeypatch.setenv("MDIO_WORKSPACE_ROOT", str(home))
    monkeypatch.setenv("MDIO_PROCESS_MODE", "control")
    monkeypatch.delenv("MDIO_LOAD_RULE", raising=False)
    from mdio_py.gateway import create_app
    return create_app()


@pytest.fixture
def harness_proj(tmp_path):
    """项目根：自己的文件在根下（不进 mdio 的 wt）。"""
    proj = tmp_path / "myproj"
    proj.mkdir()
    (proj / "README.md").write_text("# 我的项目\n- a: 1\n", encoding="utf-8")
    return proj


def call_mcp(client, code, cwd=None, rid=1):
    args = {"code": code}
    if cwd is not None:
        args["cwd"] = str(cwd)
    resp = client.post("/mcp", json={
        "jsonrpc": "2.0", "id": rid, "method": "tools/call",
        "params": {"name": "exec", "arguments": args},
    })
    assert resp.status_code in (200, 201)   # Litestar 对 POST 默认 201
    return resp.json()


def texts(out):
    return [c["text"] for c in out["result"]["content"]]


def receipt(out):
    t = texts(out)
    assert len(t) == 2, t                     # 第二块才是回执
    return t[1]


def entry_of(client, name):
    """从 genesis 的登记表里取条目的字段（走 VFS，不读盘）。"""
    body = texts(call_mcp(client, "$ /.mds/worktrees.md::"))[0]
    for block in body.split("## ")[1:]:
        lines = block.splitlines()
        if lines and lines[0].strip() == name:
            fields = {}
            for line in lines[1:]:
                if line.startswith("- ") and ":" in line:
                    k, v = line[2:].split(":", 1)
                    fields[k.strip()] = v.strip()
            return fields
    return {}


# 握手与目录

def test_handshake_legacy_and_modern(mcp_app):
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        legacy = client.post("/mcp", json={
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}).json()
        assert legacy["result"]["protocolVersion"] == "2025-11-25"
        assert "resultType" not in legacy["result"]

        modern = client.post("/mcp", json={
            "jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}}).json()
        assert modern["result"]["protocolVersion"] == "2025-11-25"
        assert modern["result"]["resultType"] == "complete"     # 2026 世代必须自带

        tools = client.post("/mcp", json={
            "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}).json()
        assert [t["name"] for t in tools["result"]["tools"]] == ["exec"]
        cwd_desc = tools["result"]["tools"][0]["inputSchema"]["properties"]["cwd"]["description"]
        assert "<cwd>/.mdio" in cwd_desc                        # schema 明确约定 mdio 根 = <cwd>/.mdio


# harness cwd → mdio 根 <cwd>/.mdio + 文档根 <cwd>/.mdio/wt

def test_no_cwd_stays_genesis(mcp_app, tmp_path):
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        out = call_mcp(client, "ls -l /")
        assert len(texts(out)) == 1                    # 没带 cwd ⇒ 没有回执块
        assert not (tmp_path / "myproj").exists()


def test_cwd_registered_as_given_and_docs_go_to_wt(mcp_app, harness_proj):
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        out = call_mcp(client, "mk -f /notes.md", cwd=harness_proj)
        assert "new=true" in receipt(out)
        e = entry_of(client, "myproj")
        mdio_root = harness_proj / ".mdio"
        assert e.get("path") == str(mdio_root)                   # mdio 根 = <cwd>/.mdio
        assert e.get("doc_subdir") == "wt"
        assert e.get("wal_rel") == ".wal"
        assert e.get("rule") == DEFAULT_RULE

        call_mcp(client, '"# n\\n- a: 1" | /> /notes.md -f', cwd=harness_proj)
        assert (mdio_root / "wt" / "notes.md").exists()          # 文档根 = <cwd>/.mdio/wt
        assert not (harness_proj / "notes.md").exists()         # 不在项目根上
        assert "a: 1" in texts(call_mcp(client, "$ /notes.md::", cwd=harness_proj))[0]

        # 幂等：同 cwd 再调不新增条目
        assert "new=false" in receipt(call_mcp(client, "ls -l /", cwd=harness_proj))


def test_mdio_suffix_is_not_special(mcp_app, tmp_path):
    """cwd 已经是 `.mdio`：它本身就是 mdio 根；条目标题取父目录名，文档在它下面的 wt。"""
    root = tmp_path / "proj" / ".mdio"
    root.mkdir(parents=True)
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        call_mcp(client, "mk -f /a.md", cwd=root)
        assert entry_of(client, "proj").get("path") == str(root)   # 标题取父目录名
        call_mcp(client, '"# a\\n- b: 2" | /> /a.md -f', cwd=root)
        assert (root / "wt" / "a.md").exists()
        assert not (tmp_path / "proj" / "wt").exists()          # 不外扩到父目录


def test_missing_cwd_is_invalid(mcp_app, tmp_path):
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        out = call_mcp(client, "ls -l /", cwd=tmp_path / "nope")
        assert out["error"]["code"] == -32602
        assert "invalid cwd" in out["error"]["message"]
        assert not (tmp_path / "nope").exists()                 # 不凭空建项目树


def test_anchor_cwd_stays_genesis(mcp_app, tmp_path):
    home = tmp_path / "mdio_home"
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        out = call_mcp(client, "ls -l /", cwd=home)
        assert "uid=genesis" in receipt(out)                    # 自家地盘 ⇒ 不注册 wt
        body = texts(call_mcp(client, "$ /.mds/worktrees.md::"))[0]
        assert str(home) not in body                            # 也不留下指向自家的条目


# rule（首注自动带；env 可覆盖）

def test_manual_readable_and_write_blocked(mcp_app, harness_proj):
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        call_mcp(client, "mk -f /x.md", cwd=harness_proj)
        assert "共享手册" in texts(call_mcp(client, "$ /docs/skills/manual.md::", cwd=harness_proj))[0]
        blocked = texts(call_mcp(client, '"# 偷改" | /> /docs/skills/manual.md -f', cwd=harness_proj))[0]
        assert "R201" in blocked or "Write blocked" in blocked


def test_rule_env_override_and_empty(mcp_app, tmp_path, monkeypatch):
    p1 = tmp_path / "p1"; p1.mkdir()
    p2 = tmp_path / "p2"; p2.mkdir()
    monkeypatch.setenv("MDIO_LOAD_RULE", "/x:y:RO")
    with TestClient(app=mcp_app, base_url="http://test.local") as client:
        call_mcp(client, "mk -f /a.md", cwd=p1)
        assert entry_of(client, "p1").get("rule") == "/x:y:RO"
        monkeypatch.setenv("MDIO_LOAD_RULE", "")
        call_mcp(client, "mk -f /a.md", cwd=p2)
        assert "rule" not in entry_of(client, "p2")             # 显式空 ⇒ 不带 rule
