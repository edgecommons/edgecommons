"""Library-owned UNS ``log`` publisher.

The public bus topic remains reserved: application code cannot publish raw messages
to ``log`` through :class:`edgecommons.messaging.messaging_client.MessagingClient`.
This module is the Python library-private publisher behind ``gg.logs()`` and the
root ``logging`` capture hook.
"""

import json
import logging
import queue
import re
import sys
import threading
import time
import traceback
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, Iterable, Optional

from edgecommons.messaging.identity import MessageIdentity
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.qos import Qos
from edgecommons.uns import Uns, UnsClass


LOG_SCHEMA = "edgecommons.log.v1"
LOG_HEADER_NAME = "log"
LOG_HEADER_VERSION = "1.0"

_LEVEL_TO_TOPIC = {
    "TRACE": "trace",
    "DEBUG": "debug",
    "INFO": "info",
    "WARN": "warn",
    "WARNING": "warn",
    "ERROR": "error",
    "FATAL": "fatal",
    "CRITICAL": "fatal",
}
_LEVEL_VALUES = {
    "TRACE": 5,
    "DEBUG": logging.DEBUG,
    "INFO": logging.INFO,
    "WARN": logging.WARNING,
    "ERROR": logging.ERROR,
    "FATAL": logging.CRITICAL,
}
_SENSITIVE_KEYS = {
    "authorization",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
}
_DEFAULT_REDACTION_PATTERNS = (
    re.compile(r"(?i)\b(authorization)(\s*[:=]\s*)(?:bearer\s+)?([^\s,;]+)"),
    re.compile(
        r"(?i)\b(password|passwd|pwd|secret|token|api[_-]?key)"
        r"(\s*[:=]\s*)([^\s,;]+)"
    ),
    re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+"),
)
_RESERVED_RECORD_ATTRS = frozenset(
    logging.makeLogRecord({}).__dict__.keys()
) | {"message", "asctime", "taskName"}


@dataclass(frozen=True)
class LogPublishQueueConfig:
    max_records: int = 1000
    on_full: str = "dropOldest"


@dataclass(frozen=True)
class LogRedactionConfig:
    enabled: bool = True
    replacement: str = "***"
    extra_patterns: tuple = field(default_factory=tuple)


@dataclass(frozen=True)
class LogPublishConfig:
    enabled: bool = False
    destination: str = "local"
    min_level: str = "INFO"
    capture_native: bool = True
    capture_console: bool = False
    max_record_bytes: int = 8192
    queue: LogPublishQueueConfig = field(default_factory=LogPublishQueueConfig)
    redaction: LogRedactionConfig = field(default_factory=LogRedactionConfig)

    @staticmethod
    def from_logging_config(logging_config: Optional[Dict[str, Any]]) -> "LogPublishConfig":
        if logging_config is None:
            return LogPublishConfig()
        if not isinstance(logging_config, dict):
            raise ValueError("logging config must be an object")
        publish = logging_config.get("publish")
        if publish is None:
            return LogPublishConfig()
        if not isinstance(publish, dict):
            raise ValueError("logging.publish must be an object")
        _reject_unknown("logging.publish", publish, {
            "enabled",
            "destination",
            "minLevel",
            "captureNative",
            "captureConsole",
            "maxRecordBytes",
            "queue",
            "redaction",
        })
        queue_cfg = _parse_queue(publish.get("queue"))
        redaction_cfg = _parse_redaction(publish.get("redaction"))
        destination = _string_choice(
            publish.get("destination", "local"),
            "logging.publish.destination",
            {"local", "northbound"},
        )
        min_level = _parse_level_name(publish.get("minLevel", "INFO"))
        return LogPublishConfig(
            enabled=_bool(publish.get("enabled", False), "logging.publish.enabled"),
            destination=destination,
            min_level=min_level,
            capture_native=_bool(
                publish.get("captureNative", True),
                "logging.publish.captureNative",
            ),
            capture_console=_bool(
                publish.get("captureConsole", False),
                "logging.publish.captureConsole",
            ),
            max_record_bytes=_positive_int(
                publish.get("maxRecordBytes", 8192),
                "logging.publish.maxRecordBytes",
            ),
            queue=queue_cfg,
            redaction=redaction_cfg,
        )

    @property
    def min_level_value(self) -> int:
        return _LEVEL_VALUES[self.min_level]


