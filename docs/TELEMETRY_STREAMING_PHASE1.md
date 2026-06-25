# Telemetry Streaming — Phase 1 implementation spec (`ggstreamlog` core)

Implementation-ready spec for **Phase 1** of [TELEMETRY_STREAMING.md](./TELEMETRY_STREAMING.md):
the Rust `ggstreamlog` core + `KinesisSink`, HOST, wired into the Rust ggcommons lib,
with crash/fuzz/bench tests. **Pure Rust first** — language bindings, Kafka, SiteWise,
RocksDB/LMDB backends, and GREENGRASS fan-in are later phases.

## 1. Scope

**In Phase 1**
- `ggstreamlog` crate: durable segment log + export engine + `KinesisSink` + AWS credentials
  + stats; config types; the `BlockStore`/`Sink`/`CredentialProvider` traits.
- Rust-native `IStreamService` in `libs/rust` (`gg.streams()`), bridging stats → ggcommons metrics.
- The **C-ABI surface designed** (header) so Phase 2 bindings aren't painted into a corner;
  its implementation is deferred to Phase 2.
- Tests: unit, property, crash-injection, fuzz, concurrency, bench, golden format vectors.

**Out (later phases):** Kafka/SiteWise sinks, Java/Python/Node bindings, RocksDB/LMDB
`BlockStore`, credential providers beyond the AWS default chain, stream priorities,
compression beyond optional zstd, GREENGRASS local-pubsub fan-in.

## 2. Crate layout

A new workspace member `libs/rust-streamlog/` (crate `ggstreamlog`). The existing `ggcommons`
Rust crate depends on it (path dep) for its `IStreamService`.

```
ggstreamlog/
  Cargo.toml                # deps: serde, serde_json, crc32fast, thiserror, tokio (internal rt),
                            #       aws-config, aws-sdk-kinesis, tracing; dev: proptest, criterion
  src/
    lib.rs                  # public Rust API (StreamService, StreamHandle, Record, traits)
    error.rs               # GgStreamError (thiserror)
    config.rs              # StreamingConfig/StreamConfig/BufferConfig/BatchConfig (serde + defaults + validate)
    record.rs             # Record; frame encode/decode; crc32c
    blockstore/
      mod.rs              # BlockStore trait, RecordReader, RecoveryReport, TruncateOutcome
      segment_log.rs      # default v1 store: directory of segments + checkpoint
      segment.rs          # one .seg writer/reader + .idx sidecar
      checkpoint.rs       # atomic checkpoint file
    log.rs                 # EmbeddedLog: append/read/commit/retention over a BlockStore + ingest thread
    export/
      mod.rs              # ExportEngine state machine; Sink trait; ExportRecord; SendOutcome
      kinesis.rs          # KinesisSink (aws-sdk-kinesis)
    creds.rs               # CredentialProvider trait + AwsDefaultCredentials (P1)
    metrics.rs             # Stats
    ffi.rs                 # C-ABI (feature = "cabi"; designed P1, fully wired P2)
  tests/  { crash_recovery.rs, conformance.rs, export_at_least_once.rs }
  fuzz/   { fuzz_targets/segment_reader.rs, recovery.rs }     # cargo-fuzz
  benches/ { append.rs, recovery.rs }                          # criterion
  testdata/golden/        # byte-exact on-disk vectors (format conformance)
```

## 3. On-disk format (normative)

All integers **little-endian**. One directory per stream.

```
<stream-dir>/
  meta.json                       # {"format":1,"stream":"telemetry","segmentBytes":..,"createdMs":..}
  checkpoint                      # see §3.3 (written temp + atomic rename)
  00000000000000000000.seg + .idx # segments named by zero-padded u64 base offset
  00000000000000065536.seg + .idx
```

