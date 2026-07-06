//! `EmbeddedLog` — the durable buffer over a [`SegmentLog`]: ordered append, fsync policy,
//! disk retention + backpressure (`DropOldest`/`Block`/`RejectNew`), and the export-facing
//! `read_batch`/`commit` cursor.
//!
//! ## Bounded ingest queue + leader/follower group commit
//! `append`/`append_batch` push records onto a bounded in-memory queue (the memory backpressure
//! point) and then become a **writer**: the first producer with no writer active claims the
//! "leader" role, drains the whole queue, writes the drained group with **one** `flush_os` + **one**
//! fsync (group-commit, per policy), and resolves every producer in the group; concurrent producers
//! that arrive while a leader is writing wait and are written as the next leader's group.
//!
//! This is group commit without a dedicated writer thread, so there is **no cross-thread hand-off**:
//! a lone producer leads every time and writes directly (≈ a plain locked append), while under
//! concurrency one leader amortizes the `flush_os`/fsync across many followers' records. Exactly one
//! writer touches the segment store at a time (the leader holds the inner lock while writing).
//!
//! `append` is **synchronous**: it returns only once its record is durable per the fsync policy
//! (`Always` → fsync every append; `PerBatch` → fsync every `append_batch`; `Interval` → flush to
//! the OS and fsync on the maintenance timer). The throughput win is fewer syscalls + amortized
//! fsync under concurrency, not returning early — durability is unchanged.
//!
//! ## Lock discipline
//! Two locks are never held together: the **ingest** lock (queue + leadership) and the **inner**
//! lock (segment store and cursor). A leader drains under ingest, releases it, then writes under
//! inner. Export `read_batch` plans under inner then reads off-lock; `commit` is an in-memory cursor
//! bump under inner; the maintenance thread persists the checkpoint with the fsync run off the inner
//! lock.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::blockstore::checkpoint;
use crate::blockstore::segment_log::{ReadChunk, SegmentLog};
use crate::blockstore::{BackingStore, BlockStore, Checkpoint, MemoryBlockStore, OwnedRecord};
use crate::config::{BufferConfig, FsyncPolicy, OnFull, StoreType};
use crate::error::{EdgeStreamError, Result};
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
    /// Records accepted into the ingest queue but not yet written by a leader.
    pub queued: u64,
    /// Age of the oldest retained record (0 if empty).
    pub oldest_unacked_age_ms: u64,
}

/// Segment store + delivery cursor + retention counters (the "inner" lock).
struct Inner {
    store: BackingStore,
    cfg: BufferConfig,
    drop_floor: u64,
    acked: u64,
    appended: u64,
    dropped: u64,
}

/// One record queued for the next group-commit.
struct Pending {
    seq: u64,
    pk: String,
    ts_ms: u64,
    payload: Vec<u8>,
    size: u64,
    /// Whether this record's durability requires an fsync before its producer is released (set per
    /// fsync policy). A group is fsynced iff ANY member needs it, so concurrent producers share one
    /// group-commit fsync.
    needs_fsync: bool,
}

/// The bounded ingest queue + leader/follower group-commit state (the "ingest" lock).
struct Ingest {
    queue: VecDeque<Pending>,
    queued_records: usize,
    /// Next submission sequence to assign (starts at 1; 0 = "nothing submitted").
    next_seq: u64,
    /// All sequences `<= resolved_seq` have been written (or rejected) by some leader.
    resolved_seq: u64,
    /// Per-sequence rejection errors (e.g. `RejectNew` → `BufferFull`); each consumed once.
    errors: HashMap<u64, EdgeStreamError>,
    /// A producer is currently the leader (writing a group); others wait.
    leader_active: bool,
}

/// Ingest queue + its condvars (`space`: producers wait for queue room; `done`: producers wait for
/// their record to be resolved by a leader).
struct IngestShared {
    mu: Mutex<Ingest>,
    space: Condvar,
    done: Condvar,
}

