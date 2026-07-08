# edgecommons Unified Namespace (UNS) — messaging namespace, identity & site-bus realization

> **Status (updated 2026-07-05): Phases 1–3 SHIPPED in all four languages, merged to `main`** and
> released as **v0.2.0** (release commit `b1d8d85`) — the topic grammar, the eight classes, the top-level `identity` element,
> `messaging()`/`gg.uns()`/`gg.instance()`, the reserved-class publish guard, `request()`'s internal
> deadline, MQTT LWT, and the library-owned `state`/`metric`/`cfg` publishers are real, tested code (see
> [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) for file:line citations and the mirror-parity
> record). **Phase 3 is also shipped**: the `_bcast` `republish-state`/`republish-cfg` listener (§9.4)
> is in all four languages, and the `uns-bridge` component (§9.1) is built and passing a live
> dual-broker end-to-end test in its own sibling repo — see
> [`DESIGN-uns-bridge.md`](DESIGN-uns-bridge.md) for exactly what's done there and what's still
> pending before general release (the edge-console site-side client and the GREENGRASS/IPC bridge
> variant; GitHub publish, the `registry/components.json` entry, and the git-rev pin to the v0.2.0 UNS
> release are now done). **The class-publish facades — `data()`/`events()`/`app()` — have also shipped in all
> four languages**, ahead of Phase 5: see [`DESIGN-class-facades.md`](DESIGN-class-facades.md) for the
> full design + decision register. They replace the raw `uns()`+`messaging()` hand-building §7.3 used
> to describe for the app-usable classes, and complete the class-facade family alongside the reserved
> `state`/`metric`/`cfg` publishers and the minimal `commands()` inbox — every one of the eight UNS
> classes now has exactly one library-owned owner (§7.3, §4). **Phases 4 (streaming enrichment, M15)
> and 5 (the southbound command family, M9, and D‑U15/16) remain design-only** — see §13 for the
> per-phase breakdown. This document specifies a
> single **Unified Namespace** for how edgecommons components address one another on the bus: the topic
> grammar, the message classes, the identity model, the messaging API surface (`messaging()` / `uns()` /
> the platform facades), streaming enrichment, and the physical realization of a **site-wide UNS bus**
> across per-device brokers. Java is canonical; the build lands in all four libraries (Java / Python /
> Rust / TS) with identical semantics. It was a deliberate, **pre-1.0 breaking change** — there was no
> production installed base to preserve.
>
> **Companion docs:** [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) (**the implementation
> companion** — concrete API shapes, per-language mirror notes, and the D‑U1…D‑U24 decisions register),
> [`DESIGN-class-facades.md`](DESIGN-class-facades.md) (the **shipped** `data()`/`events()`/`app()`
> class-publish facades — body-contract enforcement + defaults + channel routing for the three
> app-usable classes; §7.3 below is their summary, that doc is the source of truth),
> [`DESIGN-channels.md`](DESIGN-channels.md) (the local/northbound/stream channel
> model this concretizes), [`DESIGN-core.md`](DESIGN-core.md) (platform/transport resolution & identity
> chain), [`../HIERARCHICAL_CONFIG.md`](../HIERARCHICAL_CONFIG.md) (how the hierarchy + shared identity are
> distributed), [`../TELEMETRY_STREAMING.md`](../TELEMETRY_STREAMING.md) (the streaming service enriched
> here). The **`edge-console`** component (`edgecommons/edge-console`) is the first consumer and drove
> this design; its own design doc depends on this one.

---

## 1. Problem & scope

Today every edgecommons subsystem and every component invents its **own topic scheme**:

- heartbeat → `edgecommons/{ThingName}/{ComponentName}/heartbeat`
- metric → `{ThingName}/{ComponentName}/metric` (note: no `edgecommons/` root — inconsistent with heartbeat)
- southbound data → `southbound/{site}/{ComponentName}/{InstanceId}/{signalId}`
- southbound control → `southbound/{ComponentName}/{InstanceId}/control/{status|subscriptions|signals}`
- config push → `edgecommons/{ThingName}/config/{ComponentName}/updated`
- file-replicator (proposed) → `{ThingName}/file-replicator/{cmd|evt|state}/…` (yet another scheme)

Any consumer that wants a **fleet-wide** view (a monitoring console, a site historian, an MES bridge)
must therefore special-case each component. Identity is worse than inconsistent — for heartbeat/metric
it exists **only in the topic path** (the body carries no component name), so consumers parse topics,
which is fragile.

This design replaces all of that with one namespace, so a consumer subscribes a **small, uniform set of
wildcards and needs zero per-component knowledge**, and every message is **self-identifying**.

**Grounded constraints this design must live within** (verified against source, 2026-07):
- The `MessagingService` exposes **no MQTT retain flag**, and Greengrass IPC has none — "last known
  value on connect" cannot rely on retain.
- Greengrass **local IPC pub/sub is per-Nucleus (per-device)**; two cores cannot see each other's bus.
- request/reply has **no framework timeout**; an abandoned request leaks its ephemeral reply
  subscription (trips the Greengrass shared-connection quota).
- `+`/`#` wildcards work on both transports.

### Non-goals
- Not a new transport (still MQTT / Greengrass IPC per [`DESIGN-core.md`](DESIGN-core.md)).
- Not a service registry (business components discover peers via `describe` + `broadcast`; platform
  service-discovery is left to Greengrass / Kubernetes).
- Not the hierarchical-config engine — the hierarchy/identity **distribution** is [`HIERARCHICAL_CONFIG.md`](../HIERARCHICAL_CONFIG.md); this doc defines what the values *mean* and how they are stamped.

---

## 2. Decisions register (review these)

