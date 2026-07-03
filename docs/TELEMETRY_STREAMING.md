# Telemetry Streaming — design

High-throughput, store-and-forward telemetry egress for ggcommons: a pluggable model
for **Kinesis Data Streams** and **Apache Kafka** (SiteWise a likely later sink), with a
**portable, embedded, persistent buffer we own** for disconnected industrial use. Works
on both the HOST and GREENGRASS platforms. New subsystem, peer to
`messaging`/`metrics`/`heartbeat`; opt-in; does not change existing APIs.

> Status: **the core has shipped.** `ggstreamlog` (`libs/rust-streamlog/`) is built and used by all
> four languages via `gg.streams()` — Rust natively, Java via Panama/the C-ABI, Python via PyO3, and
> Node via napi-rs — with the durable segment log, `KinesisSink`, and `KafkaSink` all implemented and
> tested (see `libs/rust-streamlog/README.md`). What's still genuinely deferred: the **SiteWise
> sink** (§6, tracked separately as roadmap item RM-008), **UNS identity enrichment** (§10.1),
> `ICredentialProvider` implementations beyond the AWS default chain (§7), stream priorities,
> RocksDB/LMDB `BlockStore` backends, and GREENGRASS local-pubsub fan-in.

## 1. Decisions (settled)

1. **Delivery: at-least-once + downstream dedup.** Records carry `(partitionKey, sequence)`
   so consumers can de-duplicate; we do not attempt cross-system exactly-once.
2. **No Greengrass Stream Manager.** One portable EmbeddedLog **we control**, used
   identically on both platforms. (Verified reasons beyond the operator's experience: SDKs
   are Java/Node/Python only — **no Rust**; exports to Kinesis/SiteWise/IoT Analytics/S3
   only — **no Kafka**; ≥70 MB RAM; requires a HARD component dependency.)
3. **No fixed throughput ceiling.** Peak records/s is a function of hardware + config.
   **Disk budget, segment size, and fsync policy are configurable** per stream.
4. **First sinks: Kinesis, Kafka.** (SiteWise deferred — its model is asset/property
   *topic mapping*, not a `partitionKey`; revisit as a separate sink later.)
5. **Backpressure default: `dropOldest`** (telemetry-typical; never silent — always metered).
6. **Secrets refresh is ours, not the GG Secret Manager component** (which only refreshes
   on deployment). See §7.
7. **One shared core in Rust, shipped as native artifacts** (no per-language
   reimplementation of the log). Bound into Java/Python/Node; native in Rust. See §5.
8. **`BlockStore` is pluggable, but v1 ships only the hand-rolled segment log.** RocksDB/LMDB
   backends can be added later behind the same interface.
9. **Sinks + the export engine live in the core too** (one implementation of
   batching/retry/checkpoint/Kafka/Kinesis/credentials), with a **host-language sink override
   hook**. Kafka uses **librdkafka** (via `rdkafka`) in the core; Java may override with the
   official `kafka-clients`. See §6.

## 2. Non-goals

- Not a replacement for `IMessagingService` (control plane: pub/sub + request/reply, IPC/MQTT).
  Streaming is the **data plane**: one-way, high-rate, durable, batched.
