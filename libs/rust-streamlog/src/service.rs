//! `StreamService` — owns every configured stream (a durable [`EmbeddedLog`] + its background
//! [`ExportEngine`]) and is the single orchestration point shared by all consumers: the Rust lib
//! (`libs/rust`), the C-ABI (`ffi`, for the Java/Python/Node bindings), and tests.
//!
//! It is config-driven: [`StreamService::open`] takes a [`StreamingConfig`] (already
//! template-resolved by the caller), opens/recovers each stream's buffer, builds its export sink,
//! and starts draining. Producers get an [`EmbeddedLog`] handle via [`StreamService::stream`] and
//! call `append`. Everything else (batching, retry, checkpointing, retention, drain) is handled by
//! the core.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{SinkConfig, StreamingConfig};
use crate::error::Result;
use crate::export::{ExportEngine, Sink};
use crate::log::{EmbeddedLog, LogStats};

/// Builds a stream's export [`Sink`]. Returns `Ok(None)` for buffer-only mode (no sink available,
/// e.g. the `kinesis` feature is off). Lets tests inject a fake sink.
pub type SinkFactory = dyn Fn(&str, &SinkConfig) -> Result<Option<Box<dyn Sink>>> + Send + Sync;

/// A combined stats snapshot for one stream (buffer + export progress). Numeric-only so it maps
/// cleanly to the C-ABI `ggsl_stats_t`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceStats {
    pub appended_total: u64,
    pub exported_total: u64,
    pub dropped_total: u64,
    pub retries_total: u64,
    pub failed_total: u64,
    pub backlog: u64,
    pub queued: u64,
    pub disk_bytes: u64,
    pub acked_offset: u64,
    pub next_offset: u64,
    pub oldest_unacked_age_ms: u64,
}

struct StreamEntry {
    log: Arc<EmbeddedLog>,
    /// Background drain; kept alive so it keeps running (RAII stop on drop). `None` = buffer-only.
    engine: Option<ExportEngine>,
}

/// Owns all configured streams + their export engines.
pub struct StreamService {
    streams: HashMap<String, StreamEntry>,
}

impl StreamService {
    /// Open + recover every configured stream, building the production sink for each.
    pub fn open(cfg: StreamingConfig) -> Result<Self> {
        Self::open_with(cfg, &default_sink_factory)
    }

    /// Like [`open`](Self::open) but with a custom [`SinkFactory`] (tests inject a fake sink).
    pub fn open_with(cfg: StreamingConfig, sink_factory: &SinkFactory) -> Result<Self> {
        let mut streams = HashMap::new();
        for sc in cfg.streams {
            sc.buffer.validate()?;
            let log = Arc::new(EmbeddedLog::open(sc.buffer.clone())?);
            let engine = match sink_factory(&sc.name, &sc.sink)? {
                Some(sink) => Some(ExportEngine::start(
                    Arc::clone(&log),
                    sink,
                    sc.batch.clone(),
                    sc.delivery.clone(),
                )),
                None => {
                    tracing::warn!(
                        stream = %sc.name,
                        "no export sink available (build with the `kinesis` feature); stream is \
                         buffer-only — records persist but will not be exported"
                    );
                    None
                }
            };
            tracing::info!(stream = %sc.name, exporting = engine.is_some(), "telemetry stream opened");
            streams.insert(sc.name.clone(), StreamEntry { log, engine });
        }
        Ok(Self { streams })
    }

    /// A shared handle to the named stream's durable log (for `append`/`flush`), or `None`.
    pub fn stream(&self, name: &str) -> Option<Arc<EmbeddedLog>> {
        self.streams.get(name).map(|e| Arc::clone(&e.log))
    }

