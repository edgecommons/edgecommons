//! The durability seam: a `BlockStore` is a durable, ordered, append-only record store
//! with a single delivery cursor (checkpoint) and size/age retention. Phase 1 ships the
//! hand-rolled [`segment_log::SegmentLog`]; RocksDB/LMDB backends can implement the same
//! trait later.

pub mod checkpoint;
pub mod segment_log;

pub use checkpoint::Checkpoint;

use crate::error::Result;

/// A record read back from the store (owned; copied once into an export batch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedRecord {
    pub offset: u64,
    pub ts_ms: u64,
    pub partition_key: Vec<u8>,
    pub payload: Vec<u8>,
}

/// Result of opening/recovering a store.
#[derive(Debug, Clone, Copy)]
pub struct RecoveryReport {
    /// The next offset to assign on append (i.e. one past the last durable record).
    pub next_offset: u64,
    /// Whether a torn tail record was truncated during recovery.
    pub torn_truncated: bool,
    /// Number of segments present after recovery.
    pub segments: usize,
}

/// Durable, ordered, append-only byte store with a delivery cursor + retention.
///
/// Implementations own framing/CRC/segmentation so the upper [`crate::log`] layer is
/// storage-agnostic. Not internally synchronized — the log drives it from a single writer.
pub trait BlockStore: Send {
    /// One past the last durable offset (next to assign).
    fn next_offset(&self) -> u64;

    /// Append a record at `offset` (must equal [`next_offset`]).
    fn append(&mut self, offset: u64, ts_ms: u64, pk: &[u8], payload: &[u8]) -> Result<()>;

    /// Flush buffered writes to the OS so readers can see them (no fsync).
    fn flush_os(&mut self) -> Result<()>;

    /// fsync durable.
    fn sync(&mut self) -> Result<()>;

    /// Read records starting at `from` (inclusive), bounded by `max_records`/`max_bytes`.
    ///
    /// Takes `&mut self` so the implementation may build/cache a byte-offset index on demand
    /// (the export read path must seek, not rescan, to keep ingest+drain concurrent).
    fn read_from(&mut self, from: u64, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>>;

    /// Delete segments entirely below `offset`; returns bytes reclaimed.
    fn truncate_below(&mut self, offset: u64) -> Result<u64>;

    fn load_checkpoint(&self) -> Result<Checkpoint>;
    fn store_checkpoint(&mut self, cp: Checkpoint) -> Result<()>;

    /// Total bytes currently on disk across segments.
    fn disk_bytes(&self) -> u64;
    /// Timestamp of the oldest retained record, if any.
    fn oldest_ts_ms(&self) -> Option<u64>;
}
