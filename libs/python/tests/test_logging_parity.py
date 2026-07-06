"""
Unit tests for file-logging parity: ConfigManager now wires a size-rotated
RotatingFileHandler (logging.fileLogging) with a template-resolved path, matching
the Java (Log4j2 RollingFile) and Rust implementations.
"""

import logging
from logging.handlers import RotatingFileHandler

from edgecommons.config.manager.config_manager import ConfigManager


def _config_manager_with(config):
    cm = ConfigManager("com.example.MyComp", "thing-1", validate_config=False)
    cm._load_configuration = lambda: config
    return cm


def _root_handlers_snapshot():
    root = logging.getLogger()
    return root, root.handlers[:]


def _restore(root, saved):
    for h in root.handlers[:]:
        root.removeHandler(h)
    for h in saved:
        root.addHandler(h)


def test_config_manager_wires_rotating_file_handler(tmp_path):
    template_path = str(tmp_path / "{ComponentName}.log")
    config = {
        "logging": {
            "level": "INFO",
            "fileLogging": {
                "enabled": True,
                "filePath": template_path,
                "maxFileSize": "256KB",
                "backupCount": 2,
            },
        },
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        _config_manager_with(config).init()
        file_handlers = [h for h in root.handlers if isinstance(h, RotatingFileHandler)]
        assert len(file_handlers) == 1
        fh = file_handlers[0]
        # Size-based rotation config applied.
        assert fh.maxBytes == 256 * 1024
        assert fh.backupCount == 2
        # The {ComponentName} template was resolved and the file/dir created.
        assert (tmp_path / "MyComp.log").exists()
    finally:
        _restore(root, saved)


def test_no_file_handler_when_disabled(tmp_path):
    config = {
        "logging": {"level": "INFO", "fileLogging": {"enabled": False}},
        "component": {"global": {}, "instances": []},
    }
    root, saved = _root_handlers_snapshot()
    try:
        _config_manager_with(config).init()
        file_handlers = [h for h in root.handlers if isinstance(h, RotatingFileHandler)]
        assert file_handlers == []
    finally:
        _restore(root, saved)
