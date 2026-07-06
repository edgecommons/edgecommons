//! # Streaming → metrics bridge
//!
//! **One-liner purpose**: Periodically surface each telemetry stream's [`Stats`] through the
//! component's configured [`MetricService`] (and thus CloudWatch / messaging / log targets).
//!
//! ## Overview
//! [`StreamMetricsBridge::start`] spawns a background task that, every `interval`, reads stats
//! for every configured stream and emits them as a metric named `stream:<name>` with one
//! measure per counter. Dropping the bridge aborts the task (RAII), mirroring
//! [`crate::heartbeat::Heartbeat`].
//!
//! ## Semantics & Architecture
//! - One metric is defined per stream (`stream:<name>`) with measures `backlog`,
//!   `droppedTotal`, `exportedTotal`, `retriesTotal`, `failedTotal`, `diskBytes`,
//!   `oldestUnackedAgeMs`. `droppedTotal > 0` makes a `dropOldest` buffer visibly lossy.
//! - The task holds only `Arc`s; it never blocks the export or append paths.
//! - Error handling: emit failures are logged and swallowed (telemetry about telemetry must
//!   never take the component down).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::metrics::{MetricBuilder, MetricService};

use super::{Stats, StreamService};

/// Default cadence for emitting stream stats.
const DEFAULT_INTERVAL_SECS: u64 = 30;

/// Owns the background stats-emission task; aborts it on drop (RAII).
pub struct StreamMetricsBridge {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StreamMetricsBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl StreamMetricsBridge {
    /// Start emitting stats for all of `streams`' streams through `metrics` every
    /// `DEFAULT_INTERVAL_SECS`. Returns `None` if there are no streams to report.
    pub fn start(
        streams: Arc<dyn StreamService>,
        metrics: Arc<dyn MetricService>,
    ) -> Option<Self> {
        Self::start_with_interval(streams, metrics, Duration::from_secs(DEFAULT_INTERVAL_SECS))
    }

    /// As [`start`](Self::start) but with an explicit interval (used by tests).
    pub fn start_with_interval(
        streams: Arc<dyn StreamService>,
        metrics: Arc<dyn MetricService>,
        interval: Duration,
    ) -> Option<Self> {
        let names = streams.stream_names();
        if names.is_empty() {
            return None;
        }

        // Define one metric per stream up front.
        for name in &names {
            metrics.define_metric(define_stream_metric(name));
        }

        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Skip the immediate first tick's burst behavior; emit on each subsequent tick.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                for name in &names {
                    if let Some(stats) = streams.stats(name) {
                        let metric = stream_metric_name(name);
                        if let Err(e) = metrics.emit_metric(&metric, stats_measures(&stats)).await {
                            tracing::debug!(stream = %name, error = %e, "failed to emit stream stats");
                        }
                    }
                }
            }
        });
        Some(Self { task })
    }
}

/// The metric name for a stream's stats.
fn stream_metric_name(name: &str) -> String {
    format!("stream:{name}")
}

/// Define the per-stream metric with one measure per [`Stats`] counter.
fn define_stream_metric(name: &str) -> crate::metrics::Metric {
    MetricBuilder::create(stream_metric_name(name))
        .add_measure("backlog", "Count", 60)
        .add_measure("droppedTotal", "Count", 60)
        .add_measure("exportedTotal", "Count", 60)
        .add_measure("retriesTotal", "Count", 60)
        .add_measure("failedTotal", "Count", 60)
        .add_measure("diskBytes", "Bytes", 60)
        .add_measure("oldestUnackedAgeMs", "Milliseconds", 60)
        .build()
}

