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
that each invent their own tag model, normalization, quality/timestamp semantics, payload shape, and
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
standardizes only the **body**, published with header `name = "SouthboundTagUpdate"`, `version =
"1.0"`:

```json
{
  "header": { "name": "SouthboundTagUpdate", "version": "1.0", "timestamp": "...", "uuid": "...", "correlation_id": null },
  "tags":   { "thing": "...", "appId": "...", "site": "...", "shop": "...", "line": "..." },
  "body": {
    "device":  { "adapter": "opcua", "instance": "<instanceId>", "endpoint": "opc.tcp://host:4840" },
    "tag":     { "id": "<canonical stable id>", "name": "<human label>", "address": { /* protocol-native, opaque */ } },
    "samples": [
      { "value": <any>, "quality": "GOOD|BAD|UNCERTAIN", "qualityRaw": "<native status code>",
        "sourceTs": "<ISO-8601 UTC>", "serverTs": "<ISO-8601 UTC>" }
    ]
  }
}
```

Design rules:

- **Quality is first-class.** Every sample carries a `quality` normalized to `GOOD | BAD | UNCERTAIN`
  (see §3), plus `qualityRaw` retaining the native code for diagnostics. Consumers gate on `quality`
  without knowing the protocol.
- **Identity is split.** `tag.id` is a **canonical, stable** string the cloud keys on (e.g.
  `ns=3;i=1001`); `tag.address` is the **protocol-native** identity, opaque to consumers (OPC UA
  `{ns, nodeId}`, Modbus `{unitId, register, type}`, MQTT `{topic}`). `tag.name` is the human label.
- **Site hierarchy lives in `tags`, not the body.** `thing` + the configured `tags{}` (appId / site /
  shop / line / …) ride in the envelope's `tags`, so routing and partitioning never require parsing
  the body. This is the existing tag mechanism (`MessageBuilder.withConfig(...)`).
- **Batching.** `samples` is an array so an adapter can coalesce multiple updates for one tag into one
  message (deadband/publish-interval driven).
- **Timestamps** are ISO-8601 UTC. `sourceTs` (device/field) and `serverTs` (protocol server) are
  kept distinct; both optional but at least one SHOULD be present.

### 2.1 Mapping a protocol onto the contract (OPC UA reference)

The OPC UA bridge's legacy body was `{ tag:{ns,id,browseName,displayName}, updates:[{value,quality,serverTs,sourceTs}] }`.
It maps onto the contract as:

| Contract field | OPC UA source |
|----------------|---------------|
| `device.adapter` | `"opcua"` |
| `device.instance` | the component instance id |
| `device.endpoint` | `connectionInfo.url` |
| `tag.address` | `{ ns, nodeId: id }` |
| `tag.id` | `"ns=<ns>;i=<id>"` (or `s=<id>` for string node ids) |
| `tag.name` | `displayName` if non-empty, else `browseName` |
| `samples[]` | `updates[]` → `value`→`value`; `quality`→`qualityRaw` + normalized `quality`; `serverTs`/`sourceTs` preserved |

**Cutover safety:** an adapter MAY support a per-instance `bodySchema: "legacy-<protocol>"` toggle to
emit its pre-contract body during migration. The contract body is additive, so subscribers move
topic-by-topic.

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
      "healthThresholds":{ "staleTagSecs": 30 }
    },
    "instances": [
      {
        "id": "kep1",
        "adapter": "opcua",
        "connection":  { "endpoint": "opc.tcp://host:4840/", "securityPolicy": "Basic256Sha256", "messageMode": "SignAndEncrypt" },
        "defaults":    { "publishIntervalMs": 1000, "samplingRateMs": 500, "queueSize": 100 },
        "publish":     { "topic": "southbound/{site}/{ComponentName}/{InstanceId}/{tagId}", "batchMs": 1000 },
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
`include`/`exclude` tag specs, deadband) form the convention every adapter follows; anything
protocol-specific nests under `connection` or a tag spec's matcher. Security config is detailed in
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
| `staleTags` | Count | 60 | tags with no update past `healthThresholds.staleTagSecs` |

Optional: `reconnects`, `writeErrors`, `tagsSubscribed`. Emit on connect/disconnect transitions
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

A first-class `--kind {component|protocol-adapter}` flag (resolving `templates/<lang>-<kind>`) is a
small, optional CLI follow-up once the pattern is proven.

## 7. Reference adapter

The first consumer is the **OPC UA bridge** (Eclipse Milo, standalone component repo) — migrated from
a pre-refactor build, upgraded to Milo 1.1.x, with secure connections sourced from the credentials
vault. It demonstrates the full contract end-to-end and is the template for future adapters. See that
component's README for protocol-specific configuration (security policies, cert sources, tag-match
syntax).

## 8. Roadmap

- **Tier-2** (helpers) and **Tier-3** (`gg.devices()`) are deferred until at least two real adapters
  (OPC UA + a poll-based one such as Modbus) reveal the shared pain points worth extracting.
- Quality-mapping tables for additional protocols (Modbus, EtherNet/IP, Sparkplug B) are added here
  as adapters land.
