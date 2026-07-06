//! C ABI (feature `cabi`) — the FFI boundary for the Phase-2 language bindings (Java/Panama,
//! Python, Node). Mirrors `include/edgestreamlog.h`. Built into a `cdylib`.
//!
//! Contract: every entry point wraps the core in `catch_unwind` so a Rust panic never crosses the
//! boundary (it becomes `ESL_ERR_PANIC`). Inputs are borrowed for the call; error strings are
//! heap-allocated and freed with [`esl_str_free`]; `esl_service`/`esl_stream` are heap handles
//! freed with [`esl_shutdown`]/[`esl_stream_free`]. `esl_append`/`esl_flush` are thread-safe.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, Once};

use crate::config::{SinkConfig, StreamingConfig};
use crate::error::{EdgeStreamError, Result};
use crate::export::{CallbackSink, ExportRecord, SendOutcome, Sink};
use crate::log::EmbeddedLog;
use crate::record::Record;
use crate::service::StreamService;

// ----- status codes (must match edgestreamlog.h `esl_status`) -----
const ESL_OK: c_int = 0;
const ESL_ERR_CONFIG: c_int = 1;
const ESL_ERR_IO: c_int = 2;
const ESL_ERR_CORRUPT: c_int = 3;
const ESL_ERR_FULL: c_int = 4;
const ESL_ERR_UNKNOWN_STREAM: c_int = 5;
const ESL_ERR_SINK: c_int = 6;
const ESL_ERR_PANIC: c_int = 7;
const ESL_ERR_INVALID_ARG: c_int = 8;

/// Opaque to C: the owned [`StreamService`].
pub struct EslService {
    svc: StreamService,
}

/// Opaque to C: a caller-owned handle to one stream's durable log (a ref-count, so it outlives the
/// service for append/flush).
pub struct EslStream {
    log: Arc<EmbeddedLog>,
}

/// Numeric stats struct — must match `esl_stats_t` field order/types in edgestreamlog.h.
#[repr(C)]
pub struct EslStats {
    pub appended_total: u64,
    pub exported_total: u64,
    pub dropped_total: u64,
    pub retries_total: u64,
    pub failed_total: u64,
    pub backlog: u64,
    pub disk_bytes: u64,
    pub acked_offset: u64,
    pub next_offset: u64,
    pub oldest_unacked_age_ms: u64,
}

fn status_of(e: &EdgeStreamError) -> c_int {
    match e {
        EdgeStreamError::Io(_) => ESL_ERR_IO,
        EdgeStreamError::Corrupt(_) => ESL_ERR_CORRUPT,
        EdgeStreamError::Config(_) => ESL_ERR_CONFIG,
        EdgeStreamError::BufferFull => ESL_ERR_FULL,
        EdgeStreamError::UnknownStream(_) => ESL_ERR_UNKNOWN_STREAM,
        EdgeStreamError::Sink(_) => ESL_ERR_SINK,
    }
}

/// Set `*err` to a heap C string (no-op if `err` is null). Caller frees with [`esl_str_free`].
unsafe fn set_err(err: *mut *mut c_char, msg: &str) {
    if err.is_null() {
        return;
    }
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    unsafe { *err = c.into_raw() };
}

/// Run `f`, converting a panic into `ESL_ERR_PANIC` (panics must not cross the FFI boundary).
fn guard(err: *mut *mut c_char, f: impl FnOnce() -> c_int) -> c_int {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(_) => {
            unsafe { set_err(err, "panic in edgestreamlog") };
            ESL_ERR_PANIC
        }
    }
}