/// Durable store-and-forward buffer for one stream.
pub struct EmbeddedLog {
    /// Segment store + cursor (leader writes / read / commit / maintenance).
    inner: Arc<(Mutex<Inner>, Condvar)>,
    /// Ingest queue + group-commit leadership.
    ingest: Arc<IngestShared>,
    /// Buffer directory (for checkpoint writes off the inner lock).
    dir: PathBuf,
    /// Serializes checkpoint file writes (maintenance vs `flush`/`Drop`) without the inner lock.
    checkpoint_lock: Arc<Mutex<()>>,
    on_full: OnFull,
    fsync: FsyncPolicy,
    max_buffered_records: usize,
    /// Whether the backing store is durable (disk). False for in-memory streams, which skip all
    /// checkpoint file persistence (there is nothing to recover).
    persistent: bool,
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
        let persistent = cfg.store_type == StoreType::Disk;
        let store = match cfg.store_type {
            StoreType::Disk => BackingStore::Disk(SegmentLog::open(&cfg.path, cfg.segment_bytes)?),
            StoreType::Memory => BackingStore::Memory(MemoryBlockStore::new()),
        };
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
        let inner = Arc::new((Mutex::new(inner), Condvar::new()));
        let ingest = Arc::new(IngestShared {
            mu: Mutex::new(Ingest {
                queue: VecDeque::new(),
                queued_records: 0,
                next_seq: 1,
                resolved_seq: 0,
                errors: HashMap::new(),
                leader_active: false,
            }),
            space: Condvar::new(),
            done: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let dir = PathBuf::from(&cfg.path);
        let checkpoint_lock = Arc::new(Mutex::new(()));

        // Maintenance thread: periodic segment fsync (unless `Always`, fsynced inline by leaders),
        // retention, and checkpoint persistence — the checkpoint fsync runs OFF the inner lock.
        let maintenance = {
            let inner = Arc::clone(&inner);
            let stop = Arc::clone(&stop);
            let checkpoint_lock = Arc::clone(&checkpoint_lock);
            let dir = dir.clone();
            let interval = cfg.fsync_interval_ms.max(1);
            let fsync_inline = cfg.fsync == FsyncPolicy::Always;
            Some(std::thread::spawn(move || loop {
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
                maintenance_tick(&inner, &checkpoint_lock, &dir, fsync_inline, persistent);
            }))
        };

        Ok(Self {
            inner,
            ingest,
            dir,
            checkpoint_lock,
            on_full: cfg.on_full,
            fsync: cfg.fsync,
            max_buffered_records: cfg.max_buffered_records.max(1),
            persistent,
            stop,
            maintenance,
        })
    }

    /// Append one record. Blocks per the buffer's `on_full` policy; returns once the record is
    /// durable per the fsync policy (group-committed by whichever producer leads).
    pub fn append(&self, rec: &Record) -> Result<()> {
        let size = record::frame_size(rec.partition_key.len(), rec.payload.len()) as u64;
        // A single append is durable-before-return only under `Always` (matches the locked-append
        // semantics: `PerBatch`/`Interval` single appends flush to the OS and fsync on the interval).
        let needs_fsync = self.fsync == FsyncPolicy::Always;
        let seq = self.enqueue(rec, size, needs_fsync)?;
        self.drive_until(seq);
        self.take_result(seq)
    }

    /// Append a batch. All records are enqueued, then group-committed together; the call returns once
    /// the last is durable (or with the first rejection error).
    pub fn append_batch(&self, recs: &[Record]) -> Result<()> {
        if recs.is_empty() {
            return Ok(());
        }
        // A batch is durable-before-return under `PerBatch` and `Always` (one group-commit fsync).
        let needs_fsync = matches!(self.fsync, FsyncPolicy::PerBatch | FsyncPolicy::Always);
        let mut first = 0u64;
        let mut last = 0u64;
        {
            let mut q = self.ingest.mu.lock().unwrap();
            for (i, rec) in recs.iter().enumerate() {
                while q.queued_records >= self.max_buffered_records {
                    if self.on_full == OnFull::RejectNew {
                        return Err(EdgeStreamError::BufferFull);
                    }
                    q = self.ingest.space.wait(q).unwrap();
                }
                let size = record::frame_size(rec.partition_key.len(), rec.payload.len()) as u64;
                let seq = q.next_seq;
                q.next_seq += 1;
                if i == 0 {
                    first = seq;
                }
                last = seq;
                q.queue.push_back(Pending {
                    seq,
                    pk: rec.partition_key.clone(),
                    ts_ms: rec.timestamp_ms,
                    payload: rec.payload.clone(),
                    size,
                    needs_fsync,
                });
                q.queued_records += 1;
            }
        }
        self.drive_until(last);
        // Surface the first rejection across the batch (draining all of the batch's error entries).
        let mut q = self.ingest.mu.lock().unwrap();
        let mut err = None;
        for s in first..=last {
            if let Some(e) = q.errors.remove(&s) {
                err.get_or_insert(e);
            }
        }
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Enqueue one record, returning its submission sequence (or a backpressure error).
    fn enqueue(&self, rec: &Record, size: u64, needs_fsync: bool) -> Result<u64> {
        let mut q = self.ingest.mu.lock().unwrap();
        while q.queued_records >= self.max_buffered_records {
            if self.on_full == OnFull::RejectNew {
                return Err(EdgeStreamError::BufferFull);
            }
            q = self.ingest.space.wait(q).unwrap();
        }
        let seq = q.next_seq;
        q.next_seq += 1;
        q.queue.push_back(Pending {
            seq,
            pk: rec.partition_key.clone(),
            ts_ms: rec.timestamp_ms,
            payload: rec.payload.clone(),
            size,
            needs_fsync,
        });
        q.queued_records += 1;
        Ok(seq)
    }

    /// Drive the group-commit until `target` is resolved: lead a group (drain + write + resolve) when
    /// no leader is active, otherwise wait for the active leader to resolve it.
    fn drive_until(&self, target: u64) {
        loop {
            let mut q = self.ingest.mu.lock().unwrap();
            if q.resolved_seq >= target {
                return;
            }
            if q.leader_active {
                // Wait for the active leader to make progress, then re-check at the top.
                drop(self.ingest.done.wait(q).unwrap());
                continue;
            }
            // Become the leader: take the whole queue as this group.
            q.leader_active = true;
            let group: Vec<Pending> = q.queue.drain(..).collect();
            q.queued_records = 0;
            drop(q);
            self.ingest.space.notify_all(); // queue drained → wake producers waiting for room

            let (max_seq, errors_local) = self.write_group(&group);

            let mut q = self.ingest.mu.lock().unwrap();
            for (seq, e) in errors_local {
                q.errors.insert(seq, e);
            }
            if max_seq > q.resolved_seq {
                q.resolved_seq = max_seq;
            }
            q.leader_active = false;
            drop(q);
            self.ingest.done.notify_all();
            // Loop: re-check (our target is now resolved, or a follow-on group is needed).
        }
    }

    /// Write one drained group to the segment store under the inner lock: per-record `ensure_room`,
    /// then **one** `flush_os` + **one** fsync (iff any member needs durability). Returns the max
    /// sequence written and any per-record rejection errors.
    fn write_group(&self, group: &[Pending]) -> (u64, Vec<(u64, EdgeStreamError)>) {
        let do_fsync = group.iter().any(|p| p.needs_fsync);
        let mut max_seq = 0u64;
        let mut errors_local: Vec<(u64, EdgeStreamError)> = Vec::new();
        let mut g = self.inner.0.lock().unwrap();
        let mut wrote = false;
        for p in group {
            max_seq = max_seq.max(p.seq);
            let rejected;
            (g, rejected) = ensure_room(&self.inner.1, g, p.size);
            if let Some(e) = rejected {
                errors_local.push((p.seq, e));
                continue;
            }
            let off = g.store.next_offset();
            match g.store.append(off, p.ts_ms, p.pk.as_bytes(), &p.payload) {
                Ok(()) => {
                    g.appended += 1;
                    wrote = true;
                }
                Err(e) => errors_local.push((p.seq, e)),
            }
        }
        if wrote {
            let _ = g.store.flush_os();
            if do_fsync {
                let _ = g.store.sync();
            }
        }
        (max_seq, errors_local)
    }

    /// Consume the resolution result for `seq` (called after [`drive_until`] guarantees it).
    fn take_result(&self, seq: u64) -> Result<()> {
        let mut q = self.ingest.mu.lock().unwrap();
        match q.errors.remove(&seq) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Read up to `max_records`/`max_bytes` un-delivered records (from the `acked` cursor).
    ///
    /// Plans the byte ranges under the inner lock (index lookups, no record file I/O) and then reads
    /// the segment files **off** the lock, so a draining exporter does not block writers on read I/O.
    pub fn read_batch(&self, max_records: usize, max_bytes: usize) -> Result<Vec<OwnedRecord>> {
        // Plan/read under the inner lock; for the disk store, defer the segment file I/O until the
        // lock is released (so a draining exporter doesn't block writers). The in-memory store has
        // no file I/O, so its (cheap) read happens under the lock.
        enum Planned {
            DiskChunks(Vec<ReadChunk>),
            Records(Vec<OwnedRecord>),
        }
        let planned = {
            let mut g = self.inner.0.lock().unwrap();
            let from = g.acked;
            match &mut g.store {
                BackingStore::Disk(s) => Planned::DiskChunks(s.plan_read(from, max_records, max_bytes)?),
                BackingStore::Memory(s) => Planned::Records(s.read_from(from, max_records, max_bytes)?),
            }
        };
        match planned {
            Planned::DiskChunks(chunks) => crate::blockstore::segment_log::read_chunks(&chunks),
            Planned::Records(records) => Ok(records),
        }
    }

    /// Advance the delivery cursor past `through_offset` (inclusive) and wake any `Block`ed
    /// leaders. **No disk I/O** — the checkpoint is persisted and delivered segments reclaimed by the
    /// maintenance thread (and by a leader's `ensure_room` on the next write).
    pub fn commit(&self, through_offset: u64) -> Result<()> {
        let mut g = self.inner.0.lock().unwrap();
        let new_acked = through_offset + 1;
        if new_acked > g.acked {
            g.acked = new_acked;
        }
        drop(g);
        self.inner.1.notify_all();
        Ok(())
    }

    /// The current delivery cursor (next offset to export).
    pub fn acked(&self) -> u64 {
        self.inner.0.lock().unwrap().acked
    }

    /// Make everything submitted so far durable: ensure the queue is drained + written, fsync the
    /// segment data, and persist the checkpoint cursor.
    pub fn flush(&self) -> Result<()> {
        // Drive the group-commit until every record submitted before this call is written.
        let target = { self.ingest.mu.lock().unwrap().next_seq.saturating_sub(1) };
        self.drive_until(target);
        self.inner.0.lock().unwrap().store.sync()?;
        self.persist_checkpoint()
    }

    /// Snapshot `(acked, drop_floor)` and persist the checkpoint, doing the fsync **off** the inner
    /// lock. Serialized against the maintenance thread by `checkpoint_lock`.
    fn persist_checkpoint(&self) -> Result<()> {
        if !self.persistent {
            return Ok(()); // in-memory: nothing to persist
        }
        let _cp = self.checkpoint_lock.lock().unwrap();
        let cp = {
            let g = self.inner.0.lock().unwrap();
            Checkpoint { acked: g.acked, drop_floor: g.drop_floor }
        };
        checkpoint::store(&self.dir, cp)
    }

    /// A snapshot of buffer stats.
    pub fn stats(&self) -> LogStats {
        let queued = self.ingest.mu.lock().unwrap().queued_records as u64;
        let g = self.inner.0.lock().unwrap();
        let next_offset = g.store.next_offset();
        let oldest_unacked_age_ms =
            g.store.oldest_ts_ms().map(|ts| now_ms().saturating_sub(ts)).unwrap_or(0);
        LogStats {
            appended_total: g.appended,
            dropped_total: g.dropped,
            disk_bytes: g.store.disk_bytes(),
            next_offset,
            acked: g.acked,
            backlog: next_offset - g.acked,
            queued,
            oldest_unacked_age_ms,
        }
    }
}

impl Drop for EmbeddedLog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(m) = self.maintenance.take() {
            let _ = m.join();
        }
        // Clean shutdown: drain + data + checkpoint durable, so restart resumes with minimal
        // re-delivery. (Drop runs only with no live appenders, so the queue is already empty.)
        let _ = self.flush();
    }
}

/// Reclaim delivered segments, then enforce the disk budget per `on_full`. Returns the (possibly
/// re-acquired) guard plus an optional rejection error (`RejectNew` over budget). `Block` waits on
/// the inner condvar (released while waiting) until a `commit` frees space.
fn ensure_room<'a>(
    cv: &Condvar,
    mut g: MutexGuard<'a, Inner>,
    size: u64,
) -> (MutexGuard<'a, Inner>, Option<EdgeStreamError>) {
    loop {
        // Reclaim fully-delivered segments (not counted as drops).
        let acked = g.acked;
        let _ = g.store.truncate_below(acked);

        if g.store.disk_bytes() + size <= g.cfg.max_disk_bytes {
            return (g, None);
        }
        match g.cfg.on_full {
            OnFull::DropOldest => {
                while g.store.disk_bytes() + size > g.cfg.max_disk_bytes {
                    // Reclaim the oldest unit (a segment for disk, one record for memory). `None`
                    // means nothing more is droppable (disk: only the active segment remains).
                    let Some(end) = g.store.next_drop_boundary() else { break };
                    let _ = g.store.truncate_below(end);
                    let acked = g.acked;
                    if end > acked {
                        // Dropped un-delivered records — count + advance the cursor.
                        g.dropped += end - acked;
                        g.acked = end;
                    }
                    g.drop_floor = end;
                    // Checkpoint persisted by the maintenance thread; in-memory state is authoritative.
                }
                return (g, None); // proceed even if a single oversized active segment remains
            }
            OnFull::RejectNew => return (g, Some(EdgeStreamError::BufferFull)),
            OnFull::Block => {
                // Wait for the exporter's commit to reclaim space, then re-check.
                g = cv.wait(g).unwrap();
            }
        }
    }
}

/// One maintenance cycle: segment fsync (unless inline), retention, then checkpoint persist with the
/// fsync OFF the inner lock.
fn maintenance_tick(
    inner: &Arc<(Mutex<Inner>, Condvar)>,
    checkpoint_lock: &Mutex<()>,
    dir: &std::path::Path,
    fsync_inline: bool,
    persistent: bool,
) {
    let cp = {
        let Ok(mut g) = inner.0.lock() else { return };
        if !fsync_inline {
            let _ = g.store.sync();
        }
        let acked = g.acked;
        let _ = g.store.truncate_below(acked);
        Checkpoint { acked: g.acked, drop_floor: g.drop_floor }
    };
    // In-memory streams have nothing to persist (and no dir).
    if persistent {
        let _cp = checkpoint_lock.lock().unwrap();
        let _ = checkpoint::store(dir, cp);
    }
}
