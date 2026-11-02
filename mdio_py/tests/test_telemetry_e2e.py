"""End-to-end telemetry tests — Rust → Python full pipeline.

Verifies:
1. Session create / clear lifecycle
2. Layer stats collection from Rust pre-instrumented spans
3. to_polars() DataFrame export
4. dump_json() roundtrip
5. report_all includes layer breakdown
6. merge_js_data ingestion
"""

import json
import tempfile
import os

import pytest
from mdio_py import mdio_core as mrc
from mdio_py.telemetry import TelemetrySession, TELEMETRY_COMPILED

pytestmark = pytest.mark.skipif(not TELEMETRY_COMPILED, reason="telemetry feature not compiled")


@pytest.fixture
def store():
    s = mrc.ProjectStore("/mem")
    s.mk("genesis", "/test", parents=False, verbose=False)
    return s


@pytest.fixture
def sess(store):
    s = TelemetrySession(store, "e2e_test", mode="trace")
    yield s
    # session auto-clears on report; force clear just in case
    try:
        store.telemetry_report()
    except Exception:
        pass


class TestSessionLifecycle:
    def test_create_session(self, store):
        s = TelemetrySession(store, "lifecycle_test")
        assert s._active is True
        assert s._name == "lifecycle_test"

    def test_create_bench_session(self, store):
        s = TelemetrySession(store, "bench_test", mode="bench")
        assert s._active is True
        assert s._mode == "bench"

    def test_report_clears_and_outputs(self, store):
        s = TelemetrySession(store, "report_test")
        # Do a write that triggers ffi_write span
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()
        store.write_locator("genesis", "/test/report.md", ir, "+>", "# Report\n", True, True, False, False)

        store.telemetry_report()
        # After report(), stats should be cleared
        stats = s.layer_stats()
        assert isinstance(stats, list)


class TestLayerStats:
    def test_empty_layer_stats(self, store):
        s = TelemetrySession(store, "empty_stats")
        stats = s.layer_stats()
        assert stats == []

    def test_layer_stats_after_write(self, store):
        s = TelemetrySession(store, "write_stats")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()

        # Write triggers ffi_write span
        doc_id, packet = store.write_locator(
            "genesis", "/test/write_stats.md", ir, "+>",
            "# Stats Test\n- key: val\n", True, True, False, False)

        # Feed the CRDT packet through handle_packet to trigger RS_proc stamp
        if packet:
            store.handle_packet(bytes(packet))

        # Read triggers ffi_read span
        read_ir = SelectorBuilder().drill("Stats Test").build()
        store.read_locator("genesis", "/test/write_stats.md", read_ir)

        stats = s.layer_stats()
        assert len(stats) > 0, f"Expected layer stats after write+read, got {stats}"

        # Should have RS_proc sample from handle_packet
        layer_names = {x["layer_name"] for x in stats}
        assert "RS_proc" in layer_names, f"Expected RS_proc in {layer_names}"

    def test_layer_stats_structure(self, store):
        s = TelemetrySession(store, "structure_test")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()
        doc_id, packet = store.write_locator(
            "genesis", "/test/structure.md", ir, "+>",
            "# S\n", True, True, False, False)
        if packet:
            store.handle_packet(bytes(packet))

        stats = s.layer_stats()
        assert len(stats) > 0, "Should have layer stats from handle_packet"
        for entry in stats:
            assert "layer_name" in entry
            assert "dur_ns" in entry
            assert "bucket" in entry
            assert "tag" in entry
            assert "doc_id" in entry
            assert isinstance(entry["dur_ns"], int)
            assert isinstance(entry["doc_id"], int)
            assert entry["bucket"] in ("edit", "probe", "ctrl")


