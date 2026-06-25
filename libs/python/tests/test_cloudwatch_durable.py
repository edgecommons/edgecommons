"""
Tests for the durable store-and-forward CloudWatch metric buffer (ggstreamlog host-callback sink).

Covers:
  * record round-trip (serialize/deserialize) and stale pre-filter window,
  * 1000/1 MB chunking,
  * the drain outcome mapping (AllAcked / Partial-on-send-failure / stale-drop counter),
  * a full disconnect fault-injection integration test against the REAL native export engine with a
    MOCKED boto3 client: sever the cloud -> flat memory + disk backlog -> drain on reconnect ->
    nonzero dropped_stale once the accept window is exceeded,
  * the CloudWatch target's durable path selection (emit -> append -> drain -> put_metric_data).

The native ``ggstreamlog_native`` wheel must be installed (built from libs/rust-streamlog via
maturin). Tests that need it skip cleanly when it is absent.
"""
import json
import tempfile
import threading
import time

import pytest

from ggcommons.metrics.targets.cloudwatch_durable import (
    CloudWatchDrain,
    chunk_datums,
    deserialize_record,
    serialize_datum,
    _is_stale,
    _STALE_PAST_SECS,
    _STALE_FUTURE_SECS,
)

try:
    from ggcommons.streaming.service import StreamService
    import ggstreamlog_native  # noqa: F401
    _HAVE_NATIVE = True
except Exception:  # pragma: no cover - only when the wheel is absent
    _HAVE_NATIVE = False

native_required = pytest.mark.skipif(not _HAVE_NATIVE, reason="ggstreamlog_native wheel not installed")


# --------------------------------------------------------------------------- fakes


class FakeCloudWatchClient:
    """A minimal stand-in for the boto3 cloudwatch client.

    Records every put_metric_data call; can be configured to fail (raise) for a number of calls to
    simulate a cloud disconnect, then recover.
    """

    def __init__(self):
        self.calls = []  # list of (namespace, datums)
        self.fail_event = threading.Event()  # set => raise on put_metric_data (disconnected)
        self._lock = threading.Lock()

        class _Meta:
            region_name = "us-east-1"

        self.meta = _Meta()

    def sever(self):
        self.fail_event.set()

    def reconnect(self):
        self.fail_event.clear()

    def put_metric_data(self, Namespace=None, MetricData=None):
        if self.fail_event.is_set():
            raise RuntimeError("simulated CloudWatch disconnect")
        with self._lock:
            self.calls.append((Namespace, list(MetricData)))
        return {}

    def total_datums(self):
        with self._lock:
            return sum(len(d) for _, d in self.calls)

    def namespaces(self):
        with self._lock:
            return [ns for ns, _ in self.calls]


def make_datum(name="m", value=1.0, ts=None, unit="Count"):
    return {
        "MetricName": name,
        "Dimensions": [{"Name": "component", "Value": "c"}],
        "Timestamp": ts if ts is not None else time.time(),
        "Value": value,
        "Unit": unit,
        "StorageResolution": 60,
    }


# --------------------------------------------------------------------------- unit: serialize


class TestRecordRoundTrip:
    def test_serialize_then_deserialize_preserves_namespace_and_datum(self):
        ts = 1_700_000_000.0
        payload = serialize_datum("App/NS", make_datum("cpu", 42.0, ts=ts))
        ns, datum = deserialize_record(payload)
        assert ns == "App/NS"
        assert datum["MetricName"] == "cpu"
        assert datum["Value"] == 42.0
        assert datum["Timestamp"] == ts

    def test_serialize_is_compact_json(self):
        payload = serialize_datum("NS", make_datum())
        # compact => no spaces after separators
        assert b", " not in payload and b": " not in payload

    def test_serialize_normalizes_datetime_timestamp(self):
        from datetime import datetime, timezone

        dt = datetime(2023, 11, 14, 22, 13, 20, tzinfo=timezone.utc)
        payload = serialize_datum("NS", make_datum(ts=dt))
        _, datum = deserialize_record(payload)
        assert datum["Timestamp"] == pytest.approx(dt.timestamp())

    def test_serialize_fills_missing_timestamp(self):
        d = make_datum()
        del d["Timestamp"]
        before = time.time()
        _, datum = deserialize_record(serialize_datum("NS", d))
        assert datum["Timestamp"] >= before


