# Southbound — design (protocol-adapter contract)

**Status: Tier-1 PROPOSED.** This doc defines the cross-language *contract* that protocol-adapter
components build to. The reference adapter (an OPC UA bridge on Eclipse Milo) is the first consumer.
Java is canonical; the contract is language-agnostic and carried entirely by the existing message
envelope, config schema, and metrics subsystems — **no new runtime subsystem is required for Tier-1.**

## 1. Why a contract, not a subsystem

A protocol adapter (OPC UA, Modbus, EtherNet/IP, MQTT-Sparkplug, …) is **business logic a developer
writes as a component on ggcommons** — the framework already gives it config, messaging, metrics,
credentials, and streaming, so the author writes only the protocol code. ggcommons does **not** ship
the protocols.

But if every adapter is built independently on just the generic plumbing, the result is N adapters
that each invent their own signal model, normalization, quality/timestamp semantics, payload shape, and
health metrics — and the cloud side has to special-case each one. The framework's leverage is to
define the **southbound contract** so the *ecosystem* of adapters is consistent and interoperable,
even though it implements none of the protocols.

Three tiers, increasing in framework involvement (this doc specifies **Tier-1**):

| Tier | What | Status |
|------|------|--------|
| **Tier-1** | A normalized telemetry **envelope**, an adapter **config convention**, standard adapter **health metrics**, and a **`protocol-adapter` scaffold template**. Pure conventions + reuse of existing subsystems. | **this doc** |
| Tier-2 | Shared **helper utilities** every adapter re-implements: poll/subscribe scheduler with backpressure + deadbanding, southbound connection lifecycle (retry/backoff/circuit-breaker/health), quality+timestamp stamping, store-and-forward during WAN outage. | deferred |
| Tier-3 | An opt-in **`gg.devices()`** seam (a `DeviceSource`/`DeviceSink` interface the adapter implements; the framework supplies scheduling, reconnect, buffering, and northbound publishing) — symmetric with the northbound messaging transport abstraction. | deferred |

## 2. The normalized telemetry envelope

Adapters reuse the existing `Message` envelope (header + tags + body) **unchanged**. The contract
standardizes only the **body**, published with header `name = "SouthboundSignalUpdate"`, `version =
"1.0"`:

```json
{
  "header": { "name": "SouthboundSignalUpdate", "version": "1.0", "timestamp": "...", "uuid": "...", "correlation_id": null },
  "tags":   { "thing": "...", "appId": "...", "site": "...", "shop": "...", "line": "..." },
  "body": {
    "device":  { "adapter": "opcua", "instance": "<instanceId>", "endpoint": "opc.tcp://host:4840" },
    "signal":  { "id": "<canonical stable id>", "name": "<human label>", "address": { /* protocol-native, opaque */ } },
    "samples": [
      { "value": <any>, "quality": "GOOD|BAD|UNCERTAIN", "qualityRaw": "<native status code>",
        "sourceTs": "<ISO-8601 UTC>", "serverTs": "<ISO-8601 UTC>" }
    ]
  }
}
```

> **Terminology — envelope `tags` vs `signal`.** The word "tag" is overloaded in IoT, so this contract
> keeps the two senses apart. The envelope **`tags`** are arbitrary message metadata — there are *no*
> mandated keys; `thing` / `site` / `shop` / `line` above are only examples (the existing
> `MessageBuilder.withConfig(...)` mechanism). A **`signal`** is a single data point — one measured
> value with identity, quality, and timestamps (what OPC UA calls a "tag" and Modbus calls a
> "register"). Earlier revisions of this doc called the data point a "tag"; it is now uniformly
> **`signal`**, leaving `tags` to mean envelope metadata only.

Design rules:

- **Quality is first-class.** Every sample carries a `quality` normalized to `GOOD | BAD | UNCERTAIN`
  (see §3), plus `qualityRaw` retaining the native code for diagnostics. Consumers gate on `quality`
  without knowing the protocol.