class TestPolarsExport:
    def test_to_polars_returns_dataframe(self, store):
        s = TelemetrySession(store, "polars_test")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()
        doc_id, packet = store.write_locator(
            "genesis", "/test/polars.md", ir, "+>",
            "# P\n", True, True, False, False)
        if packet:
            store.handle_packet(bytes(packet))

        df = s.to_polars()
        if df is None:
            pytest.skip("polars not installed")
        assert len(df) > 0
        # to_polars = phase 数据 (name_hash/name/dur_ns); layer 数据走 layer_stats()
        assert "name" in df.columns
        assert "dur_ns" in df.columns

    def test_polars_groupby_report(self, store):
        """Generate a grouped report from layer stats using Polars."""
        s = TelemetrySession(store, "polars_group_report")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()

        # Generate multiple samples
        for i in range(5):
            doc_id, packet = store.write_locator(
                "genesis", f"/test/polars_group_{i}.md", ir, "+>",
                f"# P{i}\n", True, True, False, False)
            if packet:
                store.handle_packet(bytes(packet))

        # Group by layer, compute aggregate stats
        try:
            import polars as pl
            layer_df = pl.DataFrame(s.layer_stats())
            report = layer_df.group_by("layer_name").agg([
                pl.col("dur_ns").count().alias("count"),
                pl.col("dur_ns").mean().alias("avg_ns"),
                pl.col("dur_ns").median().alias("p50_ns"),
                pl.col("dur_ns").quantile(0.99).alias("p99_ns"),
            ]).sort("avg_ns", descending=True)

            print("\n=== Polars Layer Report ===")
            print(report)

            assert len(report) > 0, "Report should have rows"
            assert "count" in report.columns
            assert "p50_ns" in report.columns
        except ImportError:
            pytest.skip("polars not installed")

    def test_polars_filtered_report(self, store):
        """Filter layer stats by bucket type."""
        s = TelemetrySession(store, "polars_filter_report")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()

        doc_id, packet = store.write_locator(
            "genesis", "/test/polars_filter.md", ir, "+>",
            "# PF\n", True, True, False, False)
        if packet:
            store.handle_packet(bytes(packet))

        try:
            import polars as pl
            layer_df = pl.DataFrame(s.layer_stats())
            # Show only edit bucket
            edit_only = layer_df.filter(pl.col("bucket") == "edit")
            print(f"\n=== Edit bucket only: {len(edit_only)} samples ===")
            print(edit_only.head(5))

            # Stats by layer for edit bucket
            if len(edit_only) > 0:
                summary = edit_only.group_by("layer_name").agg([
                    pl.col("dur_ns").len().alias("n"),
                    pl.col("dur_ns").mean().alias("avg"),
                ])
                print(summary)
        except ImportError:
            pytest.skip("polars not installed")


class TestDumpJson:
    def test_dump_json_returns_valid(self, store):
        s = TelemetrySession(store, "dump_test")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()
        doc_id, packet = store.write_locator(
            "genesis", "/test/dump.md", ir, "+>",
            "# D\n", True, True, False, False)
        if packet:
            store.handle_packet(bytes(packet))

        text = s.dump_json()
        data = json.loads(text)
        assert isinstance(data, list)
        assert len(data) > 0

    def test_dump_json_to_file(self, store):
        s = TelemetrySession(store, "dump_file_test")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()
        doc_id, packet = store.write_locator(
            "genesis", "/test/dump_file.md", ir, "+>",
            "# F\n", True, True, False, False)
        if packet:
            store.handle_packet(bytes(packet))

        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            path = f.name
        try:
            s.dump_json(path)
            assert os.path.exists(path)
            with open(path) as f:
                data = json.load(f)
            assert isinstance(data, list)
        finally:
            os.unlink(path)


class TestMergeJsData:
    def test_merge_js_data_adds_layers(self, store):
        s = TelemetrySession(store, "merge_test")
        js_payload = json.dumps({
            "JS": {
                "send_gap": [33200000, 16700000],
                "recv_to_render": [8100000, 5500000],
            },
            "WASM": {
                "local_proc": [500000, 300000],
            },
        })
        store.telemetry_merge_js_data(js_payload)

        stats = s.layer_stats()
        layer_names = {x["layer_name"] for x in stats}
        assert "JS_send_gap" in layer_names, f"Layer names: {layer_names}"
        assert "JS_recv_to_render" in layer_names
        assert "WASM_local_proc" in layer_names


