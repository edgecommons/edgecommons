# DESIGN — the app-usable class publish facades: `data()`, `events()`, `app()`

> **Status: DECIDED + BUILT in Java canonical (2026-07-03).** The nine open decisions below are
> **RESOLVED** — see the decision register in §8. The Java implementation has shipped on branch
> `feat/unified-namespace`: the `Quality`/`Severity`/`Channel` enums + the `SignalUpdate` body
> builder + the `DataFacade`/`EventsFacade`/`AppFacade` facades under
> `libs/java/.../facades/`, wired onto `GgInstance.data()/events()/app()` and the `GGCommons`
> convenience `getData()/getEvents()/getApp()` (== instance `main`); the body contracts are pinned
> by new `uns-test-vectors/{data,evt,app}.json` and the refreshed `envelopes.json` goldens.
> **Python / Rust / TS mirrors follow** (§3.2 mirror notes + §7). This closes the last gap in
> the UNS class-facade family: the reserved platform classes (`state`/`metric`/`cfg`, and `log`
> deferred) each already have a **library-owned publisher** that mints the correct topic *and*
> constructs/validates the class body; the inbound `cmd` class has the `CommandInbox`. But the three
> **app-usable** publish classes — `data`, `evt`, `app` — have **no facade**. Components publish them
> **raw**: they hand-mint `gg.instance(id).uns().topic(CLASS, channel)`, hand-build the body, and call
> `messaging().publish(topic, msg)` themselves. The UNS *topic* is enforced (by `uns()`), but the UNS
> *body contract* for each class is **not** — every adapter re-invents the `SouthboundSignalUpdate`
> body, the `evt` severity/shape, and the quality/timestamp defaults, and they have **already
> diverged** (evidence in §1.1).
>
> **Companion docs:** [`DESIGN-uns.md`](DESIGN-uns.md) §7.3 (the facade table — these three are the
> `telemetry()`/`events()` rows, explicitly *deferred to the components phase*),
> [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) §7 ("Facade scope for THIS effort" — lists
> `telemetry()`/`events()` as **deferred**), [`DESIGN-channels.md`](DESIGN-channels.md) (the
> local/northbound/stream channel model these facades route on), [`../SOUTHBOUND.md`](../SOUTHBOUND.md)
> (the `SouthboundSignalUpdate` body the `data` facade constructs). Java is canonical; the build lands
> in all four libraries with identical semantics, pinned by `uns-test-vectors/`.

---

## 0. TL;DR

| Facade | Class | Owns | Constructs & defaults | Reserved? |
|---|---|---|---|---|
| `data()` | `data` | the telemetry/signal data plane | the `SouthboundSignalUpdate` body (device/signal/samples); **quality defaulted to `GOOD` when the source omits it**, `serverTs` filled, `samples` wrapper enforced | **No** — any component may publish |
| `events()` (`evt`) | `evt` | operator events & alarms | `{severity, type, message, timestamp, context, alarm?, active?}`; channel `evt/{severity}/{type}`; severity ∈ `critical\|warning\|info\|debug` | **No** |
| `app()` | `app` | free-form inter-component pub/sub | a **named** header + developer body, minted onto `app/{channel}` with `identity` stamped | **No** |

These are the **non-reserved** siblings of the reserved publishers. The reserved publishers bypass the
guard through `ReservedPublisher` (a privileged seam). These three publish through the **ordinary,
guarded** `messaging().publish(...)` — and pass the guard *because their class is not reserved*. So the
facades add **no** new privilege; they add **body-contract enforcement + sane defaults + one obvious
call site**, replacing the hand-rolled raw path.

---

## 1. Motivation — the raw path is unenforced, and it has already drifted

### 1.1 What "raw" looks like today (the thing we are replacing)

All three reference components were migrated (UNS Phase-5 pre-work) from legacy topic templates onto the
UNS `data`/`evt` classes, but each still **hand-builds the body**. Verbatim current shapes:

**OPC UA adapter — `SignalUpdatePublisher.java` (Java):** builds `device`/`signal`/`samples` by hand,
sanitizes the channel by hand, mints the topic by hand, calls `messaging.publish`:

```java
JsonObject body = new JsonObject();
body.add("device", device);              // {adapter, instance, endpoint}
body.add("signal", signal);              // {id, name, address}
body.add("samples", samples);            // ValueCodec.toSample(...) per DataValue
String signalPath = ConfigManager.sanitize(node.getNodeId().getIdentifier().toString());
topic = instance.uns().topic(UnsClass.DATA, signalPath);
Message message = instance.newMessage("SouthboundSignalUpdate", "1.0").withPayload(body).build();
messaging.publish(topic, message);
```

**Modbus adapter — `publisher.py` (Python):** the *same* body assembled independently, with its **own**
`make_sample()` default (`quality="GOOD", quality_raw="Good"`):

