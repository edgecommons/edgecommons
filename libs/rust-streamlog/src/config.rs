//! Configuration types (serde; map to the YAML schema in the design doc). Phase 1 covers
//! the buffer; batch/delivery/sink config arrive with the export milestones.

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{EdgeStreamError, Result};

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
        other => Err(serde::de::Error::custom(format!(
            "expected a number, got {other}"
        ))),
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
        other => Err(serde::de::Error::custom(format!(
            "expected a number, got {other}"
        ))),
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
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected a number, got {other}"
        ))),
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
        Self {
            max_records: 500,
            max_bytes: 4 * 1024 * 1024,
            max_latency_ms: 1000,
            compression: Compression::None,
        }
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
        Self {
            max_retries: -1,
            backoff_base_ms: 50,
            backoff_max_ms: 30_000,
            poll_interval_ms: 100,
        }
    }
}

/// File-sink output encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileFormat {
    /// Columnar Parquet (default): query-ready in Athena/BigQuery/Synapse, best compression +
    /// column pruning. Requires the `parquet` feature.
    #[default]
    Parquet,
    /// Row-oriented Avro: append-friendly landing format with true union value typing and
    /// recover-to-last-sync-block durability. Requires the `avro` feature.
    Avro,
}

/// What the file sink writes per record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileMode {
    /// Typed columnar rows. By default (no [`RowsConfig`]) the columns are the built-in
    /// `SouthboundSignalUpdate` projection — one row per `body.samples[]` element, with the envelope
    /// `tags` captured as a single JSON column and the polymorphic value in sparse typed columns; a
    /// payload that isn't a `SouthboundSignalUpdate` falls back to a sibling `_unmapped` raw file
    /// (never dropped). With a [`RowsConfig`] you declare the columns (`name`/`path`/`type`) plus an
    /// optional `explode`, mapping any message shape to a typed table.
    #[default]
    Rows,
    /// One row per message: `offset`, `partitionKey`, `tsMs`, and the opaque `payload`.
    /// Format-agnostic; works for any message.
    Raw,
}

/// Retention policy when `maxFiles` finalized files already exist under the sink directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileOnFull {
    /// Delete the oldest finalized file to stay within the ring (default).
    #[default]
    DropOldest,
    /// Stop writing (the sink reports a non-retryable failure) so the export engine stops advancing
    /// the checkpoint and the durable buffer applies backpressure / retention instead.
    Stop,
}

/// File-sink compression codec (mapped to the format's native codec at write time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileCompression {
    None,
    /// Snappy (default): fast and splittable — the conventional Parquet analytics default.
    #[default]
    Snappy,
    Zstd,
    Gzip,
}

fn default_max_file_bytes() -> u64 {
    128 * 1024 * 1024
}

/// Target type for a projected file-sink column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnType {
    /// UTF-8 string (the default); non-string JSON scalars are stringified.
    #[default]
    String,
    /// 64-bit signed integer; non-integral numbers are truncated, non-numbers null.
    Long,
    /// 64-bit float; non-numbers null.
    Double,
    /// Boolean; non-booleans null.
    Bool,
    /// The resolved value serialized as a JSON string (for objects/arrays, e.g. the envelope `tags`).
    Json,
}

/// One column in the `rows`-mode projection: a `name`, a dotted JSON `path` into the message
/// (`body.signal.id`, `tags.site`, `header.timestamp`, …), and a target `type`. With an
/// [`RowsConfig::explode`], a path under the exploded array (`body.samples[].value`) resolves
/// against the current element; other paths resolve against the message and repeat per row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnSpec {
    pub name: String,
    pub path: String,
    #[serde(rename = "type", default)]
    pub col_type: ColumnType,
}

/// The `rows`-mode projection: optionally explode an array (one output row per element) and the
/// declared columns. When the whole `rows` block is absent, the file sink uses its built-in
/// **default projection** (the SouthboundSignalUpdate layout: one row per `body.samples[]` element,
/// with the envelope `tags` captured as a single JSON column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsConfig {
    /// Path to an array; emit one row per element. Columns referencing `<explode>[]…` see the
    /// current element. Absent → one row per message.
    #[serde(default)]
    pub explode: Option<String>,
    /// The columns to write (in order).
    pub columns: Vec<ColumnSpec>,
}

