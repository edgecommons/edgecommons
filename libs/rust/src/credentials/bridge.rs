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
                m.insert("lastSyncAgeMs".to_string(), s.last_sync_age_ms.unwrap_or(0) as f64);
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
