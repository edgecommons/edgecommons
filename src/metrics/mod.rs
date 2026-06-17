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
//! [`target::cloudwatch_component`], or `target::cloudwatch` (feature `cloudwatch`).
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
///
/// The target is swappable: registering the emitter as a
/// [`crate::config::ConfigurationChangeListener`] rebuilds it on config hot-reload.
pub struct MetricEmitter {
    target: Mutex<Arc<dyn MetricTarget>>,
    metrics: Mutex<HashMap<String, Metric>>,
    messaging: Option<Arc<dyn MessagingService>>,
}

impl MetricEmitter {
    /// Build an emitter from configuration, selecting the target.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub async fn new(config: &Config, messaging: Option<Arc<dyn MessagingService>>) -> Result<MetricEmitter>`
    /// - `messaging` is required by the `messaging` and `cloudwatchcomponent` targets;
    ///   it is retained so the target can be rebuilt on config change.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Metrics` | A messaging target was selected without a messaging service, or `cloudwatch` selected without the feature | Provide messaging / enable the `cloudwatch` feature |
    ///
    /// The `log` target is fail-soft: an unwritable `logFileName` does not error
    /// here (it warns and drops metrics on emit), matching the Java target.
    pub async fn new(
        config: &Config,
        messaging: Option<Arc<dyn MessagingService>>,
    ) -> Result<MetricEmitter> {
        let target = build_target(config, messaging.clone()).await?;
        Ok(MetricEmitter {
            target: Mutex::new(target),
            metrics: Mutex::new(HashMap::new()),
            messaging,
        })
    }

    /// Look up a metric definition by name (cloned for use outside the lock).
    fn lookup(&self, name: &str) -> Option<Metric> {
        self.metrics.lock().ok()?.get(name).cloned()
    }

    /// Snapshot the current target (cheap `Arc` clone; lock not held across `.await`).
    fn current_target(&self) -> Result<Arc<dyn MetricTarget>> {
        Ok(self
            .target
            .lock()
            .map_err(|_| GgError::Metrics("metric target mutex poisoned".to_string()))?
            .clone())
    }
}

