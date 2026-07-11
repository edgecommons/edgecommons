# edgecommons UNS Phase 3 — named messaging connections, the `uns-bridge`, and the site-broker recipe

> **Status (2026-07-03): SHIPPED.** This design has been implemented and tested, not merely reviewed.
> The `uns-bridge` component (Rust; sibling repo, local-only — see §3 for its exact status) implements
> the full relay engine described below — P3-2 (relay core + hop-tag loop protection) through P3-6
> (dual-EMQX end-to-end proof, 9/9 assertions green) — plus the site-broker deploy recipes (P3-5:
> HOST compose, GREENGRASS `DockerApplicationManager` recipe, KUBERNETES manifests, the per-device
> ACL file). See §7 for the
> per-slice status and the repo's own README for the up-to-date slice table. **What's still open
> before general release:** the edge-console (its first site-side client) hasn't been built, and the
> GREENGRASS/IPC variant (today only the HOST/KUBERNETES dual-MQTT variant is built) is a documented
> follow-up. The org-integration items are now **done**: the repo is pushed to GitHub as
> `edgecommons/uns-bridge`, has a `registry/components.json` entry (`category: bridge`), and its
> `Cargo.toml` `edgecommons` git-rev pin is `b1d8d85` — the **v0.2.0 UNS release on `main`**, which
> contains the UNS core — so a pure git-rev build resolves it (local dev still uses the sibling
> `[patch]` override).
>
> Phase-3 companion to [`DESIGN-uns.md`](DESIGN-uns.md) (§9 the site-bus realization) and
> [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) (§2.3 the named connection, §6 LWT, the
> D‑U register). It turns **M8** (named/secondary messaging connection), **M1** (the `uns-bridge`
> component), and **M2** (site-broker deploy recipes) into an implementation-ready design, and
> carries the **Phase-3 decisions register (D‑B1…D‑B15)**.
>
> The UNS core (grammar, identity, `uns()`, reserved-class guard, request deadline, LWT hook,
> library publishers) is **DONE and merged to `main`** (v0.2.0, release commit `b1d8d85`); every
> library-seam claim below is verified against that source with `file:line` citations into `libs/rust/`.

---

## 0. Grounding — the committed seams this design builds on (verified 2026-07-03)

| Seam | Where (committed source) | What it gives Phase 3 |
|---|---|---|
| Service-over-provider layering | `DefaultMessagingService::new(Arc<dyn MessagingProvider>)` (`libs/rust/src/messaging/service.rs:299`) | A second, fully independent `MessagingService` is just a second `DefaultMessagingService` over a second `MqttProvider` — no new abstraction needed. |
| Reserved-class guard | `check_reserved` on every `publish*`/`request*`/`reply*` (`service.rs:341–353`, predicate `uns::reserved_class_of`, `uns.rs:585`) | The bridge relays at the **raw `MessagingProvider` level** (§1.3), so the guard is **not in the relay path** — no per-connection opt-out is needed. |
| Request deadline | `set_default_request_timeout` (`service.rs:314`), late-bound from `messaging.requestTimeoutSeconds` (`lib.rs:386–389`, `config/model.rs:497`) | Also per-instance — named connections get the same deadline default bound at build. |
| Guard root binding | `set_guard_include_root` bound to `Config::effective_include_root()` (`service.rs:333`, `model.rs:489`) | Same late-bind applies to each named connection. |
| MQTT provider | `MqttProvider::connect(&MessagingConfig)` for normal component MQTT and `MqttProvider::connect_with_last_will(..., Option<&MqttLastWill>)` for the bridge site uplink; dual-broker, blocks <= 10 s for the local CONNACK (`provider/mqtt.rs`, `CONNECT_TIMEOUT`); re-subscribes every filter on each CONNACK | The bridge reuses the core broker/provider machinery for its site connection, but passes a private Last-Will derived from the bridge's resolved state topic (§1.1). |
| LWT | There is no generic `messaging.lwt`, and no configurable `component.instances[site].lwt`. The first-party LWT is derived internally by `uns-bridge` as `MqttLastWill`; retain is hard-wired `false` and the will is never routed through `publish()` | The bridge registers the derived will only on **its site connection**, landing on the site broker - exactly the load-bearing D9/§9.3 use. |
| Reply topics | `edgecommons/reply-<uuid>` (`request_reply.rs:44`), non-`ecv1` ⇒ structurally guard-exempt (D‑U6) | The bridge mints device-side reply topics with the same prefix (§3.5). |
| Envelope tags | `MessageTags { extra: BTreeMap<String, serde_json::Value> }` (`message.rs:302–306`) — arbitrary JSON values | The hop tag `tags._relay` can be a JSON array (§3.4). |
| Filters | `Uns::filter(cls, &UnsScope)` appends `/#` for channeled classes (`uns.rs:392–404`); `UnsScope::all()/device()` (`uns.rs:214–222`) | The bridge builds its six uplink filters and its pinned downlink filter through the library, never by hand. |
| Library publishers | heartbeat/state via the crate-private `ReservedMessaging` seam (`heartbeat.rs:180–207`; seam `service.rs:166`) | The bridge's **own** state/metric/cfg appear on the device bus like any component's — and the bridge relays itself (§3.9). |
| Schema | `messaging` section is strict (`additionalProperties:false`) with `local`/`northbound`/`requestTimeoutSeconds`; broker QoS lives inside `local.qos` / `northbound.qos` (`schema/edgecommons-config-schema.json`), and `lwt` is rejected | The site Last-Will is a bridge-private derived value; no canonical `messaging` schema change or bridge instance `lwt` property is used for it. |
| Runtime wiring site | messaging built **before** config; UNS knobs late-bound **after** `Config::from_value` (`lib.rs:358–389`) | Named connections are config-declared, so they are built at exactly that post-config point (§2.3). |

---

## 1. The site-broker uplink — a bridge-owned external connection (reusing the core MQTT objects)

> **Revised 2026-07-03 (user).** The earlier draft made the second connection a *core* feature — a
> shared-schema `messaging.connections` map + a `gg.messaging("name")` API in all four languages. That
> is **dropped**. No component other than the `uns-bridge` needs a second connection, and the bridge is
> Rust — so there is no reason to change the core messaging contract in any language. Per the original
> D‑U17 intent, the site broker is the bridge's **"external system"** (exactly as an OPC UA server is the
> opcua-adapter's), configured in the bridge's OWN component config and built by **reusing the core's
> already-public MQTT objects** — a Rust-component concern, not a library API.

