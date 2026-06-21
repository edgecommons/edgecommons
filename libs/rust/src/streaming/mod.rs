//! # Telemetry streaming
//!
//! **One-liner purpose**: Durable, store-and-forward high-rate telemetry streams with a
//! pluggable export sink, wired into the component runtime as `gg.streams()`.
//!
//! ## Overview
//! A thin libs-side façade over the [`ggstreamlog`] core (the durable segment log + export
//! engine). Each configured stream gets an [`ggstreamlog::EmbeddedLog`] (append/persist/retain)
//! and, when a sink is available, a background [`ggstreamlog::ExportEngine`] draining it to AWS.
//! Component authors get a [`StreamHandle`] from [`StreamService::stream`] and call
//! [`StreamHandle::append`]; everything else (batching, retry, checkpointing, retention) is
//! handled by the core. See `docs/TELEMETRY_STREAMING.md` and `..._PHASE1.md`.
//!
//! ## Semantics & Architecture
//! - The whole module is behind the off-by-default `streaming` cargo feature; the real AWS
//!   sink ([`ggstreamlog::KinesisSink`]) additionally needs `streaming-kinesis`.
//! - [`StreamService`] is the testable seam (mirrors [`crate::metrics::MetricService`]); the
//!   default implementation is [`DefaultStreamService`].
//! - Without a usable sink (e.g. `streaming` without `streaming-kinesis`), a stream is
//!   **buffer-only**: appends still persist durably and `stats()` works, but nothing exports
//!   (a warning is logged). This keeps the feature usable in tests without AWS dependencies.
//! - Configuration is read from the `streaming` section of the raw component config, reusing
//!   `ggstreamlog`'s `BufferConfig`/`BatchConfig`/`DeliveryConfig`/`SinkConfig` (same camelCase
//!   schema). `buffer.path` and the Kinesis `streamName` are template-substituted.
//! - Error handling: [`crate::Result`] / [`GgError::Streaming`].
//!
//! ## Related Modules
//! - [`crate::metrics`] (the stats bridge target), [`ggstreamlog`] (the core).

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;

use ggstreamlog::config::{BatchConfig, BufferConfig, DeliveryConfig, SinkConfig};
use ggstreamlog::{EmbeddedLog, ExportEngine, Record, Sink};

use crate::config::model::Config;
use crate::config::template::resolve;
use crate::error::{GgError, Result};

mod bridge;
pub use bridge::StreamMetricsBridge;

pub use ggstreamlog::{LogStats, Record as StreamRecord};

/// Map a core streaming error into the library error type.
fn map_err(e: ggstreamlog::GgStreamError) -> GgError {
    GgError::Streaming(e.to_string())
}

/// `streaming` config section: a set of named streams.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamingConfig {
    pub streams: Vec<StreamConfig>,
}

/// One configured stream (name + buffer + export sink + batching/delivery tuning).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamConfig {
    pub name: String,
    pub sink: SinkConfig,
    pub buffer: BufferConfig,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub delivery: DeliveryConfig,
}

impl StreamingConfig {
    /// Parse the `streaming` section out of a component [`Config`] (empty if absent).
    pub fn from_config(config: &Config) -> Result<Self> {
        match config.raw.get("streaming") {
            None => Ok(Self::default()),
            Some(value) => serde_json::from_value(value.clone()).map_err(GgError::from),
        }
    }
}

/// A point-in-time view of one stream (buffer + export progress). Mirrors the spec's `Stats`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub appended_total: u64,
    pub exported_total: u64,
    pub dropped_total: u64,
    pub retries_total: u64,
    pub failed_total: u64,
    /// Un-delivered records currently buffered.
    pub backlog: u64,
    pub disk_bytes: u64,
    pub acked_offset: u64,
    pub next_offset: u64,
    pub oldest_unacked_age_ms: u64,
    pub last_export_error: Option<String>,
}

/// Append records to a stream. Cheap to clone; safe to share across threads.
#[derive(Clone)]
pub struct StreamHandle {
    name: String,
    log: Arc<EmbeddedLog>,
}

impl StreamHandle {
    /// Append one record (honors the buffer's `onFull` policy; may block under `Block`).
    pub fn append(&self, rec: Record) -> Result<()> {
        self.log.append(&rec).map_err(map_err)
    }

    /// Append a batch of records (one fsync at the end under `PerBatch`).
    pub fn append_batch(&self, recs: &[Record]) -> Result<()> {
        self.log.append_batch(recs).map_err(map_err)
    }

    /// Force the buffer durably to disk (does **not** wait for export to the sink).
    pub fn flush(&self) -> Result<()> {
        self.log.flush().map_err(map_err)
    }

    /// The stream name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Open/own telemetry streams. The testable seam for the streaming subsystem.
pub trait StreamService: Send + Sync {
    /// A handle to the named stream, or [`GgError::Streaming`] if it is not configured.
    fn stream(&self, name: &str) -> Result<StreamHandle>;

    /// Names of all configured streams.
    fn stream_names(&self) -> Vec<String>;

