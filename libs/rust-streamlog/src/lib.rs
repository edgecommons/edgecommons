//! # ggstreamlog
//!
//! Durable, store-and-forward telemetry log + export engine for ggcommons (Phase 1 core).
//! See `docs/TELEMETRY_STREAMING_PHASE1.md`.
//!
//! Phase 1 layers (built bottom-up):
//! - [`record`] — record model + on-disk frame format.
//! - [`blockstore`] — the durability seam ([`blockstore::segment_log::SegmentLog`]).
//! - (next) `log` — the embedded log (writer thread, retention, backpressure).
//! - (next) `export` — the export engine + sinks.

pub mod blockstore;
pub mod error;
pub mod record;

pub use error::{GgStreamError, Result};
pub use record::Record;
