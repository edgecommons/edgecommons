//! C-ABI host-sink-callback integration tests (feature `cabi`).
//!
//! Drives a `callback`-type stream end-to-end through the real C entry points
//! (`esl_set_sink_callback` → `esl_open` → `esl_append` → host callback → `esl_stats`),
//! exactly as the Java/Panama binding will. Exercises:
//!   * AllAcked: the callback receives the batch and `exported_total` advances.
//!   * Partial: only the failed offsets are re-delivered (at-least-once retry).
//!   * Failed(retryable): the batch is held and re-delivered until the host acks (the disconnect
//!     fault-injection case — nothing is lost while the "cloud" is down).
//!   * No callback registered: a callback stream is buffer-only (records persist, do not export).
//!
//! `esl_set_sink_callback` registers a PROCESS-GLOBAL callback, so these tests must not run
//! concurrently; they serialize on a shared mutex.

#![cfg(feature = "cabi")]

use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use edgestreamlog::ffi::{
    esl_append, esl_open, esl_set_sink_callback, esl_shutdown, esl_stats, esl_stream_free,
    esl_stream_get, esl_str_free, EslService, EslSinkOutcome, EslSinkRecord, EslStats,
    EslStream,
};

// Mirror of the C status codes the host writes into the outcome (kept local to the test).
const SINK_ALL_ACKED: c_int = 0;
const SINK_PARTIAL: c_int = 1;
const SINK_FAILED_RETRYABLE: c_int = 2;
const SINK_FAILED_PERMANENT: c_int = 3;
const ESL_OK: c_int = 0;

/// Serializes all tests in this file (the sink callback + tracing subscriber are process-global).
fn test_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

// ----- shared host-side state the C callbacks read/write through `user_data` -----

#[derive(Default)]
struct HostState {
    /// Every (offset, payload) the host "stored" (acked), in delivery order.
    delivered: Mutex<Vec<(u64, Vec<u8>)>>,
    /// Number of times the callback was invoked (batches seen).
    calls: AtomicUsize,
    /// Partial mode: fail these offsets exactly once (then ack on retry). Drains as offsets fail.
    fail_offsets_once: Mutex<Vec<u64>>,
    /// Disconnect mode: while > 0, reject the whole batch (retryable). The test flips it to 0 to
    /// "reconnect". Also counts rejected attempts so the test can confirm a real disconnect window.
    disconnected: AtomicU64,
    rejected_attempts: AtomicU64,
    /// Failure-mode selector for `cb_modes` (0=panic,1=non-zero rc,2=permanent,3=partial-zero).
    mode: AtomicU64,
}

/// Build a stream config JSON for a single `callback`-type durable stream rooted at `dir`.
fn callback_cfg(dir: &std::path::Path) -> CString {
    let path = dir.join("cw").to_string_lossy().replace('\\', "/");
    let json = format!(
        r#"{{"streams":[{{"name":"cw","sink":{{"type":"callback"}},
            "buffer":{{"path":"{path}","segmentBytes":65536,"maxDiskBytes":1048576,"onFull":"dropOldest"}},
            "batch":{{"maxRecords":500,"maxBytes":4194304,"maxLatencyMs":50}},
            "delivery":{{"pollIntervalMs":5,"maxRetries":-1,"backoffBaseMs":5,"backoffMaxMs":50}}}}]}}"#
    );
    CString::new(json).unwrap()
}

/// Open the service (assert OK), get the `cw` stream handle.
unsafe fn open_cw(cfg: &CString) -> (*mut EslService, *mut EslStream) {
    let mut svc: *mut EslService = std::ptr::null_mut();
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = esl_open(cfg.as_ptr(), &mut svc, &mut err);
    assert_eq!(rc, ESL_OK, "esl_open failed: {}", err_str(err));
    if !err.is_null() {
        esl_str_free(err);
    }
    let name = CString::new("cw").unwrap();
    let mut stream: *mut EslStream = std::ptr::null_mut();
    let mut err2: *mut c_char = std::ptr::null_mut();
    let rc = esl_stream_get(svc, name.as_ptr(), &mut stream, &mut err2);
    assert_eq!(rc, ESL_OK, "esl_stream_get failed: {}", err_str(err2));
    if !err2.is_null() {
        esl_str_free(err2);
    }
    (svc, stream)
}

unsafe fn err_str(err: *mut c_char) -> String {
    if err.is_null() {
        return "<none>".into();
    }
    std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned()
}

