//! Configuration types (serde; map to the YAML schema in the design doc). Phase 1 covers
//! the buffer; batch/delivery/sink config arrive with the export milestones.

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{GgStreamError, Result};

// Greengrass stores all configuration numbers as doubles, so an integer like `1048576` arrives
// over GG_CONFIG as `1048576.0`. serde's integer deserializers reject a float, which would fail
// every streaming config delivered through Greengrass. These lenient deserializers accept either
// an integer or an integer-valued float for the numeric buffer/batch/delivery fields.
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        other => Err(serde::de::Error::custom(format!("expected a number, got {other}"))),
    }
}

fn lenient_usize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<usize, D::Error> {
    lenient_u64(d).map(|v| v as usize)
}

fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<i64, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .ok_or_else(|| serde::de::Error::custom("expected an integer")),
        other => Err(serde::de::Error::custom(format!("expected a number, got {other}"))),
    }
}

fn lenient_opt_u64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<u64>, D::Error> {
    match Option::<serde_json::Value>::deserialize(d)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        Some(other) => Err(serde::de::Error::custom(format!("expected a number, got {other}"))),
    }
}

/// Backpressure policy when the on-disk budget is exceeded with un-delivered data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnFull {
    /// Drop the oldest data to stay within budget (telemetry default; never blocks producers).
    #[default]
    DropOldest,
    /// Block the producer until the exporter delivers + reclaims space (lossless).
    Block,
    /// Reject new appends while over budget.
    RejectNew,
}

/// Durability ↔ throughput dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FsyncPolicy {
    /// fsync per append_batch + on the interval timer (default).
    #[default]
    PerBatch,
    /// fsync only on the interval timer (widest crash window, fastest).
    Interval,
    /// fsync every record (safest, slowest).
    Always,
}

/// Where a stream's buffer lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoreType {
    /// Durable file-backed segment log (default): survives restarts, recovered on open.
    #[default]
    Disk,
    /// In-memory ring — **non-durable**: records are lost on component restart/crash and never
    /// touch disk. For best-effort streams where durability / high QoS is unnecessary (cheap
    /// telemetry, debug traces); no disk I/O, no recovery. Bounded by `maxDiskBytes` (interpreted
    /// as the in-memory byte budget) with `onFull` applied on overflow.
    Memory,
}

/// Local buffer settings for one stream (durable on disk, or in-memory per [`StoreType`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BufferConfig {
    /// Buffer backing store: `disk` (default, durable) or `memory` (non-durable).
    #[serde(rename = "type")]
    pub store_type: StoreType,
    /// Directory for this stream's segments + checkpoint (required for `disk`; must be omitted for `memory`).
    pub path: String,
    /// Roll a new segment when adding a record would exceed this size.
    #[serde(deserialize_with = "lenient_u64")]
    pub segment_bytes: u64,
    /// Total on-disk budget; when exceeded with un-delivered data, [`OnFull`] applies.
    #[serde(deserialize_with = "lenient_u64")]
    pub max_disk_bytes: u64,
    /// Optional age cap; records older than this are eligible for `DropOldest`.
    #[serde(deserialize_with = "lenient_opt_u64")]
    pub max_age_secs: Option<u64>,
    pub on_full: OnFull,
    pub fsync: FsyncPolicy,
    /// Cadence for the background fsync timer (PerBatch/Interval).
    #[serde(deserialize_with = "lenient_u64")]
    pub fsync_interval_ms: u64,
    /// Bound on the in-memory ingest queue (records awaiting the writer thread). The memory
    /// backpressure point: when full, producers block (or `RejectNew` returns `BufferFull`).
    #[serde(deserialize_with = "lenient_usize")]
    pub max_buffered_records: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            store_type: StoreType::Disk,
            path: String::new(),
            segment_bytes: 64 * 1024 * 1024,
            max_disk_bytes: 1024 * 1024 * 1024,
            max_age_secs: None,
            on_full: OnFull::default(),
            fsync: FsyncPolicy::default(),
            fsync_interval_ms: 1000,
            max_buffered_records: 10_000,
        }
    }
}

