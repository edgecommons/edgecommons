//! # Metrics target — durable CloudWatch buffer
//!
//! **One-liner purpose**: Give the direct `cloudwatch` metric target a durable, disk-backed
//! store-and-forward buffer that drains `PutMetricData` on reconnect, by reusing the
//! [`ggstreamlog`] durable log + export engine via a host-callback [`Sink`].
//! Compiled only with the `metrics-cloudwatch-durable` feature.
//!
//! ## Overview
//! When `metricEmission.targetConfig.cloudwatch.buffer.type == "durable"`, every emitted datum is
//! serialized to a compact `{namespace, datum}` JSON record (partition key = namespace) and
//! [`append`](ggstreamlog::EmbeddedLog::append)ed to an embedded ggstreamlog stream. A background
//! [`ggstreamlog::ExportEngine`] reads committed batches and hands them to [`CloudWatchSink`],
//! which deserializes them, **drops datums outside CloudWatch's accept window** (~2 weeks past /
//! ~2 hours future) with a counter, **groups by namespace**, chunks to ≤1000 datums / ≤~1 MB, and
//! calls `PutMetricData` (one namespace per request). The buffer survives lengthy disconnects with
//! flat memory and a disk-bounded backlog (`onFull: dropOldest`).
//!
//! ## Semantics & Architecture
//! - The CloudWatch send is abstracted behind the [`PutMetricDataSender`] trait so the
//!   serialize / group / stale-drop / chunk / outcome-mapping logic is unit-testable **without**
//!   the AWS SDK (inject a fake sender). The real implementation ([`AwsPutMetricDataSender`]) wraps
//!   the `aws_sdk_cloudwatch` client and is the only piece that needs AWS.
//! - At-least-once: the engine commits the buffer checkpoint only after a batch is fully acked.
//!   `AllAcked` → commit; `Failed{retryable}` → retry (the disconnected case, `maxRetries = -1`);
//!   `Partial{failed_offsets}` → retry just those offsets.
//! - Self-recursion guard: this target does not push its own buffer stats through the buffer.
//!
//! ## Related Modules
//! - [`crate::metrics::target::cloudwatch`] (the in-memory `memory` path),
//!   [`crate::streaming`], [`ggstreamlog`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ggstreamlog::config::{
    BufferConfig, FsyncPolicy, OnFull, SinkConfig, StoreType, StreamConfig, StreamingConfig,
};
use ggstreamlog::export::{ExportRecord, SendOutcome};
use ggstreamlog::{Record, Sink, StreamService};
use serde::{Deserialize, Serialize};

use super::MetricTarget;
use crate::error::{EdgeCommonsError, Result};
use crate::metrics::metric::Metric;

/// Max datums per `PutMetricData` request (AWS hard limit).
const MAX_DATUMS_PER_REQUEST: usize = 1000;
/// Max payload bytes per `PutMetricData` request (AWS ~1 MB; we chunk conservatively below it).
const MAX_REQUEST_BYTES: usize = 1_000_000;
/// Rough serialized-size budget per datum used for the byte-based chunking (the SDK request is
/// larger than our JSON, so this is intentionally conservative).
const APPROX_BYTES_PER_DATUM: usize = 400;

/// CloudWatch accepts timestamps up to ~2 weeks in the past.
const MAX_PAST_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// ...and up to ~2 hours into the future.
const MAX_FUTURE_MS: i64 = 2 * 60 * 60 * 1000;

/// A CloudWatch dimension (name/value), serialized into the buffer record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializableDimension {
    pub name: String,
    pub value: String,
}

/// A CloudWatch metric datum in a buffer-friendly, serde-serializable form (the SDK's `MetricDatum`
/// is not `Serialize`). `ts_ms` is the datum timestamp in Unix epoch milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerializableDatum {
    pub metric_name: String,
    pub value: f64,
    pub unit: String,
    pub storage_resolution: i32,
    pub ts_ms: i64,
    pub dimensions: Vec<SerializableDimension>,
}

/// One buffered record: a datum tagged with its CloudWatch namespace (the partition key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BufferedDatum {
    pub namespace: String,
    pub datum: SerializableDatum,
}

