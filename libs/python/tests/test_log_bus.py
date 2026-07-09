import logging
import threading

import pytest

from edgecommons.config.enhanced_logging_config import EnhancedLoggingConfiguration
from edgecommons.logs import LogPublishConfig, LogRecord, LogService
from edgecommons.messaging.errors import ReservedTopicError
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.messaging.qos import Qos
from edgecommons.validation.configuration_validator import ConfigurationValidator


IDENTITY = MessageIdentity([HierEntry("device", "gw-01")], "adapter", "main")
TS = "2026-07-01T12:00:00Z"


class _FakeConfigManager:
    def __init__(self, publish_config=None):
        self._logging_config = _FakeLoggingConfig(publish_config or LogPublishConfig())

    def get_logging_config(self):
        return self._logging_config

    def get_component_identity(self):
        return IDENTITY

    def is_topic_include_root(self):
        return False

    def get_tag_config(self):
        return None


class _FakeLoggingConfig:
    def __init__(self, publish_config):
        self._publish_config = publish_config

    def get_publish_config(self):
        return self._publish_config


class _RecordingMessaging:
    def __init__(self):
        self.local = []
        self.northbound = []
        self.northbound_qos = []

    def connected(self):
        return True

    def _publish_reserved(self, topic, msg):
        self.local.append((topic, msg))

    def _publish_reserved_northbound(self, topic, msg, qos):
        self.northbound.append((topic, msg))
        self.northbound_qos.append(qos)


class _DisconnectedMessaging(_RecordingMessaging):
    def connected(self):
        return False

    def _publish_reserved(self, topic, msg):
        raise AssertionError("reserved publish should not be called while disconnected")


class _LoggingMessaging(_RecordingMessaging):
    def _publish_reserved(self, topic, msg):
        logging.getLogger("edgecommons.messaging.provider.test").error(
            "provider publish warning should not recurse"
        )
        super()._publish_reserved(topic, msg)


class _BlockingMessaging(_RecordingMessaging):
    def __init__(self):
        super().__init__()
        self.started = threading.Event()
        self.release = threading.Event()
        self._first = True

    def _publish_reserved(self, topic, msg):
        if self._first:
            self._first = False
            self.started.set()
            assert self.release.wait(5)
        super()._publish_reserved(topic, msg)


def _service(publish_config=None, messaging=None):
    messaging = messaging or _RecordingMessaging()
    service = LogService(messaging)
    service.configure(_FakeConfigManager(publish_config), messaging)
    return service, messaging


def _enabled(**kwargs):
    raw = {
        "enabled": True,
        "destination": "local",
        "minLevel": "INFO",
        "captureNative": True,
        "captureConsole": False,
        "maxRecordBytes": 8192,
    }
    raw.update(kwargs)
    return LogPublishConfig.from_logging_config({"publish": raw})


def test_explicit_publish_uses_reserved_seam_and_canonical_envelope():
    service, messaging = _service(_enabled())
    try:
        service.publish(
            LogRecord(
                timestamp=TS,
                level="ERROR",
                logger="unit",
                message="failed",
                sequence=42,
                thread="main-thread",
                fields={"asset": "press-1"},
                error="traceback",
            )
        )
        assert service.flush(timeout=2) is True

        topic, msg = messaging.local[0]
        envelope = msg.to_dict()
        assert topic == "ecv1/gw-01/adapter/main/log/error"
        assert envelope["header"]["name"] == "log"
        assert envelope["header"]["version"] == "1.0"
        assert envelope["header"]["timestamp"] == TS
        assert envelope["identity"]["instance"] == "main"
        assert envelope["body"] == {
            "schema": "edgecommons.log.v1",
            "timestamp": TS,
            "level": "ERROR",
            "logger": "unit",
            "message": "failed",
            "sequence": 42,
            "thread": "main-thread",
            "fields": {"asset": "press-1"},
            "error": "traceback",
        }
    finally:
        service.close()


def test_public_publish_guard_still_rejects_log_class():
    with pytest.raises(ReservedTopicError):
        MessagingClient._check_reserved_topic("ecv1/gw-01/adapter/main/log/error")


