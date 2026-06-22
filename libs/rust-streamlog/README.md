# ggstreamlog — GGCommons telemetry-streaming core

`ggstreamlog` is the shared Rust core behind the GGCommons **streaming** subsystem (`gg.streams()`):
a store-and-forward telemetry log with an embedded buffer and an asynchronous export engine that
drains to Kinesis or Kafka. It is the streaming engine for **all four** GGCommons languages — Rust
uses it directly; Java, Python, and Node use it through native bindings (see `bindings/`).

It deliberately replaces AWS IoT Greengrass Stream Manager for high-rate telemetry: the buffer,
backpressure, fsync policy, and retention are all under the component's control, with no extra
component dependency or IPC hop.

## What it does

- **Durable, append-only log** with crash-safe segment files, atomic checkpoints, and torn-tail
  recovery — or an **in-memory** ring for best-effort streams where durability isn't needed.
- **Export engine** drains the buffer to a `Sink` on a background thread with at-least-once delivery
  (contiguous-prefix commit), per-offset retries, and configurable backpressure.
- **Pluggable sinks**: Kinesis (`kinesis` feature), Kafka (`kafka` feature), plus test/fake sinks.
- **Per-stream stats** (appended/exported/dropped/retries/backlog/disk-bytes) that the GGCommons
  libraries bridge into the component's metric target.

### Buffer backing — `buffer.type`

| `type` | Backing | Durability | Use for |
|--------|---------|-----------|---------|
| `disk` (default) | `SegmentLog` (file-backed segments) | survives restart; recovered on open | telemetry you can't lose |
| `memory` | `MemoryBlockStore` (RAM ring, `maxDiskBytes` = byte budget, `onFull` evicts) | non-durable; lost on restart | cheap/debug traces where high QoS is unnecessary |

See `bench/results/memory-vs-disk/` for a head-to-head append/throughput comparison.

## Layout

```
src/record.rs        framing: [len][crc32c][offset][ts][pk_len][pk][payload]
src/blockstore/      BlockStore trait; SegmentLog (disk) + MemoryBlockStore (RAM); BackingStore enum
src/log.rs           EmbeddedLog: append / read_batch / commit, fsync policy, retention, group commit
src/export/          ExportEngine (bg thread) + Sink trait + FakeSink; kinesis.rs, kafka.rs
src/config.rs        StreamingConfig / StreamConfig (sink, buffer, batch, delivery) — camelCase JSON
src/service.rs       StreamService: config-driven owner of every stream + its export engine
src/ffi.rs           C-ABI (feature `cabi`) — ggsl_* functions for the Java/Panama binding
include/ggstreamlog.h  the C-ABI header
bindings/python/     PyO3 binding (maturin → abi3 wheel)
bindings/node/       napi-rs binding (cdylib → .node addon)
bench/               perf harness (examples/loadgen.rs, Criterion benches) — see bench/README.md
```

## Build & test

```bash
cargo test                       # core (any OS)
cargo build --features kinesis   # + Kinesis sink (AWS SDK)
cargo build --features kafka     # + Kafka sink (rdkafka; needs cmake + libssl/libcurl dev headers)
cargo build --features cabi      # + C-ABI cdylib (for the Java/Panama binding)
cargo bench --bench append       # Criterion micro-benchmarks
```

Edition 2021, MSRV 1.75. Off-by-default features compose: `kinesis`, `kafka`, `cabi`.

## How the languages use it

- **Rust** (`libs/rust`): `gg.streams()` wraps `ggstreamlog::StreamService` directly, gated behind the
  `streaming` / `streaming-kinesis` / `streaming-kafka` cargo features.
- **Java**: FFM/Panama over the `ggsl_*` C-ABI (`cabi` feature → cdylib bundled in the jar).
- **Python**: PyO3 binding in `bindings/python` (maturin wheel).
- **Node/TypeScript**: napi-rs binding in `bindings/node` (`.node` addon).

All bindings auto-wire `gg.streams()`, the stats→metrics bridge, and core-log forwarding into their
host runtime, and only load the native library when a `streaming` config section is present.

See `docs/TELEMETRY_STREAMING.md` (design) and `docs/TELEMETRY_STREAMING_PHASE1.md` (spec) at the
monorepo root.
