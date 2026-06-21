"""
High-level Python API for the ``ggstreamlog`` telemetry-streaming core.

Wraps the ``ggstreamlog_native`` PyO3 module (a native extension built from the shared Rust core,
installed as a wheel) with friendly classes + error translation, giving Python components the same
durable store-and-forward streaming + config schema as the Rust, Java, and Node libraries. Mirrors
``gg.streams()``.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from typing import List, Optional

import ggstreamlog_native as _native

__all__ = ["StreamService", "StreamHandle", "StreamStats", "GgStreamError"]


class GgStreamError(Exception):
    """Raised when a native streaming call fails. ``code`` mirrors ``ggsl_status``."""

    def __init__(self, code: int, message: Optional[str] = None):
        self.code = code
        super().__init__(f"ggstreamlog error {code}" + (f": {message}" if message else ""))


def _translate(exc: BaseException) -> GgStreamError:
    """Convert a native ``ggstreamlog_native.GgStreamError`` (args = (code, message)) to ours."""
    args = getattr(exc, "args", ())
    code = args[0] if len(args) >= 1 and isinstance(args[0], int) else -1
    message = args[1] if len(args) >= 2 else None
    return GgStreamError(code, message)


@dataclass(frozen=True)
class StreamStats:
    """A snapshot of one stream's buffer + export progress (mirrors ``ggsl_stats_t``)."""

    appended_total: int
    exported_total: int
    dropped_total: int
    retries_total: int
    failed_total: int
    backlog: int
    disk_bytes: int
    acked_offset: int
    next_offset: int
    oldest_unacked_age_ms: int


class StreamHandle:
    """A producer handle to one telemetry stream."""

    def __init__(self, native_handle):
        self._h = native_handle

    def append(self, partition_key: str, timestamp_ms: int, payload: bytes) -> None:
        """Append one record; returns once durable per the stream's fsync policy."""
        try:
            self._h.append(partition_key, int(timestamp_ms), payload if payload else b"")
        except _native.GgStreamError as e:
            raise _translate(e) from None

    def flush(self) -> None:
        """Force this stream's buffer durably to disk (does not wait for export)."""
        try:
            self._h.flush()
        except _native.GgStreamError as e:
            raise _translate(e) from None

    def close(self) -> None:
        """Release the handle (the native buffer ref is dropped by GC). Idempotent."""
        self._h = None

    def __enter__(self) -> "StreamHandle":
        return self

    def __exit__(self, *exc) -> bool:
        self.close()
        return False


class StreamService:
    """Owns the native streaming service: opens streams from config, runs export, hands out handles."""

    def __init__(self, native_service):
        self._svc = native_service

    @staticmethod
    def open(config_json: str) -> "StreamService":
        """Open every stream in ``config_json`` (the ``streaming`` section; templates pre-resolved)."""
        try:
            return StreamService(_native.StreamService.open(config_json))
        except _native.GgStreamError as e:
            raise _translate(e) from None

    def stream(self, name: str) -> StreamHandle:
        """A handle to the named stream (raises ``GgStreamError`` ERR_UNKNOWN_STREAM if absent)."""
        try:
            return StreamHandle(self._svc.stream(name))
        except _native.GgStreamError as e:
            raise _translate(e) from None

    def stats(self, name: str) -> StreamStats:
        """A stats snapshot for the named stream (raises ERR_UNKNOWN_STREAM if absent)."""
        try:
            s = self._svc.stats(name)
        except _native.GgStreamError as e:
            raise _translate(e) from None
        return StreamStats(
            s.appended_total, s.exported_total, s.dropped_total, s.retries_total,
            s.failed_total, s.backlog, s.disk_bytes, s.acked_offset, s.next_offset,
            s.oldest_unacked_age_ms)

    @staticmethod
    def stream_names(config_json: str) -> List[str]:
        """The stream names declared in a ``streaming`` config document (empty if none/invalid)."""
        try:
            doc = json.loads(config_json)
        except (ValueError, TypeError):
            return []
        streams = doc.get("streams") if isinstance(doc, dict) else None
        if not isinstance(streams, list):
            return []
        return [s["name"] for s in streams if isinstance(s, dict) and "name" in s]

    def close(self) -> None:
        """Flush every buffer, stop the export engines, and free the service. Idempotent."""
        if self._svc is not None:
            self._svc.close()
            self._svc = None

    def __enter__(self) -> "StreamService":
        return self

    def __exit__(self, *exc) -> bool:
        self.close()
        return False
