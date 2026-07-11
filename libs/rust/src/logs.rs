//! # Log Bus Publishing
//!
//! Publishes application log records to the library-owned UNS `log` class without
//! exposing the reserved publish seam to component code.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::{ArcSwap, ArcSwapOption};
use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::config::ConfigurationChangeListener;
use crate::config::model::{Config, LoggingPublishDestination, LoggingPublishQueueOnFull};
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::{Message, MessageBuilder};
use crate::messaging::{Qos, ReservedMessaging};

/// Public log severity used by [`LogRecord`].
pub use crate::config::model::LoggingPublishLevel as LogLevel;

const LOG_MESSAGE_NAME: &str = "log";
const LOG_MESSAGE_VERSION: &str = "1.0";
const LOG_SCHEMA: &str = "edgecommons.log.v1";

static CAPTURE_SERVICE: OnceLock<ArcSwapOption<DefaultLogService>> = OnceLock::new();

tokio::task_local! {
    static LOG_PUBLISHING: ();
}

fn capture_service() -> &'static ArcSwapOption<DefaultLogService> {
    CAPTURE_SERVICE.get_or_init(|| ArcSwapOption::from(None))
}

fn is_log_publishing_task() -> bool {
    LOG_PUBLISHING.try_with(|_| ()).is_ok()
}

/// A log record that can be published on the EdgeCommons log bus.
#[derive(Debug, Clone)]
pub struct LogRecord {
    timestamp: Option<String>,
    level: LogLevel,
    logger: String,
    message: String,
    thread: Option<String>,
    fields: BTreeMap<String, Value>,
    error: Option<String>,
    sequence: Option<u64>,
    dropped: Option<u64>,
}

impl LogRecord {
    /// Start a log record builder.
    pub fn builder(
        level: LogLevel,
        logger: impl Into<String>,
        message: impl Into<String>,
    ) -> LogRecordBuilder {
        LogRecordBuilder {
            record: LogRecord {
                timestamp: None,
                level,
                logger: logger.into(),
                message: message.into(),
                thread: None,
                fields: BTreeMap::new(),
                error: None,
                sequence: None,
                dropped: None,
            },
        }
    }
}

/// Fluent builder for [`LogRecord`].
#[must_use]
pub struct LogRecordBuilder {
    record: LogRecord,
}

impl LogRecordBuilder {
    /// Set the RFC3339 record timestamp.
    pub fn timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.record.timestamp = Some(timestamp.into());
        self
    }

    /// Set the source thread name.
    pub fn thread(mut self, thread: impl Into<String>) -> Self {
        self.record.thread = Some(thread.into());
        self
    }

    /// Add a structured field.
    pub fn field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.record.fields.insert(key.into(), value);
        self
    }

    /// Add multiple structured fields.
    pub fn fields(mut self, fields: BTreeMap<String, Value>) -> Self {
        self.record.fields.extend(fields);
        self
    }

    /// Set the error text associated with this record.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.record.error = Some(error.into());
        self
    }

    /// Finish the record.
    pub fn build(self) -> LogRecord {
        self.record
    }
}

/// Snapshot of log publisher counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogStats {
    pub published: u64,
    pub failed: u64,
    pub queued: u64,
    pub dropped: u64,
    pub redacted: u64,
    pub truncated: u64,
}

/// Public log publishing service.
#[async_trait]
pub trait LogService: Send + Sync {
    /// Publish a record immediately through the reserved log class publisher.
    async fn publish(&self, record: LogRecord) -> Result<()>;

    /// Queue a record without blocking the caller.
    ///
    /// Returns `Ok(false)` when the record itself was dropped.
    fn try_publish(&self, record: LogRecord) -> Result<bool>;

    /// Return current publisher counters.
    fn stats(&self) -> LogStats;
}

#[derive(Debug, Clone)]
struct LogPublishSettings {
    enabled: bool,
    destination: LoggingPublishDestination,
    min_level: LogLevel,
    capture_native: bool,
    max_record_bytes: usize,
    max_records: usize,
    on_full: LoggingPublishQueueOnFull,
    redaction_enabled: bool,
    replacement: String,
    patterns: Vec<Regex>,
}

