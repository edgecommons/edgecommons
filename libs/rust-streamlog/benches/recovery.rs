//! Micro-benchmark: recovery (open) wall-clock vs log size (spec §15.5 / S3).
//!
//! Recovery scans only the active (highest-base) segment and validates/truncates its tail, so it
//! should be ~independent of total log size — this bench pins that property and catches regressions
//! (e.g. an accidental full-log rescan). Run: `cargo bench --bench recovery`.
//!
//! NOTE: for a true cold-cache number, drop the page cache before the run on Linux
//! (`echo 3 | sudo tee /proc/sys/vm/drop_caches`); criterion can't do that per-iteration, so these
//! are warm-cache figures.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use edgestreamlog::config::{BufferConfig, FsyncPolicy};
use edgestreamlog::{EmbeddedLog, Record};

fn buffer(path: &std::path::Path) -> BufferConfig {
    BufferConfig {
        path: path.to_string_lossy().into_owned(),
        segment_bytes: 64 * 1024 * 1024,
        max_disk_bytes: 8 * 1024 * 1024 * 1024,
        fsync: FsyncPolicy::PerBatch,
        ..Default::default()
    }
}

/// Write `n` records into `dir` once (not timed).
fn populate(dir: &std::path::Path, n: u64) {
    let log = EmbeddedLog::open(buffer(dir)).unwrap();
    let body = vec![b'x'; 256];
    let chunk = 2000usize;
    let mut written = 0u64;
    while written < n {
        let this = chunk.min((n - written) as usize);
        let recs: Vec<Record> =
            (0..this).map(|i| Record::new("pk", 1000 + written + i as u64, body.clone())).collect();
        log.append_batch(&recs).unwrap();
        written += this as u64;
    }
    log.flush().unwrap();
}

fn bench_recovery(c: &mut Criterion) {
    let mut group = c.benchmark_group("recovery");
    group.sample_size(20); // opening is fast; fewer samples keeps the populate cost amortized
    for &n in &[100_000u64, 1_000_000] {
        // Persist one log of size n; reopen it repeatedly (recovery is idempotent).
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path(), n);
        let cfg = buffer(dir.path());
        group.bench_with_input(BenchmarkId::from_parameter(n), &cfg, |b, cfg| {
            b.iter(|| {
                let log = EmbeddedLog::open(cfg.clone()).unwrap();
                black_box(log.stats().next_offset);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_recovery);
criterion_main!(benches);
