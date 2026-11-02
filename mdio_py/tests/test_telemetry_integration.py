"""Cross-stack integration test: Rust → Python ↔ simulated JS/WASM telemetry.

Mimics a real editing session:
1. Multiple CRDT write + handle_packet (RS_proc stamp)
2. Python layer stamps (PY_dispatch, PY_bcast)
3. Simulated JS/WASM flush via merge_js_data
4. Full report verification
5. Side-effect check: original business logic unchanged
"""

import json
import pytest
from mdio_py import mdio_core as mrc
from mdio_py.telemetry import TelemetrySession, TELEMETRY_COMPILED
from mdio_py.core.selector_builder import SelectorBuilder

pytestmark = pytest.mark.skipif(not TELEMETRY_COMPILED, reason="telemetry not compiled")


@pytest.fixture
def store():
    s = mrc.ProjectStore("/mem")
    s.mk("genesis", "/doc", parents=False, verbose=False)
    return s


class TestFullPipeline:
    """Simulate a full editing session with telemetry."""

    def test_write_read_pipeline(self, store):
        """Write + read + handle_packet → verify RS layer stats + report."""
        sess = TelemetrySession(store, "full_pipeline")
        ir = SelectorBuilder().build()

        # Phase 1: multiple writes with handle_packet
        for i in range(5):
            doc_id, packet = store.write_locator(
                "genesis", f"/doc/file_{i}.md", ir, "+>",
                f"# File {i}\n- status: active\n- count: {i}\n",
                True, True, False, False,
            )
            assert isinstance(doc_id, int), f"write_locator should return int doc_id"
            assert isinstance(packet, bytes), f"write_locator should return bytes packet"
            if packet:
                action = store.handle_packet(bytes(packet))
                # Verify business logic intact: write returns valid CRDT
                assert action is not None, "handle_packet should return action"

        # Phase 2: reads
        read_ir = SelectorBuilder().drill("File 0").build()
        results = store.read_locator("genesis", "/doc/file_0.md", read_ir)
        assert len(results) > 0
        assert "status: active" in results[0]

        # Phase 3: verify RS_proc layer stats exist
        stats = sess.layer_stats()
        rs_samples = [s for s in stats if s["layer_name"] == "RS_proc"]
        assert len(rs_samples) >= 5, f"Expected >=5 RS_proc samples, got {len(rs_samples)}"
        # All RS samples should be edit bucket (UP_SYNC → edit)
        assert all(s["bucket"] == "edit" for s in rs_samples)

        # Phase 4: report includes layer breakdown
        import io, sys
        captured = io.StringIO()
        old = sys.stdout
        sys.stdout = captured
        try:
            sess.report()
        finally:
            sys.stdout = old
        output = captured.getvalue()
        assert "Pipeline Latency (layers)" in output
        assert "RS_proc" in output

    def test_write_with_telemetry_no_change_to_behavior(self, store):
        """Verify write_locator behavior is identical with/without telemetry."""
        # Without session
        ir = SelectorBuilder().build()
        doc_id1, packet1 = store.write_locator(
            "genesis", "/doc/notel.md", ir, "+>", "# NoTel\n", True, True, False, False)

        # With session
        sess = TelemetrySession(store, "behavior_test")
        doc_id2, packet2 = store.write_locator(
            "genesis", "/doc/withtel.md", ir, "+>", "# WithTel\n", True, True, False, False)

        # Both should return valid doc_id and bytes packet
        assert isinstance(doc_id1, int) and isinstance(doc_id2, int)
        assert isinstance(packet1, bytes) and isinstance(packet2, bytes)
        # CRDT header size is 13
        assert len(packet1) >= 13
        assert len(packet2) >= 13
        # doc_ids are different
        assert doc_id1 != doc_id2

        # Read back should work identically
        r1 = store.read_locator("genesis", "/doc/notel.md", ir)
        r2 = store.read_locator("genesis", "/doc/withtel.md", ir)
        assert r1 == ["# NoTel\n"]
        assert r2 == ["# WithTel\n"]

    def test_probe_frame_non_intrusive(self, store):
        """PROBE frame: full routing verification without CRDT interference."""
        sess = TelemetrySession(store, "probe_intrusion")
        ir = SelectorBuilder().build()

        # First, do a normal write to establish baseline
        doc_id, _ = store.write_locator(
            "genesis", "/doc/probe_test.md", ir, "+>",
            "# ProbeTest\n", True, True, False, False)

        # Send PROBE frame (mimics JS tel.probe())
        import struct
        probe = struct.pack('<BIII', 0x30, 0, 0, 0)  # tag=0x30, len=0, peer=0, doc=0
        action = store.handle_packet(probe)

        # ── Routing verification ──
        # 1. PROBE must return Direct action (echo back to sender)
        assert action.is_direct, f"PROBE must be Direct, got {type(action)}"
        # 2. PROBE must NOT broadcast (only sender gets reply)
        assert not action.is_broadcast, "PROBE must NOT broadcast"
        # 3. PROBE must NOT subscribe
        assert not action.is_subscribe, "PROBE must NOT subscribe"

        # 4. Return payload must have tag changed to 0xB0
        reply = bytes(action.data)
        assert reply[0] == 0xB0, f"PROBE reply must be 0xB0, got 0x{reply[0]:02X}"

        # 5. RS_proc must classify PROBE as 'probe' bucket
        stats = sess.layer_stats()
        probe_stats = [s for s in stats if s["bucket"] == "probe"]
        assert len(probe_stats) > 0, f"PROBE frame must be in probe bucket, got {stats}"
        assert probe_stats[0]["tag"] == 0x30

        # 6. Business logic unchanged after PROBE
        result = store.read_locator("genesis", "/doc/probe_test.md", ir)
        assert "# ProbeTest\n" in result

        # 7. Multiple PROBE frames are safe
        for _ in range(5):
            action2 = store.handle_packet(probe)
            assert action2.is_direct
        stats2 = sess.layer_stats()
        assert len([s for s in stats2 if s["bucket"] == "probe"]) == 6  # 1 + 5

    def test_merge_js_with_write_pipeline(self, store):
        """Full cross-stack: writes + handle_packet + JS merge → report."""
        sess = TelemetrySession(store, "cross_stack")
        ir = SelectorBuilder().build()

        # Step 1: business writes
        for i in range(3):
            _, pkt = store.write_locator(
                "genesis", f"/doc/stack_{i}.md", ir, "+>",
                f"# Stack {i}\n", True, True, False, False)
            if pkt:
                store.handle_packet(bytes(pkt))

        # Step 2: merge JS/WASM data
        store.telemetry_merge_js_data(json.dumps({
            "JS": {"send_gap": [32000000, 28000000], "recv_to_render": [8000000]},
            "WASM": {"local_proc": [500000, 300000], "net_proc": [400000, 200000]},
        }))

        # Step 3: verify all layers present
        stats = sess.layer_stats()
        layer_names = {s["layer_name"] for s in stats}
        assert "RS_proc" in layer_names
        assert "JS_send_gap" in layer_names
        assert "JS_recv_to_render" in layer_names
        assert "WASM_local_proc" in layer_names
        assert "WASM_net_proc" in layer_names

        # Step 4: verify dump_json roundtrip
        text = sess.dump_json()
        data = json.loads(text)
        assert len(data) >= 7  # 3 RS + 4 JS/WASM

    def test_report_shows_flush_status(self, store):
        """Report layer section should reflect JS/WASM merge state."""
        # Without JS merge
        s1 = TelemetrySession(store, "no_merge")
        ir = SelectorBuilder().build()
        _, pkt = store.write_locator(
            "genesis", "/doc/flush_test.md", ir, "+>",
            "# Flush\n", True, True, False, False)
        if pkt:
            store.handle_packet(bytes(pkt))

        import io, sys
        out1 = io.StringIO()
        old = sys.stdout
        sys.stdout = out1
        try:
            s1.report()
        finally:
            sys.stdout = old
        assert "JS_send_gap" not in out1.getvalue()
        assert "JS + WASM: pending" in out1.getvalue()

        # With JS merge (new session since report clears)
        s2 = TelemetrySession(store, "with_merge")
        _, pkt = store.write_locator(
            "genesis", "/doc/flush_test2.md", ir, "+>",
            "# Flush2\n", True, True, False, False)
        if pkt:
            store.handle_packet(bytes(pkt))
        store.telemetry_merge_js_data('{"JS":{"send_gap":[1000]}}')

        out2 = io.StringIO()
        sys.stdout = out2
        try:
            s2.report()
        finally:
            sys.stdout = old
        assert "JS_send_gap" in out2.getvalue()
        assert "JS + WASM: ✓" in out2.getvalue()


