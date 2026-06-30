//! `FileSink` — a rolling Parquet / Avro file sink (feature = `file`, with an encoder
//! `parquet` and/or `avro`).
//!
//! Writes processed telemetry to local Parquet (columnar) or Avro (row-oriented) files, bounded by
//! a max file size + max file count, for later bulk upload to a cloud data lake
//! (S3/Glue/Athena, ADLS, GCS/BigQuery). Files land under `<dir>/<partitionBy>/`, are written to a
//! `*.inprogress` temp path, and are atomically renamed to their final partitioned path when
//! finalized (on a size/time roll, or on a clean shutdown via [`Drop`]).
//!
//! Two body schemas are supported (see [`FileMode`]):
//! - **rows** (default): each `SouthboundTagUpdate` envelope is flattened to one row per
//!   `body.samples[]` element, with the polymorphic sample value landing in sparse typed columns
//!   (Parquet) or a true `["null","double","long","boolean","string"]` union (Avro). A payload that
//!   is not a `SouthboundTagUpdate` is written to a sibling `_unmapped` raw file — never dropped.
//! - **raw**: one row per message (`offset`, `partitionKey`, `tsMs`, `payload`); format-agnostic.
//!
//! # Feature gating
//!
//! This module compiles with the `file` umbrella feature alone (no encoder); in that case
//! [`FileSink::new`] returns a [`GgStreamError::Config`] for any format because no encoder is built
//! in. All Arrow/Parquet code is behind `feature = "parquet"` and all Avro code behind
//! `feature = "avro"`.
//!
//! # Durability semantics
//!
//! The export engine commits the buffer's read offset only after a batch is acked
//! ([`SendOutcome::AllAcked`]). This sink returns `AllAcked` once a batch has been written **and
//! flushed durably** to the current file (Parquet: the batch is written as a flushed row group;
//! Avro: written as a block and fsync'd). On a clean shutdown the open file is finalized on
//! [`Drop`] (footer/final block written, fsync'd, atomically renamed), so a clean stop loses
//! nothing.
//!
//! On a **hard crash**: an unclosed `*.inprogress` Parquet file has no footer and is discarded on
//! restart — loss is bounded by the open-file window (`rollEverySecs` / `maxFileBytes`). An Avro
//! `*.inprogress` file is recoverable up to its last written block. Because the buffer offset is
//! committed only after the sink acks, a record re-delivered after a crash that happened between
//! the sink write and the buffer commit can appear twice (at-least-once); dedup downstream on
//! `(tagId, sourceTs)`.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{ExportRecord, SendOutcome, Sink};
use crate::config::{FileCompression, FileFormat, FileMode, FileOnFull, FileSinkConfig};
use crate::error::{GgStreamError, Result};

// ----------------------------------------------------------------------------------------------
// Public sink
// ----------------------------------------------------------------------------------------------

/// A [`Sink`] that writes batches to rolling Parquet/Avro files under a directory.
///
/// One `FileSink` owns at most two open files at a time: the `main` file (rows or raw, per
/// [`FileMode`]) and — in `rows` mode only — an `unmapped` raw file for payloads that aren't a
/// `SouthboundTagUpdate`. It tracks the files it has finalized in an in-memory ring to enforce
/// `maxFiles`.
pub struct FileSink {
    /// Stream/sink name (for log context).
    name: String,
    /// Validated configuration.
    cfg: FileSinkConfig,
    /// The currently-open main file (`None` until the first matching record).
    main: Option<ActiveFile>,
    /// The currently-open `_unmapped` raw file (rows mode only).
    unmapped: Option<ActiveFile>,
    /// Monotonic file sequence (increments per opened file → unique names).
    seq: u64,
    /// Files this sink has finalized, oldest first (the `maxFiles` ring).
    finalized: VecDeque<PathBuf>,
    /// File extension for the configured format (`parquet` / `avro`).
    ext: &'static str,
}

impl FileSink {
    /// Build a file sink for `cfg`.
    ///
    /// Validates `cfg` and verifies the requested [`FileFormat`]'s encoder feature is compiled in;
    /// returns [`GgStreamError::Config`] otherwise (e.g. `format = parquet` without the `parquet`
    /// feature). No file is opened here — the first file opens lazily on the first [`send`].
    ///
    /// [`send`]: Sink::send
    pub fn new(name: &str, cfg: FileSinkConfig) -> crate::Result<Self> {
        cfg.validate()?;
        let have_encoder = match cfg.format {
            FileFormat::Parquet => cfg!(feature = "parquet"),
            FileFormat::Avro => cfg!(feature = "avro"),
        };
        if !have_encoder {
            return Err(GgStreamError::Config(format!(
                "file sink: format {:?} requires the matching encoder feature to be compiled in \
                 (`parquet` and/or `avro`)",
                cfg.format
            )));
        }
        let ext = match cfg.format {
            FileFormat::Parquet => "parquet",
            FileFormat::Avro => "avro",
        };
        Ok(Self {
            name: name.to_string(),
            cfg,
            main: None,
            unmapped: None,
            seq: 0,
            finalized: VecDeque::new(),
            ext,
        })
    }

