//! PyO3 native module (`edgestreamlog_native`) — binds the `edgestreamlog` telemetry-streaming core
//! into Python as real native classes. Wrapped by the friendly `edgecommons.streaming` package.
//!
//! Exposes `StreamService`/`StreamHandle`/`StreamStats` + the `EdgeStreamError` exception (first arg
//! is the status code), and forwards the core's `tracing` events into Python's `logging`.

use std::sync::{Arc, Once};

use edgestreamlog::export::{CallbackSink, ExportRecord, SendOutcome};
use edgestreamlog::{
    EmbeddedLog, Record, ServiceStats, Sink, SinkConfig, StreamService as CoreService,
    StreamingConfig,
};
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple};

create_exception!(edgestreamlog_native, EdgeStreamError, PyException);

fn error_code(e: &edgestreamlog::EdgeStreamError) -> i32 {
    use edgestreamlog::EdgeStreamError as E;
    match e {
        E::Config(_) => 1,
        E::Io(_) => 2,
        E::Corrupt(_) => 3,
        E::BufferFull => 4,
        E::UnknownStream(_) => 5,
        E::Sink(_) => 6,
    }
}

fn to_pyerr(e: edgestreamlog::EdgeStreamError) -> PyErr {
    EdgeStreamError::new_err((error_code(&e), e.to_string()))
}

fn err(code: i32, message: impl Into<String>) -> PyErr {
    EdgeStreamError::new_err((code, message.into()))
}

/// A snapshot of one stream's buffer + export progress (mirrors `esl_stats_t`).
#[pyclass(get_all)]
struct StreamStats {
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

/// A producer handle to one telemetry stream.
#[pyclass]
struct StreamHandle {
    log: Arc<EmbeddedLog>,
}

#[pymethods]
impl StreamHandle {
    /// Append one record; returns once durable per the stream's fsync policy.
    fn append(&self, partition_key: &str, timestamp_ms: u64, payload: &[u8]) -> PyResult<()> {
        let rec = Record::new(partition_key, timestamp_ms, payload.to_vec());
        self.log.append(&rec).map_err(to_pyerr)
    }

    /// Force this stream's buffer durably to disk (does not wait for export).
    fn flush(&self) -> PyResult<()> {
        self.log.flush().map_err(to_pyerr)
    }
}

/// Owns the native streaming service.
#[pyclass]
struct StreamService {
    inner: Option<CoreService>,
}

#[pymethods]
impl StreamService {
    /// Open every stream in `config_json` (the `streaming` section; templates pre-resolved).
    #[staticmethod]
    fn open(py: Python<'_>, config_json: &str) -> PyResult<Self> {
        let cfg: StreamingConfig =
            serde_json::from_str(config_json).map_err(|e| err(1, format!("config: {e}")))?;
        // Release the GIL while opening: building the Kinesis sink loads the AWS config and the
        // export engine thread starts, both of which emit tracing events. Those are forwarded to
        // Python logging by PyLogLayer via `Python::attach`, which needs the GIL — holding it here
        // would deadlock the moment a background thread logs during open.
        let svc = py.detach(move || CoreService::open(cfg)).map_err(to_pyerr)?;
        Ok(Self { inner: Some(svc) })
    }

    /// Open every stream in `config_json`, binding a Python callable as the export sink for every
    /// stream whose sink is `{"type":"callback"}` (the durable CloudWatch metrics drain /
    /// bring-your-own-sink). All other sink kinds (kinesis/kafka) are built natively as in
    /// [`open`](Self::open).
    ///
    /// The export engine invokes `callback(records)` on its background thread (one call per batch);
    /// this binding reacquires the GIL inside that call via PyO3, so the engine thread blocks until
    /// the Python callback returns. `records` is a list of
    /// `(offset:int, partition_key:bytes, timestamp_ms:int, payload:bytes)` tuples.
    ///
    /// The callback's return value is mapped to the core `SendOutcome`:
    ///   * `None` (or any falsy non-list)  -> `AllAcked` (whole batch committed)
    ///   * a list/tuple of int offsets     -> `Partial` (those offsets failed, retried; rest acked)
    ///   * the tuple `("failed", error_str)` (or a bare non-empty `str`) -> `Failed` (retry whole batch)
    /// A callback that raises is treated as `Failed{retryable:true}` (the batch is retried; no commit).
    #[staticmethod]
    fn open_with_callback(py: Python<'_>, config_json: &str, callback: Py<PyAny>) -> PyResult<Self> {
        let cfg: StreamingConfig =
            serde_json::from_str(config_json).map_err(|e| err(1, format!("config: {e}")))?;
        // The Python callable is shared by every callback stream's sink. `Py<PyAny>` is Send+Sync;
        // wrapping in Arc lets the `Fn` factory hand each CallbackSink its own cheap clone without
        // needing a GIL token (the factory runs under `py.detach`, GIL released).
        let cb = Arc::new(callback);
        let factory =
            move |_name: &str, sc: &SinkConfig| -> edgestreamlog::Result<Option<Box<dyn Sink>>> {
                match sc {
                    SinkConfig::Callback { .. } => {
                        let cb = Arc::clone(&cb);
                        let sink =
                            CallbackSink::new(Box::new(move |batch: &[ExportRecord<'_>]| {
                                invoke_py_callback(&cb, batch)
                            }));
                        Ok(Some(Box::new(sink)))
                    }
                    _ => edgestreamlog::service::default_sink_factory_pub_py(sc),
                }
            };
        let svc = py
            .detach(move || CoreService::open_with(cfg, &factory))
            .map_err(to_pyerr)?;
        Ok(Self { inner: Some(svc) })
    }

