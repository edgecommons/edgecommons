//! C ABI (feature `cabi`) — the FFI boundary for the Phase-2 language bindings (Java/Panama,
//! Python, Node). Mirrors `include/ggstreamlog.h`. Built into a `cdylib`.
//!
//! Contract: every entry point wraps the core in `catch_unwind` so a Rust panic never crosses the
//! boundary (it becomes `GGSL_ERR_PANIC`). Inputs are borrowed for the call; error strings are
//! heap-allocated and freed with [`ggsl_str_free`]; `ggsl_service`/`ggsl_stream` are heap handles
//! freed with [`ggsl_shutdown`]/[`ggsl_stream_free`]. `ggsl_append`/`ggsl_flush` are thread-safe.

use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use crate::config::StreamingConfig;
use crate::error::GgStreamError;
use crate::log::EmbeddedLog;
use crate::record::Record;
use crate::service::StreamService;

// ----- status codes (must match ggstreamlog.h `ggsl_status`) -----
const GGSL_OK: c_int = 0;
const GGSL_ERR_CONFIG: c_int = 1;
const GGSL_ERR_IO: c_int = 2;
const GGSL_ERR_CORRUPT: c_int = 3;
const GGSL_ERR_FULL: c_int = 4;
const GGSL_ERR_UNKNOWN_STREAM: c_int = 5;
const GGSL_ERR_SINK: c_int = 6;
const GGSL_ERR_PANIC: c_int = 7;
const GGSL_ERR_INVALID_ARG: c_int = 8;

/// Opaque to C: the owned [`StreamService`].
pub struct GgslService {
    svc: StreamService,
}

/// Opaque to C: a caller-owned handle to one stream's durable log (a ref-count, so it outlives the
/// service for append/flush).
pub struct GgslStream {
    log: Arc<EmbeddedLog>,
}

/// Numeric stats struct — must match `ggsl_stats_t` field order/types in ggstreamlog.h.
#[repr(C)]
pub struct GgslStats {
    appended_total: u64,
    exported_total: u64,
    dropped_total: u64,
    retries_total: u64,
    failed_total: u64,
    backlog: u64,
    disk_bytes: u64,
    acked_offset: u64,
    next_offset: u64,
    oldest_unacked_age_ms: u64,
}

fn status_of(e: &GgStreamError) -> c_int {
    match e {
        GgStreamError::Io(_) => GGSL_ERR_IO,
        GgStreamError::Corrupt(_) => GGSL_ERR_CORRUPT,
        GgStreamError::Config(_) => GGSL_ERR_CONFIG,
        GgStreamError::BufferFull => GGSL_ERR_FULL,
        GgStreamError::UnknownStream(_) => GGSL_ERR_UNKNOWN_STREAM,
        GgStreamError::Sink(_) => GGSL_ERR_SINK,
    }
}

/// Set `*err` to a heap C string (no-op if `err` is null). Caller frees with [`ggsl_str_free`].
unsafe fn set_err(err: *mut *mut c_char, msg: &str) {
    if err.is_null() {
        return;
    }
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    unsafe { *err = c.into_raw() };
}

/// Run `f`, converting a panic into `GGSL_ERR_PANIC` (panics must not cross the FFI boundary).
fn guard(err: *mut *mut c_char, f: impl FnOnce() -> c_int) -> c_int {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(_) => {
            unsafe { set_err(err, "panic in ggstreamlog") };
            GGSL_ERR_PANIC
        }
    }
}

/// Open + recover every stream in `config_json` (the `streaming` section). On success `*out` gets a
/// service handle; free it with [`ggsl_shutdown`].
///
/// # Safety
/// `config_json` must be a valid NUL-terminated C string; `out` a valid `*mut *mut GgslService`.
#[no_mangle]
pub unsafe extern "C" fn ggsl_open(
    config_json: *const c_char,
    out: *mut *mut GgslService,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if config_json.is_null() || out.is_null() {
            unsafe { set_err(err, "null argument") };
            return GGSL_ERR_INVALID_ARG;
        }
        let json = match unsafe { CStr::from_ptr(config_json) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                unsafe { set_err(err, "config_json is not valid UTF-8") };
                return GGSL_ERR_CONFIG;
            }
        };
        let cfg: StreamingConfig = match serde_json::from_str(json) {
            Ok(c) => c,
            Err(e) => {
                unsafe { set_err(err, &format!("config: {e}")) };
                return GGSL_ERR_CONFIG;
            }
        };
        match StreamService::open(cfg) {
            Ok(svc) => {
                unsafe { *out = Box::into_raw(Box::new(GgslService { svc })) };
                GGSL_OK
            }
            Err(e) => {
                unsafe { set_err(err, &e.to_string()) };
                status_of(&e)
            }
        }
    })
}

