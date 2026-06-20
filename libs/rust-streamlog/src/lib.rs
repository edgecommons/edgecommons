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
//!
//! The C ABI for Phase-2 language bindings is finalized (design only) in
//! `include/ggstreamlog.h`; the Rust implementation lands in Phase 2.

pub mod blockstore;
pub mod config;
pub mod error;
pub mod export;
pub mod log;
pub mod record;

pub use config::{
    BatchConfig, BufferConfig, Compression, DeliveryConfig, FsyncPolicy, OnFull, SinkConfig,
};
pub use error::{GgStreamError, Result};
pub use export::{
    EngineStats, ExportEngine, ExportRecord, FakeSink, FakeSinkHandle, SendOutcome, Sink,
};
#[cfg(feature = "kinesis")]
pub use export::KinesisSink;
pub use log::{EmbeddedLog, LogStats};
pub use record::Record;
