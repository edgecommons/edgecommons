//! `EmbeddedLog` — the durable buffer over a [`SegmentLog`]: ordered append, fsync policy,
//! disk retention + backpressure (`DropOldest`/`Block`/`RejectNew`), and the export-facing
//! `read_batch`/`commit` cursor.
//!
//! Phase 1 uses synchronous append under a mutex + a background fsync timer. A
//! writer-thread + bounded ingest queue (to decouple append latency from fsync and batch
//! group-commits) is a later perf pass — see `docs/TELEMETRY_STREAMING_PHASE1.md` §15.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::blockstore::segment_log::SegmentLog;
use crate::blockstore::{BlockStore, Checkpoint, OwnedRecord};
use crate::config::{BufferConfig, FsyncPolicy, OnFull};
use crate::error::{GgStreamError, Result};
use crate::record::{self, Record};

/// A point-in-time view of a stream's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogStats {
    pub appended_total: u64,
    pub dropped_total: u64,
    pub disk_bytes: u64,
    pub next_offset: u64,
    pub acked: u64,
    /// Un-delivered records currently buffered (`next_offset - acked`).
    pub backlog: u64,
    /// Age of the oldest retained record (0 if empty).
    pub oldest_unacked_age_ms: u64,
}

struct Inner {
    store: SegmentLog,
    cfg: BufferConfig,
    drop_floor: u64,
    acked: u64,
    appended: u64,
    dropped: u64,
}

