//! Configuration types (serde; map to the YAML schema in the design doc). Phase 1 covers
//! the buffer; batch/delivery/sink config arrive with the export milestones.

use serde::{Deserialize, Serialize};

use crate::error::{GgStreamError, Result};

/// Backpressure policy when the on-disk budget is exceeded with un-delivered data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnFull {
    /// Drop the oldest data to stay within budget (telemetry default; never blocks producers).
    DropOldest,
    /// Block the producer until the exporter delivers + reclaims space (lossless).
    Block,
    /// Reject new appends while over budget.
    RejectNew,
}
impl Default for OnFull {
    fn default() -> Self {
        OnFull::DropOldest
    }
}

/// Durability ↔ throughput dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FsyncPolicy {
    /// fsync per append_batch + on the interval timer (default).
    PerBatch,
    /// fsync only on the interval timer (widest crash window, fastest).
    Interval,
    /// fsync every record (safest, slowest).
    Always,
}
impl Default for FsyncPolicy {
    fn default() -> Self {
        FsyncPolicy::PerBatch
    }
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
        }
    }
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
