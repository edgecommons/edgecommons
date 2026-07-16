//! # Metrics
//!
//! **One-liner purpose**: Define and emit metrics through a configured target,
//! mirroring the Java/Python `IMetricService` contract.
//!
//! ## Overview
//! Component authors build a [`Metric`] (via [`MetricBuilder`]), `define_metric` it
//! once, then `emit_metric` / `emit_metric_now` measure values by metric name. The
//! [`MetricEmitter`] routes emissions to the EFFECTIVE target — `explicit
//! metricEmission.target ▸ platform-profile default ▸ "log"` (see [`resolve_effective_target`]):
//! [`target::log`], [`target::messaging`], [`target::cloudwatch_component`],
//! `target::cloudwatch` (feature `cloudwatch`), or `target::prometheus` (feature
//! `metrics-prometheus`; the pull-based default on KUBERNETES).
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
//! # async fn demo(svc: std::sync::Arc<dyn edgecommons::metrics::MetricService>) -> edgecommons::Result<()> {
//! use edgecommons::metrics::MetricBuilder;
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

use crate::config::model::{Config, MetricConfig};
use crate::config::template::resolve;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::{MessagingService, ReservedMessaging};
use crate::platform::Platform;

/// The external AWS Greengrass CloudWatch-component contract topic — unchanged by
/// the UNS migration (non-`ecv1`, guard-exempt; D-U21).
const CLOUDWATCH_COMPONENT_TOPIC: &str = "cloudwatch/metric/put";

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
    /// The crate-private reserved-publish seam (UNS-CANONICAL-DESIGN §4.2): the
    /// `messaging` metric target publishes the reserved `metric` class through it.
    /// `None` outside the library runtime — selecting the `messaging` target then
    /// fails with a clear error.
    reserved: Option<Arc<dyn ReservedMessaging>>,
    /// The resolved runtime platform, retained so the metric target can be rebuilt with the same
    /// platform-profile default on config hot-reload (mirrors how logging/health thread it).
    platform: Platform,
}

impl MetricEmitter {
    /// Build an emitter from configuration, selecting the target.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub async fn new(config: &Config, messaging: Option<Arc<dyn MessagingService>>) -> Result<MetricEmitter>`
    /// - `messaging` is required by the `cloudwatchcomponent` target; it is retained
    ///   so the target can be rebuilt on config change. The `messaging` target
    ///   additionally requires the library runtime's privileged reserved-publish
    ///   seam (its topics are the reserved UNS `metric` class), so it is available
    ///   only through `EdgeCommonsBuilder::build()` — not this standalone constructor.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `EdgeCommonsError::Metrics` | A messaging/cloudwatchcomponent target lacks its messaging dependency, or `cloudwatch` selected without the feature | Build via the runtime / enable the `cloudwatch` feature |
    ///
    /// The `log` target is fail-soft: an unwritable `logFileName` does not error
    /// here (it warns and drops metrics on emit), matching the Java target.
    ///
    /// This convenience defaults the platform to [`Platform::Host`] (no profile metric-target
    /// default, so the effective target is `explicit config ▸ log`). The runtime builder uses
    /// [`Self::new_internal`] to thread the resolved platform + the reserved seam.
    pub async fn new(
        config: &Config,
        messaging: Option<Arc<dyn MessagingService>>,
    ) -> Result<MetricEmitter> {
        Self::new_internal(config, messaging, None, Platform::Host).await
    }

    /// Build an emitter from configuration for a specific resolved `platform`, applying the
    /// platform-profile metric-target default (FR-MET-1 / FR-RT-3): the effective target is
    /// `explicit metricEmission.target ▸ profile default (prometheus on KUBERNETES) ▸ log`. See
    /// [`resolve_effective_target`] for the precedence and the Rust `metrics-prometheus` feature gate.
    ///
    /// # Errors
    /// See [`Self::new`]; additionally, an explicit `target=prometheus` without the
    /// `metrics-prometheus` cargo feature is a [`EdgeCommonsError::Metrics`] (mirroring `cloudwatch`).
    pub async fn new_for_platform(
        config: &Config,
        messaging: Option<Arc<dyn MessagingService>>,
        platform: Platform,
    ) -> Result<MetricEmitter> {
        Self::new_internal(config, messaging, None, platform).await
    }

