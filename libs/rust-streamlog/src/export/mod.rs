//! Export engine + the `Sink` seam.
//!
//! The engine reads committed records from an [`EmbeddedLog`], batches them, sends via a
//! [`Sink`], and advances the log's checkpoint **only after the whole batch is acked**
//! (contiguous-prefix → at-least-once, order preserved, no extra duplicates from partial
//! failures). Sinks are **synchronous**; an async sink (KinesisSink) owns its own runtime
//! internally and blocks inside `send`, so the engine stays tokio-free.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::blockstore::OwnedRecord;
use crate::config::{BatchConfig, DeliveryConfig};
use crate::log::EmbeddedLog;

/// One record handed to a sink (borrows from the read batch).
pub struct ExportRecord<'a> {
    pub offset: u64,
    pub partition_key: &'a [u8],
    pub ts_ms: u64,
    pub payload: &'a [u8],
}

/// Result of a sink send.
pub enum SendOutcome {
    /// Every record in the batch was stored.
    AllAcked,
    /// These offsets were NOT stored (retry them); the rest were acked.
    Partial { failed_offsets: Vec<u64> },
    /// The whole batch failed (e.g. disconnected). `retryable` is informational.
    Failed { retryable: bool, error: String },
}

/// A destination for exported records. Synchronous; an async sink wraps its own runtime.
pub trait Sink: Send {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome;
}

// ----- FakeSink: in-memory sink for tests + perf harness (`--sink fake`) -----

struct FakeState {
    delivered: Vec<(u64, Vec<u8>)>,
    fail_remaining: usize,
    partial_once: HashSet<u64>,
    partial_done: bool,
}

/// A handle to inspect a [`FakeSink`] after it has been moved into an engine.
#[derive(Clone)]
pub struct FakeSinkHandle {
    state: Arc<Mutex<FakeState>>,
}

impl FakeSinkHandle {
    /// `(offset, payload)` of every record the sink has acked, in delivery order.
    pub fn delivered(&self) -> Vec<(u64, Vec<u8>)> {
        self.state.lock().unwrap().delivered.clone()
    }
    pub fn delivered_offsets(&self) -> Vec<u64> {
        self.state.lock().unwrap().delivered.iter().map(|(o, _)| *o).collect()
    }
}

/// In-memory sink with optional failure injection.
pub struct FakeSink {
    state: Arc<Mutex<FakeState>>,
}

impl FakeSink {
    pub fn new() -> Self {
        Self::with(0, HashSet::new())
    }
    /// Fail the first `n` sends with a retryable `Failed` (nothing delivered), then succeed.
    pub fn fail_first(n: usize) -> Self {
        Self::with(n, HashSet::new())
    }
    /// On the first send, fail exactly these offsets (`Partial`); they succeed on retry.
    pub fn partial_once(offsets: impl IntoIterator<Item = u64>) -> Self {
        Self::with(0, offsets.into_iter().collect())
    }
    fn with(fail_remaining: usize, partial_once: HashSet<u64>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState {
                delivered: Vec::new(),
                fail_remaining,
                partial_once,
                partial_done: false,
            })),
        }
    }
    pub fn handle(&self) -> FakeSinkHandle {
        FakeSinkHandle { state: Arc::clone(&self.state) }
    }
}

impl Default for FakeSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for FakeSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        let mut s = self.state.lock().unwrap();
        if s.fail_remaining > 0 {
            s.fail_remaining -= 1;
            return SendOutcome::Failed { retryable: true, error: "injected transient failure".into() };
        }
        if !s.partial_done && !s.partial_once.is_empty() {
            s.partial_done = true;
            let failed: Vec<u64> =
                batch.iter().map(|r| r.offset).filter(|o| s.partial_once.contains(o)).collect();
            if !failed.is_empty() {
                let failset: HashSet<u64> = failed.iter().copied().collect();
                for r in batch.iter().filter(|r| !failset.contains(&r.offset)) {
                    s.delivered.push((r.offset, r.payload.to_vec()));
                }
                return SendOutcome::Partial { failed_offsets: failed };
            }
        }
        for r in batch {
            s.delivered.push((r.offset, r.payload.to_vec()));
        }
        SendOutcome::AllAcked
    }
}

// ----- ExportEngine -----

#[derive(Debug, Clone)]
pub struct EngineStats {
    pub exported_total: u64,
    pub retries_total: u64,
    pub failed_total: u64,
    pub last_error: Option<String>,
}

struct Shared {
    exported: AtomicU64,
    retries: AtomicU64,
    failed: AtomicU64,
    last_error: Mutex<Option<String>>,
}

