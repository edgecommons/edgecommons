//! # Metrics target — cloudwatch component
//!
//! **One-liner purpose**: Publish metrics to the Greengrass CloudWatch Metrics
//! component over messaging.
//!
//! ## Overview
//! Mirrors the Java/Python `cloudwatchcomponent` target. It publishes to the
//! configured topic (default `cloudwatch/metric/put`) on the local bus, where the
//! AWS-provided CloudWatch Metrics component picks the data up.
//!
//! ## Semantics & Architecture
//! - `emit` and `emit_now` both publish immediately.
//! - The published payload is the EMF object. **Provisional**: the exact payload
//!   contract of the Greengrass CloudWatch Metrics component should be validated
//!   when Greengrass (Phase 2) support lands; adjust the payload shape then.
//! - Error handling: [`crate::error::Result`].
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::metrics::emf`].

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::MetricTarget;
use crate::error::Result;
use crate::messaging::MessagingService;
use crate::metrics::emf::build_emf;
use crate::metrics::metric::Metric;

/// Publishes metrics to the Greengrass CloudWatch Metrics component topic.
///
/// Note: this target does **not** honor `largeFleetWorkaround` (matching the Java
/// implementation — the component sets `coreName` itself).
pub struct CloudWatchComponentTarget {
    messaging: Arc<dyn MessagingService>,
    topic: String,
    namespace: String,
}

impl CloudWatchComponentTarget {
    pub fn new(
        messaging: Arc<dyn MessagingService>,
        topic: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Self {
        Self {
            messaging,
            topic: topic.into(),
            namespace: namespace.into(),
        }
    }

    async fn publish(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let emf = build_emf(&self.namespace, metric, values, false);
        self.messaging.publish_raw(&self.topic, &emf).await
    }
}

#[async_trait]
impl MetricTarget for CloudWatchComponentTarget {
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
        v.insert("count".to_string(), 3.0);
        v
    }

    #[tokio::test]
    async fn publishes_emf_to_the_component_topic() {
        let recorder = RecordingMessaging::new();
        let target = CloudWatchComponentTarget::new(recorder.clone(), "cloudwatch/metric/put", "demo");
        let metric = MetricBuilder::create("requests").add_measure("count", "Count", 60).build();

        target.emit(&metric, &values()).await.unwrap();
        target.emit_now(&metric, &values()).await.unwrap();

        let published = recorder.local();
        assert_eq!(published.len(), 2);
        assert_eq!(published[0].0, "cloudwatch/metric/put");
        // The published body is the EMF object (carries the `_aws` metadata key).
        assert!(published[0].1.body.get("_aws").is_some(), "payload should be EMF");
        // This target also exercises the default no-op flush/shutdown trait methods.
        target.flush().await.unwrap();
        target.shutdown().await;
    }
}