unsafe fn append(stream: *mut EslStream, ts_ms: u64, payload: &[u8]) {
    let pk = b"NamespaceX";
    let mut off: u64 = 0;
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = esl_append(
        stream,
        pk.as_ptr(),
        pk.len() as u16,
        ts_ms,
        payload.as_ptr(),
        payload.len() as u32,
        &mut off,
        &mut err,
    );
    assert_eq!(rc, ESL_OK, "esl_append failed: {}", err_str(err));
    if !err.is_null() {
        esl_str_free(err);
    }
}

unsafe fn stats(svc: *mut EslService) -> EslStats {
    let name = CString::new("cw").unwrap();
    let mut s: EslStats = std::mem::zeroed();
    let rc = esl_stats(svc, name.as_ptr(), &mut s);
    assert_eq!(rc, ESL_OK);
    s
}

/// Poll until `exported_total >= want` or `timeout`.
unsafe fn wait_exported(svc: *mut EslService, want: u64, timeout: Duration) -> u64 {
    let start = Instant::now();
    loop {
        let e = stats(svc).exported_total;
        if e >= want || start.elapsed() > timeout {
            return e;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ----- the C callbacks: one per behavior, all driven through *mut HostState user_data -----

extern "C" fn cb_all_acked(
    ud: *mut c_void,
    records: *const EslSinkRecord,
    n: usize,
    outcome: *mut EslSinkOutcome,
) -> c_int {
    let st = unsafe { &*(ud as *const HostState) };
    st.calls.fetch_add(1, Ordering::SeqCst);
    let recs = unsafe { std::slice::from_raw_parts(records, n) };
    let mut d = st.delivered.lock().unwrap();
    for r in recs {
        let payload = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
        // sanity: pk is the partition key bytes we appended
        let pk = unsafe { std::slice::from_raw_parts(r.pk, r.pk_len) };
        assert_eq!(pk, b"NamespaceX");
        d.push((r.offset, payload.to_vec()));
    }
    unsafe { (*outcome).status = SINK_ALL_ACKED };
    ESL_OK
}

extern "C" fn cb_partial(
    ud: *mut c_void,
    records: *const EslSinkRecord,
    n: usize,
    outcome: *mut EslSinkOutcome,
) -> c_int {
    let st = unsafe { &*(ud as *const HostState) };
    st.calls.fetch_add(1, Ordering::SeqCst);
    let recs = unsafe { std::slice::from_raw_parts(records, n) };
    let oc = unsafe { &mut *outcome };

    // Fail each configured offset EXACTLY ONCE, the first time it appears (robust to the engine
    // splitting the appends across batches). `fail_offsets_once` drains as offsets are failed; once
    // empty, the same offsets ack on their retry. This proves the Partial -> retry-just-those path.
    let mut to_fail = st.fail_offsets_once.lock().unwrap();
    let mut d = st.delivered.lock().unwrap();
    let mut failed: Vec<u64> = Vec::new();
    for r in recs {
        if let Some(pos) = to_fail.iter().position(|o| *o == r.offset) {
            to_fail.remove(pos);
            failed.push(r.offset);
        } else {
            let payload = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
            d.push((r.offset, payload.to_vec()));
        }
    }
    drop(to_fail);

    if failed.is_empty() {
        oc.status = SINK_ALL_ACKED;
    } else {
        // Write failed offsets into the core-supplied buffer.
        assert!(failed.len() <= oc.failed_cap, "failed_cap must be >= batch len");
        let out = unsafe { std::slice::from_raw_parts_mut(oc.failed_offsets, oc.failed_cap) };
        for (i, off) in failed.iter().enumerate() {
            out[i] = *off;
        }
        oc.failed_count = failed.len();
        oc.status = SINK_PARTIAL;
    }
    ESL_OK
}

extern "C" fn cb_disconnect(
    ud: *mut c_void,
    records: *const EslSinkRecord,
    n: usize,
    outcome: *mut EslSinkOutcome,
) -> c_int {
    let st = unsafe { &*(ud as *const HostState) };
    st.calls.fetch_add(1, Ordering::SeqCst);
    let oc = unsafe { &mut *outcome };

    // While "disconnected", reject the whole batch as retryable (nothing stored, nothing lost).
    if st.disconnected.load(Ordering::SeqCst) > 0 {
        st.rejected_attempts.fetch_add(1, Ordering::SeqCst);
        oc.status = SINK_FAILED_RETRYABLE;
        return ESL_OK;
    }
    // "Reconnected": store the batch.
    let recs = unsafe { std::slice::from_raw_parts(records, n) };
    let mut d = st.delivered.lock().unwrap();
    for r in recs {
        let payload = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
        d.push((r.offset, payload.to_vec()));
    }
    oc.status = SINK_ALL_ACKED;
    ESL_OK
}

/// Trips the chosen failure mode while "disconnected", then recovers to AllAcked.
/// `mode`: 1 = non-zero rc, 2 = permanent failure, 3 = partial with failed_count=0.
extern "C" fn cb_modes(
    ud: *mut c_void,
    records: *const EslSinkRecord,
    n: usize,
    outcome: *mut EslSinkOutcome,
) -> c_int {
    let st = unsafe { &*(ud as *const HostState) };
    st.calls.fetch_add(1, Ordering::SeqCst);
    let oc = unsafe { &mut *outcome };

    // Trip the failure mode only while still "disconnected"; then recover (AllAcked).
    if st.disconnected.load(Ordering::SeqCst) > 0 {
        st.rejected_attempts.fetch_add(1, Ordering::SeqCst);
        st.disconnected.fetch_sub(1, Ordering::SeqCst);
        match st.mode.load(Ordering::SeqCst) {
            1 => return 7,                                // non-zero rc -> retryable Failed
            2 => {
                oc.status = SINK_FAILED_PERMANENT;
                return ESL_OK;
            }
            3 => {
                // Partial but with zero failed offsets -> the core treats it as AllAcked, so the
                // host stores everything (a zero-failure partial means the whole batch succeeded).
                let recs = unsafe { std::slice::from_raw_parts(records, n) };
                let mut d = st.delivered.lock().unwrap();
                for r in recs {
                    let payload = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
                    d.push((r.offset, payload.to_vec()));
                }
                oc.status = SINK_PARTIAL;
                oc.failed_count = 0;
                return ESL_OK;
            }
            _ => {}
        }
    }
    // Recovered: store + ack.
    let recs = unsafe { std::slice::from_raw_parts(records, n) };
    let mut d = st.delivered.lock().unwrap();
    for r in recs {
        let payload = unsafe { std::slice::from_raw_parts(r.payload, r.payload_len) };
        d.push((r.offset, payload.to_vec()));
    }
    oc.status = SINK_ALL_ACKED;
    ESL_OK
}

// ----- tests -----

#[test]
fn all_acked_drains_batch_and_advances_exported() {
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let st = Box::new(HostState::default());
    let st_ptr: *const HostState = &*st;
    unsafe {
        let rc = esl_set_sink_callback(Some(cb_all_acked), st_ptr as *mut c_void);
        assert_eq!(rc, ESL_OK);

        let dir = tempfile::tempdir().unwrap();
        let cfg = callback_cfg(dir.path());
        let (svc, stream) = open_cw(&cfg);

        for i in 0..50u64 {
            append(stream, 1_700_000_000_000 + i, format!("datum-{i}").as_bytes());
        }
        let exported = wait_exported(svc, 50, Duration::from_secs(5));
        assert_eq!(exported, 50, "all 50 records should be exported via the host callback");

        let s = stats(svc);
        assert_eq!(s.appended_total, 50);
        assert_eq!(s.exported_total, 50);
        assert_eq!(s.backlog, 0);
        assert_eq!(st.delivered.lock().unwrap().len(), 50);
        assert!(st.calls.load(Ordering::SeqCst) >= 1);

        esl_stream_free(stream);
        esl_shutdown(svc);
        esl_set_sink_callback(None, std::ptr::null_mut());
    }
    drop(st);
}

#[test]
fn partial_redelivers_only_failed_offsets() {
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let st = Box::new(HostState::default());
    // Fail offsets 1 and 3 on the first send; they must be retried and then acked.
    *st.fail_offsets_once.lock().unwrap() = vec![1, 3];
    let st_ptr: *const HostState = &*st;
    unsafe {
        assert_eq!(esl_set_sink_callback(Some(cb_partial), st_ptr as *mut c_void), ESL_OK);

        let dir = tempfile::tempdir().unwrap();
        let cfg = callback_cfg(dir.path());
        let (svc, stream) = open_cw(&cfg);

        for i in 0..5u64 {
            append(stream, 1_700_000_000_000 + i, format!("d{i}").as_bytes());
        }
        let exported = wait_exported(svc, 5, Duration::from_secs(5));
        assert_eq!(exported, 5, "all 5 records eventually exported (the 2 partial-failed retried)");

        let s = stats(svc);
        assert_eq!(s.exported_total, 5);
        assert!(s.retries_total >= 1, "a partial failure must record a retry");

        // Offsets 1 and 3 were delivered twice? No — on retry the host acks them, so they land once
        // each in `delivered` (the first attempt did NOT store them). Total stored == 5.
        let delivered = st.delivered.lock().unwrap();
        let mut offs: Vec<u64> = delivered.iter().map(|(o, _)| *o).collect();
        offs.sort_unstable();
        offs.dedup();
        assert_eq!(offs, vec![0, 1, 2, 3, 4]);

        esl_stream_free(stream);
        esl_shutdown(svc);
        esl_set_sink_callback(None, std::ptr::null_mut());
    }
    drop(st);
}

#[test]
fn failed_retryable_holds_batch_until_reconnect_disconnect_fault_injection() {
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let st = Box::new(HostState::default());
    // Start "disconnected": the host rejects every batch as retryable until the test reconnects.
    st.disconnected.store(1, Ordering::SeqCst);
    let st_ptr: *const HostState = &*st;
    unsafe {
        assert_eq!(esl_set_sink_callback(Some(cb_disconnect), st_ptr as *mut c_void), ESL_OK);

        let dir = tempfile::tempdir().unwrap();
        let cfg = callback_cfg(dir.path());
        let (svc, stream) = open_cw(&cfg);

        for i in 0..10u64 {
            append(stream, 1_700_000_000_000 + i, format!("m{i}").as_bytes());
        }

        // During the lengthy "disconnect" the batch is held: nothing exported, backlog stays put
        // (no loss), the host keeps getting retried. Wait until we observe real reject attempts.
        let start = Instant::now();
        while st.rejected_attempts.load(Ordering::SeqCst) < 2 {
            assert!(start.elapsed() < Duration::from_secs(5), "expected the engine to retry the held batch");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mid = stats(svc);
        assert_eq!(mid.exported_total, 0, "nothing exports while disconnected");
        assert!(mid.backlog >= 1, "records are held on disk during the disconnect, not dropped");
        assert_eq!(st.delivered.lock().unwrap().len(), 0, "nothing stored host-side while down");

        // "Reconnect": the engine drains the held batch on the next attempt.
        st.disconnected.store(0, Ordering::SeqCst);
        let exported = wait_exported(svc, 10, Duration::from_secs(5));
        assert_eq!(exported, 10, "the buffered batch drains after reconnect (no data lost)");

        let s = stats(svc);
        assert_eq!(s.exported_total, 10);
        assert!(s.retries_total >= 1, "the disconnect must have driven retries");
        assert!(st.rejected_attempts.load(Ordering::SeqCst) >= 2, "the disconnect window had retries");
        assert_eq!(st.delivered.lock().unwrap().len(), 10);

        esl_stream_free(stream);
        esl_shutdown(svc);
        esl_set_sink_callback(None, std::ptr::null_mut());
    }
    drop(st);
}

#[test]
fn no_callback_registered_is_buffer_only() {
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        // Ensure no callback is bound.
        esl_set_sink_callback(None, std::ptr::null_mut());

        let dir = tempfile::tempdir().unwrap();
        let cfg = callback_cfg(dir.path());
        let (svc, stream) = open_cw(&cfg);

        for i in 0..5u64 {
            append(stream, 1_700_000_000_000 + i, format!("x{i}").as_bytes());
        }
        // Give the engine a chance — it should NOT export (buffer-only).
        std::thread::sleep(Duration::from_millis(100));
        let s = stats(svc);
        assert_eq!(s.appended_total, 5);
        assert_eq!(s.exported_total, 0, "no host callback => stream is buffer-only");
        assert_eq!(s.backlog, 5);

        esl_stream_free(stream);
        esl_shutdown(svc);
    }
}

#[test]
fn kinesis_sink_delegates_to_in_core_factory_buffer_only_without_feature() {
    // A non-callback (kinesis) stream opened via the C-ABI must delegate to the in-core default
    // factory. Without the `kinesis` cargo feature that yields a buffer-only stream (records persist,
    // do not export) — proving the C-ABI factory leaves the native sinks unchanged.
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        esl_set_sink_callback(None, std::ptr::null_mut());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k").to_string_lossy().replace('\\', "/");
        let json = format!(
            r#"{{"streams":[{{"name":"cw","sink":{{"type":"kinesis","streamName":"s"}},
                "buffer":{{"path":"{path}","segmentBytes":65536,"maxDiskBytes":1048576}},
                "delivery":{{"pollIntervalMs":5}}}}]}}"#
        );
        let cfg = CString::new(json).unwrap();
        let (svc, stream) = open_cw(&cfg);
        append(stream, 1_700_000_000_000, b"k0");
        std::thread::sleep(Duration::from_millis(60));
        let s = stats(svc);
        assert_eq!(s.appended_total, 1);
        assert_eq!(s.exported_total, 0, "kinesis stream is buffer-only without the kinesis feature");
        assert_eq!(s.backlog, 1);
        esl_stream_free(stream);
        esl_shutdown(svc);
    }
}