### 1.1 The core exposes the broker/provider pieces the bridge needs

The Rust lib builds its *primary* connection from two already-`pub` calls (`lib.rs:756–768`):

```rust
let provider = Arc::new(MqttProvider::connect(&mc).await?);   // mqtt.rs:124 (pub struct) / :153 (pub connect)
let service  = DefaultMessagingService::new(provider);        // service.rs:280 (pub) / :299 (pub new)
```

The `uns-bridge`, a Rust component depending on the `edgecommons` crate, constructs its **site connection
from its own config** by reusing the core broker shape and MQTT provider. The site Last-Will is not part
of `MessagingConfig`; the bridge derives `MqttLastWill` from its own resolved state topic and calls
`MqttProvider::connect_with_last_will`. This is a Rust provider hook for the bridge site uplink, not a
cross-language component messaging contract. **No `messaging.connections`. No `gg.messaging_named()`. No
canonical-schema delta. No Java/Python/TS LWT config.**

### 1.2 Config — the site broker lives in the bridge's own `component.instances[]`

The bridge declares its site broker as an entry in its **own `component.instances[]`** — the existing
per-instance config surface every component already has, exactly how the opcua-adapter configures its
OPC UA endpoints. The `siteBroker` object reuses the core `mqttBroker`/`BrokerConfig` shape. The site LWT
is not configurable; it is derived internally so the console reachability contract cannot be broken by a
topic typo. **No canonical-schema change**
(`component.instances[]` exists and its per-instance body is permissive). Sketch (finalized in §2.7):

```jsonc
"component": {
  // No `name` key here — the canonical schema's `component` object allows only
  // `global`/`instances` (`additionalProperties:false`); the component's full name comes from
  // the runtime builder (the GG recipe's `ComponentName`, or the HOST CLI invocation), not from
  // config (finalized in §2.7).
  "instances": [
    { "id": "site",
      "siteBroker": { "host": "site-broker.dallas.example", "port": 8883,
                      "credentials": { "certPath": "…", "keyPath": "…", "caPath": "…" } },
      "uplink": { "classes": ["state","cfg","evt","metric","data","log"], "rateCaps": { } } }
  ]
}
```

The bridge derives the LWT topic from the sanitized device token and its canonical UNS state topic, so no
template substitution is exposed for the will.

### 1.3 The reserved-class guard is simply not in the path (raw-provider relay)

A relay republishes other components' `state`/`metric`/`cfg`/`log` verbatim — which the
`DefaultMessagingService` guard would reject. There is **no conflict**, because the bridge relays at the
raw `MessagingProvider` level (`provider.publish(topic, bytes)` / `subscribe`), which carries no guard —
byte relay, not a client-chosen enveloped publish. The bridge holds `Arc<MqttProvider>` (site) plus its
device-bus provider and moves bytes between them; the guard (a `DefaultMessagingService` concern) never
sees the relay. **No per-connection guard flag, no `guardReservedClasses` config, no `ReservedMessaging`
seam change.** The site broker's per-device ACL (§5.4) remains the durable boundary.

### 1.4 Connect semantics — non-fatal uplink (bridge-owned)

The site link is intermittent by design (edge-first): the bridge must come up and serve the device bus
while the WAN is down. `MqttProvider::connect` blocks up to 10 s for CONNACK and errors
(`mqtt.rs:369–388`) — fine for the device bus, wrong for an uplink. The bridge **retries the site
connect in its own loop** (pure component code, no library change); the existing CONNACK handler
re-subscribes every registered filter on each connect (`mqtt.rs:318–329`), so reconnection is
transparent. `MessagingProvider::connected()` (`service.rs:666`) drives the uplink policy (§2.6). *(If a
non-blocking `MqttProvider::connect_background` later proves cleaner, it is an additive Rust-only helper —
still no contract change; but the component-owned retry loop needs nothing new.)*

---

## 2. The `uns-bridge` component (M1)

One `uns-bridge` per **device bus** (not per component): an envelope-aware relay between the
device-local bus and the site UNS broker. Rust; own repo (§4); a normal edgecommons component —
it scaffolds, configures, health-checks, and observes itself like any other.

### 2.1 The two connections, per platform

| Platform | PRIMARY = `gg.messaging()` (device bus) | SITE = the bridge's own `Arc<MqttProvider>` (reused core MQTT, from `component.instances[]`) |
|---|---|---|
| **GREENGRASS** | Greengrass IPC (`--platform GREENGRASS --transport IPC`) | site EMQX over MQTT(S) |
| **HOST** | device-local MQTT broker (`--transport MQTT <local.json>`) | site EMQX over MQTT(S) |
| **KUBERNETES** (cluster-boundary pod only, §5.3) | the in-cluster broker (Service DNS) | the site/enterprise broker outside the cluster |

The primary is the device bus **by construction**, not by convention: the library's own machinery
(heartbeat `state` keepalive, `cfg` publisher, `metric` target) publishes on the primary
(`heartbeat.rs:180–207`, `lib.rs:585–595`), so the bridge's own health appears on the device bus
exactly like every other component's — and rides its own relay to the site (§3.9). The connection
name `"site"` is the bridge's documented convention (config key `bridge.siteConnection` allows
renaming; default `"site"`).

### 2.2 The relay matrix — exactly what flows which way

| Direction | Classes | Subscribed on | Filter (built via `uns().filter()`) | Republished on |
|---|---|---|---|---|
| **Uplink** device → site | `state`, `cfg`, `evt`, `metric`, `data`, `log` — the six consumer wildcards (DESIGN-uns §4); **`app` optional, default OFF** (§3.6 policy) | PRIMARY | `ecv1/+/+/+/state` · `ecv1/+/+/+/cfg` · `ecv1/+/+/+/evt/#` · `ecv1/+/+/+/metric/#` · `ecv1/+/+/+/data/#` · `ecv1/+/+/+/log/#` | NAMED, **same topic string** |
| **Downlink** site → device | `cmd` only (incl. broadcast: `+` on the component position matches `_bcast`) | NAMED | `ecv1/{device}/+/+/cmd/#` — **pinned to this bridge's own device token** | PRIMARY, same topic string (after `reply_to` rewrite §3.5) |
| **Reply back-haul** device → site | replies to rewritten `reply_to` topics | PRIMARY (per-request ephemeral subscription) | `edgecommons/reply-<uuid>` (bridge-minted) | NAMED, to the original site-side reply topic |