/// Open + recover every stream in `config_json` (the `streaming` section). On success `*out` gets a
/// service handle; free it with [`esl_shutdown`].
///
/// # Safety
/// `config_json` must be a valid NUL-terminated C string; `out` a valid `*mut *mut EslService`.
#[no_mangle]
pub unsafe extern "C" fn esl_open(
    config_json: *const c_char,
    out: *mut *mut EslService,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if config_json.is_null() || out.is_null() {
            unsafe { set_err(err, "null argument") };
            return ESL_ERR_INVALID_ARG;
        }
        let json = match unsafe { CStr::from_ptr(config_json) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                unsafe { set_err(err, "config_json is not valid UTF-8") };
                return ESL_ERR_CONFIG;
            }
        };
        let cfg: StreamingConfig = match serde_json::from_str(json) {
            Ok(c) => c,
            Err(e) => {
                unsafe { set_err(err, &format!("config: {e}")) };
                return ESL_ERR_CONFIG;
            }
        };
        match StreamService::open_with(cfg, &cabi_sink_factory) {
            Ok(svc) => {
                unsafe { *out = Box::into_raw(Box::new(EslService { svc })) };
                ESL_OK
            }
            Err(e) => {
                unsafe { set_err(err, &e.to_string()) };
                status_of(&e)
            }
        }
    })
}

/// The C-ABI sink factory: like the in-core default factory for Kinesis/Kafka, but additionally
/// binds a [`CallbackSink`] to the registered host sink callback for `SinkConfig::Callback` streams.
/// If no host callback is registered, a callback stream is buffer-only (a clear warning is logged by
/// the service). Kinesis/Kafka selection is delegated to the in-core default factory.
fn cabi_sink_factory(name: &str, sink: &SinkConfig) -> Result<Option<Box<dyn Sink>>> {
    match sink {
        SinkConfig::Callback { id } => {
            // Bind to the registered host sink callback, if any.
            let bound = match SINK_CB.lock() {
                Ok(g) => g.is_some(),
                Err(_) => false,
            };
            if !bound {
                tracing::warn!(
                    stream = %name,
                    "no host sink callback registered (call esl_set_sink_callback before \
                     esl_open); callback stream is buffer-only — records persist but will not \
                     be exported"
                );
                return Ok(None);
            }
            let id = id.clone();
            let stream = name.to_string();
            let cb = CallbackSink::new(Box::new(move |batch: &[ExportRecord<'_>]| {
                invoke_host_sink(&stream, id.as_deref(), batch)
            }));
            Ok(Some(Box::new(cb)))
        }
        // Kinesis / Kafka: identical to the in-core default factory.
        other => crate::service::default_sink_factory_pub(name, other),
    }
}