/// Per-record payload compression (applied by the sink). Phase 1: `Zstd` is accepted but
/// treated as `None` until the sink implements it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Compression {
    #[default]
    None,
    Zstd,
}

/// How the export engine batches records before a send.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BatchConfig {
    #[serde(deserialize_with = "lenient_usize")]
    pub max_records: usize,
    #[serde(deserialize_with = "lenient_usize")]
    pub max_bytes: usize,
    /// Flush a partial batch after at most this long (so low rates still drain).
    #[serde(deserialize_with = "lenient_u64")]
    pub max_latency_ms: u64,
    pub compression: Compression,
}
impl Default for BatchConfig {
    fn default() -> Self {
        Self { max_records: 500, max_bytes: 4 * 1024 * 1024, max_latency_ms: 1000, compression: Compression::None }
    }
}

/// Delivery/retry behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeliveryConfig {
    /// Max send attempts before giving up a batch (`-1` = forever — the disconnected case).
    #[serde(deserialize_with = "lenient_i64")]
    pub max_retries: i64,
    #[serde(deserialize_with = "lenient_u64")]
    pub backoff_base_ms: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub backoff_max_ms: u64,
    /// How often the engine polls for new data when the buffer is empty.
    #[serde(deserialize_with = "lenient_u64")]
    pub poll_interval_ms: u64,
}
impl Default for DeliveryConfig {
    fn default() -> Self {
        Self { max_retries: -1, backoff_base_ms: 50, backoff_max_ms: 30_000, poll_interval_ms: 100 }
    }
}

/// Where a stream's export engine delivers (`{"type": "kinesis", ...}` / `{"type": "kafka", ...}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SinkConfig {
    Kinesis {
        stream_name: String,
        #[serde(default)]
        region: Option<String>,
        /// Override the Kinesis endpoint (LocalStack / VPC endpoint / testing). Default chain otherwise.
        #[serde(default)]
        endpoint_url: Option<String>,
    },
    Kafka {
        /// `host:port[,host:port...]` broker list (`bootstrap.servers`).
        bootstrap_servers: String,
        topic: String,
        /// Extra librdkafka producer properties (e.g. security/SASL). Applied verbatim.
        #[serde(default)]
        properties: std::collections::BTreeMap<String, String>,
    },
    /// A host-provided sink (the CloudWatch metrics drain, or a caller's "bring-your-own-sink").
    /// The send logic lives in the host; the engine drives it through a
    /// [`crate::export::CallbackSink`]. The actual callback is bound at `open_with` time (Rust lib)
    /// or via the C-ABI sink-callback registration (language bindings). The default sink factory has
    /// no callback, so a `callback` stream opened via [`crate::StreamService::open`] is buffer-only.
    Callback {
        /// Optional id, to route to a specific host callback when several callback streams exist.
        #[serde(default)]
        id: Option<String>,
    },
}

/// One configured stream: a name, its export sink, durable buffer, and batching/delivery tuning.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamConfig {
    pub name: String,
    pub sink: SinkConfig,
    pub buffer: BufferConfig,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub delivery: DeliveryConfig,
}

/// The `streaming` config section: a set of named streams. This is what the C-ABI `ggsl_open`
/// receives as JSON, and what the language libs build (after template substitution).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamingConfig {
    pub streams: Vec<StreamConfig>,
}