| # | Question | Options | Recommended |
|---|----------|---------|-------------|
| D1 | Topic depth vs hierarchy | encode hierarchy in topic · **device-only, hierarchy in the body** | ✅ device-only: `ecv1/{device}/{component}/{instance}/{class}` — constant depth regardless of hierarchy depth |
| D2 | Multi-site root in topic | always · **off by default, `topic.includeRoot` opt-in** | ✅ off (edge broker = one site); opt-in for a broker aggregating sites |
| D3 | Hierarchy shape | fixed `site→area→device` · **enterprise-configurable ordered levels** | ✅ configurable `hierarchy.levels` (arbitrary depth, freely named); the **last level is the node** |
| D4 | Identity placement | in `tags` · **top-level `identity` element** | ✅ top-level element; `tags` kept for business context (app/org/cost); `tags.thing` removed |
| D5 | Delimiter | dot (NATS/AMQP) · **`/` (MQTT-native)** | ✅ `/` throughout; `.` only literal-within-a-level (as the component name already is) |
| D6 | Heartbeat & announce as classes | separate classes · **fold into `state`** | ✅ fold: the `state` keepalive *is* the liveness beacon; announce = the first `state` |
| D7 | Developer bus access | facades only · **keep `messaging()` (arbitrary topics)** | ✅ keep it; add an explicit `uns()` builder; guard only reserved platform-class topics |
| D8 | Multi-device site aggregation | per-device console · console federates · **`uns-bridge` → site broker** | ✅ `uns-bridge` per device → one site UNS broker (envelope-aware); federation is a documented fallback |
| D9 | MQTT LWT / retain in the lib | neither · **LWT now, retain deferred** · add both now | ✅ **LWT only** (revised 2026-07-02): the one always-MQTT niche with no keepalive substitute is the `uns-bridge` UNREACHABLE signal (§9.3). Retain is MQTT-only + redundant with broadcast `republish-state` and the consumer's timestamped cache, and can't express staleness — so deferred (if ever needed, owned by the bridge). IPC no-ops. |
| D10 | `request()` timeout | in `get()` only · **internal deadline armed in `request()`** | ✅ internal deadline (cleanup must run even if `get()` is never called) + optional `request(…, timeout)` overload |
| D11 | Streaming hierarchy layout | blob column · **identity levels as columns + partitioning** | ✅ **confirmed** — hierarchy → first-class Parquet/AVRO columns; default partition-by `site`+`device` (`stream.partitionBy` override); `tags` as a map column; header subset `timestamp`+`name`+`version`; broker-sink partition key `device` |
| D12 | Reserved classes | none · **`state`/`metric`/`cfg`/`log` library-owned** | ✅ reserved (raw publish to them rejected) so no component can forge another's health/config |

---

## 3. The topic grammar

```text
ecv1 / {device} / {component} / {instance} / {class} [ / {channel...} ]
```

