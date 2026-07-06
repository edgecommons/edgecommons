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
//! - **Wire format** (matching Java `CloudWatchComponent` / Python
//!   `cloudwatch_component`): one **raw** message is published *per measure*, shaped
//!   as the Greengrass CloudWatch Metrics component's `PutMetricData` contract:
//!   ```json
//!   { "request": { "namespace": "<ns>",
//!                  "metricData": { "metricName": "<measure>",
//!                                  "timestamp": <epoch-seconds>,
//!                                  "value": <number>,
//!                                  "unit": "<unit>",
//!                                  "dimensions": [ { "name": "<k>", "value": "<v>" } ] } } }
//!   ```
//!   `timestamp` is in **seconds** (the component's PutMetricData contract — distinct
//!   from EMF's millisecond `_aws.Timestamp`). Dimensions **exclude** `coreName`
//!   (the component supplies it implicitly), matching `dimensionsAsJson(false)`.
//! - Does **not** honor `largeFleetWorkaround` (the component owns the `coreName`
//!   dimension), matching Java.
//! - Error handling: [`crate::error::Result`].
//!
//! ## Related Modules
//! - [`crate::messaging`], [`crate::metrics::metric`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::MetricTarget;
use crate::error::Result;
use crate::messaging::MessagingService;
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
        let timestamp = now_secs();
        let dimensions = dimensions_array(metric);
        // One message per measure, mirroring Java/Python.
        for (measure_name, value) in values {
            let unit = metric
                .get_measure(measure_name)
                .map(|m| m.get_unit())
                .unwrap_or("None");
            let payload = json!({
                "request": {
                    "namespace": self.namespace,
                    "metricData": {
                        "metricName": measure_name,
                        "timestamp": timestamp,
                        "value": value,
                        "unit": unit,
                        "dimensions": dimensions,
                    }
                }
            });
            self.messaging.publish_raw(&self.topic, &payload).await?;
        }
        Ok(())
    }
}

/// Build the `dimensions` array (`[{ "name", "value" }, ...]`) excluding `coreName`,
/// matching Java's `dimensionsAsJson(false)` / Python's `dimensions_as_json(include_core_name=False)`.
fn dimensions_array(metric: &Metric) -> Value {
    let dims: Vec<Value> = metric
        .get_dimensions()
        .iter()
        .filter(|(key, _)| key.as_str() != "coreName")
        .map(|(key, value)| json!({ "name": key, "value": value }))
        .collect();
    Value::Array(dims)
}

/// Seconds since the Unix epoch (the CloudWatch Metrics component's PutMetricData
/// timestamp unit; `0` if the clock is before the epoch).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

    #[tokio::test]
    async fn publishes_putmetricdata_request_per_measure() {
        let recorder = RecordingMessaging::new();
        let target =
            CloudWatchComponentTarget::new(recorder.clone(), "cloudwatch/metric/put", "demo");
        // Two measures → two published messages (one per measure).
        let metric = MetricBuilder::create("requests")
            .with_thing_name("thing-1")
            .add_measure("count", "Count", 60)
            .add_measure("latency", "Milliseconds", 60)
            .build();
        let mut vals = HashMap::new();
        vals.insert("count".to_string(), 3.0);
        vals.insert("latency".to_string(), 12.0);

        target.emit(&metric, &vals).await.unwrap();

        let published = recorder.local();
        assert_eq!(published.len(), 2, "one message per measure");
        assert_eq!(published[0].0, "cloudwatch/metric/put");

        // The raw payload follows the {request:{namespace, metricData}} contract.
        let req = &published[0].1.get_raw().expect("raw payload")["request"];
        assert_eq!(req["namespace"], "demo");
        let md = &req["metricData"];
        assert!(md.get("metricName").is_some());
        assert!(md.get("value").is_some());
        assert!(md.get("unit").is_some());
        // timestamp is epoch SECONDS (~1.7e9 in 2026), not milliseconds.
        let ts = md["timestamp"].as_u64().unwrap();
        assert!(
            ts > 1_000_000_000 && ts < 100_000_000_000,
            "timestamp must be seconds, got {ts}"
        );
        // dimensions is an array of {name,value} that EXCLUDES coreName.
        let dims = md["dimensions"].as_array().unwrap();
        assert!(
            dims.iter().all(|d| d["name"] != "coreName"),
            "coreName must be excluded"
        );
        assert!(
            dims.iter().any(|d| d["name"] == "category"),
            "category dimension present"
        );

        // Also exercises the default no-op flush/shutdown trait methods.
        target.emit_now(&metric, &vals).await.unwrap();
        target.flush().await.unwrap();
        target.shutdown().await;
    }
}