### 3.1 Segment file (`.seg`)
```
SegmentHeader (32 bytes):
  [u32 magic = 0x4C53_4747 ("GGSL")][u16 format=1][u16 flags][u64 base_offset]
  [u64 created_ms][u32 header_crc32c]              # crc over the preceding 28 bytes
Then a sequence of Records:
  [u32 frame_len]                                  # bytes that follow, i.e. crc..payload
  [u32 crc32c]                                     # crc over offset..payload (everything after this field)
  [u64 offset]                                     # absolute, monotonic per stream
  [u64 ts_ms]
  [u16 pk_len][pk bytes]
  [payload bytes]                                  # payload_len = frame_len - (4+8+8+2+pk_len)
```
- A record is **valid** iff `frame_len` fits in the file remainder, `crc32c` matches, and
  `offset == expected`. The first invalid record marks the **torn tail** (truncate from there).
- Active segment **rolls** when adding a record would exceed `segment_bytes`, or its age exceeds
  `segment_max_age` (if set). A record larger than `segment_bytes` gets its own segment (never split).

### 3.2 Index sidecar (`.idx`)
Fixed 16-byte entries `[u64 offset][u64 byte_pos]`, one every `index_interval` records (default 4096),
for O(1)-ish seek to the checkpoint on restart. Rebuildable from the `.seg` if missing/corrupt
(index is a cache, never the source of truth).

### 3.3 Checkpoint file
```
[u64 acked_offset]     # highest offset durably delivered to the sink (exclusive cursor = acked+1)
[u64 drop_floor]       # lowest offset still retained (advances on dropOldest)
[u32 crc32c]
```
Written to `checkpoint.tmp` then `rename()` over `checkpoint` (atomic on POSIX/Windows for same dir).
On a torn checkpoint (bad crc), fall back to the previous good value embedded in the active
segment scan (conservative: re-deliver — at-least-once).

## 4. Core types & traits (Rust API)

```rust
pub struct Record { pub partition_key: String, pub timestamp_ms: u64,
                    pub payload: Vec<u8>, pub headers: Vec<(String, Vec<u8>)> }

pub struct StreamService { /* owns streams + one internal tokio rt for sinks */ }
impl StreamService {
    pub fn open(cfg: StreamingConfig) -> Result<Self>;     // opens/recovers every configured stream
    pub fn stream(&self, name: &str) -> Result<StreamHandle>;
    pub fn shutdown(self) -> Result<()>;                   // flush in-memory → disk, stop engines
}

#[derive(Clone)]
pub struct StreamHandle { /* Arc to the stream's Log */ }
impl StreamHandle {
    pub fn append(&self, rec: Record) -> Result<u64>;       // returns offset; semantics per onFull (§6)
    pub fn append_batch(&self, recs: Vec<Record>) -> Result<()>;
    pub fn flush(&self) -> Result<()>;                      // block until in-memory queue is fsynced to disk
    pub fn stats(&self) -> Stats;
}
```

**`BlockStore`** — durability seam (segment_log in P1; RocksDB/LMDB later). Owns framing + CRC +
segments + checkpoint so a future KV backend can be swapped wholesale.
```rust
pub trait BlockStore: Send {
    fn recover(&mut self) -> Result<RecoveryReport>;            // -> { next_offset, torn_truncated, ... }
    fn append(&mut self, offset: u64, ts_ms: u64, pk: &[u8], payload: &[u8]) -> Result<()>;
    fn sync(&mut self) -> Result<()>;                          // fsync active segment
    fn reader_from(&self, offset: u64) -> Result<Box<dyn RecordReader>>;  // forward iterator
    fn truncate_below(&mut self, offset: u64) -> Result<TruncateOutcome>; // retention; reclaimed bytes
    fn load_checkpoint(&self) -> Result<(u64 /*acked*/, u64 /*drop_floor*/)>;
    fn store_checkpoint(&mut self, acked: u64, drop_floor: u64) -> Result<()>;
    fn disk_bytes(&self) -> u64;
    fn oldest_ts_ms(&self) -> Option<u64>;
}
```