    /// The library-runtime constructor (§4.2): additionally threads the
    /// crate-private [`ReservedMessaging`] seam so the `messaging` metric target
    /// can publish the reserved UNS `metric` class.
    pub(crate) async fn new_internal(
        config: &Config,
        messaging: Option<Arc<dyn MessagingService>>,
        reserved: Option<Arc<dyn ReservedMessaging>>,
        platform: Platform,
    ) -> Result<MetricEmitter> {
        let target = build_target(config, messaging.clone(), reserved.clone(), platform).await?;
        Ok(MetricEmitter {
            target: Mutex::new(target),
            metrics: Mutex::new(HashMap::new()),
            messaging,
            reserved,
            platform,
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
            .map_err(|_| EdgeCommonsError::Metrics("metric target mutex poisoned".to_string()))?
            .clone())
    }
}

/// Resolve the EFFECTIVE metric target for `platform` (FR-MET-1, precedence FR-RT-3):
/// `explicit metricEmission.target ▸ platform-profile default ▸ "log"`.
///
/// The only platform-profile default today is `prometheus` on KUBERNETES. The Rust
/// `metrics-prometheus` feature gate is applied HERE for the *profile-default* path only: when the
/// k8s default would select `prometheus` but the feature is NOT compiled in, this gracefully falls
/// back to `"log"` with a warning so a feature-less k8s build still runs. An EXPLICIT
/// `target=prometheus` is returned as-is regardless of the feature — the target builder then turns
/// it into a clear error when the feature is absent (mirroring the `cloudwatch` feature behavior).
///
/// Returns a lowercased target token.
pub fn resolve_effective_target(metric_config: &MetricConfig, platform: Platform) -> String {
    if let Some(explicit) = metric_config.target.as_deref() {
        return explicit.to_ascii_lowercase();
    }
    match crate::platform::profile_metric_target(platform) {
        Some("prometheus") => {
            #[cfg(feature = "metrics-prometheus")]
            {
                "prometheus".to_string()
            }
            #[cfg(not(feature = "metrics-prometheus"))]
            {
                tracing::warn!(
                    platform = ?platform,
                    "platform default metric target 'prometheus' requires the 'metrics-prometheus' \
                     cargo feature; falling back to 'log'"
                );
                "log".to_string()
            }
        }
        Some(other) => other.to_ascii_lowercase(),
        None => "log".to_string(),
    }
}

/// The metric `log` file-path template, resolved with the HOST-aware precedence: explicit
/// `metricEmission.targetConfig.logFileName` config ▸ the platform-profile default (a local path on
/// HOST/KUBERNETES, which lack `/greengrass/v2/logs`) ▸ the library default. The returned template is
/// still run through [`resolve`] for `{ComponentFullName}` etc. by the caller.
fn log_path_template(metric_config: &MetricConfig, platform: Platform) -> String {
    metric_config.explicit_log_file_name().unwrap_or_else(|| {
        crate::platform::profile_metric_log_path(platform)
            .map(str::to_string)
            .unwrap_or_else(|| metric_config.log_file_name())
    })
}

/// Build the configured metric target for the resolved `platform`.
async fn build_target(
    config: &Config,
    messaging: Option<Arc<dyn MessagingService>>,
    reserved: Option<Arc<dyn ReservedMessaging>>,
    platform: Platform,
) -> Result<Arc<dyn MetricTarget>> {
    let metric_config = &config.parsed.metric_emission;
    let namespace = metric_config.namespace().to_string();
    let large_fleet = metric_config.large_fleet_workaround;
    let target_name = resolve_effective_target(metric_config, platform);

    let log_target = || -> Result<Arc<dyn MetricTarget>> {
        let path = resolve(config, &log_path_template(metric_config, platform));
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
            // UNS §4.3: the messaging target publishes the reserved `metric` class
            // on ecv1/{device}/{component}/metric/{name} (component scope, D-U28) — it
            // needs the crate-private reserved-publish seam (§4.2), wired by the runtime.
            let reserved = reserved.ok_or_else(|| {
                EdgeCommonsError::Metrics(
                    "metric target 'messaging' requires the library runtime's privileged \
                     reserved-publish seam (its topics are the reserved UNS 'metric' class); \
                     build via EdgeCommonsBuilder"
                        .to_string(),
                )
            })?;
            // Canonical "northbound" selects the configured northbound broker; everything else
            // (e.g. "ipc"/"local") is the local transport. Matches heartbeat destination handling.
            let dest = metric_config.destination();
            let iot_core = dest.eq_ignore_ascii_case("northbound");
            Arc::new(target::messaging::MessagingMetricTarget::new(
                reserved,
                iot_core,
                namespace,
                large_fleet,
                config.clone(),
            ))
        }
        "cloudwatchcomponent" => {
            let messaging = require_messaging(messaging, "cloudwatchcomponent")?;
            // D-U21: the external Greengrass CloudWatch-component contract topic is
            // unchanged (non-ecv1, guard-exempt); the topic override is removed.
            Arc::new(
                target::cloudwatch_component::CloudWatchComponentTarget::new(
                    messaging,
                    CLOUDWATCH_COMPONENT_TOPIC,
                    namespace,
                ),
            )
        }
        "cloudwatch" => {
            build_cloudwatch_target(
                config,
                &namespace,
                large_fleet,
                metric_config.interval_secs(),
            )
            .await?
        }
        "prometheus" => build_prometheus_target(config, &namespace)?,
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
        EdgeCommonsError::Metrics(format!(
            "metric target '{target}' requires a messaging service"
        ))
    })
}