/// Get a handle to the named stream. `*out` is caller-owned; free with [`ggsl_stream_free`].
///
/// # Safety
/// `service` must be a live handle from [`ggsl_open`]; `name` a valid C string; `out` non-null.
#[no_mangle]
pub unsafe extern "C" fn ggsl_stream_get(
    service: *mut GgslService,
    name: *const c_char,
    out: *mut *mut GgslStream,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if service.is_null() || name.is_null() || out.is_null() {
            unsafe { set_err(err, "null argument") };
            return GGSL_ERR_INVALID_ARG;
        }
        let svc = unsafe { &*service };
        let name = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                unsafe { set_err(err, "name is not valid UTF-8") };
                return GGSL_ERR_INVALID_ARG;
            }
        };
        match svc.svc.stream(name) {
            Some(log) => {
                unsafe { *out = Box::into_raw(Box::new(GgslStream { log })) };
                GGSL_OK
            }
            None => {
                unsafe { set_err(err, &format!("unknown stream: {name}")) };
                GGSL_ERR_UNKNOWN_STREAM
            }
        }
    })
}

/// Append one record. `pk`/`payload` are borrowed for the call. If `out_offset` is non-null it
/// receives the log head (next offset) after the append. Thread-safe.
///
/// # Safety
/// `stream` must be a live handle; `pk`/`payload` valid for their lengths (may be null iff len 0).
#[no_mangle]
pub unsafe extern "C" fn ggsl_append(
    stream: *mut GgslStream,
    pk: *const u8,
    pk_len: u16,
    ts_ms: u64,
    payload: *const u8,
    payload_len: u32,
    out_offset: *mut u64,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if stream.is_null() {
            unsafe { set_err(err, "null stream") };
            return GGSL_ERR_INVALID_ARG;
        }
        let s = unsafe { &*stream };
        let pk_bytes: &[u8] = if pk_len == 0 || pk.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(pk, pk_len as usize) }
        };
        let payload_bytes: &[u8] = if payload_len == 0 || payload.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(payload, payload_len as usize) }
        };
        // Partition keys are UTF-8 strings; recover lossily from the raw bytes.
        let rec = Record::new(
            String::from_utf8_lossy(pk_bytes).into_owned(),
            ts_ms,
            payload_bytes.to_vec(),
        );
        match s.log.append(&rec) {
            Ok(()) => {
                if !out_offset.is_null() {
                    unsafe { *out_offset = s.log.stats().next_offset.saturating_sub(1) };
                }
                GGSL_OK
            }
            Err(e) => {
                unsafe { set_err(err, &e.to_string()) };
                status_of(&e)
            }
        }
    })
}

/// Force this stream's buffer durably to disk (does not wait for export).
///
/// # Safety
/// `stream` must be a live handle from [`ggsl_stream_get`].
#[no_mangle]
pub unsafe extern "C" fn ggsl_flush(stream: *mut GgslStream, err: *mut *mut c_char) -> c_int {
    guard(err, || {
        if stream.is_null() {
            unsafe { set_err(err, "null stream") };
            return GGSL_ERR_INVALID_ARG;
        }
        let s = unsafe { &*stream };
        match s.log.flush() {
            Ok(()) => GGSL_OK,
            Err(e) => {
                unsafe { set_err(err, &e.to_string()) };
                status_of(&e)
            }
        }
    })
}

/// Write a stats snapshot for the named stream into `out`.
///
/// # Safety
/// `service` must be live; `name` a valid C string; `out` a valid `*mut GgslStats`.
#[no_mangle]
pub unsafe extern "C" fn ggsl_stats(
    service: *mut GgslService,
    name: *const c_char,
    out: *mut GgslStats,
) -> c_int {
    guard(std::ptr::null_mut(), || {
        if service.is_null() || name.is_null() || out.is_null() {
            return GGSL_ERR_INVALID_ARG;
        }
        let svc = unsafe { &*service };
        let name = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(_) => return GGSL_ERR_INVALID_ARG,
        };
        match svc.svc.stats(name) {
            Some(s) => {
                unsafe {
                    *out = GgslStats {
                        appended_total: s.appended_total,
                        exported_total: s.exported_total,
                        dropped_total: s.dropped_total,
                        retries_total: s.retries_total,
                        failed_total: s.failed_total,
                        backlog: s.backlog,
                        disk_bytes: s.disk_bytes,
                        acked_offset: s.acked_offset,
                        next_offset: s.next_offset,
                        oldest_unacked_age_ms: s.oldest_unacked_age_ms,
                    }
                };
                GGSL_OK
            }
            None => GGSL_ERR_UNKNOWN_STREAM,
        }
    })
}

/// Release a stream handle from [`ggsl_stream_get`]. NULL is a no-op.
///
/// # Safety
/// `stream` must be a handle from [`ggsl_stream_get`] not already freed.
#[no_mangle]
pub unsafe extern "C" fn ggsl_stream_free(stream: *mut GgslStream) {
    if !stream.is_null() {
        drop(unsafe { Box::from_raw(stream) });
    }
}

/// Flush + stop + free the service. NULL is a no-op.
///
/// # Safety
/// `service` must be a handle from [`ggsl_open`] not already freed.
#[no_mangle]
pub unsafe extern "C" fn ggsl_shutdown(service: *mut GgslService) {
    if !service.is_null() {
        // StreamService::drop stops engines and flushes each buffer.
        drop(unsafe { Box::from_raw(service) });
    }
}

/// Free a heap string returned via an `err` out-parameter. NULL is a no-op.
///
/// # Safety
/// `s` must be a string from a `*err` out-parameter not already freed.
#[no_mangle]
pub unsafe extern "C" fn ggsl_str_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
