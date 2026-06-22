//! The durability seam: a `BlockStore` is a durable, ordered, append-only record store
//! with a single delivery cursor (checkpoint) and size/age retention. Phase 1 ships the
//! hand-rolled [`segment_log::SegmentLog`]; RocksDB/LMDB backends can implement the same
//! trait later.

pub mod checkpoint;
pub mod memory;
pub mod segment_log;

pub use checkpoint::Checkpoint;
pub use memory::MemoryBlockStore;
pub use segment_log::SegmentLog;

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

    /// The offset to [`truncate_below`] to free the oldest reclaimable unit (a whole segment for
    /// the disk store, a single record for the memory store), or `None` if nothing can be dropped
    /// (disk: only the active segment remains; memory: empty). Drives `onFull: dropOldest`.
    fn next_drop_boundary(&self) -> Option<u64>;
}

/// The configured backing store for one stream's buffer: durable [`SegmentLog`] (disk) or
/// non-durable [`MemoryBlockStore`] (RAM). A single enum keeps the upper [`crate::log`] layer
/// monomorphic while letting the disk path retain its specialized off-lock read planning.
pub enum BackingStore {
    Disk(SegmentLog),
    Memory(MemoryBlockStore),
}

impl BlockStore for BackingStore {
    fn next_offset(&self) -> u64 {
        match self {
            Self::Disk(s) => s.next_offset(),
            Self::Memory(s) => s.next_offset(),
        }
    }

    fn append(&mut self, offset: u64, ts_ms: u64, pk: &[u8], payload: &[u8]) -> Result<()> {
        match self {
            Self::Disk(s) => s.append(offset, ts_ms, pk, payload),
            Self::Memory(s) => s.append(offset, ts_ms, pk, payload),
        }
    }

    fn flush_os(&mut self) -> Result<()> {
        match self {
            Self::Disk(s) => s.flush_os(),
            Self::Memory(s) => s.flush_os(),
        }
    }

    fn sync(&mut self) -> Result<()> {
        match self {
            Self::Disk(s) => s.sync(),
            Self::Memory(s) => s.sync(),
        }
    }

    fn read_from(&mut self, from: u64, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        match self {
            Self::Disk(s) => s.read_from(from, max_records, max_bytes),
            Self::Memory(s) => s.read_from(from, max_records, max_bytes),
        }
    }

    fn truncate_below(&mut self, offset: u64) -> Result<u64> {
        match self {
            Self::Disk(s) => s.truncate_below(offset),
            Self::Memory(s) => s.truncate_below(offset),
        }
    }

    fn load_checkpoint(&self) -> Result<Checkpoint> {
        match self {
            Self::Disk(s) => s.load_checkpoint(),
            Self::Memory(s) => s.load_checkpoint(),
        }
    }

    fn store_checkpoint(&mut self, cp: Checkpoint) -> Result<()> {
        match self {
            Self::Disk(s) => s.store_checkpoint(cp),
            Self::Memory(s) => s.store_checkpoint(cp),
        }
    }

    fn disk_bytes(&self) -> u64 {
        match self {
            Self::Disk(s) => s.disk_bytes(),
            Self::Memory(s) => s.disk_bytes(),
        }
    }

    fn oldest_ts_ms(&self) -> Option<u64> {
        match self {
            Self::Disk(s) => s.oldest_ts_ms(),
            Self::Memory(s) => s.oldest_ts_ms(),
        }
    }

    fn next_drop_boundary(&self) -> Option<u64> {
        match self {
            Self::Disk(s) => s.next_drop_boundary(),
            Self::Memory(s) => s.next_drop_boundary(),
        }
    }
}
