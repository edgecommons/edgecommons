# Telemetry Processor — design

> Status: **design proposal** (2026-06-30), **built** — the component ships at
> `edgecommons/telemetry-processor`. Code-grounded against the Rust messaging + streaming subsystems
> as of that date.
>
> **UNS adoption status (2026-07).** The library's UNS core has since shipped
> ([`platform/DESIGN-uns.md`](platform/DESIGN-uns.md)): the envelope gained a top-level **`identity`**
> element (`tags.thing` removed), and the southbound data plane's contract topic is now the UNS
> **`data`** class — `ecv1/{device}/{component}/{instance}/data/{signalPath}` (`docs/SOUTHBOUND.md`
> §2.0) — with the six-wildcard consumer set (`ecv1/+/+/+/data/#` for telemetry). **The processor and
> the adapters have NOT yet adopted it** — that is **Phase 5 (component adoption)** of the UNS train.
> The `southbound/{site}/…` topics throughout this doc are the **legacy scheme the shipping processor
> still subscribes**; its migration re-points the route `subscribe[]` filters at the UNS `data` class
> and keys routing on envelope `identity` instead of topic segments. Likewise, rows-mode identity
> columns move to the UNS identity/hierarchy model with streaming enrichment (M15, Phase 4 — see §7.2
> note).

