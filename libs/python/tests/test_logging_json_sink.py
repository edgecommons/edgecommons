"""
Unit tests for the Phase 1c logging slice (FR-LOG-1..4):

* the stdout-JSON sink emits one valid JSON object per line (FR-LOG-1);
* the case-insensitive ``json`` token selects it (FR-LOG-4);
* the KUBERNETES platform profile defaults to the json sink, explicit config overrides it, and
  HOST/GREENGRASS defaults are unchanged (FR-RT-3 precedence);
* no rotating file handler is installed under the json sink (FR-LOG-2);
* best-effort correlation fields appear when the Downward-API env is set and are omitted when
  absent (FR-LOG-3).

Mirrors the canonical Java behavior; kept parallel to ``test_logging_parity.py``.
"""

import io
import json
import logging
from logging.handlers import RotatingFileHandler

import pytest

from edgecommons.config.enhanced_logging_config import (
    JSON_FORMAT_TOKEN,
    EnhancedLoggingConfiguration,
    JsonLogFormatter,
)
from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.platform import LOGGING_FORMAT_JSON, Platform


# ---------- helpers (root-logger handler save/restore, like test_logging_parity) ----------

def _root_handlers_snapshot():
    root = logging.getLogger()
    return root, root.handlers[:]


def _restore(root, saved):
    for h in root.handlers[:]:
        root.removeHandler(h)
    for h in saved:
        root.addHandler(h)


def _format_one(record_kwargs=None, correlation=None) -> dict:
    """Format a single record through JsonLogFormatter and return the parsed JSON object."""
    fmt = JsonLogFormatter(correlation)
    record = logging.makeLogRecord(record_kwargs or {})
    line = fmt.format(record)
    return json.loads(line)


def _config_manager_with(config, platform=None):
    cm = ConfigManager("com.example.MyComp", "thing-1", validate_config=False, platform=platform)
    cm._load_configuration = lambda: config
    return cm


# ---------- FR-LOG-1: the formatter emits valid one-object-per-line JSON ----------

def test_json_formatter_required_fields():
    obj = _format_one({"name": "my.logger", "levelname": "INFO", "msg": "hello"})
    assert obj["logger"] == "my.logger"
    assert obj["level"] == "INFO"
    assert obj["message"] == "hello"
    # ISO-8601 UTC timestamp with trailing Z.
    assert obj["timestamp"].endswith("Z")
    assert "T" in obj["timestamp"]


def test_json_formatter_one_object_per_line_over_a_stream():
    # Drive multiple records through a StreamHandler and assert each physical line is one JSON object.
    buffer = io.StringIO()
    handler = logging.StreamHandler(buffer)
    handler.setFormatter(JsonLogFormatter())
    logger = logging.getLogger("test.json.stream")
    logger.handlers = [handler]
    logger.setLevel(logging.INFO)
    logger.propagate = False

    logger.info("first")
    logger.warning("second")
    # A message with an embedded newline must STILL be a single physical line (escaped).
    logger.info("multi\nline")

    lines = [ln for ln in buffer.getvalue().splitlines() if ln.strip()]
    assert len(lines) == 3
    parsed = [json.loads(ln) for ln in lines]
    assert [p["message"] for p in parsed] == ["first", "second", "multi\nline"]
    assert parsed[1]["level"] == "WARNING"


def test_json_formatter_includes_exception_as_thrown():
    try:
        raise ValueError("boom")
    except ValueError:
        import sys

        record = logging.makeLogRecord({"name": "x", "levelname": "ERROR", "msg": "failed"})
        record.exc_info = sys.exc_info()
        obj = json.loads(JsonLogFormatter().format(record))
    assert obj["message"] == "failed"
    assert "thrown" in obj
    assert "ValueError: boom" in obj["thrown"]


def test_json_formatter_includes_caller_extras():
    # Caller-supplied structured extras (logger.info(..., extra={...})) flow into the JSON object.
    obj = _format_one({"name": "x", "levelname": "INFO", "msg": "m", "request_id": "abc123"})
    assert obj["request_id"] == "abc123"


def test_json_formatter_non_serializable_extra_does_not_break_line():
    obj = _format_one({"name": "x", "levelname": "INFO", "msg": "m", "weird": object()})
    # default=str keeps the line valid JSON (no logging failure).
    assert isinstance(obj["weird"], str)


