//! # Metrics
//!
//! **One-liner purpose**: Define and emit metrics through pluggable targets.
//!
//! ## Overview
//! `MetricService` + `Metric`/`Measure`/`MetricBuilder`, with pluggable targets
//! (`log`, `cloudwatch`, `cloudwatchcomponent`, `messaging`) behind a
//! `MetricTarget` trait.
//!
//! ## Semantics & Architecture
//! - EMF is emitted with a millisecond `_aws.Timestamp`, the ≤10-dimension cap is
//!   enforced on `Metric` itself, and `is_metric_defined` is a pure lookup —
//!   fixing the corresponding Java defects.
//! - Error handling: [`crate::error::Result`]; emission failures are isolated per
//!   target.
//!
//! ## Status
//! Stub — implementations land in a later Phase 1 sub-step.
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::heartbeat`].
