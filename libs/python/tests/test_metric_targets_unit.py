"""Unit tests for the metric targets: metric_log, messaging, cloudwatch_component, emf_helper.

These drive each target against a fake ConfigManager and (for the messaging/component
targets) a patched ``MessagingClient`` static, so no broker or filesystem-resident GG
logs directory is required.
"""
import json
import logging

import pytest

from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.metrics.targets.metric_log import MetricLog, _parse_size
from ggcommons.metrics.targets.messaging import Messaging, _is_local_destination
from ggcommons.metrics.targets.cloudwatch_component import CloudWatchComponent
from ggcommons.metrics.targets.emf_helper import build_metric_data_emf, get_metrics_metadata_emf
from ggcommons.messaging.identity import HierEntry, MessageIdentity
import ggcommons.metrics.targets.messaging as messaging_mod
import ggcommons.metrics.targets.cloudwatch_component as cwc_mod


class FakeConfigManager:
    """Minimal ConfigManager stand-in for metric targets."""

    def __init__(self, metric_config, thing="thing-1", component="comp", identity=True):
        self._mc = metric_config
        self._thing = thing
        self._component = component
        self._identity = (
            MessageIdentity([HierEntry("device", thing)], component) if identity else None
        )

    def get_metric_config(self):
        return self._mc

    def get_component_identity(self):
        return self._identity

    def is_topic_include_root(self):
        return False

    def resolve_template(self, template):
        return (
            template.replace("{ThingName}", self._thing).replace("{ComponentName}", self._component)
        )

    def get_thing_name(self):
        return self._thing

    def get_component_name(self):
        return self._component

    def get_tag_config(self):
        return None


def _metric():
    return (
        MetricBuilder.create("perf")
        .with_thing_name("thing-1")
        .with_component_name("comp")
        .with_namespace("App/NS")
        .add_measure("latency", "Milliseconds", 1)
        .add_measure("count", "Count", 60)
        .add_dimension("instance", "main")
        .build()
    )


class TestParseSize:
    def test_plain_and_units(self):
        assert _parse_size("100") == 100
        assert _parse_size("10MB") == 10 * 1024 ** 2
        assert _parse_size("512KB") == 512 * 1024
        assert _parse_size("1GB") == 1024 ** 3
        assert _parse_size("2B") == 2

    def test_empty_and_garbage_fall_back(self):
        assert _parse_size("") == 10 * 1024 ** 2
        assert _parse_size("not-a-size") == 10 * 1024 ** 2

    def test_case_insensitive(self):
        assert _parse_size("5mb") == 5 * 1024 ** 2


class TestEmfHelper:
    def test_build_metric_data_emf_structure(self):
        mc = MetricConfiguration()
        metric = _metric()
        values = {"latency": 12.5, "count": 3}
        emf = build_metric_data_emf(mc, metric, values, False)
        assert "_aws" in emf
        assert "CloudWatchMetrics" in emf["_aws"]
        assert isinstance(emf["_aws"]["Timestamp"], int)
        # measure values present as top-level keys
        assert emf["latency"] == 12.5 and emf["count"] == 3
        # dimensions present
        assert emf["instance"] == "main"
        assert emf["coreName"] == "thing-1"

    def test_large_fleet_workaround_masks_core_name(self):
        mc = MetricConfiguration()
        emf = build_metric_data_emf(mc, _metric(), {"latency": 1.0}, True)
        assert emf["coreName"] == "ALL"

    def test_metrics_metadata_uses_metric_namespace(self):
        mc = MetricConfiguration({"target": "log", "namespace": "Default/NS"})
        meta = get_metrics_metadata_emf(mc, _metric())
        # metric has its own namespace -> wins over config default
        assert meta["Namespace"] == "App/NS"
        names = {m["Name"] for m in meta["Metrics"]}
        assert names == {"latency", "count"}

    def test_metrics_metadata_falls_back_to_config_namespace(self):
        mc = MetricConfiguration({"target": "log", "namespace": "Default/NS"})
        metric = (
            MetricBuilder.create("m")
            .with_thing_name("t")
            .with_component_name("c")
            .add_measure("v", "Count", 60)
            .build()
        )
        # builder injects "GGCommons/Metrics" namespace by default, so use a metric without one
        metric.namespace = None
        meta = get_metrics_metadata_emf(mc, metric)
        assert meta["Namespace"] == "Default/NS"