Explicit non-flows (v1): `cmd` is **never uplinked** (a device component cannot command a peer
across devices through the bridge — cross-device request/reply is deferred; the disjointness of
uplink∩downlink classes is also the structural loop-guard for raw messages, §3.4). Reply topics
never match a UNS filter (non-`ecv1`), so replies only cross via the correlation map. The uplink
uses `+` for the device position (a device bus is by definition this device's traffic, and a HOST
bus carrying two logical identities still uplinks both); the **downlink pins `{device}`** — a
bridge must only pull down commands addressed to *its* device, which also matches its site-broker
ACL scope (§5.4).

**Relay mechanics** (the six + one subscriptions on the service layer):

- The topic already carries `ecv1/{device}/…`, so the relay is **topic-verbatim**: republish to
  the identical topic string on the other connection. No topic parsing, no identity re-stamping —
  the envelope is authoritative (DESIGN-uns §5) and travels untouched except for the hop tag
  (§3.4) and `reply_to` (§3.5).
- **Envelopes** re-serialize structurally identically (D‑U22; serde member order is
  deterministic in Rust). **Raw messages** (`{raw: …}`, `message.rs:308–334`) relay verbatim too —
  they carry no tags to hop-stamp, and are covered by the class-disjointness guard (§3.4).
- Subscriptions use `max_concurrency = 1` (serial, ordered per class — `service.rs:204–210`) and a
  bounded `max_messages` queue per class (defaults: `data` 512, others 64); overflow drops at the
  provider with a warning (`mqtt.rs:345–351`) and is surfaced by the drop counters (§3.6). QoS:
  the service's local publish/subscribe defaults are config-backed via `messaging.local.qos`; a
  per-class QoS override for `data` is a deferred knob.

### 2.3 (§3.4) Hop-tag loop protection — `tags._relay`

A bridge must never re-relay a message it (or a same-tier peer) already relayed — the echo risk is
chained bridges (device → site → enterprise), a second bridge on the same bus pair, or an
operator-added broker-native bridge alongside `uns-bridge`.

- **The marker is a reserved envelope-tag key `_relay`**: a JSON **array of hop identifiers**,
  appended on every relay. `MessageTags.extra` is `BTreeMap<String, serde_json::Value>`
  (`message.rs:302–306`), so an array value is legal in Rust; the other three tag maps carry
  arbitrary JSON the same way (the telemetry-processor "envelope-`tags`-as-JSON-column" precedent
  already depends on that). The `_`-prefix extends the existing reserved-token convention
  (`_bcast`, UNS-CANONICAL §4.3): **tag keys starting with `_` are library/system-reserved** —
  add that one sentence to the identity/tags docs.
- **Hop identifier** = `{device}/{component}` of the relaying bridge (e.g. `"gw-01/uns-bridge"`) —
  unique per bus by construction.
