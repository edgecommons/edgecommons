//! # ggstreamlog
//!
//! Durable, store-and-forward telemetry log + export engine for ggcommons (Phase 1 core).
//! See `docs/TELEMETRY_STREAMING_PHASE1.md`.
//!
//! Phase 1 layers (built bottom-up):
//! - [`record`] — record model + on-disk frame format.
//! - [`blockstore`] — the durability seam ([`blockstore::segment_log::SegmentLog`]).
//! - [`log`] — the embedded buffer (retention, backpressure, fsync, export cursor).
//! - [`export`] — the export engine + the [`export::Sink`] seam.
//! - [`service`] — [`service::StreamService`], the config-driven orchestration shared by the Rust
//!   lib, the C-ABI bindings, and tests.
//!
//! The C ABI for the Phase-2 language bindings lives in [`ffi`] (feature `cabi`, building a
//! `cdylib`); its header is `include/ggstreamlog.h`.

pub mod blockstore;
pub mod config;
pub mod error;
pub mod export;
#[cfg(feature = "cabi")]
pub mod ffi;
pub mod log;
pub mod record;
pub mod service;

pub use config::{
    BatchConfig, BufferConfig, ColumnSpec, ColumnType, Compression, DeliveryConfig, FileCompression,
    FileFormat, FileMode, FileOnFull, FileSinkConfig, FsyncPolicy, OnFull, RowsConfig, SinkConfig,
    StreamConfig, StreamingConfig,
};
pub use error::{GgStreamError, Result};
pub use export::{
    CallbackSink, EngineStats, ExportEngine, ExportRecord, FakeSink, FakeSinkHandle, SendOutcome,
    Sink, SinkCallback,
};
#[cfg(feature = "kinesis")]
pub use export::KinesisSink;
#[cfg(feature = "kafka")]
pub use export::KafkaSink;
#[cfg(feature = "file")]
pub use export::FileSink;
pub use log::{EmbeddedLog, LogStats};
pub use record::Record;
pub use service::{ServiceStats, SinkFactory, StreamService};
