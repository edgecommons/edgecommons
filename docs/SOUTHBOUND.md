# Southbound — design (protocol-adapter contract)

**Status: Tier-1, updated for the Unified Namespace (2026-07).** This doc defines the cross-language
*contract* that protocol-adapter components build to. The reference adapter (an OPC UA bridge on
Eclipse Milo) is the first consumer. Java is canonical; the contract is language-agnostic and carried
entirely by the existing message envelope, config schema, and metrics subsystems — **no new runtime
subsystem is required for Tier-1.**

> **UNS alignment.** The envelope and the data-plane topic below reflect the **shipped** UNS core
> ([`platform/DESIGN-uns.md`](platform/DESIGN-uns.md) /
> [`platform/UNS-CANONICAL-DESIGN.md`](platform/UNS-CANONICAL-DESIGN.md)): the envelope carries a
> top-level **`identity`** element (`tags.thing` is removed), and signal updates ride the UNS
> **`data`** class instead of the legacy `southbound/…` topic templates. The **command surface**
> (§2.2, the `cmd/sb/*` family — the UNS-addressed topics plus the cross-adapter `writes.allow[]`
> convention) is the approved **Phase 5 target design and is NOT yet built** — the shipping adapters
> still use their legacy per-instance control topics (`.../control/status|subscriptions|nodes`). The
> **capabilities** that family targets are no longer purely aspirational, though: `opcua-adapter`
> landed paged address-space browse, a confirmed write with per-entry acknowledgment, and
> regex-matched on-demand reads on its own legacy topics (merged 2026-07-02, "command-surface-parity",
> `opcua-adapter@5dbb789`) — a first per-adapter proof of the target *behavior*, not yet the UNS
> `cmd/sb/*` topic family or a shared cross-adapter facade. See §2.2 and §7.

## 1. Why a contract, not a subsystem

A protocol adapter (OPC UA, Modbus, EtherNet/IP, MQTT-Sparkplug, …) is **business logic a developer
writes as a component on edgecommons** — the framework already gives it config, messaging, metrics,
credentials, and streaming, so the author writes only the protocol code. edgecommons does **not** ship
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

Adapters reuse the standard `Message` envelope — since the UNS change, `{header, identity, tags,
body}` — with the library stamping `identity` automatically. The contract standardizes only the
**body**, published with header `name = "SouthboundSignalUpdate"`, `version = "1.0"`:

```json
{
  "header": { "name": "SouthboundSignalUpdate", "version": "1.0", "timestamp": "...", "uuid": "...", "correlation_id": null },
  "identity": {
    "hier": [
      { "level": "site",   "value": "dallas" },
      { "level": "device", "value": "gw-01" }
    ],
    "path":      "dallas/gw-01",
    "component": "opcua-adapter",
    "instance":  "kep1"
  },
  "tags":   { "appId": "..." },
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
> keeps the two senses apart. The envelope **`tags`** are arbitrary *business* metadata — there are
> *no* mandated keys; `appId` above is only an example (the existing `MessageBuilder.withConfig(...)`
> mechanism). Location/identity does **not** ride in `tags` anymore: `tags.thing` is **removed** (hard
> cut) and the site hierarchy lives in the top-level **`identity`** element. A **`signal`** is a
> single data point — one measured value with identity, quality, and timestamps (what OPC UA calls a
> "tag" and Modbus calls a "register"). Earlier revisions of this doc called the data point a "tag";
> it is now uniformly **`signal`**, leaving `tags` to mean envelope business metadata only.

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
- **Identity is the top-level `identity` element, not the body (and not `tags`).** Every publish is
  stamped with `{hier, path, component, instance}`: the enterprise hierarchy (from the top-level
  `hierarchy`/`identity` config blocks — the last `hier` entry is the device, i.e. the resolved thing
  name), the adapter's component token, and the **instance** the update pertains to (stamped
  per-message via `gg.instance(id)` — see DESIGN-uns §5.3). Routing and partitioning never require
  parsing the body *or* the topic. The former `tags.thing` field is removed.
- **Batching.** `samples` is an array so an adapter can coalesce multiple updates for one signal into one
  message (deadband/publish-interval driven).
- **Timestamps** are ISO-8601 UTC. `sourceTs` (device/field) and `serverTs` (protocol server) are
  kept distinct; both optional but at least one SHOULD be present.
- **Value typing.** `value` is JSON-native: numbers (including unsigned integers) as JSON numbers,
  booleans as JSON booleans, strings as strings, and **date/time as an ISO-8601 string**. An
  **array-valued signal is a JSON array**, each element following these same rules (and writes accept a
  JSON array, coerced to the element type). A value an adapter cannot model as one of these (e.g. an
  opaque blob or a structure) MAY be rendered as a string; adapters SHOULD document such fallbacks.

### 2.0 The data-plane topic — the UNS `data` class

Signal updates are published on the component's UNS **`data`** topic
([`platform/DESIGN-uns.md`](platform/DESIGN-uns.md) §3–§4):

```text
ecv1/{device}/{component}/{instance}/data/{signalPath}
```

minted via the instance-scoped topic builder — never a hand-assembled string:

```java
EdgeCommonsInstance kep1 = gg.instance("kep1");
String topic = kep1.uns().topic(UnsClass.DATA, "press12/temperature");
// -> ecv1/gw-01/opcua-adapter/kep1/data/press12/temperature
gg.messaging().publish(topic,
    kep1.newMessage("SouthboundSignalUpdate", "1.0").withPayload(body).build());
