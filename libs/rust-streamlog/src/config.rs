//! Configuration types (serde; map to the YAML schema in the design doc). Phase 1 covers
//! the buffer; batch/delivery/sink config arrive with the export milestones.

use serde::{Deserialize, Serialize};

use crate::error::{GgStreamError, Result};

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

/// Local persistent buffer settings for one stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BufferConfig {
    /// Directory for this stream's segments + checkpoint.
    pub path: String,
    /// Roll a new segment when adding a record would exceed this size.
    pub segment_bytes: u64,
    /// Total on-disk budget; when exceeded with un-delivered data, [`OnFull`] applies.
    pub max_disk_bytes: u64,
    /// Optional age cap; records older than this are eligible for `DropOldest`.
    pub max_age_secs: Option<u64>,
    pub on_full: OnFull,
    pub fsync: FsyncPolicy,
    /// Cadence for the background fsync timer (PerBatch/Interval).
    pub fsync_interval_ms: u64,
    /// Bound on the in-memory ingest queue (records awaiting the writer thread). The memory
    /// backpressure point: when full, producers block (or `RejectNew` returns `BufferFull`).
    pub max_buffered_records: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
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
    pub max_records: usize,
    pub max_bytes: usize,
    /// Flush a partial batch after at most this long (so low rates still drain).
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
    pub max_retries: i64,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    /// How often the engine polls for new data when the buffer is empty.
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
        Ok(())
    }
}
