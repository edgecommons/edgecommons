//! # ggstreamlog
//!
//! Durable, store-and-forward telemetry log + export engine for ggcommons (Phase 1 core).
//! See `docs/TELEMETRY_STREAMING_PHASE1.md`.
//!
//! Phase 1 layers (built bottom-up):
//! - [`record`] — record model + on-disk frame format.
//! - [`blockstore`] — the durability seam ([`blockstore::segment_log::SegmentLog`]).
//! - [`log`] — the embedded buffer (retention, backpressure, fsync, export cursor).
//! - (next) `export` — the export engine + sinks.

pub mod blockstore;
pub mod config;
pub mod error;
pub mod log;
pub mod record;

pub use config::{BufferConfig, FsyncPolicy, OnFull};
pub use error::{GgStreamError, Result};
pub use log::{EmbeddedLog, LogStats};
pub use record::Record;