```

- The message name stays **`SouthboundSignalUpdate`** — only the addressing changed. The legacy
  config-template scheme `southbound/{site}/{ComponentName}/{InstanceId}/{signalId}` is **retired**
  (DESIGN-uns §6): the topic addresses the endpoint (`device`/`component`/`instance`), and the site
  hierarchy rides in the envelope `identity`, not the topic.
- `{signalPath}` is the signal's channel form, subject to the UNS token rule and the IoT-Core depth
  guard (≤ 3 channel tokens rootless, 2 rooted — enforced by `uns()` at build time). The stable
  `signal.id` in the body remains the identity consumers key on; the exact
  sanitized-`signalId`-as-channel rule is pinned in Phase 5 (D‑U15).
- A fleet consumer subscribes **one wildcard** — `ecv1/+/+/+/data/#` — instead of per-adapter topic
  templates (one of the six-wildcard UNS consumer set).
- **Adapter adoption status:** the library surface this rides on (`identity` stamping,
  `gg.instance(id)`, the `uns()` builder, the `data` class) is **shipped in all four languages**;
  the reference adapters (`opcua-adapter`, `modbus-adapter`) re-point their publish paths in the UNS
  **component-adoption train (Phase 5)** — until those migration PRs land, the deployed adapters
  still publish on their legacy config-template topics.

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

### 2.2 Command surface — the `cmd/sb/*` family (Phase 5 / M9 — target design, NOT yet shipped)

> **Status: roadmap for the UNS topic family and the cross-adapter facade; a first per-adapter
> capability precedent already exists.** This section is the approved **target design** for the
> southbound command family (DESIGN-uns §11 mandate **M9**; decisions D‑U15/D‑U16), scheduled for
> **Phase 5** of the UNS train. The `cmd/sb/*` **topic family**, the `writes.allow[]` config
> convention, and a generalized cross-adapter `commands()` facade are **not built**. The shipping
> adapters currently still expose their **legacy per-instance control topics** — config-template
> `write.topic` / `read.topic` for batch write and on-demand read, plus the
> `southbound/{ComponentName}/{InstanceId}/control/{status|subscriptions|nodes}` topics. That said,
> `opcua-adapter` has already landed the **capabilities** this family targets — paged address-space
> browse (`control/nodes`), a confirmed write with per-entry `SouthboundWriteResult` acknowledgment,
> and regex include/exclude matchers for on-demand reads (merged 2026-07-02,
> "command-surface-parity", `opcua-adapter@5dbb789`) — on its own legacy topics, ahead of the UNS
> migration. Treat this as a validated reference implementation of the *behavior* §2.2 specifies,
> not as the family itself being shipped.