impl LogPublishSettings {
    fn from_config(config: &Config) -> Result<Self> {
        let publish = &config.parsed.logging.publish;
        let patterns = publish
            .redaction
            .extra_patterns
            .iter()
            .map(|pattern| {
                Regex::new(pattern).map_err(|e| {
                    EdgeCommonsError::Config(format!(
                        "logging.publish.redaction.extraPatterns contains invalid regex '{pattern}': {e}"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            enabled: publish.enabled,
            destination: publish.destination,
            min_level: publish.min_level,
            capture_native: publish.capture_native,
            max_record_bytes: usize::try_from(publish.max_record_bytes)
                .unwrap_or(usize::MAX)
                .max(1),
            max_records: usize::try_from(publish.queue.max_records).unwrap_or(usize::MAX),
            on_full: publish.queue.on_full,
            redaction_enabled: publish.redaction.enabled,
            replacement: publish.redaction.replacement.clone(),
            patterns,
        })
    }

    fn captures(&self, level: LogLevel) -> bool {
        self.enabled && self.capture_native && level >= self.min_level
    }
}

#[derive(Default)]
struct Counters {
    published: AtomicU64,
    failed: AtomicU64,
    queued: AtomicU64,
    dropped: AtomicU64,
    redacted: AtomicU64,
    truncated: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> LogStats {
        LogStats {
            published: self.published.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            redacted: self.redacted.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
        }
    }
}

struct Queue {
    records: Mutex<VecDeque<LogRecord>>,
    notify: Notify,
}

impl Queue {
    fn new() -> Self {
        Self {
            records: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        }
    }
}

/// Default log bus publisher.
pub struct DefaultLogService {
    config: Arc<ArcSwap<Config>>,
    settings: Arc<ArcSwap<LogPublishSettings>>,
    reserved: Option<Arc<dyn ReservedMessaging>>,
    queue: Queue,
    sequence: AtomicU64,
    dropped_since_last_record: AtomicU64,
    counters: Counters,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl DefaultLogService {
    /// Create and start the log publisher.
    pub(crate) fn start(
        config: Arc<ArcSwap<Config>>,
        reserved: Option<Arc<dyn ReservedMessaging>>,
    ) -> Result<Arc<Self>> {
        let service = Self::start_unregistered(config, reserved)?;
        capture_service().store(Some(service.clone()));
        Ok(service)
    }

    fn start_unregistered(
        config: Arc<ArcSwap<Config>>,
        reserved: Option<Arc<dyn ReservedMessaging>>,
    ) -> Result<Arc<Self>> {
        let settings = LogPublishSettings::from_config(&config.load_full())?;
        let service = Arc::new(Self {
            config,
            settings: Arc::new(ArcSwap::from_pointee(settings)),
            reserved,
            queue: Queue::new(),
            sequence: AtomicU64::new(1),
            dropped_since_last_record: AtomicU64::new(0),
            counters: Counters::default(),
            worker: Mutex::new(None),
        });
        let worker_service = service.clone();
        let handle = tokio::spawn(async move {
            worker_service.run_queue().await;
        });
        if let Ok(mut slot) = service.worker.lock() {
            *slot = Some(handle);
        }
        Ok(service)
    }

    #[cfg(test)]
    fn start_for_isolated_capture_test(
        config: Arc<ArcSwap<Config>>,
        reserved: Option<Arc<dyn ReservedMessaging>>,
    ) -> Result<Arc<Self>> {
        Self::start_unregistered(config, reserved)
    }

    fn capture(&self, record: LogRecord) {
        let _ = self.enqueue(record);
    }

    async fn run_queue(self: Arc<Self>) {
        loop {
            self.queue.notify.notified().await;
            loop {
                let next = self
                    .queue
                    .records
                    .lock()
                    .ok()
                    .and_then(|mut q| q.pop_front());
                let Some(record) = next else { break };
                let _ = self.publish(record).await;
            }
        }
    }

    fn enqueue(&self, record: LogRecord) -> Result<bool> {
        self.ensure_reserved()?;
        let settings = self.settings.load_full();
        if settings.max_records == 0 {
            self.note_drop();
            return Ok(false);
        }
        let mut queue = self
            .queue
            .records
            .lock()
            .map_err(|_| EdgeCommonsError::Messaging("log queue poisoned".to_string()))?;
        if queue.len() >= settings.max_records {
            match settings.on_full {
                LoggingPublishQueueOnFull::DropOldest => {
                    queue.pop_front();
                    self.note_drop();
                }
            }
        }
        queue.push_back(record);
        self.counters.queued.fetch_add(1, Ordering::Relaxed);
        drop(queue);
        self.queue.notify.notify_one();
        Ok(true)
    }

    fn note_drop(&self) {
        self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        self.dropped_since_last_record
            .fetch_add(1, Ordering::Relaxed);
    }

    fn ensure_reserved(&self) -> Result<&Arc<dyn ReservedMessaging>> {
        self.reserved.as_ref().ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "log publishing requires a wired reserved messaging transport".to_string(),
            )
        })
    }

    async fn publish_one(&self, mut record: LogRecord) -> Result<()> {
        let reserved = self.ensure_reserved()?;
        if !reserved.connected() {
            self.counters.failed.fetch_add(1, Ordering::Relaxed);
            return Err(EdgeCommonsError::Messaging(
                "log publishing skipped because messaging is disconnected".to_string(),
            ));
        }
        let config = self.config.load_full();
        let settings = self.settings.load_full();
        if record.sequence.is_none() {
            record.sequence = Some(self.sequence.fetch_add(1, Ordering::Relaxed));
        }
        let dropped = self.dropped_since_last_record.swap(0, Ordering::Relaxed);
        if dropped > 0 && record.dropped.is_none() {
            record.dropped = Some(dropped);
        }
        let (message, topic) = self.build_message(&config, &settings, record)?;
        let outcome = LOG_PUBLISHING.scope((), async {
            match settings.destination {
                LoggingPublishDestination::Local => reserved.publish_reserved(&topic, &message).await,
                LoggingPublishDestination::Northbound => {
                    reserved
                        .publish_reserved_northbound(&topic, &message, Qos::AtLeastOnce)
                        .await
                }
            }
        }).await;
        match outcome {
            Ok(()) => {
                self.counters.published.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    fn build_message(
        &self,
        config: &Config,
        settings: &LogPublishSettings,
        mut record: LogRecord,
    ) -> Result<(Message, String)> {
        let timestamp = record.timestamp.take().unwrap_or_else(now_rfc3339);
        let sequence = record.sequence.unwrap_or(0);
        let redacted = if settings.redaction_enabled {
            redact_record(&mut record, settings)
        } else {
            0
        };
        if redacted > 0 {
            self.counters
                .redacted
                .fetch_add(redacted, Ordering::Relaxed);
        }
        let mut body = body_value(&record, &timestamp, sequence, false);
        if serde_json::to_vec(&body)?.len() > settings.max_record_bytes {
            truncate_body(&mut body, settings.max_record_bytes);
            self.counters.truncated.fetch_add(1, Ordering::Relaxed);
        }
        let topic = format!(
            "ecv1/{}/{}/{}/log/{}",
            config.identity().device(),
            config.identity().component(),
            crate::messaging::MessageIdentity::DEFAULT_INSTANCE,
            record.level.lowercase()
        );
        let message = MessageBuilder::new(LOG_MESSAGE_NAME, LOG_MESSAGE_VERSION)
            .from_config(config)
            .timestamp(timestamp)
            .payload(body)
            .build();
        Ok((message, topic))
    }
}

impl Drop for DefaultLogService {
    fn drop(&mut self) {
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                handle.abort();
            }
        }
    }
}

#[async_trait]
impl LogService for DefaultLogService {
    async fn publish(&self, record: LogRecord) -> Result<()> {
        self.publish_one(record).await
    }

    fn try_publish(&self, record: LogRecord) -> Result<bool> {
        self.enqueue(record)
    }

    fn stats(&self) -> LogStats {
        self.counters.snapshot()
    }
}

#[async_trait]
impl ConfigurationChangeListener for DefaultLogService {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        match LogPublishSettings::from_config(&config) {
            Ok(settings) => {
                self.config.store(config);
                self.settings.store(Arc::new(settings));
                true
            }
            Err(_) => false,
        }
    }
}

/// Tracing layer that captures native Rust tracing events into the log bus.
#[derive(Default)]
pub struct LogCaptureLayer {
    #[cfg(test)]
    service: Option<Arc<DefaultLogService>>,
}

impl std::fmt::Debug for LogCaptureLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogCaptureLayer").finish()
    }
}