- **Identity is split.** `signal.id` is a **canonical, stable** string the cloud keys on (e.g.
  `ns=3;i=1001`); `signal.address` is the **protocol-native** identity for round-tripping back to the
  device (OPC UA `{ns, namespaceUri, nodeId}`, Modbus `{unitId, register, type}`, MQTT `{topic}`).
  Where a protocol's index-style handle is unstable, the address SHOULD also carry the stable form —
  e.g. OPC UA's namespace **URI** alongside the volatile namespace **index** — so consumers and
  round-trip reads/writes need not depend on the index. `signal.name` is the human label.
- **Site hierarchy lives in `tags`, not the body.** `thing` + the configured `tags{}` (appId / site /
  shop / line / …) ride in the envelope's `tags`, so routing and partitioning never require parsing
  the body. This is the existing tag mechanism (`MessageBuilder.withConfig(...)`).
- **Batching.** `samples` is an array so an adapter can coalesce multiple updates for one signal into one
  message (deadband/publish-interval driven).
- **Timestamps** are ISO-8601 UTC. `sourceTs` (device/field) and `serverTs` (protocol server) are
  kept distinct; both optional but at least one SHOULD be present.
- **Value typing.** `value` is JSON-native: numbers (including unsigned integers) as JSON numbers,
  booleans as JSON booleans, strings as strings, and **date/time as an ISO-8601 string**. An
  **array-valued signal is a JSON array**, each element following these same rules (and writes accept a
  JSON array, coerced to the element type). A value an adapter cannot model as one of these (e.g. an
  opaque blob or a structure) MAY be rendered as a string; adapters SHOULD document such fallbacks.

### 2.1 Mapping a protocol onto the contract (OPC UA reference)

The OPC UA bridge's legacy body was `{ tag:{ns,id,browseName,displayName}, updates:[{value,quality,serverTs,sourceTs}] }`.
It maps onto the contract as:

| Contract field | OPC UA source |
|----------------|---------------|
| `device.adapter` | `"opcua"` |
| `device.instance` | the component instance id |
| `device.endpoint` | `connectionInfo.url` |
| `signal.address` | `{ ns, namespaceUri, nodeId: id }` — `namespaceUri` is the stable identity; `ns` (index) can change between servers/restarts |
| `signal.id` | `"ns=<ns>;i=<id>"` (or `s=<id>` for string node ids) |
| `signal.name` | `displayName` if non-empty, else `browseName` |
| `samples[]` | `updates[]` → `value`→`value`; `quality`→`qualityRaw` + normalized `quality`; `serverTs`/`sourceTs` preserved |

**Cutover safety:** an adapter MAY support a per-instance `bodySchema: "legacy-<protocol>"` toggle to
emit its pre-contract body during migration. The contract body is additive, so subscribers move
topic-by-topic.

### 2.1.1 Mapping Modbus onto the contract (poll-based reference)

The Python Modbus adapter is the **poll-based** reference (OPC UA is the subscribe-based one). Modbus
has no eventing or discovery, so the adapter polls a config-declared register map and detects change
client-side; richer types are *synthesized* from bits + 16-bit registers (configurable byte/word
order, scale/offset, single-bit extraction).

| Contract field | Modbus source |
|----------------|---------------|
| `device.adapter` | `"modbus"` |
| `device.instance` | the component instance id |
| `device.endpoint` | e.g. `tcp://host:502 unit=1` (also serial `rtu` / `rtutcp`) |
| `signal.address` | `{ unitId, table, address, type, wordOrder?, byteOrder?, bit?, count? }` — `table` ∈ `coil`/`discrete`/`holding`/`input` |
| `signal.id` | `"u<unitId>/<table>/<address>/<type>"` (stable canonical id) |
| `signal.name` | the configured signal name |
| `samples[]` | one per poll publish (deadband-gated); `value` decoded per the signal's type; `quality` `GOOD`, or `BAD` with the exception/timeout in `qualityRaw` |

There is no namespace or discovery — signals are **declared explicitly** in config (no regex matching
against a browsed address space). For the command surface (§2.2), a Modbus `<signal-ref>` is either
`{ "name": "<configured signal>" }` (the friendly, stable form) or an explicit
`{ "unitId"?, "table", "address", "type", ... }` for arbitrary access.

### 2.2 Command surface (on-demand read + write)

