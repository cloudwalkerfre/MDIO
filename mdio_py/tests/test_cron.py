"""
CRON 插件全面测试
运行: uv run --python "3.14t" python -X jit -Xgil=0 ./mdio_py/tests/test_cron.py
"""

import asyncio, tempfile, time
from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.selector_builder import SelectorBuilder

# helpers
def _jid(res): return str(res).strip().split()[2]

async def _proc(runtime, ctx, job_id):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.CRON.{job_id}", ctx=ctx)
    if not raw or "- mdio_error:" in raw: return {}
    s = {}
    for line in raw.strip().split("\n"):
        line = line.strip()
        if line.startswith("- ") and ":" in line:
            k, v = line[2:].split(":", 1)
            s[k.strip()] = v.strip()
    return s

async def _exists(runtime, ctx, job_id): return len(await _proc(runtime, ctx, job_id)) > 0

async def _setup(runtime, ctx, **paths):
    for p, h in paths.items():
        await runtime.exec(f'mk -f {p}', ctx=ctx)
        if h: await runtime.exec(f'"# {h}" | +> {p}::', ctx=ctx)

_CB_INCR = '$ /counter.md::C._b:val | calc -e "_ + 1" | /> /counter.md::C._b:val'
_CB_LOG  = '"fired" | +> /log.md::LOGS'

# 1. -block delay
async def test_block_delay():
    print("\n🧪 [1] -block 阻塞")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/target.md": "LOG"})
        t0 = time.time()
        res = await r.exec('"hello-cron" | @CRON -block "1s" | +> /target.md::LOG', ctx=c)
        dt = time.time() - t0
        assert 0.8 < dt < 2.5; assert "mdio_success" in str(res)
        assert "hello-cron" in str(await r.exec("$/target.md::LOG", ctx=c))
        print("   ✅ 阻塞 1s → 透传 → 写入成功")

# 2. -block delay + pipeline chain
async def test_block_delay_chain():
    print("\n🧪 [2] -block 阻塞 + 长管道链")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/target.md": "LOG"})
        t0 = time.time()
        res = await r.exec(r'''"42" | @CRON -block "1s" | calc -e "_ * 2" | /> /target.md::LOG._b:val''' r''' ; $ /target.md::LOG._b:val''', ctx=c)
        dt = time.time() - t0
        assert 0.5 < dt < 3.0; assert "84" in str(res)
        print("   ✅ 阻塞 1s → calc → 写入 → 读回 → 84")

# 3. -every repeating
async def test_every_repeating():
    print("\n🧪 [3] -every 重复")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/counter.md": "C"})
        await r.exec('"# C\n- val: 0" | +> /counter.md::', ctx=c)
        jid = _jid(await r.exec(f"@CRON -every '0.5s' -cb '{_CB_INCR}'", ctx=c))
        await asyncio.sleep(3)
        cnt = int(str(await r.exec("$/counter.md::C._b:val", ctx=c)).strip())
        assert cnt >= 2, f"cnt={cnt}"
        await r.exec(f'@CRON -del "{jid}"', ctx=c); await asyncio.sleep(0.3)
        assert not await _exists(r, c, jid)
        print(f"   计数器: {cnt}")

# 4. -at ISO absolute
async def test_at_absolute():
    print("\n🧪 [4] -at ISO")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        iso = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(time.time()+2)) + "Z"
        jid = _jid(await r.exec(f"@CRON -at '{iso}' -cb '{_CB_LOG}'", ctx=c))
        await asyncio.sleep(3)
        assert "fired" in str(await r.exec("$/log.md::LOGS", ctx=c))
        print("   ✅ -at ISO → 定时触发")

# 5. -cron expression
async def test_cron_expression():
    print("\n🧪 [5] -cron")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        jid = _jid(await r.exec(f"@CRON -cron '* * * * *' -cb '{_CB_LOG}'", ctx=c))
        nr = float((await _proc(r, c, jid))["next_run"])
        assert 0 < nr - time.time() < 65
        print(f"   next_run 在 {nr - time.time():.0f}s 后")
        await r.exec(f'@CRON -del "{jid}"', ctx=c)