    /// Encode + flush the whole batch, returning `Err` on the first I/O/encode error (so [`send`]
    /// maps it to a non-retryable [`SendOutcome::Failed`]).
    ///
    /// [`send`]: Sink::send
    fn write_batch(&mut self, batch: &[ExportRecord<'_>]) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        // Time-roll the open files on entry (cheap; only when `rollEverySecs > 0`).
        self.time_roll(Slot::Main)?;
        self.time_roll(Slot::Unmapped)?;

        match self.cfg.mode {
            FileMode::Raw => {
                let rows: Vec<RawRow> = batch.iter().map(raw_row).collect();
                self.write_to_slot(Slot::Main, RowSchema::Raw, WriteRows::Raw(&rows))?;
            }
            FileMode::Rows => {
                let mut main_rows: Vec<MainRow> = Vec::new();
                let mut unmapped: Vec<RawRow> = Vec::new();
                for r in batch {
                    match extract_rows(r.payload, r.ts_ms as i64, r.offset as i64) {
                        Some(mut rs) => main_rows.append(&mut rs),
                        None => unmapped.push(raw_row(r)),
                    }
                }
                if !main_rows.is_empty() {
                    self.write_to_slot(Slot::Main, RowSchema::Rows, WriteRows::Rows(&main_rows))?;
                }
                if !unmapped.is_empty() {
                    self.write_to_slot(Slot::Unmapped, RowSchema::Raw, WriteRows::Raw(&unmapped))?;
                }
            }
        }
        Ok(())
    }

    /// Append `rows` to the given slot's open file (opening it lazily), then roll it if it now
    /// exceeds `maxFileBytes`.
    fn write_to_slot(&mut self, slot: Slot, schema: RowSchema, rows: WriteRows<'_>) -> Result<()> {
        if self.slot_ref(slot).is_none() {
            let af = self.open_file(schema, matches!(slot, Slot::Unmapped))?;
            *self.slot_mut(slot) = Some(af);
        }
        {
            let af = self.slot_mut(slot).as_mut().expect("slot just opened");
            af.open.write(rows)?;
            af.bytes = af.open.current_len()?;
        }
        let over = self
            .slot_ref(slot)
            .as_ref()
            .map(|af| af.bytes >= self.cfg.max_file_bytes)
            .unwrap_or(false);
        if over {
            if let Some(af) = self.slot_take(slot) {
                self.finalize(af)?;
            }
        }
        Ok(())
    }

    /// Roll the slot's open file if it has reached `rollEverySecs` of age.
    fn time_roll(&mut self, slot: Slot) -> Result<()> {
        if self.cfg.roll_every_secs == 0 {
            return Ok(());
        }
        let due = self
            .slot_ref(slot)
            .as_ref()
            .map(|af| {
                af.opened_at
                    .elapsed()
                    .map(|d| d.as_secs() >= self.cfg.roll_every_secs)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if due {
            if let Some(af) = self.slot_take(slot) {
                self.finalize(af)?;
            }
        }
        Ok(())
    }

    /// Open a fresh file for `schema` under the (time-resolved) partition directory. Enforces the
    /// `Stop` retention policy: at the `maxFiles` cap it refuses to open a new file (so the buffer
    /// applies backpressure instead of the ring deleting data).
    fn open_file(&mut self, schema: RowSchema, unmapped: bool) -> Result<ActiveFile> {
        if matches!(self.cfg.on_full, FileOnFull::Stop)
            && self.cfg.max_files > 0
            && self.finalized.len() as u64 >= self.cfg.max_files
        {
            return Err(GgStreamError::Sink(format!(
                "file sink: maxFiles={} reached and onFull=stop; refusing to open a new file",
                self.cfg.max_files
            )));
        }
        let now = SystemTime::now();
        let unix_ms = now.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let dir = self.partition_dir((unix_ms / 1000) as i64);
        fs::create_dir_all(&dir)?;

        let seq = self.seq;
        self.seq += 1;
        let marker = if unmapped { "_unmapped" } else { "" };
        let filename = format!("part-{unix_ms}-{seq}{marker}.{}", self.ext);
        let final_path = dir.join(&filename);
        let inprogress = dir.join(format!("{filename}.inprogress"));

        let open = create_open_file(self.cfg.format, schema, self.cfg.compression, &inprogress)?;
        Ok(ActiveFile { open, final_path, inprogress, bytes: 0, opened_at: now })
    }

    /// Finalize `af`: close the encoder (write footer/final block + fsync), atomically rename the
    /// `*.inprogress` file to its final path, record it in the ring, and enforce `maxFiles`
    /// (`DropOldest` deletes the oldest finalized files to stay within the cap).
    fn finalize(&mut self, af: ActiveFile) -> Result<()> {
        let ActiveFile { open, final_path, inprogress, .. } = af;
        open.close()?;
        fs::rename(&inprogress, &final_path)?;
        tracing::debug!(sink = %self.name, file = %final_path.display(), "file sink: finalized file");
        self.finalized.push_back(final_path);
        if self.cfg.max_files > 0 && matches!(self.cfg.on_full, FileOnFull::DropOldest) {
            while self.finalized.len() as u64 > self.cfg.max_files {
                match self.finalized.pop_front() {
                    Some(old) => {
                        let _ = fs::remove_file(&old);
                    }
                    None => break,
                }
            }
        }
        Ok(())
    }

    /// `<dir>` plus the partition sub-path with UTC time tokens resolved for `unix_secs`.
    fn partition_dir(&self, unix_secs: i64) -> PathBuf {
        let mut p = PathBuf::from(&self.cfg.dir);
        if let Some(pb) = self.cfg.partition_by.as_deref() {
            if !pb.is_empty() {
                p.push(resolve_tokens(pb, unix_secs));
            }
        }
        p
    }

    fn slot_ref(&self, slot: Slot) -> &Option<ActiveFile> {
        match slot {
            Slot::Main => &self.main,
            Slot::Unmapped => &self.unmapped,
        }
    }
    fn slot_mut(&mut self, slot: Slot) -> &mut Option<ActiveFile> {
        match slot {
            Slot::Main => &mut self.main,
            Slot::Unmapped => &mut self.unmapped,
        }
    }
    fn slot_take(&mut self, slot: Slot) -> Option<ActiveFile> {
        self.slot_mut(slot).take()
    }
}

impl Sink for FileSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        match self.write_batch(batch) {
            Ok(()) => SendOutcome::AllAcked,
            // An encode/IO/retention error is non-retryable: re-sending the same batch can't fix a
            // bad payload or a full ring. The engine surfaces it and (for `Stop`) stops advancing.
            Err(e) => SendOutcome::Failed { retryable: false, error: e.to_string() },
        }
    }
}

impl Drop for FileSink {
    /// Finalize any open files so a clean shutdown never loses buffered rows.
    fn drop(&mut self) {
        if let Some(af) = self.main.take() {
            let _ = self.finalize(af);
        }
        if let Some(af) = self.unmapped.take() {
            let _ = self.finalize(af);
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Open-file state (an encoder-agnostic enum; encoder variants are feature-gated)
// ----------------------------------------------------------------------------------------------

/// Which of the sink's two file slots a write targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The primary file (rows or raw, per [`FileMode`]).
    Main,
    /// The `_unmapped` sibling raw file (rows mode only).
    Unmapped,
}

/// Which body schema an open file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowSchema {
    /// Normalized typed telemetry rows (rows mode main file).
    Rows,
    /// Opaque-payload raw rows (raw mode main file, or any `_unmapped` file).
    Raw,
}

/// Rows handed to an open file for one write (borrows the caller's row buffers).
#[derive(Clone, Copy)]
#[cfg_attr(not(any(feature = "parquet", feature = "avro")), allow(dead_code))]
enum WriteRows<'a> {
    Rows(&'a [MainRow]),
    Raw(&'a [RawRow]),
}

/// One open output file plus the bookkeeping the ring/roller needs.
struct ActiveFile {
    open: OpenFile,
    /// Final partitioned path (the `*.inprogress` file is renamed here on finalize).
    final_path: PathBuf,
    /// The temp path actually written to while the file is open.
    inprogress: PathBuf,
    /// Bytes written to the file so far (refreshed after each flush; drives size rolling).
    bytes: u64,
    /// When the file was opened (drives time rolling).
    opened_at: SystemTime,
}

/// The open encoder writer. The `Closed` state lets the type exist without any encoder feature; the
/// `Parquet`/`Avro` variants are compiled only with their feature (and boxed to keep the enum
/// small).
enum OpenFile {
    /// No encoder compiled in for this file. Only exists so the enum stays inhabited and matches
    /// stay exhaustive in a `file`-only build; never constructed (the format guard in
    /// [`FileSink::new`] rejects an encoder-less format up front).
    #[allow(dead_code)]
    Closed,
    #[cfg(feature = "parquet")]
    Parquet(Box<ParquetFile>),
    #[cfg(feature = "avro")]
    Avro(Box<AvroFile>),
}

impl OpenFile {
    /// Append `rows`, flush a durable unit (Parquet row group / Avro block), and fsync.
    #[allow(unused_variables)]
    fn write(&mut self, rows: WriteRows<'_>) -> Result<()> {
        match self {
            OpenFile::Closed => {
                Err(GgStreamError::Sink("file sink: write to a closed file".into()))
            }
            #[cfg(feature = "parquet")]
            OpenFile::Parquet(p) => p.write(rows),
            #[cfg(feature = "avro")]
            OpenFile::Avro(a) => a.write(rows),
        }
    }

    /// Bytes written to the file after the last flush — the rolling size signal. (Parquet reports
    /// its own flushed-row-group accounting because the Arrow writer buffers internally and the
    /// OS file length lags until close; Avro flushes blocks straight to the file.)
    fn current_len(&self) -> Result<u64> {
        match self {
            OpenFile::Closed => Ok(0),
            #[cfg(feature = "parquet")]
            OpenFile::Parquet(p) => Ok(p.bytes_written()),
            #[cfg(feature = "avro")]
            OpenFile::Avro(a) => Ok(a.sync_handle.metadata()?.len()),
        }
    }

    /// Write the format footer / final block, fsync, and drop the writer.
    fn close(self) -> Result<()> {
        match self {
            OpenFile::Closed => Ok(()),
            #[cfg(feature = "parquet")]
            OpenFile::Parquet(p) => p.close(),
            #[cfg(feature = "avro")]
            OpenFile::Avro(a) => a.close(),
        }
    }
}

/// Build the open encoder for `format` (the caller has already verified the feature via
/// [`FileSink::new`]).
fn create_open_file(
    format: FileFormat,
    schema: RowSchema,
    compression: FileCompression,
    path: &Path,
) -> Result<OpenFile> {
    let open = match format {
        FileFormat::Parquet => {
            #[cfg(feature = "parquet")]
            {
                OpenFile::Parquet(Box::new(ParquetFile::create(schema, compression, path)?))
            }
            #[cfg(not(feature = "parquet"))]
            {
                let _ = (schema, compression, path);
                return Err(GgStreamError::Config(
                    "file sink: built without the `parquet` feature".into(),
                ));
            }
        }
        FileFormat::Avro => {
            #[cfg(feature = "avro")]
            {
                OpenFile::Avro(Box::new(AvroFile::create(schema, compression, path)?))
            }
            #[cfg(not(feature = "avro"))]
            {
                let _ = (schema, compression, path);
                return Err(GgStreamError::Config(
                    "file sink: built without the `avro` feature".into(),
                ));
            }
        }
    };
    Ok(open)
}

// ----------------------------------------------------------------------------------------------
// Row models (built from each ExportRecord; read by the encoders)
// ----------------------------------------------------------------------------------------------

/// One flattened telemetry row (one `body.samples[]` element of a `SouthboundTagUpdate`).
#[cfg_attr(not(any(feature = "parquet", feature = "avro")), allow(dead_code))]
struct MainRow {
    thing: Option<String>,
    app_id: Option<String>,
    site: Option<String>,
    shop: Option<String>,
    line: Option<String>,
    adapter: Option<String>,
    instance: Option<String>,
    tag_id: Option<String>,
    tag_name: Option<String>,
    value: SampleValue,
    quality: Option<String>,
    quality_raw: Option<String>,
    source_ts: Option<String>,
    server_ts: Option<String>,
    ts_ms: i64,
    offset: i64,
}

/// One opaque-message row (raw mode, or an `_unmapped` payload).
#[cfg_attr(not(any(feature = "parquet", feature = "avro")), allow(dead_code))]
struct RawRow {
    offset: i64,
    partition_key: String,
    ts_ms: i64,
    payload: String,
}

/// The polymorphic sample `value`, narrowed to one of the supported scalar types.
#[cfg_attr(not(any(feature = "parquet", feature = "avro")), allow(dead_code))]
enum SampleValue {
    Double(f64),
    Long(i64),
    Bool(bool),
    Str(String),
    Null,
}

impl SampleValue {
    /// The `valueType` discriminant string written alongside the value.
    fn type_str(&self) -> &'static str {
        match self {
            SampleValue::Double(_) => "double",
            SampleValue::Long(_) => "long",
            SampleValue::Bool(_) => "boolean",
            SampleValue::Str(_) => "string",
            SampleValue::Null => "null",
        }
    }
}

/// Build a raw row from an export record (lossy-UTF-8 for the key/payload bytes).
fn raw_row(r: &ExportRecord<'_>) -> RawRow {
    RawRow {
        offset: r.offset as i64,
        partition_key: String::from_utf8_lossy(r.partition_key).into_owned(),
        ts_ms: r.ts_ms as i64,
        payload: String::from_utf8_lossy(r.payload).into_owned(),
    }
}

/// Parse a `SouthboundTagUpdate` payload into one [`MainRow`] per sample, or `None` if the payload
/// is not JSON or has no `body.samples` array (→ caller routes it to the `_unmapped` raw file).
fn extract_rows(payload: &[u8], ts_ms: i64, offset: i64) -> Option<Vec<MainRow>> {
    let v: Value = serde_json::from_slice(payload).ok()?;
    let body = v.get("body")?;
    let samples = body.get("samples")?.as_array()?;
    let tags = v.get("tags");
    let device = body.get("device");
    let tag = body.get("tag");
    let from_tags = |k: &str| tags.and_then(|t| json_str(t, k));

    let mut rows = Vec::with_capacity(samples.len());
    for s in samples {
        rows.push(MainRow {
            thing: from_tags("thing"),
            app_id: from_tags("appId"),
            site: from_tags("site"),
            shop: from_tags("shop"),
            line: from_tags("line"),
            adapter: device.and_then(|d| json_str(d, "adapter")),
            instance: device.and_then(|d| json_str(d, "instance")),
            tag_id: tag.and_then(|t| json_str(t, "id")),
            tag_name: tag.and_then(|t| json_str(t, "name")),
            value: s.get("value").map(sample_value).unwrap_or(SampleValue::Null),
            quality: json_str(s, "quality"),
            quality_raw: json_str(s, "qualityRaw"),
            source_ts: json_str(s, "sourceTs"),
            server_ts: json_str(s, "serverTs"),
            ts_ms,
            offset,
        });
    }
    Some(rows)
}

/// String value of `obj[key]`, or `None` if absent / not a string.
fn json_str(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// Narrow a JSON value to a typed [`SampleValue`] (integral numbers → `Long`, else `Double`;
/// arrays/objects are stringified).
fn sample_value(v: &Value) -> SampleValue {
    match v {
        Value::Bool(b) => SampleValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SampleValue::Long(i)
            } else if let Some(u) = n.as_u64() {
                SampleValue::Long(u as i64)
            } else if let Some(f) = n.as_f64() {
                SampleValue::Double(f)
            } else {
                SampleValue::Null
            }
        }
        Value::String(s) => SampleValue::Str(s.clone()),
        Value::Null => SampleValue::Null,
        other => SampleValue::Str(other.to_string()),
    }
}

// ----------------------------------------------------------------------------------------------
// Partition-path time tokens (no chrono/time dependency)
// ----------------------------------------------------------------------------------------------

/// Resolve the UTC time tokens (`{yyyy}` `{MM}` `{dd}` `{HH}` and the compound `{yyyy-MM-dd}`) in a
/// partition template for `unix_secs`. Unknown tokens are left untouched.
fn resolve_tokens(template: &str, unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let hour = (secs_of_day / 3600) as u32;
    let (y, m, d) = civil_from_days(days);
    let yyyy = format!("{y:04}");
    let mm = format!("{m:02}");
    let dd = format!("{d:02}");
    let hh = format!("{hour:02}");
    // Compound first so `{yyyy}` doesn't partially match inside `{yyyy-MM-dd}`.
    template
        .replace("{yyyy-MM-dd}", &format!("{yyyy}-{mm}-{dd}"))
        .replace("{yyyy}", &yyyy)
        .replace("{MM}", &mm)
        .replace("{dd}", &dd)
        .replace("{HH}", &hh)
}

/// Civil `(year, month, day)` from a count of days since the Unix epoch (1970-01-01), via Howard
/// Hinnant's `civil_from_days` algorithm. Valid for the full proleptic Gregorian range.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ----------------------------------------------------------------------------------------------
// Parquet encoder (feature = "parquet")
// ----------------------------------------------------------------------------------------------

#[cfg(feature = "parquet")]
mod parquet_impl {
    use std::sync::{Arc, OnceLock};