**`Sink`** — export seam (KinesisSink in P1).
```rust
pub struct ExportRecord<'a> { pub offset: u64, pub partition_key: &'a str,
                              pub ts_ms: u64, pub payload: &'a [u8] }
pub enum SendOutcome {
    AllAcked,
    Partial { failed_offsets: Vec<u64> },     // these were NOT stored; retry them
    Failed  { retryable: bool, error: String } // whole batch failed (e.g. disconnected)
}
#[async_trait] pub trait Sink: Send {
    async fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome;
}
```

**`CredentialProvider`** — designed now; P1 implements only `AwsDefaultCredentials` (the SDK chain).

## 5. EmbeddedLog operations (algorithms)

- **append(rec)** → push onto a bounded in-memory queue (`max_buffered_records`/`_bytes`). The
  single **writer thread** drains the queue: assign `next_offset`, `BlockStore::append`, group-commit
  `sync()` per fsync policy, then make those offsets readable. `append` returns the assigned offset.
- **read path (export)** → a reader from `acked_offset+1`; the ExportEngine pulls batches (§7).
- **commit(offset)** → `store_checkpoint(offset, drop_floor)`, then run retention.
- **retention** (after each commit + on a `max_age` timer):
  1. `truncate_below(min(acked_offset+1, drop_floor))` — reclaim fully-delivered segments.
  2. If `disk_bytes > max_disk_bytes` (or oldest age > `max_age_secs`) with un-acked data, apply
     `on_full`:
     - **DropOldest** (default): advance `drop_floor` past the oldest segment, delete it,
       `dropped_total += n`; if `drop_floor > acked_offset` we dropped *un-exported* data — fast-forward
       `acked_offset = drop_floor` and **log a warning** (never silent).
     - **Block**: stop accepting into the ingest queue (append blocks) until retention frees space.
     - **RejectNew**: `append` returns `Err(BufferFull)`.
- **recovery (open)** → read `meta` (reject on `segmentBytes` mismatch unless `--migrate`), load
  checkpoint, locate the active (highest-base) segment, scan it to find `next_offset`, validate the
  **tail record CRC and truncate a torn tail**, rebuild a missing `.idx`.

### Fsync policy (the throughput↔durability dial)
`PerBatch` (default) — one fsync per group-commit drain. `Interval(ms)` — a background timer fsyncs;
`append` returns before fsync (faster, wider crash window). `Always` — fsync every record (safest).

### `flush()` vs delivery
`flush()` = "in-memory queue is durably on disk" (drain + fsync). It does **not** wait for the sink.
A separate `await_drained(timeout)` (P1: internal/test-only) waits until `acked_offset` catches up —
used by `shutdown()` with a bounded timeout.

## 6. Concurrency & threading

Per stream:
- **Ingest queue**: bounded MPSC (`crossbeam`/`std`), the backpressure point.
- **Writer thread** (1): sole owner of the active segment; framing + fsync + offset assignment.
- **Export task** (1): runs on the core's **internal tokio runtime** (the AWS SDK is async); the log
  itself is sync std-threads, so **FFI hosts never need a runtime**. The export task waits on a
  `Notify`/condvar signalled by the writer when new committed data exists.
- **Retention**: synchronous on commit + a periodic timer task.

`StreamHandle` is `Clone + Send + Sync`; `append` is callable from many host threads (the queue is
the synchronization point). One `tokio` multi-thread runtime is shared across all streams' export
tasks. No `tokio` leaks across the public API or the C ABI.

## 7. ExportEngine + KinesisSink

**ExportEngine loop (per stream):**
1. `read_batch` from `acked_offset+1`, bounded by `batch.max_records` / `max_bytes`; if empty, wait on
   the new-data signal **or** `batch.max_latency_ms` (flush a partial batch on latency).
2. `sink.send(batch)`:
   - `AllAcked` → `commit(last_offset)`.
   - `Partial{failed}` → retry **only the failed offsets** (same data) with backoff until they ack,
     **then** `commit(last_offset)`. Checkpoint only ever advances over a fully-acked contiguous prefix,
     preserving order and at-least-once. (Kinesis partial failures don't double-store acked records, so
     this adds no duplicates; dups arise only from a crash between `send` and `commit` → downstream
     dedup by `(partition_key, sequence)`.)
   - `Failed{retryable}` → exponential backoff + full jitter (cap `delivery.backoff_max_ms`),
     `retries_total++`; honor `delivery.max_retries` (`-1` = forever — the disconnected case; data stays
     buffered and retention governs disk).
