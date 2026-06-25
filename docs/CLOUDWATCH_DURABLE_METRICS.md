# Design & plan — durable CloudWatch metrics buffer (cross-language)

> **Status: IMPLEMENTED across the core + all four languages** (see "Implementation status" below).
> Gives the direct `cloudwatch` metric target a durable, disk-backed store-and-forward buffer that drains
> `PutMetricData` on reconnect, by **reusing the `ggstreamlog` durable log + export engine via a new
> host-callback sink**. Java canonical; per-language deltas below.
>
> **This is a standalone enhancement, independent of the Kubernetes / platform-rearch proposal**
> (`docs/k8s/`). It depends only on already-shipped pieces — the existing `cloudwatch` metric target and
> the `ggstreamlog` core + bindings — and is **not** gated by the platform×transport rearch or any
> KUBERNETES profile. Its driver is *intermittent cloud connectivity*, which already describes today's
> **GREENGRASS edge** and **STANDALONE** deployments, so it improves the current product regardless of
> k8s. It was surfaced by the k8s metrics review (k8s requirement **FR-MET-5**) and can be sequenced
> before, after, or in parallel with that effort.

## Implementation status (delivered)

Implemented + tested across the `ggstreamlog` core and all four language libs. New-code coverage exceeds
the 90% target everywhere it was measured, and **every track has a passing disconnect fault-injection
test** (flat memory / disk-bounded backlog / `dropOldest` / drain-on-reconnect / nonzero stale-drop).

| Track | Build / tests (verified) | New-code coverage | Disconnect test |
|---|---|---|---|
| Core C-ABI (`ggsl_set_sink_callback`) | `cargo test --features cabi` → **44 pass, 0 fail** (9 new `ffi_sink_callback`) | `ffi.rs` new code **92.6%** (cargo-llvm-cov) | ✅ |
| Rust lib (`CloudWatchSink`, feature `metrics-cloudwatch-durable`) | `cargo test --features metrics-cloudwatch-durable` → **133 pass** (16 new) | `cloudwatch_durable.rs` **95.76%** | ✅ |
| Java (Panama upcall + `CloudWatchDrain`) | feature suites pass; closes the Java streaming template-resolution gap | feature classes **94–100%** line (JaCoCo) | ✅ |
| TypeScript (napi threadsafe-fn + `DurableCloudWatchTarget`) | `npm run build` + **246 vitest pass** | `cloudwatch_durable.ts` **98%** | ✅ |
| Python (PyO3 GIL callback + `cloudwatch_durable.py`) | **35 pass** | `cloudwatch_durable.py` **100%** | ✅ |
| Schema | `sync-schema.sh --check` **green** (6-file sync) | — | — |

**Pre-existing gate failures (NOT introduced by this feature — verified by stashing the change):** the Java
`mvn package` suite has 10 pre-existing errors (stray local logging tests referencing a removed
`logging.format` key; gitignored TLS test resources per commit `fc993b3`), which prevents reaching the
JaCoCo *bundle* gate; the Java bundle (80%) and TS global (77%) coverage gates are dragged below threshold
by pre-existing untested files (`gg_verify.ts`, credentials, …); and `libs/rust` has one stale doctest
(`config/validation.rs`, predates this work; contradicts `required:[component]`). These are tracked
separately and do not affect this feature's own code, which passes and is ≥94% covered.

## 0. Decisions (locked)