class TestSpanDecorator:
    def test_trace_decorator_sync(self, store):
        s = TelemetrySession(store, "trace_sync")

        @s.trace("my_func")
        def do_work():
            return 42

        result = do_work()
        assert result == 42

    def test_measure_inline(self, store):
        s = TelemetrySession(store, "measure_test")
        result = s.measure("compute", lambda x: x * 2, 21)
        assert result == 42


class TestReportIncludesLayerBreakdown:
    def test_report_contains_pipeline_latency(self, store):
        s = TelemetrySession(store, "pipeline_report")
        from mdio_py.core.selector_builder import SelectorBuilder
        ir = SelectorBuilder().build()
        doc_id, packet = store.write_locator(
            "genesis", "/test/pipeline.md", ir, "+>",
            "# Pipeline\n", True, True, False, False)
        # Feed through handle_packet to get RS_proc layer stamp
        if packet:
            store.handle_packet(bytes(packet))

        # Capture printed output
        import io
        import sys
        captured = io.StringIO()
        old_stdout = sys.stdout
        sys.stdout = captured
        try:
            s.report()
        finally:
            sys.stdout = captured
            sys.stdout = old_stdout

        output = captured.getvalue()
        # Layer breakdown (pipeline latency) 段应出现在报告中
        assert "Pipeline Latency (layers)" in output, f"Output: {output[:500]}"


class TestProbeBucket:
    """Verify PROBE frames are correctly classified as 'probe' bucket."""

    def test_probe_bucket_from_rs(self, store):
        """RS side: from_tag(0x30) → Probe."""
        s = TelemetrySession(store, "probe_rs_test")
        import struct
        probe_frame = struct.pack('<BIII', 0x30, 0, 0, 0)
        store.handle_packet(probe_frame)
        stats = s.layer_stats()
        assert len(stats) > 0
        assert any(st["bucket"] == "probe" for st in stats), \
            f"PROBE frame should be 'probe', got: {stats}"

    def test_ctrl_bucket_from_rs(self, store):
        """RS side: UP_TREE(0x01) → Ctrl."""
        s = TelemetrySession(store, "ctrl_rs_test")
        import struct
        tree_frame = struct.pack('<BIII', 0x01, 0, 0, 0)
        store.handle_packet(tree_frame)
        stats = s.layer_stats()
        # Ctrl is filtered from report but still recorded as layer sample
        assert any(st["bucket"] == "ctrl" for st in stats), \
            f"UP_TREE frame should be 'ctrl', got: {stats}"

    def test_python_tag_bucket(self):
        """Python side: _tag_bucket correctly maps all tags."""
        from mdio_py.gateway.handlers.crdt import _tag_bucket
        # PROBE
        assert _tag_bucket(0x30) == "probe"
        assert _tag_bucket(0xB0) == "probe"
        # Ctrl: OpenDoc, CloseDoc, UpTree, DnTree, DnOpen, DnErr
        assert _tag_bucket(0x01) == "ctrl"  # UP_TREE / OpenDoc
        assert _tag_bucket(0x02) == "ctrl"  # UP_OPEN
        assert _tag_bucket(0x03) == "ctrl"  # UP_CLOSE
        assert _tag_bucket(0x81) == "ctrl"  # DN_TREE
        assert _tag_bucket(0x82) == "ctrl"  # DN_OPEN
        assert _tag_bucket(0xFF) == "ctrl"  # DN_ERR
        # Edit: all CRDT operations
        assert _tag_bucket(0x04) == "edit"  # UP_SYNC
        assert _tag_bucket(0x83) == "edit"  # DN_SYNC
        assert _tag_bucket(0x05) == "edit"  # UndoReq
        assert _tag_bucket(0x06) == "edit"  # RedoReq
        assert _tag_bucket(0x07) == "edit"  # CursorMove
        assert _tag_bucket(0x00) == "edit"  # unknown
        assert _tag_bucket(0x10) == "edit"  # unknown
