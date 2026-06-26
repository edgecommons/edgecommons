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
//! - `prometheus` (feature `metrics-prometheus`): a **pull-based** target — an in-process
//!   registry served as OpenMetrics/Prometheus text at an HTTP `/metrics` endpoint; the
//!   default on KUBERNETES.
//!
//! ## Semantics & Architecture
//! - Async (`tokio`); object-safe via `async_trait`.
//! - **Push targets** (`log`, `messaging`, `cloudwatch_component`, `cloudwatch`): `emit` is the
//!   buffered path and `emit_now` the immediate path; for the non-batching targets both behave the
//!   same; [`MetricTarget::flush`] delivers buffered data and [`MetricTarget::shutdown`] does a
//!   final flush.
//! - **Pull target** (`prometheus`) — INVERTED lifecycle (FR-MET-2): `emit`/`emit_now` only
//!   *update the in-process registry* (latest-value gauges); they push nothing. `flush` is a
//!   delivery no-op (the Prometheus server scrapes/pulls). `shutdown` stops the HTTP listener
//!   (releasing the port/thread). This inversion applies ONLY to the prometheus target; every
//!   other target keeps its push semantics unchanged.
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

#[cfg(feature = "metrics-prometheus")]
pub mod prometheus;

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