class TestModes:
    """Verify all telemetry modes work correctly."""

    def test_bench_mode_collects_layer_stats(self, store):
        """Bench mode + writes → layer stats populated + report includes breakdown."""
        sess = TelemetrySession(store, "bench_test", mode="bench")
        ir = SelectorBuilder().build()

        for i in range(10):
            _, pkt = store.write_locator(
                "genesis", f"/doc/bench_{i}.md", ir, "+>",
                f"# B{i}\n", True, True, False, False)
            if pkt:
                store.handle_packet(bytes(pkt))

        stats = sess.layer_stats()
        assert len(stats) > 0, "bench mode should collect layer stats"

        import io, sys
        out = io.StringIO()
        old = sys.stdout
        sys.stdout = out
        try:
            sess.report()
        finally:
            sys.stdout = old
        output = out.getvalue()
        assert "bench ·" in output, f"should contain bench header, got: {output[:200]}"
        assert "Pipeline Latency (layers)" in output, "bench report should include layer breakdown"

    def test_trace_mode_baseline(self, store):
        """Trace mode → span tree + layer breakdown."""
        sess = TelemetrySession(store, "trace_test", mode="trace")
        ir = SelectorBuilder().build()

        _, pkt = store.write_locator(
            "genesis", "/doc/trace.md", ir, "+>",
            "# T\n- k: v\n", True, True, False, False)
        if pkt:
            store.handle_packet(bytes(pkt))

        import io, sys
        out = io.StringIO()
        old = sys.stdout
        sys.stdout = out
        try:
            sess.report()
        finally:
            sys.stdout = old
        output = out.getvalue()
        assert "trace ·" in output
        assert "Pipeline Latency (layers)" in output
        assert "vfs_resolve" in output, "should have span tree"

    def test_no_active_session_silent(self, store):
        """Operations without active session → no crash, business works."""
        ir = SelectorBuilder().build()
        # No TelemetrySession created
        _, pkt = store.write_locator(
            "genesis", "/doc/nosess.md", ir, "+>",
            "# NoSession\n", True, True, False, False)
        assert pkt, "write should work even without telemetry session"
        if pkt:
            action = store.handle_packet(bytes(pkt))
            assert action is not None
        # Read back
        result = store.read_locator("genesis", "/doc/nosess.md", ir)
        assert "# NoSession\n" in result

    def test_report_group_by(self, store):
        """report(group_by='layer_name') → Polars grouped output."""
        sess = TelemetrySession(store, "groupby_test")
        ir = SelectorBuilder().build()

        for i in range(3):
            _, pkt = store.write_locator(
                "genesis", f"/doc/gb_{i}.md", ir, "+>",
                f"# G{i}\n", True, True, False, False)
            if pkt:
                store.handle_packet(bytes(pkt))

        import io, sys
        out = io.StringIO()
        old = sys.stdout
        sys.stdout = out
        try:
            sess.report(group_by="layer_name")
        finally:
            sys.stdout = old
        output = out.getvalue()
        # Polars table or skip message
        assert "layer_name" in output or "polars" in output.lower()


