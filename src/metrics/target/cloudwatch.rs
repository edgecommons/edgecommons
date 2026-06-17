//! # Metrics target — CloudWatch (AWS SDK)
//!
//! **One-liner purpose**: Send metrics directly to Amazon CloudWatch via the AWS
//! SDK (`PutMetricData`). Compiled only with the `cloudwatch` feature.
//!
//! ## Overview
//! Mirrors the Java/Python `cloudwatch` target. `emit` buffers data and a
//! background task flushes on the configured interval; `emit_now` and `flush` send
//! immediately. Each measure value becomes one `MetricDatum` (metric name = measure
//! name) carrying the metric's dimensions.
//!
//! ## Semantics & Architecture
//! - Async (`tokio`); AWS credentials/region come from the default provider chain.
//! - Buffer is a `Mutex<Vec<MetricDatum>>`; flush sends in chunks of ≤1000 datums
//!   (the `PutMetricData` limit); per-chunk failures are logged, others still sent.
//! - The background flush task is aborted on `shutdown`/drop.
//! - Error handling: [`crate::error::GgError::Metrics`].
//!
//! ## Status
//! Validated on a live Greengrass core (non-root): heartbeat measures landed in
//! CloudWatch via `PutMetricData` at the expected cadence with no dropped batches.
//! On a Greengrass core the component must (1) declare a dependency on
//! `aws.greengrass.TokenExchangeService` so the Nucleus injects
//! `AWS_CONTAINER_CREDENTIALS_FULL_URI` (which the AWS SDK's default credential chain
//! uses), and (2) the token-exchange IAM role must allow `cloudwatch:PutMetricData`.
//!
//! ## Related Modules
//! - [`crate::metrics`], [`crate::metrics::metric`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use aws_sdk_cloudwatch::primitives::DateTime;
use aws_sdk_cloudwatch::types::{Dimension, MetricDatum, StandardUnit};
use aws_sdk_cloudwatch::Client;
use tokio::task::JoinHandle;

use super::MetricTarget;
use crate::error::Result;
use crate::metrics::metric::Metric;

/// Max datums per `PutMetricData` request.
const MAX_DATUMS_PER_REQUEST: usize = 1000;

/// Sends metrics to CloudWatch via the AWS SDK.
pub struct CloudWatchTarget {
    client: Client,
    namespace: String,
    large_fleet_workaround: bool,
    pending: Arc<Mutex<Vec<MetricDatum>>>,
    flush_task: Option<JoinHandle<()>>,
}

impl CloudWatchTarget {
    /// Build the target, loading AWS config from the default provider chain and
    /// starting the periodic flush task.
    pub async fn new(
        namespace: &str,
        large_fleet_workaround: bool,
        interval_secs: u64,
    ) -> Result<Self> {
        let sdk_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = Client::new(&sdk_config);
        let pending: Arc<Mutex<Vec<MetricDatum>>> = Arc::new(Mutex::new(Vec::new()));

        let flush_task = {
            let client = client.clone();
            let namespace = namespace.to_string();
            let pending = pending.clone();
            Some(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs.max(1)));
                loop {
                    ticker.tick().await;
                    let batch = take_all(&pending);
                    if !batch.is_empty() {
                        send_batches(&client, &namespace, batch).await;
                    }
                }
            }))
        };

        Ok(Self {
            client,
            namespace: namespace.to_string(),
            large_fleet_workaround,
            pending,
            flush_task,
        })
    }

    /// All datums to emit for one emission: the normal datums plus a
    /// `coreName="ALL"` set when `large_fleet_workaround` is enabled.
    fn datums_for(&self, metric: &Metric, values: &HashMap<String, f64>) -> Vec<MetricDatum> {
        let mut datums = self.to_datums(metric, values, false);
        if self.large_fleet_workaround {
            datums.extend(self.to_datums(metric, values, true));
        }
        datums
    }

    /// Convert a metric + values into CloudWatch datums (one per measure value).
    fn to_datums(
        &self,
        metric: &Metric,
        values: &HashMap<String, f64>,
        mask_core_name: bool,
    ) -> Vec<MetricDatum> {
        let dimensions: Vec<Dimension> = metric
            .get_dimensions()
            .iter()
            .map(|(k, v)| {
                let value = if mask_core_name && k == "coreName" {
                    "ALL".to_string()
                } else {
                    v.clone()
                };
                Dimension::builder().name(k).value(value).build()
            })
            .collect();
        let timestamp = DateTime::from_millis(now_millis());

        values
            .iter()
            .map(|(measure_name, value)| {
                let (unit, resolution) = metric
                    .get_measure(measure_name)
                    .map(|m| (StandardUnit::from(m.get_unit()), m.get_storage_resolution() as i32))
                    .unwrap_or((StandardUnit::None, 60));
                MetricDatum::builder()
                    .metric_name(measure_name)
                    .value(*value)
                    .unit(unit)
                    .storage_resolution(resolution)
                    .timestamp(timestamp)
                    .set_dimensions(Some(dimensions.clone()))
                    .build()
            })
            .collect()
    }
}

impl Drop for CloudWatchTarget {
    fn drop(&mut self) {
        if let Some(task) = self.flush_task.take() {
            task.abort();
        }
    }
}

#[async_trait]
impl MetricTarget for CloudWatchTarget {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let datums = self.datums_for(metric, values);
        if let Ok(mut pending) = self.pending.lock() {
            pending.extend(datums);
        }
        Ok(())
    }

    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let datums = self.datums_for(metric, values);
        send_batches(&self.client, &self.namespace, datums).await;
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        let batch = take_all(&self.pending);
        if !batch.is_empty() {
            send_batches(&self.client, &self.namespace, batch).await;
        }
        Ok(())
    }

    async fn shutdown(&self) {
        let _ = self.flush().await;
    }
}

/// Drain the pending buffer.
fn take_all(pending: &Arc<Mutex<Vec<MetricDatum>>>) -> Vec<MetricDatum> {
    pending.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
}

/// Send datums in ≤1000-item batches; log (don't propagate) per-batch failures.
async fn send_batches(client: &Client, namespace: &str, datums: Vec<MetricDatum>) {
    for chunk in datums.chunks(MAX_DATUMS_PER_REQUEST) {
        let result = client
            .put_metric_data()
            .namespace(namespace)
            .set_metric_data(Some(chunk.to_vec()))
            .send()
            .await;
        if let Err(e) = result {
            tracing::error!(error = %e, count = chunk.len(), "PutMetricData failed; dropping batch");
        }
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