```python
body = {"device": {...}, "signal": {...}, "samples": samples}
topic = self._instance.uns().topic(UnsClass.DATA, ConfigManager.sanitize(signal.name))
msg = self._instance.new_message("SouthboundSignalUpdate", "1.0").with_payload(body).build()
self._messaging.publish(topic, msg)
```

**Telemetry-processor — `proc/route.rs` (Rust):** republishes an already-shaped `data` envelope,
restamping identity; its `evt` health events go through a component-local `EvtEmitter`.

### 1.2 The drift this already caused (the load-bearing evidence)

The `data` body is *roughly* consistent because both adapters copied `SOUTHBOUND.md` §2 by hand — but
the two **quality defaulting rules already differ in spirit**, and the **`evt` class has visibly
forked**:

| Concern | OPC UA adapter | Modbus adapter | telemetry-processor |
|---|---|---|---|
| `evt` channel convention | `critical/connection-lost`, `connection-restored`, `warning/write-rejected` (severity-prefixed, sometimes **no** severity) | `connection`, `write` (**no severity at all**) | `route_error` internal `EvtEmitter` (own shape) |
| `evt` header `name` | (adapter-chosen) | `"SouthboundEvent"` | (processor-chosen) |
| `evt` body | `{...}` ad-hoc | `{"instance","connected",...}` | `{route, topic, error}` |
| `data` sample quality default | codec-derived from `StatusCode` | `make_sample(quality="GOOD")` hard-coded in the adapter | n/a (pass-through) |