- Not a stream-processing/analytics engine (filter/aggregate is the caller's job before `append`).
- Not exactly-once. Not cross-device ordering (per-`partitionKey` order only).

## 3. Architecture

The buffer **and** the export engine + sinks live in **one shared Rust core**
(`ggstreamlog`); each language ships a thin binding. Nothing is delegated to a black box,
and there is one implementation of the hard parts.

```
 ┌─ host language (thin binding) ─┐   ┌──────────── ggstreamlog core (Rust, native) ───────────┐
 │  append(record) (non-blocking) │──►│  StreamService → EmbeddedLog → ExportEngine → Sink ─────┼──► Kinesis | Kafka
 │  flush / stats / lifecycle     │   │   (§4 durable FIFO)   (batch/retry)  ▲   (rdkafka/aws-sdk)│
 └────────────────────────────────┘   │                       checkpoint ───┘                    │
        ▲ optional sink-override (e.g. Java kafka-clients)  + CredentialProvider (§7) ────────────┘
```

- **`IStreamService`** (DI seam, like `IMessagingService`): `append`, `appendBatch`,
  `flush`, plus stream lifecycle + stats. Obtained via `gg.streams()`. A thin wrapper over
  the core (config marshaling + `append` + stats).
- **`EmbeddedLog`** (in core): the durable, ordered, at-least-once buffer (§4).
- **`ExportEngine` + `Sink`** (in core by default): the export loop pulls committed batches,
  sends via `KinesisSink`/`KafkaSink`, and on broker-ack advances the checkpoint. A
  **host-language sink override hook** lets a binding supply its own sink (e.g. Java with
  `kafka-clients`, or a custom destination) instead of the in-core one.

A `StreamRecord` = `{ payload: bytes, partitionKey: string, timestampMs: u64, headers?: map }`.
Built via `StreamRecordBuilder`. `partitionKey` drives ordering + downstream sharding
(e.g. `"{assetId}/{tag}"`).

## 4. The durable log core (`ggstreamlog`)

### 4.1 Build vs. reuse — engine evaluation

We need: append-mostly, strict FIFO, a single read cursor (checkpoint), age/size retention
with `dropOldest`, crash-safe recovery, cross-language. Candidates:

| Engine | What it is | Pros | Cons for *this* job |
|--------|-----------|------|---------------------|
| **Hand-rolled segmented append-log** | Our format (§4.2) | Exact fit; minimal; fastest sequential writes; full control; trivial bytes layout | We own crash-recovery correctness → mitigate with **one** impl + fuzz/crash tests |
| **RocksDB** (LSM+WAL, C++) | Embedded KV w/ WAL | Battle-tested at scale; write-optimized; bindings in all 4 langs (rocksdbjni, rust-rocksdb, rocksdict, level-rocksdb); crash-safe WAL | KV not a log (we'd key by offset); C++ native dep to package on every platform/lang; compaction/memtable overhead + tuning; overkill for FIFO |
| **LMDB** (mmap B+tree, C) | Embedded ACID KV | Extremely stable (OpenLDAP); tiny; fast ordered reads; bindings everywhere (lmdbjava/py-lmdb/node-lmdb/heed) | Single-writer; **pre-sized max map**; mmap/disk-full sharp edges; write-amp for large values; not log-structured |
| **SQLite (WAL)** (C) | Embedded SQL | The most-verified software in existence; ACID; bindings everywhere; simple ops | Row-store overhead; lower write ceiling; not a streaming log |
| **Chronicle Queue** (Java) | Persisted low-latency queue | Purpose-built, excellent perf | **JVM-only** → fails cross-language |
| Pure-Rust stores (**fjall** LSM, **redb**, **sled**) | Embedded KV in Rust | Pure Rust (clean FFI core, no C++) | Younger; less "verified at scale" pedigree |

**Recommendation: own the log *semantics*; default durability primitive is a hand-rolled
segmented append-log inside a single Rust core we fuzz/crash-test.** A FIFO store-and-forward
buffer is much simpler than a general KV — the proven engines are heavier than the problem,
and shipping RocksDB/LMDB native libs into four language packages is *more* packaging pain
than shipping one small Rust core. The `BlockStore` is a **pluggable interface**, but **v1
ships only the hand-rolled segment store**; RocksDB/LMDB backends can be added later behind
the same interface without changing the upper layers.

> Note: ggcommons **already ships native code** (`awscrt` under the Python/TS AWS SDKs, the
> `gg_sdk` C-FFI crate in Rust). A shared Rust core is consistent with that reality, not a
> new burden category.

### 4.2 On-disk format (the spec — identical across all bindings)

```
<stream-dir>/
  meta.json                 # format version, stream name, created-at
  checkpoint                # 16B: [u64 ackedOffset][u64 crc] (atomic rename on write)
  00000000000000000000.seg  # segment files, named by base offset (zero-padded)
  00000000000000065536.seg
  ...
```

Record framing inside a segment (little-endian):

```
[u32 len][u32 crc32c][u64 offset][u64 timestampMs][u16 pkLen][pk bytes][payload bytes]
```

- **Offset** = a monotonic per-stream u64 (record index). The checkpoint stores the last
  *acked* offset; recovery + the read cursor use it.
- **CRC32C** over `offset..payload` detects torn/corrupt tail records.
- **Segments** roll when they reach `segmentBytes` (config) or `segmentMaxAge`. A sparse
  in-file index (offset→byte position every N records) makes seek-to-checkpoint O(1)-ish.

### 4.3 Operations

- **append(record)** → assign next offset, frame, write to the active segment via a bounded
  in-memory queue + single writer. Returns immediately (the offset). **fsync policy** is the
  durability↔throughput dial (config `fsync`): `perBatch` (default — fsync at each
  group-commit), `interval(ms)`, or `always` (fsync every record; safest, slowest).
- **read cursor / export** → the sink reads forward from `checkpoint`, assembles a batch up
  to `batch.maxRecords`/`maxBytes`/`maxLatencyMs`, sends, and on ack calls
  **`commit(offset)`** which atomically advances the checkpoint. ⇒ **at-least-once** (a crash
  between send and commit re-sends the batch on restart).
- **retention** → background reclaimer deletes segments fully below the checkpoint; enforces
  `maxDiskBytes` + `maxAgeSecs`. On the cap with un-acked data, applies `onFull`:
  `dropOldest` (default — advance a *drop pointer* past the oldest segment, bump
  `recordsDropped` metric), `block` (back-pressure the producer; lossless), or `rejectNew`
  (append returns an error). **Drops are always counted, never silent.**
- **recovery** → on open: read `meta`, load `checkpoint`, scan from the checkpoint segment,
  validate CRCs, truncate a torn tail record, set the next offset.
- **backpressure** → the bounded in-memory ingest queue + `onFull` give end-to-end
  backpressure without unbounded memory.

### 4.4 Concurrency

Single writer per stream (append path); one background exporter task per stream (read +
send + commit); one background reclaimer. The checkpoint is single-writer (the exporter).
No shared mutable state between streams. This keeps the core lock-light and the ordering
trivially FIFO.

### 4.5 Performance & how config drives it

There is no hard ceiling; the disk + fsync policy are the limiter when disconnected, the
network/sink when connected. Operators tune:

- `segmentBytes` (larger = fewer rolls/syscalls, more per-file recovery scan).
- `fsync` (`perBatch`/`interval`/`always`) — the single biggest throughput lever.
- `maxDiskBytes` / `maxAgeSecs` — store-and-forward depth (sized to worst-case offline × rate).
- `batch.*` — export efficiency (align to sink limits, §6).

## 5. One core, four languages

### 5.1 A single Rust core exposed via a C ABI (the decision)

Write `ggstreamlog` **once in Rust** (the safe language for a hand-rolled durable log),
compile to a C-ABI `cdylib`, and bind it into the other three. One implementation =
benchmarked + fuzzed + crash-tested **once**; identical performance and on-disk format
everywhere; "we control it."

Bindings per language (all mainstream, distinct from the painful GG SDK FFI):

| Lang | Mechanism | Notes |
|------|-----------|-------|
| Rust | native crate | the core itself |
| Java | **Panama / Foreign Function & Memory API** | the Java lib already targets **21**, so no JNI needed |
| Python | **PyO3 / maturin** wheel (or cffi) | ships the native lib in the wheel, like `awscrt` |
| Node/TS | **napi-rs** with prebuilt binaries | same model `aws-crt` already uses |

Packaging cost is real (per-platform native artifacts: linux x64/arm64, win, mac) but
bounded, and the stack already carries native deps.

### 5.2 Rejected alternative: per-language reimplementation

Reimplementing §4.2's log in each language (pure code, no native artifact) was considered
and **rejected**: it multiplies the crash-recovery/fsync correctness surface by four and
invites on-disk-format drift, for the sole benefit of avoiding native packaging — which the
stack already does (`awscrt`, `gg_sdk`). The shared Rust core is the chosen path.

### 5.3 C ABI surface (sketch)

Because the **export engine + sinks live in the core**, the host does **not** drive export —
it only appends and reads stats. So the ABI is append/flush/stats/lifecycle (+ a credential
callback for Phase-3 Kafka), with config passed as a JSON string:

```c
int  ggsl_open(const char* config_json, ggsl_service** out, char** err);
int  ggsl_stream_get(ggsl_service*, const char* name, ggsl_stream** out, char** err);
int  ggsl_append(ggsl_stream*, const uint8_t* pk, uint16_t pk_len, uint64_t ts_ms,
                 const uint8_t* payload, uint32_t len, uint64_t* out_offset, char** err);
int  ggsl_flush(ggsl_stream*, char** err);
int  ggsl_stats(ggsl_stream*, ggsl_stats_t* out);
void ggsl_shutdown(ggsl_service*);
```

The full, normative ABI + rules (ownership, thread-safety, `catch_unwind` at the boundary)
are in the Phase-1 spec: **[TELEMETRY_STREAMING_PHASE1.md](./TELEMETRY_STREAMING_PHASE1.md) §11**.

### 5.4 Usage — identical shape in every lib

The binding is wrapped by each lib's idiomatic `IStreamService`; component authors never see
the C ABI.

**Rust**
```rust
let streams = gg.streams();
streams.stream("telemetry")
    .append(StreamRecord::new(&payload).partition_key(format!("{asset}/{tag}")));
```
**TypeScript**
```ts
await gg.streams().stream("telemetry")
  .append(StreamRecordBuilder.create(payload).partitionKey(`${asset}/${tag}`).build());
```
**Python**
```python
gg.streams().stream("telemetry").append(
    StreamRecordBuilder.create(payload).with_partition_key(f"{asset}/{tag}").build())
```
**Java**
```java
gg.streams().stream("telemetry")
  .append(StreamRecordBuilder.create(payload).withPartitionKey(asset + "/" + tag).build());
```

## 6. Sinks

Sinks live **in the core** by default (one implementation for Rust/Python/Node). The export
engine pulls committed batches and `commit()`s only on broker-ack (at-least-once);
partial-batch failures re-enqueue only the failed records; exponential backoff + jitter on
throttling; records carry `(partitionKey, sequence)` for downstream dedup. A **host-language
sink override hook** lets a binding supply its own implementation (e.g. Java with
`kafka-clients`, or a custom destination).

- **KinesisSink** (core, `aws-sdk` for Rust) — `PutRecords` (≤ **500 records / 5 MiB** per
  call → bounds `batch.*`); partition key from the record; per-record `FailedRecordCount`
  handling; creds via the AWS SDK provider chain (TES role in GG, standard chain on
  HOST). Mind shard hot-keys (partition-key cardinality).
- **KafkaSink** (core, **`rdkafka`/librdkafka**) — idempotent producer
  (`enable.idempotence=true`, `acks=all`) for no-dup within a session;
  `linger.ms`/`batch.size`/compression (lz4/zstd) aligned to `batch.*`; topic + partition
  from `partitionKey`; security `SASL_SSL`/mTLS with creds from the **CredentialProvider**
  (§7), **not** the GG Secret Manager component.

### Kafka client choice

| Lang | Client | Why |
|------|--------|-----|
| **Core (Rust/Python/Node)** | **librdkafka** via `rdkafka` | one high-perf C core shared by the non-JVM trio; full features (idempotence, SASL incl. OAUTHBEARER, compression). Pure clients (`kafkajs`, `kafka-python`) lag on throughput/features. |
| **Java (optional override)** | official `kafka-clients` | canonical, best-in-class on the JVM; pure Java. Use via the sink-override hook if a JVM shop requires it. |

> Deferred: **SiteWise** as a sink. Its model is asset/property *topic mapping*, not a
> `partitionKey`, so it needs its own mapping config and is out of scope for v1.

## 7. Credentials & secrets refresh (fixes the Secret Manager flaw)

The GG **Secret Manager component refreshes secrets only on deployment** — unusable for
rotating Kafka SASL/TLS creds. We provide a pluggable **`ICredentialProvider`** with **live
refresh**, running **in the core** (uniform across languages; no per-lib secrets code), with
a `CallbackCredentialProvider` escape hatch that FFIs back to the host for custom cases:

- **Interface**: `get(scope) -> Credential` (cached), TTL-based refresh-ahead, and
  **refresh-on-failure** (a sink auth error triggers re-fetch + reconnect). Optional
  `onRotation(cb)`.
- **Providers**:
  - `EnvCredentialProvider`
  - `FileCredentialProvider` — watches a creds file (reuses the FILE config hot-reload
    watcher) → rotate by writing the file, **no redeploy**.
  - `SecretsManagerCredentialProvider` — calls **AWS Secrets Manager directly via the AWS
    SDK**, authenticating with the device role (**TES** in GG) or the standard chain
    (HOST), with a configurable **TTL + refresh-ahead**. This **bypasses the GG Secret
    Manager component entirely**, so rotation is picked up on the TTL (e.g. 5–15 min) without
    a deployment.
  - `CallbackCredentialProvider` — caller-supplied.
- **Sink integration**: Kafka uses the client's credential callback where available (e.g.
  SASL/OAUTHBEARER token refresh) and otherwise **rebuilds the producer** on rotation (drain
  in-flight → swap), reconnecting with new creds. AWS sinks (Kinesis) rely on the
  SDK's auto-refreshing role-credential provider via TES.

## 8. Config schema + builder API (cross-language parity)

New `streaming` section in the embedded JSON schema (validated, **hot-reloadable** — batch
sizes, retention, priority, backpressure can change live via the existing reload path).

```yaml
streaming:
  streams:
    - name: "telemetry"
      sink: "kinesis"                       # kinesis | kafka
      buffer:
        path: "/var/lib/ggcommons/streams/{ComponentName}/telemetry"
        segmentBytes: 67108864              # 64 MiB
        maxDiskBytes: 2147483648            # store-and-forward depth (sized to your outage × rate)
        maxAgeSecs: 172800
        onFull: "dropOldest"               # dropOldest (default) | block | rejectNew
        fsync: "perBatch"                  # perBatch | interval | always
        fsyncIntervalMs: 1000
      batch: { maxRecords: 500, maxBytes: 4194304, maxLatencyMs: 1000, compression: "zstd" }
      delivery: { guarantee: "atLeastOnce", maxRetries: -1, priority: 5 }
      kinesis: { streamName: "...", region: "us-east-1", partitionKey: "{asset}/{tag}" }
      # kafka:    { bootstrapServers, topic, acks: all, idempotent: true,
      #             security: { protocol: SASL_SSL, mechanism: SCRAM-SHA-512,
      #                         credentials: { provider: secretsmanager, secretId: "...", ttlSecs: 600 } } }
```

`IStreamService` joins the DI registry next to `IMessagingService`/`IMetricService`; defined
in all four libs; legacy users unaffected (opt-in). `StreamRecordBuilder` mirrors `MessageBuilder`.

## 9. HOST vs GREENGRASS

Both platforms run the **same EmbeddedLog + sinks**. Differences are only environmental:

| Concern | HOST | GREENGRASS |
|---------|-----------|-----------|
| AWS creds (Kinesis) | standard SDK provider chain | device role via **TES** |
| Kafka secrets | File/Env/SecretsManager provider | SecretsManager-via-SDK (TES) or File provider — **not** GG Secret Manager |
| On-device fan-in | n/a | components publish to one streaming component over **local pub/sub IPC**; it owns the buffer + connections |
| Buffer location | configurable path | component work dir / a writable volume |

## 10. Delivery, ordering, observability

- **At-least-once**: commit-after-ack; `(partitionKey, sequence)` header for dedup; Kafka
  idempotence removes in-session dups.
- **Ordering**: FIFO in the log; per-`partitionKey` order preserved downstream (Kinesis
  per-shard, Kafka per-partition). Cross-key order is not guaranteed.
- **Observability** (via existing `metrics`/`heartbeat` targets, incl. CloudWatch): buffer
  depth, oldest-unacked age, bytes-on-disk, export throughput, retry/throttle counts, and
  **records-dropped** (so `dropOldest` is visible).

## 10.1 UNS identity enrichment — designed, NOT built (UNS Phase 4, mandate M15)

> **Status: roadmap.** The Unified Namespace ([`platform/DESIGN-uns.md`](platform/DESIGN-uns.md) §8,
> decision D11 / mandate M15) extends this subsystem so streamed records carry the same
> **self-identifying identity** the bus envelope now carries (`{hier, path, component, instance}`).
> The UNS *core* (envelope `identity`, `hierarchy.levels`, `gg.uns()`) has shipped in the libraries;
> **this streaming enrichment has not** — it is scheduled as Phase 4 of the UNS train.

When it lands:

- **Auto-enrichment**: the streaming facade stamps every record with the identity block + a header
  subset (event `timestamp`, message `name`/`version`) + `tags` — exactly as the bus facades stamp
  the envelope.
- **Columnar sinks (Parquet/AVRO)**: identity levels become **first-class columns** derived from
  `hierarchy.levels` (e.g. `site`, `factory`, `zone`, `device`) plus `component`/`instance` and a
  `tags` map column — hierarchy-queryable and **Hive-partitionable**
  (`…/site=dallas/device=gw-01/…`). Default partition-by = `site` + `device`, overridable via
  `stream.partitionBy`.
- **Broker sinks (Kinesis/Kafka)**: identity/header/tags ride in the payload; the partition key
  defaults to `device` for shard locality/ordering.
- **Provenance**: a processor's `stream:<name>` route keeps the **originating** identity (the device
  that produced the signal) plus a lightweight processing stamp.

