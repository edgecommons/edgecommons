//! Configuration types (serde; map to the YAML schema in the design doc). Phase 1 covers
//! the buffer; batch/delivery/sink config arrive with the export milestones.
//!
//! ## Numeric canonicalization at intake (D-NC6)
//!
//! Configuration stores do not agree on how they encode a JSON number: a FILE or ConfigMap
//! document carries the literal `1048576`, while the Greengrass Nucleus config store round-trips
//! every number through a Java `double` and hands back `1048576.0`. `serde` will not coerce a
//! float into a `u64`, so the same logical configuration parsed on one platform and failed on
//! another.
//!
//! [`canonicalize_json_numbers`] removes that difference **once, at this crate's own JSON intake
//! boundary** — [`StreamingConfig::from_json_str`] / [`StreamingConfig::from_json_value`], which
//! the C ABI (`esl_open`) and the Python/Node binding crates all parse through — instead of
//! per-field lenient deserializers that a new numeric field silently opts out of. It mirrors the
//! main library's `edgecommons::config::canonicalize_json_numbers` (D-NC1) exactly; the two are
//! separate implementations only because `edgestreamlog` sits *below* `edgecommons` in the
//! dependency graph and cannot depend on it.
//!
//! A document reaching this crate through `edgecommons` (`snapshot.raw["streaming"]`) is already
//! canonical, so the pass is idempotent defense in depth there and the real work happens on the
//! standalone C-ABI/binding path, which takes host JSON with nothing in front of it.
//!
//! Deserializing a config type **directly** — bypassing the intake constructors — is strict:
//! `serde` rejects a float in an integer-typed field, loudly, rather than truncating it.

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::error::{EdgeStreamError, Result};

/// `2^64` as an `f64` — the exclusive upper bound of the unsigned window.
///
/// `u64::MAX as f64` rounds *up* to exactly this value, so a naive `(f as u64) as f64 == f`
/// round-trip check would accept `2^64` and silently store `u64::MAX`. The explicit bound is what
/// makes the round-trip honest.
const U64_UPPER_EXCLUSIVE: f64 = 18_446_744_073_709_551_616.0;

