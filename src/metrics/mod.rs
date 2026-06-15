//! # Metrics
//!
//! **One-liner purpose**: Define and emit metrics through a configured target,
//! mirroring the Java/Python `IMetricService` contract.
//!
//! ## Overview
//! Component authors build a [`Metric`] (via [`MetricBuilder`]), `define_metric` it
//! once, then `emit_metric` / `emit_metric_now` measure values by metric name. The
//! [`MetricEmitter`] routes emissions to the target selected by
//! `metricEmission.target`: [`target::log`], [`target::messaging`],
//! [`target::cloudwatch_component`], or [`target::cloudwatch`] (feature `cloudwatch`).
//!
//! ## Semantics & Architecture
//! - `define_metric` / `is_metric_defined` are synchronous (pure registry ops);
//!   `emit_metric*` / `flush_metrics` / `shutdown` are async because targets do I/O.
//! - `emit_metric` is the buffered path, `emit_metric_now` the immediate path
//!   (identical for non-batching targets).
//! - Thread-safety: the metric registry is a `Mutex`; the lock is never held across
//!   an `.await`.
//! - Error handling: [`crate::error::Result`].
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(svc: std::sync::Arc<dyn ggcommons::metrics::MetricService>) -> ggcommons::Result<()> {
//! use ggcommons::metrics::MetricBuilder;
//! use std::collections::HashMap;
//!
//! svc.define_metric(MetricBuilder::create("requests").add_measure("count", "Count", 60).build());
//! let mut values = HashMap::new();
//! values.insert("count".to_string(), 1.0);
//! svc.emit_metric("requests", values).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! The heavy AWS-SDK `cloudwatch` target is behind an off-by-default cargo feature
//! so the standard build stays light; selecting it without the feature is a clear
//! error.
//!
//! ## Related Modules
//! - [`metric`], [`emf`], [`target`].

pub mod emf;
pub mod metric;
pub mod target;

pub use metric::{Measure, Metric, MetricBuilder};
pub use target::MetricTarget;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::config::model::Config;
use crate::config::template::resolve;
use crate::error::{GgError, Result};
use crate::messaging::MessagingService;

/// Define and emit metrics. Mirrors the Java/Python `IMetricService`.
#[async_trait]
pub trait MetricService: Send + Sync {
    /// Register a metric definition by name (replacing any prior definition).
    fn define_metric(&self, metric: Metric);

    /// Whether a metric with `name` has been defined.
    fn is_metric_defined(&self, name: &str) -> bool;

    /// Emit measure values for a defined metric (buffered where the target batches).
    async fn emit_metric(&self, name: &str, values: HashMap<String, f64>) -> Result<()>;

    /// Emit measure values immediately, bypassing batching.
    async fn emit_metric_now(&self, name: &str, values: HashMap<String, f64>) -> Result<()>;

    /// Flush any buffered metrics.
    async fn flush_metrics(&self) -> Result<()>;

    /// Release resources / final flush.
    async fn shutdown(&self);
}

/// Routes metric emissions to the configured [`MetricTarget`]. The default
/// [`MetricService`] implementation.
pub struct MetricEmitter {
    target: Box<dyn MetricTarget>,
    metrics: Mutex<HashMap<String, Metric>>,
}

impl MetricEmitter {
    /// Build an emitter from configuration, selecting the target.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub async fn new(config: &Config, messaging: Option<Arc<dyn MessagingService>>) -> Result<MetricEmitter>`
    /// - `messaging` is required by the `messaging` and `cloudwatchcomponent` targets.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Metrics` | A messaging target was selected without a messaging service, or `cloudwatch` selected without the feature | Provide messaging / enable the `cloudwatch` feature |
    /// | `GgError::Io` | The log target's file could not be created | Check `logFileName` and permissions |
    pub async fn new(
        config: &Config,
        messaging: Option<Arc<dyn MessagingService>>,
    ) -> Result<MetricEmitter> {
        let metric_config = &config.parsed.metric_emission;
        let namespace = metric_config.namespace().to_string();
        let large_fleet = metric_config.large_fleet_workaround;
        let target_name = metric_config.target().to_ascii_lowercase();

        let target: Box<dyn MetricTarget> = match target_name.as_str() {
            "log" => {
                let path = resolve(config, &metric_config.log_file_name());
                Box::new(target::log::LogTarget::new(
                    path,
                    namespace,
                    large_fleet,
                    &metric_config.max_file_size(),
                )?)
            }
            "messaging" => {
                let messaging = require_messaging(messaging, "messaging")?;
                let topic = resolve(config, &metric_config.topic());
                let iot_core = metric_config.destination().eq_ignore_ascii_case("iotcore");
                Box::new(target::messaging::MessagingMetricTarget::new(
                    messaging, topic, iot_core, namespace, large_fleet,
                ))
            }
            "cloudwatchcomponent" => {
                let messaging = require_messaging(messaging, "cloudwatchcomponent")?;
                let topic = resolve(config, &metric_config.topic());
                Box::new(target::cloudwatch_component::CloudWatchComponentTarget::new(
                    messaging, topic, namespace,
                ))
            }
            "cloudwatch" => build_cloudwatch_target(&namespace, large_fleet, metric_config.interval_secs()).await?,
            other => {
                tracing::warn!(target = %other, "unknown metric target; defaulting to 'log'");
                let path = resolve(config, &metric_config.log_file_name());
                Box::new(target::log::LogTarget::new(
                    path,
                    namespace,
                    large_fleet,
                    &metric_config.max_file_size(),
                )?)
            }
        };

        tracing::info!(target = %target_name, "MetricEmitter initialized");
        Ok(MetricEmitter {
            target,
            metrics: Mutex::new(HashMap::new()),
        })
    }

    /// Look up a metric definition by name (cloned for use outside the lock).
    fn lookup(&self, name: &str) -> Option<Metric> {
        self.metrics.lock().ok()?.get(name).cloned()
    }
}

/// Require a messaging service for targets that need one.
fn require_messaging(
    messaging: Option<Arc<dyn MessagingService>>,
    target: &str,
) -> Result<Arc<dyn MessagingService>> {
    messaging.ok_or_else(|| {
        GgError::Metrics(format!("metric target '{target}' requires a messaging service"))
    })
}

/// Construct the CloudWatch SDK target (feature `cloudwatch`), or error if disabled.
#[allow(unused_variables)]
async fn build_cloudwatch_target(
    namespace: &str,
    large_fleet_workaround: bool,
    interval_secs: u64,
) -> Result<Box<dyn MetricTarget>> {
    #[cfg(feature = "cloudwatch")]
    {
        Ok(Box::new(
            target::cloudwatch::CloudWatchTarget::new(namespace, large_fleet_workaround, interval_secs)
                .await?,
        ))
    }
    #[cfg(not(feature = "cloudwatch"))]
    {
        Err(GgError::Metrics(
            "metric target 'cloudwatch' requires the 'cloudwatch' cargo feature".to_string(),
        ))
    }
}

#[async_trait]
impl MetricService for MetricEmitter {
    fn define_metric(&self, metric: Metric) {
        if let Ok(mut map) = self.metrics.lock() {
            map.insert(metric.get_name().to_string(), metric);
        }
    }

    fn is_metric_defined(&self, name: &str) -> bool {
        self.metrics.lock().map(|m| m.contains_key(name)).unwrap_or(false)
    }

    async fn emit_metric(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        match self.lookup(name) {
            Some(metric) => self.target.emit(&metric, &values).await,
            None => {
                tracing::warn!(metric = %name, "metric is not defined; ignoring emit");
                Ok(())
            }
        }
    }

    async fn emit_metric_now(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        match self.lookup(name) {
            Some(metric) => self.target.emit_now(&metric, &values).await,
            None => {
                tracing::warn!(metric = %name, "metric is not defined; ignoring emit");
                Ok(())
            }
        }
    }

    async fn flush_metrics(&self) -> Result<()> {
        self.target.flush().await
    }

    async fn shutdown(&self) {
        self.target.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn log_target_emits_and_defines() {
        let dir = std::env::temp_dir().join(format!("ggcommons-emit-{}", uuid::Uuid::new_v4()));
        let path = dir.join("m.log");
        let raw = json!({
            "metricEmission": {
                "target": "log",
                "namespace": "demo",
                "targetConfig": { "logFileName": path.to_string_lossy() }
            }
        });
        let config = Config::from_value("com.example.C", "thing-1", raw).unwrap();
        let emitter = MetricEmitter::new(&config, None).await.unwrap();

        assert!(!emitter.is_metric_defined("requests"));
        emitter.define_metric(MetricBuilder::create("requests").add_measure("count", "Count", 60).build());
        assert!(emitter.is_metric_defined("requests"));

        let mut values = HashMap::new();
        values.insert("count".to_string(), 2.0);
        emitter.emit_metric("requests", values.clone()).await.unwrap();
        // Undefined metric is ignored, not an error.
        emitter.emit_metric("nope", values).await.unwrap();
        emitter.flush_metrics().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn messaging_target_requires_messaging_service() {
        let raw = json!({ "metricEmission": { "target": "messaging" } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let result = MetricEmitter::new(&config, None).await;
        assert!(matches!(result, Err(GgError::Metrics(_))));
    }
}
