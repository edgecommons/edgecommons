"""
Low-level ctypes binding to the ``ggstreamlog`` C ABI (``include/ggstreamlog.h``).

Loads the native ``cdylib`` once per process, declares the ``ggsl_*`` prototypes, and forwards the
core's log events into the standard :mod:`logging` system. The higher-level
:class:`ggcommons.streaming.service.StreamService` builds on this.

The library is located from (in order): the ``GGSTREAMLOG_LIBRARY_PATH`` env var, the bundled
package resource ``ggcommons/streaming/native/<os>-<arch>/<lib>``, or the platform loader path.
"""
from __future__ import annotations

import ctypes
import logging
import os
import platform
import threading
from ctypes import (CFUNCTYPE, POINTER, Structure, c_char_p, c_int, c_uint16,
                    c_uint32, c_uint64, c_void_p)

logger = logging.getLogger("ggstreamlog")

# ggsl_status codes (must match ggstreamlog.h).
OK = 0
ERR_CONFIG = 1
ERR_IO = 2
ERR_CORRUPT = 3
ERR_FULL = 4
ERR_UNKNOWN_STREAM = 5
ERR_SINK = 6
ERR_PANIC = 7
ERR_INVALID_ARG = 8


class GgStreamError(Exception):
    """Raised when a native ``ggsl_*`` call returns a non-zero status."""

    def __init__(self, code: int, message: str | None = None):
        self.code = code
        super().__init__(f"ggstreamlog error {code}" + (f": {message}" if message else ""))


class GgslStats(Structure):
    """Mirrors the native ``ggsl_stats_t`` (10 unsigned 64-bit counters)."""

    _fields_ = [
        ("appended_total", c_uint64),
        ("exported_total", c_uint64),
        ("dropped_total", c_uint64),
        ("retries_total", c_uint64),
        ("failed_total", c_uint64),
        ("backlog", c_uint64),
        ("disk_bytes", c_uint64),
        ("acked_offset", c_uint64),
        ("next_offset", c_uint64),
        ("oldest_unacked_age_ms", c_uint64),
    ]


_LOG_CB = CFUNCTYPE(None, c_void_p, c_int, c_char_p, c_char_p)
_LEVELS = {1: logging.ERROR, 2: logging.WARNING, 3: logging.INFO, 4: logging.DEBUG, 5: logging.DEBUG}

_lib = None
_lib_lock = threading.Lock()
_log_cb_ref = None  # keep the CFUNCTYPE alive for the process lifetime


def _os_arch() -> str:
    system = platform.system().lower()
    os_tag = "windows" if system.startswith("win") else "darwin" if system == "darwin" else "linux"
    machine = platform.machine().lower()
    arch = {"amd64": "x86_64", "x86_64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(
        machine, machine)
    return f"{os_tag}-{arch}"


def _lib_filename() -> str:
    system = platform.system().lower()
    if system.startswith("win"):
        return "ggstreamlog.dll"
    if system == "darwin":
        return "libggstreamlog.dylib"
    return "libggstreamlog.so"


def _resolve_path() -> str:
    explicit = os.environ.get("GGSTREAMLOG_LIBRARY_PATH")
    if explicit and os.path.exists(explicit):
        return explicit
    bundled = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           "native", _os_arch(), _lib_filename())
    if os.path.exists(bundled):
        return bundled
    return _lib_filename()  # let the OS loader search its default paths


def _on_log(user_data, level, target, message):  # noqa: ARG001 - C signature
    try:
        t = target.decode("utf-8", "replace") if target else "ggstreamlog"
        m = message.decode("utf-8", "replace") if message else ""
        logging.getLogger(t).log(_LEVELS.get(level, logging.DEBUG), m)
    except Exception:  # pragma: no cover - must never propagate into native
        pass


def lib():
    """Return the loaded native library, loading + wiring it on first use."""
    global _lib, _log_cb_ref
    if _lib is not None:
        return _lib
    with _lib_lock:
        if _lib is not None:
            return _lib
        path = _resolve_path()
        try:
            loaded = ctypes.CDLL(path)
        except OSError as exc:
            raise GgStreamError(
                ERR_IO,
                f"could not load ggstreamlog native library '{path}': {exc}. "
                f"Set GGSTREAMLOG_LIBRARY_PATH or bundle it at "
                f"ggcommons/streaming/native/{_os_arch()}/{_lib_filename()}.",
            ) from exc

        loaded.ggsl_open.argtypes = [c_char_p, POINTER(c_void_p), POINTER(c_char_p)]
        loaded.ggsl_open.restype = c_int
        loaded.ggsl_stream_get.argtypes = [c_void_p, c_char_p, POINTER(c_void_p), POINTER(c_char_p)]
        loaded.ggsl_stream_get.restype = c_int
        loaded.ggsl_append.argtypes = [c_void_p, c_char_p, c_uint16, c_uint64, c_char_p, c_uint32,
                                       POINTER(c_uint64), POINTER(c_char_p)]
        loaded.ggsl_append.restype = c_int
        loaded.ggsl_flush.argtypes = [c_void_p, POINTER(c_char_p)]
        loaded.ggsl_flush.restype = c_int
        loaded.ggsl_stats.argtypes = [c_void_p, c_char_p, POINTER(GgslStats)]
        loaded.ggsl_stats.restype = c_int
        loaded.ggsl_stream_free.argtypes = [c_void_p]
        loaded.ggsl_stream_free.restype = None
        loaded.ggsl_shutdown.argtypes = [c_void_p]
        loaded.ggsl_shutdown.restype = None
        loaded.ggsl_str_free.argtypes = [c_char_p]
        loaded.ggsl_str_free.restype = None
        loaded.ggsl_set_log_callback.argtypes = [_LOG_CB, c_void_p]
        loaded.ggsl_set_log_callback.restype = c_int

        cb = _LOG_CB(_on_log)
        loaded.ggsl_set_log_callback(cb, None)
        _log_cb_ref = cb
        _lib = loaded
        return _lib


def check(rc: int, err: c_char_p) -> None:
    """Raise :class:`GgStreamError` (freeing the native error string) when ``rc`` is non-zero."""
    if rc == OK:
        return
    message = None
    if err.value is not None:
        message = err.value.decode("utf-8", "replace")
        lib().ggsl_str_free(err)
    raise GgStreamError(rc, message)