/// Construct the CloudWatch target (feature `cloudwatch`), or error if disabled.
///
/// With `metrics-cloudwatch-durable` enabled and `targetConfig.cloudwatch.buffer.type == "durable"`
/// (the default), this builds the disk-backed store-and-forward
/// [`target::cloudwatch_durable::CloudWatchDurableTarget`]; otherwise (`type: memory`, or the
/// durable feature off) it builds the in-memory [`target::cloudwatch::CloudWatchTarget`].
#[allow(unused_variables)]
async fn build_cloudwatch_target(
    config: &Config,
    namespace: &str,
    large_fleet_workaround: bool,
    interval_secs: u64,
) -> Result<Arc<dyn MetricTarget>> {
    #[cfg(all(feature = "metrics-cloudwatch-durable", feature = "cloudwatch"))]
    {
        if config.parsed.metric_emission.buffer_type() == "durable" {
            return build_durable_cloudwatch_target(config, namespace, large_fleet_workaround);
        }
    }
    #[cfg(all(feature = "metrics-cloudwatch-durable", not(feature = "cloudwatch")))]
    {
        if config.parsed.metric_emission.buffer_type() == "durable" {
            return Err(EdgeCommonsError::Metrics(
                "durable CloudWatch buffer requires the 'cloudwatch' cargo feature (for \
                 PutMetricData); enable both 'metrics-cloudwatch-durable' and 'cloudwatch'"
                    .to_string(),
            ));
        }
    }
    #[cfg(feature = "cloudwatch")]
    {
        Ok(Arc::new(
            target::cloudwatch::CloudWatchTarget::new(
                namespace,
                large_fleet_workaround,
                interval_secs,
            )
            .await?,
        ))
    }
    #[cfg(not(feature = "cloudwatch"))]
    {
        Err(EdgeCommonsError::Metrics(
            "metric target 'cloudwatch' requires the 'cloudwatch' cargo feature".to_string(),
        ))
    }
}

/// Construct the pull-based `prometheus` target (feature `metrics-prometheus`), or error if the
/// feature is disabled (mirroring `cloudwatch`). Binds the `/metrics` HTTP server on the configured
/// port/path. The `metricEmission.namespace` becomes the gauge-name prefix (FR-MET-3).
#[allow(unused_variables)]
fn build_prometheus_target(config: &Config, namespace: &str) -> Result<Arc<dyn MetricTarget>> {
    #[cfg(feature = "metrics-prometheus")]
    {
        let mc = &config.parsed.metric_emission;
        let target = target::prometheus::PrometheusTarget::start(
            namespace,
            mc.prometheus_port(),
            &mc.prometheus_path(),
        )?;
        Ok(Arc::new(target))
    }
    #[cfg(not(feature = "metrics-prometheus"))]
    {
        Err(EdgeCommonsError::Metrics(
            "metric target 'prometheus' requires the 'metrics-prometheus' cargo feature"
                .to_string(),
        ))
    }
}