Beyond streaming subscriptions, an adapter exposes an on-demand command surface as **built-in `cmd`
verbs on its UNS inbox**, family-namespaced under `sb/` and addressed to

```text
ecv1/{device}/{component}/{instance}/cmd/sb/{verb}
```

(the `cmd` class is the one class whose identity path names the **recipient**; the verbs are
registered through the `commands()` facade and advertised in `describe`):

| Verb (`cmd/sb/…`) | Kind | Purpose |
|---|---|---|
| `sb/status` | request/reply | instance/connection status (replaces the legacy `…/control/status`) |
| `sb/browse` | request/reply, **paged** | enumerate the address space / configured signals |
| `sb/read` | request/reply | on-demand read of arbitrary signals (ref-accepting) |
| `sb/write` | request/reply, **confirmed** | write signals; the reply reports per-write success/failure, with an **optional read-back** |
| `sb/subscribe-preview` | request/reply | evaluate a subscription spec without subscribing |

- **`<signal-ref>`** addresses a signal by its **stable** identity where possible — for OPC UA,
  `"namespaceUri": "<uri>"` (preferred, resolved to the current index) or a literal `"ns": <int>`,
  plus `"signalId": "<id>"`; for Modbus, `{ "name": "<configured signal>" }` or an explicit
  `{ "unitId"?, "table", "address", "type", ... }`. This keeps request inputs, like the published
  `address`, independent of a volatile index.
- **Writes are confirmed and allow-listed.** Request body
  `{ "writes": [ { <signal-ref>, "value": <any>, "sourceTs": "<iso>"? }, ... ] }` (a single object
  without the `writes` array is also accepted); one round-trip writes many signals, and the reply
  confirms each write (optionally with the read-back value). An adapter accepts a write **only**
  when the target matches its **`writes.allow[]`** config allow-list, matched against the stable
  `signal.id` (D‑U16).
- **Reads** reuse the §2 value/quality encoding: request `{ "signals": [ { <signal-ref> }, ... ] }` →
  reply body `{ "id": "<instance>", "reads": [ { "signal": {id, address}, "value", "quality",
  "qualityRaw", "sourceTs", "serverTs" }, ... ] }`.

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

Verified against `schema/edgecommons-config-schema.json`: the **top level is strict**
(`additionalProperties:false`, `required:["component"]`), but **`component.global` and
`component.instances[]` are permissive** (`additionalProperties:true`). Therefore an adapter places
its config under `component.*` and needs **no schema change** (no `schema/sync-schema` run, no CI
drift-gate risk).

> Do **not** add a dedicated top-level block (e.g. `opcua`) — that would force an edit to the
> canonical `schema/edgecommons-config-schema.json`, a `sync-schema` regeneration of all four library
> copies, and a passing `schema-drift` check. Keep adapter config under `component`.

Convention — protocol-agnostic keys at the top, protocol-native detail nested:

