"""
High-level Python API for the ``ggstreamlog`` telemetry-streaming core, over the C ABI (ctypes).

Gives Python components the same durable store-and-forward streaming + config schema as the Rust
and Java libraries. Mirrors ``gg.streams()``.
"""
from __future__ import annotations

import json
from ctypes import byref, c_char_p, c_uint64, c_void_p
from dataclasses import dataclass
from typing import List, Optional

from . import _native
from ._native import GgStreamError

__all__ = ["StreamService", "StreamHandle", "StreamStats", "GgStreamError"]


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
    """A producer handle to one telemetry stream. Thread-safe ``append``."""

    def __init__(self, ptr: c_void_p, name: str):
        self._ptr: Optional[c_void_p] = ptr
        self._name = name

    @property
    def name(self) -> str:
        return self._name

    def append(self, partition_key: str, timestamp_ms: int, payload: bytes) -> None:
        """Append one record; returns once durable per the stream's fsync policy.

        Blocks per the ``onFull`` backpressure policy; raises :class:`GgStreamError` on a
        buffer/IO/sink error (e.g. ``ERR_FULL`` under ``rejectNew``).
        """
        if self._ptr is None:
            raise RuntimeError("StreamHandle is closed")
        pk = partition_key.encode("utf-8")
        if len(pk) > 0xFFFF:
            raise ValueError("partition_key exceeds 65535 bytes")
        lib = _native.lib()
        err = c_char_p()
        rc = lib.ggsl_append(self._ptr, pk if pk else None, len(pk), int(timestamp_ms),
                             payload if payload else None, len(payload) if payload else 0,
                             None, byref(err))
        _native.check(rc, err)

    def flush(self) -> None:
        """Force this stream's buffer durably to disk (does not wait for export)."""
        if self._ptr is None:
            raise RuntimeError("StreamHandle is closed")
        err = c_char_p()
        _native.check(_native.lib().ggsl_flush(self._ptr, byref(err)), err)

    def close(self) -> None:
        """Release the handle. Idempotent."""
        if self._ptr is not None:
            _native.lib().ggsl_stream_free(self._ptr)
            self._ptr = None

    def __enter__(self) -> "StreamHandle":
        return self

    def __exit__(self, *exc) -> bool:
        self.close()
        return False


class StreamService:
    """Owns the native streaming service: opens streams from config, runs export, hands out handles."""

    def __init__(self, ptr: c_void_p):
        self._ptr: Optional[c_void_p] = ptr

    @staticmethod
    def open(config_json: str) -> "StreamService":
        """Open every stream in ``config_json`` (the ``streaming`` section; templates pre-resolved)."""
        lib = _native.lib()
        out = c_void_p()
        err = c_char_p()
        rc = lib.ggsl_open(config_json.encode("utf-8"), byref(out), byref(err))
        _native.check(rc, err)
        return StreamService(out)

    def stream(self, name: str) -> StreamHandle:
        """A handle to the named stream (raises ``ERR_UNKNOWN_STREAM`` if not configured)."""
        out = c_void_p()
        err = c_char_p()
        rc = _native.lib().ggsl_stream_get(self._require(), name.encode("utf-8"), byref(out), byref(err))
        _native.check(rc, err)
        return StreamHandle(out, name)

    def stats(self, name: str) -> StreamStats:
        """A stats snapshot for the named stream (raises ``ERR_UNKNOWN_STREAM`` if not configured)."""
        st = _native.GgslStats()
        rc = _native.lib().ggsl_stats(self._require(), name.encode("utf-8"), byref(st))
        if rc != _native.OK:
            raise GgStreamError(rc)
        return StreamStats(
            st.appended_total, st.exported_total, st.dropped_total, st.retries_total,
            st.failed_total, st.backlog, st.disk_bytes, st.acked_offset, st.next_offset,
            st.oldest_unacked_age_ms)

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
        if self._ptr is not None:
            _native.lib().ggsl_shutdown(self._ptr)
            self._ptr = None

    def _require(self) -> c_void_p:
        if self._ptr is None:
            raise RuntimeError("StreamService is closed")
        return self._ptr

    def __enter__(self) -> "StreamService":
        return self

    def __exit__(self, *exc) -> bool:
        self.close()
        return False