/// Build the durable (disk-backed store-and-forward) CloudWatch target: resolve the buffer path
/// template, map the `onFull`/`fsync` policies, and open the edgestreamlog buffer draining to a real
/// AWS `PutMetricData` sender.
#[cfg(all(feature = "metrics-cloudwatch-durable", feature = "cloudwatch"))]
fn build_durable_cloudwatch_target(
    config: &Config,
    namespace: &str,
    large_fleet_workaround: bool,
) -> Result<Arc<dyn MetricTarget>> {
    use edgestreamlog::config::{FsyncPolicy, OnFull};
    use target::cloudwatch_durable::{
        AwsPutMetricDataSender, CloudWatchDurableTarget, DurableBufferSettings,
    };

    let mc = &config.parsed.metric_emission;
    let on_full = match mc.buffer_on_full().as_str() {
        "block" => OnFull::Block,
        "rejectnew" | "reject_new" => OnFull::RejectNew,
        _ => OnFull::DropOldest,
    };
    let fsync = match mc.buffer_fsync().as_str() {
        "interval" => FsyncPolicy::Interval,
        "always" => FsyncPolicy::Always,
        _ => FsyncPolicy::PerBatch,
    };
    let settings = DurableBufferSettings {
        path: resolve(config, &mc.buffer_path()),
        max_disk_bytes: mc.buffer_max_disk_bytes(),
        on_full,
        fsync,
    };
    let target = CloudWatchDurableTarget::open(
        namespace,
        large_fleet_workaround,
        settings,
        AwsPutMetricDataSender::new,
    )?;
    Ok(Arc::new(target))
}

#[async_trait]
impl MetricService for MetricEmitter {
    fn define_metric(&self, metric: Metric) {
        if let Ok(mut map) = self.metrics.lock() {
            map.insert(metric.get_name().to_string(), metric);
        }
    }