impl LogCaptureLayer {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn for_isolated_capture_test(service: Arc<DefaultLogService>) -> Self {
        Self {
            service: Some(service),
        }
    }
}

impl<S> tracing_subscriber::Layer<S> for LogCaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if is_log_publishing_task() {
            return;
        }
        #[cfg(test)]
        let service = self
            .service
            .clone()
            .or_else(|| capture_service().load_full());
        #[cfg(not(test))]
        let Some(service) = capture_service().load_full() else {
            return;
        };
        #[cfg(test)]
        let Some(service) = service else {
            return;
        };
        let level = match *event.metadata().level() {
            Level::TRACE => LogLevel::Trace,
            Level::DEBUG => LogLevel::Debug,
            Level::INFO => LogLevel::Info,
            Level::WARN => LogLevel::Warn,
            Level::ERROR => LogLevel::Error,
        };
        if !service.settings.load_full().captures(level) {
            return;
        }
        let mut visitor = CaptureVisitor::default();
        event.record(&mut visitor);
        let mut builder = LogRecord::builder(
            level,
            event.metadata().target(),
            visitor.message.unwrap_or_default(),
        )
        .timestamp(now_rfc3339());
        if let Some(name) = std::thread::current().name() {
            builder = builder.thread(name);
        }
        if !visitor.fields.is_empty() {
            builder = builder.fields(visitor.fields);
        }
        if let Some(error) = visitor.error {
            builder = builder.error(error);
        }
        service.capture(builder.build());
    }
}

