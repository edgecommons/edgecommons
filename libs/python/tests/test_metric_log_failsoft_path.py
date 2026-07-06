"""Unit tests for the metric ``log`` target's fail-soft behavior (Part A) and the HOST-aware
default log-file path (Part B).

Part A: when the log file cannot be opened, the target warns and drops file metrics rather than
raising out of construction (matching the Java/Rust/TS log targets).

Part B: the default log-file path is resolved with the precedence
``explicit logFileName`` config ▸ platform-profile default (local path on HOST/KUBERNETES) ▸ the
library default ``/greengrass/v2/logs`` — so a HOST/KUBERNETES component does not target the
Greengrass logs directory that exists only on-device.
"""
import logging

from edgecommons.config.metric_config import MetricConfiguration
from edgecommons.metrics.metric_builder import MetricBuilder
from edgecommons.metrics.targets.metric_log import MetricLog
from edgecommons.platform.platform import Platform
from edgecommons.platform.resolver import (
    METRIC_LOG_PATH_LOCAL,
    profile_metric_log_path,
)


class FakeCM:
    """Minimal ConfigManager stand-in exposing the three methods MetricLog uses."""

    def __init__(self, metric_config, platform=None, full_name="com.example.Comp"):
        self._mc = metric_config
        self._platform = platform
        self._full = full_name

    def get_metric_config(self):
        return self._mc

    def get_platform(self):
        return self._platform

    def resolve_template(self, template):
        return (
            template.replace("{ComponentFullName}", self._full)
            .replace("{ThingName}", "thing-1")
            .replace("{ComponentName}", "Comp")
        )


def _metric():
    return (
        MetricBuilder.create("perf")
        .with_thing_name("thing-1")
        .with_component_name("Comp")
        .with_namespace("App/NS")
        .add_measure("latency", "Milliseconds", 1)
        .build()
    )


def _log_config(log_file_name=None):
    cfg = {"target": "log", "targetConfig": {}}
    if log_file_name is not None:
        cfg["targetConfig"]["logFileName"] = log_file_name
    return MetricConfiguration(cfg)


# ----- Part B: platform-profile default path (resolver) -----------------------------------------


class TestProfileMetricLogPath:
    def test_host_and_kubernetes_default_to_local_path(self):
        assert profile_metric_log_path(Platform.HOST) == METRIC_LOG_PATH_LOCAL
        assert profile_metric_log_path(Platform.KUBERNETES) == METRIC_LOG_PATH_LOCAL

    def test_greengrass_has_no_override(self):
        assert profile_metric_log_path(Platform.GREENGRASS) is None

    def test_none_platform_has_no_override(self):
        assert profile_metric_log_path(None) is None


# ----- Part B: explicit logFileName tracking (config) -------------------------------------------


class TestExplicitLogFileName:
    def test_absent_when_not_configured(self):
        assert MetricConfiguration(None).get_explicit_log_file_name() is None
        assert _log_config().get_explicit_log_file_name() is None

    def test_present_when_configured(self):
        mc = _log_config("/custom/path.log")
        assert mc.get_explicit_log_file_name() == "/custom/path.log"
        assert mc.get_log_file_name_template() == "/custom/path.log"


# ----- Part B: path precedence in the target ---------------------------------------------------


class TestLogFilePathResolution:
    def test_explicit_wins_over_platform_and_library(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        target = MetricLog(FakeCM(MetricConfiguration(None), platform=Platform.HOST))
        cm = FakeCM(_log_config("/custom/{ComponentFullName}.log"), platform=Platform.HOST)
        target.config_manager = cm
        assert target._resolve_log_file_path() == "/custom/com.example.Comp.log"

    def test_host_uses_local_default(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        target = MetricLog(FakeCM(MetricConfiguration(None), platform=Platform.HOST))
        target.config_manager = FakeCM(MetricConfiguration(None), platform=Platform.HOST)
        assert target._resolve_log_file_path() == "./logs/com.example.Comp_metric.log"

    def test_greengrass_falls_through_to_library_default(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        target = MetricLog(FakeCM(MetricConfiguration(None), platform=Platform.HOST))
        target.config_manager = FakeCM(MetricConfiguration(None), platform=Platform.GREENGRASS)
        assert (
            target._resolve_log_file_path()
            == "/greengrass/v2/logs/com.example.Comp_metric.log"
        )


# ----- Part A: fail-soft + Part B: HOST default actually writes locally -------------------------


class TestFailSoftAndLocalWrite:
    def test_host_default_creates_local_file_and_emits(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        target = MetricLog(FakeCM(MetricConfiguration(None), platform=Platform.HOST))
        assert target._enabled is True
        expected = tmp_path / "logs" / "com.example.Comp_metric.log"
        assert expected.exists()

        target.emit_metric_now(_metric(), {"latency": 1.0})
        for h in logging.getLogger("metric_file").handlers:
            h.flush()
        assert expected.read_text().strip() != ""

    def test_unopenable_path_fails_soft(self, tmp_path):
        # A regular file where a directory is expected, so makedirs(parent) raises OSError.
        blocker = tmp_path / "blocker"
        blocker.write_text("x")
        bad = str(blocker / "sub" / "metric.log")
        cm = FakeCM(_log_config(bad), platform=Platform.HOST)

        target = MetricLog(cm)  # must NOT raise
        assert target._enabled is False
        # Emitting is a silent no-op (does not raise) when file metrics are disabled.
        target.emit_metric_now(_metric(), {"latency": 1.0})

    def test_reconfigure_recovers_from_disabled_state(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        blocker = tmp_path / "blocker"
        blocker.write_text("x")
        cm = FakeCM(_log_config(str(blocker / "x" / "m.log")), platform=Platform.HOST)
        target = MetricLog(cm)
        assert target._enabled is False

        # Point at a writable path and reconfigure (hot-reload path) — it should recover.
        good = tmp_path / "ok.log"
        target.config_manager = FakeCM(_log_config(str(good)), platform=Platform.HOST)
        target.on_configuration_change({})
        assert target._enabled is True
        assert good.exists()
