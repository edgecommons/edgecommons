# MTConnect adapter design

Status: **proposed design; no implementation exists**

Research snapshot: **2026-07-27**

Selected language: **Rust**

Protocol strategy: **owned thin MTConnect REST/streaming client on stock HTTP + XML libraries; no
third-party MTConnect runtime dependency exists in any EdgeCommons language**

This document is subordinate to the [shared adapter contract](README.md). A future repository copies
the decisions into its generated root `DESIGN.md`.

## 1. What it is — and, per mandate, what it is not

`com.mbreissi.edgecommons.MtconnectAdapter` is a southbound **MTConnect client**. It connects to one
or more running MTConnect **Agents** over HTTP, reads each agent's device model (`/probe`), acquires
observations by streaming (`/sample?interval=…`) with polling fallback (`/current`), and publishes
normalized EdgeCommons signals.

The role boundary is a design mandate, not an implementation shortcut:

- It is **not an MTConnect Agent**. It serves no HTTP endpoints, keeps no sequence buffer for
  downstream MTConnect clients, and does not replicate the canonical
  [cppagent](https://github.com/mtconnect/cppagent) (Apache-2.0, v2.7.0.12) in any part.
- It is **not an MTConnect Adapter** in the standard's sense: it ingests no SHDR, and it never sits
  between a device and an agent.
- A deployment that has machine tools but no agent installs the canonical agent (official Docker
  image `mtconnect/agent`) next to them; this component then consumes it. The agent is upstream
  infrastructure, exactly as a Kepware server is for the OPC UA adapter.

One `component.instances[]` entry represents one MTConnect **device** (a `Device` element with its
`uuid`) served by a configured agent. Multiple device instances on the same agent share one
component-owned agent runtime and one observation stream, invisibly below the per-instance seam
(ADP-3).

## 2. Release scope

Release 1 supports:

- MTConnect over HTTP/1.1 and HTTPS, GET only;
- agents speaking MTConnect **1.3 through 2.7** XML (namespace-version tolerant parsing; the schema
  floor is validated per agent at probe);
- `/probe` device-model retrieval, cached with a content digest;
- `/current` snapshots and `/sample` windowed reads;
- **streaming acquisition**: `interval`-driven multipart stream with per-agent heartbeat
  supervision, accepting **both** `multipart/x-mixed-replace` (the standard's content-type) and
  `multipart/mixed` (what cppagent 2.7.0.12 actually sends — verified live);
- full sequence-integrity handling: `instanceId` change → full resync; `from` outside
  `firstSequence..lastSequence` → consume the `MTConnectErrors` `OUT_OF_RANGE` document, take a
  `/current` snapshot, resume from `nextSequence`, and emit a data-loss event;
- observation categories **Samples**, **Events**, and **Condition**;
- `UNAVAILABLE` and condition-state quality mapping;
- TLS and HTTP Basic/bearer authentication with secrets via EdgeCommons credentials.

Explicit Release 1 non-goals:

- serving MTConnect (agent role), SHDR ingestion (adapter role), or any buffer/republish behavior;
- **writes of any kind** — the protocol's API is read-only by specification ("read-only and does
  not produce any side effects", Part 1 Fundamentals v2.5.0 §5.1; only GET is mandatory).
  cppagent's non-standard `AllowPut` test extension is deliberately unsupported;
- Assets (`/asset`, CuttingTool et al.) — a bounded read surface is a follow-on capability, not a
  silent R1 extension;
- the JSON representation (2.x optional) — XML is the interoperable floor; JSON is a follow-on;
- MQTT sink consumption (cppagent 2.x can publish MQTT; consuming that is a different transport
  design and does not enter this adapter by changing an enum);
- the Interfaces interaction model (Part 5) — device-to-device coordination, not a client surface;
- network discovery: MTConnect defines none; agents are configured URLs. `sb/discover` is not
  registered (as with Siemens S7).

## 3. Language and library assessment

The [assessment method](protocol-library-assessment.md#1-selection-method) applies. All findings
verified live on 2026-07-27.

| Candidate | Access/license | Role and coverage | Finding | Disposition |
|---|---|---|---|---|
| cppagent (mtconnect/cppagent) v2.7.0.12 | Public, Apache-2.0; official Docker image `mtconnect/agent`; in-tree SHDR simulator + Mazak/OKUMA demo | **Agent**, not a client | The canonical reference implementation; actively maintained (pushed 2026-07-21) | **H3-fail as an engine** (wrong role by mandate); **the primary independent test peer** — free, canonical, containerized |
| MTConnect.NET (TrakHound) v6.9.0 | Public, MIT, active (2026-07-23) | Complete .NET client+agent SDK | Strongest client implementation anywhere, but .NET is not an EdgeCommons language | **Reference only** (design/model reading; never a runtime dependency) |
| PyPI `mtconnect` 0.3.3 | Public, Apache-2.0 | **Agent** implementation | Last upload 2023-01-11; dead; wrong role | **Reject** at H3/H4 |
| npm `@mx-interface/mtconnect-ts` 1.0.5 | Public, 2026-07-02; **no license declared** in the manifest | Young TS client | Single release, undeclared license (H2-fail as-is), incongruous dependency set | **Reject** for R1; reassess if it matures and licenses |
| crates.io | — | — | Zero `mtconnect` crates exist (verified via API) | Greenfield |
| Java | — | — | No maintained client (`mtconnect/java_sdk` dead since 2013; the only Maven hit is a HiveMQ Edge module, not a client) | Greenfield |
| Owned thin client | EdgeCommons-owned | Exactly the R1 client boundary | HTTP GET + multipart reading + namespace-tolerant XML on stock libraries; the standard and all XSDs are **free** (mtconnect.org downloads; `mtconnect/schema` on GitHub, XSDs through 2.7) | **Adopt** — effort **S** (2–4 engineer-months including qualification) |

**Rust is selected by the assessment's own tie-break rule** (§1.2: "Rust wins a tie"): with no
qualifying library in any language, every language would build the same thin owned client, and no
other comparison dimension separates them. The client rides reviewed ecosystem crates
(`reqwest`/`hyper`, `quick-xml`) — this is an S-effort client, not a PROFINET-class XL protocol
engine, because the agent already did the hard real-time work.

Unlike Siemens S7 (reverse-engineered protocol, common-ancestry emulators), MTConnect's validation
peer **is the canonical implementation itself**, free and containerized — the strongest
zero-procurement verification story of any adapter in this set.

## 4. CLI initialization

Use the shared contract's Rust scaffold command ([ADP-1](README.md#adp-1-cli-scaffolding-is-mandatory))
with `--name com.mbreissi.edgecommons.MtconnectAdapter --language RUST --dir mtconnect-adapter
--bin-name mtconnect-adapter`. The generated structure is the floor (ADP-2). The owned client lives
in an `mtconnect/` module set (or a small workspace crate) with **no EdgeCommons imports**:
`client.rs` (HTTP/stream), `model.rs` (probe tree), `observations.rs` (streams parsing),
`sequence.rs` (buffer/recovery state), behind the generated `DeviceBackend`/`DeviceSession` seam.

Unlike the three earlier designs, this one scaffolds against the **post-conformance** templates
(core PR #78): renderable panels, the hierarchical `sb/browse` mode, `#/$defs/instance`, the
top-level `PAUSED` code, and the eight-measure `southbound_health` family are generated, not gaps
to re-implement.

## 5. Architecture

### 5.1 Shared agent runtimes, per-device instances

`component.global.agents[]` declares each agent once (mirroring PROFINET's `controllerRuntimes`):

- `id` (lower-kebab, stable), `url` (base, http/https), auth/TLS references, request timeouts,
  `heartbeatMs` (default 10000, the standard's default), `streaming` policy
  (`prefer` | `poll-only`), `pollIntervalMs` fallback, and reconnect bounds.

The agent runtime owns: one HTTP connection pool, the probe cache per device, **one** multipart
observation stream covering all of that agent's configured devices (server-side filtered with an
XPath `path=` query when beneficial; demultiplexed per instance by `dataItemId`), heartbeat
supervision, sequence/`instanceId` state, and shutdown. Failure of one device's observations never
tears down another device's session state; loss of the agent connection transitions every leased
instance with per-instance reconnect visibility (ADP-3).

Each instance (`connection: {agentId, deviceUuid}`) verifies at connect that the probe contains its
`deviceUuid`, compiles its configured signal set against the device's data items, and then serves
the common supervisor loop. `connectionState = 1` means: agent reachable, probe verified, and the
stream (or poll loop) delivering within the heartbeat/staleness window.

`endpoint_description()` is `mtconnect://<host>[:<port>]/<percent-encoded-device-uuid>` — derived,
non-secret, and used unchanged everywhere (ADP-4).

### 5.2 Acquisition and sequence integrity

Streaming mode (default `prefer`): `GET {base}/sample?interval={ms}&heartbeat={ms}&from={next}`
with the multipart reader accepting both verified content-types. Every received document updates
`nextSequence`; an **empty heartbeat document** proves liveness without data. Recovery ladder:

1. heartbeat missed → reconnect the stream from `nextSequence`;
2. `OUT_OF_RANGE` error document (buffer overran our position) → emit `MtconnectDataLossEvent`,
   `/current` snapshot (republish as fresh values), resume from the snapshot's `nextSequence`;
3. `instanceId` changed (agent restarted) → full resync: re-probe, re-verify the model digest,
   snapshot, resume; a probe-model change invalidates browse cursors and raises a config-drift
   event rather than silently remapping signals.

Poll mode: `/current` at `pollIntervalMs` with the same snapshot semantics. `sb/read` always uses
`/current` (optionally `path=`-scoped); `repoll` forces a snapshot publish and is refused with
top-level `PAUSED` while paused.

### 5.3 Observation → signal mapping

Configured signals bind by **`dataItemId`** within the instance's device (the standard requires
per-device uniqueness); `signal.id` remains the configured stable EdgeCommons identity. Published
`signal.address` is round-trippable:

```json
{
  "protocol": "mtconnect",
  "agentId": "line-a-agent",
  "deviceUuid": "OKUMA.123456",
  "dataItemId": "Xabs",
  "category": "SAMPLE",
  "type": "POSITION",
  "subType": "ACTUAL",
  "componentPath": "Axes/Linear[X]"
}
```

- **Samples** → JSON numbers (3D/`TimeSeries` representations → JSON arrays; `resetTriggered`
  and `duration` ride as per-sample extras).
- **Events** → strings/enums verbatim; numeric event types as numbers.
- **Condition** → the condition state (`Normal`/`Warning`/`Fault`/`Unavailable`) as the value,
  with `nativeCode`/text in extras; quality per §6.
- **`UNAVAILABLE`** → a `BAD` sample with `value: null` and `qualityRaw: "UNAVAILABLE"` — the §2
  SOUTHBOUND explicit-null rule does not apply (this is a *bad* null, not a good one).
- Per-sample **`sequence`** always rides as an extra field (the shipped `Sample.extra` path),
  giving consumers exact once-only ordering across reconnects.

Timestamps, per the SOUTHBOUND four-slot model: the observation timestamp is the **agent's
capture stamp** — published as `serverTs`. `sourceTs` is absent (MTConnect does not distinguish a
device-authored time from the agent/adapter capture chain). The adapter's own receive time rides
the per-sample `receivedTs` extra — MTConnect is a mediated protocol, so capture and receive
genuinely differ and both are published.

## 6. Quality mapping

| Condition | Quality | `qualityRaw` |
|---|---|---|
| Observation delivered, value not `UNAVAILABLE`, no `Fault` condition bound to the signal's config | `GOOD` | `MTC_OK` (+ condition state for condition signals) |
| Value held past one missed heartbeat/poll but before `staleSignalSecs`; or bound condition `Warning` | `UNCERTAIN` | `MTC_STALE:<ageMs>` / `MTC_CONDITION:WARNING:<nativeCode>` |
| `UNAVAILABLE`, bound condition `Fault`, agent unreachable, parse failure, or staleness expiry | `BAD` | `UNAVAILABLE` / `MTC_CONDITION:FAULT:<nativeCode>` / `MTC_PARSE:<code>` |

A signal MAY declare `conditionBinding: ["<conditionDataItemId>", …]` to degrade its quality from
named condition data items; unbound conditions affect only their own signals.

## 7. Commands (ADP-6 mapping)

| Verb | MTConnect behavior |
|---|---|
| `sb/status` | Agent URL (non-secret), MTConnect/schema version, `instanceId`, buffer/sequence header fields, stream vs poll mode, heartbeat state, probe digest, capability limitations. |
| `sb/signals` | Configured inventory with the §5.3 address; no network I/O. |
| `sb/browse` | The **probe tree** (Device → Component → DataItem), paged and hierarchical, served from the cached probe with its digest as `viewGeneration` — available while disconnected after first probe; entries flag which are configured. |
| `sb/read` | Bounded `/current` read of configured signal refs with per-entry results, `"mode":"current"`. |
| `sb/write` | **Registered, always refused**: every request returns top-level `WRITE_NOT_ALLOWED` with a message naming the standard's read-only mandate. The instance schema pins `writes.allow` to the empty array (`maxItems: 0`). |
| `sb/pause` / `sb/resume` | Pause suppresses publication and (in stream mode) keeps the stream draining with the latest-value cache updating; resume snapshots then resumes normal flow. |
| `reconnect` | Drop and re-establish the agent stream/session for this device; full resync ladder. |
| `repoll` | Snapshot `/current` and publish (`polled` = published signal results incl. `BAD`); refused with `PAUSED` while paused. |

`sb/status.result.protocol` (closed): `{capability: "MTCONNECT_CLIENT", standardVersion,
schemaNamespace, agentId, agentVersion, instanceId, bufferSize, firstSequence, nextSequence,
mode: "stream"|"poll", heartbeatMs, lastHeartbeatAt, probeDigest: "sha256:<hex>",
limitations: ["READ_ONLY", "XML_ONLY", "NO_ASSETS"]}` — nullable until learned. Per-entry read
failure codes: `MTC_UNAVAILABLE`, `MTC_NO_SUCH_DATAITEM`, `MTC_PARSE`, `MTC_AGENT_ERROR:<code>`.

## 8. Edge-console panels

The [shared panel contract](edge-console-panels.md) is normative, and — unlike the three earlier
designs — its renderer capabilities are **shipped** (edge-console `feat/descriptor-renderer-v2`):
these are current bindings, not a joint-release target. Views (no discovery view — MTConnect has no
network discovery):

| Order/id/title | Widgets |
|---|---|
| 10 `overview` Overview | `statusDashboard` → `sb/status` (Adapter state, Connected, Paused, Endpoint, Agent version, Standard version, Mode badge, Instance ID, Next sequence, Heartbeat age, Probe digest); `actionBar` → `sb/pause`/`sb/resume`/`reconnect`/`repoll`; `metricSeries` |
| 20 `device-structure` Device Structure | generic `treeBrowser` → `sb/browse` with columns Name/Kind/Type/SubType/Category/DataItem/Configured; read badge → `sb/read` |
| 30 `signals` Signals | generic `signalGrid` → `signalsVerb`+`subscriptionsVerb`=`sb/signals`, `readVerb`; columns Signal/Name/DataItem/Category/Type/Units/Quality binding |
| 40 `conditions` Conditions & Events | `eventFeed` (families `MtconnectConditionEvent`, `MtconnectDataLossEvent`, `MtconnectAgentEvent`); `metricSeries` |
| 50 `diagnostics` Diagnostics | `statusDashboard`; `eventFeed`; `metricSeries` (stream gaps, reconnects, heartbeat misses, parse errors) |

Every view declares its `rendererRequirements` tokens from the shared contract's closed set. No
view advertises `writeVerb` (nothing to write). `sb/write`'s permanent refusal is additionally
advertised through the shipped command-availability surface:
`setCommandAvailability("sb/write", "unsupported", "MTConnect is read-only")` — the first adapter
to use it.

## 9. Metrics and events

Eight-measure `southbound_health` per the amended SOUTHBOUND §5; `signalsSubscribed` = configured
data items currently delivered by the active stream/poll set; `writeErrors` is structurally 0
(no device write path) and stays for fleet uniformity. Additional families (low-cardinality):

| Family | Dimensions | Measures |
|---|---|---|
| `MtconnectStream` | `agentId`, `result` | `documents`, `observations`, `heartbeats`, `reconnects`, `gaps`, `outOfRange`, `latencyMs` |
| `MtconnectProbe` | `agentId`, `result` | `probes`, `modelChanges`, `latencyMs` |
| `MtconnectParse` | `instance`, `result` | `documentsParsed`, `parseErrors` |

Events: agent connect/disconnect, `instanceId` change, data loss (`OUT_OF_RANGE`), probe-model
drift, condition Fault transitions (rate-limited), config rejection. Sequence numbers, UUIDs, and
data-item ids are event fields, never metric dimensions.

## 10. Security and platform behavior

MTConnect over plain HTTP is unauthenticated and unencrypted; deployments should prefer the
agent's TLS listener and restrict routes to configured agents. Credentials (basic/bearer/TLS
material) resolve through EdgeCommons credentials and never appear in config, logs, or status.
All platforms (HOST Windows/Linux, GREENGRASS, KUBERNETES) use ordinary outbound TCP — no raw
sockets, no host networking, no elevated capabilities; the generated packs apply unchanged. Live
platform claims still require the standard per-platform gates (ADP-9).

## 11. Validation gates (all $0 — the cheapest full-evidence adapter in this set)

1. **Codec/parse gate**: golden documents for Devices/Streams/Errors across namespace versions
   1.3→2.7 (free XSDs from `mtconnect/schema`), malformed/truncated/fuzzed XML, both multipart
   content-types, heartbeat documents, `OUT_OF_RANGE` errors.
2. **Sequence gate**: virtual-clock tests for heartbeat miss, stream reconnect, buffer overrun
   recovery, `instanceId` resync, snapshot/dedupe correctness (no lost, no duplicated sequence).
3. **Simulator/peer gate**: the generated in-process simulator for EdgeCommons lifecycle; the
   **canonical cppagent** (`mtconnect/agent` Docker image, v2.7-pinned) with its in-tree SHDR
   simulator and Mazak/OKUMA demo devices as the independent peer — including agent restart,
   buffer-wrap (small `bufferSize`), and multi-device streams; the live public
   `demo.mtconnect.org` (2.7.0.12) as an internet soak/compat check (advisory, not CI-gating).
   The NIST SMS testbed is **not** cited: verified unreachable (DNS-dead) on 2026-07-27.
4. **Wire gate**: exact envelopes/quality/extras (`sequence`) over local MQTT; `sb/*` replies;
   `WRITE_NOT_ALLOWED` and `PAUSED` behavior.
5. **Panel gate**: all five views against the shipped renderer, instance selection, offline probe
   browse, unsupported-write availability state.
6. **Platform gates**: HOST, Greengrass lab, Kubernetes, per the org matrix.
7. **Real-agent gate**: at least one third-party (non-cppagent) agent implementation or a vendor
   machine-tool agent when access arises — recorded as a deferred qualification (same explicit
   re-scope pattern as the S7/BACnet zero-budget decisions), NOT a release blocker, because the
   canonical implementation itself anchors conformance.

90% line coverage for the component and the owned client; Dallas-harness integration is feasible
(cppagent container beside the existing sims) and recommended as the system E2E vehicle.

## 12. Decision register

- **D-MTC-1:** The adapter is an MTConnect **client** of one or more agents; agent and SHDR-adapter
  roles are permanently out of scope (the founding mandate).
- **D-MTC-2:** Rust, owned thin client on stock HTTP/XML crates; no third-party MTConnect runtime
  dependency exists in any EdgeCommons language (ledger verified 2026-07-27). Effort S.
- **D-MTC-3:** One instance = one MTConnect device (`uuid`); agents are shared runtimes under
  `component.global.agents[]` with one demultiplexed stream per agent.
- **D-MTC-4:** Streaming is the primary acquisition (`interval` multipart, both observed
  content-types, heartbeat-supervised) with `/current` polling fallback and the three-step
  resync ladder (reconnect → OUT_OF_RANGE snapshot recovery → instanceId full resync).
- **D-MTC-5:** Signals bind by `dataItemId`; the probe tree backs `sb/browse` from cache;
  probe-model drift is surfaced, never silently remapped.
- **D-MTC-6:** `UNAVAILABLE` is a BAD null; the observation timestamp is the agent capture stamp
  (`serverTs`), `sourceTs` is absent, adapter receive rides the `receivedTs` extra; `sequence`
  rides per-sample extras.
- **D-MTC-7:** `sb/write` is registered but permanently refused (`WRITE_NOT_ALLOWED`), schema-pinned
  empty allow-list, and advertised `unsupported` via command availability. No `sb/discover`.
- **D-MTC-8:** Conditions are signals; optional `conditionBinding` degrades bound signals' quality.
- **D-MTC-9:** cppagent is the canonical free test peer (Docker + SHDR simulator +
  `demo.mtconnect.org` soak); third-party-agent qualification is a recorded deferred gate, not a
  release blocker.
- **D-MTC-10:** Assets, JSON representation, and MQTT-sink consumption are explicit follow-on
  capabilities with their own designs.

## 13. Research references (verified live 2026-07-27)

- [MTConnect standard downloads — free, all versions](https://www.mtconnect.org/standard-download20181)
- [MTConnect model site — 2.7 current / 2.8 dev](https://model.mtconnect.org/)
- [Part 1 Fundamentals v2.5.0 PDF (read-only API §5.1; streaming §5.1.3; heartbeat §5.1.3.1.1)](https://docs.mtconnect.org/MBSD_MTConnect_Part_1_2-5-0.pdf)
- [XSD/JSON schemas 1.2–2.7](https://github.com/mtconnect/schema)
- [cppagent v2.7.0.12 (Apache-2.0; simulator/ + demo/)](https://github.com/mtconnect/cppagent/releases/tag/v2.7.0.12)
- [Official agent Docker image](https://hub.docker.com/r/mtconnect/agent)
- [Live public demo agent (2.7.0.12; /probe and /current verified)](https://demo.mtconnect.org/current)
- [MTConnect.NET v6.9.0 (MIT; reference only)](https://github.com/TrakHound/MTConnect.NET)
- Negative findings, same date: PyPI `mtconnect` 0.3.3 (dead, agent-role), npm
  `@mx-interface/mtconnect-ts` 1.0.5 (unlicensed manifest), zero crates.io results, no maintained
  Java client; NIST SMS testbed DNS-dead.