/// Local rolling-file sink settings: write processed telemetry to Parquet/AVRO files (bounded by
/// max size + max file count) for later bulk upload to a cloud data lake (S3/Glue/Athena, ADLS,
/// GCS/BigQuery). Files are written to `<dir>/<partitionBy>/` and rolled on size or time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSinkConfig {
    /// Output encoding (`parquet` default | `avro`).
    #[serde(default)]
    pub format: FileFormat,
    /// Row schema (`rows` default, normalized typed telemetry | `raw`, opaque-payload archival).
    #[serde(default)]
    pub mode: FileMode,
    /// Output directory root. Config templates (`{ThingName}` etc.) are resolved upstream by the
    /// library before this reaches the core.
    pub dir: String,
    /// Optional Hive-style partition sub-path appended to `dir`, e.g. `dt={yyyy-MM-dd}/hr={HH}`.
    /// Supports UTC time tokens `{yyyy}` / `{MM}` / `{dd}` / `{HH}` and the compound `{yyyy-MM-dd}`,
    /// resolved per file at roll time. (Per-message-field partition directories are a future
    /// enhancement — those dimensions are available as columns today.)
    #[serde(default)]
    pub partition_by: Option<String>,
    /// Roll a new file once the current one would exceed this many bytes (default 128 MiB — large
    /// enough to avoid the analytics "small files" problem).
    #[serde(default = "default_max_file_bytes", deserialize_with = "lenient_u64")]
    pub max_file_bytes: u64,
    /// Keep at most this many finalized files under `dir` (0 = unbounded). When exceeded, [`FileOnFull`] applies.
    #[serde(default, deserialize_with = "lenient_u64")]
    pub max_files: u64,
    /// Roll the current file after this many seconds, evaluated on the next send (0 = time-roll disabled).
    #[serde(default, deserialize_with = "lenient_u64")]
    pub roll_every_secs: u64,
    #[serde(default)]
    pub on_full: FileOnFull,
    #[serde(default)]
    pub compression: FileCompression,
    /// Optional `rows`-mode column projection. Absent → the built-in SouthboundSignalUpdate default
    /// projection (one row per `body.samples[]`, envelope `tags` as a single JSON column).
    #[serde(default)]
    pub rows: Option<RowsConfig>,
}

impl FileSinkConfig {
    /// Validate required fields.
    pub fn validate(&self) -> Result<()> {
        if self.dir.trim().is_empty() {
            return Err(EdgeStreamError::Config(
                "file sink: `dir` is required".into(),
            ));
        }
        if self.max_file_bytes == 0 {
            return Err(EdgeStreamError::Config(
                "file sink: `maxFileBytes` must be > 0".into(),
            ));
        }
        Ok(())
    }
}

/// Target record payload format for Kinesis/Kafka sinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SinkPayloadFormat {
    /// Decode EdgeCommons protobuf envelopes to the canonical JSON projection before export.
    #[default]
    Json,
    /// Export the original EdgeCommons protobuf envelope bytes unchanged.
    Protobuf,
}

impl SinkPayloadFormat {
    #[cfg(any(feature = "file", feature = "kinesis", feature = "kafka"))]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Protobuf => "protobuf",
        }
    }
}

/// Where a stream's export engine delivers (`{"type": "kinesis", ...}` / `{"type": "kafka", ...}` /
/// `{"type": "file", ...}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SinkConfig {
    Kinesis {
        stream_name: String,
        #[serde(default)]
        region: Option<String>,
        /// Override the Kinesis endpoint (LocalStack / VPC endpoint / testing). Default chain otherwise.
        #[serde(default)]
        endpoint_url: Option<String>,
        /// Target record payload format. Defaults to JSON for analytics compatibility.
        #[serde(default)]
        payload_format: SinkPayloadFormat,
    },
    Kafka {
        /// `host:port[,host:port...]` broker list (`bootstrap.servers`).
        bootstrap_servers: String,
        topic: String,
        /// Extra librdkafka producer properties (e.g. security/SASL). Applied verbatim.
        #[serde(default)]
        properties: std::collections::BTreeMap<String, String>,
        /// Target record payload format. Defaults to JSON for analytics compatibility.
        #[serde(default)]
        payload_format: SinkPayloadFormat,
    },
    /// Local rolling Parquet/AVRO files (bounded by max size + max file count) for later bulk
    /// upload to a cloud data lake. Built only with the `file` feature (+ `parquet`/`avro`).
    File(FileSinkConfig),
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