    /// A stats snapshot for the named stream (`None` if not configured).
    fn stats(&self, name: &str) -> Option<Stats>;
}

/// A builder for a stream's export [`Sink`]; lets tests inject a fake sink. Returns `Ok(None)`
/// for buffer-only mode (no sink available).
pub type SinkFactory = dyn Fn(&str, &SinkConfig) -> Result<Option<Box<dyn Sink>>> + Send + Sync;

struct StreamEntry {
    log: Arc<EmbeddedLog>,
    /// Kept alive so the background export loop keeps running; stopped on drop (RAII). Also
    /// read for export counters via [`ExportEngine::stats`].
    engine: Option<ExportEngine>,
}

/// The default [`StreamService`]: one [`EmbeddedLog`] (+ optional [`ExportEngine`]) per stream.
pub struct DefaultStreamService {
    streams: HashMap<String, StreamEntry>,
}

impl DefaultStreamService {
    /// Open + recover every configured stream, building the production sink for each.
    ///
    /// `buffer.path` and the Kinesis `streamName` are template-substituted against `config`.
    pub fn open(config: &Config) -> Result<Self> {
        Self::open_with(config, &default_sink_factory)
    }

    /// Like [`open`](Self::open) but with a custom [`SinkFactory`] (used by tests to inject a
    /// fake sink without AWS dependencies).
    pub fn open_with(config: &Config, sink_factory: &SinkFactory) -> Result<Self> {
        let streaming = StreamingConfig::from_config(config)?;
        let mut streams = HashMap::new();

        for mut sc in streaming.streams {
            // Resolve templates in the buffer path and (for Kinesis) the destination stream name.
            sc.buffer.path = resolve(config, &sc.buffer.path);
            let sink_cfg = resolve_sink(config, sc.sink.clone());
            sc.buffer.validate().map_err(map_err)?;

            let log = Arc::new(EmbeddedLog::open(sc.buffer.clone()).map_err(map_err)?);

            let engine = match sink_factory(&sc.name, &sink_cfg)? {
                Some(sink) => Some(ExportEngine::start(
                    Arc::clone(&log),
                    sink,
                    sc.batch.clone(),
                    sc.delivery.clone(),
                )),
                None => {
                    tracing::warn!(
                        stream = %sc.name,
                        "no export sink available (enable the 'streaming-kinesis' feature); \
                         stream is buffer-only — records persist but will not be exported"
                    );
                    None
                }
            };

            tracing::info!(stream = %sc.name, path = %sc.buffer.path, exporting = engine.is_some(),
                "telemetry stream opened");
            streams.insert(sc.name.clone(), StreamEntry { log, engine });
        }

        Ok(Self { streams })
    }
}

impl StreamService for DefaultStreamService {
    fn stream(&self, name: &str) -> Result<StreamHandle> {
        self.streams
            .get(name)
            .map(|e| StreamHandle { name: name.to_string(), log: Arc::clone(&e.log) })
            .ok_or_else(|| GgError::Streaming(format!("stream '{name}' is not configured")))
    }

    fn stream_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.streams.keys().cloned().collect();
        names.sort();
        names
    }

    fn stats(&self, name: &str) -> Option<Stats> {
        let entry = self.streams.get(name)?;
        let ls: LogStats = entry.log.stats();
        let mut stats = Stats {
            appended_total: ls.appended_total,
            dropped_total: ls.dropped_total,
            backlog: ls.backlog,
            disk_bytes: ls.disk_bytes,
            acked_offset: ls.acked,
            next_offset: ls.next_offset,
            oldest_unacked_age_ms: ls.oldest_unacked_age_ms,
            ..Default::default()
        };
        if let Some(engine) = &entry.engine {
            let e = engine.stats();
            stats.exported_total = e.exported_total;
            stats.retries_total = e.retries_total;
            stats.failed_total = e.failed_total;
            stats.last_export_error = e.last_error;
        }
        Some(stats)
    }
}

/// Substitute config templates inside a [`SinkConfig`] (e.g. `{ThingName}` in a Kinesis stream name).
fn resolve_sink(config: &Config, sink: SinkConfig) -> SinkConfig {
    match sink {
        SinkConfig::Kinesis { stream_name, region, endpoint_url } => SinkConfig::Kinesis {
            stream_name: resolve(config, &stream_name),
            region,
            endpoint_url,
        },
        SinkConfig::Kafka { bootstrap_servers, topic, properties } => SinkConfig::Kafka {
            bootstrap_servers: resolve(config, &bootstrap_servers),
            topic: resolve(config, &topic),
            properties,
        },
    }
}