def test_northbound_destination_uses_reserved_northbound_seam():
    service, messaging = _service(_enabled(destination="northbound"))
    try:
        service.publish(LogRecord(timestamp=TS, level="FATAL", logger="unit", message="fatal"))
        assert service.flush(timeout=2) is True
        assert not messaging.local
        assert messaging.northbound[0][0] == "ecv1/gw-01/adapter/main/log/fatal"
        assert messaging.northbound_qos == [Qos.AT_LEAST_ONCE]
    finally:
        service.close()


def test_disconnected_transport_counts_failure_without_provider_publish():
    service, messaging = _service(_enabled(), _DisconnectedMessaging())
    try:
        service.publish(LogRecord(timestamp=TS, level="ERROR", logger="unit", message="offline"))
        assert service.flush(timeout=2) is True
        assert messaging.local == []
        assert service.stats()["failed"] == 1
    finally:
        service.close()


def test_provider_logs_emitted_during_publish_are_not_recaptured():
    service, messaging = _service(_enabled(), _LoggingMessaging())
    try:
        service.install_handler()
        service.publish(LogRecord(timestamp=TS, level="INFO", logger="unit", message="one"))
        assert service.flush(timeout=2) is True
        assert len(messaging.local) == 1
        assert messaging.local[0][1].to_dict()["body"]["message"] == "one"
    finally:
        service.close()


def test_publish_config_defaults_and_strict_parsing():
    defaults = LogPublishConfig.from_logging_config({})
    assert defaults.enabled is False
    assert defaults.destination == "local"
    assert defaults.min_level == "INFO"
    assert defaults.capture_native is True
    assert defaults.capture_console is False
    assert defaults.max_record_bytes == 8192
    assert defaults.queue.max_records == 1000
    assert defaults.queue.on_full == "dropOldest"
    assert defaults.redaction.enabled is True
    assert defaults.redaction.replacement == "***"

    parsed = LogPublishConfig.from_logging_config(
        {
            "publish": {
                "enabled": True,
                "destination": "northbound",
                "minLevel": "warn",
                "captureNative": False,
                "captureConsole": True,
                "maxRecordBytes": 256,
                "queue": {"maxRecords": 7, "onFull": "dropOldest"},
                "redaction": {
                    "enabled": True,
                    "replacement": "[x]",
                    "extraPatterns": ["secret-[0-9]+"],
                },
            }
        }
    )
    assert parsed.destination == "northbound"
    assert parsed.min_level == "WARN"
    assert parsed.queue.max_records == 7
    assert parsed.redaction.extra_patterns[0].pattern == "secret-[0-9]+"

    with pytest.raises(ValueError, match="unsupported key"):
        LogPublishConfig.from_logging_config({"publish": {"enabled": True, "extra": 1}})
    with pytest.raises(ValueError, match="onFull"):
        LogPublishConfig.from_logging_config({"publish": {"queue": {"onFull": "block"}}})


def test_python_schema_accepts_logging_publish_section():
    ConfigurationValidator.validate(
        {
            "component": {},
            "logging": {
                "publish": {
                    "enabled": True,
                    "destination": "local",
                    "minLevel": "INFO",
                    "captureNative": True,
                    "captureConsole": False,
                    "maxRecordBytes": 8192,
                    "queue": {"maxRecords": 1000, "onFull": "dropOldest"},
                    "redaction": {
                        "enabled": True,
                        "replacement": "***",
                        "extraPatterns": [],
                    },
                }
            },
        }
    )


def test_enhanced_logging_config_exposes_publish_config():
    cfg = EnhancedLoggingConfiguration(
        {"publish": {"enabled": True, "destination": "northbound"}}
    )
    assert cfg.get_publish_config().enabled is True
    assert cfg.get_publish_config().destination == "northbound"
    as_dict = cfg.to_dict()
    assert as_dict["publish"]["enabled"] is True
    assert as_dict["publish"]["queue"]["maxRecords"] == 1000


