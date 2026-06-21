"""
Native streaming binding tests (PyO3 module ``ggstreamlog_native``). Skipped if the wheel isn't
installed (build it: ``maturin build --release`` in libs/rust-streamlog/bindings/python, then
``pip install`` the wheel). Buffer-only — no AWS needed. Mirrors the Java/Rust streaming tests.
"""
import json
import time

import pytest

pytest.importorskip("ggstreamlog_native", reason="ggstreamlog_native wheel not installed")

# ggsl_status codes.
ERR_CONFIG = 1
ERR_UNKNOWN_STREAM = 5


def _config(tmp_path):
    path = str(tmp_path / "telemetry").replace("\\", "/")
    return json.dumps({
        "streams": [{
            "name": "telemetry",
            "sink": {"type": "kinesis", "streamName": "x"},
            "buffer": {"path": path, "segmentBytes": 65536,
                       "maxDiskBytes": 1073741824, "onFull": "block"},
        }]
    })


def test_open_append_flush_stats(tmp_path):
    from ggcommons.streaming import StreamService

    with StreamService.open(_config(tmp_path)) as svc, svc.stream("telemetry") as h:
        for i in range(1000):
            h.append("pump-7", 1000 + i, f"reading-{i}".encode("utf-8"))
        h.flush()
        s = svc.stats("telemetry")
        assert s.appended_total == 1000
        assert s.next_offset == 1000
        assert s.backlog == 1000          # buffer-only: nothing exported
        assert s.dropped_total == 0       # block policy never drops
        assert s.disk_bytes > 0


def test_unknown_stream(tmp_path):
    from ggcommons.streaming import GgStreamError, StreamService

    with StreamService.open(_config(tmp_path)) as svc:
        with pytest.raises(GgStreamError) as ei:
            svc.stats("does-not-exist")
        assert ei.value.code == ERR_UNKNOWN_STREAM


def test_bad_config():
    from ggcommons.streaming import GgStreamError, StreamService

    with pytest.raises(GgStreamError) as ei:
        StreamService.open("{ not valid json")
    assert ei.value.code == ERR_CONFIG


def test_stream_names(tmp_path):
    from ggcommons.streaming import StreamService

    assert StreamService.stream_names(_config(tmp_path)) == ["telemetry"]


def test_metrics_bridge_defines_and_emits(tmp_path, monkeypatch):
    from ggcommons.metrics.metric_emitter import MetricEmitter
    from ggcommons.streaming import StreamMetricsBridge, StreamService

    defined = []
    emitted = []
    monkeypatch.setattr(MetricEmitter, "define_metric", staticmethod(lambda m: defined.append(m)))
    monkeypatch.setattr(MetricEmitter, "emit_metric", staticmethod(lambda n, v: emitted.append((n, v))))

    class StubConfig:
        def get_thing_name(self):
            return "thing"

        def get_component_name(self):
            return "comp"

    with StreamService.open(_config(tmp_path)) as svc, svc.stream("telemetry") as h:
        for i in range(10):
            h.append("k", 1000 + i, b"v")
        h.flush()
        bridge = StreamMetricsBridge(StubConfig(), svc, ["telemetry"], interval_secs=1)
        try:
            assert len(defined) == 1
            deadline = time.time() + 5
            while not emitted and time.time() < deadline:
                time.sleep(0.1)
            assert emitted, "bridge should emit at least once"
            name, values = emitted[0]
            assert name == "stream:telemetry"
            assert "backlog" in values and "diskBytes" in values
        finally:
            bridge.close()