@dataclass
class LogRecord:
    """A public log-bus record for ``LogService.publish``."""

    level: str
    logger: str
    message: str
    timestamp: Optional[str] = None
    sequence: Optional[int] = None
    thread: Optional[str] = None
    fields: Optional[Dict[str, Any]] = None
    error: Optional[str] = None
    truncated: Optional[bool] = None
    dropped: Optional[int] = None


class _Stats:
    def __init__(self):
        self.published = 0
        self.failed = 0
        self.dropped = 0
        self.redacted = 0
        self.truncated = 0


class _LogCaptureHandler(logging.Handler):
    def __init__(self, service: "LogService"):
        super().__init__(level=0)
        self._service = service
        self._edgecommons_log_bus_handler = True

    def emit(self, record: logging.LogRecord) -> None:
        self._service.capture_logging_record(record)


class _ConsoleCapture:
    def __init__(self, service: "LogService", stream, logger_name: str, level: str):
        self._service = service
        self._stream = stream
        self._logger_name = logger_name
        self._level = level
        self._buffer = ""

    def write(self, text):
        self._stream.write(text)
        if self._service._is_publishing():
            return
        self._buffer += text
        while "\n" in self._buffer:
            line, self._buffer = self._buffer.split("\n", 1)
            if line:
                self._service.publish(
                    LogRecord(level=self._level, logger=self._logger_name, message=line)
                )

    def flush(self):
        self._stream.flush()

    def isatty(self):
        return self._stream.isatty()

    @property
    def encoding(self):
        return getattr(self._stream, "encoding", None)