/// Build the configured metric target.
async fn build_target(
    config: &Config,
    messaging: Option<Arc<dyn MessagingService>>,
) -> Result<Arc<dyn MetricTarget>> {
    let metric_config = &config.parsed.metric_emission;
    let namespace = metric_config.namespace().to_string();
    let large_fleet = metric_config.large_fleet_workaround;
    let target_name = metric_config.target().to_ascii_lowercase();

    let log_target = || -> Result<Arc<dyn MetricTarget>> {
        let path = resolve(config, &metric_config.log_file_name());
        Ok(Arc::new(target::log::LogTarget::new(
            path,
            namespace.clone(),
            large_fleet,
            &metric_config.max_file_size(),
        )?))
    };

    let target: Arc<dyn MetricTarget> = match target_name.as_str() {
        "log" => log_target()?,
        "messaging" => {
            let messaging = require_messaging(messaging, "messaging")?;
            let topic = resolve(config, &metric_config.topic());
            let iot_core = metric_config.destination().eq_ignore_ascii_case("iotcore");
            Arc::new(target::messaging::MessagingMetricTarget::new(
                messaging, topic, iot_core, namespace, large_fleet,
            ))
        }
        "cloudwatchcomponent" => {
            let messaging = require_messaging(messaging, "cloudwatchcomponent")?;
            let topic = resolve(config, &metric_config.topic());
            Arc::new(target::cloudwatch_component::CloudWatchComponentTarget::new(
                messaging, topic, namespace,
            ))
        }
        "cloudwatch" => {
            build_cloudwatch_target(&namespace, large_fleet, metric_config.interval_secs()).await?
        }
        other => {
            tracing::warn!(target = %other, "unknown metric target; defaulting to 'log'");
            log_target()?
        }
    };

    tracing::info!(target = %target_name, "metric target built");
    Ok(target)
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
) -> Result<Arc<dyn MetricTarget>> {
    #[cfg(feature = "cloudwatch")]
    {
        Ok(Arc::new(
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
            Some(metric) => self.current_target()?.emit(&metric, &values).await,
            None => {
                tracing::warn!(metric = %name, "metric is not defined; ignoring emit");
                Ok(())
            }
        }
    }

    async fn emit_metric_now(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        match self.lookup(name) {
            Some(metric) => self.current_target()?.emit_now(&metric, &values).await,
            None => {
                tracing::warn!(metric = %name, "metric is not defined; ignoring emit");
                Ok(())
            }
        }
    }

    async fn flush_metrics(&self) -> Result<()> {
        self.current_target()?.flush().await
    }

    async fn shutdown(&self) {
        if let Ok(target) = self.current_target() {
            target.shutdown().await;
        }
    }
}

#[async_trait]
impl crate::config::ConfigurationChangeListener for MetricEmitter {
    /// Rebuild the metric target from the new config (keeping the previous one on error).
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        match build_target(&config, self.messaging.clone()).await {
            Ok(target) => {
                if let Ok(mut slot) = self.target.lock() {
                    *slot = target;
                }
                tracing::info!("metric target reconfigured after config change");
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to rebuild metric target on config change; keeping previous");
                false
            }
        }
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

    fn one_value() -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert("count".to_string(), 1.0);
        v
    }

    fn define(emitter: &MetricEmitter) {
        emitter.define_metric(MetricBuilder::create("m").add_measure("count", "Count", 60).build());
    }

    #[tokio::test]
    async fn messaging_target_builds_and_emits() {
        let raw = json!({ "metricEmission": { "target": "messaging", "targetConfig": { "topic": "m/t", "destination": "ipc" } } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new(&config, Some(recorder.clone())).await.unwrap();
        define(&emitter);
        emitter.emit_metric("m", one_value()).await.unwrap();
        assert_eq!(recorder.local().len(), 1, "messaging target should publish EMF");
    }

    #[tokio::test]
    async fn cloudwatchcomponent_target_builds_and_emits() {
        let raw = json!({ "metricEmission": { "target": "cloudwatchcomponent" } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new(&config, Some(recorder.clone())).await.unwrap();
        define(&emitter);
        emitter.emit_metric_now("m", one_value()).await.unwrap();
        assert_eq!(recorder.local().len(), 1);
        assert_eq!(recorder.local()[0].0, "cloudwatch/metric/put");
    }

    #[tokio::test]
    async fn cloudwatch_target_without_feature_is_error() {
        let raw = json!({ "metricEmission": { "target": "cloudwatch" } });
        let config = Config::from_value("c", "t", raw).unwrap();
        // Without the `cloudwatch` cargo feature, selecting it is a clear error.
        let result = MetricEmitter::new(&config, None).await;
        #[cfg(not(feature = "cloudwatch"))]
        assert!(matches!(result, Err(GgError::Metrics(_))));
        #[cfg(feature = "cloudwatch")]
        let _ = result; // with the feature it constructs (needs AWS env at emit time)
    }

    #[tokio::test]
    async fn unknown_target_defaults_to_log() {
        let dir = std::env::temp_dir().join(format!("ggcommons-unk-{}", uuid::Uuid::new_v4()));
        let path = dir.join("m.log");
        let raw = json!({ "metricEmission": { "target": "bogus", "targetConfig": { "logFileName": path.to_string_lossy() } } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let emitter = MetricEmitter::new(&config, None).await.unwrap();
        define(&emitter);
        emitter.emit_metric_now("m", one_value()).await.unwrap();
        emitter.flush_metrics().await.unwrap();
        emitter.shutdown().await;
        assert!(std::fs::read_to_string(&path).unwrap().lines().count() >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn on_config_change_rebuilds_target() {
        use crate::config::ConfigurationChangeListener;

        let dir = std::env::temp_dir().join(format!("ggcommons-recfg-{}", uuid::Uuid::new_v4()));
        let path = dir.join("m.log");
        let raw_log = json!({ "metricEmission": { "target": "log", "targetConfig": { "logFileName": path.to_string_lossy() } } });
        let config = Config::from_value("c", "t", raw_log).unwrap();
        // Build with messaging available so the rebuilt messaging target can be created.
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new(&config, Some(recorder.clone())).await.unwrap();
        define(&emitter);

        // Reconfigure to a messaging target.
        let raw_msg = json!({ "metricEmission": { "target": "messaging", "targetConfig": { "topic": "mt", "destination": "ipc" } } });
        let new_cfg = Arc::new(Config::from_value("c", "t", raw_msg).unwrap());
        assert!(emitter.on_configuration_change(new_cfg).await, "rebuild should succeed");

        emitter.emit_metric_now("m", one_value()).await.unwrap();
        assert_eq!(recorder.local().len(), 1, "metrics now flow to the messaging target");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn emitting_undefined_metric_is_ignored() {
        let dir = std::env::temp_dir().join(format!("ggcommons-undef-{}", uuid::Uuid::new_v4()));
        let path = dir.join("m.log");
        let raw = json!({ "metricEmission": { "target": "log", "targetConfig": { "logFileName": path.to_string_lossy() } } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let emitter = MetricEmitter::new(&config, None).await.unwrap();
        // No definition -> both emit paths are no-ops (Ok), not errors.
        emitter.emit_metric("nope", one_value()).await.unwrap();
        emitter.emit_metric_now("nope", one_value()).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