- **Rules**, applied on every would-be relay (both directions):
  1. If `tags._relay` already contains this bridge's own id → **drop silently** (it is our own
     echo), count `relay_loop_dropped`.
  2. If `len(tags._relay) ≥ bridge.maxHops` (default **4**) → drop, count `relay_loop_dropped`
     (defense against a cycle among *distinct* bridges, where rule 1 never fires on the first
     lap's other members).
  3. Else append own id and relay.
- **Raw messages** cannot carry the tag. They are protected structurally: uplink relays only the
  six pub classes, downlink relays only `cmd` — the sets are disjoint, so a single bridge can
  never echo its own raw relay, and chained bridges each apply the same disjointness. A cycle of
  ≥ 2 site brokers bridged to each other *at the same tier* could loop raw messages — that topology
  is explicitly unsupported (documented; the register carries it as the residual gap).
- The console/consumers ignore `_relay` (it is metadata); it is also the observability breadcrumb
  for "which path did this message take".

### 2.4 (§3.5) `reply_to` rewrite — the TTL'd correlation map

Request/reply crossing the bridge breaks without rewriting: a site-side requester (console) sets
`header.reply_to = edgecommons/reply-<uuid>` — an **ephemeral topic on the site broker**
(`request_reply.rs:44–52`). Relayed verbatim, the device-side responder would `reply()` onto the
device bus where nobody is subscribed. The bridge therefore proxies the reply path:

```mermaid
sequenceDiagram
  participant Con as console (site broker)
  participant SB as site broker
  participant BR as uns-bridge
  participant DB as device bus
  participant AD as opcua-adapter
  Con->>SB: cmd reload-config, reply_to = R_site (edgecommons/reply-a1)
  SB->>BR: downlink delivery (filter ecv1/gw-01/+/+/cmd/#)
  Note over BR: mint R_dev = edgecommons/reply-7f3<br/>subscribe R_dev on PRIMARY<br/>map R_dev -> (R_site, now+TTL)<br/>rewrite header.reply_to = R_dev, append hop tag
  BR->>DB: cmd reload-config, reply_to = R_dev
  DB->>AD: deliver; adapter replies via reply()
  AD->>DB: reply on R_dev (correlation_id preserved)
  DB->>BR: delivery on R_dev
  Note over BR: look up R_site; unsubscribe R_dev; remove entry
  BR->>SB: publish reply on R_site
  SB->>Con: ReplyFuture settles
```

Normative points:

- **Outbound (downlink) rewrite** happens only when `header.reply_to` is present (a `cmd` without
  `reply_to` is a notification-style command — normative per §4.3 of the canonical doc — and
  relays untouched, e.g. `set-config` push).
- `R_dev` uses the **standard `edgecommons/reply-` prefix** (`request_reply.rs:44`) so it is
  structurally exempt from the reserved-class guard (D‑U6) and indistinguishable from any other
  reply topic to the responder. The bridge subscribes it via the public
  `messaging().subscribe(R_dev, …, max_messages=1, max_concurrency=1)` and explicitly
  `unsubscribe`s on settle — obeying the "unsubscribe before exit" rule.
- **The reply relays verbatim** — `correlation_id` untouched (the requester's supervisor
  correlates by its ephemeral topic + correlation id; both survive). The reply also gets the hop
  tag appended (it is a relay like any other).
- **The map**: `HashMap<String /*R_dev*/, PendingReply { r_site: String, deadline: Instant }>`
  guarded by a mutex; a single tokio sweep task ticks every `min(ttl/4, 5 s)` and expires entries:
  unsubscribe `R_dev`, drop, count `relay_reply_expired`. **TTL default 60 s**
  (`bridge.reply.ttlSecs`) — 2× the framework request-deadline default of 30 s
  (`model.rs:46`), so the bridge never tears down a reply path before the requester's own deadline
  settles it; deployments that raise `requestTimeoutSeconds` must raise the bridge TTL in step
  (documented as a paired knob).
- **Bound**: `bridge.reply.maxPending` (default **1024**). On overflow, evict the *oldest* entry
  (expire it early, count `relay_reply_expired`) rather than refusing the new command — a stuck
  responder must not starve fresh traffic. Gauge `relay_pending_replies` makes pressure visible.
- First-reply-wins: `max_messages=1` on the `R_dev` subscription gives the same at-most-one-reply
  contract the library's own supervisor has (`REPLY_QUEUE_SIZE`, `service.rs:68`); stragglers drop
  at the provider (debug-logged).

### 2.5 (§3.6) Per-class uplink policy, rate caps, drop counters, disconnect behavior

```jsonc
// component.global.bridge (the bridge's own config subtree — full shape in §3.8)
"uplink": {
  "classes": {
    "state":  { "enabled": true },
    "cfg":    { "enabled": true },
    "evt":    { "enabled": true,  "bufferWhileDisconnected": { "maxMessages": 1000 } },
    "metric": { "enabled": true,  "maxRatePerSec": 50 },
    "data":   { "enabled": true,  "maxRatePerSec": 200, "burst": 400 },
    "log":    { "enabled": false },
    "app":    { "enabled": false }
  }
}
```

- **Defaults**: `state`/`cfg`/`evt`/`metric`/`data` **on**, `log` **off** (off-by-default at the
  source and unbounded-volume), `app` **off** (device-local application chatter; enabling it makes
  the bridge relay a seventh filter `ecv1/+/+/+/app/#`). `data` is the only class rate-capped by
  default.
- **Rate cap** = token bucket per class (`maxRatePerSec` refill, `burst` capacity, default
  `burst = 2×rate`). Exceeding traffic **drops** (never queues — the live UNS path is
  explicitly not durable; durability is the streaming subsystem's job, DESIGN-uns §8), counted per
  class.
- **Disconnect behavior** (`connected() == false` on the site connection): default **drop +
  count** (`onDisconnect: "drop"`). One exception by default: **`evt` gets a small bounded
  drop-oldest replay buffer** (`bufferWhileDisconnected.maxMessages`, default 1000, memory-only) —
  events/alarms are the one class where a WAN blip losing a raise/clear does lasting damage to the
  site view, and 1000 envelopes is trivially cheap. Buffered events replay in order on reconnect,
  after the rehydration broadcast. Flagged **needs-user** (scope call: evt-only vs none vs all).
- **Reconnect rehydration — the late-join lever lands here** (DESIGN-uns §9.3 layer 2): on each
  site-connection re-establishment (rising edge of `connected()`), the bridge publishes
  `ecv1/{device}/_bcast/main/cmd/republish-state` and `…/republish-cfg` **on the device bus**;
  every component's jittered re-announce then rides the uplink, and the site view rehydrates
  `state`/`cfg` without retain. (This makes the bridge the first consumer of the Phase-3
  `republish-*` broadcast listener + minimal `commands()` scaffolding already planned in the
  canonical build checklist.)
- **Visible drop counters — as `metric`s** through the normal metric subsystem (so they surface on
  the device bus at `ecv1/{device}/uns-bridge/main/metric/…`, get uplinked by the bridge itself,
  and reach whatever metric target is configured):

| Metric (measure names) | Meaning |
|---|---|
| `relay_uplinked` (per-class measures: `state`, `cfg`, `evt`, `metric`, `data`, `log`, `app`) | messages relayed up, per class, per interval |
| `relay_dropped_rate` (per-class) | dropped by the token bucket |
| `relay_dropped_disconnected` (per-class) | dropped because the site uplink was down |
| `relay_loop_dropped` | dropped by the hop-tag guard (§3.4) |
| `relay_reply_expired` · `relay_pending_replies` | correlation-map expiries · current gauge |
| `relay_downlinked` | commands relayed down |
| `site_connected` | 0/1 gauge of the named connection state |

### 2.6 (§3.7) LWT reachability — whole-device UNREACHABLE

The load-bearing D9/§9.3 LWT use, now concrete. The bridge derives it internally for the **site**
connection (§1.1 example); neither generic `messaging.lwt` nor `component.instances[site].lwt` is user
configuration:

- **Topic**: `ecv1/{device}/uns-bridge/main/state` — the bridge's **own UNS state topic** (the
  exact string the committed tests already pin, `config.rs:293–299`, `mqtt.rs:590–601`). There is
  no device-level topic in the grammar; the bridge *is* the device's reachability proxy, so its
  state topic is the device-reachability channel by definition. Consumers (the console FleetModel)
  interpret `uns-bridge` state = `UNREACHABLE` as **whole-device UNREACHABLE** — distinct from a
  single component's `OFFLINE` (stale/STOPPED keepalive relayed earlier).
- **Payload**: `{ "status": "UNREACHABLE" }` — deliberately a **bare JSON object, not an
  envelope**. It is registered at CONNECT and published by the *broker*, so it can carry no honest
  timestamp or uuid; consumers already MUST treat LWT event time = delivery time (§6 of the
  canonical doc). On the wire it deserializes as a raw message (no `header`/`identity` keys —
  `message.rs:398–420`); the topic identifies device+component+class, the body carries the status.
  QoS 1, retain hard-wired `false` (D9).
- **Fires on ANY ungraceful *or* graceful disconnect — and that is correct**: rumqttc tears the
  socket down without an MQTT DISCONNECT when the provider drops, so the will fires even on a
  clean bridge shutdown. Semantically right: a stopped bridge means the device **is** unreachable
  through the site bus, regardless of how politely the bridge exited. The site-side terminal
  sequence on graceful stop is: (possibly) relayed component `STOPPED` states → bridge's own
  best-effort `STOPPED` → broker-published `UNREACHABLE` — final state UNREACHABLE, which is the
  truth. Documented as intended (no DISCONNECT-suppression work).
- **No startup cross-check is needed**: the bridge derives the will topic from
  `gg.uns().topic(UnsClass::State)`, the same topic its heartbeat uses. A configured
  `component.instances[site].lwt` is rejected because a typo here would silently break console
  reachability.
- Latency: broker-detected TCP close → immediate will publish; a half-open link resolves at the
  broker's keepalive expiry (provider keepalive 30 s, `mqtt.rs:286` → worst case ~45 s at EMQX's
  1.5× rule). Both far faster and cheaper than consumer-side keepalive-miss inference across every
  component on the device.

