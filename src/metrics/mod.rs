//! Metrics subsystem (Phase 1).
//!
//! `MetricService` + `Metric`/`Measure`/`MetricBuilder`, with pluggable targets
//! (`log`, `cloudwatch`, `cloudwatchcomponent`, `messaging`) behind a
//! `MetricTarget` trait. EMF is emitted with a millisecond `_aws.Timestamp`, the
//! ≤10-dimension cap is enforced on `Metric` itself, and `is_metric_defined` is a
//! pure lookup — fixing the corresponding Java defects.
//!
//! Implementations land in Phase 1.