class LogService:
    """Public log bus facade returned by ``gg.logs()``."""

    _current = None

    def __init__(self, messaging_client=None):
        self._config = LogPublishConfig()
        self._config_manager = None
        self._messaging = messaging_client
        self._uns = None
        self._queue = queue.Queue(maxsize=self._config.queue.max_records)
        self._stats = _Stats()
        self._lock = threading.RLock()
        self._sequence = 0
        self._stop = threading.Event()
        self._worker = threading.Thread(
            target=self._run,
            name="edgecommons-log-publisher",
            daemon=True,
        )
        self._worker.start()
        self._handler = _LogCaptureHandler(self)
        self._publishing = threading.local()
        self._stdout_original = None
        self._stderr_original = None
        LogService._current = self

    @staticmethod
    def current():
        return LogService._current

    def install_handler(self) -> None:
        root = logging.getLogger()
        if self._handler not in root.handlers:
            root.addHandler(self._handler)

    def configure(self, config_manager, messaging_client=None) -> None:
        logging_config = config_manager.get_logging_config()
        publish_config = logging_config.get_publish_config()
        identity = config_manager.get_component_identity()
        uns = None
        if identity is not None:
            uns = Uns(
                identity.with_instance(MessageIdentity.DEFAULT_INSTANCE),
                config_manager.is_topic_include_root(),
            )
        with self._lock:
            if publish_config.queue.max_records != self._config.queue.max_records:
                self._resize_queue_locked(publish_config.queue.max_records)
            self._config = publish_config
            self._config_manager = config_manager
            if messaging_client is not None:
                self._messaging = messaging_client
            self._uns = uns
            self._set_console_capture_locked(publish_config.capture_console and publish_config.enabled)
        self.install_handler()

    def on_configuration_change(self, configuration) -> bool:
        if self._config_manager is None:
            return True
        self.configure(self._config_manager, self._messaging)
        return True

    def publish(self, record: LogRecord) -> None:
        body, topic_level = self._prepare_record(record, dropped=None)
        self._enqueue(body, topic_level)

    def flush(self, timeout: Optional[float] = None) -> bool:
        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            with self._queue.all_tasks_done:
                if self._queue.unfinished_tasks == 0:
                    return True
            if deadline is not None and time.monotonic() >= deadline:
                return False
            time.sleep(0.01)

    def stats(self) -> Dict[str, int]:
        with self._lock:
            return {
                "published": self._stats.published,
                "failed": self._stats.failed,
                "dropped": self._stats.dropped,
                "redacted": self._stats.redacted,
                "truncated": self._stats.truncated,
                "queued": self._queue.qsize(),
            }

    def capture_logging_record(self, record: logging.LogRecord) -> None:
        if self._is_publishing():
            return
        if record.name.startswith("edgecommons.logs"):
            return
        with self._lock:
            config = self._config
        if (
            not config.enabled
            or not config.capture_native
            or record.levelno < config.min_level_value
        ):
            return
        fields = {
            key: value
            for key, value in record.__dict__.items()
            if key not in _RESERVED_RECORD_ATTRS and not key.startswith("_")
        }
        error = None
        if record.exc_info:
            error = "".join(traceback.format_exception(*record.exc_info)).rstrip()
        elif record.exc_text:
            error = record.exc_text
        captured = LogRecord(
            timestamp=_iso_from_epoch(record.created),
            level=_canonical_level(record.levelname),
            logger=record.name,
            message=record.getMessage(),
            thread=record.threadName,
            fields=fields or None,
            error=error,
        )
        self.publish(captured)

    def close(self) -> None:
        self.flush(timeout=5)
        with self._lock:
            self._set_console_capture_locked(False)
        root = logging.getLogger()
        if self._handler in root.handlers:
            root.removeHandler(self._handler)
        self._stop.set()
        self._worker.join(timeout=5)
        if LogService._current is self:
            LogService._current = None

    def _enqueue(self, body: Dict[str, Any], topic_level: str) -> None:
        item = (topic_level, body)
        try:
            self._queue.put_nowait(item)
            return
        except queue.Full:
            pass
        with self._lock:
            self._stats.dropped += 1
        try:
            self._queue.get_nowait()
            self._queue.task_done()
        except queue.Empty:
            pass
        body["dropped"] = int(body.get("dropped", 0)) + 1
        try:
            self._queue.put_nowait(item)
        except queue.Full:
            with self._lock:
                self._stats.dropped += 1

    def _run(self) -> None:
        while not self._stop.is_set() or not self._queue.empty():
            try:
                topic_level, body = self._queue.get(timeout=0.05)
            except queue.Empty:
                continue
            try:
                self._publish_body(topic_level, body)
            finally:
                self._queue.task_done()

    def _publish_body(self, topic_level: str, body: Dict[str, Any]) -> None:
        with self._lock:
            config = self._config
            messaging = self._messaging
            uns = self._uns
            config_manager = self._config_manager
        if messaging is None:
            from edgecommons.messaging.messaging_client import MessagingClient

            messaging = MessagingClient
        if uns is None or config_manager is None:
            with self._lock:
                self._stats.failed += 1
            return
        if not _messaging_connected(messaging):
            with self._lock:
                self._stats.failed += 1
            return
        topic = uns.topic(UnsClass.LOG, topic_level)
        msg = (
            MessageBuilder.create(LOG_HEADER_NAME, LOG_HEADER_VERSION)
            .with_config(config_manager)
            .with_instance(MessageIdentity.DEFAULT_INSTANCE)
            .with_timestamp(body["timestamp"])
            .with_timestamp_ms(_epoch_millis(body["timestamp"]))
            .with_payload(body)
            .build()
        )
        try:
            self._publishing.active = True
            if config.destination == "northbound":
                messaging._publish_reserved_northbound(topic, msg, Qos.AT_LEAST_ONCE)
            else:
                messaging._publish_reserved(topic, msg)
            with self._lock:
                self._stats.published += 1
        except Exception:
            with self._lock:
                self._stats.failed += 1
        finally:
            self._publishing.active = False

    def _prepare_record(self, record: LogRecord, dropped: Optional[int]) -> tuple:
        if record is None:
            raise ValueError("record must not be None")
        level = _canonical_level(record.level)
        topic_level = _LEVEL_TO_TOPIC[level]
        timestamp = record.timestamp or _now_iso()
        sequence = record.sequence if record.sequence is not None else self._next_sequence()
        body = {
            "schema": LOG_SCHEMA,
            "timestamp": timestamp,
            "level": level,
            "logger": _required_string(record.logger, "record.logger"),
            "message": _required_string(record.message, "record.message"),
            "sequence": sequence,
        }
        if record.thread:
            body["thread"] = record.thread
        if record.fields:
            body["fields"] = record.fields
        if record.error:
            body["error"] = record.error
        if record.truncated:
            body["truncated"] = True
        if dropped is not None and dropped > 0:
            body["dropped"] = int(dropped)
        elif record.dropped is not None and record.dropped > 0:
            body["dropped"] = int(record.dropped)
        body = self._redact(body)
        body = self._truncate(body)
        return body, topic_level

    def _next_sequence(self) -> int:
        with self._lock:
            self._sequence += 1
            return self._sequence

    def _redact(self, body: Dict[str, Any]) -> Dict[str, Any]:
        with self._lock:
            config = self._config.redaction
        if not config.enabled:
            return body
        changed = False
        redacted = {}
        for key, value in body.items():
            new_value = _redact_value(key, value, config.replacement, config.extra_patterns)
            changed = changed or new_value != value
            redacted[key] = new_value
        if changed:
            with self._lock:
                self._stats.redacted += 1
        return redacted

    def _truncate(self, body: Dict[str, Any]) -> Dict[str, Any]:
        with self._lock:
            max_record_bytes = self._config.max_record_bytes
        if _json_len(body) <= max_record_bytes:
            return body
        body = dict(body)
        body["truncated"] = True
        for key in ("message", "error"):
            if key in body and _json_len(body) > max_record_bytes:
                body[key] = _shrink_text(str(body[key]), max_record_bytes, body)
        for key in ("fields", "thread", "error"):
            if key in body and _json_len(body) > max_record_bytes:
                body.pop(key, None)
        while _json_len(body) > max_record_bytes and body.get("message"):
            body["message"] = body["message"][:-1]
        if _json_len(body) > max_record_bytes:
            body["message"] = ""
        with self._lock:
            self._stats.truncated += 1
        return body

    def _resize_queue_locked(self, max_records: int) -> None:
        old = self._queue
        new_queue = queue.Queue(maxsize=max_records)
        while not old.empty() and not new_queue.full():
            try:
                new_queue.put_nowait(old.get_nowait())
                old.task_done()
            except queue.Empty:
                break
        dropped = 0
        while not old.empty():
            try:
                old.get_nowait()
                old.task_done()
                dropped += 1
            except queue.Empty:
                break
        self._stats.dropped += dropped
        self._queue = new_queue

    def _set_console_capture_locked(self, enabled: bool) -> None:
        if enabled and self._stdout_original is None:
            self._stdout_original = sys.stdout
            self._stderr_original = sys.stderr
            sys.stdout = _ConsoleCapture(self, self._stdout_original, "console.stdout", "INFO")
            sys.stderr = _ConsoleCapture(self, self._stderr_original, "console.stderr", "ERROR")
        elif not enabled and self._stdout_original is not None:
            sys.stdout = self._stdout_original
            sys.stderr = self._stderr_original
            self._stdout_original = None
            self._stderr_original = None

    def _is_publishing(self) -> bool:
        return bool(getattr(self._publishing, "active", False))