### 2.7 (§3.8) The bridge's component config — full shape

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "dallas" },

  "messaging": {
    // PRIMARY: HOST = the device-local broker (this section doubles as the
    // --transport MQTT file shape); GREENGRASS uses the same HOST/MQTT shape against
    // a device-local broker. No `connections` — the site broker is NOT a core messaging feature (§1).
    "local": { "host": "localhost", "port": 1883, "clientId": "uns-bridge-{ThingName}" },
    "requestTimeoutSeconds": 30
  },

  "heartbeat": { "enabled": true, "intervalSecs": 5 },
  "metricEmission": { "target": "messaging" },

  "component": {
    // No `name` key here — the canonical schema's `component` object is `{global, instances}`
    // only, `additionalProperties:false` (`schema/edgecommons-config-schema.json`); the component's
    // full name is supplied by the runtime builder, not by config (see the prose below).
    //
    // The SITE broker is the bridge's EXTERNAL SYSTEM, declared as an instance (like an
    // adapter's OPC UA endpoints). The bridge builds it via the reused core MqttProvider
    // (§1.1) — NOT a core `messaging.connections` feature; no shared-schema change.
    "instances": [
      { "id": "site",
        "siteBroker": { "host": "site-broker.dallas.example", "port": 8883,
                        "clientId": "uns-bridge-{ThingName}",
                        "credentials": { "certPath": "…", "keyPath": "…", "caPath": "…" } },
        "uplink": { /* §3.6 policy block */ },
        "reply":  { "ttlSecs": 60, "maxPending": 1024 },   // §3.5
        "maxHops": 4,                                       // hop-tag cap (§3.4)
        "queue":  { "data": 512, "default": 64 } }          // per-class max_messages
    ]
  }
}
```

Device identity comes from the standard chain (`-t` ▸ platform env ▸ …, D‑U1) — the bridge adds no
identity config of its own. The component's full name (e.g. `com.mbreissi.edgecommons.UnsBridge`) is supplied
by the runtime builder — the GG recipe's `ComponentName`, or the HOST/KUBERNETES invocation — never
by this config file (`component.name` is not a legal key, above). It is chosen so the sanitized
short name (the UNS token, D‑U18) is exactly **`uns-bridge`** — the token the derived LWT topic, the
console, and this document all assume.

### 2.8 (§3.9) The bridge's own observability + command inbox

Nothing bespoke: heartbeat publishes the bridge's `state` keepalive on the **device bus** via the
library seam (`heartbeat.rs:180–207`); the metric subsystem emits the §3.6 counters; the `cfg`
publisher announces its (redacted) effective config. All of it matches the uplink filters and is
**relayed by the bridge itself** (own-message delivery is inherent to MQTT subscriptions), so the
site broker sees the bridge exactly as it sees any component — plus the private derived LWT that only it sets. The
bridge also subscribes its own command inbox `ecv1/{device}/uns-bridge/+/cmd/#` on the device bus
(the standard Flow-B pattern), so a console `ping`/`describe`/policy verb round-trips through the
same downlink + reply-rewrite path as commands to any other component — the relay needs no
self-special-case.

---

## 3. Where the `uns-bridge` lives (D‑B6)

**Recommendation: a NEW sibling repo `edgecommons/uns-bridge`** — not inside the edgecommons
monorepo.

- **It is the org model.** Components live as flat-named sibling repos pinned to the library by
  git rev (`../CLAUDE.md`): `telemetry-processor` (Rust, the direct precedent), `opcua-adapter`,
  `modbus-adapter`. The monorepo holds libraries, templates, schema, CLI, docs — no components.
  "Tightly lib-coupled" does not distinguish the bridge: telemetry-processor is precisely as
  coupled, and the rev-pin bump train (library `main` first, then one migration PR per component)
  is the established mechanism.
- **Everything downstream keys on the registry**: `registry/components.json` drives
  `edgecommons registry list`, the org profile tables, and `clone.sh` (which auto-clones every
  registry component — verified: it iterates `components[].repo`). The docs site syncs each
  component's `docs/` (`website/scripts/sync-component-docs.mjs`). An in-monorepo bridge would be
  the one component invisible to all of that.
- **Local dev** uses the telemetry-processor pattern: a gitignored `.cargo/config.toml` `paths`
  override pointing at `../core/libs/rust`, so local builds use the sibling library while CI
  uses the pinned rev.

**Status update (2026-07-03): the repo now exists** — `git init`'d locally at
`C:\Users\breis\source\edgecommons\uns-bridge`, fully scaffolded and implemented through P3-6 (relay
engine, reply proxy, uplink policy, observability, deploy recipes, a passing dual-EMQX e2e suite).
The org-integration items below are now **done**:
1. **Pushed to GitHub** as `edgecommons/uns-bridge` (`origin` configured; `origin/main` published).
2. **Registry entry** added to `registry/components.json`: `name: uns-bridge`, `repo: edgecommons/uns-bridge`,
   `language: RUST`, **`category: "bridge"`** (the profile generator renders categories generically),
   `platforms: [GREENGRASS, HOST, KUBERNETES]`, `library: edgecommons`.
3. `clone.sh` needs **no change** (registry-driven) — the entry above drives it. Reusable CI
   (`component-ci.yml` from `edgecommons/.github`, plus the two-broker compose for the integration
   job §6) is wired in the repo's own `.github/`.
4. `Cargo.toml` pins `edgecommons = { git =
   "https://github.com/edgecommons/edgecommons.git", rev = "b1d8d85…", default-features = false }` — the
   **v0.2.0 UNS release on `main`**, which contains the UNS core, so a pure git-rev build resolves it;
   local dev still builds against the sibling checkout via a gitignored `.cargo/config.toml` `[patch]`
   override.

What genuinely **remains** before general release: the **edge-console** (its first site-side client)
and the **GREENGRASS/IPC** bridge variant (today only the HOST/KUBERNETES dual-MQTT variant is built).