3. Loop.

**KinesisSink:** `aws-sdk-kinesis` `PutRecords` (≤ **500 records / 5 MiB** per call → these cap
`batch.max_records`/`max_bytes`); `partition_key` from the record; parse `FailedRecordCount` +
per-entry `ErrorCode` → `Partial{failed_offsets}`; classify `ProvisionedThroughputExceededException` /
5xx / timeouts as `Failed{retryable:true}`. Optional per-record **zstd** (header flag) if
`batch.compression="zstd"`; KPL-style aggregation is a later optimization.

## 8. Credentials (P1)

Only AWS creds for Kinesis. `AwsDefaultCredentials` uses `aws-config`'s default provider chain, which
already covers env, profile, IMDS, and the **GG TES** container-credentials endpoint
(`AWS_CONTAINER_CREDENTIALS_FULL_URI`) — so HOST and (future) GREENGRASS both work with no
special code. `region` from config or the chain. The `CredentialProvider` trait exists for Phase-3
Kafka secrets but isn't otherwise implemented in P1.

## 9. Config (serde → the YAML schema)

```rust
pub struct StreamingConfig { pub streams: Vec<StreamConfig> }
pub struct StreamConfig { pub name: String, pub sink: SinkConfig,
    pub buffer: BufferConfig, pub batch: BatchConfig, pub delivery: DeliveryConfig }
pub struct BufferConfig { pub path: String, pub segment_bytes: u64 /*64MiB*/, pub max_disk_bytes: u64,
    pub max_age_secs: Option<u64>, pub on_full: OnFull /*DropOldest*/, pub fsync: FsyncPolicy /*PerBatch*/,
    pub fsync_interval_ms: u64, pub max_buffered_records: usize, pub index_interval: u32 }
pub struct BatchConfig { pub max_records: u32 /*500*/, pub max_bytes: u64 /*4MiB*/,
    pub max_latency_ms: u64 /*1000*/, pub compression: Compression /*None*/ }
pub struct DeliveryConfig { pub max_retries: i64 /*-1*/, pub backoff_max_ms: u64 }
pub enum SinkConfig { Kinesis { stream_name: String, region: Option<String> } }  // Kafka/SiteWise later
```
`validate()`: `segment_bytes>0`, `max_disk_bytes>=segment_bytes`, Kinesis batch caps not exceeded, path
writable. Hot-reload (Phase 2+) can change `batch.*`/`max_disk_bytes`/`on_full` live; `path`/
`segment_bytes` are immutable for an open stream.

## 10. Stats / observability

```rust
pub struct Stats { pub appended_total: u64, pub exported_total: u64, pub dropped_total: u64,
    pub retries_total: u64, pub buffered_records: u64, pub buffered_bytes: u64, pub disk_bytes: u64,
    pub acked_offset: u64, pub next_offset: u64, pub oldest_unacked_age_ms: u64,
    pub last_export_error: Option<String> }
```
The Rust lib bridges these into the existing `metrics`/`heartbeat` targets (incl. CloudWatch).
`dropped_total > 0` makes `DropOldest` visible.

## 11. C-ABI (designed P1, implemented P2)

> Supersedes the read_batch/commit sketch in TELEMETRY_STREAMING.md §5.3: because the **export
> engine + sinks live in the core**, the host does **not** drive export. The ABI is just
> append/flush/stats/lifecycle (+ a credential callback for Phase-3 Kafka). Config is passed as a
> JSON string to avoid a wide struct ABI.

