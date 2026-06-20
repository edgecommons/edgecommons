//! `EmbeddedLog` — the durable buffer over a [`SegmentLog`]: ordered append, fsync policy,
//! disk retention + backpressure (`DropOldest`/`Block`/`RejectNew`), and the export-facing
//! `read_batch`/`commit` cursor.
//!
//! **Checkpoint is decoupled from the append lock.** `commit` only advances the in-memory `acked`
//! cursor (authoritative) and wakes blocked producers — it does **no** disk I/O. A background
//! maintenance thread periodically (a) fsyncs the segment data, (b) reclaims fully-delivered
//! segments, and (c) persists the checkpoint, doing the checkpoint fsync **off** the append lock
//! (serialized by its own mutex). A lagging checkpoint never loses data: on crash recovery the
//! cursor resumes from the last persisted checkpoint, re-delivering at most one interval's worth of
//! already-acked records (at-least-once). `flush` and `Drop` persist synchronously for a clean cursor.
//!
//! A writer-thread + bounded ingest queue (to decouple append latency from the segment fsync and to
//! batch group-commits) is a further perf pass — see `docs/TELEMETRY_STREAMING_PHASE1.md` §15.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::blockstore::checkpoint;
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
    /// Buffer directory (for checkpoint writes off the inner lock).
    dir: PathBuf,
    /// Serializes checkpoint file writes (maintenance thread vs `flush`/`Drop`) without taking the
    /// inner lock during the checkpoint fsync.
    checkpoint_lock: Arc<Mutex<()>>,
    stop: Arc<AtomicBool>,
    maintenance: Option<JoinHandle<()>>,
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
        let dir = PathBuf::from(&cfg.path);
        let checkpoint_lock = Arc::new(Mutex::new(()));

        // Background maintenance: periodic segment fsync (unless `Always`, which fsyncs inline),
        // retention (reclaim fully-delivered segments), and checkpoint persistence. The checkpoint
        // fsync runs OFF the inner lock so it never blocks producers (only the brief snapshot does).
        let maintenance = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            let checkpoint_lock = Arc::clone(&checkpoint_lock);
            let dir = dir.clone();
            let interval = cfg.fsync_interval_ms.max(1);
            let fsync_inline = cfg.fsync == FsyncPolicy::Always;
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
                maintenance_tick(&shared, &checkpoint_lock, &dir, fsync_inline);
            }))
        };

        Ok(Self { shared, dir, checkpoint_lock, stop, maintenance })
    }

    /// Snapshot `(acked, drop_floor)` and persist the checkpoint, doing the fsync **off** the inner
    /// lock (only the snapshot briefly holds it). Serialized against the maintenance thread by
    /// `checkpoint_lock` so the shared temp file is never written concurrently.
    fn persist_checkpoint(&self) -> Result<()> {
        let _cp = self.checkpoint_lock.lock().unwrap();
        let cp = {
            let g = self.shared.0.lock().unwrap();
            Checkpoint { acked: g.acked, drop_floor: g.drop_floor }
        };
        checkpoint::store(&self.dir, cp)
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
                        // Checkpoint is persisted by the maintenance thread (off the append lock);
                        // the in-memory acked/drop_floor are authoritative until then.
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
    ///
    /// Plans the byte ranges under the buffer lock (index lookups, no record file I/O) and then
    /// reads the segment files **off** the lock, so a draining exporter does not block producers on
    /// read I/O.
    pub fn read_batch(&self, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        let plan = {
            let mut g = self.shared.0.lock().unwrap();
            let from = g.acked;
            g.store.plan_read(from, max_records, max_bytes)?
        };
        crate::blockstore::segment_log::read_chunks(&plan)
    }

    /// Advance the delivery cursor past `through_offset` (inclusive) and wake any `Block`ed
    /// producers. **No disk I/O** — the checkpoint is persisted and delivered segments reclaimed by
    /// the maintenance thread (and by `ensure_room` on the next append). Keeps the export hot path
    /// off the append lock for fsync.
    pub fn commit(&self, through_offset: u64) -> Result<()> {
        let mut g = self.shared.0.lock().unwrap();
        let new_acked = through_offset + 1;
        if new_acked > g.acked {
            g.acked = new_acked;
        }
        drop(g);
        // Wake producers blocked in `ensure_room` (they reclaim space on re-check).
        self.shared.1.notify_all();
        Ok(())
    }

    /// The current delivery cursor (next offset to export).
    pub fn acked(&self) -> u64 {
        self.shared.0.lock().unwrap().acked
    }

    /// Make everything durable now: fsync the segment data **and** persist the checkpoint cursor.
    pub fn flush(&self) -> Result<()> {
        self.shared.0.lock().unwrap().store.sync()?;
        self.persist_checkpoint()
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
        if let Some(t) = self.maintenance.take() {
            let _ = t.join();
        }
        // Clean shutdown: data + checkpoint durable, so restart resumes with minimal re-delivery.
        let _ = self.flush();
    }
}

/// One maintenance cycle: segment fsync (unless inline), retention, then checkpoint persist with the
/// fsync OFF the inner lock.
fn maintenance_tick(
    shared: &Arc<(Mutex<Inner>, Condvar)>,
    checkpoint_lock: &Mutex<()>,
    dir: &std::path::Path,
    fsync_inline: bool,
) {
    let cp = {
        let Ok(mut g) = shared.0.lock() else { return };
        if !fsync_inline {
            let _ = g.store.sync();
        }
        // Reclaim fully-delivered segments (in case appends stopped while a backlog drained).
        let acked = g.acked;
        let _ = g.store.truncate_below(acked);
        Checkpoint { acked: g.acked, drop_floor: g.drop_floor }
    };
    // Checkpoint fsync off the inner lock; serialized against `flush`/`Drop`.
    let _cp = checkpoint_lock.lock().unwrap();
    let _ = checkpoint::store(dir, cp);
}