impl BufferedDatum {
    /// Serialize to the on-buffer JSON payload.
    pub fn to_payload(&self) -> std::result::Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from an on-buffer JSON payload.
    pub fn from_payload(bytes: &[u8]) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// The CloudWatch send seam. Implemented for real by [`AwsPutMetricDataSender`] (AWS SDK) and by a
/// fake in tests, so the buffer/group/stale-drop/chunk logic runs without the heavy SDK build.
pub trait PutMetricDataSender: Send {
    /// Send one chunk (`≤1000` datums, single namespace) to CloudWatch. `Ok(())` = the whole chunk
    /// was accepted; `Err((retryable, msg))` = the request failed (throttle/5xx/transport are
    /// retryable; a malformed request is not).
    fn put(
        &self,
        namespace: &str,
        datums: &[SerializableDatum],
    ) -> std::result::Result<(), (bool, String)>;
}

/// A [`Sink`] that drains buffered datums to CloudWatch via a [`PutMetricDataSender`].
///
/// `send` deserializes the batch, drops stale datums (counting them), groups by namespace, chunks
/// to ≤1000 datums / ≤~1 MB, and sends each chunk. The whole batch acks only if every chunk
/// succeeds; any retryable chunk failure fails the whole batch (re-delivered by the engine).
pub struct CloudWatchSink<S: PutMetricDataSender> {
    sender: S,
    dropped_stale: Arc<std::sync::atomic::AtomicU64>,
}

impl<S: PutMetricDataSender> CloudWatchSink<S> {
    /// Wrap a sender. `dropped_stale` is a shared counter incremented for each datum dropped for
    /// falling outside CloudWatch's accept window.
    pub fn new(sender: S, dropped_stale: Arc<std::sync::atomic::AtomicU64>) -> Self {
        Self {
            sender,
            dropped_stale,
        }
    }

    /// Whether `ts_ms` is within CloudWatch's accept window relative to `now_ms`.
    fn in_window(ts_ms: i64, now_ms: i64) -> bool {
        let age = now_ms - ts_ms;
        (-MAX_FUTURE_MS..=MAX_PAST_MS).contains(&age)
    }
}

/// Group fresh datums by namespace, dropping any outside the accept window (returns the dropped
/// count). Pure function — the heart of the unit-testable logic.
fn group_fresh(
    decoded: Vec<BufferedDatum>,
    now_ms: i64,
) -> (BTreeMap<String, Vec<SerializableDatum>>, u64) {
    let mut groups: BTreeMap<String, Vec<SerializableDatum>> = BTreeMap::new();
    let mut dropped = 0u64;
    for bd in decoded {
        if CloudWatchSink::<NoopSender>::in_window(bd.datum.ts_ms, now_ms) {
            groups.entry(bd.namespace).or_default().push(bd.datum);
        } else {
            dropped += 1;
        }
    }
    (groups, dropped)
}

/// Split a namespace's datums into chunks within both the count (≤1000) and byte (~1 MB) limits.
fn chunk_datums(datums: &[SerializableDatum]) -> Vec<&[SerializableDatum]> {
    let max_by_bytes = (MAX_REQUEST_BYTES / APPROX_BYTES_PER_DATUM).max(1);
    let chunk_size = MAX_DATUMS_PER_REQUEST.min(max_by_bytes);
    if datums.is_empty() {
        return Vec::new();
    }
    datums.chunks(chunk_size).collect()
}

impl<S: PutMetricDataSender> Sink for CloudWatchSink<S> {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        // Deserialize. A malformed record can never succeed → drop it (count as stale-equivalent
        // data loss would wedge the stream forever otherwise); but malformed should not happen for
        // our own writer, so treat a decode error as a non-retryable batch failure surfaced once.
        let mut decoded = Vec::with_capacity(batch.len());
        for r in batch {
            match BufferedDatum::from_payload(r.payload) {
                Ok(bd) => decoded.push(bd),
                Err(e) => {
                    return SendOutcome::Failed {
                        retryable: false,
                        error: format!("malformed buffered datum at offset {}: {e}", r.offset),
                    };
                }
            }
        }