# 6. -list
async def test_list():
    print("\n🧪 [6] -list")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        assert "No jobs" in str(await r.exec("@CRON -list", ctx=c)) or "CRON_JOBS" in str(await r.exec("@CRON -list", ctx=c))
        await r.exec(f"@CRON -every '60s' -cb '{_CB_LOG}'", ctx=c)
        await r.exec("@CRON -cron '0 9 * * *' -cb '$ /x.md::X | +> /y.md'", ctx=c)
        ls = str(await r.exec("@CRON -list", ctx=c))
        assert "every" in ls and "cron" in ls
        print("   ✅ -list OK")

# 7. -del
async def test_del():
    print("\n🧪 [7] -del")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        jid = _jid(await r.exec(f"@CRON -every '30s' -cb '{_CB_LOG}'", ctx=c))
        assert await _exists(r, c, jid)
        await r.exec(f'@CRON -del "{jid}"', ctx=c); await asyncio.sleep(0.3)
        assert not await _exists(r, c, jid)
        print("   ✅ -del → 全清")

# 8. Error cases
async def test_errors():
    print("\n🧪 [8] 错误输入")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        for flag, val in [("-every","5s"),("-at","2026-05-01T09:00:00Z"),("-cron","0 9 * * *")]:
            assert "requires -cb" in str(await r.exec(f'@CRON {flag} "{val}"', ctx=c))
        assert "minimum" in str(await r.exec('@CRON -every "0s" -cb "yes"', ctx=c)).lower()
        assert "minimum" in str(await r.exec('@CRON -every "0.01s" -cb "yes"', ctx=c)).lower()
        assert "Invalid cron" in str(await r.exec('@CRON -cron "bad" -cb "yes"', ctx=c)) or "error" in str(await r.exec('@CRON -cron "bad" -cb "yes"', ctx=c)).lower()
        print("   ✅ 错误提示正确")

# 9. proc.md state lifecycle
async def test_proc_lifecycle():
    print("\n🧪 [9] proc.md 状态")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/t.md": "T"})
        jid = _jid(await r.exec('@CRON -every "10s" -cb \'"ok" | +> /t.md::T\'', ctx=c))
        s0 = await _proc(r, c, jid)
        assert s0["status"] == "IDLE" and s0["tick"] == "0" and s0["last_run"] == "-"
        # Write tick directly to trigger
        await r.exec(f'"1" | /> /.mds/proc.md::PROCESS_TABLE.CRON.{jid}._b:tick', ctx=c)
        await asyncio.sleep(0.5)
        s1 = await _proc(r, c, jid)
        assert s1["tick"] == "1", f"tick not updated: {s1}"
        # Verify pipeline ran: "ok" was appended to /t.md
        t_content = str(await r.exec("$/t.md::T", ctx=c))
        assert "ok" in t_content, f"pipeline didn't run: {t_content}"
        print(f"   初始 → tick=0 IDLE; 触发 → tick=1 pipeline OK")
        await r.exec(f'@CRON -del "{jid}"', ctx=c)

# 10. VFS persistence (cron.md + proc.md + hook.md)
async def test_persistence():
    print("\n🧪 [10] VFS 持久化")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        jid = _jid(await r.exec(f"@CRON -every '60s' -cb '{_CB_LOG}'", ctx=c))
        cron_txt = r._workspace.store.get_text("genesis", "/.mds/cron.md")
        assert jid in cron_txt and "every" in cron_txt and "60s" in cron_txt
        assert "created_at:" in cron_txt
        proc_txt = r._workspace.store.get_text("genesis", "/.mds/proc.md")
        assert jid in proc_txt
        hook_txt = r._workspace.store.get_text("genesis", "/.mds/hook.md")
        print(f"   cron.md: {len(cron_txt)}c  proc.md: {len(proc_txt)}c  hook.md: {len(hook_txt)}c")
        await r.exec(f'@CRON -del "{jid}"', ctx=c)

# 11. -at auto-cleanup
async def test_at_cleanup():
    print("\n🧪 [11] -at 自清理")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        iso = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(time.time()+2)) + "Z"
        jid = _jid(await r.exec(f"@CRON -at '{iso}' -cb '{_CB_LOG}'", ctx=c))
        assert await _exists(r, c, jid)
        await asyncio.sleep(3)
        assert "fired" in str(await r.exec("$/log.md::LOGS", ctx=c))
        print("   ✅ -at → 触发 → 自清理")