/// Drive each non-AllAcked outcome-mapping branch of the FFI marshaller: a non-zero return code, a
/// permanent-failure status, and a partial with zero failed offsets. Every one must be HELD
/// (retryable) or treated as acked — never silently lose data — and the records must all export once
/// the host recovers. (A Rust panic unwinding *out of* an `extern "C"` host callback is undefined /
/// aborts by design — a real Java/Python/Node sink throws on its own side and returns a status, so
/// that is not a representable C-ABI scenario and is not tested here.)
fn run_mode(mode: u64) {
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let st = Box::new(HostState::default());
    st.mode.store(mode, Ordering::SeqCst);
    // Trip the failure mode for the first few attempts, then recover.
    st.disconnected.store(2, Ordering::SeqCst);
    let st_ptr: *const HostState = &*st;
    unsafe {
        assert_eq!(esl_set_sink_callback(Some(cb_modes), st_ptr as *mut c_void), ESL_OK);

        let dir = tempfile::tempdir().unwrap();
        let cfg = callback_cfg(dir.path());
        let (svc, stream) = open_cw(&cfg);

        for i in 0..4u64 {
            append(stream, 1_700_000_000_000 + i, format!("v{i}").as_bytes());
        }
        // All records must eventually export: the failure modes hold the batch (at-least-once), the
        // partial-zero / permanent map cleanly, and the host recovers to AllAcked.
        let exported = wait_exported(svc, 4, Duration::from_secs(5));
        assert_eq!(exported, 4, "mode {mode}: all records export after the host recovers (no loss)");
        assert!(
            st.rejected_attempts.load(Ordering::SeqCst) >= 1,
            "mode {mode}: the injected failure must have been exercised"
        );

        esl_stream_free(stream);
        esl_shutdown(svc);
        esl_set_sink_callback(None, std::ptr::null_mut());
    }
    drop(st);
}