```c
typedef struct ggsl_service ggsl_service;
typedef struct ggsl_stream  ggsl_stream;

/* All functions: return 0 on success, non-zero on error; *err (heap str) set on error, free with ggsl_str_free. */
int  ggsl_open(const char* config_json, ggsl_service** out, char** err);
int  ggsl_stream_get(ggsl_service*, const char* name, ggsl_stream** out, char** err);
int  ggsl_append(ggsl_stream*, const uint8_t* pk, uint16_t pk_len, uint64_t ts_ms,
                 const uint8_t* payload, uint32_t payload_len, uint64_t* out_offset, char** err);
int  ggsl_flush(ggsl_stream*, char** err);
int  ggsl_stats(ggsl_stream*, ggsl_stats_t* out);     /* caller-provided struct */
void ggsl_shutdown(ggsl_service*);                     /* flush + stop + free */
void ggsl_str_free(char*);
```
**ABI rules:** inputs are borrowed (caller owns); outputs are caller-struct or core-allocated +
explicit free; `append` is thread-safe; **every entry point wraps `catch_unwind`** so a Rust panic
never crosses the boundary (returns an error instead). Bindings: Panama (Java 21), PyO3/maturin
(Python), napi-rs (Node) in Phase 2.

## 12. Test plan

| Kind | What it asserts | Tooling |
|------|-----------------|---------|
| Unit | frame encode/decode roundtrip + CRC; config defaults/validate; batch-assembly limits | `#[test]` |
| Property | append N arbitrary records → read back identical order/content; framing roundtrip for any pk/payload | `proptest` |
| **Crash-injection** | crash at every fsync point and mid-write (truncate at random byte) → recover → **no committed record lost, torn tail truncated, next_offset/checkpoint correct** | fault-injecting `BlockStore` wrapper + a subprocess kill harness |
| Fuzz | feed arbitrary bytes to the segment reader + recovery → **never panic/UB**; bad CRC always detected | `cargo-fuzz` |
| Concurrency | many appenders + exporter + retention → ordering per-pk preserved, no loss beyond at-least-once, no deadlock | stress test (+ `loom` on the queue) |
| Export semantics | `FakeSink` injecting partial/transient failures → only failed offsets retried, checkpoint advances contiguously, at-least-once across a simulated crash | `tests/export_at_least_once.rs` |
| Sink integration | `KinesisSink` against **LocalStack** (and a real stream, gated) — PutRecords limits, partial-failure handling, throttling backoff | gated integration |
| Bench | append throughput × {fsync policy, payload size, segment size}; recovery time × log size; export batch efficiency | `criterion` (baseline, no fixed target) |
| **Golden conformance** | byte-exact on-disk vectors in `testdata/golden/` — pins the format so Phase-2 bindings + future BlockStores stay compatible | `tests/conformance.rs` |

## 13. Build order (milestones)

1. `record` framing + `segment`/`checkpoint` + `segment_log` `BlockStore` + recovery — with unit /
   property / crash / fuzz / golden tests. *(The durability foundation; gets the hardest correctness
   work done first.)*
2. `EmbeddedLog` (append/read/commit/retention/dropOldest/backpressure) + writer thread + concurrency
   tests + append bench.
3. `ExportEngine` + `Sink` trait + `FakeSink` + at-least-once/partial tests.
4. `KinesisSink` (`aws-sdk-kinesis`) + LocalStack integration.
5. Rust-native `StreamService` + wire into `libs/rust` `IStreamService` (`gg.streams()`) + stats→metrics bridge.
6. Finalize the C-ABI header (impl deferred to Phase 2).

## 14. Decisions & risks (Phase 1)

- **Internal tokio runtime** for the async AWS SDK; the log/append path is sync so FFI hosts need no runtime.
- **Partial-failure checkpointing** = contiguous-prefix only (retry failed, then commit) — preserves
  order + at-least-once without extra duplicates.
- **DropOldest of un-exported data** fast-forwards `acked_offset` and is always counted + logged.
- **`flush()` = durable-to-disk, not delivered-to-sink** (delivery waited only by `shutdown`/tests).
- **Index is a cache** — correctness never depends on it; always rebuildable from `.seg`.
- Risks: getting crash-recovery + torn-tail truncation provably correct (mitigated by the
  crash-injection + fuzz + golden suites — the bulk of milestone 1); `aws-sdk-kinesis` adds weight to
  the core cdylib (watch footprint for constrained edge devices).