def _reject_unknown(path: str, obj: Dict[str, Any], allowed: Iterable[str]) -> None:
    allowed_set = set(allowed)
    unknown = sorted(k for k in obj if k not in allowed_set)
    if unknown:
        raise ValueError(f"{path} has unsupported key(s): {', '.join(unknown)}")


def _parse_queue(raw: Any) -> LogPublishQueueConfig:
    if raw is None:
        return LogPublishQueueConfig()
    if not isinstance(raw, dict):
        raise ValueError("logging.publish.queue must be an object")
    _reject_unknown("logging.publish.queue", raw, {"maxRecords", "onFull"})
    on_full = raw.get("onFull", "dropOldest")
    if on_full != "dropOldest":
        raise ValueError("logging.publish.queue.onFull must be 'dropOldest'")
    return LogPublishQueueConfig(
        max_records=_positive_int(
            raw.get("maxRecords", 1000),
            "logging.publish.queue.maxRecords",
        ),
        on_full=on_full,
    )


def _parse_redaction(raw: Any) -> LogRedactionConfig:
    if raw is None:
        return LogRedactionConfig()
    if not isinstance(raw, dict):
        raise ValueError("logging.publish.redaction must be an object")
    _reject_unknown(
        "logging.publish.redaction",
        raw,
        {"enabled", "replacement", "extraPatterns"},
    )
    replacement = raw.get("replacement", "***")
    if not isinstance(replacement, str):
        raise ValueError("logging.publish.redaction.replacement must be a string")
    extra = raw.get("extraPatterns", [])
    if not isinstance(extra, list) or not all(isinstance(p, str) for p in extra):
        raise ValueError("logging.publish.redaction.extraPatterns must be a string array")
    return LogRedactionConfig(
        enabled=_bool(raw.get("enabled", True), "logging.publish.redaction.enabled"),
        replacement=replacement,
        extra_patterns=tuple(re.compile(pattern) for pattern in extra),
    )