Beyond streaming subscriptions, an adapter MAY expose a request/reply **command surface** so clients
can read or write arbitrary signals at any time, on per-instance topics:

- **Batch write** (fire-and-forget) — body `{ "writes": [ { <signal-ref>, "value": <any>,
  "status": "GOOD|BAD|UNCERTAIN"?, "sourceTs": "<iso>"? }, ... ] }`. A single object (no `writes`
  array) is also accepted. One round-trip writes many signals.
- **On-demand read** (request/reply) — request body `{ "signals": [ { <signal-ref> }, ... ] }`; the reply
  (`SouthboundReadResult`) body is `{ "id": "<instance>", "reads": [ { "signal": {id, address}, "value",
  "quality", "qualityRaw", "sourceTs", "serverTs" }, ... ] }`.

`<signal-ref>` addresses a signal by its **stable** identity where possible — for OPC UA, `"namespaceUri":
"<uri>"` (preferred, resolved to the current index) or a literal `"ns": <int>`, plus `"signalId":
"<id>"`. This keeps request inputs, like the published `address`, independent of a volatile index.

Both batch (one round-trip for many signals) and reuse the §2 value/quality encoding. Topic templates
are per-instance config (`write.topic`, `read.topic`).

## 3. Quality normalization

`quality` is the normalized, protocol-independent verdict; `qualityRaw` preserves the native code.

| Normalized | Meaning | OPC UA (`StatusCode`) | Modbus | MQTT/passthrough |
|------------|---------|-----------------------|--------|------------------|
| `GOOD` | value is trustworthy | `isGood()` | successful read | message received |
| `UNCERTAIN` | value present but suspect | `isUncertain()` | stale/partial | n/a |
| `BAD` | value not trustworthy | `isBad()` | exception/timeout | LWT / disconnect |

Adapters MUST set `qualityRaw` to the native representation (e.g. the OPC UA status code name/number,
a Modbus exception code) so operators can diagnose without the device.

## 4. Adapter config convention

Verified against `schema/ggcommons-config-schema.json`: the **top level is strict**
(`additionalProperties:false`, `required:["component"]`), but **`component.global` and
`component.instances[]` are permissive** (`additionalProperties:true`). Therefore an adapter places
its config under `component.*` and needs **no schema change** (no `schema/sync-schema` run, no CI
drift-gate risk).

> Do **not** add a dedicated top-level block (e.g. `opcua`) — that would force an edit to the
> canonical `schema/ggcommons-config-schema.json`, a `sync-schema` regeneration of all four library
> copies, and a passing `schema-drift` check. Keep adapter config under `component`.

Convention — protocol-agnostic keys at the top, protocol-native detail nested:

```jsonc
{
  "tags":            { "appId": "...", "site": "...", "shop": "...", "line": "..." },   // replaces legacy source{}
  "messaging":       { "local": { "host": "...", "port": 1883 } },                      // replaces legacy mqtt{}
  "metricEmission":  { "target": "messaging" },
  "component": {
    "global": {
      "defaults":        { "publishIntervalMs": 1000, "samplingRateMs": 500, "queueSize": 100 },
      "healthThresholds":{ "staleSignalSecs": 30 }
    },
    "instances": [
      {
        "id": "kep1",
        "adapter": "opcua",
        "connection":  { "endpoint": "opc.tcp://host:4840/", "securityPolicy": "Basic256Sha256", "messageMode": "SignAndEncrypt" },
        "defaults":    { "publishIntervalMs": 1000, "samplingRateMs": 500, "queueSize": 100 },
        "publish":     { "topic": "southbound/{site}/{ComponentName}/{InstanceId}/{signalId}", "batchMs": 1000 },
        "write":       { "enabled": true, "topic": "southbound/{ComponentName}/{InstanceId}/write" },
        "subscriptions": [
          {
            "id": "sine",
            "include": [ { "namespace": 2, "match": "^Simulation\\.Sine.*", "samplingRateMs": 50, "queueSize": 100, "deadband": { "type": "Absolute", "value": 0.5 } } ],
            "exclude": [ { "namespace": 2, "match": "Simulation\\.Sine4" } ]
          }
        ]
      }
    ]
  }
}
```