/// Get a handle to the named stream. `*out` is caller-owned; free with [`esl_stream_free`].
///
/// # Safety
/// `service` must be a live handle from [`esl_open`]; `name` a valid C string; `out` non-null.
#[no_mangle]
pub unsafe extern "C" fn esl_stream_get(
    service: *mut EslService,
    name: *const c_char,
    out: *mut *mut EslStream,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if service.is_null() || name.is_null() || out.is_null() {
            unsafe { set_err(err, "null argument") };
            return ESL_ERR_INVALID_ARG;
        }
        let svc = unsafe { &*service };
        let name = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                unsafe { set_err(err, "name is not valid UTF-8") };
                return ESL_ERR_INVALID_ARG;
            }
        };
        match svc.svc.stream(name) {
            Some(log) => {
                unsafe { *out = Box::into_raw(Box::new(EslStream { log })) };
                ESL_OK
            }
            None => {
                unsafe { set_err(err, &format!("unknown stream: {name}")) };
                ESL_ERR_UNKNOWN_STREAM
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
pub unsafe extern "C" fn esl_append(
    stream: *mut EslStream,
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
            return ESL_ERR_INVALID_ARG;
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
                ESL_OK
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
/// `stream` must be a live handle from [`esl_stream_get`].
#[no_mangle]
pub unsafe extern "C" fn esl_flush(stream: *mut EslStream, err: *mut *mut c_char) -> c_int {
    guard(err, || {
        if stream.is_null() {
            unsafe { set_err(err, "null stream") };
            return ESL_ERR_INVALID_ARG;
        }
        let s = unsafe { &*stream };
        match s.log.flush() {
            Ok(()) => ESL_OK,
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
/// `service` must be live; `name` a valid C string; `out` a valid `*mut EslStats`.
#[no_mangle]
pub unsafe extern "C" fn esl_stats(
    service: *mut EslService,
    name: *const c_char,
    out: *mut EslStats,
) -> c_int {
    guard(std::ptr::null_mut(), || {
        if service.is_null() || name.is_null() || out.is_null() {
            return ESL_ERR_INVALID_ARG;
        }
        let svc = unsafe { &*service };
        let name = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(_) => return ESL_ERR_INVALID_ARG,
        };
        match svc.svc.stats(name) {
            Some(s) => {
                unsafe {
                    *out = EslStats {
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
                ESL_OK
            }
            None => ESL_ERR_UNKNOWN_STREAM,
        }
    })
}

/// Release a stream handle from [`esl_stream_get`]. NULL is a no-op.
///
/// # Safety
/// `stream` must be a handle from [`esl_stream_get`] not already freed.
#[no_mangle]
pub unsafe extern "C" fn esl_stream_free(stream: *mut EslStream) {
    if !stream.is_null() {
        drop(unsafe { Box::from_raw(stream) });
    }
}

/// Flush + stop + free the service. NULL is a no-op.
///
/// # Safety
/// `service` must be a handle from [`esl_open`] not already freed.
#[no_mangle]
pub unsafe extern "C" fn esl_shutdown(service: *mut EslService) {
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
pub unsafe extern "C" fn esl_str_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

// ----- log forwarding: core `tracing` events -> host logger (log4j2 / logging / Node) -----

/// Host log callback: `(user_data, level, target, message)`. Level: 1=ERROR..5=TRACE. The string
/// pointers are valid only for the duration of the call.
type EslLogCb = extern "C" fn(*mut c_void, c_int, *const c_char, *const c_char);

struct LogSink {
    cb: EslLogCb,
    /// Host pointer, stored as `usize` so the global is `Send`/`Sync`; cast back when invoking.
    user_data: usize,
}

static LOG_SINK: Mutex<Option<LogSink>> = Mutex::new(None);
static LOG_INIT: Once = Once::new();

fn level_to_int(level: &tracing::Level) -> c_int {
    match *level {
        tracing::Level::ERROR => 1,
        tracing::Level::WARN => 2,
        tracing::Level::INFO => 3,
        tracing::Level::DEBUG => 4,
        tracing::Level::TRACE => 5,
    }
}

/// Collects an event's `message` + remaining fields into a single string.
#[derive(Default)]
struct MsgVisitor {
    message: String,
    fields: String,
}

impl tracing::field::Visit for MsgVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }
}

/// A `tracing` layer that forwards each event to the registered host callback.
struct CallbackLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CallbackLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Copy the callback out, then release the lock before the upcall into the host.
        let cbud = match LOG_SINK.lock() {
            Ok(g) => g.as_ref().map(|s| (s.cb, s.user_data)),
            Err(_) => return,
        };
        let Some((cb, ud)) = cbud else { return };

        let meta = event.metadata();
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        let full = if v.fields.is_empty() { v.message } else { format!("{}{}", v.message, v.fields) };
        let target = CString::new(meta.target().replace('\0', " ")).unwrap_or_default();
        let msg = CString::new(full.replace('\0', " ")).unwrap_or_default();
        cb(ud as *mut c_void, level_to_int(meta.level()), target.as_ptr(), msg.as_ptr());
    }
}

/// Register (or clear, with `cb = NULL`) a callback that receives the core's log events, so the host
/// logger emits them. Idempotent; the forwarding subscriber is installed on first registration.
///
/// # Safety
/// `cb` must be a valid function pointer (or null); `user_data` is passed back verbatim and must
/// remain valid for as long as a callback is registered.
#[no_mangle]
pub unsafe extern "C" fn esl_set_log_callback(cb: Option<EslLogCb>, user_data: *mut c_void) -> c_int {
    guard(std::ptr::null_mut(), || {
        match cb {
            Some(cb) => {
                if let Ok(mut g) = LOG_SINK.lock() {
                    *g = Some(LogSink { cb, user_data: user_data as usize });
                }
                LOG_INIT.call_once(|| {
                    use tracing_subscriber::layer::SubscriberExt;
                    use tracing_subscriber::util::SubscriberInitExt;
                    // Forward everything; the host logger applies its own level filter. Ignore the
                    // error if a global subscriber is already installed (e.g. in-process Rust host).
                    let _ = tracing_subscriber::registry().with(CallbackLayer).try_init();
                });
            }
            None => {
                if let Ok(mut g) = LOG_SINK.lock() {
                    *g = None;
                }
            }
        }
        ESL_OK
    })
}

// ----- host sink callback: the export engine drains a Callback stream through the host -----

// Outcome status codes the host writes into `EslSinkOutcome.status` (distinct from `esl_status`).
/// Every record in the batch was stored.
const ESL_SINK_ALL_ACKED: c_int = 0;
/// Some records (listed in `failed_offsets`) were not stored; retry just those.
const ESL_SINK_PARTIAL: c_int = 1;
/// The whole batch failed but may succeed later (disconnected / throttled / 5xx). Retried.
const ESL_SINK_FAILED_RETRYABLE: c_int = 2;
/// The whole batch failed and will not succeed on retry; the engine still re-delivers it on the
/// next loop (it cannot know it is permanent), but the host should have dropped/logged it.
const ESL_SINK_FAILED_PERMANENT: c_int = 3;

/// One record passed to the host sink callback. All pointers borrow the export batch and are valid
/// ONLY for the duration of the call. Mirrors `esl_sink_record_t` in edgestreamlog.h.
#[repr(C)]
pub struct EslSinkRecord {
    /// Log offset of this record (use it to populate `failed_offsets` for a partial outcome).
    pub offset: u64,
    /// Record timestamp (epoch millis) as supplied to `esl_append`.
    pub ts_ms: u64,
    /// Partition key bytes (UTF-8; the metrics layer uses the namespace). May be null iff `pk_len`==0.
    pub pk: *const u8,
    pub pk_len: usize,
    /// Record payload bytes (the compact `{namespace, datum}` JSON for CloudWatch). May be null iff 0.
    pub payload: *const u8,
    pub payload_len: usize,
}

/// The host fills this to report a batch's [`SendOutcome`]. Mirrors `esl_sink_outcome_t`.
///
/// The core supplies `failed_offsets` pre-allocated with room for `failed_cap` entries (== the batch
/// length). For a `ESL_SINK_PARTIAL` outcome the host writes the offsets that were NOT stored into
/// that buffer and sets `failed_count`; for any other status `failed_count` is ignored.
#[repr(C)]
pub struct EslSinkOutcome {
    /// One of the `ESL_SINK_*` constants. Defaults to `ESL_SINK_FAILED_RETRYABLE` if the host
    /// leaves it untouched (so an unwritten outcome is retried, never silently acked).
    pub status: c_int,
    /// Core-owned, host-written-into array for partial failures (capacity == batch length).
    pub failed_offsets: *mut u64,
    /// Capacity of `failed_offsets` (number of u64 slots).
    pub failed_cap: usize,
    /// Host writes the number of failed offsets here (only read for `ESL_SINK_PARTIAL`).
    pub failed_count: usize,
}

/// Host sink callback: `(user_data, records, n, *mut outcome) -> int`. Invoked on the export thread
/// with a borrowed batch; the host writes the outcome and returns `ESL_OK` (non-zero is treated as
/// a retryable failure). Must be thread-safe and return promptly (it blocks that stream's drain).
type EslSinkCb =
    extern "C" fn(*mut c_void, *const EslSinkRecord, usize, *mut EslSinkOutcome) -> c_int;

struct SinkCbReg {
    cb: EslSinkCb,
    /// Host pointer as `usize` so the global is `Send`/`Sync`; cast back when invoking.
    user_data: usize,
}

static SINK_CB: Mutex<Option<SinkCbReg>> = Mutex::new(None);

/// Marshal one export batch across the boundary into the registered host sink and map the host's
/// `EslSinkOutcome` back onto a [`SendOutcome`]. Runs on the export engine thread.
///
/// If no callback is registered (cleared after `esl_open`), the batch is reported as a retryable
/// failure so the engine holds it (at-least-once) rather than dropping it.
fn invoke_host_sink(stream: &str, _id: Option<&str>, batch: &[ExportRecord<'_>]) -> SendOutcome {
    let reg = match SINK_CB.lock() {
        Ok(g) => g.as_ref().map(|r| (r.cb, r.user_data)),
        Err(_) => None,
    };
    let Some((cb, ud)) = reg else {
        return SendOutcome::Failed {
            retryable: true,
            error: format!("no host sink callback registered for stream '{stream}'"),
        };
    };

    // Build the borrowed C view of the batch (pointers valid only for this call).
    let recs: Vec<EslSinkRecord> = batch
        .iter()
        .map(|r| EslSinkRecord {
            offset: r.offset,
            ts_ms: r.ts_ms,
            pk: r.partition_key.as_ptr(),
            pk_len: r.partition_key.len(),
            payload: r.payload.as_ptr(),
            payload_len: r.payload.len(),
        })
        .collect();

    // Core-owned scratch for partial failed offsets (capacity == batch length).
    let mut failed: Vec<u64> = vec![0; batch.len()];
    let mut outcome = EslSinkOutcome {
        status: ESL_SINK_FAILED_RETRYABLE, // default: retried if the host leaves it untouched
        failed_offsets: failed.as_mut_ptr(),
        failed_cap: failed.len(),
        failed_count: 0,
    };

    // The host callback may itself panic across the boundary; contain it.
    let rc = catch_unwind(AssertUnwindSafe(|| {
        cb(ud as *mut c_void, recs.as_ptr(), recs.len(), &mut outcome as *mut EslSinkOutcome)
    }));
    let rc = match rc {
        Ok(rc) => rc,
        Err(_) => {
            return SendOutcome::Failed {
                retryable: true,
                error: format!("host sink callback panicked for stream '{stream}'"),
            };
        }
    };
    if rc != ESL_OK {
        return SendOutcome::Failed {
            retryable: true,
            error: format!("host sink callback returned status {rc} for stream '{stream}'"),
        };
    }

    match outcome.status {
        ESL_SINK_ALL_ACKED => SendOutcome::AllAcked,
        ESL_SINK_PARTIAL => {
            let n = outcome.failed_count.min(failed.len());
            let failed_offsets = failed[..n].to_vec();
            if failed_offsets.is_empty() {
                // Host reported partial with zero failures — treat as fully acked.
                SendOutcome::AllAcked
            } else {
                SendOutcome::Partial { failed_offsets }
            }
        }
        ESL_SINK_FAILED_PERMANENT => {
            SendOutcome::Failed { retryable: false, error: "host sink: permanent failure".into() }
        }
        // ESL_SINK_FAILED_RETRYABLE and any unrecognized status -> retryable failure.
        _ => SendOutcome::Failed { retryable: true, error: "host sink: retryable failure".into() },
    }
}

/// Register (or clear, with `cb = NULL`) the host sink callback that drains `callback`-type streams.
/// Call this BEFORE [`esl_open`]: the binding is captured per stream at open time, so a stream
/// opened with no callback registered is buffer-only until reopened. `user_data` is passed back
/// verbatim and must outlive the service.
///
/// # Safety
/// `cb` must be a valid function pointer (or null); `user_data` is passed back verbatim and must
/// remain valid for as long as a callback is registered (i.e. until the service is shut down or the
/// callback is cleared).
#[no_mangle]
pub unsafe extern "C" fn esl_set_sink_callback(
    cb: Option<EslSinkCb>,
    user_data: *mut c_void,
) -> c_int {
    guard(std::ptr::null_mut(), || {
        match cb {
            Some(cb) => {
                if let Ok(mut g) = SINK_CB.lock() {
                    *g = Some(SinkCbReg { cb, user_data: user_data as usize });
                }
            }
            None => {
                if let Ok(mut g) = SINK_CB.lock() {
                    *g = None;
                }
            }
        }
        ESL_OK
    })
}