/// Rewrites every JSON number in `value` that encodes an integer as an integer number.
///
/// Recurses through objects and arrays; keys, strings, booleans, and `null` are never touched.
/// A number backed by a float `f` becomes an integer **iff** `f` is finite, `f.fract() == 0.0`,
/// and the integer candidate round-trips exactly: `f >= 0.0` and `f < 2^64` yields an unsigned
/// value, `f < 0.0` a signed one. Anything else — a fractional value, a value outside the
/// exactly-representable 64-bit window, every string, boolean, and `null` — is left
/// byte-identical. The pass is pure and idempotent, so a canonical document is a fixed point.
///
/// This is the same rule as `edgecommons::config::canonicalize_json_numbers` (D-NC1); see the
/// [module docs](self) for why the two exist separately.
pub fn canonicalize_json_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(canonical) = canonical_integer(number) {
                *number = canonical;
            }
        }
        Value::Array(items) => {
            for item in items {
                canonicalize_json_numbers(item);
            }
        }
        Value::Object(map) => {
            for (_key, entry) in map.iter_mut() {
                canonicalize_json_numbers(entry);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

/// The integer form of `number`, or `None` when it is already an integer or is not an
/// exactly-representable integral value.
fn canonical_integer(number: &Number) -> Option<Number> {
    if !number.is_f64() {
        // Already an integer JSON number (`u64`/`i64`-backed) — a fixed point.
        return None;
    }
    integral_number(number.as_f64()?)
}

/// The exact integer `Number` for `f`, or `None` when `f` is not an integral value inside the
/// exactly-representable 64-bit window.
fn integral_number(f: f64) -> Option<Number> {
    if !f.is_finite() || f.fract() != 0.0 {
        return None;
    }
    if f >= 0.0 {
        if f >= U64_UPPER_EXCLUSIVE {
            return None;
        }
        let candidate = f as u64;
        // Redundant with the bound above for every finite input, kept because the round-trip *is*
        // the contract stated in the design (D-NC1 §3).
        return ((candidate as f64) == f).then(|| Number::from(candidate));
    }
    let candidate = f as i64;
    ((candidate as f64) == f).then(|| Number::from(candidate))
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
    pub max_records: usize,
    pub max_bytes: usize,
    /// Flush a partial batch after at most this long (so low rates still drain).
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
    pub max_retries: i64,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    /// How often the engine polls for new data when the buffer is empty.
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
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    /// Keep at most this many finalized files under `dir` (0 = unbounded). When exceeded, [`FileOnFull`] applies.
    #[serde(default)]
    pub max_files: u64,
    /// Roll the current file after this many seconds, evaluated on the next send (0 = time-roll disabled).
    #[serde(default)]
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

impl StreamingConfig {
    /// Parse a `streaming` document from the JSON **text** a host hands over — the crate's config
    /// intake boundary (D-NC6).
    ///
    /// Numbers are canonicalized before deserialization ([`canonicalize_json_numbers`]), so a
    /// document from a store that encodes integers as doubles (`"segmentBytes": 1048576.0`, the
    /// Greengrass Nucleus shape) parses exactly like the same document written with integer
    /// literals. Every JSON entry point into this crate parses through here — the C ABI
    /// (`esl_open`) and the Python/Node binding crates.
    ///
    /// Deserializing [`StreamingConfig`] directly is strict: use this constructor for any document
    /// that came from a configuration store.
    pub fn from_json_str(json: &str) -> serde_json::Result<Self> {
        Self::from_json_value(serde_json::from_str(json)?)
    }

    /// Parse a `streaming` document from an already-parsed JSON value, canonicalizing its numbers
    /// first. The value-shaped half of [`from_json_str`](Self::from_json_str); use it when the host
    /// hands over a `serde_json::Value` rather than text.
    pub fn from_json_value(mut value: Value) -> serde_json::Result<Self> {
        canonicalize_json_numbers(&mut value);
        serde_json::from_value(value)
    }
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
    use serde_json::json;

    /// A Greengrass-shaped document: every number written as a double, across the buffer, batch,
    /// and delivery sections (including the negative `maxRetries`).
    const GREENGRASS_SHAPED: &str = r#"{"streams":[{"name":"telemetry",
            "sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"path":"/tmp/x","segmentBytes":1048576.0,"maxDiskBytes":67108864.0,
                      "onFull":"dropOldest","maxAgeSecs":3600.0,
                      "fsyncIntervalMs":1000.0,"maxBufferedRecords":10000.0},
            "delivery":{"pollIntervalMs":1000.0,"maxRetries":-1.0,
                        "backoffBaseMs":50.0,"backoffMaxMs":30000.0},
            "batch":{"maxRecords":500.0,"maxBytes":4194304.0,"maxLatencyMs":1000.0}}]}"#;

    // Greengrass delivers config numbers as doubles (e.g. 1048576.0). A document delivered that way
    // must open exactly like the integer-literal one, or every GREENGRASS-mode deployment fails to
    // open its streams. The intake constructor is what makes it so (D-NC6).
    #[test]
    fn parses_greengrass_float_numbers() {
        let cfg =
            StreamingConfig::from_json_str(GREENGRASS_SHAPED).expect("float numbers must parse");
        let s = &cfg.streams[0];
        assert_eq!(s.buffer.segment_bytes, 1_048_576);
        assert_eq!(s.buffer.max_disk_bytes, 67_108_864);
        assert_eq!(s.buffer.max_age_secs, Some(3600));
        assert_eq!(s.buffer.fsync_interval_ms, 1000);
        assert_eq!(s.buffer.max_buffered_records, 10_000);
        assert_eq!(s.delivery.poll_interval_ms, 1000);
        assert_eq!(s.delivery.max_retries, -1);
        assert_eq!(s.delivery.backoff_base_ms, 50);
        assert_eq!(s.delivery.backoff_max_ms, 30_000);
        assert_eq!(s.batch.max_records, 500);
        assert_eq!(s.batch.max_bytes, 4_194_304);
        assert_eq!(s.batch.max_latency_ms, 1000);
    }

    // The value-shaped half of intake: a host that already parsed the JSON gets the same document.
    #[test]
    fn the_value_intake_accepts_the_same_document() {
        let value: Value = serde_json::from_str(GREENGRASS_SHAPED).unwrap();
        let cfg = StreamingConfig::from_json_value(value).expect("float numbers must parse");
        assert_eq!(cfg.streams[0].buffer.segment_bytes, 1_048_576);
        assert_eq!(cfg.streams[0].delivery.max_retries, -1);
    }

    // The file sink's own numeric fields ride the same intake (they are on the `file` sink arm,
    // which is only *built* under the `file` feature but is always parsed).
    #[test]
    fn the_file_sink_numbers_ride_the_same_intake() {
        let json = r#"{"streams":[{"name":"lake",
            "sink":{"type":"file","dir":"/tmp/lake","maxFileBytes":134217728.0,
                    "maxFiles":24.0,"rollEverySecs":900.0},
            "buffer":{"path":"/tmp/x","segmentBytes":65536.0,"maxDiskBytes":1048576.0}}]}"#;
        let cfg = StreamingConfig::from_json_str(json).expect("file sink floats must parse");
        match &cfg.streams[0].sink {
            SinkConfig::File(f) => {
                assert_eq!(f.max_file_bytes, 134_217_728);
                assert_eq!(f.max_files, 24);
                assert_eq!(f.roll_every_secs, 900);
            }
            other => panic!("expected a file sink, got {other:?}"),
        }
    }

    // The behavior change D-NC6 accepts deliberately: a caller that deserializes the config types
    // directly, bypassing intake, gets a loud `serde` error on a double instead of the silent
    // tolerance the per-field lenient deserializers used to give. Loud beats silent.
    #[test]
    fn a_direct_deserialize_rejects_a_double_instead_of_tolerating_it() {
        let err = serde_json::from_str::<StreamingConfig>(GREENGRASS_SHAPED)
            .expect_err("a direct deserialize must not silently accept a double");
        assert!(
            err.to_string().contains("floating point"),
            "expected serde's own float rejection, got: {err}"
        );
    }

    // ...and the fractional/negative values the old lenient deserializers silently truncated or
    // saturated are refused on the intake path too: canonicalization leaves them alone, and serde
    // refuses them.
    #[test]
    fn intake_refuses_a_fractional_or_negative_value_instead_of_rewriting_it() {
        let fractional = r#"{"streams":[{"name":"t","sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"path":"/tmp/x","segmentBytes":65536.5,"maxDiskBytes":1048576}}]}"#;
        let err = StreamingConfig::from_json_str(fractional)
            .expect_err("a fractional byte count must never be truncated to 65536");
        assert!(err.to_string().contains("65536.5"), "got: {err}");

        let negative = r#"{"streams":[{"name":"t","sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"path":"/tmp/x","segmentBytes":-65536.0,"maxDiskBytes":1048576}}]}"#;
        let err = StreamingConfig::from_json_str(negative)
            .expect_err("a negative byte count must never be saturated to 0");
        assert!(err.to_string().contains("-65536"), "got: {err}");
    }

    // Plain integers must still parse (non-Greengrass / FILE config) — on both paths, since a
    // direct deserialize of an integer document is unaffected by D-NC6.
    #[test]
    fn parses_integer_numbers() {
        let json = r#"{"streams":[{"name":"t","sink":{"type":"kinesis","streamName":"x"},
            "buffer":{"path":"/tmp/x","segmentBytes":65536,"maxDiskBytes":1048576}}]}"#;
        let cfg: StreamingConfig = serde_json::from_str(json).expect("integers must parse");
        assert_eq!(cfg.streams[0].buffer.segment_bytes, 65536);
        let cfg = StreamingConfig::from_json_str(json).expect("integers must parse at intake");
        assert_eq!(cfg.streams[0].buffer.segment_bytes, 65536);
    }

    // -------------------------------------------------------------------------------------------
    // The canonicalization pass itself — the D-NC1 §3 semantics table, mirrored from
    // `edgecommons::config::canonicalize_json_numbers`.
    // -------------------------------------------------------------------------------------------

    fn canonical(value: Value) -> Value {
        let mut value = value;
        canonicalize_json_numbers(&mut value);
        value
    }

    #[test]
    fn integral_doubles_become_integers() {
        assert_eq!(canonical(json!(5000.0)), json!(5000));
        assert_eq!(canonical(json!(0.0)), json!(0));
        assert_eq!(canonical(json!(1.0)), json!(1));
    }

    #[test]
    fn integers_are_a_fixed_point() {
        assert_eq!(canonical(json!(5000)), json!(5000));
        assert_eq!(canonical(json!(-5)), json!(-5));
        assert_eq!(canonical(json!(u64::MAX)), json!(u64::MAX));
        assert_eq!(canonical(json!(i64::MIN)), json!(i64::MIN));
    }

    #[test]
    fn a_fractional_value_is_left_untouched() {
        assert_eq!(canonical(json!(5000.5)), json!(5000.5));
        assert_eq!(canonical(json!(0.1)), json!(0.1));
        assert_eq!(canonical(json!(-2.5)), json!(-2.5));
    }

    #[test]
    fn a_negative_integral_double_becomes_a_signed_integer() {
        // `maxRetries: -1.0` — the one negative field in this config surface.
        let value = canonical(json!(-1.0));
        assert_eq!(value, json!(-1));
        assert_eq!(value.as_i64(), Some(-1));
        assert!(value.as_u64().is_none(), "a negative never becomes u64");
    }

    #[test]
    fn negative_zero_becomes_zero() {
        let value = canonical(json!(-0.0));
        assert_eq!(value, json!(0));
        assert_eq!(value.as_u64(), Some(0));
    }

    #[test]
    fn large_integral_doubles_inside_the_u64_window_convert() {
        // 1e19 < 2^64 and is exactly representable.
        assert_eq!(
            canonical(json!(1e19)),
            json!(10_000_000_000_000_000_000_u64)
        );
        // 2^63 exactly.
        assert_eq!(
            canonical(json!(9_223_372_036_854_775_808.0_f64)),
            json!(9_223_372_036_854_775_808_u64)
        );
        // 2^64 - 2048, the largest double strictly below 2^64.
        let largest = canonical(json!(U64_UPPER_EXCLUSIVE - 2048.0));
        assert_eq!(largest.as_u64(), Some(18_446_744_073_709_549_568));
        // 2^53 exactly.
        assert_eq!(
            canonical(json!(9_007_199_254_740_992.0_f64)),
            json!(9_007_199_254_740_992_u64)
        );
        // The negative window bound.
        assert_eq!(
            canonical(json!(-9_223_372_036_854_775_808.0_f64)),
            json!(i64::MIN)
        );
    }

    #[test]
    fn values_outside_the_sixty_four_bit_window_stay_floats() {
        assert_eq!(canonical(json!(1e20)), json!(1e20));
        assert!(canonical(json!(1e20)).as_u64().is_none());
        // Exactly 2^64: `u64::MAX as f64` rounds up to this, so a naive round-trip check would
        // wrongly accept it and store `u64::MAX`.
        assert_eq!(
            canonical(json!(U64_UPPER_EXCLUSIVE)),
            json!(U64_UPPER_EXCLUSIVE)
        );
        assert!(canonical(json!(U64_UPPER_EXCLUSIVE)).as_u64().is_none());
        assert_eq!(
            canonical(json!(-U64_UPPER_EXCLUSIVE)),
            json!(-U64_UPPER_EXCLUSIVE)
        );
        assert!(canonical(json!(-U64_UPPER_EXCLUSIVE)).as_i64().is_none());
    }

    #[test]
    fn strings_booleans_and_null_are_never_coerced() {
        assert_eq!(canonical(json!("5000")), json!("5000"));
        assert_eq!(canonical(json!("5000.0")), json!("5000.0"));
        assert_eq!(canonical(json!(true)), json!(true));
        assert_eq!(canonical(json!(null)), json!(null));
    }

    #[test]
    fn nested_objects_and_arrays_are_walked() {
        let value = canonical(json!({
            "a": { "b": [ 1.0, { "c": 2.0 }, [ 3.0 ] ] },
            "d": 4.0
        }));
        assert_eq!(
            value,
            json!({ "a": { "b": [ 1, { "c": 2 }, [ 3 ] ] }, "d": 4 })
        );
    }

    #[test]
    fn keys_are_never_touched() {
        let value = canonical(json!({ "5000.0": 1.0, "": 2.0 }));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["", "5000.0"]);
    }

    #[test]
    fn the_pass_is_idempotent() {
        let source = json!({
            "ints": [5000.0, -1.0, 5000.5, 1e20, "5000", true, null],
            "nested": { "deep": { "v": 30.0 } }
        });
        let once = canonical(source);
        let twice = canonical(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn an_empty_document_is_unchanged() {
        assert_eq!(canonical(json!({})), json!({}));
        assert_eq!(canonical(json!([])), json!([]));
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
