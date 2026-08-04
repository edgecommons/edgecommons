# MTConnect adapter — low-level implementation design

Status: **implemented — repository `edgecommons/mtconnect-adapter`**

Parent: [mtconnect-adapter.md](mtconnect-adapter.md) (the HLD; its decision register D-MTC-1..10 is
binding) and the [shared adapter contract](README.md). Baseline: the post-conformance Rust
`protocol-adapter` template, whose generated seams this document
names explicitly. Language: Rust, edition 2024, tokio, `reqwest` (rustls) + `quick-xml`.

## 1. Repository layout (scaffold + owned modules)

`edgecommons component new … --language RUST --kind protocol-adapter` generates the floor
(simulator, tests, packs, docs). The owned
client is an in-crate module tree — a thin client does not warrant a workspace crate (deviation
from PROFINET's crate split, justified by size):

```text
src/
  app.rs          # config types, backoff, health, connectivity, the publish mapping,
                  # structured-shutdown helpers (join_all_within + the staged budgets)
  driver.rs       # the device drivers: connect/poll/publish/reconnect orchestration, the
                  # control-channel service, shaping + passive-quality wiring — behind the
                  # `Wire` publish/emit seam, inside the coverage denominator
  supervisor.rs   # the thin live shell: construction, spawning, the shutdown invocation,
                  # and FacadeWire (the facade-backed `Wire`)
  device.rs       # the DeviceSession/DeviceBackend seam, the MTConnect backend/session,
                  # the condition ledger, credential resolution (§4)
  commands.rs     # the sb/* verbs and panel descriptors
  metrics.rs      # southbound_health + operational families + the HLD §9 families
  reload.rs       # pre-commit reload verdict + the live per-instance signal registry
  shaping.rs      # per-signal publish shaping (batch windows + deadband), pure, virtual-clock
  staleness.rs    # passive quality: PassiveLink + QualityWatchdog, pure, virtual-clock
  mtconnect/
    mod.rs          # AgentRuntime + the two-lane instance queue (§3); types re-exports
    config.rs       # AgentConfig, DeviceConfig, SignalConfig (serde, validated)
    client.rs       # HTTP: probe/current/sample one-shots; auth/TLS; redirects off
    stream.rs       # multipart stream reader + heartbeat supervision
    multipart.rs    # owned bounded multipart splitter (both content-types)
    xml.rs          # namespace-tolerant pull parsing: Devices/Streams/Errors docs
    model.rs        # ProbeModel tree + canonical digest + browse projection
    observations.rs # Observation, category/type decode, required-field rejection
    sequence.rs     # SequenceState machine: stream/recover/resync/poll
    selection.rs    # probe-derived selection: served_set + channel derivation
    stats.rs        # monotonic acquisition counters behind the metric families
    error.rs        # MtcError taxonomy → command codes / qualityRaw (§9)
```

**Isolation rule (enforced by `tests/isolation.rs`): `src/mtconnect/**` imports nothing from
`edgecommons`.** All EdgeCommons awareness lives above the seam
(`device.rs`/`driver.rs`/`supervisor.rs`/`commands.rs`).

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

// observations.rs — one parsed observation from a Streams document.
// dataItemId, a sequence ≥ 1, and a non-empty timestamp are REQUIRED: an observation missing
// one is rejected (DecodeReject) and counted, never defaulted.
pub struct Observation {
    pub data_item_id: String,
    pub sequence: u64,
    pub timestamp: String,             // verbatim as sent by the agent — the capture stamp (→ serverTs)
    pub value: ObsValue,               // Scalar(Value) | Unavailable | Condition(CondState)
    pub extras: SmallVec<[(&'static str, Value); 4]>, // resetTriggered, duration, nativeCode,
                                       // conditionId/nativeSeverity/qualifier/conditionText, …
    pub received: Option<String>,      // arrival stamp, set at document ingest (→ receivedTs)
}

// sequence.rs
pub enum AcqState { Connecting, Streaming { next: u64 }, Recovering, Resyncing, Polling, Backoff }
pub struct SequenceState { pub instance_id: Option<u64>, pub next: u64,
    pub last_published: HashMap<String, u64> /* dataItemId → seq, dedupe floor */ }
```

## 3. Task and channel topology

One tokio task set per **agent** (the shared runtime), plus one device task per instance
(`driver::run_device`, spawned by the supervisor shell). Ownership: `Arc<AgentRuntime>` is built
per configured agent before device tasks start (mirrors the template's construction order;
readiness stays false until handles install — ADP-3 activation ordering is generated). Every
spawned handle is retained and joined by the staged shutdown in `app.rs` (devices 6 s → agents +
tickers 4 s → metric flush 2 s; stragglers aborted and named).

```text
AgentRuntime (per agent)
  ├─ acq_task: owns reqwest client + SequenceState; runs the §5 state machine
  ├─ tx: per-instance TWO-LANE queue (InstanceTx)
  │      data lane     — cap 1024 (INSTANCE_QUEUE_DEPTH): ordinary Obs deliveries;
  │                      on overflow, latest-value coalescing per dataItemId, then
  │                      drop-and-count of the oldest — freshness over completeness
  │      critical lane — cap 256 (CRITICAL_QUEUE_DEPTH), reserved for the loss-intolerant
  │                      classes; when full the sender waits a bounded CRITICAL_SEND_BUDGET
  │                      (5 s), cancellation-aware, then drops-and-counts — see D-R2 below
  └─ ctl: mpsc<AgentCtl> (Reconnect, Snapshot(reply), Shutdown, …)

InstanceEvent = Obs(Observation) | Snapshot(Vec<Observation>) | AgentUp(AgentInfo)
             | AgentDown(reason) | DataLoss{skipped: u64} | ModelDrift{old, new_digest}
             | StreamDegraded{failures: u32}

loss-intolerant ⇔ AgentUp | AgentDown | DataLoss | ModelDrift | StreamDegraded | Snapshot(_)
```

Ordinary flow is delivered per observation (`Obs`); `Snapshot` means a genuine re-baseline
(connect, resync, resume) — the distinction is what lets the publish-side deadband anchor survive
across cycles and re-arm only on a true re-baseline. Every drop, either lane, folds into the
runtime's `dropped_events`/`queue_counters()` and is logged; a coalesce is not a loss and is
counted separately.

**D-R2 (bounded critical lane).** The critical lane is bounded rather than an unbounded
`send().await`, settled with the user: the shared publish path is the one true cross-agent
coupling, so unbounded backpressure against a dead broker/nucleus would freeze ALL acquisition
indefinitely while the backpressured events could not be published anyway. The bound is a reserved
cap (256) that data volume can never consume, a 5 s bounded wait so real consumer lag still
backpressures properly, drop-and-count past the bound, and cancellation-awareness so shutdown
always preempts the wait.

The generated `DeviceSession` for an instance is a **handle**: `{agent: Arc<AgentRuntime>,
uuid, model, rx}` — it owns no socket (ADP-3/D-MTC-3). `read_now`/`repoll`
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
    pub source_ts: Option<String>,            // machine ts — absent for MTConnect
    pub capture_ts: Option<String>,           // observation timestamp (agent capture stamp)
    pub received_ts: Option<String>,          // payload arrival at the adapter (worker
                                              // read-completion fallback if unstamped)
    pub extra: Option<serde_json::Map<String, Value>>, // sequence, resetTriggered, passive, …
    pub channel: Option<String>,              // explicit UNS channel override
    pub component_path: Option<String>,       // canonical untruncated path (→ update-level
                                              // componentPath extra; "" device-level, None unmodelled)
}
```

Publish path (`app.rs`'s `build_sample`, carried by the drivers through the `Wire` seam): build
`Sample` via the core facade — `Sample::null_value()` **with `quality: Bad` and
`quality_raw: "UNAVAILABLE"`** for unavailable observations (the shipped facade gates only the
null *permission* on `explicit_null`; quality is free), ordinary `Sample::new(v)` otherwise; attach
`extra` entries (`sequence` always; `resetTriggered`/`duration`/`nativeCode` when present, plus
`receivedTs` per the four-slot model — MTConnect is mediated, so receive differs from capture) via
the shipped `Sample::extra`; map the observation timestamp to `serverTs` (capture); `sourceTs`
stays absent. This matches the post-timestamps template seam (three optional slots) rather than
widening it locally. Simulator, scheduled publisher, `sb/read`, and
tests move together with this extension (contract rule: command reads must not stay lossy).

## 5. Acquisition state machine (`sequence.rs` + `stream.rs`)

```text
Connecting ──probe ok──▶ Snapshot(/current) ──▶ Streaming(next = header.nextSequence)
Streaming:
  GET {base}/sample?interval=250&heartbeat={h}&from={next}      (STREAM_INTERVAL_MS = 250;
                                                                 unfiltered — demux is local)
  loop parts:
    Streams doc  → publish obs where seq ≥ from, seq > last_published[dataItemId];
                   next = doc.header.nextSequence; liveness touched
    empty doc    → heartbeat: refresh liveness deadline only
    Errors doc(OUT_OF_RANGE) → Recovering
  silence ≥ 2×heartbeat_ms → mark down → drop stream → Streaming (same next)     [ladder 1]
  transport lost / malformed / EOF → mark down → re-establish                     [ladder 1]
Recovering:  emit DataLoss{skipped = firstSequence.saturating_sub(next)};  (NO mark-down: the
             OUT_OF_RANGE document proves the agent alive — D-R3)
             Snapshot(/current) → publish as fresh; next = snapshot.nextSequence
             → Streaming                                                          [ladder 2]
InstanceId change (any doc header) → Resyncing (NO mark-down — D-R3): re-probe FIRST — nothing
             from the restarted agent's document is published until the re-probe and recompile
             complete, and a failed probe fails the cycle; digest≠cached → ModelDrift event,
             browse cursors invalidated (viewGeneration=digest), signals recompiled against the
             new model (missing dataItemId → that signal → permanent BAD MTC_NO_SUCH_DATAITEM);
             then Snapshot → Streaming                                            [ladder 3]
Establish accounting (D-R4): a stream is established only after its first liveness part; a
             zero-part exit is an establish failure and waits the backoff. After
             STREAM_ESTABLISH_FAILURE_LIMIT (3) consecutive failures → StreamDegraded event →
Polling (StreamPolicy::PollOnly, or degraded as above):
  /current every poll_interval_ms; same snapshot/dedupe rules; Streaming retried per reconnect cfg.
  HTTP-200 MTConnectErrors on /current → AgentError (parsed-OK in the counters, no liveness
  refresh); the 3rd consecutive erroring cycle (CURRENT_ERROR_DOWN_STREAK) marks down (D-R9).
Backoff: capped exponential + full jitter (template's generated policy) on connect/probe failure.
```

Dedupe rule is per data item (`last_published`), not global — one stream serves many devices and
`/current` snapshots overlap the stream window. Pause (D-MTC-7/HLD §7): acq continues, cache
updates, per-instance publish gate closes; resume forces `Snapshot` first, reading the **live**
inventory at resume time — a signal a reload added during the pause is in the snapshot. Only the
acquisition path's ingest/mark-down pair writes `connected` (D-R1/D-R5); a command-path snapshot
refreshes `last_liveness` but never flips the flag.

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
  namespace URI version (1.3–2.7 tolerance; a *prefixed* MTConnect declaration — `xmlns:m="…"` —
  is detected exactly like a default one, so qualifying the elements cannot bypass the 1.3 floor);
  three document parsers (`Devices`, `Streams`, `Errors`) that skip unknown elements
  (forward-compat); depth cap 64 (`MAX_DEPTH`), attribute count/length caps, and a 250 000-element
  document cap (`MAX_NODES`) independent of `maxDocumentBytes`; no DTD/entity resolution
  (quick-xml default; asserted by test with an XXE fixture). Header struct: `{instanceId,
  bufferSize, firstSequence, lastSequence, nextSequence, version, sender}`.
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
(`pollIntervalMs`, `publishMode`, `batchMs`, `maxDocumentBytes`, reconnect), and
`healthThresholds.staleSignalSecs`. `#/$defs/device`
(aliased by the generated `#/$defs/instance`): `{id, adapter: "mtconnect", connection:
{agentId, deviceUuid}, signals: [SignalConfig…], selection?, writes: {allow: {type: array,
maxItems: 0}}}` —
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
`(total, interval)` counter pairs, dimensions `agentId`/`instance`/`result` only; `MtconnectParse`
carries `documentsParsed`, `parseErrors`, and `rejectedObservations` (required-field rejects —
D-R10/D-R11). Events via the
generated event helper: `MtconnectAgentEvent` (up/down/degraded), `MtconnectDataLossEvent`
(skipped count, sequence window), `MtconnectModelDriftEvent` (old/new digest),
`MtconnectConditionEvent` (transitions of the activation **aggregate** into Fault, rate-limited
1/min per dataItemId; context carries `conditionId` and `activeConditions`).

Passive quality (HLD §6 rows on the liveness clock) is wired in `staleness.rs` + `driver.rs`:
`DeviceSession::passive_input` reports the link facts (`unreachable`, `liveness_age`,
`liveness_window`) straight from the connectivity authority; the per-instance `QualityWatchdog` is
fed every reading that reaches the wire and evaluated on every poll tick and link transition,
emitting the synthetic transitions (held value + `passive` marker) that `publish_readings` carries
to the wire, bypassing shaping. Synthetic readings feed neither the watchdog nor
`DeviceMetrics.last_update` (D-R13), so recovery restores the held verdict and the `staleSignals`
metric keeps meaning value silence.

## 11. Panels

Descriptors exactly per HLD §8, emitted from `commands.rs` beside the generated trio (replacing
their content, keeping registration order/ids where shared). Every view carries
`rendererRequirements`; grids/trees declare generic `columns` (shipped renderer). Snapshot test
pins the full manifest + absence of `writeVerb` + the `sb/write` availability state in `describe`.

## 12. Testing plan

| Layer | Vehicle | Key cases |
|---|---|---|
| multipart.rs | unit + `cargo fuzz` target | both content-types, split boundaries across chunks, oversize part, missing length, junk between parts |
| xml.rs | unit + fuzz + goldens | goldens per ns version 1.3/1.7/2.0/2.7 (free XSD-derived fixtures), XXE fixture inert, unknown-element skip, prefixed-declaration floor, caps (depth/attrs/nodes), header extraction |
| sequence.rs | virtual-clock unit | ladder 1/2/3, dedupe overlap (snapshot ∩ stream), heartbeat expiry math, PollOnly, N-failure poll degradation |
| device seam | fake AgentRuntime | Reading extension, UNAVAILABLE null+BAD publish, extras (`sequence`) on the wire body, pause cache-update/no-publish |
| commands | generated harness + fake runtime | write refusal + availability, browse paged/hierarchical/cold-cache, read scoped snapshot, PAUSED |
| driver orchestration | `src/driver.rs` in-module suite: fake `DeviceBackend`/`DeviceSession` over a recording `Wire` | pause clears windows / resume snapshots the live inventory, shaping-generation swaps flush on the old policy, passive transitions reach the wire, cancellation flushes before detach |
| shaping / staleness | virtual-clock unit tables | window/deadband/lifecycle rules; the passive ladder (stale → expired → unreachable → recovered), verbatim restoration, synthetic-reading shape |
| integration | `tests/agent_integration.rs`, env-gated `EC_MTC_AGENT` (compose file pinning `mtconnect/agent:2.7.0.12` + in-tree SHDR simulator); `EC_REQUIRE_LIVE` turns the self-skip into a hard failure | probe/stream E2E, agent restart (instanceId), buffer-wrap with `bufferSize=128`, multi-device demux, TLS |
| soak (manual/advisory) | `demo.mtconnect.org` | long-run stream, content-type tolerance |
| wire | local MQTT | exact envelope + extras assertions (incl. `passive`, `conditionId`/`activeConditions`, `componentPath`, `sequence` on synthetic readings) |

Coverage: component + `mtconnect/` inside the 90% gate, `driver.rs` included; excluded are only
`supervisor.rs` (the thin live shell), `main.rs`, and the env-gated live suites, each exclusion
pinned to a reason in the CI workflow.

## 13. Delivery status

The design above is implemented in `edgecommons/mtconnect-adapter`, whose root `DESIGN.md` is the
component's design-fidelity contract (this document's decisions plus the local
D-MtconnectAdapter-L* and remediation D-R* registers). Release gates recorded open there: the
extended wire gate over local MQTT and the Greengrass/Kubernetes platform legs.

## 14. Settled design points

1. **In-crate `src/mtconnect/` module tree**, not a workspace crate — a thin client does not
   warrant the PROFINET-style split.
2. **The streaming `interval` is a fixed 250 ms floor** (`STREAM_INTERVAL_MS`); per-signal cadence
   is the publish-shaping engine's job, above the session.
3. **The stream is unfiltered and demultiplexed locally.** `path=` scoping applies only to
   command-path `/current` reads (`sb/read`), where the request names its data items.
4. **Condition quality applies to the condition signal itself**, from the activation aggregate —
   `WARNING` → `UNCERTAIN`, `FAULT` → `BAD` — in addition to degrading bound signals.

## Appendix — revision history

| Date | Change |
|---|---|
| 2026-08-03 | Status: implemented. §1 real module layout (`driver.rs`, `shaping.rs`, `staleness.rs`, `reload.rs`, `selection.rs`, `stats.rs`); §2 required-field rejection and the arrival stamp; §3 two-lane bounded queue with the D-R2 decision; §5 mark-down scope, establish accounting, resync-before-publish, `/current` errors policy; §6 prefixed-declaration floor and the element cap; §10 `rejectedObservations` + passive-quality wiring; §12 current suites and coverage split; §§13–14 delivery status and settled points. |
| 2026-07-27 | Initial design. |