## 15. Performance testing (local persistence + draining)

Per the decisions, there is **no fixed throughput target** — peak rate is a function of hardware +
config. So the goal of perf testing is to **establish per-(target × config) baselines, characterize
the throughput↔durability curve, find the bottleneck (CPU vs disk vs network vs fsync), and catch
regressions** — not to pass an absolute number.

### 15.1 The two halves to measure separately

- **Local persistence (ingest path)** — `append` → in-memory queue → writer thread → segment +
  fsync. Bottleneck is usually **disk + fsync**. Isolate it with a no-op/instant sink so the network
  is out of the picture. Also covers **recovery time** and **dropOldest under disk pressure**.
- **Draining (export path)** — ExportEngine → batch → sink → checkpoint. Bottleneck is the **sink +
  network** (or, for backlog catch-up, disk read + sink). Measured with a rate-limited FakeSink (to
  isolate the engine) and with the real `KinesisSink`/LocalStack (to measure true drain).

### 15.2 Metrics captured (per run)

Throughput (records/s, MB/s); **append latency** p50/p99/p999; **end-to-end latency** (append→acked);
CPU%; RSS (leak watch); disk write bandwidth + IOPS + fsync/s; **recovery wall-clock**; `dropped_total`;
**export lag** (oldest-unacked age); drain rate + time-to-catch-up after reconnect. Plus the env block
(§15.7).

### 15.3 Parameter matrix (swept)

| Dimension | Values |
|-----------|--------|
| Payload size | 256 B, 1 KiB, 4 KiB, 64 KiB |
| fsync policy | `PerBatch`, `Interval(1000ms)`, `Always` |
| Segment size | 16 MiB, 64 MiB, 256 MiB |
| Appender threads | 1, 4, N(cores) |
| `on_full` | `dropOldest`, `block` |
| Sink | `none` (instant), `fake-rate` (capped), `kinesis`/`localstack` |
| Storage (Pi) | microSD, USB3-SSD, NVMe-HAT |

### 15.4 Test targets

| Target | Role | CPU / arch | RAM | Storage | FS | What it validates |
|--------|------|-----------|-----|---------|----|-------------------|
| **Windows dev box** (this machine) | dev + cross-platform correctness + upper bound | x86_64, many cores | high | NVMe | NTFS | the **Windows durability path**: `FlushFileBuffers` as fsync, atomic `rename()` over an open dir, file-locking; not a deployment target |
| **lab-5950x** | **primary Linux perf** + powerful-gateway profile + later GG integration | Ryzen 5950X (16c/32t), x86_64 | high | NVMe | ext4 | headline throughput, soak/endurance, real Kinesis drain |
| **Raspberry Pi 5** | **constrained-edge reality check** + aarch64 | Cortex-A76 4-core, arm64 | 4/8/16 GB | **microSD (worst case)** and/or USB3-SSD / NVMe-HAT | ext4 / f2fs | the real edge limits: fsync latency on flash, memory pressure, `dropOldest` under a slow disk, the **aarch64 build** |

**Storage dominates the local-persistence numbers — call it out explicitly.** A microSD card has poor
random-write + very high `fsync` latency and periodic wear-leveling stalls; it's the meaningful *floor*
for an edge device, so test on it — but also test the Pi with a **USB3-SSD/NVMe-HAT** to show the
achievable range, and recommend SSD for real high-rate workloads. On flash, also try **f2fs** vs ext4
(f2fs is flash-friendly) and note SD endurance/wear in soak runs. `Always` fsync on microSD will be
dramatically slower than `PerBatch` — this is exactly the knob the matrix exposes.

### 15.5 Harness

- **Micro (criterion)** — `benches/append.rs`, `benches/recovery.rs`: append latency/throughput per
  config; recovery time vs log size. Stable, in-process, for the throughput↔fsync curve.