A fleet consumer (the `edge-console`, a historian) that subscribes `ecv1/+/+/+/evt/#` therefore **cannot
assume** a severity segment exists or a body field is present — the whole point of the UNS ("subscribe a
small uniform set of wildcards, zero per-component knowledge") is undermined for `evt`, and is one
copy-paste away from breaking for `data`. **The facade is where the body contract becomes real code
instead of prose every component re-types.**

### 1.3 The quality guarantee (the crux)

`SOUTHBOUND.md` §2 says *"Quality is first-class. **Every** sample carries a `quality` normalized to
`GOOD | BAD | UNCERTAIN`."* That is a **contract on the body**, and today nothing enforces it — a Modbus
register or an MQTT-passthrough source has **no native quality notion**, so the adapter has to *remember*
to synthesize one. The `data()` facade makes the guarantee structural: **you cannot emit a sample without
a `quality`, because the facade fills it (`GOOD` by default) if you don't.** That single invariant is the
concrete value the whole design is built around.

---

## 2. The three facades

Each facade is a thin, per-instance object that: (1) knows its class, (2) exposes a small typed
build-and-publish surface, (3) **constructs the class body** with defaults, (4) mints the topic via the
bound `Uns` (so the depth/char guards run at build time), (5) stamps the envelope `identity` via the
bound `MessageBuilder`, and (6) routes to the resolved channel (§4). They are obtained from the instance
handle (primary) or from `gg` at instance `main` (convenience) — see §3.

### 2.1 `data()` — the telemetry / signal data plane

**Class:** `data` · **Channel:** the sanitized signal path (1 token; ≤3 rootless per the depth guard) ·
**Header name:** `SouthboundSignalUpdate`, version `1.0`.

**Body contract it constructs + validates** (a superset of `SouthboundSignalUpdate`, §5 for the
subsume-vs-wrap decision):

```jsonc
"body": {
  "device":  { "adapter": "<str>", "instance": "<str>", "endpoint": "<str>" },   // optional block
  "signal":  { "id": "<stable str, REQUIRED>", "name": "<str?>", "address": { } },
  "samples": [
    { "value": <any, REQUIRED>,
      "quality": "GOOD|BAD|UNCERTAIN",     // DEFAULTED to GOOD if omitted
      "qualityRaw": "<native code?>",       // DEFAULTED to a marker when quality was defaulted
      "sourceTs": "<iso?>",                 // left null if the source has none
      "serverTs": "<iso>" }                 // DEFAULTED to now() if omitted
  ]
}
```

**Exact defaulting rules** (the crux — identical in all four languages, pinned by `data.json` vectors):

1. **`quality` → `GOOD`** when the caller omits it on a sample **that carries a `value`**. Rationale: a
   read that produced a value is, absent contrary information, trustworthy — this matches the Modbus
   adapter's existing `make_sample(quality="GOOD")` and OPC UA's "value present" path. A source that
   *knows* the value is stale/failed passes `BAD`/`UNCERTAIN` explicitly. (Conservative-default
   alternative — `UNCERTAIN` — was weighed under D2, §8; `GOOD` is the resolved default.)
2. **`qualityRaw`**: passed through verbatim if given; when `quality` was *defaulted*, `qualityRaw` is set
   to a fixed synthetic marker (`"unspecified"`) so a consumer can tell a synthesized GOOD from a
   device-reported GOOD. A source with no native quality codes at all (Modbus OK reads) may still set its
   own marker.
3. **`serverTs` → `now()`** (ISO-8601 UTC, `…Z`) when omitted. `sourceTs` is **never** synthesized (a
   missing device timestamp is information — leave it `null`), honoring `SOUTHBOUND.md` §2 "at least one
   SHOULD be present" by guaranteeing `serverTs`.
4. **`samples` wrapper enforced**: the value-shorthand form (`publish(signalPath, value)`) wraps the
   single value into a one-element `samples` array with the defaults above, so a caller never emits a
   bare value. The batch form takes an explicit `samples` list.
5. **`signal.id` is REQUIRED** and is the only hard reject inside the body: a `data` publish with no
   stable `signal.id` throws `IllegalArgumentException` — the id is what every consumer keys on, so a
   missing one is a programming error, not a defaultable omission. `signal.name`/`signal.address`/`device`
   are optional (defaulted absent).
6. **Channel sanitization moves into the facade.** Today each adapter calls `ConfigManager.sanitize(...)`
   before `uns().topic(DATA, …)`. The facade does this internally: `data().publish("press12/temperature",
   …)` sanitizes each `/`-separated path token to a UNS token. The raw stable `signal.id` still rides the
   body untouched (D-U15 keeps the sanitized-path-vs-stable-id split).

**How the adapter stops hand-building `SouthboundSignalUpdate`:** the facade exposes a `SignalUpdate`
builder (`signal(id).name(n).address(a).addSample(value, quality, sourceTs)…`) *and* a value-shorthand.
The adapter maps its protocol read onto `addSample(...)` and never touches the envelope or topic. See §6
for the before/after.

### 2.2 `events()` — operator events & alarms (`evt`)

**Class:** `evt` · **Channel:** `{severity}/{type}` (2 tokens) · **Header name:** `evt`, version `1.0`.

This is the facade that **stops the fork in §1.2**: it makes the `evt/{severity}/{type}` channel and the
body shape non-negotiable.

**Severity taxonomy** (a library enum, lowercase wire tokens): `critical` · `warning` · `info` ·
`debug`. These become the **first channel token**, so a console can subscribe `ecv1/+/+/+/evt/critical/#`
for just alarms.

**Body contract:**

```jsonc
"body": {
  "severity":  "critical|warning|info|debug",   // REQUIRED (enum; the channel's 1st token)
  "type":      "<str, REQUIRED>",                // the event type; the channel's 2nd token (sanitized)
  "message":   "<human str?>",                   // optional operator text
  "timestamp": "<iso>",                          // DEFAULTED to now()
  "context":   { },                              // optional free-form structured data
  "alarm":     <bool?>,                          // present only for raiseAlarm/clearAlarm
  "active":    <bool?>                           // true = raised, false = cleared (alarm only)
}
```

**API shape:** two entry points —
- `emit(severity, type, message, context)` — a one-shot event.
- `raiseAlarm(type, message, context)` / `clearAlarm(type, context)` — stateful alarms; these set
  `alarm=true` + `active=true|false` and default `severity` to `critical` for a raise (overridable). This
  directly subsumes OPC UA's `connection-lost`/`connection-restored` pair.

**Defaulting rules:** `timestamp → now()`; `severity` on `emit` defaults to `info` if the caller uses a
message-only convenience overload; `type` is REQUIRED (it is a channel token — no default is meaningful);
`message`/`context` optional. The `{severity}/{type}` channel is built from the body's own `severity` +
`type`, so **the topic and body can never disagree** (today they're set independently).

### 2.3 `app()` — free-form inter-component pub/sub

**Class:** `app` · **Channel:** developer-chosen (`order/received`, …) · **Header name:** developer-chosen.

`app` is the intentionally-open class (DESIGN-uns §4: "arbitrary application pub/sub between
components"). The facade's value is **not** body enforcement (there is no contract to enforce) — it is
removing the three-line raw ritual and guaranteeing identity/topic correctness:

```jsonc
"body": { /* whatever the developer passes — untouched */ }
```

**API shape:** `app().publish(name, channel, body)` where `name` becomes the header `name`, `channel` is
the `app/{channel}` tail, and `body` is passed through verbatim. Optional `publish(name, channel, body,
Channel routing)` for northbound. A `subscribe(channelFilter, handler)` convenience is **out of scope**
(consumers already use `messaging().subscribe(uns().filter(APP, scope), …)`); `app()` is publish-side
sugar to match its two siblings, so the trio is symmetric. D3 (§8) resolves this by **shipping `app()`**
as thin publish-sugar.

---

## 3. API surface (per language)

### 3.1 Where the accessors live — mirror the existing convention

The facades hang off **two** places, exactly like `uns()` does today (`gg.getUns()` component-bound;
`gg.instance(id).uns()` instance-bound):

- **`GgInstance`** (primary) — `data()` / `events()` / `app()`, bound to that instance token. This is the
  natural home for `data`/`evt` because the data plane is inherently per-instance (an adapter serves
  `kep1`, `plc-2`). DESIGN-uns §3 already earmarked this: *"Components phase adds: telemetry(), events(),
  commands() here — same handle, no rework."*
- **`GGCommons`** (convenience) — `getData()` / `getEvents()` / `getApp()` == `instance("main").data()`
  etc., for single-instance components.

**Accessor names (D4, RESOLVED — §8):** match each language's existing style —

| | Java (canonical) | Python | Rust | TS |
|---|---|---|---|---|
| component-bound | `gg.getData()` `gg.getEvents()` `gg.getApp()` | `gg.data()` `gg.events()` `gg.app()` | `gg.data()` `gg.events()` `gg.app()` | `gg.data()` `gg.events()` `gg.app()` |
| instance-bound | `gg.instance(id).data()` … | `gg.instance(id).data()` … | `gg.instance(id)?.data()` … | `gg.instance(id).data()` … |

(Java uses the `getX()` prefix for `getMetrics()`/`getCommands()`/`getStreams()`; the other three drop it
— `gg.metrics()`/`gg.streams()`. The facades follow suit. I recommend the class-name `events()` — not
`evt()` — to align with DESIGN-uns §7.3's `gg.events()`; `data()` and `app()` match the class token.)

### 3.2 Java canonical surface (sketch)

```java
// New library types
public enum Quality { GOOD, BAD, UNCERTAIN }
public enum Severity { CRITICAL, WARNING, INFO, DEBUG }   // wire token = lowercase name
public enum Channel  { LOCAL, NORTHBOUND }                // + Channel.stream(String name)  (§4)

public final class DataFacade {                            // bound to (identity, includeRoot, channelDefault, streams)
    public void publish(SignalUpdate update);              // full form
    public SignalUpdate.Builder signal(String id);         // fluent body builder -> .publish()
    public void publish(String signalPath, Object value);  // shorthand: quality=GOOD, serverTs=now
    public void publish(String signalPath, Object value, Quality q);
    public DataFacade via(Channel channel);                // per-call channel override (returns a bound view)
}

public final class EventsFacade {
    public void emit(Severity sev, String type, String message, JsonObject context);
    public void emit(Severity sev, String type, String message);          // context = {}
    public void raiseAlarm(String type, String message, JsonObject context);
    public void clearAlarm(String type, JsonObject context);
    public EventsFacade via(Channel channel);
}

public final class AppFacade {
    public void publish(String name, String channel, JsonObject body);
    public void publish(String name, String channel, JsonObject body, Channel routing);
}

// GgInstance additions:  DataFacade data();  EventsFacade events();  AppFacade app();
// GGCommons additions:   DataFacade getData(); EventsFacade getEvents(); AppFacade getApp();
```

`SignalUpdate` is the constructed body object (`device?`, `signal{id,name?,address?}`,
`samples[{value,quality,qualityRaw?,sourceTs?,serverTs?}]`) — the thing that replaces the adapters'
hand-assembled `JsonObject`. The facade calls the **public** `messaging().publish(topic, msg)` (guarded,
but `data`/`evt`/`app` pass) — **not** `ReservedPublisher`.

**Mirror notes:** Python — `ggcommons/facades/{data,events,app}.py`, snake methods (`raise_alarm`,
`clear_alarm`), `Quality`/`Severity` as `str, Enum`; works over the process-global `MessagingClient`
exactly as the raw path does today (the facade holds the bound `Uns` + a `MessageBuilder` factory, no
per-instance client state — same reasoning as `GgInstance` in UNS-CANONICAL §3). Rust —
`ggcommons::facades` with `struct DataFacade`/`EventsFacade`/`AppFacade`, builder methods returning
`Result<()>` (the `uns()` topic build is fallible), `Quality`/`Severity` enums with
`#[serde(rename_all="UPPERCASE"/"lowercase")]`; obtained from `GgInstance` (Rust's is `Result`-returning).
TS — `src/facades/*.ts`, `class DataFacade` etc., `enum Quality`/`Severity`, methods return `void`/throw.

### 3.3 Call sites (adapter before → after)

**OPC UA `data` (Java):**

```java
// BEFORE (raw): ~20 lines building device/signal/samples + sanitize + uns().topic + newMessage + publish
// AFTER:
instance.data().publish(
    instance.data().signal(node.getNodeId().toParseableString())
        .name(displayName.isEmpty() ? browseName : displayName)
        .address(ValueCodec.address(node.getNodeId(), namespaceTable))
        .device("opcua", serverConfig.getId(), serverConfig.getConnection().getEndpoint())
        .addSamples(values.stream().map(ValueCodec::toSampleParts).toList())   // value+quality+ts parts
        .signalPath("press12/temperature"));                                    // sanitized by the facade
```

**Modbus `data` (Python):**

```python
# AFTER — quality defaults to GOOD inside the facade; serverTs filled; no hand-built body/topic
self._instance.data().signal(signal.signal_id(group.unit_id)) \
    .name(signal.name).address(signal.address_dict(group.unit_id)) \
    .device("modbus", self._config.id, self._config.connection.describe()) \
    .add_samples(samples).signal_path(signal.name).publish()
```

**Modbus `evt` (Python) — the divergence fix:**

```python
# BEFORE:  self._events.emit("connection", {"instance": id, "connected": connected, ...})
# AFTER:   gg.instance(id).events().emit(Severity.WARNING if not up else Severity.INFO,
#              "connection", "modbus link down", {"connected": up})
#          -> ecv1/{device}/modbus-adapter/{id}/evt/warning/connection   (now severity-segmented like OPC UA)
```

---

## 4. Channel routing (the real decision — options + recommendation)

`DESIGN-channels.md` generalizes message routing to a uniform `{ local, northbound, stream:<name> }`
channel address and proposes an adapter `publish.channel` config (global + per-signal). The `data()`
facade is the natural place to honor it. Three options:

- **Option A — local only.** Facades always `messaging().publish(...)` (channel 1). Streaming stays a
  separate explicit `gg.streams().stream(name).append(...)` call the component makes itself. *Simplest;
  zero new coupling.* But it leaves the data-plane/control-plane split (the OT pattern: bulk telemetry →
  stream, alarms → local/north) entirely to the component, and doesn't realize DESIGN-channels for the
  facade.
- **Option B — explicit per-call channel.** The facade takes an optional `Channel` (`LOCAL` default /
  `NORTHBOUND` / `stream(name)`). `data().via(Channel.stream("hot")).publish(...)` serializes the built
  envelope and routes to `getStreams().stream("hot").append(partitionKey, ts, bytes)` instead of the bus.
  *Unifies the three channels behind one call site.* Cost: the facade must hold a `StreamService`
  reference and a partition-key policy, and handle `getStreams()==null`.
- **Option C — config-driven default.** The facade reads a resolved `publish.channel` from the instance
  config (DESIGN-channels §"config-driven components") and routes automatically; no code change to switch
  a signal from bus to stream. Cost: a schema touch (`publish.channel` enum) and the routing indirection
  is invisible at the call site.

**RESOLVED (D1, §8): B + C.** Both shipped in Java canonical — C's `publish.channel` lives under the
already-permissive `component.*` (instance ▸ global), so it needed no schema change. The facade resolves
its channel as `per-call override (B) ▸ config publish.channel (C) ▸ LOCAL`. Concretely:

- **`data()`** honors all three channels. When the resolved channel is `stream:<name>`: the facade builds
  the same envelope, serializes it (the exact bytes it would have published), and calls
  `getStreams().stream(name).append(StreamRecord(partitionKey = signal.id, ts = serverTs, payload))` —
  the seam DESIGN-channels §"reference components" describes, with `signal.id` as the natural partition
  key. If `streaming` is not configured (`getStreams()==null`), a `stream:` route is a **configuration
  error — but D1a (§8) resolves this as **readiness/no-streaming → local**: the facade falls a
  `stream:` route back to a LOCAL publish (WARN once) rather than dropping or failing fast, never a
  silent no-op.
- **`events()`** honors `LOCAL` (default) and `NORTHBOUND` (alarms often go straight to the cloud control
  plane) but **not** `stream` — events are low-rate control-plane, not bulk telemetry; a `stream` route
  on `events()` is rejected at build time.
- **`app()`** honors `LOCAL` (default) and `NORTHBOUND`; `stream` rejected (same reasoning).

**Relationship to `getStreams()`:** the facade *composes* `StreamService`, it does not replace it — a
component that wants raw byte control still calls `gg.streams().stream(name).append(...)` directly. The
facade's stream route is the *enriched* path: it guarantees the streamed record carries the same
`identity`/header the bus envelope would (which is exactly what M15 / DESIGN-uns §8 wants the streaming
service to do — so `data().via(stream)` is the on-ramp to M15's auto-enrichment, and the two should share
the enrichment code when M15 lands). **Readiness stays local-only** (DESIGN-channels): a northbound or
stream outage inside a facade must not flip `/readyz`, so facade routing catches/ tallies channel-2/3
failures and never propagates them into the local connection's health.

---

## 5. Payload enforcement mechanics

### 5.1 Where validation lives

Inside each facade's `build()` step, in three layers, in order:

1. **Topic** — delegated to the bound `Uns.topic(class, channel)`: char-set, `..`, depth (≤7 slashes /
   ≤3 channel tokens), 256-byte length. Already exists; throws `UnsValidationException`. The facade adds
   **channel sanitization** ahead of it (so a raw signal name with `/` becomes a token, as the adapters
   do today).
2. **Identity** — delegated to `MessageBuilder.build()` + the bound instance token. Already exists.
3. **Body** — new, per class: **default what is defaultable, reject only the impossible.**

### 5.2 Reject vs default vs warn — the policy

The app-usable classes are **open** (DESIGN-uns D7/§4). Being strict-reject on them would fight adoption
and contradict the "keep `messaging()` open" decision. So the policy is deliberately **asymmetric** with
the reserved classes:

| Situation | Reserved publisher (state/metric/cfg) | These app facades (`data`/`evt`/`app`) |
|---|---|---|
| omitted defaultable field (quality, serverTs, timestamp) | n/a | **default it** (silently; `quality` default carries the `unspecified` marker so it's detectable) |
| omitted **structural** field (`data.signal.id`, `evt.type`, `evt.severity`) | n/a | **reject** — `IllegalArgumentException` (programming error; these are channel tokens or the consumer key) |
| bad topic (depth/char) | build throws | build throws (same `uns()` path) |
| raw publish to the class | **guard rejects** (reserved) | **allowed** (non-reserved) — the facade is the *recommended*, not the *only*, path |

So a contract violation the facade *can* absorb becomes a default; a violation it *cannot* absorb (no
`signal.id`) is a fail-fast exception at the call site (not a dropped message on the wire). This keeps the
guarantee ("every sample has a quality") true without making `data` a reserved class.

### 5.3 Test-vector plan (consistency with `uns-test-vectors/`)

`uns-test-vectors/` today has `topics.json`, `envelopes.json`, `bcast.json`, `commands.json`. Note
`envelopes.json`'s `data`/`evt`/`app` goldens are **stubs** (`data` body is a toy `{signalId,value,
quality}`, not the real `SouthboundSignalUpdate` shape) — placeholders that only exercised
topic/identity. This design **replaces those stubs with real body contracts.** Add three vector files,
generated from the Java canonical (D-U12), consumed by all four suites + the interop harness:

- **`data.json`** — cases `{ signal, samples-in, channelResolved } → { topic, body-out }` pinning: the
  `GOOD` quality default (+`qualityRaw:"unspecified"` marker), the `serverTs=now` fill (with an injected
  clock so it's deterministic — the same injected-clock discipline the `_bcast` listener uses), the
  `samples` wrapper for the shorthand, the missing-`signal.id` → error case, and channel sanitization
  (`"a/b"` → two tokens; `"a+b"` → sanitized).
- **`evt.json`** — cases pinning the `{severity}/{type}` channel derivation from the body, the four
  severity tokens, the `timestamp=now` default, and the `raiseAlarm`/`clearAlarm` `alarm/active` fields.
- **`app.json`** — cases pinning "body passed through verbatim, header `name` = caller's name, topic =
  `app/{channel}`" (the thin-facade guarantee).

Interop (`test-infra/interop/*`): extend the existing `uns-pub` node role to publish one `data` and one
`evt` **through the facade** (not the raw builder) with a fixed identity + injected clock, and assert the
four languages emit byte-identical topics and structurally-identical bodies (the harness already compares
structurally, D-U22). This is the cross-language conformance gate that keeps the §1.2 drift from
recurring.

---

## 6. The complete class-facade family (so the model is coherent)

With these three, **every one of the eight UNS classes has exactly one library-owned owner** — the
design becomes closed:

| Class | Owner | Kind | Accessor | Body constructed by | Status |
|---|---|---|---|---|---|
| `state` | `Heartbeat` | reserved, **automatic** | *(none — free)* / `getStatus()`* | `Heartbeat` → `ReservedPublisher` | **shipped** |
| `metric` | `MetricEmitter` | reserved | `getMetrics()` / `gg.metrics()` | `MetricEmitter` → `ReservedPublisher` | **shipped** |
| `cfg` | `EffectiveConfigPublisher` | reserved, **automatic** | *(none — free)* | that publisher → `ReservedPublisher` | **shipped** |
| `log` | *(log-tail publisher)* | reserved | *(none — free)* | deferred | **deferred** |
| `cmd` (inbound) | `CommandInbox` | request/reply | `getCommands()` / `gg.commands()` | inbox + `register(verb,handler)` | **shipped (Java); mirrors pending** |
| **`data`** | **`DataFacade`** | **app-usable** | **`getData()` / `data()`** | **the facade (this doc)** | **PROPOSED** |
| **`evt`** | **`EventsFacade`** | **app-usable** | **`getEvents()` / `events()`** | **the facade (this doc)** | **PROPOSED** |
| **`app`** | **`AppFacade`** | **app-usable** | **`getApp()` / `app()`** | **the facade (this doc)** | **PROPOSED** |

\* `getStatus()` (health nuance: `update(DEGRADED, reason)`, `registerCheck`) is a separate deferred
refinement of the automatic `state` publisher; not part of this doc.

The symmetry to state plainly: **reserved classes** are library-*owned* and published through the
privileged `ReservedPublisher` seam (a component may not forge them); **app-usable classes** are
component-*published* through the ordinary guarded `publish()`, and the facade's job is **body-contract
enforcement + defaults**, not privilege. `data`/`evt`/`app` facades are to the app classes what the
reserved publishers are to the reserved classes — the same "mint-the-topic + construct-the-body" pattern,
minus the guard bypass.

---

## 7. Adoption path

### 7.1 Sequencing

This is the **follow-on to UNS Phase 5** (the adapter UNS adoption that re-pointed them onto raw
`uns()`+`messaging`). Two orderings:

1. **Library facades first (recommended), as their own slice ("S3" / "Phase 4.5"):** land
   `data()`/`events()`/`app()` in all four libraries with vectors, *then* the components adopt them in
   their Phase-5 migration PRs. Clean separation; the library change is independently testable under the
   90% gate; components pin the new lib rev when they migrate.
2. Fold into Phase 5 (facades + adapter adoption in one train). Faster wall-clock, but couples a
   four-language library change to three component-repo migrations — larger blast radius per the
   `tags.thing` lesson (UNS-CANONICAL Risks #4).

Recommend **(1)**: library first, `main`-merged and rev-tagged, then one migration PR per component repo.

### 7.2 Four-language build/parity/test plan

Java canonical → mirrors → vectors → interop, under the 90% gate in each language:

1. **Java canonical:** `Quality`/`Severity`/`Channel` enums; `SignalUpdate` body builder; `DataFacade`/
   `EventsFacade`/`AppFacade`; `GgInstance.data()/events()/app()` + `GGCommons.getData()/getEvents()/
   getApp()`; channel routing (B; C behind the schema add); the `data.json`/`evt.json`/`app.json`
   generator test. JaCoCo 90%.
2. **`uns-test-vectors/`**: add the three files (regenerate; refresh the `envelopes.json` data/evt/app
   goldens to the real bodies); drift gate green.
3. **Mirrors (parallel):** Python (`facades/`), Rust (`facades`, feature-gate the `stream` route behind
   `streaming`), TS (`facades/`) — each validated against the vectors; per-lang 90% (Py/Rust on
   Linux/WSL).
4. **Interop:** extend `uns-pub` to publish `data`+`evt` through the facade; assert byte-identical
   topics + structural bodies 4×4.
5. **Component adoption (their repos):** `opcua-adapter` (Java) `SignalUpdatePublisher` + `EventEmitter`
   → `data()`/`events()`; `modbus-adapter` (Python) `publisher.py` + `events.py` + `command_service.py`
   write-audit → `data()`/`events()`; `telemetry-processor` (Rust) `route.rs` `Local` dispatch →
   `data()` (keeping its identity-restamp/self-echo guard — the facade must not fight the restamp; likely
   the processor keeps a lower-level path here and adopts `events()` for its `route_error` health events
   first). Each is a per-repo PR with the rev bump.
6. **Validation matrix:** HOST dual-MQTT smoke (EMQX) confirming `ecv1/+/+/+/data/#` and
   `ecv1/+/+/+/evt/#` carry the enforced bodies; kind + GG-lab as per the standing matrix.

---

## 8. Decision register (D1–D9 — RESOLVED)

The nine decisions are **RESOLVED** as below and built in Java canonical; the rationale/tradeoff each
was chosen against is kept for the mirrors. "Built as" states exactly what the Java code does, so the
Python/Rust/TS ports have an unambiguous target.

1. **Channel-routing approach — RESOLVED: B + C.** Resolve **per-call `Channel` override ▸ config
   `publish.channel` (instance ▸ global) ▸ `LOCAL`**. `data()` honors all three channels; a
   `stream:<name>` route serializes the same envelope and appends it to
   `getStreams().stream(name).append(partitionKey = signal.id, ts = serverTs, payload)` (COMPOSE
   `StreamService`, don't replace it). `events()`/`app()` are **local/northbound only** — a `stream`
   channel on them is rejected at build time. *Open Decision **1a** (no-streaming handling) is
   RESOLVED as **readiness/no-streaming → local**:* when `getStreams()==null` (no sink wired) a
   `stream:` route **falls back to a LOCAL publish** (WARN once), never a drop and never a fail-fast —
   a stream/northbound transport failure is caught + logged and never flips local readiness. *Built
   as:* `Channel {LOCAL, NORTHBOUND, stream(name)}`, `DataFacade.resolveChannel(via)`,
   `Channel.fromConfig("local"|"northbound"|"stream:<name>")`, the `StreamSink` seam
   (`getStreams()` bound in `GGCommons.streamSink()`).
2. **`data` quality default policy — RESOLVED: `GOOD`.** An omitted sample quality defaults to `GOOD`
   and the synthesis is marked `qualityRaw:"unspecified"` (a caller-supplied `GOOD` with its own
   `qualityRaw` is distinguishable). A source that knows a read is stale/failed passes
   `BAD`/`UNCERTAIN` explicitly. `serverTs` defaults to `now()`; `sourceTs` is **never** synthesized
   (absent when the source has none). The **only** hard reject is a missing/empty `signal.id` (plus an
   empty `samples` list or a sample with no `value`) → `IllegalArgumentException` at the call site.
   *Built as:* `DataFacade.buildBody(SignalUpdate)`; pinned by `uns-test-vectors/data.json`.
3. **`app()` enforcement — RESOLVED: ship it as thin publish-sugar.** Named header + verbatim body onto
   `app/{channel}` (each `/`-token sanitized) with `identity` stamped; **no body contract**. Kept for a
   symmetric trio + one obvious call site. *Built as:* `AppFacade.publish(name, channel, body[, routing])`.
4. **Accessor naming — RESOLVED: per-language convention, class-name `events()` (not `evt()`).** Java
   `getData()/getEvents()/getApp()` (component-bound) + `gg.instance(id).data()/events()/app()`
   (instance-bound); the other three languages `gg.data()/events()/app()`. *Built as those names.*
5. **Subsume vs wrap `SouthboundSignalUpdate` — RESOLVED: subsume, with a raw escape hatch.** The facade
   *constructs* the body via the `SignalUpdate` builder (`signal(id).name().address().device()
   .addSample(...).signalPath().publish()`); the escape hatch `publishBody(signalPath, body[, via])`
   publishes a caller-owned pre-built body verbatim (no defaulting). The library now owns the
   `SouthboundSignalUpdate` shape — a future contract change is a library change (the coordination cost
   is accepted; it is exactly what kills the per-adapter drift). *Built as:* `SignalUpdate` +
   `DataFacade.signal(...)` / `publishBody(...)`.
6. **Where the facade lives — RESOLVED: both.** Instance-bound on `GgInstance` (primary — the data plane
   is per-instance: `instance("kep1").data()`) + component-bound convenience on `GGCommons` (==
   `instance("main")`). *Built as:* `GgInstance.data()/events()/app()` (lazily cached) +
   `GGCommons.getData()/getEvents()/getApp()`.
7. **Sequencing — RESOLVED: library-facades-first slice.** Land the four-language library facades
   (Java-canonical first, this build) `main`-merged and rev-tagged before the component-adoption PRs
   (Phase 5). Raw publishing is **not** deprecated. Java canonical is shipped here; the mirrors + adapter
   adoption follow.
8. **`events()` alarm state — RESOLVED: include `raiseAlarm`/`clearAlarm` now.** Body
   `{severity, type, message?, timestamp, context?, alarm?, active?}`; `raiseAlarm`/`clearAlarm` set
   `alarm=true` + `active=true|false`. Both default `severity` to **`critical`** (overridable) so an
   alarm's raise and clear ride the **same** `evt/critical/{type}` channel — a console tracking
   `evt/critical/#` sees both. The `{severity}/{type}` channel is derived from the body, so topic and
   body can never disagree. *Built as:* `EventsFacade.emit/raiseAlarm/clearAlarm`; pinned by
   `uns-test-vectors/evt.json`.
9. **Deprecate raw publishing? — RESOLVED: no.** The facades are enforcement-*by-default*, not
   prohibition; raw `messaging().publish(uns().topic(class, ch), …)` stays allowed (honoring D7's "keep
   `messaging()` open"). Only the reserved classes are truly closed. §1.2-style drift is discouraged,
   not impossible.

---

*Java canonical is built on `feat/unified-namespace` (uncommitted, pending review): the facades +
enums + `SignalUpdate` under `libs/java/.../facades/`, the accessor wiring, the
`uns-test-vectors/{data,evt,app}.json` + refreshed `envelopes.json` goldens, and the unit tests under
`mvn verify` (JaCoCo 90%). The Python / Rust / TS mirrors replicate the §3.2 seams + the same vectors.*
