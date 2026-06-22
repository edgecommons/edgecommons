//! In-memory, **non-durable** [`BlockStore`]: a bounded ring of records held in RAM.
//!
//! Records are lost on component restart/crash and never touch disk. Used for best-effort streams
//! (`StoreType::Memory`) where durability / high QoS is unnecessary (cheap telemetry, debug
//! traces): no disk I/O, no recovery. Retention and `onFull` are driven by the upper [`crate::log`]
//! layer exactly as for the disk store — `disk_bytes()` here reports the in-RAM byte total, and
//! `truncate_below()` frees delivered records.

use std::collections::VecDeque;

use super::{BlockStore, Checkpoint, OwnedRecord};
use crate::error::Result;

/// Approximate per-record memory overhead beyond the payload + partition-key bytes (offset, ts,
/// length prefixes, Vec headers). Keeps `disk_bytes()` a useful budget signal.
const RECORD_OVERHEAD: u64 = 24;

fn record_bytes(pk: &[u8], payload: &[u8]) -> u64 {
    pk.len() as u64 + payload.len() as u64 + RECORD_OVERHEAD
}

/// Non-durable in-RAM record store. Holds a contiguous run of offsets `[front.offset, next_offset)`.
pub struct MemoryBlockStore {
    records: VecDeque<OwnedRecord>,
    next_offset: u64,
    bytes: u64,
}

impl MemoryBlockStore {
    pub fn new() -> Self {
        Self { records: VecDeque::new(), next_offset: 0, bytes: 0 }
    }
}

impl Default for MemoryBlockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockStore for MemoryBlockStore {
    fn next_offset(&self) -> u64 {
        self.next_offset
    }

    fn append(&mut self, offset: u64, ts_ms: u64, pk: &[u8], payload: &[u8]) -> Result<()> {
        debug_assert_eq!(offset, self.next_offset, "append offset must equal next_offset");
        self.bytes += record_bytes(pk, payload);
        self.records.push_back(OwnedRecord {
            offset,
            ts_ms,
            partition_key: pk.to_vec(),
            payload: payload.to_vec(),
        });
        self.next_offset = offset + 1;
        Ok(())
    }

    /// No-op: nothing is buffered outside RAM.
    fn flush_os(&mut self) -> Result<()> {
        Ok(())
    }

    /// No-op: non-durable by design (there is no fsync target).
    fn sync(&mut self) -> Result<()> {
        Ok(())
    }

    fn read_from(&mut self, from: u64, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        let mut out = Vec::new();
        if max_records == 0 || from >= self.next_offset {
            return Ok(out);
        }
        let base = self.records.front().map(|r| r.offset).unwrap_or(self.next_offset);
        // Older records may have been truncated after delivery; never read below what's retained.
        let start = from.max(base);
        let skip = (start - base) as usize;
        let mut bytes = 0usize;
        for rec in self.records.iter().skip(skip) {
            if out.len() >= max_records {
                break;
            }
            let sz = rec.partition_key.len() + rec.payload.len();
            // Always include at least one record so a single oversized record still drains.
            if !out.is_empty() && bytes + sz > max_bytes {
                break;
            }
            bytes += sz;
            out.push(rec.clone());
        }
        Ok(out)
    }

    fn truncate_below(&mut self, offset: u64) -> Result<u64> {
        let mut reclaimed = 0u64;
        while let Some(front) = self.records.front() {
            if front.offset >= offset {
                break;
            }
            let r = self.records.pop_front().expect("front exists");
            let b = record_bytes(&r.partition_key, &r.payload);
            reclaimed += b;
            self.bytes = self.bytes.saturating_sub(b);
        }
        Ok(reclaimed)
    }

    /// Non-durable: nothing persists. On restart the store is empty and the cursor resets to 0
    /// (everything was lost — that's the contract).
    fn load_checkpoint(&self) -> Result<Checkpoint> {
        Ok(Checkpoint::default())
    }

    fn store_checkpoint(&mut self, _cp: Checkpoint) -> Result<()> {
        Ok(())
    }

    fn disk_bytes(&self) -> u64 {
        self.bytes
    }

    fn oldest_ts_ms(&self) -> Option<u64> {
        self.records.front().map(|r| r.ts_ms)
    }

    /// Drop the single oldest record (truncate below front.offset + 1), or `None` if empty.
    fn next_drop_boundary(&self) -> Option<u64> {
        self.records.front().map(|r| r.offset + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(s: &mut MemoryBlockStore, payload: &[u8]) {
        let off = s.next_offset();
        s.append(off, 1000 + off, b"pk", payload).unwrap();
    }

    #[test]
    fn append_read_truncate_and_bytes() {
        let mut s = MemoryBlockStore::new();
        assert_eq!(s.next_offset(), 0);
        assert_eq!(s.disk_bytes(), 0);
        assert!(s.oldest_ts_ms().is_none());

        rec(&mut s, b"a");
        rec(&mut s, b"bb");
        rec(&mut s, b"ccc");
        assert_eq!(s.next_offset(), 3);
        assert!(s.disk_bytes() > 0);
        assert_eq!(s.oldest_ts_ms(), Some(1000));

        // Read from 0: all three, in order.
        let all = s.read_from(0, 100, usize::MAX).unwrap();
        assert_eq!(all.iter().map(|r| r.offset).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(all[1].payload, b"bb");

        // max_records bound.
        assert_eq!(s.read_from(0, 2, usize::MAX).unwrap().len(), 2);

        // Truncate below 2 drops offsets 0,1; reads clamp to retained data.
        let reclaimed = s.truncate_below(2).unwrap();
        assert!(reclaimed > 0);
        let after = s.read_from(0, 100, usize::MAX).unwrap();
        assert_eq!(after.iter().map(|r| r.offset).collect::<Vec<_>>(), vec![2]);
        assert_eq!(s.oldest_ts_ms(), Some(1002));

        // Reading at/after next_offset yields nothing; sync/flush are no-ops; checkpoint is default.
        assert!(s.read_from(3, 100, usize::MAX).unwrap().is_empty());
        s.sync().unwrap();
        s.flush_os().unwrap();
        assert_eq!(s.load_checkpoint().unwrap(), Checkpoint::default());
    }

    #[test]
    fn read_from_includes_at_least_one_when_oversized() {
        let mut s = MemoryBlockStore::new();
        rec(&mut s, b"hello");
        // max_bytes smaller than the record: still returns the one record.
        let got = s.read_from(0, 10, 1).unwrap();
        assert_eq!(got.len(), 1);
    }
}
