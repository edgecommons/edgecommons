"""Unit tests for the MetricEmitter static facade and target resolution.

MetricEmitter holds process-global static state, so each test resets it. The active
target is the in-process `log` target writing to a temp file (no broker / cloud).
"""
from unittest.mock import MagicMock

import pytest

from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.platform.platform import Platform


class FakeConfigManager:
    def __init__(self, metric_config, platform=None, thing="thing-1", comp="comp"):
        self._mc = metric_config
        self._platform = platform
        self._thing = thing
        self._comp = comp
        self.listeners = []

    def get_metric_config(self):
        return self._mc

    def get_thing_name(self):
        return self._thing

    def get_component_name(self):
        return self._comp

    def get_platform(self):
        return self._platform

    def resolve_template(self, t):
        return t.replace("{ThingName}", self._thing).replace("{ComponentName}", self._comp)

    def add_config_change_listener(self, listener):
        self.listeners.append(listener)


@pytest.fixture(autouse=True)
def _reset_emitter():
    MetricEmitter.metric_target = None
    MetricEmitter.metrics = {}
    MetricEmitter.metric_config = None
    yield
    MetricEmitter.shutdown()
    MetricEmitter.metric_target = None
    MetricEmitter.metrics = {}


def _log_config(tmp_path):
    return MetricConfiguration({
        "target": "log",
        "namespace": "App/NS",
        "targetConfig": {"logFileName": str(tmp_path / "m.log")},
    })


def _metric():
    return (
        MetricBuilder.create("perf")
        .with_thing_name("thing-1")
        .with_component_name("comp")
        .add_measure("latency", "Milliseconds", 1)
        .build()
    )


class TestInit:
    def test_init_log_target(self, tmp_path):
        cm = FakeConfigManager(_log_config(tmp_path))
        MetricEmitter.init(cm)
        assert MetricEmitter.metric_target is not None
        assert MetricEmitter.get_thing_name() == "thing-1"
        assert MetricEmitter.get_component_name() == "comp"
        assert MetricEmitter.get_metric_config() is not None
        # the target registered itself as a config-change listener
        assert cm.listeners

    def test_invalid_target_falls_back_to_log(self, tmp_path, monkeypatch):
        # `logFileName` in targetConfig is only honored when target == "log"; the
        # fallback path uses the default template, so point that default at tmp_path
        # (the real default is /greengrass/v2/logs/... which doesn't exist off-device).
        monkeypatch.setattr(
            MetricConfiguration,
            "DEFAULT_METRIC_FILE_NAME_TEMPLATE",
            str(tmp_path / "m.log"),
        )
        mc = MetricConfiguration({"target": "bogus-target"})
        cm = FakeConfigManager(mc)
        MetricEmitter.init(cm)
        from ggcommons.metrics.targets.metric_log import MetricLog
        assert isinstance(MetricEmitter.metric_target, MetricLog)

    def test_init_is_idempotent_target(self, tmp_path):
        cm = FakeConfigManager(_log_config(tmp_path))
        MetricEmitter.init(cm)
        first = MetricEmitter.metric_target
        MetricEmitter.init(cm)  # target already set -> not recreated
        assert MetricEmitter.metric_target is first


class TestResolveTarget:
    def test_explicit_target_wins(self, tmp_path):
        cm = FakeConfigManager(_log_config(tmp_path))
        assert MetricEmitter._resolve_target(cm) == "log"

    def test_kubernetes_profile_default_prometheus(self):
        # no explicit target + KUBERNETES platform -> prometheus profile default
        mc = MetricConfiguration({})  # no 'target' key -> explicit is None
        cm = FakeConfigManager(mc, platform=Platform.KUBERNETES)
        assert MetricEmitter._resolve_target(cm) == "prometheus"

    def test_library_default_log(self):
        mc = MetricConfiguration({})  # explicit None
        cm = FakeConfigManager(mc, platform=None)
        assert MetricEmitter._resolve_target(cm) == "log"


class TestDefineAndEmit:
    def test_define_and_is_defined(self, tmp_path):
        MetricEmitter.init(FakeConfigManager(_log_config(tmp_path)))
        MetricEmitter.define_metric(_metric())
        assert MetricEmitter.is_metric_defined("perf") is True
        assert MetricEmitter.is_metric_defined("absent") is False

    def test_emit_metric_defined_writes(self, tmp_path):
        log_file = tmp_path / "m.log"
        mc = MetricConfiguration({
            "target": "log", "namespace": "App/NS",
            "targetConfig": {"logFileName": str(log_file)},
        })
        MetricEmitter.init(FakeConfigManager(mc))
        MetricEmitter.define_metric(_metric())
        MetricEmitter.emit_metric("perf", {"latency": 3.0})
        for h in MetricEmitter.metric_target.metric_logger.handlers:
            h.flush()
        assert log_file.read_text().strip()

    def test_emit_metric_undefined_is_noop(self, tmp_path):
        MetricEmitter.init(FakeConfigManager(_log_config(tmp_path)))
        # no exception, just a warning
        MetricEmitter.emit_metric("never-defined", {"x": 1.0})

    def test_emit_metric_now_defined(self, tmp_path):
        MetricEmitter.init(FakeConfigManager(_log_config(tmp_path)))
        MetricEmitter.metric_target = MagicMock()
        MetricEmitter.define_metric(_metric())
        MetricEmitter.emit_metric_now("perf", {"latency": 1.0})
        MetricEmitter.metric_target.emit_metric_now.assert_called_once()

    def test_emit_metric_now_undefined_is_noop(self, tmp_path):
        MetricEmitter.init(FakeConfigManager(_log_config(tmp_path)))
        MetricEmitter.metric_target = MagicMock()
        MetricEmitter.emit_metric_now("absent", {"x": 1.0})
        MetricEmitter.metric_target.emit_metric_now.assert_not_called()


class TestShutdown:
    def test_shutdown_closes_target(self, tmp_path):
        MetricEmitter.init(FakeConfigManager(_log_config(tmp_path)))
        target = MagicMock()
        MetricEmitter.metric_target = target
        MetricEmitter.shutdown()
        target.close.assert_called_once()
        assert MetricEmitter.metric_target is None
        assert MetricEmitter.metrics == {}

    def test_shutdown_swallows_close_error(self):
        target = MagicMock()
        target.close.side_effect = RuntimeError("boom")
        MetricEmitter.metric_target = target
        MetricEmitter.shutdown()  # must not raise
        assert MetricEmitter.metric_target is None

    def test_shutdown_when_uninitialized(self):
        MetricEmitter.metric_target = None
        MetricEmitter.shutdown()  # safe no-op
