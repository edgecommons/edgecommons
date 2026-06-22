# Memory vs disk backing — append/throughput perf

Equivalent of the disk-backed S1 perf runs, repeated for the in-memory backing
(`buffer.type: "memory"`, `MemoryBlockStore`). Both backings measured on the **same
machine, same build** for an apples-to-apples comparison.

- Harness: `examples/loadgen.rs`, new `--store disk|memory` flag.
- Scenario **S1** (ingest), payload **1 KiB**, duration **8 s**, threads **1/4/8**.
- Sinks: `instant` (real running-component path — export drains as producers append)
  and `ingest-only` (isolated append ceiling, no export engine).
- Run: `loadgen --scenario S1 --payload 1024 --duration 8 --threads <t> --store <s> --sink <k>`.
- Platform: Windows (NTFS) — the disk numbers are NTFS-fsync-bound; ext4/NVMe disk is faster,
  but the *relative* memory-vs-disk gap is the point.

## Results (this machine)

| sink        | th | store  |        rps |  MB/s | p50 µs | p99 µs | p999 µs |  max µs |
|-------------|---:|--------|-----------:|------:|-------:|-------:|--------:|--------:|
| instant     |  1 | Disk   |    228,434 | 233.9 |    2.0 |   13.9 |    42.6 | 47,416  |
| instant     |  1 | Memory |  1,810,022 |1,853.5|    0.2 |    0.6 |    63.8 |  1,202  |
| instant     |  4 | Disk   |    213,250 | 218.4 |    9.3 |   49.9 |   121.0 | 24,601  |
| instant     |  4 | Memory |  1,423,435 |1,457.6|    1.7 |   15.4 |   156.7 |  1,101  |
| instant     |  8 | Disk   |    245,109 | 251.0 |   14.6 |   71.1 | 6,155.4 | 67,756  |
| instant     |  8 | Memory |    871,646 | 892.6 |    5.7 |   58.2 |   282.4 |    806  |
| ingest-only |  1 | Disk   |    253,141 | 259.2 |    1.9 |    9.9 |    31.8 | 15,304  |
| ingest-only |  1 | Memory |  2,040,776 |2,089.8|    0.3 |    0.7 |     1.2 |    133  |
| ingest-only |  4 | Disk   |    208,446 | 213.4 |    9.4 |   49.5 |   119.2 | 20,775  |
| ingest-only |  4 | Memory |  1,750,830 |1,792.8|    1.9 |    4.9 |    35.0 |  4,091  |
| ingest-only |  8 | Disk   |    267,562 | 274.0 |   13.6 |   66.6 |   640.3 | 21,033  |
| ingest-only |  8 | Memory |  1,060,471 |1,085.9|    5.9 |   28.9 |    57.7 |  4,308  |

## Takeaways

- **~8× higher single-thread throughput** (memory ~1.8–2.0M rps vs disk ~0.23–0.25M rps),
  and **~20× lower append p99** (~0.6 µs vs ~10–14 µs).
- **No fsync / segment-roll tail latency.** Disk `max` spikes to 15–68 ms (fsync stalls,
  segment rolls); memory `max` stays ~1 ms with `instant` drain.
- **Memory scales DOWN with thread count** (1.8M→1.4M→0.87M): with disk I/O gone, the shared
  buffer `Mutex` is the bottleneck and contention dominates. Disk stays flat (~0.21–0.27M rps)
  because group-commit batches under the dominant I/O cost. So memory is a single-/few-writer win.
- **`ingest-only` memory ran to its 1 GB RAM budget** and evicted oldest (`dropOldest`):
  t1 appended 16.3M, dropped 15.3M (kept ~1 GB). `throughput_rps` still reflects the true append
  rate; the `instant` runs drain continuously so RAM stays small. `disk_bytes_final` for memory
  reports the in-memory byte count (capped at `maxDiskBytes` = the RAM budget).

## Not applicable to memory (disk-only by nature)

`S3` (recovery time vs log size) and `S8` (crash-resume cost) measure durability/restart behavior.
The memory backing is **non-durable** — nothing survives restart, recovery is a no-op — so those
scenarios have no memory equivalent.