---

## 4. Site-broker recipes (M2)

The site broker is **EMQX** everywhere (the ecosystem's established broker: test-infra, k8s
manifests, lab). Recipes live **in the `uns-bridge` repo under `deploy/site-broker/`** (broker and
bridge deploy as a pair; the docs-site sync publishes them) — D‑B13.

### 4.1 HOST — Docker on the gateway box

`deploy/site-broker/compose.yaml`, structurally the monorepo's `test-infra/compose.yaml` (EMQX,
1883 plaintext / 8883 mTLS / 18083 dashboard, `gen-tls-certs.sh` for the CA + server cert) plus
the authz file of §4.4 mounted at `/opt/emqx/etc/acl.conf`. One instance on the site gateway; every
device's bridge points its `connections.site.local` at it. Production posture: 8883 mTLS with
per-device client certs; 1883 disabled or firewalled to the gateway host.

### 4.2 GREENGRASS — a GG-managed container on the gateway core

A Greengrass component `com.mbreissi.edgecommons.SiteBroker` whose recipe runs the same compose file via the
stock `aws.greengrass.DockerApplicationManager`:

```yaml
# recipe.yaml (sketch)
ComponentDependencies:
  aws.greengrass.DockerApplicationManager: { VersionRequirement: ~2.0.0 }
Manifests:
  - Platform: { os: linux }
    Artifacts:
      - URI: "docker:emqx/emqx:5"          # image pinned in the real recipe
      - URI: "s3://…/site-broker-config.zip"   # acl.conf + certs + compose.yaml
    Lifecycle:
      run: docker compose -f {artifacts:decompressedPath}/site-broker/compose.yaml up
      shutdown: docker compose -f {artifacts:decompressedPath}/site-broker/compose.yaml down
```

The gateway core also runs its own `uns-bridge` (its local IPC bus is a device bus like any
other). Note the deployment ordering is loose by design: bridges background-connect (§1.3), so
broker-after-bridge start order works.

### 4.3 KUBERNETES — the in-cluster broker IS the aggregation point

Inside a cluster there is **no bridge**: every component already shares the one in-cluster broker
(`emqx.mqtt.svc.cluster.local` — the shape the messaging config tests pin, `config.rs:191–202`),
so aggregation is native (DESIGN-uns §9.2). The recipe here is documentation plus:

- the existing EMQX Deployment/Service manifests (test-infra `k8s/` as the base) with the §4.4
  ACL ConfigMap;
- **a bridge pod only at a cluster boundary** — when the cluster is one line of a bigger site: a
  single-replica Deployment of `uns-bridge` with PRIMARY = the in-cluster broker Service DNS and
  `connections.site` = the external site broker; config via the standard `CONFIGMAP` source;
  identity via the Downward API. Single-replica is a correctness requirement (two bridges on the
  same bus pair duplicate every message — §3.4 note), so `replicas: 1` + `strategy: Recreate`.

### 4.4 Broker-side ACLs — the durable enforcement (DESIGN-uns §7.5 pt 3)

Two principals; usernames from mTLS cert CN. EMQX 5 authz file sketch:

```erlang
%% Device bridges: username == the device token (CN=gw-01).
{allow, all, publish,   ["ecv1/${username}/#", "edgecommons/+"]}.      % own subtree + reply back-haul
{allow, all, subscribe, ["ecv1/${username}/+/+/cmd/#", "edgecommons/+"]}.  % own downlink cmds + (defensive) replies
%% Site consumers (console/historian/MES; CN in a consumer group — separate listener or user prefix):
%%   subscribe the six class wildcards + reply topics; publish cmd + reply topics only.
{allow, {username, {re, "^consumer-"}}, subscribe, ["ecv1/#", "edgecommons/+"]}.
{allow, {username, {re, "^consumer-"}}, publish,   ["ecv1/+/+/+/cmd/#", "ecv1/+/_bcast/main/cmd/#", "edgecommons/+"]}.
{deny, all}.
```

This ACL is what makes the raw-provider relay (§1.3) safe: a bridge can relay **its own device's**
reserved classes and nothing else. (Reply topics are 2-level — `edgecommons/reply-…` — hence the
`edgecommons/+` grants.)

---

## 5. Local testability (two brokers on one dev box)

The existing infra has **one** EMQX (`edgecommons-emqx`, 1883/8883). The bridge needs a device
broker *and* a site broker:

- **`test-infra/compose.dual.yaml` in the `uns-bridge` repo**: two EMQX services —
  `device` (host ports **1883/8883**, reusing the monorepo's cert layout so it can double as the
  standard broker) and `site` (host ports **1884/8884**, container name `uns-bridge-emqx-site`).
  On a machine already running `edgecommons-emqx`, a device-only override reuses it and starts just
  the site broker — the compose file keys both brokers' ports off env vars so the interop machine
  and CI agree.
- **Test pyramid**:
  1. **Unit (the 90 % gate)** — the relay core is deliberately pure-logic over the
     `MessagingService` trait: policy engine (per-class enable + token bucket), hop-tag rules,
     correlation map + TTL sweep, rehydration edge-trigger. Tested against two in-memory fakes
     (the `FakeProvider` pattern, `service.rs:717–769`) wired as "device" and "site" — no broker,
     no clock sleeps (tokio `time::pause`).
  2. **Integration (two real EMQX)** — `tests/` in the bridge repo, gated like the library's
     broker tests; CI job runs the dual compose. Assertions:
     - **Relay**: publish a golden `state` envelope (from `uns-test-vectors/envelopes.json`) on
       the device broker → assert it arrives on the site broker, same topic, structurally equal
       plus `tags._relay == ["<dev>/uns-bridge"]`.
     - **reply_to rewrite round-trip**: fake responder on the device broker
       (`…/comp/main/cmd/ping` → `reply()`); site-side client requests with `reply_to`; assert the
       reply lands on the site-side reply topic with `correlation_id` preserved, **and** that the
       device side observed a *different* `edgecommons/reply-…` topic (the rewrite happened).
     - **TTL expiry**: request with no responder → after `ttlSecs` the `relay_reply_expired`
       metric increments and `relay_pending_replies` returns to 0 (observed via the bridge's
       metric messages on the device broker).
     - **LWT / reachability**: run the bridge as a child process, `SIGKILL` it → the site
       subscriber on `ecv1/<dev>/uns-bridge/main/state` receives `{"status":"UNREACHABLE"}`
       (socket close → immediate will; no keepalive wait). Graceful-stop variant asserts the same
       terminal UNREACHABLE after the best-effort `STOPPED`.
     - **Rate cap**: burst N ≫ cap `data` messages → site receives ≤ `burst + rate·t`, and
       `relay_dropped_rate.data` accounts for the rest.
     - **Loop guard**: publish an envelope pre-stamped with the bridge's own `_relay` id → assert
       it never appears on the site broker; `relay_loop_dropped` increments.
     - **Disconnect/rehydrate**: `docker pause` the site broker → publish states → `unpause` →
       assert the device bus sees `…/_bcast/main/cmd/republish-state` and the site view converges;
       buffered `evt`s replay.
  3. **Interop tie-in (monorepo)** — unchanged single-EMQX interop keeps owning cross-language
     envelope/topic conformance; the bridge repo consumes `uns-test-vectors/` for its golden
     envelopes (the vectors are plain JSON, vendorable at the pinned rev). A later optional
     interop extension runs one bridge under the 4×4 harness with the second broker.
