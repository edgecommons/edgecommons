//! # Metrics — Embedded Metric Format (EMF)
//!
//! **One-liner purpose**: Build CloudWatch Embedded Metric Format JSON for a
//! metric and its measure values.
//!
//! ## Overview
//! EMF lets CloudWatch extract metrics from structured log/messages. The produced
//! object carries dimension values and measure values at the top level plus an
//! `_aws` metadata block describing the namespace, dimension set, and measures.
//!
//! ## Semantics & Architecture
//! - Pure function over a [`Metric`] and values; no I/O, no async.
//! - **Correctness**: `_aws.Timestamp` is emitted in **milliseconds since the Unix
//!   epoch**, as required by the official CloudWatch Embedded Metric Format
//!   specification ("Values MUST be expressed as the number of milliseconds after
//!   Jan 1, 1970 00:00:00 UTC"). The Java target divides by 1000 (seconds), which
//!   deviates from the spec; Rust follows the spec (as does Python).
//! - `large_fleet_workaround` emits the `coreName` dimension value as `"ALL"`.
//! - Error handling: infallible.
//!
//! ## Usage Example
//! ```
//! use ggcommons::metrics::metric::MetricBuilder;
//! use ggcommons::metrics::emf::build_emf;
//! use std::collections::HashMap;
//!
//! let metric = MetricBuilder::create("requests").add_measure("count", "Count", 60).build();
//! let mut values = HashMap::new();
//! values.insert("count".to_string(), 5.0);
//! let emf = build_emf("MyApp", &metric, &values, false);
//! assert_eq!(emf["count"], 5.0);
//! assert!(emf["_aws"]["Timestamp"].as_u64().unwrap() > 1_000_000_000_000);
//! ```
//!
//! ## Related Modules
//! - [`crate::metrics::metric`], [`crate::metrics::target`].

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::metrics::metric::Metric;

/// Build an EMF JSON object for `metric` with the given `measure_values`.
///
/// # Semantics & Syntax
/// - **Signature**: `pub fn build_emf(namespace, metric, measure_values, large_fleet_workaround) -> Value`
/// - Borrows its inputs; returns an owned [`Value`].
///
/// # Algorithmic Choices
/// Flattens dimensions and measure values to the top level and attaches the
/// `_aws` metadata block (single dimension set = all dimension keys), matching the
/// Java/Python EMF layout. `_aws.Timestamp` is in **milliseconds** per the official
/// EMF specification.
pub fn build_emf(
    namespace: &str,
    metric: &Metric,
    measure_values: &HashMap<String, f64>,
    large_fleet_workaround: bool,
) -> Value {
    let mut root = Map::new();

    // Dimension values at top level.
    for (key, value) in metric.get_dimensions() {
        let v = if large_fleet_workaround && key == "coreName" {
            "ALL".to_string()
        } else {
            value.clone()
        };
        root.insert(key.clone(), Value::String(v));
    }

    // Measure values at top level.
    for (key, value) in measure_values {
        root.insert(key.clone(), json!(value));
    }

    root.insert("_aws".to_string(), metrics_metadata(namespace, metric));
    Value::Object(root)
}

/// The EMF objects to emit for one metric emission.
///
/// Returns the normal EMF object, plus a second `coreName="ALL"` duplicate when
/// `large_fleet_workaround` is set (matching the Java/Python behavior of emitting
/// both records, not just the masked one).
pub fn build_emf_variants(
    namespace: &str,
    metric: &Metric,
    measure_values: &HashMap<String, f64>,
    large_fleet_workaround: bool,
) -> Vec<Value> {
    let mut variants = vec![build_emf(namespace, metric, measure_values, false)];
    if large_fleet_workaround {
        variants.push(build_emf(namespace, metric, measure_values, true));
    }
    variants
}

/// Build the `_aws` metadata block (`Timestamp`, `CloudWatchMetrics`).
fn metrics_metadata(namespace: &str, metric: &Metric) -> Value {
    let dimension_keys: Vec<Value> = metric
        .get_dimensions()
        .keys()
        .map(|k| Value::String(k.clone()))
        .collect();

    let measures: Vec<Value> = metric
        .get_measures()
        .values()
        .map(|m| {
            json!({
                "Name": m.get_name(),
                "Unit": m.get_unit(),
                "StorageResolution": m.get_storage_resolution(),
            })
        })
        .collect();

    json!({
        "Timestamp": now_millis(),
        "CloudWatchMetrics": [ {
            "Namespace": namespace,
            "Dimensions": [ dimension_keys ],
            "Metrics": measures,
        } ],
    })
}

/// Milliseconds since the Unix epoch (0 if the clock is before the epoch).
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::metric::MetricBuilder;

    fn values() -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert("count".to_string(), 7.0);
        v
    }

    #[test]
    fn emf_has_dimensions_measures_and_metadata() {
        let metric = MetricBuilder::create("requests")
            .with_thing_name("thing-1")
            .add_measure("count", "Count", 60)
            .build();
        let emf = build_emf("MyApp", &metric, &values(), false);

        assert_eq!(emf["count"], 7.0);
        assert_eq!(emf["coreName"], "thing-1");
        assert_eq!(emf["category"], "requests");
        assert_eq!(emf["_aws"]["CloudWatchMetrics"][0]["Namespace"], "MyApp");
        // Single dimension set listing the dimension keys.
        assert!(emf["_aws"]["CloudWatchMetrics"][0]["Dimensions"][0]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "coreName"));
    }

    #[test]
    fn timestamp_is_in_milliseconds() {
        let metric = MetricBuilder::create("m").add_measure("v", "None", 60).build();
        let emf = build_emf("ns", &metric, &values(), false);
        // Milliseconds since epoch are ~1.7e12 in 2026; seconds would be ~1.7e9.
        assert!(emf["_aws"]["Timestamp"].as_u64().unwrap() > 1_000_000_000_000);
    }

    #[test]
    fn large_fleet_workaround_masks_core_name() {
        let metric = MetricBuilder::create("m")
            .with_thing_name("thing-1")
            .add_measure("v", "None", 60)
            .build();
        let emf = build_emf("ns", &metric, &values(), true);
        assert_eq!(emf["coreName"], "ALL");
    }
}