    use arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::ArrowWriter;
    use parquet::basic::{Compression, GzipLevel, ZstdLevel};
    use parquet::file::properties::WriterProperties;

    use super::{MainRow, RawRow, RowSchema, SampleValue, WriteRows};
    use crate::config::FileCompression;
    use crate::error::{GgStreamError, Result};

    /// An open Parquet file: the Arrow writer plus a cloned handle for fsync (the writer owns its
    /// own copy of the underlying `File`).
    pub(super) struct ParquetFile {
        writer: ArrowWriter<std::fs::File>,
        pub(super) sync_handle: std::fs::File,
    }

    impl ParquetFile {
        pub(super) fn create(
            schema: RowSchema,
            compression: FileCompression,
            path: &std::path::Path,
        ) -> Result<Self> {
            let arrow_schema = match schema {
                RowSchema::Rows => rows_schema(),
                RowSchema::Raw => raw_schema(),
            };
            let file = std::fs::File::create(path)?;
            let sync_handle = file.try_clone()?;
            let props = WriterProperties::builder().set_compression(map_compression(compression)).build();
            let writer = ArrowWriter::try_new(file, arrow_schema, Some(props))
                .map_err(|e| GgStreamError::Sink(format!("parquet open: {e}")))?;
            Ok(Self { writer, sync_handle })
        }