class TestMetricLog:
    def test_emit_writes_emf_json_line(self, tmp_path):
        log_file = tmp_path / "metric.log"
        mc = MetricConfiguration({
            "target": "log",
            "namespace": "App/NS",
            "targetConfig": {"logFileName": str(log_file), "maxFileSize": "1MB"},
        })
        cm = FakeConfigManager(mc)
        target = MetricLog(cm)
        target.emit_metric_now(_metric(), {"latency": 7.0, "count": 1})
        # flush handlers
        for h in target.metric_logger.handlers:
            h.flush()
        content = log_file.read_text().strip()
        assert content, "metric log should have a line"
        parsed = json.loads(content.splitlines()[0])
        assert parsed["latency"] == 7.0
        assert "_aws" in parsed

    def test_large_fleet_workaround_emits_two_lines(self, tmp_path):
        log_file = tmp_path / "metric.log"
        mc = MetricConfiguration({
            "target": "log",
            "namespace": "App/NS",
            "largeFleetWorkaround": True,
            "targetConfig": {"logFileName": str(log_file)},
        })
        target = MetricLog(FakeConfigManager(mc))
        target.emit_metric_now(_metric(), {"latency": 1.0})
        for h in target.metric_logger.handlers:
            h.flush()
        lines = [l for l in log_file.read_text().splitlines() if l.strip()]
        assert len(lines) == 2
        # second line has masked coreName
        assert json.loads(lines[1])["coreName"] == "ALL"

    def test_on_configuration_change_reconfigures(self, tmp_path):
        log_file = tmp_path / "metric.log"
        mc = MetricConfiguration({"target": "log", "targetConfig": {"logFileName": str(log_file)}})
        target = MetricLog(FakeConfigManager(mc))
        n_before = len(target.metric_logger.handlers)
        assert target.on_configuration_change(None) is True
        # handlers rebuilt from scratch (no duplicate stacking)
        assert len(target.metric_logger.handlers) == n_before