# --------------------------------------------------------------------------- unit: stale window


class TestStaleWindow:
    def test_fresh_datum_is_not_stale(self):
        assert not _is_stale(make_datum(ts=time.time()), time.time())

    def test_too_old_is_stale(self):
        now = time.time()
        assert _is_stale(make_datum(ts=now - _STALE_PAST_SECS - 60), now)

    def test_too_far_future_is_stale(self):
        now = time.time()
        assert _is_stale(make_datum(ts=now + _STALE_FUTURE_SECS + 60), now)

    def test_edge_just_inside_window_is_not_stale(self):
        now = time.time()
        assert not _is_stale(make_datum(ts=now - _STALE_PAST_SECS + 60), now)
        assert not _is_stale(make_datum(ts=now + _STALE_FUTURE_SECS - 60), now)

    def test_missing_timestamp_not_stale(self):
        d = make_datum()
        del d["Timestamp"]
        assert not _is_stale(d, time.time())


# --------------------------------------------------------------------------- unit: chunking


class TestChunking:
    def test_chunks_by_datum_count(self):
        datums = [make_datum(name=f"m{i}") for i in range(2500)]
        chunks = chunk_datums(datums)
        assert len(chunks) == 3
        assert [len(c) for c in chunks] == [1000, 1000, 500]

    def test_chunks_by_byte_size(self):
        # Big datums (large dimension blobs) trip the ~1 MB cap well before 1000 datums.
        big = []
        for i in range(50):
            d = make_datum(name=f"big{i}")
            d["Dimensions"] = [{"Name": "k", "Value": "x" * 30000}]
            big.append(d)
        chunks = chunk_datums(big)
        assert len(chunks) > 1
        assert all(len(c) <= 1000 for c in chunks)

    def test_empty_input(self):
        assert chunk_datums([]) == []

    def test_single_oversized_datum_still_emitted(self):
        d = make_datum()
        d["Dimensions"] = [{"Name": "k", "Value": "y" * (2 * 1024 * 1024)}]
        chunks = chunk_datums([d])
        assert len(chunks) == 1 and len(chunks[0]) == 1


# --------------------------------------------------------------------------- unit: drain outcome