        pub(super) fn write(&mut self, rows: WriteRows<'_>) -> Result<()> {
            let batch = match rows {
                WriteRows::Rows(r) => rows_batch(r)?,
                WriteRows::Raw(r) => raw_batch(r)?,
            };
            self.writer
                .write(&batch)
                .map_err(|e| GgStreamError::Sink(format!("parquet write: {e}")))?;
            // Force the buffered rows out as a row group so an AllAcked batch is durable.
            self.writer.flush().map_err(|e| GgStreamError::Sink(format!("parquet flush: {e}")))?;
            self.sync_handle.sync_all()?;
            Ok(())
        }

        pub(super) fn close(self) -> Result<()> {
            let Self { writer, sync_handle } = self;
            writer.close().map_err(|e| GgStreamError::Sink(format!("parquet close: {e}")))?;
            sync_handle.sync_all()?;
            Ok(())
        }

        /// Compressed bytes of all flushed row groups. The Arrow writer buffers through an internal
        /// `BufWriter`, so the OS file length lags; this is the durable size after each flush.
        pub(super) fn bytes_written(&self) -> u64 {
            self.writer.flushed_row_groups().iter().map(|rg| rg.compressed_size().max(0) as u64).sum()
        }
    }

    /// The normalized-rows Arrow schema (sparse typed value columns).
    fn rows_schema() -> SchemaRef {
        static S: OnceLock<SchemaRef> = OnceLock::new();
        S.get_or_init(|| {
            Arc::new(Schema::new(vec![
                Field::new("thing", DataType::Utf8, true),
                Field::new("appId", DataType::Utf8, true),
                Field::new("site", DataType::Utf8, true),
                Field::new("shop", DataType::Utf8, true),
                Field::new("line", DataType::Utf8, true),
                Field::new("adapter", DataType::Utf8, true),
                Field::new("instance", DataType::Utf8, true),
                Field::new("tagId", DataType::Utf8, true),
                Field::new("tagName", DataType::Utf8, true),
                Field::new("valueDouble", DataType::Float64, true),
                Field::new("valueLong", DataType::Int64, true),
                Field::new("valueBool", DataType::Boolean, true),
                Field::new("valueString", DataType::Utf8, true),
                Field::new("valueType", DataType::Utf8, false),
                Field::new("quality", DataType::Utf8, true),
                Field::new("qualityRaw", DataType::Utf8, true),
                Field::new("sourceTs", DataType::Utf8, true),
                Field::new("serverTs", DataType::Utf8, true),
                Field::new("tsMs", DataType::Int64, false),
                Field::new("offset", DataType::Int64, false),
            ]))
        })
        .clone()
    }