## 11. Phasing

1. ✅ **MVP** (shipped): Rust `ggstreamlog` core (append/read/commit/retention/recovery, fsync
   policies, `dropOldest`) + fuzz/crash tests + bench; `KinesisSink`; HOST; metrics. Pure-Rust
   first proves the core before bindings.
   → **Implementation-ready spec: [TELEMETRY_STREAMING_PHASE1.md](./TELEMETRY_STREAMING_PHASE1.md).**
2. ✅ **Bindings** (shipped): napi-rs / PyO3 / Panama + `IStreamService` in TS/Python/Java.
3. ✅/◻ `KafkaSink` **shipped** (core, librdkafka), and the host-language sink-override hook
   **shipped** too (`SinkConfig::Callback` + `ggsl_set_sink_callback`, used today to drive the
   durable CloudWatch metrics buffer — see `docs/CLOUDWATCH_DURABLE_METRICS.md`) — though no one has
   used it yet for a Java `kafka-clients` override specifically. `ICredentialProvider` (File +
   SecretsManager-via-SDK refresh) remains **not implemented** — Kinesis/Kafka creds still come from
   the plain AWS SDK chain / literal librdkafka properties.
4. **Not yet implemented:** stream priorities; compression beyond an accepted-but-unapplied
   `zstd` batch-compression flag; GREENGRASS fan-in (local pub/sub) + TES creds.
5. **Not yet implemented:** additional `BlockStore` backends (RocksDB/LMDB); SiteWise sink with its
   own topic→asset/property mapping (RM-008).

## 12. Settled / open

**Settled (this review):**
- Native packaging of the shared Rust core — **accepted** (per-lib reimplementation rejected, §5.2).
- `BlockStore` pluggable interface, **hand-rolled segment store only in v1**.
- Sinks (Kinesis/Kafka) + export engine **in the core**; Kafka via **librdkafka**; Java may
  override with `kafka-clients` via the sink hook.
- **SiteWise deferred** (topic-style asset/property mapping, not a partition key).

**Open / risks:**
- Per-platform native artifact matrix (linux x64/arm64, win, mac) + CI to build/publish them
  into maven (Panama), the wheel (PyO3/maturin), and npm (napi-rs prebuilds).
- Core size: log + `rdkafka` (librdkafka) + AWS SDK in one cdylib — keep an eye on footprint
  for constrained edge devices.
- Sink-override hook ergonomics across Panama/PyO3/napi-rs (FFI callback for a host-supplied sink).
- `CredentialProvider` callback FFI for the `CallbackCredentialProvider` case (host-supplied creds).
