//! # Metrics target — log
//!
//! **One-liner purpose**: Append EMF JSON, one object per line, to a log file.
//!
//! ## Overview
//! Mirrors the Java/Python `log` metric target. The file path comes from
//! `metricEmission.targetConfig.logFileName` (template-resolved by the emitter).
//!
//! ## Semantics & Architecture
//! - Holds a single open file handle behind a `std::sync::Mutex`; writes are short
//!   and never hold the lock across an `.await`.
//! - `emit` and `emit_now` behave identically (no batching for a log file).
//! - Size-based rotation (`maxFileSize`) is a later refinement.
//! - Error handling: [`crate::error::GgError::Io`] / `Metrics`.
//!
//! ## Related Modules
//! - [`crate::metrics::emf`], [`super`].

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;

use super::MetricTarget;
use crate::error::{GgError, Result};
use crate::metrics::emf::build_emf;
use crate::metrics::metric::Metric;

/// Appends EMF JSON lines to a file.
pub struct LogTarget {
    namespace: String,
    large_fleet_workaround: bool,
    file: Mutex<File>,
}

impl LogTarget {
    /// Open (creating parent directories) the metric log file at `path`.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Io` | The file or its parent directory cannot be created/opened | Check the configured `logFileName` path and permissions |
    pub fn new(
        path: impl AsRef<Path>,
        namespace: impl Into<String>,
        large_fleet_workaround: bool,
    ) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            namespace: namespace.into(),
            large_fleet_workaround,
            file: Mutex::new(file),
        })
    }

    fn write_line(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let emf = build_emf(&self.namespace, metric, values, self.large_fleet_workaround);
        let line = serde_json::to_string(&emf)?;
        let mut file = self
            .file
            .lock()
            .map_err(|_| GgError::Metrics("metric log file mutex poisoned".to_string()))?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}

#[async_trait]
impl MetricTarget for LogTarget {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.write_line(metric, values)
    }

    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.write_line(metric, values)
    }

    async fn flush(&self) -> Result<()> {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::metric::MetricBuilder;

    #[tokio::test]
    async fn writes_emf_line_to_file() {
        let dir = std::env::temp_dir().join(format!("ggcommons-log-{}", uuid::Uuid::new_v4()));
        let path = dir.join("metric.log");
        let target = LogTarget::new(&path, "ns", false).unwrap();

        let metric = MetricBuilder::create("requests").add_measure("count", "Count", 60).build();
        let mut values = HashMap::new();
        values.insert("count".to_string(), 3.0);
        target.emit_now(&metric, &values).await.unwrap();
        target.flush().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents.lines().next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["count"], 3.0);
        assert_eq!(parsed["category"], "requests");
        assert_eq!(parsed["_aws"]["CloudWatchMetrics"][0]["Namespace"], "ns");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
