//! ExportEngine behavior: in-order delivery, transient-failure retry, partial-failure retry,
//! and at-least-once redelivery when records are read but never committed (crash before ack).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ggstreamlog::config::{BatchConfig, BufferConfig, DeliveryConfig, FsyncPolicy, OnFull};
use ggstreamlog::{EmbeddedLog, ExportEngine, FakeSink, Record};

fn buf_cfg(path: &std::path::Path) -> BufferConfig {
    BufferConfig {
        path: path.to_string_lossy().into_owned(),
        segment_bytes: 1 << 20,
        max_disk_bytes: 1 << 30,
        on_full: OnFull::Block,
        fsync: FsyncPolicy::PerBatch,
        ..Default::default()
    }
}

fn fast_delivery() -> DeliveryConfig {
    // Tiny backoff/poll so tests drain quickly.
    DeliveryConfig { backoff_base_ms: 1, backoff_max_ms: 5, poll_interval_ms: 5, ..Default::default() }
}

fn rec(pk: &str, payload: &[u8]) -> Record {
    Record::new(pk, 1000, payload)
}

/// Wait until `f()` is true or `timeout` elapses; returns whether it became true.
fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    f()
}

#[test]
fn concurrent_append_and_export_no_loss() {
    // Many producers appending while the export engine drains concurrently (the real running-
    // component profile, and the path where reads/checkpoint run off the append lock). Every
    // record must be delivered exactly once with no loss — exercises the off-lock plan/read split
    // under contention.
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(EmbeddedLog::open(buf_cfg(dir.path())).unwrap());
    let threads = 4usize;
    let per_thread = 5_000u64;
    let total = threads as u64 * per_thread;

    let sink = FakeSink::new();
    let handle = sink.handle();
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig { max_records: 256, ..Default::default() },
        fast_delivery(),
    );

    std::thread::scope(|scope| {
        for _ in 0..threads {
            let log = Arc::clone(&log);
            scope.spawn(move || {
                for i in 0..per_thread {
                    log.append(&rec("pk", format!("v{i}").as_bytes())).unwrap();
                }
            });
        }
    });

    assert!(
        wait_until(Duration::from_secs(20), || log.acked() == total),
        "engine should drain all {total}, acked={}",
        log.acked()
    );
    let mut offsets = handle.delivered_offsets();
    offsets.sort_unstable();
    // No concurrent crash → exactly-once: every offset 0..total delivered once, in contiguous order.
    assert_eq!(offsets.len() as u64, total, "no duplicates or loss");
    assert_eq!(offsets, (0..total).collect::<Vec<_>>(), "every offset delivered exactly once");
    assert_eq!(engine.stats().exported_total, total);
    engine.stop();
}

#[test]
fn engine_delivers_all_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(EmbeddedLog::open(buf_cfg(dir.path())).unwrap());
    for i in 0..100u64 {
        log.append(&rec("k", format!("v{i}").as_bytes())).unwrap();
    }
    let sink = FakeSink::new();
    let handle = sink.handle();
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig { max_records: 10, ..Default::default() },
        fast_delivery(),
    );

    assert!(
        wait_until(Duration::from_secs(5), || handle.delivered().len() == 100),
        "all 100 records should be delivered, got {}",
        handle.delivered().len()
    );
    // Offsets arrive contiguously and in order.
    let offsets = handle.delivered_offsets();
    let expected: Vec<u64> = (0..100).collect();
    assert_eq!(offsets, expected, "records delivered in offset order");
    // Payloads match.
    for (off, payload) in handle.delivered() {
        assert_eq!(payload, format!("v{off}").as_bytes());
    }
    // The log's cursor advanced past everything → buffer drained.
    assert!(wait_until(Duration::from_secs(2), || log.acked() == 100));
    let stats = engine.stats();
    assert_eq!(stats.exported_total, 100);
    assert_eq!(stats.failed_total, 0);
    engine.stop();
}

#[test]
fn engine_retries_transient_failures() {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(EmbeddedLog::open(buf_cfg(dir.path())).unwrap());
    for i in 0..10u64 {
        log.append(&rec("k", format!("v{i}").as_bytes())).unwrap();
    }
    // Fail the first 3 sends wholesale, then succeed.
    let sink = FakeSink::fail_first(3);
    let handle = sink.handle();
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig { max_records: usize::MAX, max_bytes: usize::MAX, ..Default::default() },
        fast_delivery(),
    );

    assert!(
        wait_until(Duration::from_secs(5), || handle.delivered().len() == 10),
        "all records delivered after transient failures, got {}",
        handle.delivered().len()
    );
    assert_eq!(handle.delivered_offsets(), (0..10).collect::<Vec<_>>());
    let stats = engine.stats();
    assert!(stats.retries_total >= 3, "should record >=3 retries, got {}", stats.retries_total);
    assert_eq!(stats.exported_total, 10);
    assert!(stats.last_error.is_some(), "last_error captured from the transient failures");
    engine.stop();
}

#[test]
fn engine_handles_partial_failures() {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(EmbeddedLog::open(buf_cfg(dir.path())).unwrap());
    for i in 0..10u64 {
        log.append(&rec("k", format!("v{i}").as_bytes())).unwrap();
    }
    // On the first send, offsets 3,5,7 fail (Partial); they succeed on retry. With a single
    // big batch the others are acked immediately and the trio is redelivered.
    let sink = FakeSink::partial_once([3, 5, 7]);
    let handle = sink.handle();
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig { max_records: usize::MAX, max_bytes: usize::MAX, ..Default::default() },
        fast_delivery(),
    );

    assert!(
        wait_until(Duration::from_secs(5), || handle.delivered().len() == 10),
        "every record eventually delivered, got {}",
        handle.delivered().len()
    );
    // Every offset 0..10 present exactly once (the partial trio is retried, not re-acked).
    let mut offsets = handle.delivered_offsets();
    offsets.sort_unstable();
    assert_eq!(offsets, (0..10).collect::<Vec<_>>(), "all offsets delivered exactly once");
    let stats = engine.stats();
    assert!(stats.retries_total >= 1, "partial failure should drive a retry");
    assert_eq!(stats.exported_total, 10);
    engine.stop();
}

#[test]
fn at_least_once_redelivery_after_crash() {
    // Simulate a crash mid-delivery: read a batch but never commit, drop the log, reopen.
    // The same records must be re-readable (at-least-once).
    let dir = tempfile::tempdir().unwrap();
    {
        let log = EmbeddedLog::open(buf_cfg(dir.path())).unwrap();
        for i in 0..20u64 {
            log.append(&rec("k", format!("v{i}").as_bytes())).unwrap();
        }
        // Export "sent" the first 10 but the process died before commit().
        let batch = log.read_batch(10, usize::MAX).unwrap();
        assert_eq!(batch.len(), 10);
        assert_eq!(batch[0].offset, 0);
        log.flush().unwrap();
        // drop without commit → checkpoint still at 0
    }

    // Reopen: nothing was acked, so all 20 are redelivered from offset 0.
    let log = Arc::new(EmbeddedLog::open(buf_cfg(dir.path())).unwrap());
    assert_eq!(log.acked(), 0, "no commit happened → cursor unmoved");

    let sink = FakeSink::new();
    let handle = sink.handle();
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig::default(),
        fast_delivery(),
    );
    assert!(
        wait_until(Duration::from_secs(5), || handle.delivered().len() == 20),
        "all 20 redelivered after crash, got {}",
        handle.delivered().len()
    );
    assert_eq!(handle.delivered_offsets(), (0..20).collect::<Vec<_>>());
    engine.stop();
}
