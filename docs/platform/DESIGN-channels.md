# DESIGN — the three-channel messaging/telemetry model

> Status: **proposal for review** (2026-06-29). Code-grounded against the current Java/Python/Rust/TS
> messaging + streaming subsystems and both reference adapters. Nothing here is implemented yet; it
> defines the target model and the touch-points so the change can be scoped and phased.

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
| **2** | **Northbound control plane** | **always MQTT**, may be IoT Core | lower-throughput, lower-QoS commands/status/alarms | `publishToIotCore`/… — **hardwired to IoT Core** in names, config, and TLS |
| **3** | **Northbound streaming** | native durable buffer → Kinesis/Kafka | high-throughput, high-durability telemetry | `gg.streams()` — a fully independent subsystem |

The skeleton of all three already exists. The work is (a) **decoupling channel 2 from "IoT Core"** into a generic northbound MQTT broker, and (b) **positioning channel 3 as a first-class peer** with a clean per-tag routing story so components can split traffic across 2 and 3.

## Current state (grounded)

- **Channel 1 is already generic.** In all four languages the "local" path is an ordinary MQTT client (Java `StandaloneMessagingProvider` `tcp://`/`ssl://` host:port with plain / user-pass / server-TLS / mutual-TLS keyed on `caPath`; Rust `rumqttc`, TS `mqtt.js`, Python paho). On Greengrass it's Nucleus IPC.
- **Channel 2 is the same generic MQTT client, but artificially IoT-Core-shaped.** The northbound (`iotCore`) broker is the *same* dual-MQTT client distinguished by a role tag, but it is hardwired `ssl://`-only, **mandates** mutual TLS (refuses to connect otherwise), applies IoT-Core-flavored connect options, and is named "IoT Core" throughout the API (`publishToIoTCore`, `getNativeIotCoreClient`, …), the config (`messaging.iotCore`, field `endpoint` vs local's `host`), and the schema. The QoS type is even imported from the **Greengrass SDK** (`software.amazon.awssdk.aws.greengrass.model.QOS`) on the standalone path. None of this is *topic* semantics — there's no `$aws/...` reserved-topic logic — it's purely connection/auth/naming.
- **Channel 3 is clean and disjoint.** Streaming shares no code, transport, envelope, or topic with messaging. Records are opaque (`append(partitionKey, timestampMs, byte[])` to a named sink stream), it's opt-in (only when a top-level `streaming` section exists), and a component can already configure `messaging` **and** `streaming` together.
- **A channel selector already exists** for framework-emitted messages: metrics/heartbeat use a `destination` string (`ipc`/`local` vs `iotcore`/`iot_core`) to pick channel 1 vs 2.
- **Readiness is already channel-aware:** `connected()` reports only the *local* broker — a dropped cloud link must not flip `/readyz`. (Preserve this.)
- **The reference adapters use channel 1 only.** Both OPC UA and Modbus publish via local `publish()` with config-driven topic templates (`southbound/{ComponentName}/{InstanceId}/{tagId}`) and per-tag topic overrides; neither calls IoT Core (their recipes grant the IPC IoT-Core policy but the code never uses it) and neither streams. Their per-tag batching buffer (`samples[]` + `batchMs`) is the natural seam to divert high-rate tags to channel 3.

## Proposed design — library level

### Config schema (`schema/ggcommons-config-schema.json` → `sync-schema`)
- **Add `messaging.northbound`** (a `$ref` to the existing generic `mqttBroker` definition). Keep **`messaging.iotCore` as a back-compat alias** — loaders treat `iotCore` as `northbound` when `northbound` is absent. (Fuller option: `messaging.northbound[]` as an array, for multiple northbound brokers — e.g. cloud + on-prem historian.)
- **Generalize `mqttBroker`**: it's already generic (`host`/`endpoint`/`port`/`clientId`/`credentials`). Accept `host` **or** `endpoint` interchangeably, and document that the northbound broker supports the same auth options as local (plaintext / username-password / server-TLS / mutual-TLS) — i.e. **drop the IoT-Core-only "must be `ssl://` + mutual TLS" assumption**; key TLS on `caPath` exactly like local.
- **Extend the `destination` enum** in `metricEmission.targetConfig.destination` and `heartbeat…destination` to include `"northbound"` (alias of `iotcore`/`iot_core`).
- **`streaming`** needs no schema change for the model — optionally document it as "channel 3."

### Messaging API / provider
- **Generic northbound connect.** Build the northbound MQTT client from the *same* connect path as local (scheme from TLS presence, optional auth, TLS keyed on `caPath`) instead of the IoT-Core-hardwired path. IoT Core then becomes "a northbound broker that happens to require mutual TLS to an `ssl://` endpoint" — a config, not a code branch.
- **Java / Python (no service interface):** add a `…Northbound` method family (`publishNorthbound`, `subscribeNorthbound`, `requestNorthbound`, `replyNorthbound`, …) and make the existing `…ToIotCore`/`*_to_iot_core` methods **thin deprecated delegators**. This keeps the canonical surface generic without breaking callers.
- **Rust / TS (destination-enum seam — almost there):** add `Destination::Northbound` (keep `IotCore` as a deprecated alias), and add `…Northbound` convenience methods on the high-level service that route through the already destination-agnostic provider. Minimal change.
- **Library-owned QoS enum** on the Java public API instead of the Greengrass-SDK `QOS`, so a non-Greengrass northbound doesn't drag in a Greengrass type (Rust/TS already have their own `Qos`; see also the QoS-levels work in issue #26).
- **Preserve** `connected()` local-only readiness semantics.

### Channel selection (how a message picks a channel)
Generalize the existing per-target `destination` selector to a uniform **`{ local, northbound, stream:<name> }`** channel address:
- Framework emitters (metrics, heartbeat) already take `destination` — extend to `northbound` and `stream:<name>`.
- Component business logic picks a channel by the method/accessor it calls (`publish` vs `publishNorthbound` vs `streams().stream(name).append`).
- For **config-driven components** (the adapters), add an optional **`publish.channel`** alongside the existing `publish.topic`, valued `local` (default) / `northbound` / `stream:<name>`, settable globally and overridable per subscription/poll-group/tag.

### Platform-profile defaults
`--transport` (IPC|MQTT) stays the **channel-1** axis only. The northbound channel and streaming are **config-presence** concerns (like streaming today), *not* CLI axes — no new `--transport` value. If a per-platform northbound default is wanted (e.g. the IoT-Core bridge on GREENGRASS), add a `defaultNorthbound` field to the platform-profile record (it's designed to be extended additively).

### Backward compatibility (hard requirements)
- Existing `messaging.iotCore` configs keep working (alias for `northbound`).
- Existing `publishToIotCore(...)` / `*_to_iot_core(...)` callers keep working (deprecated delegators).
- `destination: "iotcore"/"iot_core"` keeps routing to the northbound broker.
- `connected()` stays local-only; a northbound/stream outage must not flip readiness.

## Proposed design — reference components

Both adapters today publish every tag to channel 1. The model lets an integrator split a device's tags across channels **by config**, which is exactly the OT pattern (bulk process data ≠ alarms/commands):

- **`publish.channel`** (global + per-subscription/poll-group/tag): route high-rate process tags to **`stream:<name>`** (channel 3), and alarms/status/command replies to **`northbound`** or **`local`** (channels 2/1).
- The adapters open a `streaming` section when any tag routes to a stream, and in the per-tag publish path call `gg.streams().stream(name).append(partitionKey = tagId, ts, payloadBytes)` for `stream:`-routed tags instead of `publish(topic, msg)`. The stable `tag.id` is a natural partition key; the existing `samples[]`/`batchMs` batching seam is where the split happens.
- This realizes the data-plane/control-plane split the adapters already approximate *by topic on one channel*, promoting it to *by channel*. `docs/SOUTHBOUND.md` already anticipates this (the deferred `gg.devices()` Tier-3 seam is described as "northbound publishing — symmetric with the northbound messaging transport abstraction").

## Phasing

- **Phase A — minimal (recommended first):** rename "IoT Core" → "northbound" as an **alias layer** — `messaging.northbound` config (with `iotCore` alias), `…Northbound` methods (with `…ToIotCore` delegators), `"northbound"` in the `destination` enum, and the generalized non-mTLS northbound connect. No new subsystem, no CLI change. Channel 3 already exists. This delivers the conceptual model and a usable generic northbound with low risk.
- **Phase B — fuller generalization:** `messaging.northbound[]` (multiple brokers), a unified `Destination`/`Channel` parameter converging Java/Python onto the Rust/TS enum model, the library-owned QoS enum, a per-platform northbound default, and the adapter `publish.channel` routing (channels 2/3 from config).
- **Phase C — symmetric southbound seam:** fold this into the deferred `gg.devices()` Tier-3 work in `docs/SOUTHBOUND.md` so adapters get a uniform northbound-publish abstraction across channels.

## Decisions for review

1. **Scope:** Phase A only for now, or commit to A→B?
2. **Northbound cardinality:** single `messaging.northbound` (simple) vs `messaging.northbound[]` (multi-broker)?
3. **API shape for Java/Python:** add `…Northbound` method family (matches the current style) **or** introduce a `Destination`/`Channel` parameter (matches Rust/TS, better long-term, bigger four-way change)?
4. **QoS enum:** replace the Greengrass-SDK `QOS` on the Java API with a library enum now (couples well with issue #26) or defer?
5. **Adapter routing:** add `publish.channel` to the southbound contract (`docs/SOUTHBOUND.md`) now, or after Phase A lands in the library?

Once a direction is chosen this becomes an implementation plan with the per-language touch-points already mapped (see the file list in the analysis).
