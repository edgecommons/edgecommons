//! PyO3 native module (`ggstreamlog_native`) — binds the `ggstreamlog` telemetry-streaming core
//! into Python as real native classes. Wrapped by the friendly `ggcommons.streaming` package.
//!
//! Exposes `StreamService`/`StreamHandle`/`StreamStats` + the `GgStreamError` exception (first arg
//! is the status code), and forwards the core's `tracing` events into Python's `logging`.

use std::sync::{Arc, Once};

use ggstreamlog::{EmbeddedLog, Record, ServiceStats, StreamService as CoreService, StreamingConfig};
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

create_exception!(ggstreamlog_native, GgStreamError, PyException);

fn error_code(e: &ggstreamlog::GgStreamError) -> i32 {
    use ggstreamlog::GgStreamError as E;
    match e {
        E::Config(_) => 1,
        E::Io(_) => 2,
        E::Corrupt(_) => 3,
        E::BufferFull => 4,
        E::UnknownStream(_) => 5,
        E::Sink(_) => 6,
    }
}

fn to_pyerr(e: ggstreamlog::GgStreamError) -> PyErr {
    GgStreamError::new_err((error_code(&e), e.to_string()))
}

fn err(code: i32, message: impl Into<String>) -> PyErr {
    GgStreamError::new_err((code, message.into()))
}

/// A snapshot of one stream's buffer + export progress (mirrors `ggsl_stats_t`).
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
    fn open(config_json: &str) -> PyResult<Self> {
        let cfg: StreamingConfig =
            serde_json::from_str(config_json).map_err(|e| err(1, format!("config: {e}")))?;
        let svc = CoreService::open(cfg).map_err(to_pyerr)?;
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
    fn close(&mut self) {
        self.inner = None;
    }
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
fn ggstreamlog_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<StreamService>()?;
    m.add_class::<StreamHandle>()?;
    m.add_class::<StreamStats>()?;
    m.add("GgStreamError", m.py().get_type::<GgStreamError>())?;
    m.add_function(wrap_pyfunction!(install_log_forwarding, m)?)?;
    install_log_forwarding();
    Ok(())
}