/// The `streaming` config section: a set of named streams. This is what the C-ABI `esl_open`
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
                    return Err(EdgeStreamError::Config(
                        "buffer.path must be omitted for an in-memory buffer (type: memory)".into(),
                    ));
                }
                if self.max_disk_bytes == 0 {
                    return Err(EdgeStreamError::Config(
                        "buffer.maxDiskBytes (the in-memory byte budget) must be > 0".into(),
                    ));
                }
            }
            StoreType::Disk => {
                if self.path.is_empty() {
                    return Err(EdgeStreamError::Config("buffer.path is required".into()));
                }
                if self.segment_bytes == 0 {
                    return Err(EdgeStreamError::Config(
                        "buffer.segmentBytes must be > 0".into(),
                    ));
                }
                if self.max_disk_bytes < self.segment_bytes {
                    return Err(EdgeStreamError::Config(
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
    fn kinesis_and_kafka_payload_format_defaults_to_json() {
        let json = r#"{"streams":[
            {"name":"kinesis-json","sink":{"type":"kinesis","streamName":"x"},
             "buffer":{"path":"/tmp/x","segmentBytes":65536,"maxDiskBytes":1048576}},
            {"name":"kafka-json","sink":{"type":"kafka","bootstrapServers":"b:9092","topic":"t"},
             "buffer":{"path":"/tmp/y","segmentBytes":65536,"maxDiskBytes":1048576}}
        ]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).expect("payloadFormat default");
        match &cfg.streams[0].sink {
            SinkConfig::Kinesis { payload_format, .. } => {
                assert_eq!(*payload_format, SinkPayloadFormat::Json)
            }
            other => panic!("expected Kinesis sink, got {other:?}"),
        }
        match &cfg.streams[1].sink {
            SinkConfig::Kafka { payload_format, .. } => {
                assert_eq!(*payload_format, SinkPayloadFormat::Json)
            }
            other => panic!("expected Kafka sink, got {other:?}"),
        }
    }

    #[test]
    fn parses_explicit_protobuf_payload_format() {
        let json = r#"{"streams":[{"name":"t",
            "sink":{"type":"kafka","bootstrapServers":"b:9092","topic":"t","payloadFormat":"protobuf"},
            "buffer":{"path":"/tmp/x","segmentBytes":65536,"maxDiskBytes":1048576}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).expect("explicit protobuf format");
        match &cfg.streams[0].sink {
            SinkConfig::Kafka { payload_format, .. } => {
                assert_eq!(*payload_format, SinkPayloadFormat::Protobuf);
            }
            other => panic!("expected Kafka sink, got {other:?}"),
        }
    }

    #[test]
    fn memory_buffer_parses_and_validates() {
        // type: memory, no path, maxDiskBytes = in-memory budget.
        let json = r#"{"streams":[{"name":"m","sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"type":"memory","maxDiskBytes":65536}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.streams[0].buffer.store_type, StoreType::Memory);
        cfg.streams[0]
            .buffer
            .validate()
            .expect("valid memory buffer");

        // A path on a memory buffer is rejected.
        let mut bad = cfg.streams[0].buffer.clone();
        bad.path = "/tmp/x".into();
        assert!(bad.validate().is_err(), "memory buffer must reject a path");

        // maxDiskBytes (the memory budget) must be > 0.
        let mut zero = BufferConfig {
            store_type: StoreType::Memory,
            path: String::new(),
            max_disk_bytes: 0,
            ..Default::default()
        };
        zero.max_disk_bytes = 0;
        assert!(
            zero.validate().is_err(),
            "memory buffer must require a budget"
        );

        // Default (disk) still requires a path.
        let disk = BufferConfig::default();
        assert_eq!(disk.store_type, StoreType::Disk);
        assert!(
            disk.validate().is_err(),
            "disk buffer still requires a path"
        );
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
        assert_eq!(
            cfg.streams[0].sink,
            SinkConfig::Callback {
                id: Some("metrics-cw".into())
            }
        );
    }
}