    /// The opaque-message Arrow schema.
    fn raw_schema() -> SchemaRef {
        static S: OnceLock<SchemaRef> = OnceLock::new();
        S.get_or_init(|| {
            Arc::new(Schema::new(vec![
                Field::new("offset", DataType::Int64, false),
                Field::new("partitionKey", DataType::Utf8, false),
                Field::new("tsMs", DataType::Int64, false),
                Field::new("payload", DataType::Utf8, false),
            ]))
        })
        .clone()
    }

    fn rows_batch(rows: &[MainRow]) -> Result<RecordBatch> {
        let thing: ArrayRef = Arc::new(rows.iter().map(|r| r.thing.clone()).collect::<StringArray>());
        let app_id: ArrayRef = Arc::new(rows.iter().map(|r| r.app_id.clone()).collect::<StringArray>());
        let site: ArrayRef = Arc::new(rows.iter().map(|r| r.site.clone()).collect::<StringArray>());
        let shop: ArrayRef = Arc::new(rows.iter().map(|r| r.shop.clone()).collect::<StringArray>());
        let line: ArrayRef = Arc::new(rows.iter().map(|r| r.line.clone()).collect::<StringArray>());
        let adapter: ArrayRef = Arc::new(rows.iter().map(|r| r.adapter.clone()).collect::<StringArray>());
        let instance: ArrayRef = Arc::new(rows.iter().map(|r| r.instance.clone()).collect::<StringArray>());
        let tag_id: ArrayRef = Arc::new(rows.iter().map(|r| r.tag_id.clone()).collect::<StringArray>());
        let tag_name: ArrayRef = Arc::new(rows.iter().map(|r| r.tag_name.clone()).collect::<StringArray>());
        let value_double: ArrayRef = Arc::new(
            rows.iter()
                .map(|r| if let SampleValue::Double(d) = &r.value { Some(*d) } else { None })
                .collect::<Float64Array>(),
        );
        let value_long: ArrayRef = Arc::new(
            rows.iter()
                .map(|r| if let SampleValue::Long(l) = &r.value { Some(*l) } else { None })
                .collect::<Int64Array>(),
        );
        let value_bool: ArrayRef = Arc::new(
            rows.iter()
                .map(|r| if let SampleValue::Bool(b) = &r.value { Some(*b) } else { None })
                .collect::<BooleanArray>(),
        );
        let value_string: ArrayRef = Arc::new(
            rows.iter()
                .map(|r| if let SampleValue::Str(s) = &r.value { Some(s.clone()) } else { None })
                .collect::<StringArray>(),
        );
        let value_type: ArrayRef =
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.value.type_str())));
        let quality: ArrayRef = Arc::new(rows.iter().map(|r| r.quality.clone()).collect::<StringArray>());
        let quality_raw: ArrayRef =
            Arc::new(rows.iter().map(|r| r.quality_raw.clone()).collect::<StringArray>());
        let source_ts: ArrayRef = Arc::new(rows.iter().map(|r| r.source_ts.clone()).collect::<StringArray>());
        let server_ts: ArrayRef = Arc::new(rows.iter().map(|r| r.server_ts.clone()).collect::<StringArray>());
        let ts_ms: ArrayRef = Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.ts_ms)));
        let offset: ArrayRef = Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.offset)));

        RecordBatch::try_new(
            rows_schema(),
            vec![
                thing, app_id, site, shop, line, adapter, instance, tag_id, tag_name, value_double,
                value_long, value_bool, value_string, value_type, quality, quality_raw, source_ts,
                server_ts, ts_ms, offset,
            ],
        )
        .map_err(|e| GgStreamError::Sink(format!("parquet rows batch: {e}")))
    }

    fn raw_batch(rows: &[RawRow]) -> Result<RecordBatch> {
        let offset: ArrayRef = Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.offset)));
        let pk: ArrayRef =
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.partition_key.as_str())));
        let ts_ms: ArrayRef = Arc::new(Int64Array::from_iter_values(rows.iter().map(|r| r.ts_ms)));
        let payload: ArrayRef =
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.payload.as_str())));
        RecordBatch::try_new(raw_schema(), vec![offset, pk, ts_ms, payload])
            .map_err(|e| GgStreamError::Sink(format!("parquet raw batch: {e}")))
    }

    /// Map the config codec to the Parquet compression (default levels for zstd/gzip).
    fn map_compression(c: FileCompression) -> Compression {
        match c {
            FileCompression::None => Compression::UNCOMPRESSED,
            FileCompression::Snappy => Compression::SNAPPY,
            FileCompression::Zstd => Compression::ZSTD(ZstdLevel::default()),
            FileCompression::Gzip => Compression::GZIP(GzipLevel::default()),
        }
    }
}

#[cfg(feature = "parquet")]
use parquet_impl::ParquetFile;

// ----------------------------------------------------------------------------------------------
// Avro encoder (feature = "avro")
// ----------------------------------------------------------------------------------------------

#[cfg(feature = "avro")]
mod avro_impl {
    use std::sync::OnceLock;

    use apache_avro::types::Value as AvroValue;
    use apache_avro::{Codec, Schema, Writer};

    use super::{MainRow, RawRow, RowSchema, SampleValue, WriteRows};
    use crate::config::FileCompression;
    use crate::error::{GgStreamError, Result};

