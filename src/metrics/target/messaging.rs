//! # Metrics target — messaging
//!
//! **One-liner purpose**: Publish EMF metrics over the messaging service (local
//! broker or AWS IoT Core).
//!
//! ## Overview
//! Mirrors the Java/Python `messaging` metric target. The topic comes from
//! `metricEmission.targetConfig.topic` (template-resolved by the emitter) and the
//! destination from `targetConfig.destination` (`ipc`/`local` or `iotcore`).
//!
//! ## Semantics & Architecture
//! - `emit` and `emit_now` both publish immediately (no batching).
//! - Uses `publish_raw` / `publish_to_iot_core_raw` since EMF is raw JSON.
//! - Error handling: [`crate::error::Result`].
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::metrics::emf`].

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::MetricTarget;
use crate::error::Result;
use crate::messaging::{MessagingService, Qos};
use crate::metrics::emf::build_emf_variants;
use crate::metrics::metric::Metric;

/// Publishes EMF metrics over messaging.
pub struct MessagingMetricTarget {
    messaging: Arc<dyn MessagingService>,
    topic: String,
    iot_core: bool,
    namespace: String,
    large_fleet_workaround: bool,
}

impl MessagingMetricTarget {
    /// Create the target. `iot_core` selects AWS IoT Core over the local broker.
    pub fn new(
        messaging: Arc<dyn MessagingService>,
        topic: impl Into<String>,
        iot_core: bool,
        namespace: impl Into<String>,
        large_fleet_workaround: bool,
    ) -> Self {
        Self {
            messaging,
            topic: topic.into(),
            iot_core,
            namespace: namespace.into(),
            large_fleet_workaround,
        }
    }

    async fn publish(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        // large_fleet_workaround emits both the normal and the coreName="ALL" record.
        for emf in build_emf_variants(&self.namespace, metric, values, self.large_fleet_workaround) {
            if self.iot_core {
                self.messaging
                    .publish_to_iot_core_raw(&self.topic, &emf, Qos::AtLeastOnce)
                    .await?;
            } else {
                self.messaging.publish_raw(&self.topic, &emf).await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl MetricTarget for MessagingMetricTarget {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.publish(metric, values).await
    }

    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.publish(metric, values).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricBuilder;
    use crate::testutil::RecordingMessaging;

    fn values() -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert("count".to_string(), 1.0);
        v
    }

    fn metric() -> Metric {
        MetricBuilder::create("requests").add_measure("count", "Count", 60).build()
    }

    #[tokio::test]
    async fn emits_to_local_broker() {
        let recorder = RecordingMessaging::new();
        let target = MessagingMetricTarget::new(recorder.clone(), "m/topic", false, "demo", false);
        target.emit(&metric(), &values()).await.unwrap();

        assert!(recorder.iot().is_empty());
        let local = recorder.local();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].0, "m/topic");
        assert!(local[0].1.body.get("_aws").is_some());
    }

    #[tokio::test]
    async fn emits_to_iot_core_when_selected() {
        let recorder = RecordingMessaging::new();
        let target = MessagingMetricTarget::new(recorder.clone(), "m/topic", true, "demo", false);
        target.emit_now(&metric(), &values()).await.unwrap();

        assert!(recorder.local().is_empty());
        assert_eq!(recorder.iot().len(), 1);
    }

    #[tokio::test]
    async fn large_fleet_workaround_emits_two_variants() {
        let recorder = RecordingMessaging::new();
        let target = MessagingMetricTarget::new(recorder.clone(), "m/topic", false, "demo", true);
        target.emit(&metric(), &values()).await.unwrap();

        // Normal record + the coreName="ALL" record.
        assert_eq!(recorder.local().len(), 2);
    }
}