Keys that are protocol-agnostic (`connection`, `defaults`, `publish`, `write`, `subscriptions` with
`include`/`exclude` signal specs, deadband) form the convention every adapter follows; anything
protocol-specific nests under `connection` or a signal spec's matcher. Security config is detailed in
the OPC UA adapter's own doc (cert sources: `vault` / `file` / `pkcs11`).

## 5. Standard adapter health metrics

Every adapter emits one metric, `southbound_health`, dimensioned by `instance` (plus the
auto-injected `coreName`/`component`), via `MetricBuilder` → `MetricEmitter`. The destination is
config-driven (`metricEmission.target`: `log` / `messaging` / `cloudwatch` / `prometheus`), so no
code change is needed to route it.

| Measure | Unit | Resolution | Meaning |
|---------|------|-----------|---------|
| `connectionState` | Count | 1 | 1 = connected, 0 = down |
| `publishLatencyMs` | Milliseconds | 1 | northbound publish latency |
| `pollLatencyMs` | Milliseconds | 1 | read/poll round-trip |
| `readErrors` | Count | 60 | read errors over the interval |
| `staleSignals` | Count | 60 | signals with no update past `healthThresholds.staleSignalSecs` |

Optional: `reconnects`, `writeErrors`, `signalsSubscribed`. Emit on connect/disconnect transitions
(`emitMetricNow`) and on a periodic sampler.

## 6. The `protocol-adapter` scaffold template

There is no component "kind" concept today — the CLI keys templates by language only. Tier-1 ships a
`templates/java-protocol-adapter/` directory (mirror of `templates/java/` with a modernized,
Builder + `CountDownLatch` lifecycle skeleton, an OPC UA-ready `pom.xml`, and a `recipe.yaml` /
`test-configs` seeding the §4 convention). Scaffold it with the existing `--template-url` flag (no CLI
change required):

```bash
ggcommons create-component -l JAVA -u ./templates/java-protocol-adapter \
  -n com.example.MyAdapter --platforms GREENGRASS,HOST
```

A `templates/python-protocol-adapter/` mirror ships too — a Builder + per-instance worker-thread
skeleton with `recipe.yaml`, `Dockerfile`, and `k8s/` — scaffolded the same way:

```bash
ggcommons create-component -l PYTHON -u ./templates/python-protocol-adapter \
  -n com.example.MyAdapter --platforms GREENGRASS,HOST,KUBERNETES
```

A first-class `--kind {component|protocol-adapter}` flag (resolving `templates/<lang>-<kind>`) is a
small, optional CLI follow-up once the pattern is proven.

## 7. Reference adapter

The first consumer is the **OPC UA bridge** (Eclipse Milo, standalone component repo) — migrated from
a pre-refactor build, upgraded to Milo 1.1.x, with secure connections sourced from the credentials
vault. It demonstrates the full contract end-to-end and is the template for future adapters. See that
component's README for protocol-specific configuration (security policies, cert sources, signal-match
syntax).

The **Modbus adapter** (pymodbus, **Python**, standalone repo) is the second reference and the
**poll-based** counterpart to OPC UA's subscribe model. It validates that the contract is
language-agnostic and exercises the parts OPC UA does not — polling with register coalescing,
client-side change/deadband, and a synthesized type/scaling layer (byte/word order, scale/offset, bit
extraction). Its mapping is §2.1.1; protocol-specific configuration is in its own docs.

## 8. Roadmap

- With the OPC UA (subscribe-based, Java) and Modbus (poll-based, Python) adapters now landed, the
  two-adapter precondition for **Tier-2** (shared helpers: poll/subscribe scheduler with
  backpressure + deadbanding, connection lifecycle, quality/timestamp stamping, store-and-forward) is
  met — Tier-2 extraction is the natural next step (still deferred from this Tier-1 doc). **Tier-3**
  (`gg.devices()`) remains further out.
- Quality + address mappings now cover OPC UA (§2.1) and Modbus (§2.1.1, §3); further protocols
  (EtherNet/IP, Sparkplug B) are added here as adapters land.