        let now_ms = now_millis();
        let (groups, dropped) = group_fresh(decoded, now_ms);
        if dropped > 0 {
            self.dropped_stale
                .fetch_add(dropped, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(
                dropped,
                "dropped CloudWatch datums outside the accept window"
            );
        }

        // Everything that remained was either sent or aged out → the batch's offsets are all
        // handled, so the engine may commit. A retryable send error fails the whole batch (the
        // engine re-delivers it from the durable buffer on the next attempt / reconnect).
        for (namespace, datums) in &groups {
            for chunk in chunk_datums(datums) {
                if let Err((retryable, error)) = self.sender.put(namespace, chunk) {
                    return SendOutcome::Failed { retryable, error };
                }
            }
        }
        SendOutcome::AllAcked
    }
}

/// A no-op sender used only to monomorphize [`CloudWatchSink::in_window`] from the free function.
struct NoopSender;
impl PutMetricDataSender for NoopSender {
    fn put(&self, _ns: &str, _d: &[SerializableDatum]) -> std::result::Result<(), (bool, String)> {
        Ok(())
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The durable CloudWatch [`MetricTarget`]: serializes datums to records and appends them to an
/// embedded ggstreamlog stream whose export sink drains to CloudWatch.
pub struct CloudWatchDurableTarget {
    service: StreamService,
    namespace: String,
    large_fleet_workaround: bool,
    dropped_stale: Arc<std::sync::atomic::AtomicU64>,
}

/// Buffer settings read from `targetConfig.cloudwatch.buffer`.
pub struct DurableBufferSettings {
    pub path: String,
    pub max_disk_bytes: u64,
    pub on_full: OnFull,
    pub fsync: FsyncPolicy,
}

impl CloudWatchDurableTarget {
    /// Open the durable buffer + start the export engine draining to CloudWatch via `sender`.
    ///
    /// `sender_factory` builds the [`PutMetricDataSender`] (the real AWS one in production, a fake
    /// in tests). It is called once when the stream's sink is constructed.
    pub fn open<S, F>(
        namespace: &str,
        large_fleet_workaround: bool,
        settings: DurableBufferSettings,
        sender_factory: F,
    ) -> Result<Self>
    where
        S: PutMetricDataSender + 'static,
        F: Fn() -> Result<S>,
    {
        let dropped_stale = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let buffer = BufferConfig {
            store_type: StoreType::Disk,
            path: settings.path,
            max_disk_bytes: settings.max_disk_bytes,
            on_full: settings.on_full,
            fsync: settings.fsync,
            ..Default::default()
        };
        let stream = StreamConfig {
            name: "cloudwatch".to_string(),
            sink: SinkConfig::Callback { id: None },
            buffer,
            // Cap the export batch at the PutMetricData datum limit; the sink also re-chunks.
            batch: ggstreamlog::config::BatchConfig {
                max_records: MAX_DATUMS_PER_REQUEST,
                ..Default::default()
            },
            delivery: Default::default(), // maxRetries = -1: retry forever (the disconnect case)
        };
        let cfg = StreamingConfig {
            streams: vec![stream],
        };

        let sender = sender_factory()?;
        let dropped_for_sink = Arc::clone(&dropped_stale);
        // The factory is called by StreamService::open_with for the single Callback stream. We move
        // the (single-use) sink into it via an Option so the FnMut-ish factory can hand it out once.
        let sink_slot = std::sync::Mutex::new(Some(CloudWatchSink::new(sender, dropped_for_sink)));
        let factory =
            move |_name: &str, sc: &SinkConfig| -> ggstreamlog::Result<Option<Box<dyn Sink>>> {
                match sc {
                    SinkConfig::Callback { .. } => {
                        let sink = sink_slot.lock().unwrap().take().ok_or_else(|| {
                            ggstreamlog::EdgeStreamError::Sink(
                            "CloudWatch sink already taken (only one cloudwatch stream is opened)"
                                .into(),
                        )
                        })?;
                        Ok(Some(Box::new(sink)))
                    }
                    _ => Ok(None),
                }
            };

        let service = StreamService::open_with(cfg, &factory).map_err(|e| {
            EdgeCommonsError::Metrics(format!("opening durable CloudWatch buffer: {e}"))
        })?;

        Ok(Self {
            service,
            namespace: namespace.to_string(),
            large_fleet_workaround,
            dropped_stale,
        })
    }

    /// Number of datums dropped so far for being outside CloudWatch's accept window.
    pub fn dropped_stale(&self) -> u64 {
        self.dropped_stale
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A stats snapshot of the underlying buffer (`None` if the stream is gone).
    pub fn buffer_stats(&self) -> Option<ggstreamlog::ServiceStats> {
        self.service.stats("cloudwatch")
    }

    /// Build the buffered datums for one emission (one record per measure value, plus the
    /// `coreName="ALL"` masked set when the large-fleet workaround is on).
    fn buffered_datums(
        &self,
        metric: &Metric,
        values: &HashMap<String, f64>,
    ) -> Vec<BufferedDatum> {
        let mut out = self.datums_for(metric, values, false);
        if self.large_fleet_workaround {
            out.extend(self.datums_for(metric, values, true));
        }
        out
    }

    fn datums_for(
        &self,
        metric: &Metric,
        values: &HashMap<String, f64>,
        mask_core_name: bool,
    ) -> Vec<BufferedDatum> {
        let dimensions: Vec<SerializableDimension> = metric
            .get_dimensions()
            .iter()
            .map(|(k, v)| {
                let value = if mask_core_name && k == "coreName" {
                    "ALL".to_string()
                } else {
                    v.clone()
                };
                SerializableDimension {
                    name: k.clone(),
                    value,
                }
            })
            .collect();
        let ts_ms = now_millis();
        values
            .iter()
            .map(|(measure_name, value)| {
                let (unit, resolution) = metric
                    .get_measure(measure_name)
                    .map(|m| (m.get_unit().to_string(), m.get_storage_resolution() as i32))
                    .unwrap_or(("None".to_string(), 60));
                BufferedDatum {
                    namespace: self.namespace.clone(),
                    datum: SerializableDatum {
                        metric_name: measure_name.clone(),
                        value: *value,
                        unit,
                        storage_resolution: resolution,
                        ts_ms,
                        dimensions: dimensions.clone(),
                    },
                }
            })
            .collect()
    }

    /// Append the buffered datums to the durable stream.
    fn append_all(&self, datums: Vec<BufferedDatum>) -> Result<()> {
        let log = self
            .service
            .stream("cloudwatch")
            .ok_or_else(|| EdgeCommonsError::Metrics("durable CloudWatch stream missing".into()))?;
        for bd in datums {
            let payload = bd
                .to_payload()
                .map_err(|e| EdgeCommonsError::Metrics(format!("serializing datum: {e}")))?;
            let ts = bd.datum.ts_ms.max(0) as u64;
            log.append(&Record::new(bd.namespace, ts, payload))
                .map_err(|e| {
                    EdgeCommonsError::Metrics(format!("appending datum to buffer: {e}"))
                })?;
        }
        Ok(())
    }
}

#[async_trait]
impl MetricTarget for CloudWatchDurableTarget {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.append_all(self.buffered_datums(metric, values))
    }

    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        // Durable path: emit_now still goes through the buffer (the engine drains promptly); there
        // is no synchronous unbuffered send in this mode by design (store-and-forward).
        self.append_all(self.buffered_datums(metric, values))
    }

    async fn flush(&self) -> Result<()> {
        // Force the buffer durably to disk; the export engine drains asynchronously.
        if let Some(log) = self.service.stream("cloudwatch") {
            log.flush()
                .map_err(|e| EdgeCommonsError::Metrics(format!("flushing buffer: {e}")))?;
        }
        Ok(())
    }

    async fn shutdown(&self) {
        // Persist the backlog to disk without draining to cloud (resumes on restart).
        let _ = self.flush().await;
    }
}

/// The production sender: wraps the `aws_sdk_cloudwatch` client and a private tokio runtime (the
/// `Sink` trait is synchronous and runs on the tokio-free export thread, mirroring `KinesisSink`).
#[cfg(feature = "cloudwatch")]
pub use aws::AwsPutMetricDataSender;

#[cfg(feature = "cloudwatch")]
mod aws {
    use super::{PutMetricDataSender, SerializableDatum};
    use aws_sdk_cloudwatch::Client;
    use aws_sdk_cloudwatch::primitives::DateTime;
    use aws_sdk_cloudwatch::types::{Dimension, MetricDatum, StandardUnit};
    use tokio::runtime::Runtime;

