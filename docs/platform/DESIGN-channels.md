# DESIGN — the three-channel messaging/telemetry model

> Status: **accepted / Phase A implemented** (2026-07-06) — **concretized by the Unified Namespace**
> ([`DESIGN-uns.md`](DESIGN-uns.md) / [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md)), whose
> **core has since shipped**. The UNS supplies the concrete topic grammar
> (`ecv1/{device}/{component}/{instance}/{class}`), message classes, top-level `identity`, the
> `gg.uns()` builder, and (as designed roadmap) the streaming identity-enrichment for channel 3
> (DESIGN-uns §8 / M15). What this doc calls "today" is the **pre-UNS** state it was grounded
> against; overtaken specifics are flagged inline. The channel-2 "northbound ≠ IoT Core"
> generalization is now the library contract for config and public messaging APIs.

## Motivation

Today EdgeCommons exposes a **dual-broker** messaging model — a *local* broker and an *AWS IoT Core*
broker — plus a separate, independent **streaming** subsystem (`gg.streams()`). In practice the
useful conceptual model is **three channels**, and "northbound = AWS IoT Core" is too specific: the
northbound control plane is just *an MQTT broker*, which **may** be IoT Core but could equally be an
on-prem broker, a cloud broker, or a historian gateway. High-rate telemetry, meanwhile, usually
should **not** go through the northbound MQTT broker at all — it belongs on the streaming channel.

This design generalizes the model to three first-class channels a component can use in combination:

| # | Channel | Transport | Profile | Today |
|---|---------|-----------|---------|-------|
| **1** | **Local bus** | IPC (Greengrass only) **or** MQTT | in-process / on-host pub-sub | `publish`/`subscribe`/`request`/`reply` (generic) |
| **2** | **Northbound control plane** | **always MQTT**, may be IoT Core | lower-throughput, lower-QoS commands/status/alarms | `publishNorthbound` / `publish_northbound` / `Destination::Northbound` — generic in names, config, and TLS |
| **3** | **Northbound streaming** | native durable buffer → Kinesis/Kafka | high-throughput, high-durability telemetry | `gg.streams()` — a fully independent subsystem |

The skeleton of all three already exists. The work is (a) **decoupling channel 2 from "IoT Core"** into a generic northbound MQTT broker, and (b) **positioning channel 3 as a first-class peer** with a clean per-signal routing story so components can split traffic across 2 and 3.

## Current state (grounded)

- **Channel 1 is already generic.** In all four languages the "local" path is an ordinary MQTT client (Java `StandaloneMessagingProvider` `tcp://`/`ssl://` host:port with plain / user-pass / server-TLS / mutual-TLS keyed on `caPath`; Rust `rumqttc`, TS `mqtt.js`, Python paho). On Greengrass it's Nucleus IPC.
- **Channel 2 is the same generic MQTT client, formerly artificially IoT-Core-shaped.** The northbound broker is the *same* dual-MQTT client distinguished by a role tag. The config is `messaging.northbound`; TLS is keyed on `caPath` like the local broker, so the northbound broker can be plaintext, username/password, server-TLS, or mutual-TLS MQTT. Public messaging APIs now use `northbound` names; AWS IoT Core is one possible broker/Greengrass bridge, not the API name.
- **Channel 3 is clean and disjoint.** Streaming shares no code, transport, envelope, or topic with messaging. Records are opaque (`append(partitionKey, timestampMs, byte[])` to a named sink stream), it's opt-in (only when a top-level `streaming` section exists), and a component can already configure `messaging` **and** `streaming` together.
- **A channel selector already exists** for framework-emitted messages: metrics/heartbeat use a `destination` string (`ipc`/`local` vs `northbound`) to pick channel 1 vs 2.
- **Readiness is already channel-aware:** `connected()` reports only the *local* broker — a dropped cloud link must not flip `/readyz`. (Preserve this.)
- **The reference adapters use channel 1 only.** Both OPC UA and Modbus publish via local `publish()` with config-driven topic templates (`southbound/{ComponentName}/{InstanceId}/{signalId}`) and per-signal topic overrides; neither calls IoT Core (their recipes grant the IPC IoT-Core policy but the code never uses it) and neither streams. Their per-signal batching buffer (`samples[]` + `batchMs`) is the natural seam to divert high-rate signals to channel 3. *(Overtaken: the UNS retires those topic templates — channel-1 data now rides `ecv1/{device}/{component}/{instance}/data/{signalPath}`, minted via `gg.instance(id).uns()`; the adapters re-point in UNS Phase 5. See `../SOUTHBOUND.md` §2.0.)*

## Proposed design — library level

### Config schema (`schema/edgecommons-config-schema.json` → `sync-schema`)
- **Use `messaging.northbound`** (a `$ref` to the existing generic `mqttBroker` definition). There is no `messaging.iotCore` back-compat alias; configs hard-cut to the generic name. (Fuller option: `messaging.northbound[]` as an array, for multiple northbound brokers — e.g. cloud + on-prem historian.)
- **Generalize `mqttBroker`**: it's already generic (`host`/`endpoint`/`port`/`clientId`/`credentials`). Accept `host` **or** `endpoint` interchangeably, and document that the northbound broker supports the same auth options as local (plaintext / username-password / server-TLS / mutual-TLS) — i.e. **drop the IoT-Core-only "must be `ssl://` + mutual TLS" assumption**; key TLS on `caPath` exactly like local.
- **Use `"northbound"` in the `destination` enum** in `metricEmission.targetConfig.destination` and `heartbeat.destination`. There are no `iotcore`/`iot_core` destination aliases.
- **`streaming`** needs no schema change for the model — optionally document it as "channel 3."