class TestMessagingTarget:
    def test_is_local_destination(self):
        assert _is_local_destination("ipc") is True
        assert _is_local_destination("local") is True
        assert _is_local_destination("anything") is True
        assert _is_local_destination("iot_core") is False
        assert _is_local_destination("iotcore") is False
        assert _is_local_destination("IoTCore") is False

    def test_emit_publishes_local_on_uns_metric_topic(self, monkeypatch):
        # UNS-CANONICAL-DESIGN par. 4.3: the messaging target publishes to
        # ecv1/{device}/{component}/main/metric/{metricName} through the privileged
        # seam (the metric class is reserved).
        published = []
        monkeypatch.setattr(
            messaging_mod.MessagingClient, "_publish_reserved",
            staticmethod(lambda topic, msg: published.append((topic, msg))),
        )
        mc = MetricConfiguration({
            "target": "messaging",
            "targetConfig": {"destination": "local"},
        })
        target = Messaging(FakeConfigManager(mc))
        assert target.send_to_local is True
        target.emit_metric_now(_metric(), {"latency": 5.0})
        assert len(published) == 1
        topic, msg = published[0]
        assert topic == "ecv1/thing-1/comp/main/metric/perf"
        assert "_aws" in msg.get_body()

    def test_emit_publishes_iot_core(self, monkeypatch):
        published = []
        monkeypatch.setattr(
            messaging_mod.MessagingClient, "_publish_reserved_to_iot_core",
            staticmethod(lambda topic, msg, qos: published.append((topic, msg, qos))),
        )
        mc = MetricConfiguration({
            "target": "messaging",
            "targetConfig": {"destination": "iot_core"},
        })
        target = Messaging(FakeConfigManager(mc))
        assert target.send_to_local is False
        target.emit_metric_now(_metric(), {"latency": 5.0})
        assert len(published) == 1
        assert published[0][0] == "ecv1/thing-1/comp/main/metric/perf"

    def test_metric_name_sanitized_into_channel_token(self, monkeypatch):
        published = []
        monkeypatch.setattr(
            messaging_mod.MessagingClient, "_publish_reserved",
            staticmethod(lambda topic, msg: published.append((topic, msg))),
        )
        mc = MetricConfiguration({"target": "messaging"})
        target = Messaging(FakeConfigManager(mc))
        metric = (
            MetricBuilder.create("per+f")
            .with_thing_name("thing-1")
            .with_component_name("comp")
            .with_namespace("App/NS")
            .add_measure("latency", "Milliseconds", 1)
            .build()
        )
        target.emit_metric_now(metric, {"latency": 5.0})
        assert published[0][0] == "ecv1/thing-1/comp/main/metric/per_f"

    def test_no_identity_warns_once_and_drops(self, monkeypatch):
        published = []
        monkeypatch.setattr(
            messaging_mod.MessagingClient, "_publish_reserved",
            staticmethod(lambda topic, msg: published.append((topic, msg))),
        )
        mc = MetricConfiguration({"target": "messaging"})
        target = Messaging(FakeConfigManager(mc, identity=False))
        target.emit_metric_now(_metric(), {"latency": 5.0})
        target.emit_metric_now(_metric(), {"latency": 6.0})
        assert published == []

    def test_large_fleet_workaround_publishes_twice(self, monkeypatch):
        published = []
        monkeypatch.setattr(
            messaging_mod.MessagingClient, "_publish_reserved",
            staticmethod(lambda topic, msg: published.append(msg)),
        )
        mc = MetricConfiguration({
            "target": "messaging",
            "largeFleetWorkaround": True,
            "targetConfig": {"destination": "local"},
        })
        target = Messaging(FakeConfigManager(mc))
        target.emit_metric_now(_metric(), {"latency": 5.0})
        assert len(published) == 2

    def test_on_configuration_change_updates_destination(self, monkeypatch):
        mc = MetricConfiguration({
            "target": "messaging",
            "targetConfig": {"destination": "local"},
        })
        cm = FakeConfigManager(mc)
        target = Messaging(cm)
        assert target.send_to_local is True
        # mutate the underlying metric config and re-apply
        mc._destination = "iot_core"
        assert target.on_configuration_change(None) is True
        assert target.send_to_local is False


class TestCloudWatchComponent:
    def test_emit_publishes_raw_per_measure(self, monkeypatch):
        published = []
        monkeypatch.setattr(
            cwc_mod.MessagingClient, "publish_raw",
            staticmethod(lambda topic, data: published.append((topic, data))),
        )
        mc = MetricConfiguration({
            "target": "cloudwatchcomponent",
            "namespace": "App/NS",
        })
        target = CloudWatchComponent(FakeConfigManager(mc))
        # D-U21: the cloudwatchcomponent topic is the fixed external AWS Greengrass
        # component contract - no override.
        assert target.topic == "cloudwatch/metric/put"
        target.emit_metric_now(_metric(), {"latency": 9.0, "count": 2})
        # one publish per measure value
        assert len(published) == 2
        topic, data = published[0]
        assert topic == "cloudwatch/metric/put"
        req = data["request"]
        assert req["namespace"] == "App/NS"
        assert req["metricData"]["metricName"] in ("latency", "count")
        # dimensions exclude coreName
        dim_names = [d["name"] for d in req["metricData"]["dimensions"]]
        assert "coreName" not in dim_names

    def test_on_configuration_change_updates_topic(self):
        mc = MetricConfiguration({"target": "cloudwatchcomponent"})
        target = CloudWatchComponent(FakeConfigManager(mc))
        mc._topic = "b"
        assert target.on_configuration_change(None) is True
        assert target.topic == "b"
