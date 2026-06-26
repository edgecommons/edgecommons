"""Unit tests for the in-memory (legacy batching) path of the CloudWatch metric target.

boto3's CloudWatch client is mocked, so no AWS access is needed. The durable path
(ggstreamlog) is covered separately (test_cloudwatch_durable.py); here buffer.type=memory
selects the in-memory batching path so the flush/queue/reconfigure logic runs in-process.
"""
from unittest.mock import MagicMock

import pytest

import ggcommons.metrics.targets.cloudwatch as cw_mod
from ggcommons.metrics.targets.cloudwatch import CloudWatch
from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.metrics.metric_builder import MetricBuilder


class FakeConfigManager:
    def __init__(self, metric_config, thing="thing-1", comp="comp"):
        self._mc = metric_config
        self._thing = thing
        self._comp = comp

    def get_metric_config(self):
        return self._mc

    def get_thing_name(self):
        return self._thing

    def get_component_name(self):
        return self._comp

    def resolve_template(self, t):
        return t.replace("{ComponentName}", self._comp).replace("{ThingName}", self._thing)


def _memory_config(interval=3600, large_fleet=False):
    # large interval so the background flush thread never fires during a test
    return MetricConfiguration({
        "target": "cloudwatch",
        "namespace": "App/NS",
        "largeFleetWorkaround": large_fleet,
        "targetConfig": {"cloudwatch": {"intervalSecs": interval, "buffer": {"type": "memory"}}},
    })


def _metric():
    return (
        MetricBuilder.create("perf")
        .with_thing_name("thing-1")
        .with_component_name("comp")
        .with_namespace("App/NS")
        .add_measure("latency", "Milliseconds", 1)
        .build()
    )


@pytest.fixture
def fake_client(monkeypatch):
    client = MagicMock()
    client.meta.region_name = "us-east-1"
    monkeypatch.setattr(cw_mod.boto3, "client", lambda *a, **k: client)
    return client


class TestInMemoryEmit:
    def test_emit_metric_queues_then_flush_sends(self, fake_client):
        target = CloudWatch(FakeConfigManager(_memory_config()))
        try:
            target.emit_metric(_metric(), {"latency": 5.0})
            assert "App/NS" in target._pending_metrics
            assert len(target._pending_metrics["App/NS"]) == 1
            target._flush_metrics()
            fake_client.put_metric_data.assert_called_once()
            ns = fake_client.put_metric_data.call_args.kwargs["Namespace"]
            assert ns == "App/NS"
            # queue drained
            assert target._pending_metrics.get("App/NS", []) == []
        finally:
            target.close()

    def test_emit_metric_now_sends_immediately(self, fake_client):
        target = CloudWatch(FakeConfigManager(_memory_config()))
        try:
            target.emit_metric_now(_metric(), {"latency": 9.0})
            fake_client.put_metric_data.assert_called_once()
        finally:
            target.close()

    def test_large_fleet_workaround_doubles_datums(self, fake_client):
        target = CloudWatch(FakeConfigManager(_memory_config(large_fleet=True)))
        try:
            data = target._prepare_metric_data(_metric(), {"latency": 1.0})
            assert len(data) == 2  # normal + ALL-coreName variant
            core_values = [
                d["Value"] for d in data for dim in d["Dimensions"]
                if dim["Name"] == "coreName" and dim["Value"] == "ALL"
            ]
            assert core_values  # the masked variant exists
        finally:
            target.close()

    def test_flush_empty_is_noop(self, fake_client):
        target = CloudWatch(FakeConfigManager(_memory_config()))
        try:
            target._flush_metrics()
            fake_client.put_metric_data.assert_not_called()
        finally:
            target.close()

    def test_flush_keeps_datums_on_send_failure(self, fake_client):
        target = CloudWatch(FakeConfigManager(_memory_config()))
        try:
            fake_client.put_metric_data.side_effect = RuntimeError("network down")
            target.emit_metric(_metric(), {"latency": 1.0})
            target._flush_metrics()
            # failed send -> datum retained for next flush
            assert len(target._pending_metrics["App/NS"]) == 1
        finally:
            target.close()

    def test_unknown_measure_skipped(self, fake_client):
        target = CloudWatch(FakeConfigManager(_memory_config()))
        try:
            data = target._prepare_metric_data(_metric(), {"latency": 1.0, "ghost": 2.0})
            names = [d["MetricName"] for d in data]
            assert names == ["latency"]
        finally:
            target.close()


class TestReconfigure:
    def test_on_configuration_change_restarts_flush(self, fake_client):
        # Use a short interval: on_configuration_change joins the flush thread without
        # setting the wake event, so the thread must be able to return from its
        # interval wait on its own (a long interval would block the join).
        cm = FakeConfigManager(_memory_config(interval=1))
        target = CloudWatch(cm)
        try:
            cm._mc._interval_secs = 2
            assert target.on_configuration_change(None) is True
            assert target._interval_secs == 2
        finally:
            target.close()


class TestClientFallback:
    def test_region_fallback_on_first_client_error(self, monkeypatch):
        calls = {"n": 0}

        def flaky(*a, **k):
            calls["n"] += 1
            if calls["n"] == 1 and "region_name" not in k:
                raise RuntimeError("no region resolvable")
            c = MagicMock()
            c.meta.region_name = "us-east-1"
            return c

        monkeypatch.setattr(cw_mod.boto3, "client", flaky)
        target = CloudWatch(FakeConfigManager(_memory_config()))
        try:
            assert calls["n"] >= 2  # first failed, retried with explicit region
        finally:
            target.close()
