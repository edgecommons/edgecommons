//! EmbeddedLog behavior: append/read/commit cycle, durability, retention/backpressure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use edgestreamlog::config::{BufferConfig, FsyncPolicy, OnFull};
use edgestreamlog::{EmbeddedLog, Record};

fn cfg(path: &std::path::Path, segment_bytes: u64, max_disk: u64, on_full: OnFull) -> BufferConfig {
    BufferConfig {
        path: path.to_string_lossy().into_owned(),
        segment_bytes,
        max_disk_bytes: max_disk,
        on_full,
        fsync: FsyncPolicy::PerBatch,
        ..Default::default()
    }
}

fn rec(pk: &str, payload: &[u8]) -> Record {
    Record::new(pk, 1000, payload)
}

#[test]
fn append_read_commit_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let log = EmbeddedLog::open(cfg(dir.path(), 1 << 20, 1 << 30, OnFull::Block)).unwrap();
    for i in 0..20u64 {
        log.append(&rec("k", format!("v{i}").as_bytes())).unwrap();
    }
    let batch = log.read_batch(usize::MAX, usize::MAX).unwrap();
    assert_eq!(batch.len(), 20);
    assert_eq!(batch[0].offset, 0);

    // Commit the first 10; the next read starts at 10.
    log.commit(9).unwrap();
    assert_eq!(log.acked(), 10);
    let batch = log.read_batch(usize::MAX, usize::MAX).unwrap();
    assert_eq!(batch.len(), 10);
    assert_eq!(batch[0].offset, 10);

    log.commit(19).unwrap();
    assert!(log.read_batch(usize::MAX, usize::MAX).unwrap().is_empty());
    let s = log.stats();
    assert_eq!(s.acked, 20);
    assert_eq!(s.backlog, 0);
    assert_eq!(s.dropped_total, 0);
}

#[test]
fn durability_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let log = EmbeddedLog::open(cfg(dir.path(), 1 << 20, 1 << 30, OnFull::Block)).unwrap();
        for i in 0..30u64 {
            log.append(&rec("k", format!("v{i}").as_bytes())).unwrap();
        }
        log.commit(9).unwrap(); // 0..=9 delivered
        log.flush().unwrap();
    }
    let log = EmbeddedLog::open(cfg(dir.path(), 1 << 20, 1 << 30, OnFull::Block)).unwrap();
    assert_eq!(log.acked(), 10, "checkpoint persisted");
    let batch = log.read_batch(usize::MAX, usize::MAX).unwrap();
    assert_eq!(batch.len(), 20);
    assert_eq!(batch[0].offset, 10);
    assert_eq!(batch.last().unwrap().offset, 29);
}

#[test]
fn drop_oldest_bounds_disk() {
    let dir = tempfile::tempdir().unwrap();
    // Tiny segments + budget so we must drop undelivered data; never commit.
    let log = EmbeddedLog::open(cfg(dir.path(), 256, 1024, OnFull::DropOldest)).unwrap();
    for i in 0..5000u64 {
        log.append(&rec("k", format!("payload-number-{i}").as_bytes()))
            .unwrap();
    }
    let s = log.stats();
    // Disk stays within budget + at most one (active) segment of overshoot.
    assert!(
        s.disk_bytes <= 1024 + 256,
        "disk_bytes={} over budget",
        s.disk_bytes
    );
    assert!(
        s.dropped_total > 0,
        "should have dropped undelivered records"
    );
    assert_eq!(s.appended_total, 5000);
    // The cursor advanced past dropped data; survivors are the newest records.
    assert!(s.acked > 0 && s.acked == s.dropped_total);
    let batch = log.read_batch(usize::MAX, usize::MAX).unwrap();
    assert!(!batch.is_empty());
    assert_eq!(batch.last().unwrap().offset, 4999, "newest record retained");
    assert_eq!(batch[0].offset, s.acked, "survivors start at the cursor");
}

#[test]
fn reject_new_when_full() {
    let dir = tempfile::tempdir().unwrap();
    let log = EmbeddedLog::open(cfg(dir.path(), 256, 1024, OnFull::RejectNew)).unwrap();
    let mut rejected = false;
    for i in 0..5000u64 {
        match log.append(&rec("k", format!("payload-number-{i}").as_bytes())) {
            Ok(()) => {}
            Err(edgestreamlog::EdgeStreamError::BufferFull) => {
                rejected = true;
                break;
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
    assert!(
        rejected,
        "RejectNew must eventually reject when the disk budget is hit"
    );
    assert_eq!(
        log.stats().dropped_total,
        0,
        "RejectNew never drops persisted data"
    );
}

#[test]
fn block_unblocks_on_commit() {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(EmbeddedLog::open(cfg(dir.path(), 256, 1024, OnFull::Block)).unwrap());

    // Fill until the next append would block (disk full, nothing delivered yet).
    let mut filled = 0u64;
    loop {
        // Use try via a thread? Instead, fill to just under budget by appending a bounded count
        // that we know fits, then prove a blocked appender is released by a commit.
        if log.stats().disk_bytes + 64 > 1024 {
            break;
        }
        log.append(&rec("k", b"some-payload-bytes")).unwrap();
        filled += 1;
    }
    assert!(filled > 0);

    // A second appender blocks because the buffer is full and on_full=Block.
    let done = Arc::new(AtomicBool::new(false));
    let log2 = Arc::clone(&log);
    let done2 = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        log2.append(&rec("k", b"blocked-record-xxxx")).unwrap();
        done2.store(true, Ordering::Release);
    });

    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        !done.load(Ordering::Acquire),
        "appender should be blocked while full"
    );

    // Deliver everything → frees space → the blocked appender proceeds.
    log.commit(filled - 1).unwrap();
    for _ in 0..50 {
        if done.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        done.load(Ordering::Acquire),
        "commit should unblock the appender"
    );
    handle.join().unwrap();
}