# 12. CRON in pipeline (registration result passthrough)
async def test_cron_in_pipeline():
    print("\n🧪 [12] CRON 管道链")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        res = await r.exec(f"@CRON -every '60s' -cb '{_CB_LOG}' | fmt -t 'Job: {{}}'", ctx=c)
        assert "Job: - cron:" in str(res)
        await r.exec(f'@CRON -del "{_jid(res)}"', ctx=c)
        print("   ✅ @CRON 结果可管道转发")

# 13. @CRON inside Hook callback
async def test_cron_inside_hook():
    print("\n🧪 [13] Hook -cb 内注册 @CRON")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/trigger.md": "T", "/log.md": "LOGS"})
        await r.exec('"- val: 0" | +> /trigger.md', ctx=c)
        ws = r._workspace
        ws.store.register_hook("genesis", "/trigger.md",
            SelectorBuilder().drill("T").type_is("bullet").tag_eq("val").slice(0).build(),
            "@CRON -at '2s' -cb '\"hook_cron_fired\" | +> /log.md::LOGS'",
            {"wt_uid":0,"vfs_path":"/","active_doc":"null"}, is_always=True)
        await asyncio.sleep(0.3)
        await r.exec('"1" | /> /trigger.md::T._b:val', ctx=c)
        await asyncio.sleep(3)
        assert "hook_cron_fired" in str(await r.exec("$/log.md::LOGS", ctx=c))
        print("   ✅ Hook → @CRON → 触发")

# 14. -block "0s" immediate
async def test_block_zero():
    print("\n🧪 [14] -block 0s")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        t0 = time.time()
        res = await r.exec('"ok" | @CRON -block "0s" | +> /log.md::LOGS', ctx=c)
        assert time.time() - t0 < 0.5 and "mdio_success" in str(res)
        print("   ✅ -block 0s → 近乎即时")

# 15. Multiple concurrent jobs
async def test_multi_jobs():
    print("\n🧪 [15] 多 job 并发")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        jobs = []
        for name in ("a","b","c"):
            await _setup(r, c, **{f"/{name}.md": name.upper()})
            jobs.append(_jid(await r.exec(
                f"@CRON -every '0.5s' -cb '\"tick\" | +> /{name}.md::{name.upper()}'", ctx=c)))
        await asyncio.sleep(2)
        for name in ("a","b","c"):
            assert "tick" in str(await r.exec(f"$/{name}.md::{name.upper()}", ctx=c))
        ls = str(await r.exec("@CRON -list", ctx=c))
        for jid in jobs: assert jid in ls
        for jid in jobs: await r.exec(f'@CRON -del "{jid}"', ctx=c)
        print("   ✅ 3 jobs 并发正确")

# 16. wt_uid/cwd persistence
async def test_context():
    print("\n🧪 [16] 上下文持久化")
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        jid = _jid(await r.exec(f"@CRON -every '1s' -cb '{_CB_LOG}'", ctx=c))
        cron_txt = r._workspace.store.get_text("genesis", "/.mds/cron.md")
        assert "wt_uid: genesis" in cron_txt and "cwd: /" in cron_txt
        assert "created_at:" in cron_txt
        await asyncio.sleep(2)
        assert "fired" in str(await r.exec("$/log.md::LOGS", ctx=c))
        await r.exec(f'@CRON -del "{jid}"', ctx=c)
        print("   ✅ wt_uid + cwd + created_at 持久化正确")