def test_enhanced_logging_config_lowers_root_only_for_bus_capture():
    root = logging.getLogger()
    old_handlers = list(root.handlers)
    old_level = root.level
    try:
        cfg = EnhancedLoggingConfiguration(
            {
                "level": "INFO",
                "publish": {"enabled": True, "minLevel": "DEBUG"},
            }
        )
        cfg.configure_logging()
        assert root.level == logging.DEBUG
        stream_handlers = [h for h in root.handlers if isinstance(h, logging.StreamHandler)]
        assert stream_handlers
        assert all(h.level == logging.INFO for h in stream_handlers)
    finally:
        for handler in root.handlers[:]:
            root.removeHandler(handler)
        for handler in old_handlers:
            root.addHandler(handler)
        root.setLevel(old_level)


def test_redaction_applies_to_message_error_and_fields():
    config = _enabled(
        redaction={
            "enabled": True,
            "replacement": "[redacted]",
            "extraPatterns": ["asset-[0-9]+"],
        }
    )
    service, messaging = _service(config)
    try:
        service.publish(
            LogRecord(
                timestamp=TS,
                level="INFO",
                logger="unit",
                message="password=abc token:xyz asset-42",
                sequence=1,
                fields={"secret": "abc", "plain": "ok"},
                error="Authorization: Bearer abcdef",
            )
        )
        assert service.flush(timeout=2) is True
        body = messaging.local[0][1].to_dict()["body"]
        assert body["message"] == "password=[redacted] token:[redacted] [redacted]"
        assert body["fields"] == {"secret": "[redacted]", "plain": "ok"}
        assert body["error"] == "Authorization: [redacted]"
        assert service.stats()["redacted"] == 1
    finally:
        service.close()


def test_truncation_sets_flag_and_keeps_body_under_limit():
    config = _enabled(maxRecordBytes=220)
    service, messaging = _service(config)
    try:
        service.publish(
            LogRecord(
                timestamp=TS,
                level="INFO",
                logger="unit",
                message="x" * 1000,
                sequence=1,
                fields={"large": "y" * 1000},
            )
        )
        assert service.flush(timeout=2) is True
        body = messaging.local[0][1].to_dict()["body"]
        assert body["truncated"] is True
        assert "fields" not in body
        assert len(str(body).encode("utf-8")) <= 260
        assert service.stats()["truncated"] == 1
    finally:
        service.close()


def test_queue_drop_oldest_never_blocks_and_reports_drop():
    messaging = _BlockingMessaging()
    config = _enabled(queue={"maxRecords": 1, "onFull": "dropOldest"})
    service, _ = _service(config, messaging)
    try:
        service.publish(LogRecord(timestamp=TS, level="INFO", logger="unit", message="first"))
        assert messaging.started.wait(5)
        service.publish(LogRecord(timestamp=TS, level="INFO", logger="unit", message="second"))
        service.publish(LogRecord(timestamp=TS, level="INFO", logger="unit", message="third"))
        messaging.release.set()
        assert service.flush(timeout=2) is True

        bodies = [msg.to_dict()["body"] for _, msg in messaging.local]
        assert [body["message"] for body in bodies] == ["first", "third"]
        assert bodies[1]["dropped"] == 1
        assert service.stats()["dropped"] == 1
    finally:
        messaging.release.set()
        service.close()


def test_root_logging_handler_capture_honors_config_and_min_level():
    service, messaging = _service(_enabled(minLevel="WARN"))
    service.install_handler()
    logger = logging.getLogger("unit.logbus.capture")
    old_level = logger.level
    old_propagate = logger.propagate
    logger.setLevel(logging.DEBUG)
    logger.propagate = True
    try:
        logger.info("ignored")
        logger.warning("captured %s", "warning", extra={"asset": "press-1"})
        assert service.flush(timeout=2) is True
        assert len(messaging.local) == 1
        topic, msg = messaging.local[0]
        body = msg.to_dict()["body"]
        assert topic == "ecv1/gw-01/adapter/main/log/warn"
        assert body["level"] == "WARN"
        assert body["logger"] == "unit.logbus.capture"
        assert body["message"] == "captured warning"
        assert body["fields"] == {"asset": "press-1"}
        assert body["thread"]
    finally:
        logger.setLevel(old_level)
        logger.propagate = old_propagate
        service.close()


def test_edgecommons_logs_accessor_returns_bound_service():
    from edgecommons.edgecommons import EdgeCommons

    gg = object.__new__(EdgeCommons)
    service, _ = _service(_enabled())
    try:
        gg._logs = service
        assert gg.logs() is service
    finally:
        service.close()
