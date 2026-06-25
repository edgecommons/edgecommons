//! # Metrics — targets
//!
//! **One-liner purpose**: The [`MetricTarget`] trait and its implementations
//! (where metrics are sent), mirroring the Java/Python target set.
//!
//! ## Overview
//! - [`log`]: append EMF JSON lines to a file.
//! - [`messaging`]: publish EMF over the messaging service (local or IoT Core).
//! - [`cloudwatch_component`]: publish to the Greengrass CloudWatch Metrics
//!   component topic.
//! - `cloudwatch` (feature `cloudwatch`): send to CloudWatch via the AWS SDK.
//!
//! ## Semantics & Architecture
//! - Async (`tokio`); object-safe via `async_trait`.
//! - `emit` is the buffered path and `emit_now` the immediate path; for targets
//!   without batching (log, messaging, cloudwatch_component) both behave the same.
//! - Error handling: [`crate::error::Result`].
//!
//! ## Related Modules
//! - [`crate::metrics`], [`crate::metrics::emf`].

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::Result;
use crate::metrics::metric::Metric;

pub mod cloudwatch_component;
pub mod log;
pub mod messaging;

#[cfg(feature = "cloudwatch")]
pub mod cloudwatch;

#[cfg(feature = "metrics-cloudwatch-durable")]
pub mod cloudwatch_durable;

/// A destination for emitted metrics.
#[async_trait]
pub trait MetricTarget: Send + Sync {
    /// Emit (buffered where the target batches) the given measure values.
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()>;

    /// Emit immediately, bypassing any batching.
    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()>;

    /// Flush any buffered metrics. Default: no-op (targets without batching).
    async fn flush(&self) -> Result<()> {
        Ok(())
    }

    /// Release resources / final flush. Default: no-op.
    async fn shutdown(&self) {}
}