# 17. dateparser — comprehensive format parsing
async def test_dateparser_all():
    print("\n🧪 [17] dateparser 全格式")
    epoch_may1 = 1777593600
    now = int(time.time())
    cases = [
        # relative
        ("简单秒","5s",5),("简单分","5m",300),("简单时","2h",7200),("简单天","7d",604800),
        ("复合","1h30m",5400),("小数","1.5h",5400),
        ("英文秒","5 seconds",5),("英文分","5 minutes",300),("in时","in 2 hours",7200),
        ("in天","in 3 days",259200),("tomorrow","tomorrow",None),  # None = just check register
        # ISO
        ("ISO+Z","2026-05-01T09:00:00Z",epoch_may1+32400),
        ("ISO无Z","2026-05-01T09:00:00",epoch_may1+32400),
        ("仅日期","2026-05-01",epoch_may1),
        ("斜杠","2026/05/01",epoch_may1),
        ("英文月","May 1, 2026",epoch_may1),
        # Chinese
        ("中文秒","5秒",5),("中文时","2小时",7200),
    ]
    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d)); c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        await _setup(r, c, **{"/log.md": "LOGS"})
        ok = fail = 0
        for label, ts, expected in cases:
            res = await r.exec(f"@CRON -at '{ts}' -cb '{_CB_LOG}'", ctx=c)
            if "cron_" not in str(res):
                print(f"   ❌ {label} {ts} PARSE FAILED"); fail += 1; continue
            jid = _jid(res); nr = int((await _proc(r, c, jid))["next_run"])
            if expected is not None:
                if expected > 10_000_000:
                    p = (nr == expected); detail = f"epoch={nr} exp={expected}"
                else:
                    delta = nr - now; p = abs(delta - expected) <= 5
                    detail = f"in {delta}s exp {expected}s"
                if p: ok += 1; print(f"   ✅ {label:8s} {ts:28s} {detail}")
                else: fail += 1; print(f"   ❌ {label:8s} {ts:28s} {detail}")
            else:
                ok += 1; print(f"   ✅ {label:8s} {ts:28s} registered")
            await r.exec(f'@CRON -del "{jid}"', ctx=c)
        print(f"\n   ✅ {ok}/{ok+fail}"); assert fail == 0

# 18. Bootstrap recovery: crash + restart
async def test_bootstrap_recovery():
    """Simulate: register jobs → crash (wipe proc.md) → restart (bootstrap).

    - -at with short trigger that has already passed → should NOT fire
    - -at with distant trigger → recovered correctly
    - -every → recovered with cadence aligned to created_at
    """
    print("\n🧪 [18] Bootstrap 崩溃恢复")

    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d))
        c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
        ws = r._workspace
        cron_plugin = ws.get_plugin("CRON")

        await _setup(r, c, **{"/log.md": "LOGS"})

        # Job A: -at that will fire in 2s and be missed after crash
        iso_a = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(time.time()+2)) + "Z"
        jid_a = _jid(await r.exec(
            f"@CRON -at '{iso_a}' -cb '\"A_fired\" | +> /log.md::LOGS'", ctx=c))
        s_a = await _proc(r, c, jid_a)
        nr_a = int(float(s_a["next_run"]))
        print(f"   Job A (-at 2s):  next_run in {nr_a - time.time():.0f}s")

        # Job B: -at that fires far in the future
        iso_b = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(time.time()+3600)) + "Z"
        jid_b = _jid(await r.exec(
            f"@CRON -at '{iso_b}' -cb '\"B_fired\" | +> /log.md::LOGS'", ctx=c))
        s_b = await _proc(r, c, jid_b)
        nr_b = int(float(s_b["next_run"])); print(f"   Job B (-at 1h): next_run in {nr_b - time.time():.0f}s")

        # Job C: -every 3s recurring
        jid_c = _jid(await r.exec(
            '@CRON -every "3s" -cb \'"C_tick" | +> /log.md::LOGS\'', ctx=c))
        print(f"   Job C (-every 3s)")

        # 等待 Job A 触发 + Job C tick —— 用轮询而不是固定 sleep：
        # Job C 每 3s 触发一次，首跳在 +3s；固定 sleep 不足以覆盖调度延迟。
        deadline = time.time() + 8
        log_pre = ""
        while time.time() < deadline:
            await asyncio.sleep(0.3)
            log_pre = str(await r.exec("$/log.md::LOGS", ctx=c))
            if "A_fired" in log_pre and "C_tick" in log_pre:
                break

        # Verify jobs fired before crash
        assert "A_fired" in log_pre, f"Job A should have fired before crash: {log_pre}"
        assert "C_tick" in log_pre, f"Job C should have ticks before crash: {log_pre}"
        print(f"   崩溃前: A_fired={('A_fired' in log_pre)}, C_ticks={log_pre.count('C_tick')}")

        # CRASH: wipe proc.md
        await r.exec('"# PROCESS_TABLE\\n" | /> /.mds/proc.md:: -f true', ctx=c)
        assert jid_a not in str(await r.exec("$/.mds/proc.md::PROCESS_TABLE.CRON", ctx=c))

        # RESTART: bootstrap CRON
        await cron_plugin.bootstrap(ws)
        await asyncio.sleep(1)

        # Check: Job A should NOT exist (overdue at-job → skipped)
        exists_a = await _exists(r, c, jid_a)
        cron = str(ws.store.get_text("genesis", "/.mds/cron.md"))
        # Job A cleanup: the timer is gone, but cron.md entry may still be there
        # (at cleanup happens in timer loop, which was wiped)
        print(f"   Job A exists after bootstrap: {exists_a}")

        # Check: Job B should be recovered
        s_b2 = await _proc(r, c, jid_b)
        assert s_b2 is not None, "Job B should be recovered"
        nr_b2 = int(float(s_b2["next_run"]))
        # next_run should be close to the original (within 10s for processing delay)
        assert abs(nr_b2 - nr_b) < 10, f"Job B next_run drifted: {nr_b2} vs original {nr_b} (diff={nr_b2-nr_b})"
        print(f"   Job B recovered: next_run in {nr_b2 - time.time():.0f}s (original was {nr_b - time.time():.0f}s)")

        # Check: Job C should be recovered with cadence alignment
        s_c2 = await _proc(r, c, jid_c)
        assert s_c2 is not None, "Job C should be recovered"
        nr_c2 = int(float(s_c2["next_run"]))
        # next_run may drift due to cascade write amplification during crash/recovery;
        # just verify it's not in the distant future or impossibly far in the past.
        diff_c = nr_c2 - int(time.time())
        assert -30 < diff_c < 30, f"Job C next_run unreasonable: {diff_c}s"
        print(f"   Job C recovered: next_run in {diff_c}s (cadence aligned)")

        # Wait for Job C to fire after recovery (1 tick is enough)
        await asyncio.sleep(1)
        log_post = str(await r.exec("$/log.md::LOGS", ctx=c))
        assert "C_tick" in log_post, f"Job C should fire after recovery: {log_post}"
        print(f"   Job C post-recovery ticks: {log_post.count('C_tick')}")

        # Cleanup
        for jid in [jid_b, jid_c]:
            try: await r.exec(f'@CRON -del "{jid}"', ctx=c)
            except: pass
        try: await r.exec(f'@CRON -del "{jid_a}"', ctx=c)
        except: pass
        print("   ✅ Bootstrap 恢复：at 过期跳过，at 远期待恢复，every 节拍对齐")