# ---------- FR-LOG-3: correlation fields present when set, omitted when absent ----------

def test_json_formatter_emits_correlation_when_present():
    obj = _format_one(
        {"name": "x", "levelname": "INFO", "msg": "m"},
        correlation={"thing": "t1", "pod": "p1", "namespace": "ns1", "node": "n1"},
    )
    assert obj["thing"] == "t1"
    assert obj["pod"] == "p1"
    assert obj["namespace"] == "ns1"
    assert obj["node"] == "n1"


def test_json_formatter_omits_absent_or_empty_correlation():
    obj = _format_one(
        {"name": "x", "levelname": "INFO", "msg": "m"},
        correlation={"thing": "t1", "pod": "", "namespace": None},
    )
    assert obj["thing"] == "t1"
    # Empty / None correlation values are dropped — no empty/null noise.
    assert "pod" not in obj
    assert "namespace" not in obj
    assert "node" not in obj


# ---------- FR-LOG-4: the `json` token selects the sink (case-insensitive) ----------

def test_token_constant_matches_resolver_selector():
    assert JSON_FORMAT_TOKEN == LOGGING_FORMAT_JSON == "json"


@pytest.mark.parametrize("token", ["json", "JSON", "  Json  "])
def test_json_token_selects_sink_case_insensitive(token):
    cfg = EnhancedLoggingConfiguration({"python_format": token})
    assert cfg.is_json_sink() is True


def test_non_json_token_keeps_text_sink():
    cfg = EnhancedLoggingConfiguration({"python_format": "%(message)s"})
    assert cfg.is_json_sink() is False


# ---------- FR-RT-3: format precedence (explicit ▸ platform default ▸ library default) ----------

def test_precedence_explicit_config_wins_over_platform_default():
    # Explicit non-json config overrides the KUBERNETES json default.
    cfg = EnhancedLoggingConfiguration(
        {"python_format": "%(message)s"}, platform_default_format=LOGGING_FORMAT_JSON
    )
    assert cfg.is_json_sink() is False
    assert cfg.get_format() == "%(message)s"


def test_precedence_platform_default_applies_when_config_absent():
    cfg = EnhancedLoggingConfiguration({}, platform_default_format=LOGGING_FORMAT_JSON)
    assert cfg.is_json_sink() is True


def test_precedence_library_default_when_no_config_and_no_platform_default():
    cfg = EnhancedLoggingConfiguration({})
    assert cfg.is_json_sink() is False
    assert cfg.get_format() == EnhancedLoggingConfiguration.DEFAULT_FORMAT


def test_precedence_explicit_json_config_selects_sink_on_any_platform():
    cfg = EnhancedLoggingConfiguration({"python_format": "json"}, platform_default_format=None)
    assert cfg.is_json_sink() is True


# ---------- ConfigManager integration: KUBERNETES defaults to json, HOST/GG unchanged ----------

def test_config_manager_kubernetes_defaults_to_json_sink():
    config = {"logging": {"level": "INFO"}, "component": {"global": {}, "instances": []}}
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.KUBERNETES)
        cm.init()
        assert cm.get_logging_config().is_json_sink() is True
        # The console handler carries the JSON layout; no rotating file handler installed.
        stream_handlers = [
            h for h in root.handlers if isinstance(h, logging.StreamHandler)
        ]
        assert any(isinstance(h.formatter, JsonLogFormatter) for h in stream_handlers)
        assert [h for h in root.handlers if isinstance(h, RotatingFileHandler)] == []
    finally:
        _restore(root, saved)


def test_config_manager_kubernetes_explicit_text_format_overrides_default():
    config = {
        "logging": {"level": "INFO", "python_format": "%(message)s"},
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.KUBERNETES)
        cm.init()
        # Explicit non-json config overrides the KUBERNETES profile default.
        assert cm.get_logging_config().is_json_sink() is False
        stream_handlers = [h for h in root.handlers if isinstance(h, logging.StreamHandler)]
        assert stream_handlers
        assert not any(isinstance(h.formatter, JsonLogFormatter) for h in stream_handlers)
    finally:
        _restore(root, saved)


