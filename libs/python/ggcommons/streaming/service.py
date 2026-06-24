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
from typing import Callable, List, Optional, Sequence, Tuple, Union

# The native core (built out-of-band via maturin from libs/rust-streamlog) is an OPTIONAL
# runtime dependency: importing ggcommons.streaming must not hard-fail a component that doesn't
# use streaming. Defer the failure to first use, with an actionable message (parity with the
# TS optional-addon / Rust feature-gate posture).
try:
    import ggstreamlog_native as _native
    _NATIVE_IMPORT_ERROR: Optional[ImportError] = None
except ImportError as _e:  # pragma: no cover - exercised only when the wheel is absent
    _native = None
    _NATIVE_IMPORT_ERROR = _e

__all__ = [
    "StreamService",
    "StreamHandle",
    "StreamStats",
    "GgStreamError",
    "ExportRecord",
    "SinkCallback",
    "SinkOutcome",
]

# One record handed to a host sink callback: (offset, partition_key, timestamp_ms, payload).
ExportRecord = Tuple[int, bytes, int, bytes]
# A host sink callback's return value (see ``StreamService.open_with_callback``):
#   * ``None`` / falsy            -> the whole batch was accepted (committed).
#   * a list of int offsets       -> those offsets failed (retried); the rest were accepted.
#   * ``("failed", error)`` / str -> the whole batch failed (retried; nothing committed).
SinkOutcome = Union[None, Sequence[int], Tuple[str, str], str]
SinkCallback = Callable[[List[ExportRecord]], SinkOutcome]


def _require_native():
    """Return the native module or raise a clear error if the wheel was never installed."""
    if _native is None:
        raise GgStreamError(
            -1,
            "telemetry streaming requires the 'ggstreamlog-native' wheel (build it from "
            "libs/rust-streamlog via maturin and pip install it); native module not importable"
            + (f": {_NATIVE_IMPORT_ERROR}" if _NATIVE_IMPORT_ERROR else ""),
        )
    return _native


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


# Native error type(s) for `except` clauses. When the native module is absent this is an empty
# tuple (catches nothing), so the actionable _require_native() error from open() propagates.
_NativeError = (_native.GgStreamError,) if _native is not None else ()


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
        except _NativeError as e:
            raise _translate(e) from None

    def flush(self) -> None:
        """Force this stream's buffer durably to disk (does not wait for export)."""
        try:
            self._h.flush()
        except _NativeError as e:
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
            return StreamService(_require_native().StreamService.open(config_json))
        except _NativeError as e:
            raise _translate(e) from None

    @staticmethod
    def open_with_callback(config_json: str, callback: SinkCallback) -> "StreamService":
        """Open every stream in ``config_json``, binding ``callback`` as the export sink for every
        stream whose sink is ``{"type": "callback"}`` (the durable CloudWatch metrics drain / a
        caller's bring-your-own-sink). Kinesis/Kafka streams are built natively as in :meth:`open`.

        The native export engine invokes ``callback(records)`` on its background thread — one call
        per batch — reacquiring the GIL for the duration of the call (the engine thread blocks on
        the result). ``records`` is a list of :data:`ExportRecord` tuples
        ``(offset, partition_key, timestamp_ms, payload)``; the return value is a
        :data:`SinkOutcome`. A callback that raises is treated as a retryable failure (the batch is
        re-delivered, never committed), so a transient cloud outage cannot lose buffered data.
        """
        try:
            return StreamService(
                _require_native().StreamService.open_with_callback(config_json, callback)
            )
        except _NativeError as e:
            raise _translate(e) from None

    def stream(self, name: str) -> StreamHandle:
        """A handle to the named stream (raises ``GgStreamError`` ERR_UNKNOWN_STREAM if absent)."""
        try:
            return StreamHandle(self._svc.stream(name))
        except _NativeError as e:
            raise _translate(e) from None

    def stats(self, name: str) -> StreamStats:
        """A stats snapshot for the named stream (raises ERR_UNKNOWN_STREAM if absent)."""
        try:
            s = self._svc.stats(name)
        except _NativeError as e:
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
