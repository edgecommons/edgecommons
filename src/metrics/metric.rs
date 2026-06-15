//! # Metrics — model
//!
//! **One-liner purpose**: The [`Metric`] / [`Measure`] value types and the fluent
//! [`MetricBuilder`], mirroring the Java/Python metric model.
//!
//! ## Overview
//! A [`Metric`] has a name, optional namespace, a set of [`Measure`]s (the
//! measurements it carries), and string dimensions. As in the Python library, the
//! builder injects the standard dimensions `coreName` (thing name), `category`
//! (metric name), and `component` (component name) when those are known.
//!
//! ## Semantics & Architecture
//! - Plain owned value types (`Clone`); no I/O, no async.
//! - `Measure` storage resolution is coerced to CloudWatch's allowed values: `1`
//!   (high resolution) for anything `< 60`, else `60`.
//! - Error handling: not applicable (infallible construction).
//!
//! ## Usage Example
//! ```
//! use ggcommons::metrics::metric::MetricBuilder;
//!
//! let m = MetricBuilder::create("requests")
//!     .with_namespace("MyApp")
//!     .with_thing_name("thing-1")
//!     .add_measure("count", "Count", 60)
//!     .build();
//! assert_eq!(m.get_dimensions().get("category").map(String::as_str), Some("requests"));
//! assert_eq!(m.get_dimensions().get("coreName").map(String::as_str), Some("thing-1"));
//! ```
//!
//! ## Design Choices
//! Measure values are `f64` (Python-aligned). Dimensions/measures use `BTreeMap`
//! for deterministic ordering (stable EMF output and tests).
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`crate::metrics::emf`], [`crate::metrics`].

use std::collections::BTreeMap;

use crate::config::model::Config;

/// One measurement within a [`Metric`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measure {
    name: String,
    unit: String,
    storage_resolution: u32,
}

impl Measure {
    /// Create a measure. `storage_resolution` is coerced to `1` (high resolution)
    /// for values `< 60`, otherwise `60` (CloudWatch's only accepted values).
    pub fn new(name: impl Into<String>, unit: impl Into<String>, storage_resolution: u32) -> Self {
        Self {
            name: name.into(),
            unit: unit.into(),
            storage_resolution: if storage_resolution < 60 { 1 } else { 60 },
        }
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }
    pub fn get_unit(&self) -> &str {
        &self.unit
    }
    pub fn get_storage_resolution(&self) -> u32 {
        self.storage_resolution
    }
}

/// A metric definition: name, namespace, measures, and dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metric {
    name: String,
    namespace: Option<String>,
    measures: BTreeMap<String, Measure>,
    dimensions: BTreeMap<String, String>,
}

impl Metric {
    /// Add or replace a measure.
    pub fn add_measure(&mut self, measure: Measure) {
        self.measures.insert(measure.name.clone(), measure);
    }

    /// Add or replace a dimension.
    pub fn add_dimension(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.dimensions.insert(name.into(), value.into());
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }
    pub fn get_namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }
    pub fn get_measures(&self) -> &BTreeMap<String, Measure> {
        &self.measures
    }
    pub fn get_measure(&self, name: &str) -> Option<&Measure> {
        self.measures.get(name)
    }
    pub fn get_dimensions(&self) -> &BTreeMap<String, String> {
        &self.dimensions
    }
}

/// Fluent builder for [`Metric`] (the supported construction path; mirrors the Java
/// `MetricBuilder` / Python `MetricBuilder`).
#[derive(Debug, Clone, Default)]
pub struct MetricBuilder {
    name: String,
    namespace: Option<String>,
    thing_name: Option<String>,
    component_name: Option<String>,
    measures: BTreeMap<String, Measure>,
    dimensions: BTreeMap<String, String>,
}

impl MetricBuilder {
    /// Start building a metric with the given name.
    pub fn create(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Set the metric namespace.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Set the thing name (becomes the `coreName` dimension).
    pub fn with_thing_name(mut self, thing_name: impl Into<String>) -> Self {
        self.thing_name = Some(thing_name.into());
        self
    }

    /// Set the component name (becomes the `component` dimension).
    pub fn with_component_name(mut self, component_name: impl Into<String>) -> Self {
        self.component_name = Some(component_name.into());
        self
    }

    /// Populate thing name, component name, and namespace from a config snapshot.
    pub fn with_config(mut self, config: &Config) -> Self {
        self.thing_name = Some(config.thing_name.clone());
        self.component_name = Some(config.component_name.clone());
        if self.namespace.is_none() {
            self.namespace = config.parsed.metric_emission.namespace.clone();
        }
        self
    }

    /// Add a measure with the given unit and storage resolution.
    pub fn add_measure(
        mut self,
        name: impl Into<String>,
        unit: impl Into<String>,
        storage_resolution: u32,
    ) -> Self {
        let measure = Measure::new(name, unit, storage_resolution);
        self.measures.insert(measure.name.clone(), measure);
        self
    }

    /// Add a custom dimension.
    pub fn add_dimension(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.dimensions.insert(key.into(), value.into());
        self
    }

    /// Build the metric, injecting the standard `coreName` / `category` /
    /// `component` dimensions where known (matching the Python library).
    pub fn build(mut self) -> Metric {
        self.dimensions.insert("category".to_string(), self.name.clone());
        if let Some(thing) = &self.thing_name {
            self.dimensions.insert("coreName".to_string(), thing.clone());
        }
        if let Some(component) = &self.component_name {
            self.dimensions
                .insert("component".to_string(), component.clone());
        }
        Metric {
            name: self.name,
            namespace: self.namespace,
            measures: self.measures,
            dimensions: self.dimensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_coerces_storage_resolution() {
        assert_eq!(Measure::new("m", "Count", 1).get_storage_resolution(), 1);
        assert_eq!(Measure::new("m", "Count", 30).get_storage_resolution(), 1);
        assert_eq!(Measure::new("m", "Count", 60).get_storage_resolution(), 60);
        assert_eq!(Measure::new("m", "Count", 120).get_storage_resolution(), 60);
    }

    #[test]
    fn builder_injects_standard_dimensions() {
        let m = MetricBuilder::create("requests")
            .with_namespace("MyApp")
            .with_thing_name("thing-1")
            .with_component_name("com.example.C")
            .add_measure("count", "Count", 60)
            .add_dimension("instance", "main")
            .build();

        assert_eq!(m.get_name(), "requests");
        assert_eq!(m.get_namespace(), Some("MyApp"));
        let dims = m.get_dimensions();
        assert_eq!(dims.get("category").map(String::as_str), Some("requests"));
        assert_eq!(dims.get("coreName").map(String::as_str), Some("thing-1"));
        assert_eq!(dims.get("component").map(String::as_str), Some("com.example.C"));
        assert_eq!(dims.get("instance").map(String::as_str), Some("main"));
        assert!(m.get_measure("count").is_some());
    }

    #[test]
    fn builder_without_thing_or_component_omits_those_dimensions() {
        let m = MetricBuilder::create("m").add_measure("v", "None", 60).build();
        assert!(m.get_dimensions().get("coreName").is_none());
        assert!(m.get_dimensions().get("component").is_none());
        assert_eq!(m.get_dimensions().get("category").map(String::as_str), Some("m"));
    }
}