class TestCliReport:
    """MdioRuntime 运行时报告路径 (CLI/脚本统一出口 report_session)."""

    async def test_runtime_telemetry_report_full(self):
        """telemetry='full' auto session → telemetry_report() 出完整报告 + trace mode 头部."""
        from mdio_py import MdioRuntime
        rt = MdioRuntime("/mem", telemetry="full")
        ir = SelectorBuilder().build()
        rt.store.mk("genesis", "/doc", parents=False, verbose=False)
        _, pkt = rt.store.write_locator(
            "genesis", "/doc/cli.md", ir, "+>", "# CLI\n", True, True, False, False)
        if pkt:
            rt.store.handle_packet(bytes(pkt))

        import io, sys
        out = io.StringIO(); old = sys.stdout; sys.stdout = out
        try:
            rt.telemetry_report("cli_report_test")
        finally:
            sys.stdout = old
        output = out.getvalue()
        assert "Pipeline Latency (layers)" in output, f"should have layer section, got: {output[:300]}"
        assert "trace ·" in output, f"auto session is trace mode, got: {output[:200]}"
        assert "saved:" in output

    async def test_runtime_telemetry_report_consumes(self):
        """报告后 session 清空 — 二次调用返回 no data."""
        from mdio_py import MdioRuntime
        rt = MdioRuntime("/mem", telemetry="full")
        ir = SelectorBuilder().build()
        rt.store.mk("genesis", "/doc", parents=False, verbose=False)
        _, pkt = rt.store.write_locator(
            "genesis", "/doc/cli2.md", ir, "+>", "# CLI2\n", True, True, False, False)
        if pkt:
            rt.store.handle_packet(bytes(pkt))

        import io, sys
        out = io.StringIO(); old = sys.stdout; sys.stdout = out
        try:
            rt.telemetry_report("cli_consume_test")
            rt.telemetry_report("cli_consume_test")
        finally:
            sys.stdout = old
        output = out.getvalue()
        assert "no telemetry data" in output