    /// A handle to the named stream (raises ERR_UNKNOWN_STREAM if not configured).
    fn stream(&self, name: &str) -> PyResult<StreamHandle> {
        let svc = self.inner.as_ref().ok_or_else(|| err(5, "service is closed"))?;
        svc.stream(name)
            .map(|log| StreamHandle { log })
            .ok_or_else(|| err(5, format!("unknown stream: {name}")))
    }

    /// A stats snapshot for the named stream (raises ERR_UNKNOWN_STREAM if not configured).
    fn stats(&self, name: &str) -> PyResult<StreamStats> {
        let svc = self.inner.as_ref().ok_or_else(|| err(5, "service is closed"))?;
        let s: ServiceStats = svc.stats(name).ok_or_else(|| err(5, format!("unknown stream: {name}")))?;
        Ok(StreamStats {
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
        })
    }

    /// Flush every buffer, stop the export engines, and free the service. Idempotent.
    fn close(&mut self, py: Python<'_>) {
        // Take the Python-visible ownership first, so the service is already closed even if the
        // native drop panics. Dropping the core joins its export workers; a callback worker can be
        // waiting to acquire the GIL, so release it before waiting for that join.
        if let Some(inner) = self.inner.take() {
            py.detach(move || drop(inner));
        }
    }
}

// ----- host-callback sink: invoke the Python callable on the export thread -----

/// Invoke the host Python callback for one export batch, reacquiring the GIL. Runs on the core
/// export engine thread (which blocks here until Python returns), so it must never panic across the
/// FFI boundary: a Python exception or an unexpected return type is mapped to a retryable `Failed`
/// (the batch is re-delivered, never committed/lost). Return-value mapping is documented on
/// [`StreamService::open_with_callback`].
fn invoke_py_callback(cb: &Py<PyAny>, batch: &[ExportRecord<'_>]) -> SendOutcome {
    Python::attach(|py| {
        // Build the records list: [(offset, partition_key: bytes, ts_ms, payload: bytes), ...].
        let items: Vec<Bound<'_, PyAny>> = batch
            .iter()
            .map(|r| {
                let tup = PyTuple::new(
                    py,
                    [
                        r.offset.into_pyobject(py).unwrap().into_any(),
                        PyBytes::new(py, r.partition_key).into_any(),
                        r.ts_ms.into_pyobject(py).unwrap().into_any(),
                        PyBytes::new(py, r.payload).into_any(),
                    ],
                )
                .unwrap();
                tup.into_any()
            })
            .collect();
        let records = match PyList::new(py, items) {
            Ok(l) => l,
            Err(e) => {
                return SendOutcome::Failed {
                    retryable: true,
                    error: format!("failed to marshal export batch to Python: {e}"),
                }
            }
        };
        match cb.call1(py, (records,)) {
            Ok(ret) => map_callback_result(py, ret.bind(py)),
            Err(e) => SendOutcome::Failed {
                retryable: true,
                error: format!("host metrics callback raised: {e}"),
            },
        }
    })
}

/// Map a Python callback return value to a [`SendOutcome`] (see `open_with_callback` docs).
fn map_callback_result(py: Python<'_>, ret: &Bound<'_, PyAny>) -> SendOutcome {
    // `("failed", error_str)` (or a bare str) -> retry the whole batch.
    if let Ok(s) = ret.extract::<String>() {
        return SendOutcome::Failed { retryable: true, error: s };
    }
    if let Ok(tup) = ret.downcast::<PyTuple>() {
        if tup.len() == 2 {
            if let Ok(tag) = tup.get_item(0).and_then(|t| t.extract::<String>()) {
                if tag == "failed" {
                    let msg = tup
                        .get_item(1)
                        .and_then(|m| m.extract::<String>())
                        .unwrap_or_else(|_| "host sink reported failure".into());
                    return SendOutcome::Failed { retryable: true, error: msg };
                }
            }
        }
    }
    // A list/tuple of int offsets -> Partial (those failed; the rest acked).
    if let Ok(offsets) = ret.extract::<Vec<u64>>() {
        if offsets.is_empty() {
            return SendOutcome::AllAcked;
        }
        return SendOutcome::Partial { failed_offsets: offsets };
    }
    // None / anything falsy / unrecognized -> AllAcked (whole batch committed).
    let _ = py;
    SendOutcome::AllAcked
}

// ----- log forwarding: core tracing -> Python logging -----

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

struct PyLogLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PyLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let meta = event.metadata();
        // Python logging levels: ERROR 40, WARNING 30, INFO 20, DEBUG 10.
        let level = match *meta.level() {
            tracing::Level::ERROR => 40,
            tracing::Level::WARN => 30,
            tracing::Level::INFO => 20,
            tracing::Level::DEBUG => 10,
            tracing::Level::TRACE => 5,
        };
        let target = meta.target().to_string();
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        let msg = if v.fields.is_empty() { v.message } else { format!("{}{}", v.message, v.fields) };

        Python::attach(|py| {
            if let Ok(logging) = py.import("logging") {
                if let Ok(logger) = logging.call_method1("getLogger", (target,)) {
                    let _ = logger.call_method1("log", (level, msg));
                }
            }
        });
    }
}

static LOG_INIT: Once = Once::new();

/// Install the tracing -> Python logging forwarder (idempotent; auto-called on import).
#[pyfunction]
fn install_log_forwarding() {
    LOG_INIT.call_once(|| {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let _ = tracing_subscriber::registry().with(PyLogLayer).try_init();
    });
}

#[pymodule]
fn edgestreamlog_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<StreamService>()?;
    m.add_class::<StreamHandle>()?;
    m.add_class::<StreamStats>()?;
    m.add("EdgeStreamError", m.py().get_type::<EdgeStreamError>())?;
    m.add_function(wrap_pyfunction!(install_log_forwarding, m)?)?;
    install_log_forwarding();
    Ok(())
}
