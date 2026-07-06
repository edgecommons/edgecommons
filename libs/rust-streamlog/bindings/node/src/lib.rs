//! napi-rs native addon (`streamlog-node`) — binds the `edgestreamlog` telemetry-streaming core
//! into Node as native classes. Wrapped by the `edgecommons` TS lib's `streaming` module.
//!
//! Errors are thrown as JS `Error`s whose message is `esl:<code>:<message>` (the TS wrapper parses
//! the status code). Core `tracing` logs are forwarded to a JS callback registered via
//! `setLogCallback`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};

use edgestreamlog::{
    CallbackSink, EmbeddedLog, ExportRecord, Record, SendOutcome, ServiceStats, Sink,
    SinkConfig, StreamService as CoreService, StreamingConfig,
};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

fn status_code(e: &edgestreamlog::EdgeStreamError) -> i32 {
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

fn map_err(e: edgestreamlog::EdgeStreamError) -> Error {
    Error::from_reason(format!("esl:{}:{}", status_code(&e), e))
}

fn err(code: i32, message: impl AsRef<str>) -> Error {
    Error::from_reason(format!("esl:{}:{}", code, message.as_ref()))
}

/// A snapshot of one stream's buffer + export progress (mirrors `esl_stats_t`).
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
    ///
    /// A `callback`-sink stream is wired to a host JS sink **iff** a callback was registered for its
    /// name via [`register_sink_callback`] before this call; otherwise it stays buffer-only (parity
    /// with the core's default factory). Native Kinesis/Kafka sinks are unaffected.
    #[napi(factory)]
    pub fn open(config_json: String) -> Result<StreamService> {
        let cfg: StreamingConfig =
            serde_json::from_str(&config_json).map_err(|e| err(1, format!("config: {e}")))?;
        let factory = |name: &str, sink: &SinkConfig| -> edgestreamlog::Result<Option<Box<dyn Sink>>> {
            if let SinkConfig::Callback { .. } = sink {
                if let Some(cb) = sink_callback_for(name) {
                    return Ok(Some(Box::new(cb)));
                }
            }
            // Defer to the core's default sink construction for everything else (native
            // Kinesis/Kafka where those features are enabled, else buffer-only — including a
            // `callback` stream with no registered host sink).
            CoreService::default_sink(name, sink)
        };
        let svc = CoreService::open_with(cfg, &factory).map_err(map_err)?;
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

// ----- host-callback sink bridge: core export thread <-> async JS drain -----
//
// The export engine drives a [`Sink`] *synchronously* on its background thread; the JS sink drains
// via the (async) AWS SDK. The bridge (validated by the §9 spike in docs/CLOUDWATCH_DURABLE_METRICS.md):
//   1. the core `CallbackSink` closure (on the export thread) hands the batch to JS through a
//      `ThreadsafeFunction` (NonBlocking) tagged with a unique batch id, then blocks on a
//      `sync_channel` receiver keyed by that id;
//   2. the async JS sink does its `PutMetricData` work and calls `resolveOutcome(id, code, failedOffsets)`;
//   3. `resolveOutcome` sends the decoded `SendOutcome` over the channel, unblocking the export thread.
// The JS event loop never blocks (the tsfn call is non-blocking); only the native export thread waits.

/// One record handed to the JS sink (a plain napi object — the batch is `Vec<SinkRecord>`).
#[napi(object)]
pub struct SinkRecord {
    /// Log offset (opaque; echo it back in `resolveOutcome`'s failedOffsets to mark it un-acked).
    pub offset: f64,
    /// Partition key (for CloudWatch: the namespace).
    pub partition_key: String,
    pub timestamp_ms: f64,
    /// The serialized record payload (the compact `{namespace,datum}` JSON for CloudWatch).
    pub payload: Buffer,
}

/// Outcome codes returned by JS via `resolveOutcome` (mirrors the core `SendOutcome`).
/// 0 = AllAcked, 1 = Partial (use `failedOffsets`), 2 = Failed (retryable).
const OUTCOME_ALL_ACKED: i32 = 0;
const OUTCOME_PARTIAL: i32 = 1;
const OUTCOME_FAILED: i32 = 2;

type SinkTsfn = ThreadsafeFunction<(f64, Vec<SinkRecord>), ()>;

struct SinkBridge {
    /// stream name -> the JS sink callback (tsfn).
    callbacks: HashMap<String, Arc<SinkTsfn>>,
    /// batch id -> the one-shot sender the blocked export thread is waiting on.
    pending: HashMap<u64, SyncSender<SendOutcome>>,
}

static SINK_BRIDGE: OnceLock<Mutex<SinkBridge>> = OnceLock::new();
static BATCH_SEQ: AtomicU64 = AtomicU64::new(1);

fn bridge() -> &'static Mutex<SinkBridge> {
    SINK_BRIDGE.get_or_init(|| {
        Mutex::new(SinkBridge { callbacks: HashMap::new(), pending: HashMap::new() })
    })
}

/// Build a core [`CallbackSink`] for `stream_name` if a JS callback is registered for it. The
/// returned sink, when sent a batch by the export engine, marshals it to JS and blocks on the
/// per-batch channel until `resolveOutcome` signals.
fn sink_callback_for(stream_name: &str) -> Option<CallbackSink> {
    let tsfn = bridge().lock().unwrap().callbacks.get(stream_name).cloned()?;
    Some(CallbackSink::new(Box::new(move |batch: &[ExportRecord<'_>]| {
        let id = BATCH_SEQ.fetch_add(1, Ordering::Relaxed);
        // Rendezvous channel (capacity 1): resolveOutcome hands back exactly one outcome.
        let (tx, rx): (SyncSender<SendOutcome>, Receiver<SendOutcome>) = sync_channel(1);
        bridge().lock().unwrap().pending.insert(id, tx);

        let records: Vec<SinkRecord> = batch
            .iter()
            .map(|r| SinkRecord {
                offset: r.offset as f64,
                partition_key: String::from_utf8_lossy(r.partition_key).into_owned(),
                timestamp_ms: r.ts_ms as f64,
                payload: Buffer::from(r.payload.to_vec()),
            })
            .collect();

        tsfn.call(Ok((id as f64, records)), ThreadsafeFunctionCallMode::NonBlocking);

        // Block this export thread until JS resolves (or the channel is dropped → treat as a
        // retryable failure so the batch is re-delivered, never silently committed).
        match rx.recv() {
            Ok(outcome) => outcome,
            Err(_) => {
                bridge().lock().unwrap().pending.remove(&id);
                SendOutcome::Failed { retryable: true, error: "sink callback channel closed".into() }
            }
        }
    })))
}

/// Register the JS sink callback for a `callback`-sink stream. Must be called **before**
/// `StreamService.open` so the bridge wires the host sink into that stream's export engine.
/// The callback receives `(batchId, records)` and must eventually call `resolveOutcome(batchId, ...)`.
/// Idempotent per stream name (last registration wins).
#[napi]
pub fn register_sink_callback(stream_name: String, callback: SinkTsfn) {
    bridge().lock().unwrap().callbacks.insert(stream_name, Arc::new(callback));
}

/// Signal the export engine that batch `batch_id` finished. `code`: 0 AllAcked, 1 Partial (the
/// `failed_offsets` were not stored → retried), 2 Failed (retryable; whole batch re-delivered).
/// Unblocks the export thread waiting on this batch. Unknown/duplicate ids are ignored.
#[napi]
pub fn resolve_outcome(batch_id: f64, code: i32, failed_offsets: Option<Vec<f64>>) {
    let id = batch_id as u64;
    let tx = bridge().lock().unwrap().pending.remove(&id);
    let Some(tx) = tx else { return };
    let outcome = match code {
        OUTCOME_PARTIAL => SendOutcome::Partial {
            failed_offsets: failed_offsets
                .unwrap_or_default()
                .into_iter()
                .map(|o| o as u64)
                .collect(),
        },
        OUTCOME_FAILED => SendOutcome::Failed { retryable: true, error: "host sink failed".into() },
        // Treat any other code (including OUTCOME_ALL_ACKED) as a full ack.
        _ => {
            let _ = OUTCOME_ALL_ACKED;
            SendOutcome::AllAcked
        }
    };
    let _ = tx.send(outcome);
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
