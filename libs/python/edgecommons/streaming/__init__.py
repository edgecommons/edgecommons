"""Telemetry streaming for Python components — durable store-and-forward over the shared Rust
``edgestreamlog`` core (C ABI / ctypes). See :class:`StreamService`."""
from .metrics_bridge import StreamMetricsBridge
from .service import EdgeStreamError, StreamHandle, StreamService, StreamStats

__all__ = [
    "StreamService",
    "StreamHandle",
    "StreamStats",
    "StreamMetricsBridge",
    "EdgeStreamError",
]