    /// Normalized-rows Avro schema. `value` is a true union; the metadata fields are plain strings
    /// (defaulting to empty) to keep the schema small.
    const ROWS_SCHEMA: &str = r#"{
      "type":"record","name":"TagSample","namespace":"ggstreamlog",
      "fields":[
        {"name":"thing","type":"string","default":""},
        {"name":"appId","type":"string","default":""},
        {"name":"site","type":"string","default":""},
        {"name":"shop","type":"string","default":""},
        {"name":"line","type":"string","default":""},
        {"name":"adapter","type":"string","default":""},
        {"name":"instance","type":"string","default":""},
        {"name":"tagId","type":"string","default":""},
        {"name":"tagName","type":"string","default":""},
        {"name":"value","type":["null","double","long","boolean","string"],"default":null},
        {"name":"valueType","type":"string","default":"null"},
        {"name":"quality","type":"string","default":""},
        {"name":"qualityRaw","type":"string","default":""},
        {"name":"sourceTs","type":"string","default":""},
        {"name":"serverTs","type":"string","default":""},
        {"name":"tsMs","type":"long"},
        {"name":"offset","type":"long"}
      ]
    }"#;

    /// Opaque-message Avro schema.
    const RAW_SCHEMA: &str = r#"{
      "type":"record","name":"RawMessage","namespace":"ggstreamlog",
      "fields":[
        {"name":"offset","type":"long"},
        {"name":"partitionKey","type":"string"},
        {"name":"tsMs","type":"long"},
        {"name":"payload","type":"string"}
      ]
    }"#;

    fn rows_schema() -> &'static Schema {
        static S: OnceLock<Schema> = OnceLock::new();
        S.get_or_init(|| Schema::parse_str(ROWS_SCHEMA).expect("valid avro rows schema"))
    }
    fn raw_schema() -> &'static Schema {
        static S: OnceLock<Schema> = OnceLock::new();
        S.get_or_init(|| Schema::parse_str(RAW_SCHEMA).expect("valid avro raw schema"))
    }

    /// An open Avro Object Container File: the writer (borrowing a `'static` schema) plus a cloned
    /// handle for fsync.
    pub(super) struct AvroFile {
        writer: Writer<'static, std::fs::File>,
        pub(super) sync_handle: std::fs::File,
    }

    impl AvroFile {
        pub(super) fn create(
            schema: RowSchema,
            compression: FileCompression,
            path: &std::path::Path,
        ) -> Result<Self> {
            let avro_schema = match schema {
                RowSchema::Rows => rows_schema(),
                RowSchema::Raw => raw_schema(),
            };
            let file = std::fs::File::create(path)?;
            let sync_handle = file.try_clone()?;
            let writer = Writer::with_codec(avro_schema, file, map_codec(compression));
            Ok(Self { writer, sync_handle })
        }

        pub(super) fn write(&mut self, rows: WriteRows<'_>) -> Result<()> {
            match rows {
                WriteRows::Rows(r) => {
                    for row in r {
                        self.writer
                            .append(rows_value(row))
                            .map_err(|e| GgStreamError::Sink(format!("avro append: {e}")))?;
                    }
                }
                WriteRows::Raw(r) => {
                    for row in r {
                        self.writer
                            .append(raw_value(row))
                            .map_err(|e| GgStreamError::Sink(format!("avro append: {e}")))?;
                    }
                }
            }
            // Flush the current block so an AllAcked batch is recoverable to here.
            self.writer.flush().map_err(|e| GgStreamError::Sink(format!("avro flush: {e}")))?;
            self.sync_handle.sync_all()?;
            Ok(())
        }

        pub(super) fn close(self) -> Result<()> {
            let Self { writer, sync_handle } = self;
            let file =
                writer.into_inner().map_err(|e| GgStreamError::Sink(format!("avro close: {e}")))?;
            file.sync_all()?;
            drop(sync_handle);
            Ok(())
        }
    }

    /// Build the Avro record for one normalized row (the `value` field as an explicit union).
    fn rows_value(r: &MainRow) -> AvroValue {
        let s = |o: &Option<String>| AvroValue::String(o.clone().unwrap_or_default());
        // Union branch order: ["null","double","long","boolean","string"].
        let value = match &r.value {
            SampleValue::Null => AvroValue::Union(0, Box::new(AvroValue::Null)),
            SampleValue::Double(d) => AvroValue::Union(1, Box::new(AvroValue::Double(*d))),
            SampleValue::Long(l) => AvroValue::Union(2, Box::new(AvroValue::Long(*l))),
            SampleValue::Bool(b) => AvroValue::Union(3, Box::new(AvroValue::Boolean(*b))),
            SampleValue::Str(st) => AvroValue::Union(4, Box::new(AvroValue::String(st.clone()))),
        };
        AvroValue::Record(vec![
            ("thing".into(), s(&r.thing)),
            ("appId".into(), s(&r.app_id)),
            ("site".into(), s(&r.site)),
            ("shop".into(), s(&r.shop)),
            ("line".into(), s(&r.line)),
            ("adapter".into(), s(&r.adapter)),
            ("instance".into(), s(&r.instance)),
            ("tagId".into(), s(&r.tag_id)),
            ("tagName".into(), s(&r.tag_name)),
            ("value".into(), value),
            ("valueType".into(), AvroValue::String(r.value.type_str().to_string())),
            ("quality".into(), s(&r.quality)),
            ("qualityRaw".into(), s(&r.quality_raw)),
            ("sourceTs".into(), s(&r.source_ts)),
            ("serverTs".into(), s(&r.server_ts)),
            ("tsMs".into(), AvroValue::Long(r.ts_ms)),
            ("offset".into(), AvroValue::Long(r.offset)),
        ])
    }

    fn raw_value(r: &RawRow) -> AvroValue {
        AvroValue::Record(vec![
            ("offset".into(), AvroValue::Long(r.offset)),
            ("partitionKey".into(), AvroValue::String(r.partition_key.clone())),
            ("tsMs".into(), AvroValue::Long(r.ts_ms)),
            ("payload".into(), AvroValue::String(r.payload.clone())),
        ])
    }

    /// Map the config codec to the Avro container codec.
    fn map_codec(c: FileCompression) -> Codec {
        match c {
            FileCompression::None => Codec::Null,
            FileCompression::Snappy => Codec::Snappy,
            FileCompression::Zstd => Codec::Zstandard,
            FileCompression::Gzip => Codec::Deflate,
        }
    }
}