    /// Names of all configured streams (sorted for determinism).
    pub fn stream_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.streams.keys().cloned().collect();
        names.sort();
        names
    }

    /// A combined stats snapshot for the named stream (`None` if not configured).
    pub fn stats(&self, name: &str) -> Option<ServiceStats> {
        let entry = self.streams.get(name)?;
        let ls: LogStats = entry.log.stats();
        let mut s = ServiceStats {
            appended_total: ls.appended_total,
            dropped_total: ls.dropped_total,
            backlog: ls.backlog,
            queued: ls.queued,
            disk_bytes: ls.disk_bytes,
            acked_offset: ls.acked,
            next_offset: ls.next_offset,
            oldest_unacked_age_ms: ls.oldest_unacked_age_ms,
            ..Default::default()
        };
        if let Some(engine) = &entry.engine {
            let e = engine.stats();
            s.exported_total = e.exported_total;
            s.retries_total = e.retries_total;
            s.failed_total = e.failed_total;
        }
        Some(s)
    }

    /// Stop all export engines and flush every buffer to disk (also done on drop).
    pub fn shutdown(self) {
        // Dropping each entry stops its engine (RAII) and flushes the log on Drop.
        drop(self);
    }
}

/// The production sink factory: build a [`crate::KinesisSink`] (feature `kinesis`), else buffer-only.
#[allow(unused_variables)]
fn default_sink_factory(name: &str, sink: &SinkConfig) -> Result<Option<Box<dyn Sink>>> {
    match sink {
        SinkConfig::Kinesis { stream_name, region, endpoint_url } => {
            #[cfg(feature = "kinesis")]
            {
                let s = crate::KinesisSink::new(
                    stream_name.clone(),
                    region.clone(),
                    endpoint_url.clone(),
                )
                .map_err(|e| crate::error::GgStreamError::Sink(e.to_string()))?;
                Ok(Some(Box::new(s)))
            }
            #[cfg(not(feature = "kinesis"))]
            {
                let _ = (stream_name, region, endpoint_url);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BufferConfig, OnFull, SinkConfig};
    use crate::{FakeSink, Record};

    fn cfg(dir: &std::path::Path) -> StreamingConfig {
        StreamingConfig {
            streams: vec![crate::config::StreamConfig {
                name: "telemetry".into(),
                sink: SinkConfig::Kinesis {
                    stream_name: "s".into(),
                    region: None,
                    endpoint_url: None,
                },
                buffer: BufferConfig {
                    path: dir.join("telemetry").to_string_lossy().into_owned(),
                    segment_bytes: 65536,
                    max_disk_bytes: 1 << 20,
                    on_full: OnFull::Block,
                    ..Default::default()
                },
                batch: Default::default(),
                delivery: crate::config::DeliveryConfig { poll_interval_ms: 5, ..Default::default() },
            }],
        }
    }

    #[test]
    fn open_append_and_stats_with_injected_sink() {
        let dir = tempfile::tempdir().unwrap();
        let factory = |_n: &str, _s: &SinkConfig| -> Result<Option<Box<dyn Sink>>> {
            Ok(Some(Box::new(FakeSink::new())))
        };
        let svc = StreamService::open_with(cfg(dir.path()), &factory).unwrap();
        assert_eq!(svc.stream_names(), vec!["telemetry"]);
        let log = svc.stream("telemetry").unwrap();
        for i in 0..100u64 {
            log.append(&Record::new("pk", 1000 + i, format!("v{i}").as_bytes())).unwrap();
        }
        // Wait for the engine to drain.
        let start = std::time::Instant::now();
        while svc.stats("telemetry").unwrap().exported_total < 100 {
            if start.elapsed() > std::time::Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let s = svc.stats("telemetry").unwrap();
        assert_eq!(s.appended_total, 100);
        assert_eq!(s.exported_total, 100);
        assert!(svc.stats("missing").is_none());
    }

    #[test]
    fn buffer_only_without_sink() {
        let dir = tempfile::tempdir().unwrap();
        let factory =
            |_n: &str, _s: &SinkConfig| -> Result<Option<Box<dyn Sink>>> { Ok(None) };
        let svc = StreamService::open_with(cfg(dir.path()), &factory).unwrap();
        let log = svc.stream("telemetry").unwrap();
        log.append(&Record::new("k", 1, b"x")).unwrap();
        log.flush().unwrap();
        let s = svc.stats("telemetry").unwrap();
        assert_eq!(s.appended_total, 1);
        assert_eq!(s.exported_total, 0);
        assert_eq!(s.backlog, 1);
    }
}
