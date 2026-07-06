//! # Metrics target — messaging
//!
//! **One-liner purpose**: Publish EMF metrics to the library-owned UNS metric topic
//! `ecv1[/{site}]/{device}/{component}/main/metric/{metricName}` through the
//! privileged reserved-publish seam (UNS-CANONICAL-DESIGN §4.3).
//!
//! ## Overview
//! Mirrors the Java canonical `Messaging` metric target. The topic is minted per
//! metric from the component's resolved UNS identity — the metric name passes the
//! template sanitizer to become the channel token (§2.2) — and the destination
//! comes from `metricEmission.targetConfig.destination` (`ipc`/`local` or
//! `northbound`, D-U9). The legacy `targetConfig.topic` override is removed
//! — hard cut.
//!
//! ## Semantics & Architecture
//! - `emit` and `emit_now` both publish immediately (no batching).
//! - The EMF object is wrapped in a [`Message`](crate::messaging::Message) envelope
//!   (`name = "Metric"`, `version = "1.0"`, body = EMF, identity + tags stamped
//!   from the config — mirroring Java's `withConfig`).
//! - The `metric` class is reserved (§4.1), so publishes go through the
//!   crate-private [`ReservedMessaging`] seam (§4.2) — the target's constructor is
//!   `pub(crate)` and only the library runtime can build it.
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::metrics::emf`], [`crate::uns`].

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::MetricTarget;
use crate::config::model::Config;
use crate::config::template::sanitize;
use crate::error::Result;
use crate::messaging::message::MessageBuilder;
use crate::messaging::{Qos, ReservedMessaging};
use crate::metrics::emf::build_emf_variants;
use crate::metrics::metric::Metric;
use crate::uns::{Uns, UnsClass};

/// Publishes EMF metrics to the UNS metric topic through the reserved-publish seam.
pub struct MessagingMetricTarget {
    reserved: Arc<dyn ReservedMessaging>,
    iot_core: bool,
    namespace: String,
    large_fleet_workaround: bool,
    /// The configuration snapshot the target was built from: supplies the resolved
    /// identity + effective root for topic minting and the identity/tags stamped
    /// into each envelope (rebuilt on config hot-reload by the emitter).
    config: Config,
}

impl MessagingMetricTarget {
    /// Create the target (crate-private — the `metric` class is reserved, §4.2).
    /// `true` selects the northbound broker over the local broker.
    pub(crate) fn new(
        reserved: Arc<dyn ReservedMessaging>,
        iot_core: bool,
        namespace: impl Into<String>,
        large_fleet_workaround: bool,
        config: Config,
    ) -> Self {
        Self {
            reserved,
            iot_core,
            namespace: namespace.into(),
            large_fleet_workaround,
            config,
        }
    }

    /// The metric's UNS topic —
    /// `ecv1[/{site}]/{device}/{component}/main/metric/{name}` with the metric name
    /// passed through the template sanitizer (the §2.2 channel-token rule).
    fn metric_topic(&self, metric: &Metric) -> Result<String> {
        // The RAW includeRoot flag (Java parity): Uns applies D-U25 internally.
        Uns::new(
            self.config.identity().clone(),
            self.config.topic_include_root(),
        )
        .topic_with_channel(UnsClass::Metric, &sanitize(metric.get_name()))
    }

    async fn publish(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let topic = self.metric_topic(metric)?;
        // large_fleet_workaround emits both the normal and the coreName="ALL" record.
        for emf in build_emf_variants(&self.namespace, metric, values, self.large_fleet_workaround)
        {
            let message = MessageBuilder::new("Metric", "1.0")
                .payload(emf)
                .from_config(&self.config)
                .build();
            // The metric class is reserved (§4.1) — publish through the seam (§4.2).
            if self.iot_core {
                self.reserved
                    .publish_reserved_northbound(&topic, &message, Qos::AtLeastOnce)
                    .await?;
            } else {
                self.reserved.publish_reserved(&topic, &message).await?;
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
    use serde_json::json;

    fn values() -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert("count".to_string(), 1.0);
        v
    }

    fn metric(name: &str) -> Metric {
        MetricBuilder::create(name)
            .add_measure("count", "Count", 60)
            .build()
    }

    fn config() -> Config {
        Config::from_value(
            "com.example.MyComp",
            "thing-1",
            json!({ "tags": { "site": "factory-1" } }),
        )
        .unwrap()
    }

    fn target(
        recorder: Arc<RecordingMessaging>,
        iot_core: bool,
        large_fleet: bool,
    ) -> MessagingMetricTarget {
        MessagingMetricTarget::new(recorder, iot_core, "demo", large_fleet, config())
    }

    #[tokio::test]
    async fn emits_enveloped_metric_on_the_uns_topic_via_the_seam() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), false, false);
        t.emit(&metric("requests"), &values()).await.unwrap();

        assert!(recorder.reserved_iot().is_empty());
        assert!(
            recorder.local().is_empty(),
            "must use the SEAM, not publish()"
        );
        let published = recorder.reserved_local();
        assert_eq!(published.len(), 1);
        let (topic, msg) = &published[0];
        assert_eq!(topic, "ecv1/thing-1/MyComp/main/metric/requests");
        // EMF is carried in the envelope BODY (not raw); envelope is a "Metric" message.
        assert!(!msg.is_raw());
        assert_eq!(msg.header.name, "Metric");
        assert_eq!(msg.header.version, "1.0");
        assert!(msg.body.get("_aws").is_some(), "EMF body present");
        // Identity + tags are stamped from the config (withConfig parity).
        let identity = msg.identity.as_ref().expect("identity stamped");
        assert_eq!(identity.device(), "thing-1");
        assert_eq!(identity.instance(), "main");
        let tags = msg.tags.as_ref().expect("tags stamped");
        assert_eq!(tags.extra.get("site"), Some(&json!("factory-1")));
    }

    #[tokio::test]
    async fn metric_name_is_sanitized_into_the_channel_token() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), false, false);
        t.emit(&metric("req/rate+p99"), &values()).await.unwrap();
        assert_eq!(
            recorder.reserved_local()[0].0,
            "ecv1/thing-1/MyComp/main/metric/req_rate_p99"
        );
    }

    #[tokio::test]
    async fn emits_to_iot_core_when_selected() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), true, false);
        t.emit_now(&metric("requests"), &values()).await.unwrap();

        assert!(recorder.reserved_local().is_empty());
        assert_eq!(recorder.reserved_iot().len(), 1);
        assert_eq!(recorder.reserved_iot()[0].1.header.name, "Metric");
    }

    #[tokio::test]
    async fn large_fleet_workaround_emits_two_variants() {
        let recorder = RecordingMessaging::new();
        let t = target(recorder.clone(), false, true);
        t.emit(&metric("requests"), &values()).await.unwrap();

        // Normal record + the coreName="ALL" record, on the same topic.
        let published = recorder.reserved_local();
        assert_eq!(published.len(), 2);
        assert_eq!(published[0].0, published[1].0);
    }
}