#[test]
fn host_callback_nonzero_rc_is_retried() {
    run_mode(1);
}

#[test]
fn callback_cleared_after_open_holds_batch_as_retryable() {
    // A stream opened WITH a callback binds an export closure that re-reads the global registration
    // on each drain. If the host clears the callback (esl_set_sink_callback(None)) after open, the
    // in-flight drain finds no callback and must HOLD the batch (retryable Failed) — never lose it.
    let _g = test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let st = Box::new(HostState::default());
    // Stay "disconnected" so the engine keeps the batch buffered while we clear the callback.
    st.disconnected.store(1, Ordering::SeqCst);
    let st_ptr: *const HostState = &*st;
    unsafe {
        assert_eq!(esl_set_sink_callback(Some(cb_disconnect), st_ptr as *mut c_void), ESL_OK);

        let dir = tempfile::tempdir().unwrap();
        let cfg = callback_cfg(dir.path());
        let (svc, stream) = open_cw(&cfg);

        for i in 0..3u64 {
            append(stream, 1_700_000_000_000 + i, format!("h{i}").as_bytes());
        }
        // Let at least one drain happen against the registered (disconnected) callback.
        let start = Instant::now();
        while st.rejected_attempts.load(Ordering::SeqCst) < 1 {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        // Now clear the callback: the bound closure's next drain takes the "no callback" path.
        esl_set_sink_callback(None, std::ptr::null_mut());
        std::thread::sleep(Duration::from_millis(80));

        let s = stats(svc);
        assert_eq!(s.exported_total, 0, "no record should be lost while the callback is gone");
        assert!(s.backlog >= 1, "the batch is held (at-least-once), not dropped");

        esl_stream_free(stream);
        esl_shutdown(svc);
    }
    drop(st);
}

#[test]
fn host_callback_permanent_failure_is_held_then_recovers() {
    run_mode(2);
}

#[test]
fn host_callback_partial_with_zero_failures_is_acked() {
    run_mode(3);
}
