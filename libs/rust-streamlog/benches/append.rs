//! Micro-benchmark: append latency/throughput across the fsync × payload curve (spec §15.5).
//!
//! In-process, stable, isolates the ingest path (no sink/network). Run:
//! `cargo bench --bench append`. There is no fixed target — this characterizes the
//! throughput↔durability dial and catches regressions.

use std::time::Instant;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use edgestreamlog::config::{BufferConfig, FsyncPolicy};
use edgestreamlog::{EmbeddedLog, Record};

/// Records appended per measured iteration.
const BATCH: usize = 1000;

fn buffer(path: &std::path::Path, fsync: FsyncPolicy) -> BufferConfig {
    BufferConfig {
        path: path.to_string_lossy().into_owned(),
        segment_bytes: 64 * 1024 * 1024,
        // Bounded so a long bench window recycles via dropOldest rather than filling the disk.
        max_disk_bytes: 512 * 1024 * 1024,
        fsync,
        ..Default::default()
    }
}

fn bench_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("append");
    group.throughput(Throughput::Elements(BATCH as u64));

    // Always-fsync is dramatically slower; keep its sample/payload set small so the bench finishes.
    for &fsync in &[FsyncPolicy::PerBatch, FsyncPolicy::Always] {
        for &payload in &[256usize, 1024, 4096] {
            if fsync == FsyncPolicy::Always && payload == 4096 {
                continue; // trim the matrix — the costly corner is covered by 256/1024
            }
            let id = BenchmarkId::from_parameter(format!("{fsync:?}/{payload}B"));
            group.bench_with_input(id, &(fsync, payload), |b, &(fsync, payload)| {
                // One log per measurement (created + dropped OUTSIDE the timed region via
                // iter_custom), so neither open() nor the Drop-time fsync/timer-join pollute the
                // append timing. `append` borrows the record, so one reusable Record is enough.
                b.iter_custom(|iters| {
                    let dir = tempfile::tempdir().unwrap();
                    let log = EmbeddedLog::open(buffer(dir.path(), fsync)).unwrap();
                    let rec = Record::new("pk", 1000, vec![b'x'; payload]);
                    let start = Instant::now();
                    for _ in 0..iters {
                        for _ in 0..BATCH {
                            log.append(&rec).unwrap();
                        }
                    }
                    start.elapsed()
                });
            });
        }
    }
    group.finish();
}

/// Batched append (one fsync per `append_batch`) — the high-throughput ingest path.
fn bench_append_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("append_batch");
    group.throughput(Throughput::Elements(BATCH as u64));
    for &payload in &[256usize, 1024, 4096] {
        let id = BenchmarkId::from_parameter(format!("PerBatch/{payload}B"));
        group.bench_with_input(id, &payload, |b, &payload| {
            b.iter_custom(|iters| {
                let dir = tempfile::tempdir().unwrap();
                let log = EmbeddedLog::open(buffer(dir.path(), FsyncPolicy::PerBatch)).unwrap();
                let recs: Vec<Record> = (0..BATCH)
                    .map(|i| Record::new("pk", 1000 + i as u64, vec![b'x'; payload]))
                    .collect();
                let start = Instant::now();
                for _ in 0..iters {
                    log.append_batch(&recs).unwrap();
                }
                start.elapsed()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_append, bench_append_batch);
criterion_main!(benches);