impl BufferConfig {
    pub fn validate(&self) -> Result<()> {
        match self.store_type {
            StoreType::Memory => {
                // In-memory: no path/segments; maxDiskBytes is the in-memory byte budget.
                if !self.path.is_empty() {
                    return Err(GgStreamError::Config(
                        "buffer.path must be omitted for an in-memory buffer (type: memory)".into(),
                    ));
                }
                if self.max_disk_bytes == 0 {
                    return Err(GgStreamError::Config(
                        "buffer.maxDiskBytes (the in-memory byte budget) must be > 0".into(),
                    ));
                }
            }
            StoreType::Disk => {
                if self.path.is_empty() {
                    return Err(GgStreamError::Config("buffer.path is required".into()));
                }
                if self.segment_bytes == 0 {
                    return Err(GgStreamError::Config("buffer.segmentBytes must be > 0".into()));
                }
                if self.max_disk_bytes < self.segment_bytes {
                    return Err(GgStreamError::Config(
                        "buffer.maxDiskBytes must be >= segmentBytes".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Greengrass delivers config numbers as doubles (e.g. 1048576.0). The streaming config must
    // accept integer-valued floats for the numeric buffer/batch/delivery fields, or every
    // GREENGRASS-mode deployment fails to open its streams.
    #[test]
    fn parses_greengrass_float_numbers() {
        let json = r#"{"streams":[{"name":"telemetry",
            "sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"path":"/tmp/x","segmentBytes":1048576.0,"maxDiskBytes":67108864.0,
                      "onFull":"dropOldest","maxAgeSecs":3600.0},
            "delivery":{"pollIntervalMs":1000.0,"maxRetries":-1.0},
            "batch":{"maxRecords":500.0,"maxBytes":4194304.0}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).expect("float numbers must parse");
        let s = &cfg.streams[0];
        assert_eq!(s.buffer.segment_bytes, 1_048_576);
        assert_eq!(s.buffer.max_disk_bytes, 67_108_864);
        assert_eq!(s.buffer.max_age_secs, Some(3600));
        assert_eq!(s.delivery.poll_interval_ms, 1000);
        assert_eq!(s.delivery.max_retries, -1);
        assert_eq!(s.batch.max_records, 500);
        assert_eq!(s.batch.max_bytes, 4_194_304);
    }

    // Plain integers must still parse (non-Greengrass / FILE config).
    #[test]
    fn parses_integer_numbers() {
        let json = r#"{"streams":[{"name":"t","sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"path":"/tmp/x","segmentBytes":65536,"maxDiskBytes":1048576}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).expect("integers must parse");
        assert_eq!(cfg.streams[0].buffer.segment_bytes, 65536);
    }

    #[test]
    fn memory_buffer_parses_and_validates() {
        // type: memory, no path, maxDiskBytes = in-memory budget.
        let json = r#"{"streams":[{"name":"m","sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"type":"memory","maxDiskBytes":65536}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.streams[0].buffer.store_type, StoreType::Memory);
        cfg.streams[0].buffer.validate().expect("valid memory buffer");

        // A path on a memory buffer is rejected.
        let mut bad = cfg.streams[0].buffer.clone();
        bad.path = "/tmp/x".into();
        assert!(bad.validate().is_err(), "memory buffer must reject a path");

        // maxDiskBytes (the memory budget) must be > 0.
        let mut zero = BufferConfig { store_type: StoreType::Memory, path: String::new(), max_disk_bytes: 0, ..Default::default() };
        zero.max_disk_bytes = 0;
        assert!(zero.validate().is_err(), "memory buffer must require a budget");

        // Default (disk) still requires a path.
        let disk = BufferConfig::default();
        assert_eq!(disk.store_type, StoreType::Disk);
        assert!(disk.validate().is_err(), "disk buffer still requires a path");
    }

    #[test]
    fn parses_callback_sink() {
        // Bare callback sink (single host callback).
        let json = r#"{"streams":[{"name":"cw","sink":{"type":"callback"},
            "buffer":{"path":"/tmp/x","segmentBytes":65536,"maxDiskBytes":1048576}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.streams[0].sink, SinkConfig::Callback { id: None });

        // With an explicit id for routing among several callback streams.
        let json = r#"{"streams":[{"name":"cw","sink":{"type":"callback","id":"metrics-cw"},
            "buffer":{"path":"/tmp/x","segmentBytes":65536,"maxDiskBytes":1048576}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.streams[0].sink, SinkConfig::Callback { id: Some("metrics-cw".into()) });
    }
}