- Root `ecv1` is a **fused** literal (EdgeCommons namespace + grammar major version). Fusing it, and
  keeping the identity path to three tokens, holds the core classes within **AWS IoT Core's 7-slash
  topic limit**, so `state`, `cfg`, `metric/{name}`, `cmd/{verb}` transit to the cloud verbatim. Grammar
  bumps are `ecv1 → ecv2` (Sparkplug's `spBv1.0` precedent); *payload* versions stay in `header.version`.
- **The topic addresses the endpoint, not the hierarchy.** `{device}` is the physical node (reachability
  + bridge + command-ACL anchor — it replaces today's `thing`); `{component}`/`{instance}` are the
  edgecommons addressing suffix. The enterprise's logical hierarchy lives in the message `identity` (§5),
  **not** the topic — so topic depth is **constant regardless of hierarchy depth** (D1). This buys back
  depth budget for the data-plane signal path.
- `{class}` is one of the closed set in §4; consumers subscribe per class.
- **Optional root** (D2): a multi-site broker may set `topic.includeRoot: true` for `ecv1/{site}/{device}/…`.

Broker-side scoping is therefore **by device** (`ecv1/{device}/#`) — the unit that matters for bridge
ACLs, command routing, and reachability. Finer grouping (area/line/zone) is done by the consumer from
`identity`.

---

## 4. Message classes

Two of today's concepts are intentionally **not** classes (D6): **heartbeat** folds into `state` (the
keepalive *is* the liveness beacon; system stats move to `metric`), and **announce/discovery** folds into
`state` plus pull-on-demand `cmd` verbs.

| Class | What it carries | Direction | Publisher | Example tail |
|---|---|---|---|---|
| `state` | Lifecycle + health + keepalive — **the liveness beacon** | pub | library (`status()`) — **reserved** | `state` |
| `metric` | Operational metrics incl. built-in `sys` (cpu/mem/…) | pub | library (`metrics()`) — **reserved** | `metric/signals-ingested` |
| `cfg` | Effective-config snapshot (redacted), on change / on request | pub | library — **reserved** | `cfg` |
| `log` | Structured log records for remote tail (off by default) | pub | library — **reserved** | `log/error` |
| `data` | Telemetry data plane — `SouthboundSignalUpdate`, processed outputs | pub | `data()` — **shipped facade** (raw `messaging()`+`uns()` still allowed) | `data/press12/temperature` |
| `evt` | Events and alarms (raise/clear) | pub | `events()` — **shipped facade** (raw `messaging()`+`uns()` still allowed) | `evt/critical/overtemp` |
| `cmd` | Commands — request/reply, built-in + component verbs | **req/reply (inbound)** | any (§7) | `cmd/reload-config` |
| `app` | Arbitrary application pub/sub between components | pub/sub | `app()` — **shipped facade** (raw `messaging()` still allowed) | `app/order/received` |

The first four are **reserved platform classes** (D12) — library-owned, so a component cannot hand-forge
another's health or config. `cmd` is the only class whose identity path names the **recipient**;
everything else names the **publisher**.

**A consumer's complete subscription set** is six uniform wildcards, no per-component branches:

```text
ecv1/+/+/+/state        ecv1/+/+/+/cfg        ecv1/+/+/+/evt/#
ecv1/+/+/+/metric/#     ecv1/+/+/+/data/#     ecv1/+/+/+/log/#
```

---

## 5. Identity

### 5.1 Configurable hierarchy (D3)

A deployment declares its levels once — an ordered, freely-named list whose **deepest level is the
physical node** (nothing sits below it but software), so "last level = node" is an invariant, not a flag:

```jsonc
"hierarchy": { "levels": ["site", "factory", "zone", "device"] }   // last level = the node (the thing)
```

`["site","area","device"]`, `["region","plant","line","cell"]` — all valid. `component`/`instance` are
**not** part of this; they are the edgecommons addressing suffix.

### 5.2 Top-level `identity` element (D4)

Every envelope carries identity as a **top-level element** (sibling of `header`/`tags`/`body`, not buried
in `tags`), self-describing and order-safe:

```json
{
  "header":   { "name": "state", "version": "1.0", "uuid": "…", "correlation_id": null },
  "identity": {
    "hier": [
      { "level": "site",    "value": "dallas" },
      { "level": "factory", "value": "finishing" },
      { "level": "zone",    "value": "zone-3" },
      { "level": "device",  "value": "gw-01" }
    ],
    "path":      "dallas/finishing/zone-3/gw-01",
    "component": "opcua-adapter",
    "instance":  "kep1"
  },
  "tags": { "app": "line-ctl", "org": "ops-west" },
  "body": { "…class-specific…" }
}
```

- `hier` is the single source of truth; its **last entry is the device**. `path` is the precomputed join
  (consumers group/display without re-joining or needing the schema) — its trailing device value is
  inherent to a join, not a separate field. There is **no standalone `device` field on the wire**;
  consumers read the last `hier` entry, and the in-memory identity object exposes a computed `device`
  accessor. (This removes the earlier 3× duplication of the device value.)
- **`tags.thing` is removed** (redundant with the device level). `tags` survives for business context.

### 5.3 `instance` is per-message, not a config field

A component commonly serves **many** instances — its existing `component.instances[]` (e.g. an OPC UA
adapter with `kep1`, `plc-2`). The `{instance}` segment is resolved **per message**, from the instance the
message pertains to — via an instance-scoped facade handle (`gg.instance("kep1").data().publish(…)`)
or the originating-instance context — never a single `identity.instance` value. Component-level messages
(the overall `state`) use the default token `main`.

### 5.4 Distribution via Hierarchical Config

Identity is **not repeated per component**. The `hierarchy` schema and the shared location values
(`site`/`factory`/`zone` — everything above the device) live in inherited hierarchy layers and reach
every component via the lineage merge of [`HIERARCHICAL_CONFIG.md`](../HIERARCHICAL_CONFIG.md):

```jsonc
// inherited hierarchy layer               // a component's own config: nothing identity-related
"hierarchy": { "levels": [ … ] },          //   device    = resolved thing name
"identity":  { "site":"dallas", … }        //   component = its own name; instance = per message
```

The UNS is the **concrete driver** for the multi-level hierarchy: location levels live in this
`identity`/`hierarchy` block and can be inherited from enterprise, site, zone, line, or other
catalog scopes. Component layers supply only component-specific behavior.

Location identity resolves **once** at startup (fail-fast if it doesn't cover `hierarchy.levels`); the
library stamps `identity` (with `path`) on every message; `instance` is stamped per message.

---

## 6. Every legacy scheme collapses onto this

| Legacy (today) | UNS (new) |
|---|---|
| `edgecommons/{Thing}/{Comp}/heartbeat` | `…/{comp}/{inst}/state` (keepalive) + `…/metric/sys` |
| `{Thing}/{Comp}/metric` (no root) | `…/{comp}/{inst}/metric/{name}` |
| `southbound/{site}/{Comp}/{Inst}/{signalId}` | `…/{comp}/{inst}/data/{signalPath}` |
| `southbound/{Comp}/{Inst}/control/status` | `…/{comp}/{inst}/cmd/sb/status` |
| `edgecommons/{Thing}/config/{Comp}/updated` | `…/{comp}/{inst}/cmd/set-config` + outbound `…/cfg` |
| file-replicator `{Thing}/file-replicator/cmd\|evt\|state/…` | the standard `cmd` / `evt` / `state` classes under its identity path |

file-replicator's `cmd | evt | state` intuition survives — it just stops inventing its own address.

---

## 7. The messaging model — `messaging()`, `uns()`, and the platform facades

### 7.1 `gg.messaging()` — the raw bus (northbound surface + one hardening)

Takes a **literal topic**; no hidden construction. The full existing surface remains, **including the
northbound variants**:

| Method | Notes |
|---|---|
| `publish(topic, msg)` | exactly what hits the wire — UNS-compliant **or** an external, non-UNS topic (for components that bridge a legacy MQTT system) |
| `subscribe(topicFilter, handler)` · `reply(request, response)` | **unchanged** from today |
| `request(topic, msg[, timeout])` | returns today's future/IoU; **hardened** per §7.4 |
| `publishNorthbound` / `subscribeNorthbound` / `requestNorthbound` / `replyNorthbound` | **retained** — the local↔northbound split is orthogonal to the UNS; a `uns()`-built topic is IoT-Core-depth-safe by construction. |

### 7.2 `gg.uns()` — the explicit topic builder + validator

For a UNS-native topic, this converts a class + channel into a validated topic from *this* component's
identity — and it is where the **depth guard** lives, visibly, at build time:

```java
String t = gg.uns().topic("app", "order/received");   // → ecv1/gw-01/opcua-adapter/kep1/app/order/received
gg.messaging().publish(t, msg);
gg.uns().topicFor(targetIdentity, "cmd", "reload-config");   // address another component
gg.uns().filter("state", Scope.site());                      // build a wildcard subscription filter
gg.uns().validate(topic);                                    // char-set + ≤7-slash-to-IoT-Core check
```

There is **no hidden leaf-injection**: the developer sees and controls the topic; an over-deep channel
fails at build time, not as a dropped message at IoT Core.

### 7.3 Platform facades (reserved, mostly automatic)

Build their topics internally via `uns()`. Most components call *none* of these and are still fully
observable:

| Facade | Purpose | Typical developer use |
|---|---|---|
| *(automatic)* | heartbeat/`state`, identity, `describe`, `cfg`, `log`-tail | **nothing** — visible for free |
| `gg.status()` | health nuance | `update(DEGRADED, reason)`, `registerCheck(name, supplier)` |
| `gg.metrics()` | metrics (kept `MetricEmitter`; topic now UNS-minted) | `emit(measure)` |
| `gg.data()` (Java: `gg.getData()`) | signal/`data` plane — **shipped, non-reserved** | `signal(id).addSample(value).publish()` / `publish(path, value)` |
| `gg.events()` (Java: `gg.getEvents()`) | operator alarms/events (`evt`) — **shipped, non-reserved** | `emit(sev, type, msg, ctx)`, `raiseAlarm/clearAlarm` |
| `gg.app()` (Java: `gg.getApp()`) | free-form inter-component pub/sub (`app`) — **shipped, non-reserved** | `publish(name, channel, body)` |
| `gg.commands()` | **console-exposed** request/reply | `register(verb, schema, handler, {danger})` |
| `gg.discovery()` | capability manifest + panels | `describe(manifest)`, `registerPanel(id, asset)` |
| `gg.streams()` | durable telemetry egress (§8) | `publish(stream, record)` — auto-enriched |

`gg.commands()` is the **console-facing specialization** of request/reply: a handler registered here is
*also* advertised in `describe` with a JSON-Schema, danger tier, and RBAC. Peer-to-peer business RPC the
operator never sees just uses `messaging().request()` / `reply()`. Built-in verbs the library
auto-registers (lowercase-hyphenated; a `/`-namespace level for families):

`ping` · `describe` · `get-panel-asset` · `republish-state` / `republish-cfg` (late-join lever) ·
`get-configuration` / `reload-config` (+ `set-config` on CONFIG_COMPONENT) · `set-log-level`. Families add
a namespace level: the southbound-adapter kit registers `sb/status`, `sb/browse`, `sb/read`, `sb/write`,
`sb/subscribe-preview`; file-replicator registers `fr/…`.

> **Shipped so far (slice S2, §9.5):** the **command inbox** + the minimal registration seam
> (`register(verb, handler)` / `unregister`) + the built-ins `ping`, `reload-config` and Flow-B
> `get-configuration` (`republish-state`/`republish-cfg` ride the §9.4 `_bcast` listener;
> `set-config` rides the CONFIG_COMPONENT source). The schema/danger/RBAC-annotated registration,
> `describe`/`get-panel-asset`, `set-log-level` and the `sb/*` family remain deferred (Phase 5).

> **Shipped (the class-facades slice, [`DESIGN-class-facades.md`](DESIGN-class-facades.md)):** the
> three **app-usable** class-publish facades — `data()`/`events()`/`app()` — are real, tested code in
> all four languages (Java canonical `2283189`, Py/Rust/TS mirrors `8d3e3c9`), pinned by
> `uns-test-vectors/{data,evt,app}.json`. They replace the raw hand-building this section used to
> illustrate: `data()` **constructs and validates** the `SouthboundSignalUpdate` body (`quality`
> defaults to `GOOD` with a `qualityRaw:"unspecified"` marker when the source omits it, `serverTs`
> defaults to now, the `signal.id` is the only hard reject); `events()` **derives** the
> `evt/{severity}/{type}` channel from the body's own severity + type (so the topic and body can never
> disagree — the exact `evt` drift §1.2 of that doc documents) and adds `raiseAlarm`/`clearAlarm`;
> `app()` is thin publish-sugar (named header + verbatim body). All three resolve channel routing
> per-call override ▸ config `publish.channel` (instance ▸ global) ▸ `LOCAL` (D1), with `data()`
> alone honoring a `stream:<name>` route (composing `getStreams()`, falling back to LOCAL when no sink
> is wired). They publish through the **ordinary, guarded** `messaging().publish(...)` — non-reserved,
> so raw `uns()`+`messaging()` publishing to `data`/`evt`/`app` remains allowed (D9: facades are
> enforcement-by-default, not prohibition). With these three, **every one of the eight UNS classes now
> has exactly one library-owned owner** (§4's reserved publishers, the `cmd` inbox above, and these) —
> see that doc's §6 for the complete family table.

### 7.4 Request/reply hardening (D10)

`request()` keeps returning today's future/IoU. It arms an **internal default deadline** (framework-owned
timer, default = a configurable max) — cleanup must run even if the caller never calls `get()` (`join()`,
`thenApply()`, or a dropped reference all bypass it). When the deadline fires it completes the future
exceptionally **and** unsubscribes the reply topic. Consequences: no-arg `get()` no longer blocks forever;
`get(timeout)` waits **less**, never more, than the deadline; a custom deadline is the optional
`request(topic, msg, timeout)` overload. `reply()` and `subscribe()` are untouched.

### 7.5 Enforcement (narrow, checkable, broker-backed)

1. **Reserved platform classes are library-owned:** `messaging().publish` rejects a raw publish to
   `ecv1/…/{state|metric|cfg|log}`. Everything else — arbitrary external topics, your own
   `data`/`evt`/`cmd`/`app` UNS topics — is open.
2. **The `uns()` builder validates** char-set + slash budget.
3. **The broker is the durable enforcement:** per-device ACLs scope each component (and its `uns-bridge`)
   to publish only under its own `ecv1/{device}/#` subtree.
4. **The config schema drops the drift knobs** (`metricEmission.targetConfig.topic`, heartbeat
   messaging-target topic override); top-level `additionalProperties:false` fails stale configs.

```mermaid
sequenceDiagram
  participant App as component code
  participant U as gg uns builder
  participant M as gg messaging
  participant B as Broker or IPC
  participant C as consumer
  App->>U: build a data topic for press12/temperature
  U-->>App: ecv1/gw-01/opcua-adapter/kep1/data/press12/temperature
  App->>M: publish that topic with envelope and identity
  Note over M: a raw publish to a reserved-class topic is rejected
  M->>B: PUBLISH
  B->>C: matches the data-class wildcard
  C->>C: identity read from the top-level block, never the topic
```

---

## 8. The UNS beyond the bus — durable streaming (D11)

The `data` class is the *live* path (pub/sub, rate-sampled). High-rate telemetry that must be durably
captured goes through the streaming service ([`TELEMETRY_STREAMING.md`](../TELEMETRY_STREAMING.md)) —
`gg.streams()` and telemetry-processor's `stream:<name>` target → Kinesis / Kafka / rolling **Parquet·AVRO**
files. A separate, batched, durable channel — carrying the **same UNS identity**, automatically.

- **Auto-enrichment:** the streaming facade stamps every record with the identity block + a header subset
  (event `timestamp`, message `name`/`version`) + `tags` — exactly as the bus facades stamp the envelope.
- **Columnar sinks → hierarchy becomes columns + partitions:** the Parquet/AVRO sink derives its identity
  **columns from `hierarchy.levels`** (a `site`, `factory`, `zone`, `device` column …) + `component`/
  `instance` + a `tags` map column (the existing envelope-tags-as-column precedent). Data is then
  self-describing and **hierarchy-queryable** (`WHERE site='dallas' AND device='gw-01'`) and
  **Hive-partitionable** (`…/site=dallas/factory=finishing/device=gw-01/…`) — the UNS tree in the
  lakehouse, with partition pruning for free. Default partition-by = `site` + `device`, configurable via
  `stream.partitionBy`.
- **Broker sinks (Kinesis/Kafka):** identity/header/tags in the payload; partition key defaults to
  `device` for shard locality/ordering.
- **Provenance:** a `stream:<name>` route keeps the **originating** identity (the device that produced the
  signal — what analytics keys on) plus a lightweight processing stamp.

---

## 9. Physical realization — the site UNS bus

The UNS above is a *logical* namespace. Making it a real **site-wide** bus across many devices is the
physical-topology problem, because each device has its **own** bus (a local broker on HOST; a device-local
Nucleus IPC bus on GREENGRASS — confirmed device-local, no cross-core visibility). There is no horizontal
bridging today.

### 9.1 The `uns-bridge` component (D8)

**One `uns-bridge` per device bus** subscribes the device's local traffic, republishes it onto a
**site-level UNS broker** under the device's namespace, and relays commands back down. It is
**envelope-aware** — unlike a dumb broker bridge it rewrites `reply_to` for commands (TTL'd correlation
map), stamps UNS identity, rate-caps the data plane, protects against loops (hop tag), and emits a
Last-Will "offline" for fast reachability detection. Any site-scoped consumer (a historian, an MES bridge,
the console) then connects to **one** bus.

```mermaid
flowchart TB
  Cloud["AWS IoT Core / streams"]
  subgraph SITE["Site: dallas (gateway box)"]
    SB[("Site UNS broker — EMQX")]
    Consumers["site consumers<br/>(edge-console, historian, MES bridge)"]
  end
  subgraph L1["Line: packaging-3"]
    subgraph GW1["Device gw-01 (Greengrass)"]
      BR1["uns-bridge"] --- IPC1(["local IPC bus"])
      O1["opcua-adapter"] --- IPC1
    end
  end
  subgraph L2["Line: packaging-4"]
    subgraph GW2["Device gw-02 (HOST)"]
      BR2["uns-bridge"] --- LB2[("local MQTT")]
      O2["modbus-adapter"] --- LB2
    end
  end
  Consumers <-->|"6 UNS subs + commands"| SB
  BR1 <-->|"relay under ecv1/gw-01/..."| SB
  BR2 <-->|"relay under ecv1/gw-02/..."| SB
  SB -.->|"optional northbound relay"| Cloud
```

Alternatives (documented, not primary): broker-native bridging (device→site, blind relay, MQTT only);
consumer-federated multi-connection (can't reach IPC; pushes N clocks into the consumer). The AWS
Greengrass **MQTT bridge component** (`aws.greengrass.clientdevices.mqtt.Bridge`, relays IPC ↔ local MQTT ↔
IoT Core) is the "no custom code on the device" path, but is a blind relay needing 2–3 chained components;
`uns-bridge` does it in one envelope-aware component.

### 9.2 Deployment defaults per platform

| Scenario | Site broker | Bridging |
|---|---|---|
| **HOST, multi-device site** | EMQX on the gateway box | `uns-bridge` on each device (local MQTT ↔ site MQTT) |
| **Greengrass, multi-device site** | EMQX on the gateway (GG-managed container) | `uns-bridge` per Nucleus (IPC ↔ site MQTT) |
| **Single device** | none | none — a consumer connects to the local bus directly |
| **Kubernetes** | the in-cluster broker *is* the aggregation point | none inside a cluster; one `uns-bridge` pod if the cluster is one line of a bigger site |

On **Kubernetes** the cluster already runs one shared broker, so aggregation is native (no bridge inside a
cluster); the hierarchy becomes logical labels via the Downward API. A bridge reappears only at cluster
boundaries.

### 9.3 Reachability & late-join (D9)

Because retain and consumer-side miss-detection don't exist, correctness rests on three layers, in
priority: (1) the periodic **keepalive** `state` (both transports); (2) a **broadcast re-announce**
(`ecv1/{device}/_bcast/main/cmd/republish-state` — the reserved `_bcast` pseudo-component, see
UNS-CANONICAL-DESIGN §4.3) for an instant snapshot on a consumer's (re)connect, with jittered
replies; (3) the consumer's own **last-known-value cache** — each entry **timestamped**, so a late joiner
gets the value *and* its age (the application-layer "retained value" pattern; the console realizes this in
its FleetModel). MQTT **LWT** is added to the library (§7 / D9) as an *accelerator* — IPC no-ops it, so
behavior is identical minus latency. Its load-bearing use is the `uns-bridge` LWT on the **site broker** (an
always-MQTT hop), making a whole device detectable as **UNREACHABLE** (distinct from a component being
OFFLINE) far faster than keepalive-miss. **Broker retain is deferred**: MQTT-only (so it can never be what
IPC relies on), redundant with (2)+(3), and — unlike the timestamped cache — unable to express staleness
(a retained message sits stale until explicitly evicted). If ever wanted, its owner is the bridge, not each
component's provider.

```mermaid
sequenceDiagram
  participant Con as consumer
  participant SB as site broker
  participant C1 as component
  Con->>SB: subscribe the six UNS patterns
  Con->>SB: publish ecv1/{device}/_bcast/main/cmd/republish-state
  SB->>C1: deliver broadcast
  Note over C1: wait a random 0 to 2s to avoid a stampede
  C1-->>Con: state
  Note over Con: cache hydrated; consumers snapshot from it
```

### 9.4 The `_bcast` republish listener — normative behavior (slice G-S1; shipped in all four languages)

Layer 2 of §9.3, made concrete. **Every** edgecommons component runs a library-owned republish
listener (Java: `uns/RepublishListener`, wired by the `EdgeCommons` runtime after init completes) —
the answering half of the `uns-bridge`'s reconnect-rehydration broadcast (DESIGN-uns-bridge §2.5)
and the lever the edge-console's config-review pulls. The wire contract is pinned by
**`uns-test-vectors/bcast.json`** (topics byte-for-byte, envelope structural, behavior constants);
the Python/Rust/TS mirrors replicate ALL of the following (shipped, four-way parity):

- **Subscriptions** — at startup, on the component's **primary** (local/IPC) connection, the two
  exact own-device topics `ecv1/{device}/_bcast/main/cmd/republish-state` and
  `…/republish-cfg`, built through the topic builder with the reserved `_bcast` pseudo-component
  identity (single-level hierarchy `[{device}]`, instance `main` — so the topic is **rootless by
  D-U25 regardless of the component's own hierarchy/root mode**, matching what the bridge
  publishes). Unsubscribed on shutdown, before messaging closes. (When the Phase-3 `commands()`
  facade lands its full `ecv1/{device}/_bcast/main/cmd/#` inbox, it may absorb these two
  subscriptions — the observable behavior below is what's normative, not the subscription count.)
- **Trigger** — an envelope whose `header.name` equals the topic's verb. `version`, `body` and any
  `reply_to` are ignored; there is **no reply** (fire-and-forget notification). Malformed or
  foreign payloads (headerless/raw JSON, mismatched name, garbage) are ignored at DEBUG and must
  never crash the component.
- **Jitter** — an accepted trigger re-announces after a uniformly random delay in
  **[0, 2000] ms** (`jitterWindowMs = 2000`, the §9.3 anti-stampede window: a whole fleet receives
  the broadcast at once). The RNG and clock are injected seams (no inline
  `random()`/`now()` calls) so every language's listener unit-tests deterministically — the same
  injected-clock discipline the bridge uses.
- **Coalescing / cooldown** — per verb, independently: a trigger is accepted only when no
  re-announce for that verb is pending AND at least **5000 ms** (`cooldownMs`) have elapsed since
  the last **accepted** trigger for that verb (measured from acceptance, not the jittered fire).
  Everything else coalesces — a looping or duplicated broadcast amplifies to at most one
  re-announce per verb per cooldown window.
- **Actions** (both through the reserved-class publish seam — library-internal by construction):
  - `republish-state` → re-emit the heartbeat `state` keepalive immediately, out of band from the
    periodic schedule: the exact tick payload `{"status":"RUNNING","uptimeSecs":n}`, same
    `heartbeat.destination` routing (Java: `Heartbeat.publishStateNow()`). **Respects
    `heartbeat.enabled`** — a component whose operator disabled the state keepalive does not
    re-announce state (the broadcast cannot re-enable an opted-out state surface).
  - `republish-cfg` → re-run the effective-config publisher (the standard redacted `cfg` push;
    Java: `EffectiveConfigPublisher.publishNow()`). Always on.
  - Both best-effort: a failing publish is logged and swallowed.
- **No config surface** — always on; core plumbing, not a feature toggle. No schema change.

### 9.5 The command inbox + built-in verbs — normative behavior (slice S2; shipped in all four languages)

The minimal `commands()` facade (§7.3), sized to what the edge-console needs first: **every**
edgecommons component runs a library-owned command inbox (Java: `commands/CommandInbox`, wired by the
`EdgeCommons` runtime after init completes, right after the §9.4 republish listener) so the console —
or any peer — can address it with request/reply verbs. The wire contract is pinned by
**`uns-test-vectors/commands.json`** (inbox filter + topics byte-for-byte, request/reply envelopes
structural, reply bodies equal to a live inbox dispatch, behavior flags/sets normative); the
Python/Rust/TS mirrors replicate ALL of the following:

- **Inbox subscription** — at startup, on the component's **primary** (local/IPC) connection, the
  single own-inbox wildcard `ecv1/{device}/{component}/main/cmd/#`, built through the topic
  builder's filter API under the component's own identity + root mode with every scope token
  pinned (rooted deployments get `ecv1/{site}/{device}/…/cmd/#` automatically). Unsubscribed on
  shutdown, before messaging closes. One subscription total — registering a custom verb never
  subscribes anything new. Only the **`main`-instance** inbox exists in this slice; per-instance
  inboxes ride the full facade (Phase 5). (This inbox does NOT absorb the §9.4 `_bcast` topics —
  those live under the `_bcast` pseudo-component, a different path.)
- **Verb identification** — the verb is the **topic channel**: everything after `cmd/`
  (`/`-namespaced verbs like `sb/status` included). The envelope's **`header.name` must equal the
  topic's verb** (the §9.4 rule); the pair is the well-formedness gate.
- **Reply contract** — a request carries `header.reply_to` + `correlation_id`. The handler's
  result is published to `reply_to` through the existing `reply()` mechanism, which stamps the
  **request's `correlation_id`** onto the reply (the `uns-bridge` rewrites `reply_to` for
  cross-broker request/reply, so this works from the console over the site broker transparently —
  console timeouts stay ≤ 30 s, under the bridge's 60 s mapping TTL). The reply envelope: header
  `{name: <verb>, version: "1.0"}`, no `reply_to`, the **responder's** `identity` (+ `tags` when
  configured — metadata, not normative). Body shapes (pinned):
  - success — `{"ok": true, "result": <verb-specific object>}` (a `null`/void handler result is
    the empty object `{}` — a plain ack);
  - error — `{"ok": false, "error": {"code": "<CODE>", "message": "<text>"}}`. Base codes:
    `UNKNOWN_VERB` (well-formed request, no such verb), `HANDLER_ERROR` (a handler threw an
    uncoded error), `RELOAD_FAILED`, `NO_CONFIG` (verb-specific, below). Custom handlers add
    codes by throwing the language's coded command exception (`CommandException(code, message)`);
    `error.message` is informative, not normative (except `UNKNOWN_VERB`'s library-composed text,
    which the vectors pin).
  - A `cmd` with **no `reply_to` is fire-and-forget**: the handler runs, nothing is published
    (handler failures are logged only). An unknown fire-and-forget verb is ignored at DEBUG.
- **Malformed / foreign** — a missing header, a `header.name` ≠ the topic verb, or any parse
  anomaly is **ignored at DEBUG — never replied to, never a crash**. (Deliberate, and stricter
  than "error reply": replying to mismatched-name payloads would double-answer foreign
  conventions that share `cmd` topics — e.g. the Flow-A config rendezvous uses header name
  `GetConfiguration` on a `get-configuration` channel — and §9.4 set the ignore precedent.)
- **Delegated verbs** — `set-config` is owned by the `CONFIG_COMPONENT` source's own subscription
  on the same inbox path (§4.3); the inbox always ignores it (DEBUG), so the two subscribers never
  double-handle and a `set-config` on a non-CONFIG_COMPONENT component is a silent no-op.
- **The three built-in verbs** (auto-registered; request body `{}`, args ignored):
  - **`ping`** → `{"status": "RUNNING", "uptimeSecs": n}` — the state keepalive's RUNNING body
    shape, same uptime source as the heartbeat (Java: `Heartbeat.getUptimeSecs()`). Proves the
    component is not just alive (the keepalive shows that) but **responsive to addressed
    commands**. Always answered, even when `heartbeat.enabled` is false.
  - **`reload-config`** → re-fetch from the **active config source** and re-apply (Java:
    `ConfigManager.reloadFromProvider()` — provider re-read/re-request, schema validation,
    `applyConfig`; listeners fire, so a successful reload also re-announces the `cfg` push).
    Reply `{"reloaded": true}`, or `RELOAD_FAILED` when the fetch fails / returns nothing / the
    document is schema-invalid (the previous config is kept; a reload never crashes a component).
  - **`get-configuration`** → the current **redacted effective config** as
    `{"config": <redacted config>}` — byte-identical to the `cfg` push body (one shared snapshot
    source; Java: `EffectiveConfigPublisher.redactedEffectiveConfig()`), or `NO_CONFIG`. This is
    **Flow B** (the console pulls a component's own config as a reply) — distinct from **Flow A**
    (§4.3, shipped earlier), where a *component* fetches its own config *from a config server* at
    `ecv1/{device}/config/main/cmd/get-configuration`. Same verb token, different recipient and
    payload. *(Known future consideration: a config-server component named `config` would need a
    library-privileged override of this built-in to serve Flow A — deferred to the full facade's
    `register(verb, schema, handler, {danger})`.)*
- **Registration seam** — `gg.getCommands().register(verb, handler)` (Java; mirrors:
  `gg.commands().register(...)` / the language's idiom), callable before or after startup. Verb
  tokens pass the §2.2 token rule (`/`-namespaces allowed). **No shadowing, ever**: registering a
  built-in, the delegated `set-config`, or an already-registered verb throws — replace a custom
  handler via `unregister(verb)` first (built-ins cannot be unregistered). The handler receives
  the full request envelope and returns the result object; it runs synchronously on the messaging
  delivery thread (keep it fast, or hand off internally). `set-log-level`, `describe`,
  `get-panel-asset` and the `sb/*` family remain deferred (§7.3 / Phase 5).
- **No config surface** — always on; core plumbing, not a feature toggle. No schema change.

---

## 10. Migration is a hard cut

No production installed base → one release train, **no dual-publish compatibility mode** (the `tag →
signal` rename is the precedent). Every topic, both southbound adapters' publish paths, the processor's
filters, the config-push source, and the interop harness change together. Old configs fail validation with
a precise error — silent coexistence of old and new topics is the worst outcome.

---

## 11. Mandated changes (all four languages; Java canonical)

> **Mandate walk — approved 2026-07-02.** All of M1–M9, M11, M14, M15 accepted, with two refinements:
> **M7 → LWT only** (broker retain deferred; D9/§9.3), and **M9 accepted in full** including confirmed `sb/write`.
> M1/M2/M8 committed as P0 (site-wide from day one), sequenced at Phase 3. Console-side M10/M12/M13 are tracked in
> the edge-console design and remain parked with the console.

| # | Change | Priority |
|---|---|---|
| M1 | **`uns-bridge`** component (Rust; one per device bus) — envelope-aware relay, `reply_to` rewrite (TTL'd map), per-class uplink policy with visible drop counters, hop-tag loop protection, LWT reachability | P0 |
| M2 | **Site-broker deploy recipes** (EMQX; HOST Docker / GG container / K8s notes) | P0 |
| M3 | **The UNS grammar** — `ecv1/{device}/{component}/{instance}/{class}`, the message classes, `/`-delimited verbs | P0 |
| M4 | **Messaging model** — `messaging()` (arbitrary topics; northbound method family; `request()` hardened) + `uns()` builder/validator + reserved facades; reserved-class guard | P0 |
| M5 | **`identity` + `hierarchy`** — configurable hierarchy schema (last = node); top-level `identity` (ordered `hier` + `path`; per-message `instance`); distributed via hierarchical config layers; `tags.thing` removed | P0 |
| M6 | **Request/reply hardening** — internal default deadline + optional timeout overload + guaranteed reply-topic cleanup; `reply()`/`subscribe()` unchanged | P0 |
| M7 | **MQTT LWT hook** in `MessagingProvider` — `uns-bridge` derives and sets it on the site-broker connection → whole-device UNREACHABLE; generic HOST/K8s component config does not expose an LWT. **Retain deferred** (redundant with broadcast `republish-state` + the consumer's timestamped cache; can't express staleness; if ever wanted, owned by the bridge). | P0 |
| M8 | **Named/secondary MessagingClient** (two connections in one process; the bridge needs it) | P0 |
| M9 | **Southbound command family** — `sb/browse` (paged), `sb/read` (ref-accepting), **confirmed `sb/write`** (with optional read-back), `sb/subscribe-preview` + adapter `writes.allow[]`; an adapter-contract change | P1 |
| M11 | **Heartbeat defaults + parity** — on / 5 s / local target in all four languages, reconciled into the `state` keepalive ([edgecommons#33](https://github.com/edgecommons/edgecommons/issues/33)) | P1 |
| M14 | **`uns-test-vectors/`** — cross-language byte-identical topic + well-formed top-level `identity` assertions in the interop harness, under the 90% coverage gate | P1 |
| M15 | **UNS in the streaming service** — auto-enrich records with identity + header + tags; Parquet/AVRO hierarchy columns + partitioning; broker-sink partition key = device; `stream:<name>` preserves originating identity | P1 |

*(M10/M12/M13 — panel schema, console config, k8s Ingress — are console-specific and live in the
`edge-console` design.)*

---

## 12. Parity & testing

- **`uns-test-vectors/`** (mirroring `vault-test-vectors/`): a JSON table of `(identity, class, channel) →
  exact topic` plus golden envelopes per class; all four suites consume it; the interop harness gains a
  UNS suite asserting cross-language byte-identical topics and `identity` == the resolved values.
- The hierarchical-config lineage merge is the identity-distribution conformance target (shared vectors there).
- 90% line-coverage gate applies to every new facade in all four languages (pure CI-testable surface).

## 13. Phasing

1. **[DONE — all four languages]** **Grammar + classes + top-level `identity` + `messaging()`/`uns()`**
   (Java-canonical first, mirrors before merge) + `uns-test-vectors` + **heartbeat-default parity
   (M11)** + the library-owned state/metric/cfg publishers + the config-component rendezvous remap.
   *(M11 moved up from step 5: the hard cut retires the legacy heartbeat topic here, and
   `heartbeat.targets[]` — where the topic drift knobs live — is dropped in the same schema break;
   deferring it would break the heartbeat config twice. See
   [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) Risks #1.)*
2. **[DONE]** **Reserved-class guard, `request()` hardening, MQTT LWT (retain deferred), northbound
   API naming.** The "named MessagingClient (Rust only — D‑U17)" item originally scoped
   here was superseded by D‑U17's final resolution (see [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md)
   §2.3): no core "named client" API was built or is needed — the `uns-bridge` builds its second
   connection by reusing the core's already-public `MqttProvider`/`DefaultMessagingService`
   constructors directly.
3. **[DONE]** **`uns-bridge` + site-broker recipes** (the site-bus realization) + reachability/late-join.
   The `_bcast` listener (§9.4) is shipped in all four languages; the `uns-bridge` component itself is
   built and passing a live dual-broker end-to-end suite in its own sibling repo (now published to
   GitHub, registered in `registry/components.json`, and pinned to the v0.2.0 UNS release — see
   [`DESIGN-uns-bridge.md`](DESIGN-uns-bridge.md) for exactly what remains before general release).
   **Slice S2 (§9.5) — the minimal `commands()` facade — [DONE, all four languages]:**
   the component command inbox + `ping`/`reload-config`/Flow-B `get-configuration` + the
   `register(verb, handler)` seam, pinned by `uns-test-vectors/commands.json`.
4. **[NOT STARTED]** **Streaming enrichment** (M15) + **southbound command family** (M9). (`opcua-adapter`
   has separately landed the M9 *capabilities* — browse/write-ack/regex-read — on its own legacy
   topics, ahead of and independent of this UNS migration; see `SOUTHBOUND.md` §2.2/§8.)
5. **[NOT STARTED as designed here, but the library facades have shipped ahead of it]** **Docs/recipes/
   scaffold updates; retire the legacy schemes** (incl. the stale `SouthboundTagUpdate`
   python-protocol-adapter template) + component adoption. Per
   [`DESIGN-class-facades.md`](DESIGN-class-facades.md) §7.1's "library-facades-first" sequencing, the
   **`data()`/`events()`/`app()` class-publish facades themselves are DONE, all four languages** (§7.3
   above) — what remains here is the per-repo component-adoption PRs (`opcua-adapter`'s
   `SignalUpdatePublisher`/`EventEmitter`, `modbus-adapter`'s `publisher.py`/`events.py`,
   `telemetry-processor`'s `route.rs`) that migrate off their own hand-built bodies onto the shipped
   facades, plus the docs/scaffold retirement this step was always scoped to.

> **Implementation companion:** [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) turns this design
> into concrete Java-canonical API shapes + per-language mirror notes and carries the running decisions
> register (D‑U1…D‑U24) — the source of truth for the build.

> The **`edge-console`** component consumes this design; see its design doc for the console-side FleetModel,
> WebSocket bridge, screens, dynamic panels, and console-specific mandates (M10/M12/M13).