```jsonc
{
  "hierarchy":       { "levels": ["site", "device"] },              // UNS enterprise hierarchy (last level = the device)
  "identity":        { "site": "dallas" },                          // values for every level except the last (= thing name)
  "tags":            { "appId": "..." },                            // business metadata only (location moved to identity)
  "messaging":       { "local": { "host": "...", "port": 1883 } },  // replaces legacy mqtt{}
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
        "publish":     { "batchMs": 1000 },              // topic is UNS-minted (§2.0), no longer a config template
        "writes":      { "allow": [ "ns=3;i=1001" ] },   // Phase 5 (M9): write allow-list by stable signal.id — NOT yet shipped
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

Keys that are protocol-agnostic (`connection`, `defaults`, `publish`, `writes`, `subscriptions` with
`include`/`exclude` signal specs, deadband) form the convention every adapter follows; anything
protocol-specific nests under `connection` or a signal spec's matcher. Security config is detailed in
the OPC UA adapter's own doc (cert sources: `vault` / `file` / `pkcs11`).

> **Transition note.** `hierarchy` / `identity` / `topic.includeRoot` are top-level **schema**
> sections (shipped — see DESIGN-uns §5 and the canonical `schema/edgecommons-config-schema.json`).
> Until the Phase-5 adapter migration lands, the **shipping** reference adapters still accept their
> legacy `publish.topic` / `write.topic` template keys under `component.*`; those keys disappear
> with the migration (the data-plane topic is minted by `uns()`, and `write` is replaced by the
> `cmd/sb/*` family + `writes.allow[]`).

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
`test-configs` seeding the §4 convention). The **`protocol-adapter` kind** is a first-class scaffold
axis — `-k/--kind` selects the archetype, `-l/--language` the language:

```bash
edgecommons component new -l JAVA -k protocol-adapter \
  -n com.example.MyAdapter --platforms GREENGRASS,HOST
```

A Python mirror ships too — a Builder + per-instance worker-thread skeleton with `recipe.yaml`,
`Dockerfile`, and `k8s/`:

```bash
edgecommons component new -l PYTHON -k protocol-adapter \
  -n com.example.MyAdapter --platforms GREENGRASS,HOST,KUBERNETES
```

Both scaffolds ship a `config.schema.json` modelling the southbound adapter's own configuration
(`connection`, `subscriptions`, per-signal rules), so `edgecommons component validate` checks an
adapter's config against the contract in §4 rather than merely against the library envelope.
Run `edgecommons template list` for the full language × kind matrix.

## 7. Reference adapter

The first consumer is the **OPC UA bridge** (Eclipse Milo, standalone component repo) — migrated from
a pre-refactor build, upgraded to Milo 1.1.x, with secure connections sourced from the credentials
vault. It demonstrates the full contract end-to-end and is the template for future adapters. It has
also landed the Phase-5 command-surface *capabilities* early — paged address-space browse, a
confirmed write with per-entry acknowledgment, and regex-matched on-demand reads (§2.2) — on its own
legacy control topics, ahead of the UNS `cmd/sb/*` migration. See that component's README for
protocol-specific configuration (security policies, cert sources, signal-match syntax).

The **Modbus adapter** (pymodbus, **Python**, standalone repo) is the second reference and the
**poll-based** counterpart to OPC UA's subscribe model. It validates that the contract is
language-agnostic and exercises the parts OPC UA does not — polling with register coalescing,
client-side change/deadband, and a synthesized type/scaling layer (byte/word order, scale/offset, bit
extraction). Its mapping is §2.1.1; protocol-specific configuration is in its own docs.

## 8. Roadmap

- **UNS Phase 5 — adapter adoption + the command family (M9).** The reference adapters re-point
  their publish paths onto the UNS `data` class (§2.0) and gain the `cmd/sb/*` command family +
  `writes.allow[]` (§2.2) — an adapter-contract change tracked in
  [`platform/DESIGN-uns.md`](platform/DESIGN-uns.md) §13 (with D‑U15/D‑U16 pinned there). Until it
  lands, deployed adapters use the legacy topics. `opcua-adapter`'s 2026-07-02
  command-surface-parity work (browse/write-ack/regex-read, on its legacy topics) is a head start on
  the *behavior* this migration re-platforms onto UNS — it does not by itself close M9.
- With the OPC UA (subscribe-based, Java) and Modbus (poll-based, Python) adapters now landed, the
  two-adapter precondition for **Tier-2** (shared helpers: poll/subscribe scheduler with
  backpressure + deadbanding, connection lifecycle, quality/timestamp stamping, store-and-forward) is
  met — Tier-2 extraction is the natural next step (still deferred from this Tier-1 doc). **Tier-3**
  (`gg.devices()`) remains further out.
- Quality + address mappings now cover OPC UA (§2.1) and Modbus (§2.1.1, §3); further protocols
  (EtherNet/IP, Sparkplug B) are added here as adapters land.
