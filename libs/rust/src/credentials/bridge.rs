//! # Credentials → metrics bridge
//!
//! **One-liner purpose**: Periodically surface non-sensitive credential-subsystem [`CredentialStats`]
//! through the component's [`MetricService`] (CloudWatch / messaging / log), mirroring
//! [`crate::streaming::StreamMetricsBridge`]. **Never emits secret values.**

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::service::CredentialService;
use crate::metrics::{MetricBuilder, MetricService};

const DEFAULT_INTERVAL_SECS: u64 = 30;
const METRIC: &str = "credentials";

/// Owns the background stats-emission task; aborts it on drop (RAII).
pub struct CredentialMetricsBridge {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for CredentialMetricsBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl CredentialMetricsBridge {
    /// Emit credential stats every [`DEFAULT_INTERVAL_SECS`] through `metrics`.
    pub fn start(creds: Arc<dyn CredentialService>, metrics: Arc<dyn MetricService>) -> Self {
        Self::start_with_interval(creds, metrics, Duration::from_secs(DEFAULT_INTERVAL_SECS))
    }

    /// As [`start`](Self::start) but with an explicit interval (used by tests).
    pub fn start_with_interval(
        creds: Arc<dyn CredentialService>,
        metrics: Arc<dyn MetricService>,
        interval: Duration,
    ) -> Self {
        metrics.define_metric(
            MetricBuilder::create(METRIC)
                .add_measure("secretCount", "Count", 60)
                .add_measure("lastSyncAgeMs", "Milliseconds", 60)
                .add_measure("syncFailures", "Count", 60)
                .add_measure("rotations", "Count", 60)
                .build(),
        );
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let s = creds.stats();
                let mut m = HashMap::with_capacity(4);
                m.insert("secretCount".to_string(), s.secret_count as f64);
                m.insert(
                    "lastSyncAgeMs".to_string(),
                    s.last_sync_age_ms.unwrap_or(0) as f64,
                );
                m.insert("syncFailures".to_string(), s.sync_failures as f64);
                m.insert("rotations".to_string(), s.rotations as f64);
                if let Err(e) = metrics.emit_metric(METRIC, m).await {
                    tracing::debug!(error = %e, "failed to emit credential stats");
                }
            }
        });
        Self { task }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::service::{CredentialStats, Secret, SecretMeta};
    use crate::credentials::{CredentialService, PutOptions};
    use crate::error::Result;
    use crate::metrics::Metric;
    use std::collections::HashMap as Map;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A credential service that only reports fixed stats (the only thing the bridge reads).
    struct StatsOnlyCreds(CredentialStats);
    impl CredentialService for StatsOnlyCreds {
        fn get(&self, _: &str) -> Result<Option<Secret>> {
            Ok(None)
        }
        fn get_version(&self, _: &str, _: &str) -> Result<Option<Secret>> {
            Ok(None)
        }
        fn exists(&self, _: &str) -> Result<bool> {
            Ok(false)
        }
        fn list(&self, _: &str) -> Result<Vec<SecretMeta>> {
            Ok(Vec::new())
        }
        fn versions(&self, _: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn put(&self, _: &str, _: &[u8], _: PutOptions) -> Result<String> {
            Ok("1".into())
        }
        fn delete(&self, _: &str) -> Result<bool> {
            Ok(false)
        }
        fn stats(&self) -> CredentialStats {
            self.0.clone()
        }
    }

    /// A metric service that records define/emit calls for assertions.
    #[derive(Default)]
    struct RecordingMetrics {
        defined: Mutex<Vec<String>>,
        emitted: Mutex<Vec<(String, Map<String, f64>)>>,
    }
    #[async_trait::async_trait]
    impl MetricService for RecordingMetrics {
        fn define_metric(&self, m: Metric) {
            self.defined.lock().unwrap().push(m.get_name().to_string());
        }
        fn is_metric_defined(&self, n: &str) -> bool {
            self.defined.lock().unwrap().iter().any(|d| d == n)
        }
        async fn emit_metric(&self, n: &str, v: Map<String, f64>) -> Result<()> {
            self.emitted.lock().unwrap().push((n.to_string(), v));
            Ok(())
        }
        async fn emit_metric_now(&self, n: &str, v: Map<String, f64>) -> Result<()> {
            self.emit_metric(n, v).await
        }
        async fn flush_metrics(&self) -> Result<()> {
            Ok(())
        }
        async fn shutdown(&self) {}
    }

    #[tokio::test]
    async fn bridge_defines_and_emits_credential_stats() {
        let creds: Arc<dyn CredentialService> = Arc::new(StatsOnlyCreds(CredentialStats {
            secret_count: 4,
            last_sync_age_ms: Some(250),
            sync_failures: 2,
            rotations: 3,
        }));
        let metrics = Arc::new(RecordingMetrics::default());
        let bridge = CredentialMetricsBridge::start_with_interval(
            creds,
            metrics.clone() as Arc<dyn MetricService>,
            Duration::from_millis(30),
        );

        // The metric is defined up front.
        assert!(metrics.is_metric_defined(METRIC));

        // Wait for at least one tick to emit the stats.
        let mut emission = None;
        for _ in 0..50 {
            if let Some(e) = metrics.emitted.lock().unwrap().first().cloned() {
                emission = Some(e);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(bridge);

        let (name, values) = emission.expect("the bridge should have emitted credential stats");
        assert_eq!(name, METRIC);
        assert_eq!(values["secretCount"], 4.0);
        assert_eq!(values["lastSyncAgeMs"], 250.0);
        assert_eq!(values["syncFailures"], 2.0);
        assert_eq!(values["rotations"], 3.0);
    }
}
