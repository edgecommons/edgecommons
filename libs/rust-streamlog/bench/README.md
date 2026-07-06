# edgestreamlog perf harness

Implements the performance plan in `docs/TELEMETRY_STREAMING_PHASE1.md` §15. **There is no
fixed throughput target** — the goal is per-(target × config) baselines, the
throughput↔durability curve, the bottleneck (CPU vs disk vs fsync vs network), and
regression detection.

## Pieces

| Tool | What it does | Run |
|------|--------------|-----|
| `examples/loadgen.rs` | End-to-end scenario driver (S1–S8); emits one JSON results record. | `cargo run --release --example loadgen -- …` |
| `examples/bench_compare.rs` | Flags any metric regressing > threshold (default 10%) vs a baseline; exits non-zero (CI gate). | `cargo run --release --example bench_compare -- --baseline A.json --current B.json` |
| `benches/append.rs` | Criterion micro-bench: append latency/throughput across fsync × payload. | `cargo bench --bench append` |
| `benches/recovery.rs` | Criterion micro-bench: recovery (open) wall-clock vs log size. | `cargo bench --bench recovery` |
| `bench/run_matrix.sh` / `.ps1` | Sweep the matrix into `bench/results/<target>/`. | `bench/run_matrix.sh <target-name> <buffer-path>` |

## loadgen flags

```
--scenario <S1..S8>     --payload <bytes>        --threads <n>
--rate <r|unbounded>    --duration <s>           --count <n>
--fsync <PerBatch|Interval|Always>               --segment-bytes <n>
--max-disk-bytes <n>    --on-full <dropOldest|block|rejectNew>
--sink <none|fake-rate:<r>|disconnect:<secs>|kinesis:<stream>|localstack:<stream>>
--path <dir>            --batch-records <n>      --drop-caches
--target-name <s>       --git-sha <s>            --results-dir <dir> | --out <file> | --keep
```

**Sinks.** `instant` (default) = **concurrent ingest+drain** with an instant-ack sink — the standard
operating profile of a running component (producers append while an export engine drains; the
bottleneck is disk + framing + the shared buffer lock, not the network). `ingest-only` = no export
engine — a best-case ceiling only (isolated, not real-world). `fake-rate:<r>` caps drain to model a
slow sink. `disconnect:<secs>` fails for N s then acks (backlog → drain). `kinesis:`/`localstack:` is
real `PutRecords` (needs the `kinesis` cargo feature; `localstack:` points at `http://localhost:4566`
for floci/LocalStack).

> **Always report the concurrent number as the real-world figure.** The ingest-only ceiling is just
> the upper bound. After (a) decoupling the checkpoint + export read I/O from the append lock and
> (b) adding the bounded ingest queue with leader/follower **group commit**, concurrent ingest+drain
> now *exceeds* the old single-thread ceiling and scales with producer count on this Windows box:
> ~223k (1 thread) / ~227k (4) / ~264k (8) rps @1 KiB, append p50 ~2µs. Group commit batches many
> concurrent producers' records into one `flush_os` + one fsync; a lone producer leads every time
> and writes directly (no hand-off), so there is no low-concurrency regression. `Always` fsync is
> still fsync-bound on NTFS but amortizes across concurrent producers (much better on ext4/NVMe —
> i.e. lab-5950x / Pi-SSD).

## Scenarios (§15.6)

| # | Scenario | Sink | Notes |
|---|----------|------|-------|
| S1 | Ingest throughput sweep | `instant` (+`ingest-only` ceiling) | concurrent ingest+drain; sweep payload × fsync × segment × threads |
| S2 | Append latency under load | `instant` | add `--rate` for a fixed sustainable rate |
| S3 | Recovery time | n/a | `--count` sets log size; `--drop-caches` for a cold number (Linux, root) |
| S4 | Backpressure | `fake-rate` | small `--max-disk-bytes`; check `dropped_total`/`rejected_total` + bounded RSS |
| S5 | Soak / endurance | `fake-rate`/`kinesis` | long `--duration`; watch `rss_peak_bytes`, `disk_bytes_final` |
| S6 | Disconnected backlog → drain | `disconnect`/`kinesis` | `drain_rate_rps` + `time_to_catchup_ms`, no loss |
| S7 | Real-sink drain | `kinesis`/`localstack` | needs `--features kinesis` + an emulator/real stream |
| S8 | Crash during drain | `fake-rate` | crashes mid-drain; `potential_duplicates` = at-least-once redelivery |

## Methodology (§15.7)

- **Run on the device under test**; put `--path` on the target's real volume (the results record
  captures the fs + mount opts on Linux).
- Warm up, then a steady-state window; **≥5 repeats**, report median + p99.
- **S3 cold-cache:** `--drop-caches` (needs root; on Linux it writes `/proc/sys/vm/drop_caches`).
- Storage dominates local-persistence numbers — test the Pi on **microSD** (edge floor) *and*
  USB3-SSD/NVMe (achievable range); try f2fs vs ext4 on flash.

## Captured metrics

Throughput (rps, MB/s, logical write MB/s); append p50/p99/p999/max; export-lag p99; recovery ms;
drain rate + time-to-catch-up; `dropped_total`/`rejected_total`; backlog peak; potential duplicates
(S8); disk bytes; RSS start/peak + CPU% (Linux via `/proc`; `null` elsewhere); plus the env block
(CPU, mem, kernel, fs/mount, build profile).

## Baselines & regression tracking (§15.8)

Each run writes `bench/results/<target>/<scenario>-<git-sha>.json` (via `--results-dir bench/results`).
Commit a baseline per target; in CI, run the same config and:

```sh
cargo run --release --example bench_compare -- \
  --baseline bench/results/lab-5950x/S1-<baseline-sha>.json \
  --current  bench/results/lab-5950x/S1-<new-sha>.json
```

It flags throughput-down / latency-up / recovery-up / RSS-up beyond the threshold and exits 1.
`append_max_ns` is reported but **not** gated (single-outlier noise).

## Targets (§15.4)

| Target | Role | Build |
|--------|------|-------|
| Windows dev box | dev + Windows durability path + upper bound | native `cargo build --release` |
| lab-5950x (Ryzen 5950X, ext4/NVMe) | primary Linux perf + real Kinesis | native or WSL |
| Raspberry Pi 5 (aarch64) | constrained-edge floor (microSD) + range (SSD) | native or cross-compile |