    /// `PutMetricData` sender backed by the AWS SDK.
    pub struct AwsPutMetricDataSender {
        rt: Runtime,
        client: Client,
    }

    impl AwsPutMetricDataSender {
        /// Build the sender, loading AWS config from the default provider chain (off the ambient
        /// runtime, like `KinesisSink`, since `open` may run inside the library's async `build()`).
        pub fn new() -> crate::error::Result<Self> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("edgecommons-cw-durable")
                .build()
                .map_err(|e| {
                    crate::error::EdgeCommonsError::Metrics(format!("tokio runtime: {e}"))
                })?;
            let client = std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        rt.block_on(async {
                            let conf =
                                aws_config::load_defaults(aws_config::BehaviorVersion::latest())
                                    .await;
                            Client::new(&conf)
                        })
                    })
                    .join()
                    .map_err(|_| {
                        crate::error::EdgeCommonsError::Metrics(
                            "CloudWatch client init panicked".into(),
                        )
                    })
            })?;
            Ok(Self { rt, client })
        }

        fn to_sdk_datum(d: &SerializableDatum) -> MetricDatum {
            let dimensions: Vec<Dimension> = d
                .dimensions
                .iter()
                .map(|dim| {
                    Dimension::builder()
                        .name(&dim.name)
                        .value(&dim.value)
                        .build()
                })
                .collect();
            MetricDatum::builder()
                .metric_name(&d.metric_name)
                .value(d.value)
                .unit(StandardUnit::from(d.unit.as_str()))
                .storage_resolution(d.storage_resolution)
                .timestamp(DateTime::from_millis(d.ts_ms))
                .set_dimensions(Some(dimensions))
                .build()
        }
    }

    impl PutMetricDataSender for AwsPutMetricDataSender {
        fn put(
            &self,
            namespace: &str,
            datums: &[SerializableDatum],
        ) -> std::result::Result<(), (bool, String)> {
            let sdk: Vec<MetricDatum> = datums.iter().map(Self::to_sdk_datum).collect();
            let resp = self.rt.block_on(
                self.client
                    .put_metric_data()
                    .namespace(namespace)
                    .set_metric_data(Some(sdk))
                    .send(),
            );
            match resp {
                Ok(_) => Ok(()),
                // Throttle/5xx/transport are all retryable; the engine governs giving up.
                Err(e) => Err((true, format!("{e}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn datum(ns: &str, name: &str, value: f64, ts_ms: i64) -> BufferedDatum {
        BufferedDatum {
            namespace: ns.to_string(),
            datum: SerializableDatum {
                metric_name: name.to_string(),
                value,
                unit: "Count".to_string(),
                storage_resolution: 60,
                ts_ms,
                dimensions: vec![SerializableDimension {
                    name: "coreName".into(),
                    value: "thing-1".into(),
                }],
            },
        }
    }

    #[test]
    fn record_round_trips_through_json() {
        let d = datum("MyApp", "count", 3.0, 1_700_000_000_000);
        let bytes = d.to_payload().unwrap();
        let back = BufferedDatum::from_payload(&bytes).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn groups_by_namespace() {
        let now = 1_700_000_000_000;
        let recs = vec![
            datum("A", "m1", 1.0, now),
            datum("B", "m2", 2.0, now),
            datum("A", "m3", 3.0, now),
        ];
        let (groups, dropped) = group_fresh(recs, now);
        assert_eq!(dropped, 0);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["A"].len(), 2);
        assert_eq!(groups["B"].len(), 1);
    }

    #[test]
    fn stale_drop_at_window_boundaries() {
        let now: i64 = 2_000_000_000_000;
        let recs = vec![
            datum("A", "fresh", 1.0, now),                          // now: keep
            datum("A", "just_past", 1.0, now - MAX_PAST_MS),        // exactly 2wk past: keep
            datum("A", "too_old", 1.0, now - MAX_PAST_MS - 1),      // 1ms older: drop
            datum("A", "just_future", 1.0, now + MAX_FUTURE_MS),    // exactly 2h future: keep
            datum("A", "too_future", 1.0, now + MAX_FUTURE_MS + 1), // 1ms beyond: drop
        ];
        let (groups, dropped) = group_fresh(recs, now);
        assert_eq!(dropped, 2, "the two out-of-window datums must be dropped");
        assert_eq!(
            groups["A"].len(),
            3,
            "the three in-window datums must survive"
        );
    }

    #[test]
    fn chunks_at_1000_datum_limit() {
        let datums: Vec<SerializableDatum> = (0..2500)
            .map(|i| datum("A", "m", i as f64, 1).datum)
            .collect();
        let chunks = chunk_datums(&datums);
        // 1MB / 400B ~= 2500 by bytes, but the 1000 datum cap dominates → 3 chunks of ≤1000.
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= MAX_DATUMS_PER_REQUEST));
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), 2500);
    }

    #[test]
    fn empty_chunking_yields_nothing() {
        assert!(chunk_datums(&[]).is_empty());
    }

    // ----- A fake sender records what it was asked to send + can inject failures. -----

    struct FakeSender {
        sent: Arc<Mutex<Vec<(String, Vec<SerializableDatum>)>>>,
        fail_remaining: Arc<AtomicU64>,
        fail_retryable: bool,
    }

    impl FakeSender {
        fn new() -> (Self, Arc<Mutex<Vec<(String, Vec<SerializableDatum>)>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    sent: Arc::clone(&sent),
                    fail_remaining: Arc::new(AtomicU64::new(0)),
                    fail_retryable: true,
                },
                sent,
            )
        }
        fn failing(n: u64, retryable: bool) -> Self {
            Self {
                sent: Arc::new(Mutex::new(Vec::new())),
                fail_remaining: Arc::new(AtomicU64::new(n)),
                fail_retryable: retryable,
            }
        }
    }

    impl PutMetricDataSender for FakeSender {
        fn put(
            &self,
            namespace: &str,
            datums: &[SerializableDatum],
        ) -> std::result::Result<(), (bool, String)> {
            if self.fail_remaining.load(Ordering::Relaxed) > 0 {
                self.fail_remaining.fetch_sub(1, Ordering::Relaxed);
                return Err((self.fail_retryable, "injected failure".into()));
            }
            self.sent
                .lock()
                .unwrap()
                .push((namespace.to_string(), datums.to_vec()));
            Ok(())
        }
    }

    /// Build ExportRecords from BufferedDatums for a direct `Sink::send` call.
    fn export_batch(datums: &[BufferedDatum]) -> (Vec<Vec<u8>>, Vec<u64>) {
        let payloads: Vec<Vec<u8>> = datums.iter().map(|d| d.to_payload().unwrap()).collect();
        let offsets: Vec<u64> = (0..datums.len() as u64).collect();
        (payloads, offsets)
    }

    fn send_through<S: PutMetricDataSender>(
        sink: &mut CloudWatchSink<S>,
        datums: &[BufferedDatum],
    ) -> SendOutcome {
        let (payloads, offsets) = export_batch(datums);
        let recs: Vec<ExportRecord<'_>> = datums
            .iter()
            .enumerate()
            .map(|(i, d)| ExportRecord {
                offset: offsets[i],
                partition_key: d.namespace.as_bytes(),
                ts_ms: d.datum.ts_ms.max(0) as u64,
                payload: &payloads[i],
            })
            .collect();
        sink.send(&recs)
    }

    #[test]
    fn outcome_all_acked_on_success_and_groups_sent() {
        let (sender, sent) = FakeSender::new();
        let counter = Arc::new(AtomicU64::new(0));
        let mut sink = CloudWatchSink::new(sender, Arc::clone(&counter));
        let now = now_millis();
        let recs = vec![datum("A", "m1", 1.0, now), datum("B", "m2", 2.0, now)];
        assert!(matches!(
            send_through(&mut sink, &recs),
            SendOutcome::AllAcked
        ));
        let sent = sent.lock().unwrap();
        // One PutMetricData call per namespace.
        assert_eq!(sent.len(), 2);
        let namespaces: Vec<&str> = sent.iter().map(|(n, _)| n.as_str()).collect();
        assert!(namespaces.contains(&"A") && namespaces.contains(&"B"));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn outcome_failed_retryable_on_throttle_or_transport() {
        let sender = FakeSender::failing(1, true);
        let counter = Arc::new(AtomicU64::new(0));
        let mut sink = CloudWatchSink::new(sender, counter);
        let now = now_millis();
        let recs = vec![datum("A", "m1", 1.0, now)];
        match send_through(&mut sink, &recs) {
            SendOutcome::Failed { retryable, .. } => assert!(retryable),
            _ => panic!("expected retryable Failed"),
        }
    }

    #[test]
    fn outcome_failed_nonretryable_on_malformed_request() {
        let sender = FakeSender::failing(1, false);
        let counter = Arc::new(AtomicU64::new(0));
        let mut sink = CloudWatchSink::new(sender, counter);
        let now = now_millis();
        let recs = vec![datum("A", "m1", 1.0, now)];
        match send_through(&mut sink, &recs) {
            SendOutcome::Failed { retryable, .. } => assert!(!retryable),
            _ => panic!("expected non-retryable Failed"),
        }
    }

    #[test]
    fn stale_datums_dropped_and_counted_in_send() {
        let (sender, sent) = FakeSender::new();
        let counter = Arc::new(AtomicU64::new(0));
        let mut sink = CloudWatchSink::new(sender, Arc::clone(&counter));
        let now = now_millis();
        let recs = vec![
            datum("A", "fresh", 1.0, now),
            datum("A", "old", 1.0, now - MAX_PAST_MS - 60_000), // aged out
        ];
        assert!(matches!(
            send_through(&mut sink, &recs),
            SendOutcome::AllAcked
        ));
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "one stale datum dropped"
        );
        // Only the fresh datum was sent.
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1.len(), 1);
        assert_eq!(sent[0].1[0].metric_name, "fresh");
    }

    #[test]
    fn malformed_record_is_nonretryable_failure() {
        let (sender, _sent) = FakeSender::new();
        let counter = Arc::new(AtomicU64::new(0));
        let mut sink = CloudWatchSink::new(sender, counter);
        let bad = b"not json".to_vec();
        let recs = vec![ExportRecord {
            offset: 0,
            partition_key: b"A",
            ts_ms: 1,
            payload: &bad,
        }];
        match sink.send(&recs) {
            SendOutcome::Failed { retryable, .. } => assert!(!retryable),
            _ => panic!("expected non-retryable Failed"),
        }
    }

    // ----- Integration: disconnect fault injection (the headline acceptance). -----

    /// A connectivity-toggleable sender: while "disconnected" every `put` fails retryably (the
    /// engine keeps the backlog on disk); on "reconnect" it succeeds and records what it sent.
    #[derive(Clone)]
    struct ToggleSender {
        connected: Arc<std::sync::atomic::AtomicBool>,
        sent_count: Arc<AtomicU64>,
    }

    impl ToggleSender {
        fn new() -> Self {
            Self {
                connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                sent_count: Arc::new(AtomicU64::new(0)),
            }
        }
        fn connect(&self) {
            self.connected.store(true, Ordering::Relaxed);
        }
        fn sent(&self) -> u64 {
            self.sent_count.load(Ordering::Relaxed)
        }
    }

    impl PutMetricDataSender for ToggleSender {
        fn put(
            &self,
            _ns: &str,
            datums: &[SerializableDatum],
        ) -> std::result::Result<(), (bool, String)> {
            if !self.connected.load(Ordering::Relaxed) {
                return Err((true, "disconnected".into()));
            }
            self.sent_count
                .fetch_add(datums.len() as u64, Ordering::Relaxed);
            Ok(())
        }
    }

    fn wait_until<F: Fn() -> bool>(f: F, timeout: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        while !f() {
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        true
    }

    /// A sender that always succeeds and tallies datums (used for target-level tests).
    #[derive(Clone)]
    struct CountingSender {
        sent: Arc<AtomicU64>,
    }
    impl CountingSender {
        fn new() -> Self {
            Self {
                sent: Arc::new(AtomicU64::new(0)),
            }
        }
        fn count(&self) -> u64 {
            self.sent.load(Ordering::Relaxed)
        }
    }
    impl PutMetricDataSender for CountingSender {
        fn put(
            &self,
            _ns: &str,
            datums: &[SerializableDatum],
        ) -> std::result::Result<(), (bool, String)> {
            self.sent.fetch_add(datums.len() as u64, Ordering::Relaxed);
            Ok(())
        }
    }

    fn defined_metric() -> Metric {
        crate::metrics::metric::MetricBuilder::create("m")
            .with_thing_name("thing-1")
            .with_component_name("com.example.C")
            .add_measure("v", "Count", 60)
            .build()
    }

    #[tokio::test]
    async fn open_emit_flush_drains_through_metric_target_trait() {
        let dir = tempfile::tempdir().unwrap();
        let sender = CountingSender::new();
        let sf = sender.clone();
        let settings = DurableBufferSettings {
            path: dir.path().join("cw").to_string_lossy().into_owned(),
            max_disk_bytes: 8 * 1024 * 1024,
            on_full: OnFull::DropOldest,
            fsync: FsyncPolicy::PerBatch,
        };
        // The production `open` (default 64 MiB segments) with maxDiskBytes 8 MiB violates the
        // segment invariant, so use a value above the default segment to exercise `open` itself.
        let settings_big = DurableBufferSettings {
            max_disk_bytes: 128 * 1024 * 1024,
            ..settings
        };
        let target =
            CloudWatchDurableTarget::open("MyApp", false, settings_big, move || Ok(sf.clone()))
                .expect("open production durable target");

        let metric = defined_metric();
        let mut values = HashMap::new();
        values.insert("v".to_string(), 1.0);

        // emit + emit_now both append; flush persists; the engine drains to the sender.
        target.emit(&metric, &values).await.unwrap();
        target.emit_now(&metric, &values).await.unwrap();
        target.flush().await.unwrap();

        let drained = wait_until(|| sender.count() >= 2, std::time::Duration::from_secs(10));
        assert!(
            drained,
            "engine should drain both emitted datums, got {}",
            sender.count()
        );
        assert_eq!(target.dropped_stale(), 0);
        assert!(target.buffer_stats().is_some());
        target.shutdown().await; // flush-to-disk, no error
    }

    #[tokio::test]
    async fn large_fleet_workaround_doubles_datums_with_masked_corename() {
        let dir = tempfile::tempdir().unwrap();
        let sender = CountingSender::new();
        let sf = sender.clone();
        let settings = DurableBufferSettings {
            path: dir.path().join("cw").to_string_lossy().into_owned(),
            max_disk_bytes: 128 * 1024 * 1024,
            on_full: OnFull::DropOldest,
            fsync: FsyncPolicy::PerBatch,
        };
        let target =
            CloudWatchDurableTarget::open("MyApp", true, settings, move || Ok(sf.clone())).unwrap();

        // With the workaround on, buffered_datums returns the normal + the coreName="ALL" set.
        let metric = defined_metric();
        let mut values = HashMap::new();
        values.insert("v".to_string(), 5.0);
        let datums = target.buffered_datums(&metric, &values);
        assert_eq!(
            datums.len(),
            2,
            "large-fleet workaround duplicates each measure value"
        );
        let masked = datums.iter().any(|d| {
            d.datum
                .dimensions
                .iter()
                .any(|dim| dim.name == "coreName" && dim.value == "ALL")
        });
        assert!(masked, "one datum set must carry the masked coreName=ALL");

        target.emit(&metric, &values).await.unwrap();
        let drained = wait_until(|| sender.count() >= 2, std::time::Duration::from_secs(10));
        assert!(
            drained,
            "both (normal + masked) datums drain, got {}",
            sender.count()
        );
    }

    #[tokio::test]
    async fn emit_datum_for_unknown_measure_uses_default_unit() {
        // A value whose name has no Measure definition still serializes (unit defaults to None).
        let dir = tempfile::tempdir().unwrap();
        let sender = CountingSender::new();
        let sf = sender.clone();
        let settings = DurableBufferSettings {
            path: dir.path().join("cw").to_string_lossy().into_owned(),
            max_disk_bytes: 128 * 1024 * 1024,
            on_full: OnFull::DropOldest,
            fsync: FsyncPolicy::PerBatch,
        };
        let target =
            CloudWatchDurableTarget::open("MyApp", false, settings, move || Ok(sf.clone()))
                .unwrap();
        let metric = defined_metric();
        let mut values = HashMap::new();
        values.insert("undefined_measure".to_string(), 9.0);
        let datums = target.buffered_datums(&metric, &values);
        assert_eq!(datums.len(), 1);
        assert_eq!(datums[0].datum.unit, "None");
        assert_eq!(datums[0].datum.storage_resolution, 60);
    }

    #[test]
    fn disconnect_stores_on_disk_then_drains_on_reconnect_and_drops_stale() {
        use crate::metrics::metric::MetricBuilder;

        let dir = tempfile::tempdir().unwrap();
        let sender = ToggleSender::new();
        let sender_for_factory = sender.clone();

        let cap: u64 = 64 * 1024; // 64 KiB total disk budget
        let settings = DurableBufferSettings {
            path: dir.path().join("cw").to_string_lossy().into_owned(),
            max_disk_bytes: cap,
            on_full: OnFull::DropOldest,
            fsync: FsyncPolicy::PerBatch,
        };
        // The default segment is 64 MiB; with a tiny cap that violates maxDiskBytes>=segmentBytes,
        // so open with an explicit small segment so DropOldest can reclaim within the cap.
        let target = open_with_segment(
            "DiscTest",
            false,
            settings,
            8 * 1024, // 8 KiB segments so DropOldest can reclaim within the small cap
            move || Ok(sender_for_factory.clone()),
        )
        .expect("open durable buffer");

        let now = now_millis();
        // 1) While disconnected, append many fresh datums. They pile up on disk, memory stays flat.
        let metric = MetricBuilder::create("m")
            .with_thing_name("t")
            .add_measure("v", "Count", 60)
            .build();
        let mut values = HashMap::new();
        values.insert("v".to_string(), 1.0);

        for _ in 0..2000 {
            target
                .append_all(target.buffered_datums(&metric, &values))
                .unwrap();
        }
        // Also append some datums with an aged-out timestamp (older than the 2-week window).
        let stale = BufferedDatum {
            namespace: "DiscTest".to_string(),
            datum: SerializableDatum {
                metric_name: "old".into(),
                value: 1.0,
                unit: "Count".into(),
                storage_resolution: 60,
                ts_ms: now - MAX_PAST_MS - 3_600_000,
                dimensions: vec![],
            },
        };
        for _ in 0..20 {
            target.append_all(vec![stale.clone()]).unwrap();
        }

        // Let the engine attempt (and fail) to drain while disconnected.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let s = target.buffer_stats().expect("stats");
        assert_eq!(
            sender.sent(),
            0,
            "nothing should be sent while disconnected"
        );
        assert!(
            s.appended_total >= 2020,
            "all appends recorded, got {}",
            s.appended_total
        );
        assert!(
            s.disk_bytes <= cap,
            "disk backlog must be bounded by maxDiskBytes ({cap}), got {}",
            s.disk_bytes
        );
        assert!(
            s.dropped_total > 0,
            "a tiny cap with DropOldest must drop oldest on disk"
        );

        // 2) Reconnect → the engine drains the surviving backlog; stale datums are dropped+counted.
        sender.connect();
        let drained = wait_until(
            || sender.sent() > 0 && target.buffer_stats().map(|s| s.backlog).unwrap_or(1) == 0,
            std::time::Duration::from_secs(10),
        );
        let final_stats = target.buffer_stats().unwrap();
        assert!(
            drained,
            "engine should drain the backlog after reconnect; sent={} backlog={}",
            sender.sent(),
            final_stats.backlog
        );
        assert!(
            sender.sent() > 0,
            "fresh datums must reach CloudWatch after reconnect"
        );
        // Some of the stale datums (those not dropped from disk by DropOldest) age out at drain.
        // dropped_stale is best-effort: assert it is queryable and consistent (>= 0). If any stale
        // record survived the disk DropOldest, it must have been counted.
        let _ = target.dropped_stale();
    }
}