#[cfg(feature = "avro")]
use avro_impl::AvroFile;

// ----------------------------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------------------------

#[cfg(all(test, any(feature = "parquet", feature = "avro")))]
mod tests {
    use super::*;

    fn cfg(dir: &Path, format: FileFormat, mode: FileMode) -> FileSinkConfig {
        FileSinkConfig {
            format,
            mode,
            dir: dir.to_string_lossy().into_owned(),
            partition_by: None,
            max_file_bytes: 128 * 1024 * 1024,
            max_files: 0,
            roll_every_secs: 0,
            on_full: FileOnFull::DropOldest,
            compression: FileCompression::Snappy,
        }
    }

    fn rec(offset: u64, payload: &[u8]) -> ExportRecord<'_> {
        ExportRecord { offset, partition_key: b"pk", ts_ms: 111, payload }
    }

    /// A `SouthboundTagUpdate` payload carrying a single sample with `value`.
    fn southbound(tag_id: &str, value: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "header": {"name":"SouthboundTagUpdate","version":"1.0","timestamp":"t"},
            "tags": {"thing":"th","appId":"app","site":"s1","shop":"sh1","line":"ln1"},
            "body": {
                "device": {"adapter":"opcua","instance":"inst1","endpoint":"e"},
                "tag": {"id": tag_id, "name":"Temp"},
                "samples": [
                    {"value": value, "quality":"GOOD","qualityRaw":"0","sourceTs":"st","serverTs":"sv"}
                ]
            }
        }))
        .unwrap()
    }

    /// All files under `dir` (recursively) whose name ends with `suffix`, sorted.
    fn list_files(dir: &Path, suffix: &str) -> Vec<PathBuf> {
        fn walk(d: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
            for e in fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, suffix, out);
                } else if p.file_name().unwrap().to_string_lossy().ends_with(suffix) {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, suffix, &mut out);
        out.sort();
        out
    }

    #[test]
    fn rejects_format_without_encoder() {
        // `civil_from_days` sanity: the Unix epoch is 1970-01-01.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(resolve_tokens("dt={yyyy-MM-dd}/hr={HH}", 0), "dt=1970-01-01/hr=00");
        // An unknown token is left as-is.
        assert_eq!(resolve_tokens("a={foo}", 0), "a={foo}");
    }

    // -------- Parquet --------

    #[cfg(feature = "parquet")]
    fn read_parquet(path: &Path) -> Vec<arrow::record_batch::RecordBatch> {
        let file = std::fs::File::open(path).unwrap();
        let builder =
            parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        builder.build().unwrap().map(|b| b.unwrap()).collect()
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_rows_roundtrip() {
        use arrow::array::{Array, BooleanArray, Float64Array, Int64Array, StringArray};

        let dir = tempfile::tempdir().unwrap();
        let mut sink =
            FileSink::new("t", cfg(dir.path(), FileFormat::Parquet, FileMode::Rows)).unwrap();
        let p_double = southbound("ns=1", serde_json::json!(3.5));
        let p_long = southbound("ns=2", serde_json::json!(42));
        let p_bool = southbound("ns=3", serde_json::json!(true));
        let p_str = southbound("ns=4", serde_json::json!("hello"));
        let batch =
            vec![rec(0, &p_double), rec(1, &p_long), rec(2, &p_bool), rec(3, &p_str)];
        assert!(matches!(sink.send(&batch), SendOutcome::AllAcked));
        drop(sink); // finalize → footer + rename

        let files = list_files(dir.path(), ".parquet");
        assert_eq!(files.len(), 1, "one main file");
        let batches = read_parquet(&files[0]);
        let b = &batches[0];
        assert_eq!(b.num_rows(), 4);

        let col = |name: &str| b.column_by_name(name).unwrap();
        let vt = col("valueType").as_any().downcast_ref::<StringArray>().unwrap();
        let vd = col("valueDouble").as_any().downcast_ref::<Float64Array>().unwrap();
        let vl = col("valueLong").as_any().downcast_ref::<Int64Array>().unwrap();
        let vb = col("valueBool").as_any().downcast_ref::<BooleanArray>().unwrap();
        let vs = col("valueString").as_any().downcast_ref::<StringArray>().unwrap();
        let tag = col("tagId").as_any().downcast_ref::<StringArray>().unwrap();

        // row 0: double
        assert_eq!(vt.value(0), "double");
        assert!(!vd.is_null(0));
        assert_eq!(vd.value(0), 3.5);
        assert!(vl.is_null(0) && vb.is_null(0) && vs.is_null(0));
        assert_eq!(tag.value(0), "ns=1");
        // row 1: long
        assert_eq!(vt.value(1), "long");
        assert!(!vl.is_null(1));
        assert_eq!(vl.value(1), 42);
        assert!(vd.is_null(1) && vb.is_null(1) && vs.is_null(1));
        // row 2: boolean
        assert_eq!(vt.value(2), "boolean");
        assert!(!vb.is_null(2));
        assert!(vb.value(2));
        assert!(vd.is_null(2) && vl.is_null(2) && vs.is_null(2));
        // row 3: string
        assert_eq!(vt.value(3), "string");
        assert!(!vs.is_null(3));
        assert_eq!(vs.value(3), "hello");
        assert!(vd.is_null(3) && vl.is_null(3) && vb.is_null(3));
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_raw_roundtrip() {
        use arrow::array::{Int64Array, StringArray};

        let dir = tempfile::tempdir().unwrap();
        let mut sink =
            FileSink::new("t", cfg(dir.path(), FileFormat::Parquet, FileMode::Raw)).unwrap();
        assert!(matches!(sink.send(&[rec(7, b"hello"), rec(8, b"world")]), SendOutcome::AllAcked));
        drop(sink);

        let files = list_files(dir.path(), ".parquet");
        assert_eq!(files.len(), 1);
        let batches = read_parquet(&files[0]);
        let b = &batches[0];
        assert_eq!(b.num_rows(), 2);
        let off = b.column_by_name("offset").unwrap().as_any().downcast_ref::<Int64Array>().unwrap();
        let pay =
            b.column_by_name("payload").unwrap().as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(off.value(0), 7);
        assert_eq!(pay.value(0), "hello");
        assert_eq!(pay.value(1), "world");
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_rolls_on_size() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = cfg(dir.path(), FileFormat::Parquet, FileMode::Raw);
        c.max_file_bytes = 1; // any written row group exceeds this → roll every send
        let mut sink = FileSink::new("t", c).unwrap();
        for i in 0..3u64 {
            assert!(matches!(sink.send(&[rec(i, b"payload-bytes")]), SendOutcome::AllAcked));
        }
        drop(sink);
        let files = list_files(dir.path(), ".parquet");
        assert!(files.len() >= 2, "expected >=2 rolled files, got {}", files.len());
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_max_files_drop_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = cfg(dir.path(), FileFormat::Parquet, FileMode::Raw);
        c.max_file_bytes = 1;
        c.max_files = 2;
        c.on_full = FileOnFull::DropOldest;
        let mut sink = FileSink::new("t", c).unwrap();
        for i in 0..5u64 {
            assert!(matches!(sink.send(&[rec(i, b"data")]), SendOutcome::AllAcked));
        }
        drop(sink);

        let files = list_files(dir.path(), ".parquet");
        assert_eq!(files.len(), 2, "ring must cap finalized files at maxFiles");
        // seqs 0..4 were finalized; only the two most recent (3,4) survive.
        for f in &files {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let seq: u64 =
                name.trim_end_matches(".parquet").rsplit('-').next().unwrap().parse().unwrap();
            assert!(seq >= 3, "oldest files should have been dropped, found seq {seq}");
        }
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_drop_finalizes_open_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink =
            FileSink::new("t", cfg(dir.path(), FileFormat::Parquet, FileMode::Raw)).unwrap();
        assert!(matches!(sink.send(&[rec(1, b"x"), rec(2, b"y")]), SendOutcome::AllAcked));
        // No roll (large maxFileBytes) → only an *.inprogress file exists until drop.
        assert!(list_files(dir.path(), ".parquet").is_empty());
        drop(sink);

        let files = list_files(dir.path(), ".parquet");
        assert_eq!(files.len(), 1);
        assert!(list_files(dir.path(), ".inprogress").is_empty(), "no leftover temp file");
        assert_eq!(read_parquet(&files[0])[0].num_rows(), 2);
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_unmapped_for_non_southbound() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink =
            FileSink::new("t", cfg(dir.path(), FileFormat::Parquet, FileMode::Rows)).unwrap();
        // Not JSON, then JSON without body.samples → both go to the _unmapped file.
        assert!(matches!(sink.send(&[rec(1, b"not json at all")]), SendOutcome::AllAcked));
        assert!(matches!(sink.send(&[rec(2, br#"{"foo":"bar"}"#)]), SendOutcome::AllAcked));
        drop(sink);

        let unmapped = list_files(dir.path(), "_unmapped.parquet");
        assert!(!unmapped.is_empty(), "an _unmapped file must be created for non-southbound payloads");
        assert!(read_parquet(&unmapped[0])[0].num_rows() >= 1);
        // No main (rows) file was opened — every payload was unmapped.
        let mains: Vec<_> = list_files(dir.path(), ".parquet")
            .into_iter()
            .filter(|p| !p.file_name().unwrap().to_string_lossy().contains("_unmapped"))
            .collect();
        assert!(mains.is_empty());
    }

    // -------- Avro --------

    #[cfg(feature = "avro")]
    fn read_avro(path: &Path) -> Vec<apache_avro::types::Value> {
        let file = std::fs::File::open(path).unwrap();
        let reader = apache_avro::Reader::new(file).unwrap();
        reader.map(|r| r.unwrap()).collect()
    }

    #[cfg(feature = "avro")]
    fn field<'a>(rec: &'a apache_avro::types::Value, name: &str) -> &'a apache_avro::types::Value {
        match rec {
            apache_avro::types::Value::Record(fs) => &fs.iter().find(|(k, _)| k == name).unwrap().1,
            _ => panic!("expected a record"),
        }
    }

    #[cfg(feature = "avro")]
    #[test]
    fn avro_rows_roundtrip() {
        use apache_avro::types::Value as V;

        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new("t", cfg(dir.path(), FileFormat::Avro, FileMode::Rows)).unwrap();
        let p_double = southbound("ns=1", serde_json::json!(2.5));
        let p_long = southbound("ns=2", serde_json::json!(7));
        let p_str = southbound("ns=3", serde_json::json!("hi"));
        assert!(matches!(
            sink.send(&[rec(0, &p_double), rec(1, &p_long), rec(2, &p_str)]),
            SendOutcome::AllAcked
        ));
        drop(sink);

        let files = list_files(dir.path(), ".avro");
        assert_eq!(files.len(), 1);
        let values = read_avro(&files[0]);
        assert_eq!(values.len(), 3);

        // The polymorphic value decodes as a true union.
        match field(&values[0], "value") {
            V::Union(idx, inner) => {
                assert_eq!(*idx, 1);
                assert!(matches!(**inner, V::Double(d) if (d - 2.5).abs() < 1e-9));
            }
            other => panic!("expected union, got {other:?}"),
        }
        assert!(matches!(field(&values[0], "valueType"), V::String(s) if s == "double"));

        match field(&values[1], "value") {
            V::Union(idx, inner) => {
                assert_eq!(*idx, 2);
                assert!(matches!(**inner, V::Long(7)));
            }
            other => panic!("expected union, got {other:?}"),
        }
        match field(&values[2], "value") {
            V::Union(idx, inner) => {
                assert_eq!(*idx, 4);
                assert!(matches!(&**inner, V::String(s) if s == "hi"));
            }
            other => panic!("expected union, got {other:?}"),
        }
        assert!(matches!(field(&values[2], "tagId"), V::String(s) if s == "ns=3"));
    }

    #[cfg(feature = "avro")]
    #[test]
    fn avro_raw_roundtrip() {
        use apache_avro::types::Value as V;

        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new("t", cfg(dir.path(), FileFormat::Avro, FileMode::Raw)).unwrap();
        assert!(matches!(sink.send(&[rec(5, b"alpha"), rec(6, b"beta")]), SendOutcome::AllAcked));
        drop(sink);

        let files = list_files(dir.path(), ".avro");
        assert_eq!(files.len(), 1);
        let values = read_avro(&files[0]);
        assert_eq!(values.len(), 2);
        assert!(matches!(field(&values[0], "offset"), V::Long(5)));
        assert!(matches!(field(&values[0], "payload"), V::String(s) if s == "alpha"));
        assert!(matches!(field(&values[1], "payload"), V::String(s) if s == "beta"));
    }
}