/// The production sink factory: build a [`ggstreamlog::KinesisSink`] (feature `streaming-kinesis`),
/// else buffer-only.
#[allow(unused_variables)]
fn default_sink_factory(name: &str, sink: &SinkConfig) -> Result<Option<Box<dyn Sink>>> {
    match sink {
        SinkConfig::Kinesis { stream_name, region, endpoint_url } => {
            #[cfg(feature = "streaming-kinesis")]
            {
                let s = ggstreamlog::KinesisSink::new(
                    stream_name.clone(),
                    region.clone(),
                    endpoint_url.clone(),
                )
                .map_err(map_err)?;
                Ok(Some(Box::new(s)))
            }
            #[cfg(not(feature = "streaming-kinesis"))]
            {
                Ok(None) // buffer-only without the AWS sink feature
            }
        }
        SinkConfig::Kafka { bootstrap_servers, topic, properties } => {
            #[cfg(feature = "streaming-kafka")]
            {
                let s = ggstreamlog::KafkaSink::new(bootstrap_servers, topic, properties)
                    .map_err(map_err)?;
                Ok(Some(Box::new(s)))
            }
            #[cfg(not(feature = "streaming-kafka"))]
            {
                Ok(None) // buffer-only without the Kafka sink feature
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggstreamlog::FakeSink;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn config_with_streams(path: &std::path::Path) -> Config {
        let raw = json!({
            "streaming": {
                "streams": [{
                    "name": "telemetry",
                    "sink": { "type": "kinesis", "streamName": "ts-{ThingName}" },
                    "buffer": {
                        "path": path.join("telemetry").to_string_lossy(),
                        "segmentBytes": 65536,
                        "maxDiskBytes": 1048576,
                        "onFull": "block"
                    },
                    "batch": { "maxRecords": 50 },
                    "delivery": { "pollIntervalMs": 10, "backoffBaseMs": 5, "backoffMaxMs": 50 }
                }]
            }
        });
        Config::from_value("com.example.C", "thing-7", raw).unwrap()
    }

    #[test]
    fn parses_streaming_config_section() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_with_streams(dir.path());
        let parsed = StreamingConfig::from_config(&cfg).unwrap();
        assert_eq!(parsed.streams.len(), 1);
        assert_eq!(parsed.streams[0].name, "telemetry");
        assert_eq!(parsed.streams[0].batch.max_records, 50);
        match &parsed.streams[0].sink {
            SinkConfig::Kinesis { stream_name, .. } => assert_eq!(stream_name, "ts-{ThingName}"),
            other => panic!("expected Kinesis sink, got {other:?}"),
        }
    }

    #[test]
    fn absent_section_yields_no_streams() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        let svc = DefaultStreamService::open(&cfg).unwrap();
        assert!(svc.stream_names().is_empty());
        assert!(svc.stream("nope").is_err());
        assert!(svc.stats("nope").is_none());
    }

    #[test]
    fn buffer_only_mode_persists_without_a_sink() {
        // default_sink_factory returns None without `streaming-kinesis`: append still works.
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_with_streams(dir.path());
        let svc = DefaultStreamService::open(&cfg).unwrap();
        assert_eq!(svc.stream_names(), vec!["telemetry"]);
        let h = svc.stream("telemetry").unwrap();
        for i in 0..10u64 {
            h.append(Record::new("pk", 1000 + i, format!("v{i}").as_bytes())).unwrap();
        }
        h.flush().unwrap();
        let s = svc.stats("telemetry").unwrap();
        assert_eq!(s.appended_total, 10);
        assert_eq!(s.next_offset, 10);
        assert_eq!(s.backlog, 10, "buffer-only: nothing exported");
        assert_eq!(s.exported_total, 0);
    }

    #[test]
    fn template_substitution_in_buffer_path() {
        let dir = tempfile::tempdir().unwrap();
        let raw = json!({
            "streaming": { "streams": [{
                "name": "t",
                "sink": { "type": "kinesis", "streamName": "x" },
                "buffer": { "path": dir.path().join("{ThingName}").to_string_lossy(),
                            "segmentBytes": 65536, "maxDiskBytes": 1048576 }
            }]}
        });
        let cfg = Config::from_value("c", "thing-9", raw).unwrap();
        let svc = DefaultStreamService::open(&cfg).unwrap();
        let h = svc.stream("t").unwrap();
        h.append(Record::new("k", 1, b"x")).unwrap();
        h.flush().unwrap();
        // The {ThingName}-substituted directory must now exist on disk.
        assert!(dir.path().join("thing-9").is_dir(), "buffer path template should resolve to thing-9");
    }

    #[test]
    fn injected_sink_drains_and_stats_reflect_export() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_with_streams(dir.path());
        // Inject a FakeSink instead of Kinesis so the engine drains without AWS.
        let factory = |_name: &str, _sink: &SinkConfig| -> Result<Option<Box<dyn Sink>>> {
            Ok(Some(Box::new(FakeSink::new())))
        };
        let svc = DefaultStreamService::open_with(&cfg, &factory).unwrap();
        let h = svc.stream("telemetry").unwrap();
        for i in 0..100u64 {
            h.append(Record::new("pk", 1000 + i, format!("v{i}").as_bytes())).unwrap();
        }

        let start = Instant::now();
        let drained = loop {
            if svc.stats("telemetry").unwrap().exported_total == 100 {
                break true;
            }
            if start.elapsed() > Duration::from_secs(5) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let s = svc.stats("telemetry").unwrap();
        assert!(drained, "engine should drain all 100 records, got exported={}", s.exported_total);
        assert_eq!(s.exported_total, 100);
        assert_eq!(s.failed_total, 0);
    }
}