class TestDrainOutcomeMapping:
    def _records(self, items):
        # items: list of (offset, namespace, datum) -> the (offset, pk, ts_ms, payload) shape.
        return [
            (off, ns.encode(), int(d["Timestamp"] * 1000), serialize_datum(ns, d))
            for off, ns, d in items
        ]

    def test_all_acked_returns_none(self):
        client = FakeCloudWatchClient()
        drain = CloudWatchDrain(client)
        recs = self._records([(0, "NS", make_datum("a")), (1, "NS", make_datum("b"))])
        assert drain.drain_batch(recs) is None
        assert client.total_datums() == 2

    def test_groups_by_namespace_one_call_each(self):
        client = FakeCloudWatchClient()
        drain = CloudWatchDrain(client)
        recs = self._records([
            (0, "NS1", make_datum("a")),
            (1, "NS2", make_datum("b")),
            (2, "NS1", make_datum("c")),
        ])
        assert drain.drain_batch(recs) is None
        assert sorted(client.namespaces()) == ["NS1", "NS2"]

    def test_send_failure_returns_failed_offsets(self):
        client = FakeCloudWatchClient()
        client.sever()
        drain = CloudWatchDrain(client)
        recs = self._records([(5, "NS", make_datum("a")), (6, "NS", make_datum("b"))])
        out = drain.drain_batch(recs)
        assert sorted(out) == [5, 6]  # both retried
        assert drain.dropped_stale == 0

    def test_partial_failure_per_namespace_isolation(self):
        # NS_BAD lives in a namespace whose put fails; NS_OK succeeds. Only NS_BAD offsets retry.
        class SelectiveClient(FakeCloudWatchClient):
            def put_metric_data(self, Namespace=None, MetricData=None):
                if Namespace == "NS_BAD":
                    raise RuntimeError("nope")
                return super().put_metric_data(Namespace=Namespace, MetricData=MetricData)

        client = SelectiveClient()
        drain = CloudWatchDrain(client)
        recs = self._records([
            (0, "NS_OK", make_datum("a")),
            (1, "NS_BAD", make_datum("b")),
            (2, "NS_OK", make_datum("c")),
        ])
        out = drain.drain_batch(recs)
        assert out == [1]
        assert client.total_datums() == 2  # the two NS_OK datums went through

    def test_stale_datums_dropped_and_counted_not_sent(self):
        client = FakeCloudWatchClient()
        drain = CloudWatchDrain(client)
        now = time.time()
        recs = self._records([
            (0, "NS", make_datum("fresh", ts=now)),
            (1, "NS", make_datum("ancient", ts=now - _STALE_PAST_SECS - 100)),
        ])
        out = drain.drain_batch(recs)
        assert out is None  # stale offset is committed (dropped), fresh one sent
        assert drain.dropped_stale == 1
        assert client.total_datums() == 1

    def test_undeserializable_record_dropped_not_wedged(self):
        client = FakeCloudWatchClient()
        drain = CloudWatchDrain(client)
        recs = [(0, b"NS", 123, b"not-json{")]
        out = drain.drain_batch(recs)
        assert out is None
        assert drain.dropped_stale == 1

    def test_stats_snapshot(self):
        drain = CloudWatchDrain(FakeCloudWatchClient())
        s = drain.stats()
        assert s["dropped_stale"] == 0 and s["last_error"] is None


# --------------------------------------------------------------------------- config parsing


class TestMetricConfigBufferParsing:
    """The real MetricConfiguration must expose the cloudwatch `buffer` block."""

    def _cfg(self, target_config):
        from ggcommons.config.metric_config import MetricConfiguration

        return MetricConfiguration({"target": "cloudwatch", "targetConfig": target_config})

    def test_nested_cloudwatch_buffer(self):
        mc = self._cfg({"cloudwatch": {"intervalSecs": 60, "buffer": {"type": "durable",
                                                                       "maxDiskBytes": 123}}})
        b = mc.get_cloudwatch_buffer()
        assert b == {"type": "durable", "maxDiskBytes": 123}
        assert mc.get_interval_secs() == 60

    def test_flat_buffer(self):
        mc = self._cfg({"intervalSecs": 30, "buffer": {"type": "memory"}})
        assert mc.get_cloudwatch_buffer() == {"type": "memory"}

    def test_no_buffer(self):
        mc = self._cfg({"intervalSecs": 10})
        assert mc.get_cloudwatch_buffer() is None

    def test_non_dict_buffer_ignored(self):
        mc = self._cfg({"buffer": "nope"})
        assert mc.get_cloudwatch_buffer() is None


# --------------------------------------------------------------------------- integration (native)