Southbound **protocol adapters** (OPC UA, Modbus, …) publish every signal update to the **local bus
(channel 1)** as a `SouthboundSignalUpdate` envelope on config-driven topics
(`southbound/{site}/{ComponentName}/{InstanceId}/{signalId}` — the legacy scheme; see the UNS
adoption note above and `docs/SOUTHBOUND.md` §2). That
high-rate telemetry then needs a way **northbound**, but a raw firehose-to-cloud is wrong: integrators
want to **filter** (drop BAD quality, uninteresting signals), **sample/downsample** (1 kHz → 1 Hz), and
**aggregate** (per-signal windowed min/max/avg) *before* it leaves the edge — then route each result to
the right channel: low-rate control/alarm data to **northbound MQTT / IoT Core (channel 2)**, bulk
process telemetry to a **durable stream → Kinesis/Kafka (channel 3)**, or **local Parquet/Avro files**
(bounded by max size + max file count) for later bulk upload to a cloud data lake.
`docs/platform/DESIGN-channels.md` explicitly anticipates this seam (high-rate telemetry "belongs on
the streaming channel"; per-signal `publish.channel ∈ {local, northbound, stream:<name>}`) but nothing
implements the **processing/forwarding** stage that sits between the adapters' local publish and those
channels. The **Telemetry Processor** is that stage — the **first-class reference Rust component**,
living in its own `edgecommons/telemetry-processor` repo. The monorepo carries only this doc, a new
shared **file sink** in the streaming core, and the registry entry.

## 1. Decisions (settled)

1. **Processing model = declarative pipeline + Rhai.** A route's `pipeline` is an ordered list of
   stages drawn from built-in operators (`filter`, `sample`, `aggregate`, `project`) plus a `script`
   (Rhai) stage. `filter` accepts either a fast built-in predicate **or** a Rhai expression. All
   stages implement one internal `Processor` trait (§5) so they compose uniformly. The Rhai engine is
   **always compiled in** (no feature gate) — simpler, and the runtime cost is negligible when no
   route uses a script stage.
2. **File target = shared `ggstreamlog` sink.** A new `SinkConfig::File` variant in the streaming
   core, so a file destination is a normal stream sink alongside Kinesis/Kafka and inherits the
   durable buffer + `ExportEngine` batching/retry/at-least-once. It benefits all four languages (§10),
   and the component implements **no sink of its own** — it reaches the file sink purely through config
   (`target: "stream:archive"` where stream `archive` has a `file` sink).
3. **File contents = both modes.** Default = normalized typed telemetry **rows** (query-ready,
   partitioned). Fallback = **raw** envelope archival (one row per message, opaque payload). Encoder
   selectable Parquet (default) or Avro (for BigQuery / union-typed value fidelity). See §7.
4. **Home = own external repo,** `edgecommons/telemetry-processor`, a first-class component. The
   monorepo carries only: this design doc, the `ggstreamlog`/schema file-sink changes, and the
   registry entry.
5. **Each route is a `component.instances[]` entry** (`{ id, subscribe, pipeline, target, … }`) — the
   framework's existing array of independently-id'd worker units, enumerated via the built-in
   `Config::instance_ids()` / `instance(id)` accessors, mirroring how southbound adapters use
   `instances[]` (an instance is an independent worker unit; a route's `pipeline` is the per-route
   analog of an adapter instance's `subscriptions[]`). `component.global` holds cross-route **defaults**
   a route MAY override (the `global ⊕ instance` pattern). Both subtrees are permissive
   (`additionalProperties:true` per `docs/SOUTHBOUND.md` §4) → **no canonical-schema change** for
   routes. The **only** canonical schema edit is the `file` streaming-sink variant (§9).

## 2. Non-goals

- **Not a new subsystem.** The processor is a *component* built on the existing
  `messaging`/`metrics`/`streaming` subsystems, exactly as the adapters are (`docs/SOUTHBOUND.md` §1).
  The only library-level change is the shared file sink.
- **Not a general stream-processing engine.** Tumbling windows, simple reducers, sampling, projection,
  and a Rhai escape hatch — not joins across routes, not a SQL surface, not exactly-once. Sliding
  windows and percentile reducers are deferred (§12).
- **Not a replacement for the messaging control plane.** Request/reply and on-demand command surfaces
  stay with the adapters; the processor is a one-way telemetry transform-and-forward stage.
- **Not a new transport.** Routing targets reuse `publish` / `publish_to_iot_core` /
  `gg.streams().stream(n).append` verbatim — net-new code is only the dispatch glue (§6).
- **Not exactly-once.** Output to a `stream:` target inherits the streaming subsystem's at-least-once +
  downstream-dedup contract (`docs/TELEMETRY_STREAMING.md` §1); the file sink documents its own
  crash/duplicate semantics precisely (§7).

## 3. Architecture

```
 adapters ──publish(local)──▶  southbound/{site}/{Comp}/{Inst}/{signalId}   (channel 1, MQTT/IPC)
                                          │
                                          ▼
                          ┌──────────────────────────────────┐
                          │   Telemetry Processor (this comp) │
                          │                                   │
   route.subscribe[]  ───▶│  subscribe(filter, handler, 1, …) │  (one ordered consumer per route)
                          │        │  thin producer            │
                          │        ▼  bounded internal mpsc    │
                          │  ┌─────────────────────────────┐  │
                          │  │ route worker (single task): │  │
                          │  │  pipeline = [filter, sample,│  │
                          │  │   aggregate(window), project│  │
                          │  │   , script]  + flush timer  │  │
                          │  │  tokio::select!{ recv, tick }│  │
                          │  └─────────────────────────────┘  │
                          │        │ processed Message(s)     │
                          │        ▼                          │
                          │   target dispatch                 │
                          └───┬──────────┬───────────┬────────┘
                              │          │           │
                   target=local    target=northbound   target=stream:<name>
              publish(topic,msg)  publish_to_iot_core   gg.streams().stream(n).append(rec)
                                                              │
                                            ┌─────────────────┴─────────────────┐
                                       kinesis sink        kafka sink        FILE sink (new)
                                                                          parquet/avro, rows|raw,
                                                                          rolling maxBytes/maxFiles
```

**Library vs component split.** The processing engine and target routing are the **component's** own
code. The **file sink is a library/core** change (`ggstreamlog`), reachable from the component purely
via config. This keeps the throughput-critical durable-buffer/encoder logic in the one shared,
fuzz/crash-tested Rust core (consistent with `docs/TELEMETRY_STREAMING.md` §5), and keeps the component
small.

## 4. Route model & config

The processor enumerates routes via the existing `Config::instance_ids()` / `instance(id)` accessors;
**each `component.instances[]` entry is one route**, and `component.global` holds the cross-route
defaults each route overlays (`global ⊕ instance`). Because both subtrees are permissive, routes need
**no** canonical-schema change (`docs/SOUTHBOUND.md` §4).

Each route (instance) entry:

| Field | Meaning |
|-------|---------|
| `id` (required by `instances[]`) | route id — used for logs, the `processor_health` dimension, and hot-reload diffing |
| `subscribe` | `[string]` topic filters. MQTT `+`/`#` wildcards allowed; each filter is run through `ggcommons::config::template::resolve(&cfg, filter)` so `{ThingName}` / `{ComponentName}` / `{signal}` substitution works |
| `pipeline` | `[stage]` ordered stages (§5) |
| `target` | `"local"` \| `"northbound"` \| `"stream:<name>"` (§6) |
| `publish` | target topic template (for `local`/`northbound`) and `partitionKey` source (for `stream`; default `body.signal.id`) |
| `maxQueue` | subscribe / internal-mpsc queue depth |
| `key` | aggregation / dedup key path (default `body.signal.id`) |

Topic filters MUST support MQTT `+`/`#` wildcards and MUST be resolved through the existing
`ggcommons::config::template::resolve` so an operator can write
`southbound/{site}/{ComponentName}/+/{signal}` and have it expand against the active config. A route MAY
omit any field present in `component.global`; the resolved route is `global ⊕ instance` with the
instance winning per key.

```jsonc
{
  "component": {
    "global": {                                  // cross-route defaults (global ⊕ instance)
      "maxQueue": 10000,
      "key": "body.signal.id",
      "publish": { "topic": "telemetry/{site}/{ComponentName}/{signal}" }
    },
    "instances": [
      {
        "id": "good-1hz",                         // route 1: filter + downsample → local
        "subscribe": [ "southbound/{site}/+/+/+" ],
        "pipeline": [
          { "filter": { "field": "body.samples[].quality", "op": "eq", "value": "GOOD" } },
          { "sample": { "everyMs": 1000 } }
        ],
        "target": "local"
      },
      {
        "id": "windowed-avg",                     // route 2: tumbling aggregate → durable stream
        "subscribe": [ "southbound/{site}/+/+/+" ],
        "pipeline": [
          { "filter": { "field": "body.samples[].quality", "op": "eq", "value": "GOOD" } },
          { "aggregate": { "window": "10s", "by": "signal.id", "fn": [ "avg", "max", "count" ] } }
        ],
        "target": "stream:hot",
        "publish": { "partitionKey": "body.signal.id" }
      },
      {
        "id": "archive-raw",                      // route 3: project → file sink (via stream)
        "subscribe": [ "southbound/{site}/+/+/+" ],
        "pipeline": [
          { "project": { "keep": [ "signal.id", "signal.name", "samples" ] } }
        ],
        "target": "stream:archive"
      }
    ]
  }
}
```

## 5. Processing engine

### 5.1 The `Processor` trait

Every stage — built-ins **and** the Rhai stage — implements one internal seam so they compose
uniformly. `process` returns 0..N messages (`filter` → 0/1, `aggregate` flush → N, `project`/`script`
→ 1); `on_tick` lets a stateful stage (windowed aggregation) emit on a flush timer independent of
message arrival.

```rust
// internal seam — built-ins AND the Rhai stage implement this
trait Processor: Send {
    /// Transform one message. Returns 0..N output messages:
    /// filter → 0 or 1, sample → 0 or 1, project/script → 1, aggregate → 0 (accumulates).
    fn process(&mut self, ctx: &mut RouteCtx, msg: ProcMsg) -> SmallVec<[ProcMsg; 1]>;

    /// Timer-driven flush (tumbling-window emit). Default: no output.
    fn on_tick(&mut self, ctx: &mut RouteCtx, now_ms: u64) -> SmallVec<[ProcMsg; 1]> {
        smallvec![]
    }
}
```

`ProcMsg` is a parsed, read-only view over the message header/tags/body; `RouteCtx` carries the route
config, the resolved `key` accessor, and the health counters.

### 5.2 Stage types

- **`filter`** — keep/drop. Two forms:
  - **built-in (fast path):** `{ "filter": { "field": "body.samples[].quality", "op": "eq",
    "value": "GOOD" } }` plus shorthands (`quality`, numeric range, topic-glob, signal-set membership).
    Compiled to a closure **once at startup** — no per-message parsing.
  - **Rhai:** `{ "filter": { "script": "samples.all(|s| s.quality == \"GOOD\" && s.value < 100.0)" } }`
    — an arbitrary boolean predicate over a read-only message view.
- **`sample`** — downsample, per key. `{ "sample": { "everyMs": 1000 } }` (time) or
  `{ "sample": { "everyN": 100 } }` (count). Stateful per key → runs in the single route worker.
- **`aggregate`** — windowed reduction. `{ "aggregate": { "window": "10s", "by": "signal.id",
  "fn": [ "avg", "max", "min", "sum", "count", "first", "last" ] } }`. **Tumbling** windows (time or
  count) for MVP — sliding deferred (§12). Per-key state lives in a `HashMap<KeyHash, Accum>` with
  eviction on flush. On the worker's flush tick it emits **one message per (key, window)**, shaped as a
  `SouthboundSignalUpdate`-compatible envelope whose `samples[]` carry the aggregates plus a `window`
  block (`{ start, end, count }`) so downstream consumers parse it like any other southbound message.
- **`project`** — reshape/whitelist: `{ "project": { "keep": [ "signal.id", "samples" ],
  "set": { "tags.appId": "rollup" } } }`.
- **`script`** (Rhai) — full transform: input message view → returns a new body (or `()` to drop). For
  arbitrary enrichment/reshaping the built-ins don't cover.

### 5.3 Concurrency, ordering, backpressure (the key correctness design)

- The messaging `subscribe` callback is a **thin producer**: it forwards `(topic, Message)` into the
  route's **bounded internal `mpsc`** and returns immediately. Subscribe is opened with
  `max_concurrency = 1` for any route containing a **stateful** stage (`sample`/`aggregate`), so
  per-key order is preserved; a purely stateless route (`filter`→`project`→forward) MAY use
  `max_concurrency > 1`.
- Each route owns a **single async worker task** running
  `tokio::select! { msg = mpsc.recv() => …, _ = flush_interval.tick() => … }`. All stateful pipeline
  state lives **only in this task** → no locks, no races. Window flushing is **timer-driven**
  (independent of message arrival), which tumbling aggregation requires.
- **Loss semantics (documented):** the messaging subscribe queue is **drop-on-full** at the broker
  edge. For lossless aggregation, size `maxQueue` generously and prefer a **`stream:` target** (durable
  buffer) for the output. The internal-channel-full policy is configurable: `block` (backpressure to
  the subscribe queue → drops at the edge) or `drop` (with a `dropped_overflow` counter). Operators who
  need strict no-loss SHOULD route output to a `stream:` target rather than `local`/`northbound`.
- **Hot reload:** the component implements `ConfigurationChangeListener`; on change it diffs routes,
  re-resolves topic templates, and re-subscribes / replaces workers **without a process restart**
  (the skeleton's atomic-generation pattern generalizes to a route-set swap).

### 5.4 Health metrics

Each route emits `processor_health` (via `MetricBuilder` → `gg.metrics()`, destination config-driven),
dimensioned by route `id`, mirroring the southbound adapter health convention (`docs/SOUTHBOUND.md`
§5): `in_msgs`, `out_msgs`, `dropped_filtered`, `dropped_overflow`, `agg_windows_emitted`,
`pipeline_latency_ms`, `target_errors`.

## 6. Targets

Net-new code is only the dispatch glue; every target reuses an existing API.

| `target` | Implementation | Reused API |
|----------|----------------|------------|
| `local` | republish the processed message to a local topic (template from `publish.topic`) | `MessagingService::publish` |
| `northbound` | publish to IoT Core / northbound MQTT, QoS configurable | `publish_to_iot_core` |
| `stream:<name>` | append to a durable stream (→ kinesis / kafka / **file**) | `gg.streams().stream(n).append(Record)` |

For a `stream:` target the partition key defaults to **`body.signal.id`** — the southbound contract's
stable canonical id (`docs/SOUTHBOUND.md` §2) — exactly as `docs/platform/DESIGN-channels.md`
recommends; it is overridable via `publish.partitionKey`.

## 7. The file sink

A new `SinkConfig::File` variant lives **in the shared `ggstreamlog` core**, so a file destination is a
normal stream sink alongside Kinesis/Kafka and inherits the durable buffer + `ExportEngine`
batching/retry/at-least-once (`docs/TELEMETRY_STREAMING.md` §4, §6).

### 7.1 Config

```jsonc
{ "type": "file",
  "format": "parquet",          // parquet (default) | avro
  "mode": "rows",               // rows (default, normalized typed) | raw (envelope archival)
  "dir": "/data/{ThingName}/archive",
  "partitionBy": "dt={yyyy-MM-dd}/hr={HH}",  // Hive-style; UTC time tokens + config vars
  "maxFileBytes": 134217728,    // ~128 MiB — analytics-friendly target (the small-files lever)
  "maxFiles": 50,               // ring cap
  "rollEverySecs": 300,         // time-based roll
  "onFull": "dropOldest",       // dropOldest (default) | stop
  "compression": "snappy" }     // none | snappy (default) | zstd | gzip
```

`dir` / `partitionBy` go through the existing `resolve_sink` template substitution for config vars
(`{ThingName}` etc.); the UTC time tokens `{yyyy}` / `{MM}` / `{dd}` / `{HH}` (and the compound
`{yyyy-MM-dd}`) are resolved **per file at roll time** (Hive-style partition directories). The site
hierarchy (`site`, `shop`, `line`, `adapter`) rides as **typed columns** in every row, so Athena/
BigQuery still filter on them via column stats; **per-message-field partition *directories*** (one
open writer per distinct `site=…/adapter=…`) are a documented **deferral** (single open writer in
Phase 1) — see §12.

### 7.2 rows mode — normalized typed telemetry

Each `ExportRecord.payload` is decoded as a `SouthboundSignalUpdate` and **each `samples[]` element is
flattened into one row**. The polymorphic `value` (numbers / booleans / strings / ISO-8601, per
`docs/SOUTHBOUND.md` §2) is written as **sparse typed columns** —
`valueDouble | valueLong | valueBool | valueString` — with a `valueType` discriminator naming which
column is populated (the proven historian / EAV pattern; crawls cleanly in Glue/BigQuery/Synapse). In
**Avro** the value MAY instead be a true `union { double, long, boolean, string }` for better BigQuery
fidelity (§8). Columns:

```
thing, appId, site, shop, line,            -- from envelope tags{} (legacy pre-UNS envelope; see note)
adapter, instance,                         -- from body.device
signalId, signalName,                            -- from body.signal
valueDouble, valueLong, valueBool, valueString, valueType,   -- polymorphic sample value
quality, qualityRaw,                       -- normalized + native (SOUTHBOUND §3)
sourceTs, serverTs                         -- ISO-8601 UTC
```

> **UNS note (roadmap — not built).** Under the shipped UNS envelope, `thing`/`site`/`shop`/`line`
> no longer ride in `tags` (`tags.thing` is removed; location lives in the top-level `identity`
> element with configurable `hierarchy.levels`). The designed replacement is the **streaming
> identity-enrichment (M15, UNS Phase 4)**: identity levels become first-class Parquet/AVRO columns
> derived from `hierarchy.levels` (+ `component`/`instance` + a `tags` map column), with default
> Hive partitioning by `site`+`device` (`stream.partitionBy` override) — see
> [`platform/DESIGN-uns.md`](platform/DESIGN-uns.md) §8. Until Phase 4/5 land, the shipping file
> sink keeps the column set above (with envelope `tags` as a JSON column per the v2 redesign).

A **non-southbound** payload in rows mode MUST NOT be dropped — it is routed to a `_unmapped` **raw**
file (§7.3) so nothing is silently lost.

### 7.3 raw mode — envelope archival

One row per message: `topic, recvTs, name, version, payload` (the payload kept opaque as
`string` / `bytes`). Used for forensics / replay and as the `_unmapped` fallback for rows mode.

### 7.4 Rolling, retention, atomic finalize

A file is written to `*.inprogress` and **rolled** when it reaches `maxFileBytes` **or** `rollEverySecs`
elapses (a time-based roll is evaluated on the **next send**, not on a wall-clock interrupt). On roll
the sink **finalizes** (writes the Parquet footer / flushes the Avro block) and then performs an
**atomic rename** to the final partitioned path. `maxFiles` caps the ring: when full, `onFull` decides —
`dropOldest` (default: delete the oldest finalized file) or `stop`
(`SendOutcome::Failed { retryable: false }`).

### 7.5 Durability semantics (state precisely)

The file sink is part of the at-least-once streaming pipeline; the exact guarantees:

- **Offset commit.** The `ExportEngine` commits the durable-buffer offset **only on `AllAcked`** — i.e.
  after the sink has accepted the batch. A crash before commit re-delivers the batch on restart.
- **Clean shutdown (no loss).** On a clean stop the sink **finalizes the open file on `Drop`**, so a
  graceful shutdown loses nothing — the in-progress file is footer-written / flushed and renamed.
- **Hard crash.** **Avro recovers to its last sync block** (no loss up to that marker — a point in its
  favor as a landing format). **Parquet discards the unclosed, footer-less `*.inprogress` file** →
  loss is **bounded by the open-file window** (`rollEverySecs` / `maxFileBytes`).
- **Duplicates (at-least-once).** Records re-delivered after a crash that occurred **between sink-write
  and buffer-commit** MAY appear twice. Consumers MUST de-duplicate downstream on **`(signalId, sourceTs)`**.
- **Recommendation.** When strict no-loss matters, choose **Avro** as the landing format **or** a small
  `rollEverySecs` so the Parquet open-file window stays small.

## 8. Why typed columnar + cloud-lake fit

The rows-mode schema is shaped for the cloud lakehouses operators actually land this data in:

- **AWS S3 / Glue / Athena** want **Parquet + Hive partitioning + typed columns**: typed columns enable
  **column pruning**, and `partitionBy` directories enable **partition projection** so a Glue crawler
  infers the schema and Athena scans only the relevant `site=…/dt=…/hr=…` prefixes. The sparse typed
  value columns crawl cleanly (no JSON-string parsing at query time).
- **GCP BigQuery** prefers **Avro** as a load format, and **union types** preserve the polymorphic
  sample value faithfully — hence the Avro-union value option in §7.2.
- **Azure Synapse / ADX** consume the same Parquet + partition layout (external tables over the lake),
  so one sink config serves all three clouds.
- **The small-files problem.** Many tiny files murder lakehouse query planners; `maxFileBytes`
  (~128 MiB default) and `maxFiles` (ring cap) are the knobs that keep file sizes analytics-friendly
  and bound on-disk footprint at the edge.

## 9. Config schema

The **one** canonical schema edit is a `file` branch added to the `streamSink` `oneOf` in
`schema/ggcommons-config-schema.json`, followed by `schema/sync-schema.sh` to propagate to the four
library copies and pass `sync-schema.sh --check` (the CI drift gate). **Routes need no schema change** —
they live in the permissive `component.instances[]` / `component.global` (`docs/SOUTHBOUND.md` §4).

A full example — `component.instances[]` routes plus a `streaming` section whose `archive` stream has a
`file` sink:

```jsonc
{
  "component": {
    "name": "com.mbreissi.greengrass.TelemetryProcessor",
    "global": { "maxQueue": 10000, "key": "body.signal.id" },
    "instances": [
      {
        "id": "windowed-avg",
        "subscribe": [ "southbound/{site}/+/+/+" ],
        "pipeline": [
          { "filter": { "field": "body.samples[].quality", "op": "eq", "value": "GOOD" } },
          { "aggregate": { "window": "10s", "by": "signal.id", "fn": [ "avg", "max", "count" ] } }
        ],
        "target": "stream:hot"
      },
      {
        "id": "archive",
        "subscribe": [ "southbound/{site}/+/+/+" ],
        "pipeline": [ { "project": { "keep": [ "signal.id", "signal.name", "samples" ] } } ],
        "target": "stream:archive"
      }
    ]
  },
  "streaming": {
    "streams": [
      {
        "name": "hot",
        "sink": { "type": "kinesis", "streamName": "telemetry-hot", "region": "us-east-1" },
        "buffer": { "path": "/var/lib/ggcommons/streams/{ComponentName}/hot",
                    "maxDiskBytes": 2147483648, "onFull": "dropOldest", "fsync": "perBatch" },
        "batch":  { "maxRecords": 500, "maxBytes": 4194304, "maxLatencyMs": 1000 }
      },
      {
        "name": "archive",
        "sink": { "type": "file", "format": "parquet", "mode": "rows",
                  "dir": "/data/{ThingName}/archive", "partitionBy": "dt={yyyy-MM-dd}/hr={HH}",
                  "maxFileBytes": 134217728, "maxFiles": 50, "rollEverySecs": 300,
                  "onFull": "dropOldest", "compression": "snappy" },
        "buffer": { "path": "/var/lib/ggcommons/streams/{ComponentName}/archive",
                    "maxDiskBytes": 2147483648, "onFull": "dropOldest", "fsync": "perBatch" },
        "batch":  { "maxRecords": 5000, "maxBytes": 8388608, "maxLatencyMs": 5000 }
      }
    ]
  }
}
```

## 10. Cross-language parity

The file sink lives in the **`ggstreamlog` core**, not behind the per-language binding seam — so
Java / Python / Node gain it automatically **when their `cabi` native lib builds the `file` feature**.
That build-flag enablement plus a per-language smoke test is a **follow-on parity task** for the
four-way register, out of scope for this Rust component but tracked: each binding's prebuilt artifact
must be rebuilt with `ggstreamlog/file` (and `parquet`/`avro`) on, and a round-trip test added.

The **processor component itself** is **Rust-only** by design — it is the **reference Rust component**
(richer than the `examples/rust` skeleton) and lives in `edgecommons/telemetry-processor`. There is no
plan to mirror the component in the other three languages; parity applies to the **file sink** (shared
core), not the processor.

## 11. Phasing

- **Phase 0 — design doc + schema.** This doc; add the `file` branch to the canonical `streamSink`
  `oneOf` + `sync-schema`; land the registry `processor` category. (Monorepo PR.)
- **Phase 1 — file sink in `ggstreamlog`.** `SinkConfig::File`, `ParquetSink` (rows + raw, sparse typed
  columns), rolling / `maxFiles` / atomic-rename, factory arm, features/deps. Unit tests: Parquet
  round-trip, rolling at `maxFileBytes`, `maxFiles` eviction, crash-safety (drop writer mid-file → no
  offset commit). Then `AvroSink` (+ union value). 90% coverage gate. **Unblocks the component.**
- **Phase 2 — component skeleton + routing.** New repo from `examples/rust`; config structs; route
  worker (mpsc + flush timer); `local` / `northbound` / `stream:` dispatch; built-in `filter` +
  `sample`. HOST smoke.
- **Phase 3 — aggregate + project.** Tumbling windows + reducers; `project`; hot-reload listener.
- **Phase 4 — Rhai.** Rhai `filter` option + full `script` stage; message-view bindings (Rhai always
  compiled in). Benchmark built-in vs Rhai filter throughput.
- **Phase 5 — packaging + validation.** `recipe.yaml` / `gdk-config.json` / `Dockerfile` / `k8s`; lab
  GREENGRASS end-to-end with a live adapter; registry publish.

## 12. Settled / open

**Settled:**
- Processing model = declarative pipeline + Rhai (always compiled in); each route = one
  `component.instances[]` entry with `global ⊕ instance` defaults — **no route schema change**.
- File sink in the shared `ggstreamlog` core (both rows + raw modes; Parquet default, Avro option);
  the **only** canonical schema edit is the `file` `streamSink` variant.
- Durability: clean shutdown loses nothing; hard-crash loss bounded by the open-file window for
  Parquet, none for Avro to its last sync block; at-least-once with `(signalId, sourceTs)` dedup.

**Open / deferred:**
- **Per-message-field partition directories** (one open writer per distinct `site=…/adapter=…`) — Phase 1
  uses a single open writer and partitions by UTC time + config tokens only; the site/adapter
  dimensions ride as typed columns (still column-prunable). Multi-writer partitioning is additive.
- **Sliding windows + percentile reducers** (p95/p99) — after the tumbling MVP.
- **WASM processing stage** (an alternative to Rhai) — the `Processor` trait already accommodates it.
- **`--kind processor` CLI scaffold flag** (+ `templates/rust-processor/`) — only if a second processor
  appears (the southbound doc's "prove the pattern first" stance, `docs/SOUTHBOUND.md` §6).
- **Folding target routing into a future library `publish.channel` helper**
  (`docs/platform/DESIGN-channels.md` Phase B) once that lands.
- **Four-way file-sink parity** — build the `file` feature into the Java/Python/Node `cabi` libs + a
  smoke test per language (§10).
