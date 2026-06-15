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
use crate::metrics::emf::build_emf;
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
        let emf = build_emf(&self.namespace, metric, values, self.large_fleet_workaround);
        if self.iot_core {
            self.messaging
                .publish_to_iot_core_raw(&self.topic, &emf, Qos::AtLeastOnce)
                .await
        } else {
            self.messaging.publish_raw(&self.topic, &emf).await
        }
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