#[derive(Default)]
struct CaptureVisitor {
    message: Option<String>,
    error: Option<String>,
    fields: BTreeMap<String, Value>,
}

impl Visit for CaptureVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field.name(), Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field.name(), Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field.name(), Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field.name(), Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_value(field.name(), Value::from(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_value(field.name(), Value::from(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_value(field.name(), Value::from(format!("{value:?}")));
    }
}

impl CaptureVisitor {
    fn record_value(&mut self, name: &str, value: Value) {
        match name {
            "message" => {
                self.message = value
                    .as_str()
                    .map(ToString::to_string)
                    .or(Some(value.to_string()))
            }
            "error" | "exception" => {
                self.error = value
                    .as_str()
                    .map(ToString::to_string)
                    .or(Some(value.to_string()));
            }
            other => {
                self.fields.insert(other.to_string(), value);
            }
        }
    }
}

fn body_value(record: &LogRecord, timestamp: &str, sequence: u64, truncated: bool) -> Value {
    let mut body = Map::new();
    body.insert("schema".to_string(), Value::from(LOG_SCHEMA));
    body.insert("timestamp".to_string(), Value::from(timestamp));
    body.insert("level".to_string(), Value::from(record.level.uppercase()));
    body.insert("logger".to_string(), Value::from(record.logger.clone()));
    body.insert("message".to_string(), Value::from(record.message.clone()));
    body.insert("sequence".to_string(), Value::from(sequence));
    if let Some(thread) = &record.thread {
        body.insert("thread".to_string(), Value::from(thread.clone()));
    }
    if !record.fields.is_empty() {
        body.insert(
            "fields".to_string(),
            Value::Object(fields_map(&record.fields)),
        );
    }
    if let Some(error) = &record.error {
        body.insert("error".to_string(), Value::from(error.clone()));
    }
    if truncated {
        body.insert("truncated".to_string(), Value::Bool(true));
    }
    if let Some(dropped) = record.dropped {
        body.insert("dropped".to_string(), Value::from(dropped));
    }
    Value::Object(body)
}

fn fields_map(fields: &BTreeMap<String, Value>) -> Map<String, Value> {
    fields
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn redact_record(record: &mut LogRecord, settings: &LogPublishSettings) -> u64 {
    let mut count = 0;
    count += redact_string(&mut record.message, settings);
    if let Some(error) = &mut record.error {
        count += redact_string(error, settings);
    }
    for value in record.fields.values_mut() {
        count += redact_value(value, settings);
    }
    count
}

fn redact_value(value: &mut Value, settings: &LogPublishSettings) -> u64 {
    match value {
        Value::String(text) => redact_string(text, settings),
        Value::Array(values) => values
            .iter_mut()
            .map(|value| redact_value(value, settings))
            .sum(),
        Value::Object(map) => map
            .values_mut()
            .map(|value| redact_value(value, settings))
            .sum(),
        _ => 0,
    }
}

fn redact_string(text: &mut String, settings: &LogPublishSettings) -> u64 {
    let mut count = 0;
    for pattern in &settings.patterns {
        if pattern.is_match(text) {
            let replaced = pattern
                .replace_all(text, settings.replacement.as_str())
                .to_string();
            *text = replaced;
            count += 1;
        }
    }
    count
}

fn truncate_body(body: &mut Value, max_bytes: usize) {
    set_truncated(body);
    while serde_json::to_vec(body).map_or(0, |bytes| bytes.len()) > max_bytes {
        if !truncate_string_field(body, "message")
            && !drop_field(body, "fields")
            && !truncate_string_field(body, "error")
        {
            break;
        }
    }
}

fn set_truncated(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("truncated".to_string(), Value::Bool(true));
    }
}

fn truncate_string_field(body: &mut Value, key: &str) -> bool {
    let Some(text) = body
        .as_object()
        .and_then(|obj| obj.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return false;
    };
    if text.is_empty() {
        return false;
    }
    let mut next_len = text.len() / 2;
    while next_len > 0 && !text.is_char_boundary(next_len) {
        next_len -= 1;
    }
    let mut next = text[..next_len].to_string();
    next.push_str("...");
    if let Some(obj) = body.as_object_mut() {
        obj.insert(key.to_string(), Value::from(next));
    }
    true
}

fn drop_field(body: &mut Value, key: &str) -> bool {
    body.as_object_mut()
        .and_then(|obj| obj.remove(key))
        .is_some()
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingMessaging;
    use async_trait::async_trait;
    use serde_json::json;
    use std::time::Duration;
    use tracing_subscriber::prelude::*;

    fn config(raw: Value) -> Arc<ArcSwap<Config>> {
        Arc::new(ArcSwap::from_pointee(
            Config::from_value("com.example.MyComp", "gw-01", raw).unwrap(),
        ))
    }

    fn connected_messaging() -> Arc<RecordingMessaging> {
        let messaging = RecordingMessaging::new();
        messaging.set_connected(true);
        messaging
    }

    #[tokio::test]
    async fn explicit_publish_uses_reserved_log_topic_and_body_schema() {
        let messaging = connected_messaging();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": { "enabled": true } } })),
            Some(messaging.clone() as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        service
            .publish(
                LogRecord::builder(LogLevel::Info, "app", "started")
                    .timestamp("2026-07-09T00:00:00Z")
                    .thread("main")
                    .field("pid", json!(7))
                    .build(),
            )
            .await
            .unwrap();

        let published = messaging.reserved_local();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, "ecv1/gw-01/MyComp/main/log/info");
        let body = &published[0].1.body;
        assert_eq!(published[0].1.header.name, "log");
        assert_eq!(published[0].1.header.version, "1.0");
        assert_eq!(published[0].1.header.timestamp, "2026-07-09T00:00:00Z");
        assert_eq!(body["schema"], "edgecommons.log.v1");
        assert_eq!(body["timestamp"], "2026-07-09T00:00:00Z");
        assert_eq!(body["level"], "INFO");
        assert_eq!(body["logger"], "app");
        assert_eq!(body["message"], "started");
        assert_eq!(body["sequence"], 1);
        assert_eq!(body["thread"], "main");
        assert_eq!(body["fields"]["pid"], 7);
    }

    #[tokio::test]
    async fn northbound_destination_uses_reserved_northbound_path() {
        let messaging = connected_messaging();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": { "destination": "northbound" } } })),
            Some(messaging.clone() as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        service
            .publish(LogRecord::builder(LogLevel::Error, "app", "failed").build())
            .await
            .unwrap();
        assert_eq!(
            messaging.reserved_iot()[0].0,
            "ecv1/gw-01/MyComp/main/log/error"
        );
        assert!(messaging.reserved_local().is_empty());
    }

    #[tokio::test]
    async fn disconnected_transport_counts_failure_without_reserved_publish() {
        let messaging = RecordingMessaging::new();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": { "enabled": true } } })),
            Some(messaging.clone() as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        let err = service
            .publish(LogRecord::builder(LogLevel::Error, "app", "offline").build())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("disconnected"));
        assert!(messaging.reserved_local().is_empty());
        assert_eq!(service.stats().failed, 1);
    }

    #[tokio::test]
    async fn redaction_applies_extra_patterns() {
        let messaging = connected_messaging();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": { "redaction": {
                "extraPatterns": ["token=[A-Za-z0-9]+"]
            } } } })),
            Some(messaging.clone() as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        service
            .publish(
                LogRecord::builder(LogLevel::Warn, "app", "token=abc123")
                    .field("nested", json!({ "value": "token=xyz" }))
                    .build(),
            )
            .await
            .unwrap();
        let body = &messaging.reserved_local()[0].1.body;
        assert_eq!(body["message"], "***");
        assert_eq!(body["fields"]["nested"]["value"], "***");
        assert_eq!(service.stats().redacted, 2);
    }

    #[tokio::test]
    async fn truncation_marks_record_and_preserves_required_fields() {
        let messaging = connected_messaging();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": { "maxRecordBytes": 180 } } })),
            Some(messaging.clone() as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        service
            .publish(LogRecord::builder(LogLevel::Info, "app", "x".repeat(1000)).build())
            .await
            .unwrap();
        let body = &messaging.reserved_local()[0].1.body;
        assert_eq!(body["schema"], "edgecommons.log.v1");
        assert_eq!(body["truncated"], true);
        assert_eq!(service.stats().truncated, 1);
    }

    #[tokio::test]
    async fn queue_drop_oldest_is_nonblocking_and_reports_dropped() {
        let messaging = connected_messaging();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": { "queue": { "maxRecords": 1 } } } })),
            Some(messaging.clone() as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        service
            .queue
            .records
            .lock()
            .unwrap()
            .push_back(LogRecord::builder(LogLevel::Info, "app", "old").build());
        assert!(
            service
                .try_publish(LogRecord::builder(LogLevel::Info, "app", "new").build())
                .unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        let published = messaging.reserved_local();
        assert!(
            published
                .iter()
                .any(|(_, msg)| msg.body["message"] == "new")
        );
        assert_eq!(service.stats().dropped, 1);
    }

    #[tokio::test]
    async fn capture_layer_builds_record_when_enabled() {
        let messaging = connected_messaging();
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": {
                "enabled": true,
                "minLevel": "DEBUG"
            } } })),
            Some(messaging as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("trace"))
            .with(LogCaptureLayer::for_isolated_capture_test(service.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(answer = 42_u64, "hello");
        });
        assert_eq!(service.stats().queued, 1);
    }

    struct LoggingReserved {
        inner: Arc<RecordingMessaging>,
    }

    #[async_trait]
    impl ReservedMessaging for LoggingReserved {
        async fn publish_reserved(&self, topic: &str, msg: &Message) -> Result<()> {
            tracing::error!("provider publish warning should not recurse");
            self.inner.publish_reserved(topic, msg).await
        }

        async fn publish_reserved_northbound(
            &self,
            topic: &str,
            msg: &Message,
            qos: Qos,
        ) -> Result<()> {
            self.inner.publish_reserved_northbound(topic, msg, qos).await
        }

        fn connected(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn capture_layer_suppresses_events_emitted_by_log_publisher() {
        let inner = connected_messaging();
        let reserved = Arc::new(LoggingReserved {
            inner: inner.clone(),
        });
        let service = DefaultLogService::start_for_isolated_capture_test(
            config(json!({ "logging": { "publish": {
                "enabled": true,
                "minLevel": "TRACE"
            } } })),
            Some(reserved as Arc<dyn ReservedMessaging>),
        )
        .unwrap();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("trace"))
            .with(LogCaptureLayer::for_isolated_capture_test(service.clone()));

        let _subscriber = tracing::subscriber::set_default(subscriber);
        service
            .publish(
                LogRecord::builder(LogLevel::Info, "app", "one").build(),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(inner.reserved_local().len(), 1);
        assert_eq!(service.stats().queued, 0);
    }
}