/// Background export loop driving one stream's log → sink.
pub struct ExportEngine {
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl ExportEngine {
    /// Start the export loop on a background thread.
    pub fn start(
        log: Arc<EmbeddedLog>,
        mut sink: Box<dyn Sink>,
        batch_cfg: BatchConfig,
        delivery: DeliveryConfig,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            exported: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            last_error: Mutex::new(None),
        });
        let handle = {
            let stop = Arc::clone(&stop);
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                run(&log, sink.as_mut(), &batch_cfg, &delivery, &stop, &shared);
            })
        };
        Self { stop, shared, handle: Some(handle) }
    }

    pub fn stats(&self) -> EngineStats {
        EngineStats {
            exported_total: self.shared.exported.load(Ordering::Relaxed),
            retries_total: self.shared.retries.load(Ordering::Relaxed),
            failed_total: self.shared.failed.load(Ordering::Relaxed),
            last_error: self.shared.last_error.lock().unwrap().clone(),
        }
    }

    /// Signal the loop to stop and join it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ExportEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    log: &EmbeddedLog,
    sink: &mut dyn Sink,
    batch_cfg: &BatchConfig,
    delivery: &DeliveryConfig,
    stop: &AtomicBool,
    shared: &Shared,
) {
    while !stop.load(Ordering::Acquire) {
        let batch = match log.read_batch(batch_cfg.max_records, batch_cfg.max_bytes) {
            Ok(b) => b,
            Err(_) => {
                sleep_chunked(Duration::from_millis(delivery.poll_interval_ms.max(1)), stop);
                continue;
            }
        };
        if batch.is_empty() {
            sleep_chunked(Duration::from_millis(delivery.poll_interval_ms.max(1)), stop);
            continue;
        }
        deliver(log, sink, &batch, delivery, stop, shared);
    }
}

/// Deliver one batch with retry/partial handling, then advance the checkpoint past it.
fn deliver(
    log: &EmbeddedLog,
    sink: &mut dyn Sink,
    batch: &[OwnedRecord],
    delivery: &DeliveryConfig,
    stop: &AtomicBool,
    shared: &Shared,
) {
    let last_offset = batch.last().expect("non-empty batch").offset;
    let mut pending: Vec<&OwnedRecord> = batch.iter().collect();
    let mut attempt: i64 = 0;

    loop {
        if stop.load(Ordering::Acquire) {
            return; // do not commit; the batch re-delivers on restart (at-least-once)
        }
        let recs: Vec<ExportRecord<'_>> = pending
            .iter()
            .map(|r| ExportRecord {
                offset: r.offset,
                partition_key: &r.partition_key,
                ts_ms: r.ts_ms,
                payload: &r.payload,
            })
            .collect();

        match sink.send(&recs) {
            SendOutcome::AllAcked => {
                shared.exported.fetch_add(pending.len() as u64, Ordering::Relaxed);
                break;
            }
            SendOutcome::Partial { failed_offsets } => {
                let failset: HashSet<u64> = failed_offsets.into_iter().collect();
                let acked = pending.iter().filter(|r| !failset.contains(&r.offset)).count();
                shared.exported.fetch_add(acked as u64, Ordering::Relaxed);
                pending.retain(|r| failset.contains(&r.offset));
                if pending.is_empty() {
                    break;
                }
                attempt += 1;
                shared.retries.fetch_add(1, Ordering::Relaxed);
                backoff(attempt, delivery, stop);
            }
            SendOutcome::Failed { retryable: _, error } => {
                *shared.last_error.lock().unwrap() = Some(error);
                attempt += 1;
                if delivery.max_retries >= 0 && attempt > delivery.max_retries {
                    // Poison-pill escape: give up this batch (data loss) so the stream isn't
                    // wedged forever. Default max_retries = -1 never reaches here.
                    shared.failed.fetch_add(pending.len() as u64, Ordering::Relaxed);
                    break;
                }
                shared.retries.fetch_add(1, Ordering::Relaxed);
                backoff(attempt, delivery, stop);
            }
        }
    }

    let _ = log.commit(last_offset);
}

fn backoff(attempt: i64, delivery: &DeliveryConfig, stop: &AtomicBool) {
    let shift = attempt.clamp(0, 20) as u32;
    let base = delivery.backoff_base_ms.saturating_mul(1u64 << shift);
    let jitter = (now_nanos() % delivery.backoff_base_ms.max(1) as u128) as u64;
    let dur = base.saturating_add(jitter).min(delivery.backoff_max_ms);
    sleep_chunked(Duration::from_millis(dur), stop);
}

/// Sleep up to `dur`, waking early in ≤50ms chunks if `stop` is set.
fn sleep_chunked(dur: Duration, stop: &AtomicBool) {
    let mut left = dur;
    let chunk = Duration::from_millis(50);
    while left > Duration::ZERO {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let nap = left.min(chunk);
        std::thread::sleep(nap);
        left = left.saturating_sub(nap);
    }
}

fn now_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}
