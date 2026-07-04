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

use std::sync::Arc;

use ggstreamlog::config::SinkConfig;
use ggstreamlog::{EmbeddedLog, Record};

use crate::config::model::Config;
use crate::config::template::resolve;
use crate::error::{GgError, Result};

mod bridge;
pub use bridge::StreamMetricsBridge;

// The config schema + the core orchestration live in `ggstreamlog`; this module is a thin
// libs-side wrapper (template resolution against the component `Config` + the metrics bridge).
pub use ggstreamlog::{
    LogStats, Record as StreamRecord, ServiceStats, Sink, SinkFactory, StreamConfig, StreamingConfig,
};

/// Map a core streaming error into the library error type.
fn map_err(e: ggstreamlog::GgStreamError) -> GgError {
    GgError::Streaming(e.to_string())
}

/// Parse the `streaming` section out of a component [`Config`] (empty if absent), **without**
/// template resolution.
pub fn streaming_config(config: &Config) -> Result<StreamingConfig> {
    match config.raw.get("streaming") {
        None => Ok(StreamingConfig::default()),
        Some(value) => serde_json::from_value(value.clone()).map_err(GgError::from),
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

/// The production [`crate::facades::StreamSink`] adapter: composes a [`StreamService`] so
/// `data().via(Channel::stream(name))` (DESIGN-class-facades §4) can append the facade's
/// serialized envelope without depending on the native `ggstreamlog` binding directly. Wired by
/// [`crate::GgCommonsBuilder::build`] whenever the `streaming` feature is compiled in — the facade
/// seam itself lives in [`crate::facades`] so it (and `DataFacade`) build standalone too.
pub struct StreamServiceSink(Arc<dyn StreamService>);

impl StreamServiceSink {
    /// Wraps a [`StreamService`] as a [`crate::facades::StreamSink`].
    pub fn new(service: Arc<dyn StreamService>) -> Self {
        Self(service)
    }
}

impl crate::facades::StreamSink for StreamServiceSink {
    fn append(
        &self,
        stream_name: &str,
        partition_key: &str,
        timestamp_ms: u64,
        payload: Vec<u8>,
    ) -> Result<()> {
        self.0.stream(stream_name)?.append(Record::new(partition_key, timestamp_ms, payload))
    }
}

/// The default [`StreamService`]: a thin wrapper over [`ggstreamlog::StreamService`], adding the
/// component-facing handle/stats types and config-template resolution. The orchestration (opening
/// buffers, building sinks, running export engines) lives in the core and is shared with the
/// other-language bindings.
pub struct DefaultStreamService {
    inner: ggstreamlog::StreamService,
}

impl DefaultStreamService {
    /// Open + recover every configured stream, building the production sink for each (Kinesis under
    /// `streaming-kinesis`, Kafka under `streaming-kafka`, else buffer-only).
    ///
    /// `buffer.path` and the sink's stream name / brokers are template-substituted against `config`.
    pub fn open(config: &Config) -> Result<Self> {
        let inner = ggstreamlog::StreamService::open(resolved_config(config)?).map_err(map_err)?;
        Ok(Self { inner })
    }

    /// Like [`open`](Self::open) but with a custom [`SinkFactory`] (used by tests to inject a fake
    /// sink without AWS/Kafka dependencies).
    pub fn open_with(config: &Config, sink_factory: &SinkFactory) -> Result<Self> {
        let inner = ggstreamlog::StreamService::open_with(resolved_config(config)?, sink_factory)
            .map_err(map_err)?;
        Ok(Self { inner })
    }
}

impl StreamService for DefaultStreamService {
    fn stream(&self, name: &str) -> Result<StreamHandle> {
        self.inner
            .stream(name)
            .map(|log| StreamHandle { name: name.to_string(), log })
            .ok_or_else(|| GgError::Streaming(format!("stream '{name}' is not configured")))
    }

    fn stream_names(&self) -> Vec<String> {
        self.inner.stream_names()
    }

    fn stats(&self, name: &str) -> Option<Stats> {
        self.inner.stats(name).map(|s: ServiceStats| Stats {
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
            last_export_error: None,
        })
    }
}

/// Parse + template-resolve the `streaming` section (buffer paths + sink stream names / brokers).
fn resolved_config(config: &Config) -> Result<StreamingConfig> {
    let mut cfg = streaming_config(config)?;
    for sc in &mut cfg.streams {
        sc.buffer.path = resolve(config, &sc.buffer.path);
        sc.sink = resolve_sink(config, sc.sink.clone());
    }
    Ok(cfg)
}

/// Substitute config templates inside a [`SinkConfig`] (e.g. `{ThingName}` in a Kinesis stream
/// name or a Kafka topic/broker list).
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
        SinkConfig::File(mut f) => {
            // Resolve config templates ({ThingName} etc.) in the output dir + partition path; any
            // remaining tokens (UTC time tokens like {yyyy-MM-dd}) are left for the sink to resolve
            // per file at roll time.
            f.dir = resolve(config, &f.dir);
            f.partition_by = f.partition_by.map(|p| resolve(config, &p));
            SinkConfig::File(f)
        }
        // A host-callback sink has no templated fields; pass it through unchanged.
        SinkConfig::Callback { id } => SinkConfig::Callback { id },
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
        let parsed = streaming_config(&cfg).unwrap();
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
        // Inject a FakeSink instead of Kinesis so the engine drains without AWS. The factory
        // returns the core's Result type (ggstreamlog::StreamService::open_with's contract).
        let factory = |_name: &str, _sink: &SinkConfig| -> ggstreamlog::Result<Option<Box<dyn Sink>>> {
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