| # | Decision |
|---|---|
| Q1 | **Host-callback sink** (reuse core buffer + export loop; CloudWatch send stays in the metrics layer). Additive: Kinesis/Kafka stay native in-core; the callback is also a **public bring-your-own-sink** extension point (see §3a). |
| Q2 | **Always-buffer** (every metric goes disk → drain; no separate "spill on failure" path). |
| Q3 | **Durability is a runtime config decision** (`buffer: durable\|memory`); native lib always bundled (Java/Python/TS), cargo-feature-gated in Rust. **Default `durable`** for cloudwatch. |
| Q4 | Record = compact JSON `{namespace, datum}`; partition key = **namespace**. |
| Q5 | **Single buffer**, group-by-namespace **in the sink**. |
| Q6 | **Drop stale on drain + counter** (datums outside CloudWatch's ~2wk-past/~2h-future window). |
| Q7 | **Defer aggregation** (replay raw in v1; design the sink so StatisticSet folding can be added later). |
| Q8 | `onFull: dropOldest` + **configurable** `maxDiskBytes`; `fsync: perBatch`. |
| Q9 | **Direct `cloudwatch` target only** (prometheus / log-EMF-stdout / messaging / cloudwatchcomponent unaffected). |
| Q10 | **Java-canonical-first**, then Rust → TS → Python; close the Java streaming-template-resolution gap. |
| S1 | **Node callback threading: VALIDATED by spike** (§9) — the blocking callback works; the `read_batch`/`commit` queue-API fallback is **not needed**. |
| S2 | **Self-metrics (buffer stats) emit on a non-buffered path** (log + `stats()` + heartbeat; if pushed to CloudWatch, send direct/unbuffered) to avoid a feedback loop. |
| S3 | **Bring-your-own-sink is exposed + documented** as a public sink type (§3a). |
| S4 | Integration test: use floci if it emulates `PutMetricData`; otherwise a **stub HTTP endpoint** asserting request shape/limits. |

> CloudWatch limits relied on (confirm against current AWS docs at implementation): `PutMetricData`
> ≤ **1000 datums** and ≤ **~1 MB** per request; timestamps accepted **~2 weeks past to ~2 hours future**.

## 1. Architecture — two sink kinds

```
                          ggstreamlog core (Rust)
   append(record) ─► EmbeddedLog (durable segment log) ─► ExportEngine (read_batch → sink.send → commit;
                                                            at-least-once, retry/backoff, single-writer)
                                                                     │ sink: dyn Sink
                              ┌──────────────────────────────────────┼───────────────────────────────┐
                       native in-core sinks                    host-callback sink (NEW, public)
                       KinesisSink / KafkaSink                  CallbackSink ─► host fn(batch)->Outcome
                       (write-once, feature-gated)              (CloudWatch send / bring-your-own-sink)
```

**Decision rule** (why not move everything to callbacks): a sink is **native in-core** when its impl
should be shared across all four languages and does not already exist per-language (Kinesis, Kafka — the
core's whole "write export once" value); it is a **host-callback** when the impl already lives in (or is
more naturally owned by) the host — CloudWatch (reuses the existing per-language SDK + datum builder) or a
component-specific custom destination. The callback never replaces the native sinks.

## 2. Core + ABI delta (the reusable half)

**Core (`libs/rust-streamlog/`):**
- Add `export/callback.rs`: `CallbackSink { send_fn }` implementing the existing `Sink` trait; `send(batch)`
  invokes `send_fn(batch) -> SendOutcome`, mapped onto the existing
  `AllAcked | Partial{failed_offsets} | Failed{retryable}` (commit/retry/backoff unchanged).
- Select it via the existing `open_with(SinkFactory)` seam (`service.rs:131`) + a `SinkConfig::Callback`
  variant. No change to `EmbeddedLog`, retry, backoff, or commit semantics.

**C ABI (`ffi.rs`) — one new primitive, mirroring `ggsl_set_log_callback`:**
- `ggsl_set_sink_callback(service, ctx, fn_ptr)` where `fn_ptr` is
  `extern "C" fn(ctx, *const Record, n, *mut Outcome) -> status`; called on the export thread with a borrowed
  batch; host fills `Outcome` (status + out-array of failed offsets) before returning. `catch_unwind`-wrapped.
- `Record` view: `{ offset:u64, ts_ms:u64, pk:*const u8, pk_len, payload:*const u8, payload_len }`.

**Rust lib (`libs/rust/`):** no FFI — the Rust metrics module implements the `Sink` trait **directly**
in-process (a `CloudWatchSink`), validating the core extension with zero marshalling. The ABI callback is
only the bridge for Java/Python/TS.

## 3. Metrics-side CloudWatch drain (the host callback)

In the existing `CloudWatch` metric target (`CloudWatch.java` + mirrors):

**On `emit`/`emitNow` (when `buffer: durable`):** build the datum as today (reuse `Metric`/`EmfHelper`/SDK
`MetricDatum`) → serialize to `{namespace, datum}` (pk = namespace) → `append` to the component's CloudWatch
ggstreamlog stream (replaces the in-memory `pendingMetrics` queue).

**On drain (the sink callback):** deserialize batch → **group by namespace** → **pre-filter stale datums**
(ts outside the window; `dropped_stale++`; never sent) → chunk to ≤1000/≤1 MB → `PutMetricData` (existing SDK
client) → map outcome (all ack → `AllAcked`; throttle/5xx/transport → `Failed{retryable}`; residual per-datum
reject → `Partial` drop). All CloudWatch specifics stay in the metrics layer; the durable log + at-least-once
loop come from the core.

## 3a. Public bring-your-own-sink (S3)

`SinkConfig::Callback` / the `ggsl_set_sink_callback` ABI is exposed as a **documented public extension**:
a component (or a downstream library) can register a host sink to drain a `ggstreamlog` stream to any
destination (a proprietary on-prem system, a local file, a custom protocol) while reusing the durable buffer
+ export orchestration. The documented **host-sink contract**: `send(batch) -> {AllAcked | Partial{failed
offsets} | Failed{retryable}}`; the callback is invoked on the export thread and must be thread-safe and
return promptly; partial/failed offsets follow the same at-least-once commit rules as the native sinks.
CloudWatch is the first in-tree consumer of this contract.

## 4. Config & schema

Add a `buffer` object to the `cloudwatch` branch of `metricEmission.targetConfig` (which is
`additionalProperties:false` → must be declared; schema is a **6-file synced commit**):

```jsonc
"metricEmission": { "target": "cloudwatch", "targetConfig": { "cloudwatch": {
  "namespace": "...", "intervalSecs": 60,
  "buffer": {
    "type": "durable",                 // durable | memory   (default: durable)
    "path": "/var/lib/ggcommons/metrics/{ComponentName}/cw",
    "maxDiskBytes": 134217728,         // ~128 MiB default; configurable
    "onFull": "dropOldest", "fsync": "perBatch"
  } } } }
```

- `durable` → ggstreamlog disk buffer + CallbackSink; `memory` → today's in-memory batching. **Runtime**
  decision; default `durable`.
- Path templating (`{ComponentName}`/`{ThingName}`) **closes the Java streaming-template-resolution gap**
  (`StreamService.java:46`) for this path.
- Rust: a cargo feature pulls in `ggstreamlog`; absent it, `type: durable` errors clearly. Java/Python/TS:
  native lib bundled, loaded on demand.

## 5. Lifecycle & semantics

- `emit`/`emitNow` → `append`. `flush()` → drain now. `close()` → **flush to disk (fsync) + stop the engine;
  do NOT drain to cloud** (backlog persists, resumes on restart; shutdown budget = fsync + unsubscribe only).
- **Self-observability without recursion (S2):** buffer `dropped_stale` / backlog depth /
  `oldest_unacked_age_ms` / retries are surfaced via the log + `stats()` + heartbeat; if published to
  CloudWatch they go on a **direct, unbuffered** `PutMetricData` path (never through the same buffer).

## 6. Disconnect-tolerance behavior (the acceptance)

During a lengthy disconnect: datums accumulate **on disk** (bounded by `maxDiskBytes`; `dropOldest` on
overflow), **memory stays flat**, the component keeps running. On reconnect the engine drains; datums aged
past the CloudWatch window are dropped + counted. This is exactly what the in-memory target lacks today.

## 7. Parity & sequencing (independent of Phase 0)

1. **Rust core**: `CallbackSink` + `SinkConfig::Callback` + `ggsl_set_sink_callback` (with a `FakeSink`-style callback test).
2. **Rust lib (canonical for sink logic)**: `CloudWatchSink` implementing `Sink` directly — serialization, namespace grouping, stale-drop, chunking, outcome mapping.
3. **Java (canonical for the language binding)**: Panama upcall stub; reuse the SDK client + `EmfHelper`/`Metric`; resolve the buffer path.
4. **TS**: napi-rs threadsafe-function bridge — **pattern validated** (§9): `ThreadsafeFunction` (fire-and-forget) hands the batch to JS; a `resolveOutcome(id, outcome)` napi fn lets the async JS sink signal the result back, unblocking the export thread. (Production refinement: the resolve carries failed-offsets to map onto `Partial`, and is wired internally by the metrics layer, not exposed as public API.)
5. **Python**: PyO3 callback acquiring the GIL; reuse boto3.

No change to the wire envelope, vault, or interop. This sequence can run as its own release, decoupled from
the platform-rearch phasing.

## 8. Test matrix

- **Unit** (each lang): record round-trip; namespace grouping; stale pre-filter + counter; 1000/1 MB chunking; outcome mapping.
- **Core**: `CallbackSink` drives the export loop (commit only on `AllAcked`; `Failed` re-delivers; at-least-once duplicate on crash-between-send-and-commit).
- **Integration (S4)**: drain to floci if it emulates `PutMetricData`; else a **stub HTTP endpoint** asserting request shape/limits.
- **Disconnect fault-injection (headline)**: sever CloudWatch for an extended period → assert **flat memory**, disk backlog bounded by `maxDiskBytes`, `dropOldest` past the cap, clean resume, nonzero `dropped_stale` once the window is exceeded.

## 9. Node callback threading — VALIDATED (risk retired)

The one genuinely tricky build task — a native export thread blocking on an async JS sink result without
deadlocking the Node event loop — was **validated by a working napi-rs spike** before committing the
per-language work:

- **Setup:** napi-rs 3.9 / Node 24 / `x86_64-pc-windows-msvc`, a minimal cdylib (no heavy deps), built with
  `cargo build` (the `napi_build::setup()` build script handles Windows Node-symbol linking) and loaded as a
  renamed `.node`.
- **What ran:** a spawned native thread handed 8 batches to a JS callback via a `ThreadsafeFunction`
  (`NonBlocking`) and **blocked per-batch** on a `std::sync::mpsc` channel; the JS callback did genuinely
  async work (`await setTimeout(40ms)`) and then called a `resolveOutcome(id, code)` napi fn that signaled
  the channel, unblocking the worker. A `done` callback reported the summary so the calling JS thread never
  blocked.
- **Result (PASS):** all 8 outcomes received by the blocked worker; **event loop stayed alive** (a 5 ms
  `setInterval` ticked 24× during the run → no deadlock); `runExportLoop` returned synchronously; per-batch
  blocking serialized as expected (`elapsedMs` 383 ≈ 8 × 40 ms). Clean exit.
- **Conclusion:** the host-callback sink model works for Node as-is; the S1 queue-API fallback is **not
  needed**. Spike source: `scratchpad/node-sink-spike/` (`src/lib.rs` + `test.cjs`).

**Production mapping:** worker thread = the `ExportEngine` per-stream thread; the `ThreadsafeFunction` = the
host sink callback; `resolveOutcome` = how JS returns the `SendOutcome` (extended to carry failed-offsets →
`Partial`); the `std::sync::mpsc` block = the engine's synchronous `sink.send`. Everything else in this plan
is decided; no open risks remain for the v1 design.