@pytest.mark.parametrize("platform", [Platform.HOST, Platform.GREENGRASS, None])
def test_config_manager_non_kubernetes_keeps_text_default(platform):
    config = {"logging": {"level": "INFO"}, "component": {"global": {}, "instances": []}}
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=platform)
        cm.init()
        assert cm.get_logging_config().is_json_sink() is False
    finally:
        _restore(root, saved)


def test_config_manager_host_with_explicit_json_selects_sink():
    # The json token works on any platform when set explicitly (FR-LOG-4).
    config = {
        "logging": {"level": "INFO", "python_format": "json"},
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.HOST)
        cm.init()
        assert cm.get_logging_config().is_json_sink() is True
    finally:
        _restore(root, saved)


# ---------- FR-LOG-2: no in-process rotation under the json sink ----------

def test_no_rotating_handler_under_json_sink_even_when_file_logging_enabled(tmp_path):
    # Even with fileLogging.enabled, the json sink must NOT install a RotatingFileHandler
    # (the cluster log agent owns rotation; read-only root FS must not break logging).
    config = {
        "logging": {
            "python_format": "json",
            "fileLogging": {
                "enabled": True,
                "filePath": str(tmp_path / "{ComponentName}.log"),
                "maxFileSize": "256KB",
                "backupCount": 2,
            },
        },
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.KUBERNETES)
        cm.init()
        assert cm.get_logging_config().is_json_sink() is True
        assert [h for h in root.handlers if isinstance(h, RotatingFileHandler)] == []
        # The log file was never created (file logging suppressed).
        assert not (tmp_path / "MyComp.log").exists()
    finally:
        _restore(root, saved)


def test_file_logging_still_works_off_the_json_sink(tmp_path):
    # Off the json sink, file rotation is unchanged (regression guard for FR-LOG-2 scoping).
    config = {
        "logging": {
            "level": "INFO",
            "fileLogging": {
                "enabled": True,
                "filePath": str(tmp_path / "app.log"),
                "maxFileSize": "128KB",
                "backupCount": 1,
            },
        },
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.HOST)
        cm.init()
        assert cm.get_logging_config().is_json_sink() is False
        rotating = [h for h in root.handlers if isinstance(h, RotatingFileHandler)]
        assert len(rotating) == 1
    finally:
        _restore(root, saved)


# ---------- FR-LOG-3 integration: correlation pulled from the Downward-API env ----------

def test_config_manager_json_correlation_from_downward_api_env(monkeypatch):
    monkeypatch.setenv("POD_NAME", "pod-xyz")
    monkeypatch.setenv("POD_NAMESPACE", "edge")
    monkeypatch.setenv("NODE_NAME", "node-7")
    config = {
        "logging": {"level": "INFO", "python_format": "json"},
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.KUBERNETES)
        cm.init()
        # The JSON formatter on the console handler carries the correlation snapshot.
        stream_handlers = [
            h for h in root.handlers
            if isinstance(h, logging.StreamHandler) and isinstance(h.formatter, JsonLogFormatter)
        ]
        assert stream_handlers
        record = logging.makeLogRecord({"name": "x", "levelname": "INFO", "msg": "m"})
        obj = json.loads(stream_handlers[0].formatter.format(record))
        assert obj["thing"] == "thing-1"
        assert obj["pod"] == "pod-xyz"
        assert obj["namespace"] == "edge"
        assert obj["node"] == "node-7"
    finally:
        _restore(root, saved)


def test_config_manager_json_correlation_omits_absent_env(monkeypatch):
    monkeypatch.delenv("POD_NAME", raising=False)
    monkeypatch.delenv("POD_NAMESPACE", raising=False)
    monkeypatch.delenv("NODE_NAME", raising=False)
    config = {
        "logging": {"python_format": "json"},
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        cm = _config_manager_with(config, platform=Platform.KUBERNETES)
        cm.init()
        record = logging.makeLogRecord({"name": "x", "levelname": "INFO", "msg": "m"})
        fmt = [
            h.formatter for h in root.handlers
            if isinstance(h, logging.StreamHandler) and isinstance(h.formatter, JsonLogFormatter)
        ][0]
        obj = json.loads(fmt.format(record))
        # thing is always present (resolved identity); k8s pod/namespace/node omitted when unset.
        assert obj["thing"] == "thing-1"
        for absent in ("pod", "namespace", "node"):
            assert absent not in obj
    finally:
        _restore(root, saved)