/// Durable store-and-forward buffer for one stream.
pub struct EmbeddedLog {
    shared: Arc<(Mutex<Inner>, Condvar)>,
    stop: Arc<AtomicBool>,
    timer: Option<JoinHandle<()>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl EmbeddedLog {
    /// Open + recover the buffer at `cfg.path`.
    pub fn open(cfg: BufferConfig) -> Result<Self> {
        cfg.validate()?;
        let store = SegmentLog::open(&cfg.path, cfg.segment_bytes)?;
        let cp = store.load_checkpoint()?;
        // A checkpoint can name an offset beyond what survived recovery (e.g. torn tail);
        // clamp the cursor to what's actually present.
        let acked = cp.acked.min(store.next_offset());
        let inner = Inner {
            store,
            drop_floor: cp.drop_floor,
            acked,
            appended: 0,
            dropped: 0,
            cfg: cfg.clone(),
        };
        let shared = Arc::new((Mutex::new(inner), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));

        // Background fsync timer (PerBatch/Interval get periodic durability; Always fsyncs inline).
        let timer = if cfg.fsync != FsyncPolicy::Always {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            let interval = cfg.fsync_interval_ms.max(1);
            Some(std::thread::spawn(move || loop {
                // Sleep the interval in small chunks so Drop (which sets `stop`) wakes us promptly
                // instead of blocking up to a full interval on shutdown.
                let mut slept = 0u64;
                while slept < interval {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    let chunk = (interval - slept).min(50);
                    std::thread::sleep(Duration::from_millis(chunk));
                    slept += chunk;
                }
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(mut g) = shared.0.lock() {
                    let _ = g.store.sync();
                }
            }))
        } else {
            None
        };

        Ok(Self { shared, stop, timer })
    }

    /// Append one record. Honors the buffer's `on_full` policy (may block on `Block`).
    pub fn append(&self, rec: &Record) -> Result<()> {
        let mut g = self.shared.0.lock().unwrap();
        let size = record::frame_size(rec.partition_key.len(), rec.payload.len()) as u64;
        g = self.ensure_room(g, size)?;
        let off = g.store.next_offset();
        g.store.append(off, rec.timestamp_ms, rec.partition_key.as_bytes(), &rec.payload)?;
        g.store.flush_os()?;
        if g.cfg.fsync == FsyncPolicy::Always {
            g.store.sync()?;
        }
        g.appended += 1;
        Ok(())
    }

    /// Append a batch (one fsync at the end under `PerBatch`).
    pub fn append_batch(&self, recs: &[Record]) -> Result<()> {
        let mut g = self.shared.0.lock().unwrap();
        let mut wrote = false;
        for rec in recs {
            let size = record::frame_size(rec.partition_key.len(), rec.payload.len()) as u64;
            g = self.ensure_room(g, size)?;
            let off = g.store.next_offset();
            g.store.append(off, rec.timestamp_ms, rec.partition_key.as_bytes(), &rec.payload)?;
            g.appended += 1;
            wrote = true;
        }
        if wrote {
            g.store.flush_os()?;
            if matches!(g.cfg.fsync, FsyncPolicy::PerBatch | FsyncPolicy::Always) {
                g.store.sync()?;
            }
        }
        Ok(())
    }

    /// Reclaim delivered segments, then enforce the disk budget per `on_full`. Returns the
    /// (possibly re-acquired) guard, or an error for `RejectNew` when over budget.
    fn ensure_room<'a>(
        &'a self,
        mut g: std::sync::MutexGuard<'a, Inner>,
        size: u64,
    ) -> Result<std::sync::MutexGuard<'a, Inner>> {
        loop {
            // Reclaim fully-delivered segments (not counted as drops).
            let acked = g.acked;
            let _ = g.store.truncate_below(acked);

            if g.store.disk_bytes() + size <= g.cfg.max_disk_bytes {
                return Ok(g);
            }
            match g.cfg.on_full {
                OnFull::DropOldest => {
                    while g.store.disk_bytes() + size > g.cfg.max_disk_bytes
                        && g.store.segment_count() > 1
                    {
                        let Some(end) = g.store.oldest_end() else { break };
                        let _ = g.store.truncate_below(end);
                        let acked = g.acked;
                        if end > acked {
                            // Dropped un-delivered records — count + advance the cursor.
                            g.dropped += end - acked;
                            g.acked = end;
                        }
                        g.drop_floor = end;
                        let cp = Checkpoint { acked: g.acked, drop_floor: g.drop_floor };
                        let _ = g.store.store_checkpoint(cp);
                    }
                    return Ok(g); // proceed even if a single oversized active segment remains
                }
                OnFull::RejectNew => return Err(GgStreamError::BufferFull),
                OnFull::Block => {
                    // Wait for the exporter's commit to reclaim space, then re-check.
                    g = self.shared.1.wait(g).unwrap();
                }
            }
        }
    }

    /// Read up to `max_records`/`max_bytes` un-delivered records (from the `acked` cursor).
    pub fn read_batch(&self, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        let g = self.shared.0.lock().unwrap();
        let from = g.acked;
        g.store.read_from(from, max_records, max_bytes)
    }

    /// Advance the delivery cursor past `through_offset` (inclusive), reclaim, and wake any
    /// `Block`ed producers.
    pub fn commit(&self, through_offset: u64) -> Result<()> {
        let mut g = self.shared.0.lock().unwrap();
        let new_acked = through_offset + 1;
        if new_acked > g.acked {
            g.acked = new_acked;
            let _ = g.store.truncate_below(new_acked);
            let cp = Checkpoint { acked: g.acked, drop_floor: g.drop_floor };
            g.store.store_checkpoint(cp)?;
        }
        self.shared.1.notify_all();
        Ok(())
    }

    /// The current delivery cursor (next offset to export).
    pub fn acked(&self) -> u64 {
        self.shared.0.lock().unwrap().acked
    }

    /// fsync now.
    pub fn flush(&self) -> Result<()> {
        self.shared.0.lock().unwrap().store.sync()
    }

    /// A snapshot of buffer stats.
    pub fn stats(&self) -> LogStats {
        let g = self.shared.0.lock().unwrap();
        let next_offset = g.store.next_offset();
        let oldest_unacked_age_ms = g.store.oldest_ts_ms().map(|ts| now_ms().saturating_sub(ts)).unwrap_or(0);
        LogStats {
            appended_total: g.appended,
            dropped_total: g.dropped,
            disk_bytes: g.store.disk_bytes(),
            next_offset,
            acked: g.acked,
            backlog: next_offset - g.acked,
            oldest_unacked_age_ms,
        }
    }
}

impl Drop for EmbeddedLog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.timer.take() {
            let _ = t.join();
        }
        if let Ok(mut g) = self.shared.0.lock() {
            let _ = g.store.sync();
        }
    }
}
