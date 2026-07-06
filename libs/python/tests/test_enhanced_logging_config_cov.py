"""
Coverage tests for ``edgecommons.config.enhanced_logging_config`` targeting the branches that
``test_logging_json_sink.py`` and ``test_logging_parity.py`` do not exercise.

Specifically:

* :class:`JsonLogFormatter` ``exc_text`` (no ``exc_info``) and ``stack_info`` branches (lines 83, 85);
* per-logger level parsing — dict-with-``level`` and bare-string forms, plus ignored shapes (152-155);
* ``_parse_level`` int pass-through (171) and ``_parse_file_size`` int pass-through / unit / fallback
  (196, 214);
* ``configure_logging``: the file-logging failure path (281-282), per-logger level application
  (286-287) and the "configured N logger levels" info log (290);
* the simple getters (294, 306, 310, 314, 318).

Everything drives :class:`EnhancedLoggingConfiguration` (and the module's formatter) directly with
in-memory dicts / ``tmp_path`` — no ConfigManager, network, AWS, broker, or ``/greengrass`` paths.
Kept parallel in style to the sibling logging tests.
"""

import json
import logging
from logging.handlers import RotatingFileHandler

import pytest

from edgecommons.config.enhanced_logging_config import (
    EnhancedLoggingConfiguration,
    JsonLogFormatter,
)


# ---------- helpers (root-logger handler save/restore, like the sibling tests) ----------

def _root_handlers_snapshot():
    root = logging.getLogger()
    return root, root.handlers[:]


def _restore(root, saved):
    for h in root.handlers[:]:
        root.removeHandler(h)
    for h in saved:
        root.addHandler(h)


# ---------- JsonLogFormatter: exc_text and stack_info branches (lines 83, 85) ----------

def test_json_formatter_uses_exc_text_when_no_exc_info():
    # No live exc_info, but a previously-cached exc_text must still surface as `thrown` (line 83).
    record = logging.makeLogRecord({"name": "x", "levelname": "ERROR", "msg": "m"})
    record.exc_info = None
    record.exc_text = "Traceback (most recent call last): cached"
    obj = json.loads(JsonLogFormatter().format(record))
    assert obj["thrown"] == "Traceback (most recent call last): cached"


def test_json_formatter_includes_stack_info():
    # stack_info (logger.x(..., stack_info=True)) is rendered into `stack` (line 85).
    record = logging.makeLogRecord({"name": "x", "levelname": "INFO", "msg": "m"})
    record.stack_info = "Stack (most recent call last):\n  File ..."
    obj = json.loads(JsonLogFormatter().format(record))
    assert "stack" in obj
    assert obj["stack"].startswith("Stack (most recent call last):")


# ---------- per-logger level parsing: dict / string / ignored shapes (lines 152-155) ----------

def test_logger_levels_parsed_from_dict_and_string_forms():
    cfg = EnhancedLoggingConfiguration(
        {
            "loggers": {
                "a.dict": {"level": "DEBUG"},   # dict with 'level' (152-153)
                "b.string": "warning",           # bare string, case-insensitive (154-155)
                "c.no_level": {"foo": "bar"},   # dict WITHOUT 'level' -> ignored
                "d.bad_type": 123,               # neither dict nor str -> ignored
            }
        }
    )
    levels = cfg.get_logger_levels()  # also exercises the getter (line 314)
    assert levels["a.dict"] == logging.DEBUG
    assert levels["b.string"] == logging.WARNING
    assert "c.no_level" not in levels
    assert "d.bad_type" not in levels


# ---------- _parse_level int pass-through (line 171) + get_level (line 294) ----------

def test_int_level_passthrough_and_get_level():
    cfg = EnhancedLoggingConfiguration({"level": logging.DEBUG})
    # An int level is returned as-is (171) and surfaced by get_level (294).
    assert cfg.get_level() == logging.DEBUG


# ---------- _parse_file_size: int pass-through / units / fallback (lines 196, 214) ----------

def test_parse_file_size_int_passthrough():
    cfg = EnhancedLoggingConfiguration({})
    assert cfg._parse_file_size(2048) == 2048  # line 196


@pytest.mark.parametrize(
    "size_str,expected",
    [
        ("1B", 1),
        ("2KB", 2 * 1024),
        ("3MB", 3 * 1024 * 1024),
        ("1GB", 1024 * 1024 * 1024),
        ("not-a-size", 10 * 1024 * 1024),  # no suffix matches -> fallback (214)
        ("MB", 10 * 1024 * 1024),          # suffix matches but int("") raises -> pass -> fallback (211, 214)
    ],
)
def test_parse_file_size_units_and_fallback(size_str, expected):
    cfg = EnhancedLoggingConfiguration({})
    assert cfg._parse_file_size(size_str) == expected


# ---------- configure_logging: file-logging failure path (lines 281-282) ----------

class _RaisingConfigManager:
    """A minimal config manager whose template resolution blows up, to drive the except branch."""

    def resolve_template(self, path):  # noqa: D401 - test double
        raise RuntimeError("boom resolving template")


def test_configure_logging_file_failure_is_caught(tmp_path):
    cfg = EnhancedLoggingConfiguration(
        {"fileLogging": {"enabled": True, "filePath": str(tmp_path / "x.log")}}
    )
    root, saved = _root_handlers_snapshot()
    try:
        # Must NOT raise — the failure is caught and logged (281-282).
        cfg.configure_logging(config_manager=_RaisingConfigManager())
        # Setup failed before the handler was added, so no rotating file handler exists.
        assert [h for h in root.handlers if isinstance(h, RotatingFileHandler)] == []
    finally:
        _restore(root, saved)


# ---------- configure_logging: per-logger level application + info log (286-287, 290) ----------

def test_configure_logging_applies_per_logger_levels():
    cfg = EnhancedLoggingConfiguration({"loggers": {"cov.sub.logger": "ERROR"}})
    root, saved = _root_handlers_snapshot()
    try:
        cfg.configure_logging()  # loops over logger levels (286-287) then logs the count (290)
        assert logging.getLogger("cov.sub.logger").level == logging.ERROR
    finally:
        _restore(root, saved)
        logging.getLogger("cov.sub.logger").setLevel(logging.NOTSET)  # don't leak level state


# ---------- simple getters: 306, 310, 318 ----------

def test_file_logging_and_global_control_getters():
    cfg = EnhancedLoggingConfiguration(
        {
            "fileLogging": {"enabled": True, "filePath": "relative/path.log"},
            "globalControl": True,
        }
    )
    assert cfg.is_file_logging_enabled() is True          # line 306
    assert cfg.get_log_file_path() == "relative/path.log"  # line 310
    assert cfg.is_global_control_enabled() is True         # line 318


def test_file_logging_getters_defaults():
    cfg = EnhancedLoggingConfiguration({})
    assert cfg.is_file_logging_enabled() is False
    assert cfg.get_log_file_path() is None
    assert cfg.is_global_control_enabled() is False