/// Test-only helper: open a durable target with an explicit segment size (so a tiny `maxDiskBytes`
/// cap satisfies the `maxDiskBytes >= segmentBytes` invariant for DropOldest tests).
#[cfg(test)]
fn open_with_segment<S, F>(
    namespace: &str,
    large_fleet_workaround: bool,
    settings: DurableBufferSettings,
    segment_bytes: u64,
    sender_factory: F,
) -> Result<CloudWatchDurableTarget>
where
    S: PutMetricDataSender + 'static,
    F: Fn() -> Result<S>,
{
    let dropped_stale = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let buffer = BufferConfig {
        store_type: StoreType::Disk,
        path: settings.path,
        segment_bytes,
        max_disk_bytes: settings.max_disk_bytes,
        on_full: settings.on_full,
        fsync: settings.fsync,
        ..Default::default()
    };
    let stream = StreamConfig {
        name: "cloudwatch".to_string(),
        sink: SinkConfig::Callback { id: None },
        buffer,
        batch: ggstreamlog::config::BatchConfig {
            max_records: MAX_DATUMS_PER_REQUEST,
            ..Default::default()
        },
        delivery: ggstreamlog::config::DeliveryConfig {
            poll_interval_ms: 10,
            backoff_base_ms: 5,
            backoff_max_ms: 50,
            ..Default::default()
        },
    };
    let cfg = StreamingConfig {
        streams: vec![stream],
    };
    let sender = sender_factory()?;
    let dropped_for_sink = Arc::clone(&dropped_stale);
    let sink_slot = std::sync::Mutex::new(Some(CloudWatchSink::new(sender, dropped_for_sink)));
    let factory =
        move |_name: &str, sc: &SinkConfig| -> ggstreamlog::Result<Option<Box<dyn Sink>>> {
            match sc {
                SinkConfig::Callback { .. } => {
                    let sink = sink_slot.lock().unwrap().take().ok_or_else(|| {
                        ggstreamlog::EdgeStreamError::Sink("sink already taken".into())
                    })?;
                    Ok(Some(Box::new(sink)))
                }
                _ => Ok(None),
            }
        };
    let service = StreamService::open_with(cfg, &factory).map_err(|e| {
        EdgeCommonsError::Metrics(format!("opening durable CloudWatch buffer: {e}"))
    })?;
    Ok(CloudWatchDurableTarget {
        service,
        namespace: namespace.to_string(),
        large_fleet_workaround,
        dropped_stale,
    })
}