@native_required
class TestDurableIntegration:
    def _open(self, client, tmpdir, max_disk=1 << 20, poll_ms=20):
        drain = CloudWatchDrain(client)
        cfg = {
            "streams": [{
                "name": "metrics-cw",
                "sink": {"type": "callback"},
                "buffer": {
                    "type": "disk",
                    "path": f"{tmpdir}/cw",
                    "segmentBytes": 65536,
                    "maxDiskBytes": max_disk,
                    "onFull": "dropOldest",
                    "fsync": "perBatch",
                },
                "delivery": {"maxRetries": -1, "pollIntervalMs": poll_ms, "backoffBaseMs": 20,
                             "backoffMaxMs": 200},
                "batch": {"maxRecords": 1000, "maxBytes": 900 * 1024, "maxLatencyMs": 100},
            }]
        }
        svc = StreamService.open_with_callback(json.dumps(cfg), drain.drain_batch)
        return svc, drain

    def _append(self, handle, namespace, datum):
        handle.append(namespace, int(datum["Timestamp"] * 1000), serialize_datum(namespace, datum))

    def test_happy_path_drains_to_cloud(self):
        client = FakeCloudWatchClient()
        with tempfile.TemporaryDirectory() as d:
            svc, drain = self._open(client, d)
            h = svc.stream("metrics-cw")
            for i in range(20):
                self._append(h, "App/NS", make_datum(f"m{i}", ts=time.time()))
            deadline = time.time() + 5
            while svc.stats("metrics-cw").exported_total < 20 and time.time() < deadline:
                time.sleep(0.02)
            assert svc.stats("metrics-cw").exported_total == 20
            assert client.total_datums() == 20
            svc.close()

    def test_disconnect_fault_injection_backlog_then_drain_and_stale_drop(self):
        """Headline acceptance: sever the cloud for an extended period, assert the backlog
        accumulates on disk (memory stays flat — nothing held in Python), then reconnect and assert
        a clean drain. Datums whose timestamp ages past the accept window are dropped + counted."""
        client = FakeCloudWatchClient()
        with tempfile.TemporaryDirectory() as d:
            svc, drain = self._open(client, d)
            h = svc.stream("metrics-cw")

            # --- SEVER: cloud is down. Append a fresh datum and one already-stale datum.
            client.sever()
            now = time.time()
            for i in range(30):
                self._append(h, "App/NS", make_datum(f"fresh{i}", ts=now))
            # This datum is already older than the accept window: it can never be sent and must be
            # dropped (counted) on drain rather than wedging the stream forever.
            self._append(h, "App/NS", make_datum("ancient", ts=now - _STALE_PAST_SECS - 600))
            h.flush()

            # While severed: the engine retries forever; nothing reaches the cloud, backlog builds
            # on disk, and Python memory does not grow (no in-Python pending queue).
            time.sleep(0.4)
            stats = svc.stats("metrics-cw")
            assert client.total_datums() == 0, "nothing should reach the cloud while severed"
            assert stats.backlog > 0, "records must accumulate in the durable buffer"
            assert stats.disk_bytes > 0, "backlog must be on disk, not in memory"

            # --- RECONNECT: the engine drains the backlog.
            client.reconnect()
            deadline = time.time() + 8
            # 30 fresh datums should drain; the 1 stale datum is dropped (committed) not sent.
            while svc.stats("metrics-cw").exported_total < 31 and time.time() < deadline:
                time.sleep(0.05)
            final = svc.stats("metrics-cw")
            assert client.total_datums() == 30, "all fresh datums drain on reconnect"
            assert drain.dropped_stale >= 1, "the aged-out datum is dropped + counted"
            assert final.backlog == 0, "buffer drains clean on reconnect"
            svc.close()

    def test_dropoldest_bounds_disk_under_lengthy_disconnect(self):
        """With the cloud severed and a tiny disk budget, onFull=dropOldest must bound disk usage
        (the lengthy-disconnect / disk-bounded-backlog guarantee)."""
        client = FakeCloudWatchClient()
        client.sever()
        with tempfile.TemporaryDirectory() as d:
            # Small budget so the flood forces dropOldest.
            svc, drain = self._open(client, d, max_disk=256 * 1024, poll_ms=1000)
            h = svc.stream("metrics-cw")
            now = time.time()
            for i in range(5000):
                self._append(h, "App/NS", make_datum(f"m{i}", ts=now))
            h.flush()
            s = svc.stats("metrics-cw")
            assert s.dropped_total > 0, "a tiny budget under disconnect must drop oldest"
            assert s.disk_bytes <= 256 * 1024 + 65536, "disk stays bounded by the budget (+1 segment)"
            assert client.total_datums() == 0
            svc.close()


# --------------------------------------------------------------------------- target durable path


class _FakeMetricConfig:
    def __init__(self, buffer):
        self._buffer = buffer

    def get_namespace(self):
        return "App/Default"

    def get_interval_secs(self):
        return 1

    def get_large_fleet_workaround(self):
        return False

    def get_cloudwatch_buffer(self):
        return self._buffer


