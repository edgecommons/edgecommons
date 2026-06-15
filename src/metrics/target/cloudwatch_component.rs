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
pub struct CloudWatchComponentTarget {
    messaging: Arc<dyn MessagingService>,
    topic: String,
    namespace: String,
    large_fleet_workaround: bool,
}

impl CloudWatchComponentTarget {
    pub fn new(
        messaging: Arc<dyn MessagingService>,
        topic: impl Into<String>,
        namespace: impl Into<String>,
        large_fleet_workaround: bool,
    ) -> Self {
        Self {
            messaging,
            topic: topic.into(),
            namespace: namespace.into(),
            large_fleet_workaround,
        }
    }

    async fn publish(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let emf = build_emf(&self.namespace, metric, values, self.large_fleet_workaround);
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
