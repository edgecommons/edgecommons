//! napi-rs native addon (`ggstreamlog-node`) — binds the `ggstreamlog` telemetry-streaming core
//! into Node as native classes. Wrapped by the `ggcommons` TS lib's `streaming` module.
//!
//! Errors are thrown as JS `Error`s whose message is `ggsl:<code>:<message>` (the TS wrapper parses
//! the status code). Core `tracing` logs are forwarded to a JS callback registered via
//! `setLogCallback`.

use std::sync::{Arc, OnceLock};

use ggstreamlog::{EmbeddedLog, Record, ServiceStats, StreamService as CoreService, StreamingConfig};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

fn status_code(e: &ggstreamlog::GgStreamError) -> i32 {
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

fn map_err(e: ggstreamlog::GgStreamError) -> Error {
    Error::from_reason(format!("ggsl:{}:{}", status_code(&e), e))
}

fn err(code: i32, message: impl AsRef<str>) -> Error {
    Error::from_reason(format!("ggsl:{}:{}", code, message.as_ref()))
}

/// A snapshot of one stream's buffer + export progress (mirrors `ggsl_stats_t`).
#[napi(object)]
pub struct StreamStats {
    pub appended_total: f64,
    pub exported_total: f64,
    pub dropped_total: f64,
    pub retries_total: f64,
    pub failed_total: f64,
    pub backlog: f64,
    pub disk_bytes: f64,
    pub acked_offset: f64,
    pub next_offset: f64,
    pub oldest_unacked_age_ms: f64,
}

impl From<ServiceStats> for StreamStats {
    fn from(s: ServiceStats) -> Self {
        StreamStats {
            appended_total: s.appended_total as f64,
            exported_total: s.exported_total as f64,
            dropped_total: s.dropped_total as f64,
            retries_total: s.retries_total as f64,
            failed_total: s.failed_total as f64,
            backlog: s.backlog as f64,
            disk_bytes: s.disk_bytes as f64,
            acked_offset: s.acked_offset as f64,
            next_offset: s.next_offset as f64,
            oldest_unacked_age_ms: s.oldest_unacked_age_ms as f64,
        }
    }
}

/// A producer handle to one telemetry stream.
#[napi]
pub struct StreamHandle {
    log: Arc<EmbeddedLog>,
}

#[napi]
impl StreamHandle {
    /// Append one record; resolves once durable per the stream's fsync policy.
    #[napi]
    pub fn append(&self, partition_key: String, timestamp_ms: f64, payload: Buffer) -> Result<()> {
        let rec = Record::new(partition_key, timestamp_ms as u64, payload.to_vec());
        self.log.append(&rec).map_err(map_err)
    }

    /// Force this stream's buffer durably to disk (does not wait for export).
    #[napi]
    pub fn flush(&self) -> Result<()> {
        self.log.flush().map_err(map_err)
    }
}

/// Owns the native streaming service.
#[napi]
pub struct StreamService {
    inner: Option<CoreService>,
}

#[napi]
impl StreamService {
    /// Open every stream in `configJson` (the `streaming` section; templates pre-resolved).
    #[napi(factory)]
    pub fn open(config_json: String) -> Result<StreamService> {
        let cfg: StreamingConfig =
            serde_json::from_str(&config_json).map_err(|e| err(1, format!("config: {e}")))?;
        let svc = CoreService::open(cfg).map_err(map_err)?;
        Ok(StreamService { inner: Some(svc) })
    }

    /// A handle to the named stream (throws ERR_UNKNOWN_STREAM if not configured).
    #[napi]
    pub fn stream(&self, name: String) -> Result<StreamHandle> {
        let svc = self.inner.as_ref().ok_or_else(|| err(5, "service is closed"))?;
        svc.stream(&name)
            .map(|log| StreamHandle { log })
            .ok_or_else(|| err(5, format!("unknown stream: {name}")))
    }

    /// A stats snapshot for the named stream (throws ERR_UNKNOWN_STREAM if not configured).
    #[napi]
    pub fn stats(&self, name: String) -> Result<StreamStats> {
        let svc = self.inner.as_ref().ok_or_else(|| err(5, "service is closed"))?;
        svc.stats(&name)
            .map(StreamStats::from)
            .ok_or_else(|| err(5, format!("unknown stream: {name}")))
    }

    /// Flush every buffer, stop the export engines, and free the service. Idempotent.
    #[napi]
    pub fn close(&mut self) {
        self.inner = None;
    }
}

// ----- log forwarding: core tracing -> a JS callback -----

static LOG_TSFN: OnceLock<ThreadsafeFunction<LogEvent, ()>> = OnceLock::new();
static LOG_INIT: std::sync::Once = std::sync::Once::new();

/// One forwarded log event (level: 1=ERROR..5=TRACE).
#[napi(object)]
pub struct LogEvent {
    pub level: i32,
    pub target: String,
    pub message: String,
}

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

struct NodeLogLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for NodeLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(tsfn) = LOG_TSFN.get() else { return };
        let meta = event.metadata();
        let level = match *meta.level() {
            tracing::Level::ERROR => 1,
            tracing::Level::WARN => 2,
            tracing::Level::INFO => 3,
            tracing::Level::DEBUG => 4,
            tracing::Level::TRACE => 5,
        };
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        let message = if v.fields.is_empty() { v.message } else { format!("{}{}", v.message, v.fields) };
        let ev = LogEvent { level, target: meta.target().to_string(), message };
        tsfn.call(Ok(ev), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

/// Register a callback that receives core log events `{ level, target, message }`. Idempotent;
/// installs the forwarding subscriber on first call.
#[napi]
pub fn set_log_callback(callback: ThreadsafeFunction<LogEvent, ()>) {
    let _ = LOG_TSFN.set(callback);
    LOG_INIT.call_once(|| {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let _ = tracing_subscriber::registry().with(NodeLogLayer).try_init();
    });
}