def _bool(value: Any, path: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{path} must be a boolean")
    return value


def _positive_int(value: Any, path: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f"{path} must be a positive integer")
    return value


def _string_choice(value: Any, path: str, choices: set) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{path} must be a string")
    normalized = value.strip().lower()
    if normalized not in choices:
        raise ValueError(f"{path} must be one of: {', '.join(sorted(choices))}")
    return normalized


def _parse_level_name(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("logging.publish.minLevel must be a string")
    level = value.strip().upper()
    if level == "WARNING":
        level = "WARN"
    if level == "CRITICAL":
        level = "FATAL"
    if level not in _LEVEL_VALUES:
        raise ValueError("logging.publish.minLevel must be TRACE, DEBUG, INFO, WARN, ERROR, or FATAL")
    return level


def _canonical_level(value: str) -> str:
    level = _parse_level_name(value)
    return "WARN" if level == "WARNING" else "FATAL" if level == "CRITICAL" else level


def _required_string(value: Any, path: str) -> str:
    if not isinstance(value, str) or value == "":
        raise ValueError(f"{path} must be a non-empty string")
    return value


def _messaging_connected(messaging) -> bool:
    connected = getattr(messaging, "connected", None)
    if connected is None:
        return True
    try:
        return bool(connected() if callable(connected) else connected)
    except Exception:
        return False


def _redact_value(key: str, value: Any, replacement: str, extra_patterns: tuple):
    if key.lower() in _SENSITIVE_KEYS:
        return replacement
    if isinstance(value, str):
        updated = value
        for pattern in _DEFAULT_REDACTION_PATTERNS:
            if pattern.pattern.lower().startswith("(?i)\\bbearer"):
                updated = pattern.sub(f"Bearer {replacement}", updated)
            else:
                updated = pattern.sub(lambda m: f"{m.group(1)}{m.group(2)}{replacement}", updated)
        for pattern in extra_patterns:
            updated = pattern.sub(replacement, updated)
        return updated
    if isinstance(value, dict):
        return {
            k: _redact_value(str(k), v, replacement, extra_patterns)
            for k, v in value.items()
        }
    if isinstance(value, list):
        return [_redact_value(key, item, replacement, extra_patterns) for item in value]
    return value


def _json_len(body: Dict[str, Any]) -> int:
    return len(json.dumps(body, separators=(",", ":"), default=str).encode("utf-8"))


def _shrink_text(text: str, max_record_bytes: int, body: Dict[str, Any]) -> str:
    over = _json_len(body) - max_record_bytes
    if over <= 0:
        return text
    keep = max(0, len(text) - over - 16)
    return text[:keep]


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _iso_from_epoch(seconds: float) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


def _epoch_millis(timestamp: str) -> int:
    try:
        normalized = timestamp[:-1] + "+00:00" if timestamp.endswith("Z") else timestamp
        return int(datetime.fromisoformat(normalized).timestamp() * 1000)
    except Exception:
        return int(datetime.now(timezone.utc).timestamp() * 1000)