- **Validation matrix fit**: HOST two-broker e2e on the dev box (restart brokers before smokes —
  they crash under build load); GREENGRASS on lab-5950x with the *dev box's* site EMQX as the site
  broker (192.168.1.224 reachable from the lab, the Modbus-sim precedent); the k8s boundary case
  on kind → site broker on the host.

---

## 6. Phase-3 decisions register

| ID | Decision | Resolution (recommended) | Conf. | Reversible? | Needs user? |
|---|---|---|---|---|---|
| D‑B1 | Where the site-broker connection is configured | ✅ **REVISED 2026-07-03 (user), refined 2026-07-06: the bridge's OWN `component.instances[]`** — the site broker is the bridge's external system (like the opcua-adapter's OPC UA endpoints), reusing the existing `MessagingConfig`/`mqttBroker` shape for the endpoint only. The site LWT is private and derived, not configured. **No shared-schema `messaging.connections`; no canonical-schema change.** §1.2 | High | Easy | resolved |
| D‑B2 | How the bridge obtains the 2nd connection | ✅ **REVISED: reuse the core's already-`pub` MQTT objects directly** — `MqttProvider::connect(&site_cfg)` (`mqtt.rs:153`) + raw `MessagingProvider` relay, inside the bridge. **No `gg.messaging_named()`/`gg.messaging("name")` core API in any language.** At most a one-line Rust-only `pub use` re-export for path ergonomics. §1.1 | High | Easy | resolved |
| D‑B3 | Site-connection lifecycle | ✅ **REVISED: bridge-owned.** The bridge builds `MqttProvider::connect(&site_cfg)` and retries in its own loop (non-fatal uplink; CONNACK re-subscribe `mqtt.rs:318` makes reconnection transparent); shutdown = the bridge dropping its handle. **No library lifecycle change.** §1.4 | High | Easy | resolved |
| D‑B4 | Guard vs relay | ✅ **REVISED: no guard flag needed** — the bridge relays at the raw `MessagingProvider` level (byte relay), which carries no reserved-class guard. **No `guardReservedClasses` config, no `ReservedMessaging` seam change.** Site-broker ACL = the durable boundary. §1.3 | High | Easy | resolved |
| D‑B5 | Cross-language impact | ✅ **REVISED: NONE.** No core messaging-contract change in ANY language, no schema change, no Java/Python/TS work, no fast-follow — the whole "named connection in the core" surface is dropped (user, 2026-07-03). Fully honors D‑U17 "no divergence": there is nothing to diverge. §1 | High | Easy | resolved |
| D‑B6 | Where the bridge lives | New sibling repo `edgecommons/uns-bridge` (org model; registry/clone.sh/docs-sync/CI all key on it); monorepo holds no components; `.cargo` paths override for local dev | High | Hard once published | **yes** — repo creation + registry entry are org actions |
| D‑B7 | Relay matrix | Uplink = the six classes (`app` optional, default off) with `+` device; downlink = `cmd` only, **pinned to own `{device}`** (covers `_bcast` via the `+` component position); no cross-device request/reply v1; relay is topic-verbatim | High | Easy | no |
| D‑B8 | Loop protection | Reserved tag key `tags._relay` = JSON array of hop ids (`{device}/uns-bridge`), drop-if-self + `maxHops` (default 4); raw messages covered by uplink/downlink class disjointness; `_`-prefixed tag keys become library-reserved; same-tier broker-to-broker cycles unsupported (documented residual) | Med-High | Easy | no |
| D‑B9 | Reply correlation map | Bridge-minted `edgecommons/reply-` device topics; TTL 60 s (2× the 30 s request default, paired knob), `maxPending` 1024 evict-oldest; expiry unsubscribes + counts; reply relays verbatim (correlation_id preserved) | High | Easy | no |
| D‑B10 | Disconnect durability | Live path drops + counts by default (durability = streaming's job); **exception: bounded `evt` replay buffer (default on, 1000, drop-oldest)**; `state`/`cfg` rehydrate via `republish-*` `_bcast` on reconnect (the §9.3 late-join lever) | Med | Easy | **yes** — scope of the evt buffer (evt-only vs none vs all classes) |
| D‑B11 | LWT | Private bridge-console contract derived by the bridge. Topic = the bridge's own state topic `ecv1/{device}/uns-bridge/main/state`; payload = bare `{"status":"UNREACHABLE"}` (raw, event-time = delivery-time); QoS 1, no retain; fires on graceful stop too — **intended** (a stopped bridge = an unreachable device); no configurable LWT or advisory cross-check | High | Easy | no |
| D‑B12 | Multi-site rooting across the bridge | Relay never rewrites topics; a rooted site broker means components set `topic.includeRoot` end-to-end (D2/D‑U11); bridge-side root injection deferred to an enterprise-tier phase | Med | Moderate | no |
| D‑B13 | Recipe home | `uns-bridge/deploy/site-broker/` (broker+bridge deploy as a pair; docs-site syncs it); EMQX everywhere; ACL file = the durable enforcement | Med-High | Easy | no |
| D‑B14 | Test infra | Dual-EMQX compose in the bridge repo (site broker on 1884/8884; device broker reuses/mirrors `edgecommons-emqx`); relay core is pure-logic over trait fakes for the 90 % gate; e2e list in §6 | High | Easy | no |
| D‑B15 | K8s posture | No bridge inside a cluster (in-cluster broker = aggregation point); boundary bridge = `replicas: 1` + `Recreate` (duplication hazard); CONFIGMAP + Downward-API standard | High | Easy | no |

---

## 7. Phasing — build slices (each independently green: build + tests + gate)

> **Status (updated 2026-07-05):** P3-2 through P3-6 are **done** in the `uns-bridge` repo (folded slightly
> differently than first planned — the repo split P3-4 into P3-4 "uplink policy" and a P3-4b "bridge's
> own observability" slice; see its README for the exact breakdown). Most of P3-6's "org integration"
> is now **done**: the repo is on GitHub as `edgecommons/uns-bridge`, has a `registry/components.json`
> entry, and carries its own CI. What genuinely remains: docs-site sync and the lab/kind validation
> matrix. P3-1 folded into P3-2 as anticipated (no core/other-language change was owed).

| Slice | Contents | Where | Status |
|---|---|---|---|
| **P3-1 site-connection reuse (minimal, Rust-only if anything)** | Verify `MqttProvider`/`DefaultMessagingService`/`MessagingProvider` module paths are reachable from an external crate; add a one-line `pub use` re-export ONLY if needed; smoke-test that a second `MqttProvider::connect` + raw relay works. **No schema, no core API, no other-language change.** Folds into P3-2 if nothing is owed | monorepo (if any) | done (folded into P3-2; nothing was owed) |
| **P3-2 bridge core** | Repo scaffold (`edgecommons/uns-bridge`, rev-pinned, `.cargo` override); relay engine over two `MessagingService` handles: six uplink filters + pinned downlink filter, topic-verbatim republish, hop tag (`_relay`, maxHops); unit (fakes) + dual-EMQX relay/loop e2e | uns-bridge | **done** |
| **P3-3 reply_to rewrite** | Correlation map + TTL sweep + maxPending eviction + reply back-haul; round-trip + expiry e2e | uns-bridge | **done** |
| **P3-4 uplink policy + LWT** | Per-class enable/rate caps/evt buffer; drop-counter metrics; reconnect `republish-*` broadcast (+ the library's Phase-3 `_bcast` listener if not yet landed); private derived site LWT; rate-cap/disconnect/LWT e2e | uns-bridge (+ small monorepo bit for the `_bcast` listener) | **done** (the `_bcast` listener also shipped, all four languages) |
| **P3-5 recipes (M2)** | `deploy/site-broker/`: HOST compose, GG DockerApplicationManager recipe, k8s notes + boundary-bridge Deployment, ACL file, TLS notes; docs pages | uns-bridge | **done** |
| **P3-6 org integration + validation** | Registry entry (`category: bridge`) → profile regen; docs-site sync; validation matrix: HOST dual-broker on dev box, GREENGRASS on lab-5950x (site broker = dev box), kind boundary case | registry / .github / website | **the bridge-level dual-EMQX e2e proof is done** (9/9 assertions) and the registry entry + repo CI have landed; **docs-site sync and the lab/kind validation matrix remain** |
---

## 8. Risks

1. **The site broker MUST enforce a per-device ACL** — the bridge relays other components'
   `state`/`metric`/`cfg`/`log` verbatim to the site broker at the raw `MessagingProvider` level (no
   in-process guard is in the relay path, by design — §1.3). The per-device ACL, not any in-process
   guard, is what confines the bridge to its own `ecv1/{device}/#` subtree (the D‑U4/§7.5 posture).
   The recipe ships ACL-on by default; the docs say plainly: the ACL is the boundary.
3. **Live-path loss during WAN outages is by design** — `data`/`metric`/`log` gaps on the site
   view are permanent (streaming owns durability); only `evt` gets the bounded replay buffer and
   only `state`/`cfg` rehydrate via the broadcast. If site-side historians need lossless
   telemetry, they must consume the streaming path, not the bus — repeat this in the bridge docs.
4. **Duplicate bridges duplicate traffic** — two bridges on one bus pair double-deliver everything
   (hop tags prevent loops, not duplication). Deployment rule: exactly one bridge per device bus;
   k8s boundary bridge pinned to `replicas: 1` + `Recreate`.
5. **Reply-TTL vs raised request timeouts** — a deployment that raises
   `messaging.requestTimeoutSeconds` past 60 s without raising `bridge.reply.ttlSecs` gets
   bridge-expired replies. Paired-knob warning in docs; the bridge could later read the component
   default and floor the TTL at 2× it (cheap follow-up).
6. **Background connect hides misconfig** — a typo'd site-broker host no longer fails the build;
   the bridge runs forever "connected: false". Counter-mitigations: the `site_connected` gauge, a
   WARN log on each failed connect cycle (the provider already warns, `mqtt.rs:359–362`), and the
   console surfacing device UNREACHABLE.
7. **Hop-tag residual** — same-tier broker-to-broker bridges (outside `uns-bridge`) can loop raw
   messages (§3.4). Unsupported topology; documented.
8. **Coverage gate** — the relay core is gate-friendly pure logic, but the e2e surface
   (dual-broker, process-kill LWT) lives outside the gate like the library's broker tests; keep
   the core/IO split disciplined so the 90 % line gate holds on Linux/WSL (Windows undercounts).
9. **EMQX version drift** — recipes pin the image tag; the ACL file syntax is EMQX-5-specific.
   Alternate brokers (Mosquitto etc.) are out of scope for M2; note the LWT + wildcard + mTLS
   requirements for anyone substituting one.

---

## 9. Future enhancements

- **Optional site broker for single-device sites (deferred).** A single-device site has nothing to
  aggregate — one device bus, no fan-in — so the entire site-broker + bridge-uplink layer is dead weight
  there. Future enhancement: a **single-device mode** in which NO separate site broker (and no bridge
  uplink) is deployed; the edge-console attaches **directly to that one device's local broker**. The
  console already anticipates this (its topology decision falls back to the local bus for a single device);
  this formalizes it on the deployment side so a single-device site is not forced to run an extra MQTT
  broker. The `uns-bridge` remains required only when **≥2 device buses** must fan into one site view.
  Work when taken up: a "single-device / no-site-broker" recipe variant + docs making the console's
  device-bus connection the default for that topology. No core/library change — it's a deployment + wiring
  simplification.