    fn is_metric_defined(&self, name: &str) -> bool {
        self.metrics
            .lock()
            .map(|m| m.contains_key(name))
            .unwrap_or(false)
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
        match build_target(
            &config,
            self.messaging.clone(),
            self.reserved.clone(),
            self.platform,
        )
        .await
        {
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
        let dir = std::env::temp_dir().join(format!("edgecommons-emit-{}", uuid::Uuid::new_v4()));
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
        emitter.define_metric(
            MetricBuilder::create("requests")
                .add_measure("count", "Count", 60)
                .build(),
        );
        assert!(emitter.is_metric_defined("requests"));

        let mut values = HashMap::new();
        values.insert("count".to_string(), 2.0);
        emitter
            .emit_metric("requests", values.clone())
            .await
            .unwrap();
        // Undefined metric is ignored, not an error.
        emitter.emit_metric("nope", values).await.unwrap();
        emitter.flush_metrics().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn messaging_target_requires_the_reserved_seam() {
        // The messaging target publishes the reserved `metric` class; without the
        // runtime's privileged seam (§4.2) it is a clear error, even with a
        // messaging service present.
        let raw = json!({ "metricEmission": { "target": "messaging" } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let recorder = crate::testutil::RecordingMessaging::new();
        let result = MetricEmitter::new(&config, Some(recorder)).await;
        assert!(matches!(result, Err(EdgeCommonsError::Metrics(_))));
    }

    fn one_value() -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert("count".to_string(), 1.0);
        v
    }

    fn define(emitter: &MetricEmitter) {
        emitter.define_metric(
            MetricBuilder::create("m")
                .add_measure("count", "Count", 60)
                .build(),
        );
    }

    #[tokio::test]
    async fn messaging_target_builds_and_emits_on_the_uns_topic() {
        let raw = json!({ "metricEmission": { "target": "messaging", "targetConfig": { "destination": "ipc" } } });
        let config = Config::from_value("com.example.C", "t", raw).unwrap();
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new_internal(
            &config,
            Some(recorder.clone()),
            Some(recorder.clone()),
            Platform::Host,
        )
        .await
        .unwrap();
        define(&emitter);
        emitter.emit_metric("m", one_value()).await.unwrap();
        let published = recorder.reserved_local();
        assert_eq!(
            published.len(),
            1,
            "messaging target should publish EMF via the seam"
        );
        assert_eq!(published[0].0, "ecv1/t/C/metric/m");
    }

    #[tokio::test]
    async fn messaging_target_northbound_destination_routes_to_iot_core_api() {
        let raw = json!({ "metricEmission": { "target": "messaging", "targetConfig": { "destination": "northbound" } } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new_internal(
            &config,
            Some(recorder.clone()),
            Some(recorder.clone()),
            Platform::Host,
        )
        .await
        .unwrap();
        define(&emitter);
        emitter.emit_metric_now("m", one_value()).await.unwrap();
        assert_eq!(
            recorder.reserved_iot().len(),
            1,
            "northbound should publish to the second broker"
        );
        assert!(
            recorder.reserved_local().is_empty(),
            "northbound must not publish locally"
        );
    }

    #[tokio::test]
    async fn cloudwatchcomponent_target_builds_and_emits() {
        let raw = json!({ "metricEmission": { "target": "cloudwatchcomponent" } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new(&config, Some(recorder.clone()))
            .await
            .unwrap();
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
        assert!(matches!(result, Err(EdgeCommonsError::Metrics(_))));
        #[cfg(feature = "cloudwatch")]
        let _ = result; // with the feature it constructs (needs AWS env at emit time)
    }

    #[tokio::test]
    async fn unknown_target_defaults_to_log() {
        let dir = std::env::temp_dir().join(format!("edgecommons-unk-{}", uuid::Uuid::new_v4()));
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

        let dir = std::env::temp_dir().join(format!("edgecommons-recfg-{}", uuid::Uuid::new_v4()));
        let path = dir.join("m.log");
        let raw_log = json!({ "metricEmission": { "target": "log", "targetConfig": { "logFileName": path.to_string_lossy() } } });
        let config = Config::from_value("c", "t", raw_log).unwrap();
        // Build with messaging + the seam so the rebuilt messaging target can be created.
        let recorder = crate::testutil::RecordingMessaging::new();
        let emitter = MetricEmitter::new_internal(
            &config,
            Some(recorder.clone()),
            Some(recorder.clone()),
            Platform::Host,
        )
        .await
        .unwrap();
        define(&emitter);

        // Reconfigure to a messaging target.
        let raw_msg = json!({ "metricEmission": { "target": "messaging", "targetConfig": { "destination": "ipc" } } });
        let new_cfg = Arc::new(Config::from_value("c", "t", raw_msg).unwrap());
        assert!(
            emitter.on_configuration_change(new_cfg).await,
            "rebuild should succeed"
        );

        emitter.emit_metric_now("m", one_value()).await.unwrap();
        assert_eq!(
            recorder.reserved_local().len(),
            1,
            "metrics now flow to the messaging target"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- effective-target precedence (FR-MET-1 / FR-RT-3) ----------

    fn metric_config(raw: serde_json::Value) -> MetricConfig {
        Config::from_value("c", "t", raw)
            .unwrap()
            .parsed
            .metric_emission
    }

    #[test]
    fn host_and_greengrass_default_to_log() {
        // No explicit target + no profile metric default → library default "log" (unchanged).
        let mc = metric_config(json!({ "metricEmission": {} }));
        assert_eq!(resolve_effective_target(&mc, Platform::Host), "log");
        assert_eq!(resolve_effective_target(&mc, Platform::Greengrass), "log");
    }

    #[test]
    fn explicit_target_overrides_platform_default() {
        // Explicit config wins everywhere, including over the KUBERNETES prometheus default.
        let mc = metric_config(json!({ "metricEmission": { "target": "messaging" } }));
        assert_eq!(
            resolve_effective_target(&mc, Platform::Kubernetes),
            "messaging"
        );
        let mc_log = metric_config(json!({ "metricEmission": { "target": "log" } }));
        assert_eq!(
            resolve_effective_target(&mc_log, Platform::Kubernetes),
            "log"
        );
    }

    // ---------- HOST-aware metric-log path precedence ----------

    #[test]
    fn log_path_template_host_uses_local_default() {
        // No explicit logFileName + HOST/KUBERNETES → the local platform default (not /greengrass).
        let mc = metric_config(json!({ "metricEmission": { "target": "log" } }));
        assert_eq!(
            log_path_template(&mc, Platform::Host),
            crate::platform::METRIC_LOG_PATH_LOCAL
        );
        assert_eq!(
            log_path_template(&mc, Platform::Kubernetes),
            crate::platform::METRIC_LOG_PATH_LOCAL
        );
    }

    #[test]
    fn log_path_template_greengrass_uses_library_default() {
        // No explicit logFileName + GREENGRASS → the library default (the on-device Greengrass path).
        let mc = metric_config(json!({ "metricEmission": { "target": "log" } }));
        assert_eq!(
            log_path_template(&mc, Platform::Greengrass),
            "/greengrass/v2/logs/{ComponentFullName}.metric.log"
        );
    }

    #[test]
    fn log_path_template_explicit_wins_over_platform_default() {
        // An explicit logFileName wins on every platform, including HOST.
        let mc = metric_config(
            json!({ "metricEmission": { "target": "log", "targetConfig": { "logFileName": "/custom/x.log" } } }),
        );
        assert_eq!(log_path_template(&mc, Platform::Host), "/custom/x.log");
        assert_eq!(
            log_path_template(&mc, Platform::Greengrass),
            "/custom/x.log"
        );
    }

    #[test]
    fn explicit_prometheus_is_returned_as_is_regardless_of_feature() {
        // The feature gate for the EXPLICIT path is applied at build time (a clear error without the
        // feature), not in the precedence resolver — so the token is returned verbatim here.
        let mc = metric_config(json!({ "metricEmission": { "target": "PROMETHEUS" } }));
        assert_eq!(resolve_effective_target(&mc, Platform::Host), "prometheus");
    }

    #[cfg(feature = "metrics-prometheus")]
    #[test]
    fn kubernetes_default_selects_prometheus_with_feature() {
        let mc = metric_config(json!({ "metricEmission": {} }));
        assert_eq!(
            resolve_effective_target(&mc, Platform::Kubernetes),
            "prometheus"
        );
    }

    #[cfg(not(feature = "metrics-prometheus"))]
    #[test]
    fn kubernetes_default_falls_back_to_log_without_feature() {
        // Feature-less k8s build must still run: the prometheus profile default gracefully degrades
        // to "log" (with a warning) rather than failing.
        let mc = metric_config(json!({ "metricEmission": {} }));
        assert_eq!(resolve_effective_target(&mc, Platform::Kubernetes), "log");
    }

    #[cfg(not(feature = "metrics-prometheus"))]
    #[tokio::test]
    async fn explicit_prometheus_without_feature_is_error() {
        // An EXPLICIT target=prometheus without the cargo feature is a clear error (mirrors the
        // cloudwatch-without-feature behavior), on any platform.
        let raw = json!({ "metricEmission": { "target": "prometheus" } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let result = MetricEmitter::new_for_platform(&config, None, Platform::Host).await;
        assert!(matches!(result, Err(EdgeCommonsError::Metrics(_))));
    }

    #[cfg(not(feature = "metrics-prometheus"))]
    #[tokio::test]
    async fn kubernetes_default_builds_without_feature_via_log_fallback() {
        // No explicit target on KUBERNETES, feature off → builds the log target (fallback), not an
        // error. Use a writable temp log path so the (fail-soft) log target has somewhere to go.
        let dir = std::env::temp_dir().join(format!("edgecommons-k8sfb-{}", uuid::Uuid::new_v4()));
        let path = dir.join("m.log");
        let raw = json!({ "metricEmission": { "targetConfig": { "logFileName": path.to_string_lossy() } } });
        let config = Config::from_value("c", "t", raw).unwrap();
        let emitter = MetricEmitter::new_for_platform(&config, None, Platform::Kubernetes)
            .await
            .expect("k8s should build the log fallback without the feature");
        define(&emitter);
        emitter.emit_metric_now("m", one_value()).await.unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().lines().count() >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "metrics-prometheus")]
    #[tokio::test]
    async fn kubernetes_default_builds_and_serves_prometheus_with_feature() {
        use std::io::{Read, Write};
        use std::net::{SocketAddr, TcpListener, TcpStream};

        // Pick a free port (bind :0, read it, release) so the build-path server has a known address.
        let port = {
            let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            l.local_addr().unwrap().port()
        };
        let raw = json!({ "metricEmission": { "targetConfig": { "port": port } } });
        let config = Config::from_value("com.example.C", "thing-1", raw).unwrap();
        // No explicit target + KUBERNETES profile → effective target is prometheus.
        let emitter = MetricEmitter::new_for_platform(&config, None, Platform::Kubernetes)
            .await
            .expect("k8s should build the prometheus target with the feature");

        emitter.define_metric(
            MetricBuilder::create("requests")
                .add_measure("count", "Count", 60)
                .build(),
        );
        emitter.emit_metric("requests", one_value()).await.unwrap();
        // flush is a no-op for prometheus (pull); it must not error.
        emitter.flush_metrics().await.unwrap();

        // Scrape the /metrics endpoint and confirm the gauge is present (selection end-to-end).
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect(addr).expect("connect to prometheus build-path server");
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(
            response.contains("200 OK"),
            "expected 200, got:\n{response}"
        );
        assert!(
            response.contains("edgecommons_count"),
            "missing gauge in:\n{response}"
        );

        // close() stops the listener.
        emitter.shutdown().await;
    }

    #[tokio::test]
    async fn emitting_undefined_metric_is_ignored() {
        let dir = std::env::temp_dir().join(format!("edgecommons-undef-{}", uuid::Uuid::new_v4()));
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