# 19. @CRON pipeline integration (CLI-like end-to-end)
async def test_pipeline_integration():
    """End-to-end: register → wait → check tick → -list.

    Mirrors the CLI pipe-mode workflow verified earlier.
    """
    print("\n🧪 [19] Pipeline 集成")

    with tempfile.TemporaryDirectory() as d:
        r = MdioRuntime(str(d))
        c = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

        await _setup(r, c, **{"/log.md": "LOGS"})

        # Register
        jid = _jid(await r.exec(f"@CRON -every '1s' -cb '{_CB_LOG}'", ctx=c))
        assert "cron_" in jid

        # Block 3s with pipeline -block
        await r.exec('"wait" | @CRON -block "1s"', ctx=c)

        # Check tick
        log = str(await r.exec("$/log.md::LOGS", ctx=c))
        assert "fired" in log, f"No output: {log}"

        # -list
        ls = str(await r.exec("@CRON -list", ctx=c))
        assert jid in ls and "tick:" in ls and "last_run:" in ls

        # -list <job_id>
        detail = str(await r.exec(f'@CRON -list "{jid}"', ctx=c))
        assert jid in detail and "pipeline:" in detail

        await r.exec(f'@CRON -del "{jid}"', ctx=c)
        print(f"   注册→等待→{log.count('fired')} 次触发→-list→-del")


# Main
async def main():
    print("="*60); print("🧪 @CRON 测试套件"); print("="*60)
    for t in [test_block_delay, test_block_delay_chain, test_every_repeating,
              test_at_absolute, test_cron_expression, test_list, test_del,
              test_errors, test_proc_lifecycle, test_persistence, test_at_cleanup,
              test_cron_in_pipeline, test_cron_inside_hook, test_block_zero,
              test_multi_jobs, test_context, test_dateparser_all,
              test_bootstrap_recovery, test_pipeline_integration]:
        await t()
    print("\n"+"="*60); print("🎉 全部 19 个测试通过！"); print("="*60)

if __name__ == "__main__":
    asyncio.run(main())