class _FakeConfigManager:
    def __init__(self, buffer):
        self._mc = _FakeMetricConfig(buffer)

    def get_metric_config(self):
        return self._mc

    def resolve_template(self, template):
        return template.replace("{ComponentName}", "TestComp").replace("{ThingName}", "TestThing")


class _FakeMeasure:
    def get_unit(self):
        return "Count"

    def get_storage_resolution(self):
        return 60


class _FakeMetric:
    def __init__(self, namespace=None):
        self._ns = namespace

    def get_name(self):
        return "metric"

    def get_namespace(self):
        return self._ns

    def get_measure(self, name):
        return _FakeMeasure()

    def dimensions_as_collection(self, large):
        return [{"Name": "component", "Value": "c"}]


@native_required
class TestCloudWatchTargetDurablePath:
    def _make_target(self, monkeypatch, tmpdir, fake_client):
        import ggcommons.metrics.targets.cloudwatch as cw

        monkeypatch.setattr(cw.boto3, "client", lambda *a, **k: fake_client)
        buffer = {
            "type": "durable",
            "path": f"{tmpdir}/{{ComponentName}}/cw",
            "maxDiskBytes": 1 << 20,
            "onFull": "dropOldest",
            "fsync": "perBatch",
        }
        cm = _FakeConfigManager(buffer)
        return cw.CloudWatch(cm)

    def test_emit_appends_and_drains_via_target(self, monkeypatch):
        with tempfile.TemporaryDirectory() as d:
            client = FakeCloudWatchClient()
            target = self._make_target(monkeypatch, d, client)
            assert target._durable is True
            for i in range(10):
                target.emit_metric(_FakeMetric(), {"metric": float(i)})
            deadline = time.time() + 5
            while client.total_datums() < 10 and time.time() < deadline:
                time.sleep(0.02)
            assert client.total_datums() == 10
            # Self-observability surface is available and consistent.
            ds = target.get_durable_stats()
            assert ds is not None and ds["exported_total"] == 10
            target.close()

    def test_emit_now_appends_and_flushes(self, monkeypatch):
        with tempfile.TemporaryDirectory() as d:
            client = FakeCloudWatchClient()
            target = self._make_target(monkeypatch, d, client)
            target.emit_metric_now(_FakeMetric("App/Explicit"), {"metric": 7.0})
            deadline = time.time() + 5
            while client.total_datums() < 1 and time.time() < deadline:
                time.sleep(0.02)
            assert client.namespaces() == ["App/Explicit"]
            target.close()

    def test_path_template_resolved(self, monkeypatch):
        import os
        with tempfile.TemporaryDirectory() as d:
            client = FakeCloudWatchClient()
            target = self._make_target(monkeypatch, d, client)
            # The {ComponentName} segment must have been resolved on disk.
            assert os.path.isdir(os.path.join(d, "TestComp", "cw"))
            target.close()

    def test_on_configuration_change_ignored_for_durable(self, monkeypatch):
        with tempfile.TemporaryDirectory() as d:
            client = FakeCloudWatchClient()
            target = self._make_target(monkeypatch, d, client)
            assert target.on_configuration_change({}) is True  # no-op, must not touch flush thread
            target.close()

    def test_close_handles_flush_error(self, monkeypatch):
        with tempfile.TemporaryDirectory() as d:
            client = FakeCloudWatchClient()
            target = self._make_target(monkeypatch, d, client)

            class _BadHandle:
                def flush(self):
                    raise RuntimeError("disk gone")

            target._stream_handle = _BadHandle()
            target.close()  # must swallow the flush error and still stop the engine
            assert target._stream_service is None

    def test_close_persists_backlog_without_draining(self, monkeypatch):
        with tempfile.TemporaryDirectory() as d:
            client = FakeCloudWatchClient()
            client.sever()  # cloud down: emits buffer, never drain
            target = self._make_target(monkeypatch, d, client)
            for i in range(5):
                target.emit_metric(_FakeMetric(), {"metric": float(i)})
            target._stream_handle.flush()
            target.close()  # flush-to-disk + stop engine, no cloud drain
            assert client.total_datums() == 0
            # Reopen: the persisted backlog is recovered and (cloud now up) drains.
            client.reconnect()
            target2 = self._make_target(monkeypatch, d, client)
            deadline = time.time() + 5
            while client.total_datums() < 5 and time.time() < deadline:
                time.sleep(0.02)
            assert client.total_datums() == 5
            target2.close()