/// Map a [`Stats`] snapshot to measure values.
fn stats_measures(s: &Stats) -> HashMap<String, f64> {
    let mut m = HashMap::with_capacity(7);
    m.insert("backlog".to_string(), s.backlog as f64);
    m.insert("droppedTotal".to_string(), s.dropped_total as f64);
    m.insert("exportedTotal".to_string(), s.exported_total as f64);
    m.insert("retriesTotal".to_string(), s.retries_total as f64);
    m.insert("failedTotal".to_string(), s.failed_total as f64);
    m.insert("diskBytes".to_string(), s.disk_bytes as f64);
    m.insert("oldestUnackedAgeMs".to_string(), s.oldest_unacked_age_ms as f64);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Config;
    use crate::metrics::MetricEmitter;
    use crate::streaming::DefaultStreamService;
    use edgestreamlog::{FakeSink, Record, Sink};
    use serde_json::json;
    use std::path::Path;

    fn cfg(path: &Path) -> Config {
        let raw = json!({
            "metricEmission": { "target": "log",
                "targetConfig": { "logFileName": path.join("m.log").to_string_lossy() } },
            "streaming": { "streams": [{
                "name": "telemetry",
                "sink": { "type": "kinesis", "streamName": "x" },
                "buffer": { "path": path.join("telemetry").to_string_lossy(),
                            "segmentBytes": 65536, "maxDiskBytes": 1048576, "onFull": "block" },
                "delivery": { "pollIntervalMs": 10 }
            }]}
        });
        Config::from_value("com.example.C", "thing-1", raw).unwrap()
    }

    #[tokio::test]
    async fn bridge_emits_stream_stats_to_metric_target() {
        let dir = tempfile::tempdir().unwrap();
        let config = cfg(dir.path());

        let factory = |_n: &str, _s: &super::super::SinkConfig| -> edgestreamlog::Result<Option<Box<dyn Sink>>> {
            Ok(Some(Box::new(FakeSink::new())))
        };
        let svc: Arc<dyn StreamService> =
            Arc::new(DefaultStreamService::open_with(&config, &factory).unwrap());
        let h = svc.stream("telemetry").unwrap();
        for i in 0..20u64 {
            h.append(Record::new("pk", 1000 + i, format!("v{i}").as_bytes())).unwrap();
        }

        let metrics: Arc<dyn MetricService> =
            Arc::new(MetricEmitter::new(&config, None).await.unwrap());
        let bridge = StreamMetricsBridge::start_with_interval(
            Arc::clone(&svc),
            Arc::clone(&metrics),
            Duration::from_millis(50),
        )
        .expect("bridge should start when streams exist");

        // The metric was defined for the stream.
        assert!(metrics.is_metric_defined("stream:telemetry"));

        // Wait for at least one emission to land in the log target.
        let log_path = dir.path().join("m.log");
        let mut wrote = false;
        for _ in 0..40 {
            if std::fs::read_to_string(&log_path).map(|c| !c.trim().is_empty()).unwrap_or(false) {
                wrote = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        drop(bridge);
        assert!(wrote, "bridge should have emitted at least one stats line to the metric target");
    }

    #[test]
    fn bridge_is_none_without_streams() {
        // No tokio runtime needed: start() returns before spawning when there are no streams.
        let svc: Arc<dyn StreamService> = Arc::new(EmptyStreams);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let metrics: Arc<dyn MetricService> = Arc::new(NoopMetrics);
        assert!(StreamMetricsBridge::start(svc, metrics).is_none());
    }

    struct EmptyStreams;
    impl StreamService for EmptyStreams {
        fn stream(&self, _: &str) -> crate::Result<crate::streaming::StreamHandle> {
            Err(crate::EdgeCommonsError::Streaming("none".into()))
        }
        fn stream_names(&self) -> Vec<String> {
            Vec::new()
        }
        fn stats(&self, _: &str) -> Option<Stats> {
            None
        }
    }

    struct NoopMetrics;
    #[async_trait::async_trait]
    impl MetricService for NoopMetrics {
        fn define_metric(&self, _: crate::metrics::Metric) {}
        fn is_metric_defined(&self, _: &str) -> bool {
            false
        }
        async fn emit_metric(&self, _: &str, _: HashMap<String, f64>) -> crate::Result<()> {
            Ok(())
        }
        async fn emit_metric_now(&self, _: &str, _: HashMap<String, f64>) -> crate::Result<()> {
            Ok(())
        }
        async fn flush_metrics(&self) -> crate::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) {}
    }
}