### Messaging API / provider
- **Generic northbound connect.** Build the northbound MQTT client from the *same* connect path as local (scheme from TLS presence, optional auth, TLS keyed on `caPath`) instead of the IoT-Core-hardwired path. IoT Core then becomes "a northbound broker that happens to require mutual TLS to an `ssl://` endpoint" — a config, not a code branch.
- **Java / Python:** expose only the `…Northbound` / `*_northbound` method family (`publishNorthbound`, `subscribeNorthbound`, `requestNorthbound`, `replyNorthbound`, …). The old IoT-Core-named public methods are not retained as aliases in the hard-cut API.
- **Rust / TS (destination-enum seam):** expose `Destination::Northbound` / `Destination.Northbound` and the `…Northbound` / `*_northbound` high-level service methods. The old `IotCore` / `IoTCore` public names are not retained as aliases.
- **Library-owned QoS enum** on the Java public API instead of the Greengrass-SDK `QOS`, so a non-Greengrass northbound doesn't drag in a Greengrass type (Rust/TS already have their own `Qos`; see also the QoS-levels work in issue #26).
- **Preserve** `connected()` local-only readiness semantics.

### Channel selection (how a message picks a channel)
Generalize the existing per-target `destination` selector to a uniform **`{ local, northbound, stream:<name> }`** channel address:
- Framework emitters (metrics, heartbeat) already take `destination` — extend to `northbound` and `stream:<name>`.
- Component business logic picks a channel by the method/accessor it calls (`publish` vs `publishNorthbound` vs `streams().stream(name).append`).
- For **config-driven components** (the adapters), add an optional **`publish.channel`** alongside the existing `publish.topic`, valued `local` (default) / `northbound` / `stream:<name>`, settable globally and overridable per subscription/poll-group/tag.

### Platform-profile defaults
`--transport` (IPC|MQTT) stays the **channel-1** axis only. The northbound channel and streaming are **config-presence** concerns (like streaming today), *not* CLI axes — no new `--transport` value. If a per-platform northbound default is wanted (e.g. the IoT-Core bridge on GREENGRASS), add a `defaultNorthbound` field to the platform-profile record (it's designed to be extended additively).

### Compatibility boundary
- Old `publishToIoTCore(...)` / `*_to_iot_core(...)` callers must move to the `northbound` method family.
- Existing `messaging.iotCore` configs do **not** keep working; config hard-cuts to `messaging.northbound`.
- `destination: "iotcore"` / `"iot_core"` does **not** keep routing; config hard-cuts to `destination: "northbound"`.
- `connected()` stays local-only; a northbound/stream outage must not flip readiness.

## Proposed design — reference components

Both adapters today publish every tag to channel 1. The model lets an integrator split a device's tags across channels **by config**, which is exactly the OT pattern (bulk process data ≠ alarms/commands):

- **`publish.channel`** (global + per-subscription/poll-group/tag): route high-rate process tags to **`stream:<name>`** (channel 3), and alarms/status/command replies to **`northbound`** or **`local`** (channels 2/1).
- The adapters open a `streaming` section when any signal routes to a stream, and in the per-signal publish path call `gg.streams().stream(name).append(partitionKey = signalId, ts, payloadBytes)` for `stream:`-routed signals instead of `publish(topic, msg)`. The stable `signal.id` is a natural partition key; the existing `samples[]`/`batchMs` batching seam is where the split happens.
- This realizes the data-plane/control-plane split the adapters already approximate *by topic on one channel*, promoting it to *by channel*. `docs/SOUTHBOUND.md` already anticipates this (the deferred `gg.devices()` Tier-3 seam is described as "northbound publishing — symmetric with the northbound messaging transport abstraction").

## Phasing

- **Phase A — minimal (current):** hard-cut config and public messaging APIs from "IoT Core" → "northbound" — `messaging.northbound` config, `"northbound"` in the `destination` enum, northbound method families in all four languages, and the generalized non-mTLS northbound connect. No new subsystem, no CLI change. Channel 3 already exists. This delivers the conceptual model and a usable generic northbound with low risk.
- **Phase B — fuller generalization:** `messaging.northbound[]` (multiple brokers), a unified `Destination`/`Channel` parameter converging Java/Python onto the Rust/TS enum model, the library-owned QoS enum, a per-platform northbound default, and the adapter `publish.channel` routing (channels 2/3 from config).
- **Phase C — symmetric southbound seam:** fold this into the deferred `gg.devices()` Tier-3 work in `docs/SOUTHBOUND.md` so adapters get a uniform northbound-publish abstraction across channels.

## Decisions for review

1. **Scope:** Phase A only for now, or commit to A→B?
2. **Northbound cardinality:** single `messaging.northbound` (simple) vs `messaging.northbound[]` (multi-broker)?
3. **API shape for Java/Python:** add `…Northbound` method family (matches the current style) **or** introduce a `Destination`/`Channel` parameter (matches Rust/TS, better long-term, bigger four-way change)?
4. **QoS enum:** replace the Greengrass-SDK `QOS` on the Java API with a library enum now (couples well with issue #26) or defer?
5. **Adapter routing:** add `publish.channel` to the southbound contract (`docs/SOUTHBOUND.md`) now, or after Phase A lands in the library?

Once a direction is chosen this becomes an implementation plan with the per-language touch-points already mapped (see the file list in the analysis).