class TestCloudWatchTargetMemoryPathUnaffected:
    """Only explicit buffer.type=memory keeps the legacy in-memory batching path."""

    def test_memory_buffer_uses_inmemory_path(self):
        import ggcommons.metrics.targets.cloudwatch as cw

        cm = _FakeConfigManager({"type": "memory"})
        # boto3 client is real here but never called (we don't flush); just assert path selection.
        target = cw.CloudWatch.__new__(cw.CloudWatch)
        # Drive __init__ via the fake config manager but with a stub client to avoid AWS.
        import unittest.mock as mock
        with mock.patch.object(cw.boto3, "client", return_value=FakeCloudWatchClient()):
            target.__init__(cm)
        assert target._durable is False
        assert target._flush_thread is not None
        target.close()


class TestCloudWatchTargetDefaultsToDurable:
    """The cloudwatch target defaults to the durable buffer when no buffer block is configured —
    parity with the Java/TS targets and the schema default. An ABSENT native core fails fast (the
    core is bundled by design); a buffer-OPEN failure when the core IS present (e.g. a bad path)
    degrades gracefully to in-memory batching."""

    def test_no_buffer_section_defaults_to_durable(self, monkeypatch):
        import ggcommons.metrics.targets.cloudwatch as cw
        import ggcommons.streaming.service as svc

        # Mock the durable init so the test is hermetic (no disk needed) and force the native core
        # "present" so the absent-core guard passes; asserts only the selection: an absent buffer
        # block must request the durable path with the durable defaults (an empty buffer dict).
        seen = {}

        def fake_init_durable(self, buffer):
            seen["buffer"] = buffer
            self._durable = True

        monkeypatch.setattr(svc, "native_available", lambda: True)
        monkeypatch.setattr(cw.CloudWatch, "_init_durable", fake_init_durable)
        monkeypatch.setattr(cw.boto3, "client", lambda *a, **k: FakeCloudWatchClient())

        target = cw.CloudWatch(_FakeConfigManager(None))
        assert seen == {"buffer": {}}  # absent block -> durable defaults
        assert target._durable is True
        assert target._flush_thread is None  # no in-memory flush thread on the durable path

    def test_absent_native_core_fails_fast(self, monkeypatch):
        import ggcommons.metrics.targets.cloudwatch as cw
        import ggcommons.streaming.service as svc

        # Native core not installed for this platform -> durable can't be honored -> fail fast
        # (rather than silently degrading and losing metrics across a disconnect).
        monkeypatch.setattr(svc, "native_available", lambda: False)
        monkeypatch.setattr(cw.boto3, "client", lambda *a, **k: FakeCloudWatchClient())

        with pytest.raises(RuntimeError, match="native core"):
            cw.CloudWatch(_FakeConfigManager(None))

    def test_open_failure_with_core_present_falls_back_to_inmemory(self, monkeypatch):
        import ggcommons.metrics.targets.cloudwatch as cw
        import ggcommons.streaming.service as svc

        # Core IS present, but opening the buffer fails (e.g. an unwritable path) -> graceful
        # fallback to in-memory batching (the absent-core case fails fast; this one does not).
        def boom(self, buffer):
            self._stream_service = None
            raise RuntimeError("buffer path is not a directory")

        monkeypatch.setattr(svc, "native_available", lambda: True)
        monkeypatch.setattr(cw.CloudWatch, "_init_durable", boom)
        monkeypatch.setattr(cw.boto3, "client", lambda *a, **k: FakeCloudWatchClient())

        target = cw.CloudWatch(_FakeConfigManager(None))
        assert target._durable is False
        assert target._flush_thread is not None
        assert target.get_durable_stats() is None
        target.close()
