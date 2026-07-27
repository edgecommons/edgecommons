# MTConnect adapter — low-level implementation design

Status: **proposed for review; no implementation exists**

Parent: [mtconnect-adapter.md](mtconnect-adapter.md) (the HLD; its decision register D-MTC-1..10 is
binding) and the [shared adapter contract](README.md). Target baseline: the post-conformance Rust
`protocol-adapter` template (core `feat/adapter-core-enablers`), whose generated seams this document
names explicitly. Language: Rust, edition 2024, tokio, `reqwest` (rustls) + `quick-xml`.

## 1. Repository layout (scaffold + owned modules)

`edgecommons component new … --language RUST --kind protocol-adapter` generates the floor
(`src/{main,app,supervisor,device,commands,metrics}.rs`, simulator, tests, packs, docs). The owned
client is an in-crate module tree — a thin client does not warrant a workspace crate (deviation
from PROFINET's crate split, justified by size; revisit if `mtconnect/` exceeds ~3 kLOC):

```text
src/
  app.rs  supervisor.rs  commands.rs  metrics.rs      # generated, extended in place
  device.rs                                           # generated seam, extended (§4)
  mtconnect/
    mod.rs          # pub facade: AgentRuntime, AgentHandle, types re-exports
    config.rs       # AgentConfig, DeviceConfig, SignalConfig (serde, validated)
    client.rs       # HTTP: probe/current/sample one-shots; auth/TLS; redirects off
    stream.rs       # multipart stream reader + heartbeat supervision
    multipart.rs    # owned bounded multipart splitter (both content-types)
    xml.rs          # namespace-tolerant pull parsing: Devices/Streams/Errors docs
    model.rs        # ProbeModel tree + canonical digest + browse projection
    observations.rs # Observation, category/type decode, UNAVAILABLE handling
    sequence.rs     # SequenceState machine: stream/recover/resync/poll
    error.rs        # MtcError taxonomy → command codes / qualityRaw (§9)
```

**Isolation rule (enforced in review + a CI grep): `src/mtconnect/**` imports nothing from
`edgecommons`.** All EdgeCommons awareness lives in `device.rs`/`supervisor.rs`/`commands.rs`.

## 2. Core types (signatures are normative; field lists may grow additively)

```rust
// config.rs — component.global.agents[] entry
pub struct AgentConfig {
    pub id: String,                    // lower-kebab, unique
    pub url: Url,                      // base, http/https only, no userinfo
    pub auth: Option<AuthRef>,         // Basic{user, secret_ref} | Bearer{secret_ref}
    pub tls: Option<TlsRef>,           // ca/cert/key refs via credentials
    pub heartbeat_ms: u32,             // default 10_000 (standard default)
    pub streaming: StreamPolicy,       // Prefer | PollOnly
    pub poll_interval_ms: u32,         // default 1_000 (fallback + PollOnly)
    pub request_timeout_ms: u32,       // default 10_000 (one-shots)
    pub reconnect: ReconnectCfg,       // initial_ms/max_ms, full jitter
}

pub struct SignalConfig {
    pub id: String,                    // stable EdgeCommons id (lower-kebab)
    pub name: Option<String>,
    pub channel: Option<String>,       // signal_path override
    pub data_item_id: String,          // binding key within the device
    pub condition_binding: Vec<String>,// condition dataItemIds degrading this signal
    pub publish: PublishCfg,           // on-change | interval, batch_ms, deadband(Samples)
}

// model.rs — probe projection (built once per probe, immutable, Arc-shared)
pub struct ProbeModel {
    pub raw_digest: [u8; 32],          // sha256 over the canonicalized Device subtree
    pub device: DeviceNode,            // uuid, name, mtconnect version floor observed
    pub items: HashMap<String, DataItemMeta>, // dataItemId → meta
    pub tree: Vec<BrowseNode>,         // pre-ordered browse projection (§7)
}
pub struct DataItemMeta { pub id: String, pub category: Category, // SAMPLE|EVENT|CONDITION
    pub type_: String, pub sub_type: Option<String>, pub units: Option<String>,
    pub native_units: Option<String>, pub component_path: String, pub representation: Repr }

// observations.rs — one parsed observation from a Streams document
pub struct Observation {
    pub data_item_id: String,
    pub sequence: u64,
    pub timestamp: String,             // RFC3339 as sent by the agent (→ sourceTs)
    pub value: ObsValue,               // Scalar(Value) | Unavailable | Condition(CondState)
    pub extras: SmallVec<(­&'static str, Value)>, // resetTriggered, duration, nativeCode, …
}

// sequence.rs
pub enum AcqState { Connecting, Streaming { next: u64 }, Recovering, Resyncing, Polling, Backoff }
pub struct SequenceState { pub instance_id: Option<u64>, pub next: u64,
    pub last_published: HashMap<String, u64> /* dataItemId → seq, dedupe floor */ }
```

## 3. Task and channel topology

One tokio task set per **agent** (the shared runtime), plus the template's generated per-instance
supervisor tasks. Ownership: `app.rs` builds `Arc<AgentRuntime>` per configured agent before
instance supervisors start (mirrors the template's construction order; readiness stays false until
handles install — ADP-3 activation ordering is generated).

```text
AgentRuntime (per agent)
  ├─ acq_task: owns reqwest client + SequenceState; runs the §5 state machine
  ├─ tx: per-instance bounded mpsc<InstanceEvent> (cap 1024, latest-value coalescing
  │      for Scalar observations on overflow + drop counter; Condition/lifecycle
  │      events are loss-intolerant → send().await backpressure into acq_task)
  └─ ctl: mpsc<AgentCtl> (Reconnect, Pause(uuid), Resume(uuid), Snapshot(reply), Shutdown)

InstanceEvent = Obs(Observation) | Snapshot(Vec<Observation>) | AgentUp(AgentInfo)
             | AgentDown(reason) | DataLoss{skipped: u64} | ModelDrift{old, new_digest}
```

The generated `DeviceSession` for an instance is a **handle**: `{agent: Arc<AgentRuntime>,
uuid, model: ArcSwap<ProbeModel>, rx}` — it owns no socket (ADP-3/D-MTC-3). `read_now`/`repoll`
route through `ctl` as `Snapshot` requests scoped to the instance's dataItemIds; the acq_task
serializes them with streaming work (single-owner rule; command handlers never touch HTTP).

## 4. Device-seam extension (the generated `Reading` is lossy — extend it, per ADP/D-ADP-9)

The template's `Reading {signal_id, name, value, quality, quality_raw}` cannot carry `sourceTs`,
address, or extras. Extend **in place** (template file, adapter-owned after scaffold):

```rust
pub struct Reading {
    pub signal_id: String, pub name: Option<String>,
    pub value: Option<serde_json::Value>,     // None + explicit BAD for UNAVAILABLE
    pub quality: Quality, pub quality_raw: Option<String>,
    pub source_ts: Option<String>,            // observation timestamp (agent-relayed)
    pub extra: Option<serde_json::Map<String, Value>>, // sequence, resetTriggered, …
}
```

Publish path (in `device.rs`, replacing the template's `_publish_reading` equivalent): build
`Sample` via the core facade — `Sample::null_value()` **with `quality: Bad` and
`quality_raw: "UNAVAILABLE"`** for unavailable observations (the shipped facade gates only the
null *permission* on `explicit_null`; quality is free), ordinary `Sample::new(v)` otherwise; attach
`extra` entries (`sequence` always; `resetTriggered`/`duration`/`nativeCode` when present) via the
shipped `Sample::extra`; set `source_ts` verbatim. Simulator, scheduled publisher, `sb/read`, and
tests move together with this extension (contract rule: command reads must not stay lossy).

## 5. Acquisition state machine (`sequence.rs` + `stream.rs`)

```text
Connecting ──probe ok──▶ Snapshot(/current) ──▶ Streaming(next = header.nextSequence)
Streaming:
  GET {base}/sample?interval={i}&heartbeat={h}&from={next}[&path={xpath}]
  loop parts:
    Streams doc  → publish obs where seq ≥ from, seq > last_published[dataItemId];
                   next = doc.header.nextSequence
    empty doc    → heartbeat: refresh liveness deadline only
    Errors doc(OUT_OF_RANGE) → Recovering
  deadline = now + 2×heartbeat_ms missed → drop stream → Streaming (same next)   [ladder 1]
Recovering:  emit DataLoss{skipped = firstSequence.saturating_sub(next)};
             Snapshot(/current) → publish as fresh; next = snapshot.nextSequence
             → Streaming                                                          [ladder 2]
InstanceId change (any doc header) → Resyncing: re-probe; digest≠cached → ModelDrift event,
             browse cursors invalidated (viewGeneration=digest), signals recompiled against the
             new model (missing dataItemId → that signal → permanent BAD MTC_NO_SUCH_DATAITEM);
             then Snapshot → Streaming                                            [ladder 3]
Polling (StreamPolicy::PollOnly, or after N consecutive stream-establish failures with an event):
  /current every poll_interval_ms; same snapshot/dedupe rules; Streaming retried per reconnect cfg.
Backoff: capped exponential + full jitter (template's generated policy) on connect/probe failure.
```

Dedupe rule is per data item (`last_published`), not global — one stream serves many devices and
`/current` snapshots overlap the stream window. Pause (D-MTC-7/HLD §7): acq continues, cache
updates, per-instance publish gate closes; resume forces `Snapshot` first.

## 6. HTTP + multipart + XML details

- **client.rs**: one `reqwest::Client` per agent (rustls, pooled, no redirects, `Accept:
  application/xml`); auth header injected per request from resolved credentials; one-shots bounded
  by `request_timeout_ms`; response bodies size-capped (default 16 MiB, config `maxDocumentBytes`).
- **multipart.rs** (owned, ~150 LOC, fuzz target): accepts `multipart/x-mixed-replace` **and**
  `multipart/mixed` (HLD-verified cppagent deviation); boundary from the Content-Type param;
  incremental scan over the chunked byte stream; each part's own `Content-length` is trusted but
  capped (`maxDocumentBytes`); malformed part → `MtcError::Multipart` → stream drop → ladder 1.
  No crate dependency: streaming x-mixed-replace parsers are scarce/unmaintained (assessment), and
  the grammar is 2 headers + delimiter.
- **xml.rs**: `quick_xml::Reader` pull parsing, **matching local names only** and recording the
  namespace URI version (1.3–2.7 tolerance); three document parsers (`Devices`, `Streams`,
  `Errors`) that skip-and-count unknown elements (forward-compat, `MtconnectParse.unknownElements`
  measure); depth cap 64, attribute count/length caps; no DTD/entity resolution (quick-xml default;
  asserted by test with an XXE fixture). Header struct: `{instanceId, bufferSize, firstSequence,
  lastSequence, nextSequence, version, sender}`.
- **model.rs digest**: canonical serialization = pre-order (element local-name, sorted attributes)
  of the instance's `Device` subtree only → sha256 → `probeDigest`/browse `viewGeneration`.

## 7. Commands (mapping onto the generated `Commander`)

Generated routing (`body.instance`, `NO_SUCH_INSTANCE`/`BAD_ARGS`) is untouched. Per verb:

- `sb/status`: assemble the HLD §7 closed `protocol` object from `AgentRuntime` published state
  (an `ArcSwap<AgentInfo>` refreshed by acq_task — no ctl round-trip, never blocks).
- `sb/signals`: configured inventory + §5.3 address built from `SignalConfig` × `DataItemMeta`.
- `sb/browse`: the generated paged + hierarchical dual mode backed by `ProbeModel.tree`
  (pre-order: Device → Components → DataItems; ids `mtc:/component/<path>` and
  `mtc:/item/<dataItemId>`; entry extras: Kind/Type/SubType/Category/Configured flag). Cursor
  `viewGeneration = "sha256:" + probeDigest`; served from the cached model — works disconnected
  after first probe (cold start with no cache → `BROWSE_FAILED` with `MTC_NO_PROBE`).
- `sb/read`: resolve refs → scoped `Snapshot` via ctl (timeout `request_timeout_ms` + 2 s) →
  per-entry results `"mode":"current"`; agent down → per-entry BAD `MTC_AGENT_ERROR:UNREACHABLE`
  with top-level ok (item-vs-session rule) unless the runtime itself is absent → `DEVICE_UNAVAILABLE`.
- `sb/write`: **before** entry processing, unconditional top-level `WRITE_NOT_ALLOWED`
  ("MTConnect is read-only (Part 1 §5.1)"). Startup order (generated pre-activation window):
  register all verbs → `set_command_availability("sb/write", "unsupported", "MTConnect is
  read-only")` → register panels → activate. Schema pins `writes.allow: {maxItems: 0}`.
- `sb/pause`/`sb/resume`/`reconnect`/`repoll`: generated semantics; `repoll` = forced scoped
  snapshot publish, `polled` = published results incl. BAD; paused → top-level `PAUSED` (generated).

## 8. Config schema (`config.schema.json` deltas from the generated floor)

`component.global`: `agents[]` (closed AgentConfig shape; ≥1; unique ids/urls), `defaults`
(`publishMode`, `batchMs`, `staleSignalSecs`, `maxDocumentBytes`, reconnect). `#/$defs/device`
(aliased by the generated `#/$defs/instance`): `{id, adapter: "mtconnect", connection:
{agentId, deviceUuid}, signals: [SignalConfig…], writes: {allow: {type: array, maxItems: 0}}}` —
all objects `additionalProperties: false`. Cross-invariants in the semantic validator: every
`connection.agentId` resolves; device uuids unique per agent; `conditionBinding` ids distinct from
the signal's own `dataItemId`; deadband only on SAMPLE-category signals. Reload per ADP-4:
`agents[]` edits are `RESTART_REQUIRED` (they own live sockets/streams); instance/signal changes
ride the generated last-good swap (recompile against the cached model, cursors invalidated).

## 9. Error taxonomy (`error.rs`)

| `MtcError` | Source | Surfaces as |
|---|---|---|
| `Http{status}` / `Timeout` / `Tls` / `Auth` | client.rs | reconnect (transient); `sb/read` per-entry `MTC_AGENT_ERROR:<c>` |
| `Multipart(reason)` | multipart.rs | stream drop → ladder 1; `MtconnectParse.parseErrors` |
| `Xml(reason)` | xml.rs | doc dropped + counted; repeated (≥3 consecutive) → stream drop |
| `OutOfRange{first}` | Errors doc | ladder 2 + `MtconnectDataLossEvent` |
| `InstanceChanged` | header check | ladder 3 |
| `NoSuchDevice` / `NoSuchDataItem` | probe compile | permanent config failure / per-signal BAD `MTC_NO_SUCH_DATAITEM` |
| `AgentError{code}` | Errors doc (non-range) | per-entry `MTC_AGENT_ERROR:<code>`; event |

Permanent vs transient classification feeds the generated supervisor unchanged.

## 10. Metrics, events, health wiring

Generated `Health` gains nothing new structurally: `signalsSubscribed` = size of the instance's
compiled, currently-delivered signal set (stream or poll) while `connectionState==1`, else 0;
`writeErrors` stays 0 (asserted by test). New families (HLD §9) emit through the generated
family-pattern seam in `metrics.rs`: `MtconnectStream`/`MtconnectProbe`/`MtconnectParse` with
`(total, interval)` counter pairs, dimensions `agentId`/`instance`/`result` only. Events via the
generated event helper: `MtconnectAgentEvent` (up/down/instanceId), `MtconnectDataLossEvent`
(skipped count, sequence window), `MtconnectModelDriftEvent` (old/new digest),
`MtconnectConditionEvent` (Fault transitions, rate-limited 1/min per dataItemId).

## 11. Panels

Descriptors exactly per HLD §8, emitted from `commands.rs` beside the generated trio (replacing
their content, keeping registration order/ids where shared). Every view carries
`rendererRequirements`; grids/trees declare generic `columns` (shipped renderer). Snapshot test
pins the full manifest + absence of `writeVerb` + the `sb/write` availability state in `describe`.

## 12. Testing plan

| Layer | Vehicle | Key cases |
|---|---|---|
| multipart.rs | unit + `cargo fuzz` target | both content-types, split boundaries across chunks, oversize part, missing length, junk between parts |
| xml.rs | unit + fuzz + goldens | goldens per ns version 1.3/1.7/2.0/2.7 (free XSD-derived fixtures), XXE fixture inert, unknown-element skip counting, header extraction |
| sequence.rs | virtual-clock unit | ladder 1/2/3, dedupe overlap (snapshot ∩ stream), heartbeat expiry math, PollOnly, N-failure poll degradation |
| device seam | fake AgentRuntime | Reading extension, UNAVAILABLE null+BAD publish, extras (`sequence`) on the wire body, pause cache-update/no-publish |
| commands | generated harness + fake runtime | write refusal + availability, browse paged/hierarchical/cold-cache, read scoped snapshot, PAUSED |
| integration | `tests/agent_integration.rs`, env-gated `EC_MTC_AGENT` (compose file pinning `mtconnect/agent:2.7.0.12` + in-tree SHDR simulator) | probe/stream E2E, agent restart (instanceId), buffer-wrap with `bufferSize=128`, multi-device demux, TLS |
| soak (manual/advisory) | `demo.mtconnect.org` | long-run stream, content-type tolerance |
| wire | local MQTT | exact envelope + extras assertions |

Coverage: component + `mtconnect/` inside the 90% gate; fuzz targets and the env-gated integration
excluded per the template's documented pattern (live-seam pragma rules).

## 13. Milestones (each ends green + committed)

1. **M1 scaffold+model**: CLI scaffold; `config.rs`/`xml.rs`(Devices)/`model.rs` + digest;
   sim-backed suite green.
2. **M2 client+snapshot**: `client.rs`, `/probe`+`/current`, poll-mode acquisition end-to-end
   against cppagent-docker; Reading extension + publish path.
3. **M3 streaming**: `multipart.rs`+`stream.rs`+`sequence.rs` ladders; fuzz targets; integration
   restart/overrun cases.
4. **M4 commands+panels**: full verb surface, browse dual-mode, write refusal/availability,
   panel manifest snapshot; wire gate.
5. **M5 hardening**: TLS/auth, config reload swap, metrics/events complete, coverage to gate,
   docs (Diátaxis) current-state, HOST validation; then the platform gates per org matrix.

## 14. Open questions for review

1. In-crate `src/mtconnect/` vs workspace crate — LLD picks in-crate (size); flip if you want the
   PROFINET-style layout uniformly.
2. Default `interval` for streaming (proposed: per-signal `publish` min, floor 250 ms) — or a
   fixed agent-level `streamIntervalMs`?
3. `path=` server-side filtering: propose ON when configured signals cover <30% of the device's
   data items, else stream unfiltered and demux locally — acceptable heuristic, or config-explicit
   only?
4. Condition observations: R1 publishes state as value with extras; should `Warning`→`UNCERTAIN`
   quality apply to the condition signal itself (proposed: yes) in addition to bound signals?
