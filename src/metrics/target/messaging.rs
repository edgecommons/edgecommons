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
//! - The EMF object is wrapped in a [`Message`] envelope (`name = "Metric"`,
//!   `version = "1.0"`, body = EMF, tags = thing name + configured tags) and sent via
//!   `publish` / `publish_to_iot_core`, matching the Java `Messaging` metric target
//!   (`MessageBuilder.create("Metric","1.0").withPayload(emf).withConfig(...)`).
//! - Error handling: [`crate::error::Result`].
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::metrics::emf`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::MetricTarget;
use crate::error::Result;
use crate::messaging::message::MessageBuilder;
use crate::messaging::{MessagingService, Qos};
use crate::metrics::emf::build_emf_variants;
use crate::metrics::metric::Metric;

/// Publishes EMF metrics over messaging, wrapped in a `Metric` message envelope.
pub struct MessagingMetricTarget {
    messaging: Arc<dyn MessagingService>,
    topic: String,
    iot_core: bool,
    namespace: String,
    large_fleet_workaround: bool,
    /// Thing name carried in the message envelope's tags.
    thing_name: String,
    /// Configured tags carried in the message envelope.
    tags: BTreeMap<String, Value>,
}

impl MessagingMetricTarget {
    /// Create the target. `iot_core` selects AWS IoT Core over the local broker.
    /// `thing_name` and `tags` populate the message envelope (mirroring `withConfig`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        messaging: Arc<dyn MessagingService>,
        topic: impl Into<String>,
        iot_core: bool,
        namespace: impl Into<String>,
        large_fleet_workaround: bool,
        thing_name: impl Into<String>,
        tags: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            messaging,
            topic: topic.into(),
            iot_core,
            namespace: namespace.into(),
            large_fleet_workaround,
            thing_name: thing_name.into(),
            tags,
        }
    }

    async fn publish(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        // large_fleet_workaround emits both the normal and the coreName="ALL" record.
        for emf in build_emf_variants(&self.namespace, metric, values, self.large_fleet_workaround) {
            let mut builder = MessageBuilder::new("Metric", "1.0")
                .payload(emf)
                .thing_name(self.thing_name.clone());
            for (key, value) in &self.tags {
                builder = builder.tag(key.clone(), value.clone());
            }
            let message = builder.build();
            if self.iot_core {
                self.messaging
                    .publish_to_iot_core(&self.topic, &message, Qos::AtLeastOnce)
                    .await?;
            } else {
                self.messaging.publish(&self.topic, &message).await?;
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

    fn target(recorder: Arc<RecordingMessaging>, iot_core: bool, large_fleet: bool) -> MessagingMetricTarget {
        let mut tags = BTreeMap::new();
        tags.insert("site".to_string(), serde_json::json!("factory-1"));
        MessagingMetricTarget::new(recorder, "m/topic", iot_core, "demo", large_fleet, "thing-1", tags)
    }

    #[tokio::test]
    async fn emits_enveloped_metric_to_local_broker() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), false, false);
        t.emit(&metric(), &values()).await.unwrap();

        assert!(recorder.iot().is_empty());
        let local = recorder.local();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].0, "m/topic");
        // EMF is carried in the envelope BODY (not raw); envelope is a "Metric" message.
        let msg = &local[0].1;
        assert!(!msg.is_raw());
        assert_eq!(msg.header.name, "Metric");
        assert_eq!(msg.header.version, "1.0");
        assert!(msg.body.get("_aws").is_some(), "EMF body present");
        // Tags carry the thing name and configured tags.
        assert_eq!(msg.tags.thing_name, "thing-1");
        assert_eq!(msg.tags.extra.get("site"), Some(&serde_json::json!("factory-1")));
    }

    #[tokio::test]
    async fn emits_to_iot_core_when_selected() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), true, false);
        t.emit_now(&metric(), &values()).await.unwrap();

        assert!(recorder.local().is_empty());
        assert_eq!(recorder.iot().len(), 1);
        assert_eq!(recorder.iot()[0].1.header.name, "Metric");
    }

    #[tokio::test]
    async fn large_fleet_workaround_emits_two_variants() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), false, true);
        t.emit(&metric(), &values()).await.unwrap();

        // Normal record + the coreName="ALL" record.
        assert_eq!(recorder.local().len(), 2);
    }
}
