//! BlockStore (SegmentLog) behavior + recovery tests.

use edgestreamlog::blockstore::segment_log::SegmentLog;
use edgestreamlog::blockstore::{BlockStore, Checkpoint};

fn append(store: &mut SegmentLog, off: u64, pk: &str, payload: &[u8]) {
    store.append(off, off * 10, pk.as_bytes(), payload).unwrap();
}

#[test]
fn indexed_batched_read_is_contiguous_across_reopen() {
    // Catch-up draining: read in small bounded batches across many segments, after a reopen that
    // forces lazy byte-index builds on the non-tail segments. Must yield 0..N exactly once, in order.
    let dir = tempfile::tempdir().unwrap();
    let n = 1000u64;
    {
        let mut store = SegmentLog::open(dir.path(), 256).unwrap(); // tiny → many segments
        for i in 0..n {
            append(&mut store, i, "k", format!("payload-{i}").as_bytes());
        }
        store.sync().unwrap();
    }
    // Reopen: only the active segment has a live index; older ones build lazily on read.
    let mut store = SegmentLog::open(dir.path(), 256).unwrap();
    let mut cursor = 0u64;
    let mut seen = Vec::new();
    loop {
        let batch = store.read_from(cursor, 7, 256).unwrap(); // bounded by records AND bytes
        if batch.is_empty() {
            break;
        }
        for r in &batch {
            assert_eq!(
                r.offset, cursor,
                "offsets must be contiguous with no gaps/dupes"
            );
            assert_eq!(r.payload, format!("payload-{}", r.offset).into_bytes());
            cursor += 1;
            seen.push(r.offset);
        }
    }
    assert_eq!(seen.len() as u64, n, "every record read back exactly once");
    assert_eq!(*seen.last().unwrap(), n - 1);
}

#[test]
fn append_read_roundtrip_across_segment_rolls() {
    let dir = tempfile::tempdir().unwrap();
    // Tiny segments so we roll frequently.
    let mut store = SegmentLog::open(dir.path(), 256).unwrap();
    for i in 0..200u64 {
        append(
            &mut store,
            i,
            &format!("k{}", i % 4),
            format!("payload-{i}").as_bytes(),
        );
    }
    store.sync().unwrap();
    assert_eq!(store.next_offset(), 200);

    let recs = store.read_from(0, usize::MAX, usize::MAX).unwrap();
    assert_eq!(recs.len(), 200);
    for (i, r) in recs.iter().enumerate() {
        assert_eq!(r.offset, i as u64);
        assert_eq!(r.ts_ms, i as u64 * 10);
        assert_eq!(r.payload, format!("payload-{i}").into_bytes());
    }

    // Read from a mid offset, bounded.
    let mid = store.read_from(150, 10, usize::MAX).unwrap();
    assert_eq!(mid.len(), 10);
    assert_eq!(mid[0].offset, 150);
}

#[test]
fn reopen_recovers_next_offset() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut store = SegmentLog::open(dir.path(), 1024).unwrap();
        for i in 0..50u64 {
            append(&mut store, i, "k", b"data");
        }
        store.sync().unwrap();
    }
    let mut store = SegmentLog::open(dir.path(), 1024).unwrap();
    assert_eq!(store.next_offset(), 50);
    assert!(!store.recovery().torn_truncated);
    assert_eq!(
        store.read_from(0, usize::MAX, usize::MAX).unwrap().len(),
        50
    );
}

#[test]
fn torn_tail_is_truncated_on_recovery() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut store = SegmentLog::open(dir.path(), 1 << 20).unwrap();
        for i in 0..10u64 {
            append(&mut store, i, "k", b"hello");
        }
        store.sync().unwrap();
    }
    // Corrupt the tail: append garbage bytes to the (single) active segment file.
    let seg = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|x| x.to_str()) == Some("seg"))
        .unwrap();
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&seg).unwrap();
        f.write_all(&[0xAB; 37]).unwrap(); // partial/garbage frame
    }
    let mut store = SegmentLog::open(dir.path(), 1 << 20).unwrap();
    assert!(
        store.recovery().torn_truncated,
        "garbage tail should be detected + truncated"
    );
    assert_eq!(store.next_offset(), 10, "only the 10 valid records survive");
    assert_eq!(
        store.read_from(0, usize::MAX, usize::MAX).unwrap().len(),
        10
    );
    // The store is appendable again from offset 10.
    append(&mut store, 10, "k", b"more");
    store.sync().unwrap();
    assert_eq!(store.next_offset(), 11);
}

#[test]
fn truncate_below_reclaims_old_segments() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SegmentLog::open(dir.path(), 128).unwrap(); // tiny → many segments
    for i in 0..100u64 {
        append(&mut store, i, "k", b"some-payload");
    }
    store.sync().unwrap();
    let before = store.disk_bytes();
    let reclaimed = store.truncate_below(50).unwrap();
    assert!(reclaimed > 0);
    assert!(store.disk_bytes() < before);
    // Records >= 50 remain readable; < 50 are gone.
    let recs = store.read_from(0, usize::MAX, usize::MAX).unwrap();
    assert!(recs.iter().all(|r| r.offset >= 1)); // some early segments removed
    assert_eq!(recs.last().unwrap().offset, 99);
}

#[test]
fn checkpoint_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = SegmentLog::open(dir.path(), 1024).unwrap();
    assert_eq!(store.load_checkpoint().unwrap(), Checkpoint::default());
    store
        .store_checkpoint(Checkpoint {
            acked: 42,
            drop_floor: 10,
        })
        .unwrap();
    let cp = SegmentLog::open(dir.path(), 1024)
        .unwrap()
        .load_checkpoint()
        .unwrap();
    assert_eq!(
        cp,
        Checkpoint {
            acked: 42,
            drop_floor: 10
        }
    );
}