- **Load generator** — a `loadgen` example binary driving end-to-end scenarios:
  `--rate <r|unbounded> --payload <bytes> --threads <n> --duration <s> --fsync <policy>
   --segment-bytes <n> --on-full <p> --sink <none|fake-rate:<r>|kinesis|localstack> --scenario <S1..S8>`.
  Emits one **JSON results record** per run (→ baseline matrix + regression diff, §15.8).
- **FakeSink modes**: `none` (instant ack — isolates disk), `fake-rate:<r/s>` (caps drain to model a
  slow/limited sink), `disconnect(T)-then-ack` (for backlog + drain). Keeps perf runs offline + free.

### 15.6 Scenarios

| # | Scenario | Measures | Sink |
|---|----------|----------|------|
| **S1** | Ingest throughput sweep | max append rate per fsync × payload × segment (disk-bound) | `none` |
| **S2** | Append latency under load | p50/p99/p999 append latency at a fixed sustainable rate | `none` |
| **S3** | Recovery time | `open()`/recover wall-clock vs log size (1e6 / 1e7 / 1e8 records); correctness | n/a |
| **S4** | Backpressure | `dropOldest`/`block`/`rejectNew` behavior + `dropped_total` + bounded RSS when producer > disk | `fake-rate` |
| **S5** | Soak / endurance | hours at a fixed rate: RSS (no leak), disk steady-state (retention), SD latency drift/wear | `fake-rate` / `kinesis` |
| **S6** | Disconnected backlog → drain | accumulate backlog B over outage T, reconnect, measure **drain rate + time-to-catch-up**, no loss, order preserved | `disconnect-then-ack` / `kinesis` |
| **S7** | Real-sink drain | export throughput vs shard count, PutRecords batch efficiency, throttling/backoff | `kinesis` / `localstack` |
| **S8** | Crash during drain | kill mid-drain → restart time + resume-from-checkpoint correctness (at-least-once) | `fake-rate` |

### 15.7 Methodology

- **Run on the device under test** (don't extrapolate across arch/disk). Put the buffer dir on the
  target's real volume; record `path`'s fs + mount opts.
- **Warm-up** then a steady-state measurement window; **≥5 repeats**, report **median + p99**.
- **Recovery (S3)**: drop the page cache before each measurement (`echo 3 >
  /proc/sys/vm/drop_caches` on Linux) so you measure disk, not cache.
- **Isolate**: dedicated path/partition if possible; no competing load; for the Pi, watch thermals
  (throttling) and power (a weak PSU throttles the A76).
- **Env block** captured into each result: CPU model, disk model (`lsblk`/`smartctl`/`wmic`), fs +
  mount opts, kernel, `rustc` version, build profile (`release` + `lto`), and the config used.

### 15.8 Baselines & regression tracking

No absolute pass/fail. Each run writes `bench/results/<target>/<git-sha>.json`; a small comparator
flags any metric that regresses **> 10%** vs the recorded baseline for the **same target + config**
(throughput down, latency/recovery/RSS up). Publish a per-target baseline matrix in the docs so the
achievable envelope on Windows / 5950X / Pi-5 (SD vs SSD) is visible. Treat the Pi-on-microSD numbers
as the conservative edge floor and the 5950X/NVMe numbers as the high end.

### 15.9 Building/running on each target

- **Windows** — native `cargo build --release` + `cargo bench`; run `loadgen` against an NTFS path.
- **lab-5950x** — build in WSL (cargo at `~/.cargo/bin`) or natively on the box; copy the `loadgen`
  binary + run against an ext4 path; LocalStack/real Kinesis for S6/S7.
- **Raspberry Pi 5 (aarch64)** — cross-compile (`cross`, `cargo-zigbuild`, or a WSL `aarch64-unknown-
  linux-gnu` target) **or** build natively on the Pi; run `loadgen` against microSD **and** a
  USB3-SSD/NVMe-HAT path to capture both ends of the storage range. (Confirms the aarch64 build, which
  the future bindings + cross-compile work will also need.)
