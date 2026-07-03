# ggcommons UNS Phase 3 — named messaging connections, the `uns-bridge`, and the site-broker recipe (PROPOSED)

> **Status: PROPOSED — design for review. No code.** Phase-3 companion to
> [`DESIGN-uns.md`](DESIGN-uns.md) (§9 the site-bus realization) and
> [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) (§2.3 the named connection, §6 LWT, the
> D‑U register). It turns **M8** (named/secondary messaging connection), **M1** (the `uns-bridge`
> component), and **M2** (site-broker deploy recipes) into an implementation-ready design, and
> carries the **Phase-3 decisions register (D‑B1…D‑B15)**.
>
> The UNS core (grammar, identity, `uns()`, reserved-class guard, request deadline, LWT hook,
> library publishers) is **DONE and committed** on `feat/unified-namespace`; every library-seam
> claim below is verified against that HEAD with `file:line` citations into `libs/rust/`.

---

## 0. Grounding — the committed seams this design builds on (verified 2026-07-03)

| Seam | Where (committed source) | What it gives Phase 3 |
|---|---|---|
| Service-over-provider layering | `DefaultMessagingService::new(Arc<dyn MessagingProvider>)` (`libs/rust/src/messaging/service.rs:299`) | A second, fully independent `MessagingService` is just a second `DefaultMessagingService` over a second `MqttProvider` — no new abstraction needed. |
| Reserved-class guard | `check_reserved` on every `publish*`/`request*`/`reply*` (`service.rs:341–353`, predicate `uns::reserved_class_of`, `uns.rs:585`) | Per-*instance* state — a named connection **automatically inherits the guard** (and needs a per-connection opt-out for the relay, §2.4). |
| Request deadline | `set_default_request_timeout` (`service.rs:314`), late-bound from `messaging.requestTimeoutSeconds` (`lib.rs:386–389`, `config/model.rs:497`) | Also per-instance — named connections get the same deadline default bound at build. |
| Guard root binding | `set_guard_include_root` bound to `Config::effective_include_root()` (`service.rs:333`, `model.rs:489`) | Same late-bind applies to each named connection. |
| MQTT provider | `MqttProvider::connect(&MessagingConfig)` — dual-broker, blocks ≤ 10 s for the local CONNACK (`provider/mqtt.rs:153`, `CONNECT_TIMEOUT` `mqtt.rs:64`); re-subscribes every filter on each CONNACK (`mqtt.rs:318–329`) | A named connection reuses this provider verbatim; the CONNACK re-subscribe machinery is what makes a **background** (non-blocking) connect safe (§2.3). |
| LWT | `messaging.lwt` (`messaging/config.rs:52–82`), applied to the provider's **local** connection at CONNECT, retain hard-wired `false`, re-registered on reconnect (`mqtt.rs:156–161`, `build_last_will` `mqtt.rs:398–419`); never routed through `publish()` | A named connection's `lwt` lands on **its** broker — i.e. the bridge's site broker. Exactly the load-bearing D9/§9.3 use. |
| Reply topics | `ggcommons/reply-<uuid>` (`request_reply.rs:44`), non-`ecv1` ⇒ structurally guard-exempt (D‑U6) | The bridge mints device-side reply topics with the same prefix (§3.5). |
| Envelope tags | `MessageTags { extra: BTreeMap<String, serde_json::Value> }` (`message.rs:302–306`) — arbitrary JSON values | The hop tag `tags._relay` can be a JSON array (§3.4). |
| Filters | `Uns::filter(cls, &UnsScope)` appends `/#` for channeled classes (`uns.rs:392–404`); `UnsScope::all()/device()` (`uns.rs:214–222`) | The bridge builds its six uplink filters and its pinned downlink filter through the library, never by hand. |
| Library publishers | heartbeat/state via the crate-private `ReservedMessaging` seam (`heartbeat.rs:180–207`; seam `service.rs:166`) | The bridge's **own** state/metric/cfg appear on the device bus like any component's — and the bridge relays itself (§3.9). |
| Schema | `messaging` section is strict (`additionalProperties:false`) with `local`/`iotCore`/`requestTimeoutSeconds`/`lwt` (`schema/ggcommons-config-schema.json:304–346`) | The new `connections` key must be added to the **canonical** schema (one file, synced 4-ways) — there is no Rust-only schema. |
| Runtime wiring site | messaging built **before** config; UNS knobs late-bound **after** `Config::from_value` (`lib.rs:358–389`) | Named connections are config-declared, so they are built at exactly that post-config point (§2.3). |

---

## 1. The named/secondary messaging connection (M8 — the D‑U17 implementation)

D‑U17 was resolved 2026-07-02: **a uniform, config-declared named connection in all four
languages — no per-language imperative divergence** (`UNS-CANONICAL-DESIGN.md` §2.3). This section
fixes the three open sub-decisions: the config shape and where it lives, the runtime API, and the
per-language scope for Phase 3.

### 1.1 Config shape — a shared-schema `messaging.connections` map (D‑B1)

**Recommendation: a new `connections` object inside the SHARED `messaging` section, keyed by
connection name, each value having the *same shape as the `messaging` section itself* (minus
`connections`).** A named connection is literally "another `MessagingConfig`":

```jsonc
"messaging": {
  "local":   { "host": "localhost", "port": 1883, "clientId": "uns-bridge-{ThingName}" },
  "requestTimeoutSeconds": 30,

  "connections": {
    "site": {                                       // the name passed to gg.messaging("site")
      "local": {                                    // the named connection's broker
        "host": "site-broker.dallas.example", "port": 8883,
        "clientId": "uns-bridge-{ThingName}",
        "credentials": { "certPath": "…", "keyPath": "…", "caPath": "…" }
      },
      "lwt": {                                      // per-connection LWT — lands on THIS broker
        "topic": "ecv1/{ThingName}/uns-bridge/main/state",
        "payload": { "status": "UNREACHABLE" },
        "qos": 1
      },
      "requestTimeoutSeconds": 30,                  // optional; default = the component's value
      "guardReservedClasses": false                 // §2.4 — relay uplinks NEED this (default true)
    }
  }
}
```

Rationale, and why **not** the alternatives:

- **vs the bridge's own component config (DESIGN-uns §9.1's original "no schema change")** — that
  was the pre-resolution position; D‑U17's resolution explicitly supersedes it ("*the bridge
  declares its site-broker uplink as a named messaging connection in config … the library
  provisions + manages it*"). A component-private shape would also mean the bridge hand-constructs
  a `MessagingConfig` + `MqttProvider` + `DefaultMessagingService` itself — bypassing the library's
  connect/LWT/guard/deadline wiring and re-creating it in component code. The library-managed path
  is 20 lines of `build()` wiring; the component-managed path is a copy of the library's
  transport-injection site living outside the library.
- **Map vs array (`connections[]` / `uplinks[]`)** — a map keyed by name gives collision-free names
  for free (a JSON object cannot carry duplicate keys), direct lookup, and reads naturally in
  config review. Names validate against `^[A-Za-z0-9_-]+$` (the D‑U10 level-name rule); the name
  `default` is **reserved** (startup error) so nobody shadows the unnamed primary.
- **Distinct from `component.instances[]` / `gg.instance()` (D‑U3)** — stated normatively: the
  per-message **instance** token addresses *message identity* (who a message is about); a named
  **connection** addresses *transport* (which broker it rides). They never interact: an
  instance-scoped envelope publishes over whichever connection the caller chose. The schema keeps
  them in different sections (`component.instances[]` vs `messaging.connections`) and the docs must
  say this sentence verbatim.
- **Reuse of the existing shape** — each connection value deserializes into the existing
  `Messaging` struct (`messaging/config.rs:52`): `local` (required), `iotCore` (optional, rarely
  used on a named connection), `lwt` (optional). Two additive per-connection keys:
  `requestTimeoutSeconds` (override; absent = inherit the component-level value) and
  `guardReservedClasses` (§2.4). Nothing else is invented.
- **Template substitution is normative**: every string in a connection object passes the standard
  template resolution (`{ThingName}`, `{ComponentName}`, … — `config::template::resolve`, already
  used for vault/cache paths at `lib.rs:469,501`) before use. That is what makes the LWT topic
  (`ecv1/{ThingName}/…`) and per-device `clientId` writable once in a fleet-shared config. Note the
  sanitizer caveat: `{ThingName}` substitutes the **sanitized** thing name, which is also the UNS
  device token — so the substituted LWT topic matches the bridge's real state topic (§3.7 adds a
  startup cross-check).

Schema delta (canonical `schema/ggcommons-config-schema.json`, then `schema/sync-schema.sh`):
`messaging.properties` += `connections` (type object, `patternProperties` `^[A-Za-z0-9_-]+$` →
`$ref: #/definitions/namedConnection`, `additionalProperties:false`); new definition
`namedConnection` = `{ local (required, $ref mqttBroker), iotCore, lwt (same shape as
messaging.lwt), requestTimeoutSeconds, guardReservedClasses (boolean, default true) }`,
`additionalProperties:false`. The `messaging` section's own `additionalProperties:false`
(`schema:345`) stays — the drift gate keeps all four copies identical.

### 1.2 Runtime API (Rust now) — config-declared retrieval, not an imperative open (D‑B2)

**Recommendation: retrieval-by-name of a config-declared connection.** There is **no**
`gg.messaging_named(name, cfg)`-style imperative open: passing a `MessagingConfig` at runtime is
exactly the per-language imperative API D‑U17 rejected, and it would create connections the
library cannot describe from config alone (breaking the "config-review shows the topology"
property the `cfg` publisher provides).

Cross-language contract: `gg.messaging()` = the primary (unnamed) connection, unchanged;
`gg.messaging("<name>")` = the named connection. In Rust (no overloading):

```rust
impl GgCommons {
    /// The primary/default messaging connection (unchanged). lib.rs:176
    pub fn messaging(&self) -> Result<Arc<dyn MessagingService>>;

    /// A config-declared named connection (messaging.connections.<name>), fully
    /// independent of the primary: its own MqttProvider (own broker, own LWT), its
    /// own guard + request-deadline state. Errors when the name is not declared.
    pub fn messaging_named(&self, name: &str) -> Result<Arc<dyn MessagingService>>;
}
```

Java: `getMessaging()` / `getMessaging(String name)`. Python: `gg.messaging(name: str | None =
None)` — the static/global `MessagingClient` becomes a keyed registry (default + named), exactly
as §2.3 of the canonical doc anticipates. TS: `gg.messaging(name?: string)`.

Error contract: an undeclared name is `GgError::Messaging("no named messaging connection '<x>' —
declare it under messaging.connections")` (Java `IllegalArgumentException`, Python `KeyError`-style
`ValueError`, TS `Error`) — **fail loud at retrieval**, not a silent `None`.

### 1.3 Construction, lifecycle, shutdown (D‑B3)

Named connections are built inside `GgCommonsBuilder::build()` **immediately after
`Config::from_value` + the existing UNS late-binds** (`lib.rs:377–389`) — they are config-declared,
so unlike the primary they *cannot* exist before config:

1. Parse `messaging.connections` from the typed section (`MessagingSectionConfig` gains
   `connections: Option<BTreeMap<String, Value>>`; each value → template-resolve → deserialize as
   `Messaging` + the two extra keys).
2. For each entry, build an `MqttProvider` and wrap it in a `DefaultMessagingService`, then bind
   the same knobs the primary gets: `set_default_request_timeout(connection override ▸ component
   value)` and `set_guard_include_root(cfg.effective_include_root())`.
3. Store as `named_messaging: HashMap<String, Arc<DefaultMessagingService>>` on `GgCommons`.
   **Shutdown registration is RAII, same as everything else in the runtime**: dropping `GgCommons`
   drops the map → `DefaultMessagingService::drop` aborts all dispatchers (`service.rs:511`) →
   `BrokerConn::drop` aborts the event-loop task (`mqtt.rs:110`) → the socket closes **without an
   MQTT DISCONNECT**, so a registered LWT fires (§3.7 makes this a feature, not a bug).

**Connect semantics differ from the primary (deliberately):** `MqttProvider::connect` blocks up to
10 s for the CONNACK and errors out (`mqtt.rs:369–388`) — right for the device-local bus (a
component without its local bus is useless), wrong for an **uplink**: the site link is
intermittent by definition (edge-first, `README.md` §connectivity), and a bridge must come up and
serve the local bus while the WAN is down. So named connections use a new
`MqttProvider::connect_background(&MessagingConfig)` that starts the event loop and returns
**without waiting for CONNACK**. This is safe with zero new machinery: subscriptions register in
the routing registry immediately and the existing CONNACK handler re-subscribes every registered
filter on (each) connect (`mqtt.rs:318–329`); the SUBACK waiter already degrades to a warning
(`mqtt.rs:241–244`). `MessagingService::connected()` (`service.rs:666`) exposes the live state —
the bridge's uplink policy keys off it (§3.6). Publishes while disconnected fail/drop with the
existing error path — counted, never buffered by the transport (§3.6 owns that policy).

**MQTT-only, and why that's fine on GREENGRASS:** a named connection is always an `MqttProvider`
(requires the `standalone` cargo feature — the default). There is no "second IPC bus" to name: IPC
is per-Nucleus and singular. On GREENGRASS the primary is IPC and the named connection is MQTT —
exactly the bridge's topology (§3.1). A GREENGRASS bridge build composes
`--features greengrass` (Linux/WSL) with the default `standalone`.

**Hot reload:** v1 resolves connections **once at build**. A changed `messaging.connections` on
hot reload logs `WARN named messaging connections changed — restart required` (the internal
config-change listener that already reconfigures metrics/logging, `lib.rs:579–583`, gains this
check). Dynamic connection add/remove is deferred with the dynamic-streams precedent.

### 1.4 Per-connection guard opt-out — `guardReservedClasses` (D‑B4)

The relay has a structural collision with the reserved-class guard: the bridge republishes **other
components' `state`/`metric`/`cfg`/`log` messages verbatim** to the site broker, and every
`DefaultMessagingService` rejects client-chosen reserved topics (`service.rs:341–353`). The
crate-private `ReservedMessaging` seam (`service.rs:166`) is deliberately unreachable from another
crate — the bridge is a separate component and **must not** get a public forgery API.

**Recommendation: a per-connection config key `guardReservedClasses` (default `true`);
`false` builds that one `DefaultMessagingService` with the guard disabled.** Grounds:

- The guard is **misuse prevention, not a security boundary** (D‑U4/D‑U24; broker ACLs are the
  durable enforcement, DESIGN-uns §7.5 pt 3). A relay uplink is the one legitimate "publishes
  other components' reserved classes" role, and it is *declared in config* — visible in config
  review and in the `cfg` effective-config announcement, not hidden in code.
- Scope is minimal: the **primary connection never gets the opt-out** (the key exists only under
  `connections.<name>`), so no component can quietly forge on the device bus it shares with
  others. On the site broker the bridge's ACL (§5.4) confines it to `ecv1/{device}/#` — it can
  relay its own device, never forge another's.
- Implementation: `DefaultMessagingService` gains a `guard_enabled: AtomicBool` (default `true`);
  `check_reserved` short-circuits when disabled; the named-connection wiring is the **only** caller
  that sets it, from config. The `ReservedMessaging` seam is untouched.

Flagged **needs-user** in the register (security-adjacent knob), with the recommendation to accept.

### 1.5 Language scope — schema uniform now, runtime Rust now, mirrors fast-follow (D‑B5)

- **The config contract ships uniform, immediately and unavoidably**: there is one canonical
  schema synced into all four libraries (`schema/sync-schema.sh`; drift-gated in CI) — adding
  `messaging.connections` for Rust *is* adding it for all four. All four validators accept it from
  day one.
- **The runtime lands in Rust now** (the bridge is Rust and is the only consumer in Phase 3).
- **Java/Python/TS runtime wiring is a deferred fast-follow parity slice** — a tracked parity item
  (the four-way-parity rule applies; it is *sequenced by need*, not waived): Java
  `getMessaging(String)`, Python keyed registry, TS `messaging(name?)`. Until wired, each of the
  three logs a startup **WARN** when `messaging.connections` is present: *"named messaging
  connections are declared but not yet supported in this library — declared connections are
  inert"*. Silent acceptance of a schema-valid-but-ignored section is the one dishonest outcome
  this design refuses.

**Honest reconciliation with D‑U17 "no divergence":** the resolution's substance was *no API-shape
divergence* — one uniform config-declared mechanism, identical retrieval surface. This design
keeps that: the shape, names, semantics, and schema are identical 4-ways from day one; only the
*implementation order* is staggered, with an explicit WARN in the unwired languages and a named
parity slice (P3-L, §8) to close it. If the user wants strict simultaneity instead, slice P3-L
moves before the bridge — at the cost of ~3 language-ports of work with zero consumers. Flagged
**needs-user**.

---

## 2. The `uns-bridge` component (M1)

One `uns-bridge` per **device bus** (not per component): an envelope-aware relay between the
device-local bus and the site UNS broker. Rust; own repo (§4); a normal ggcommons component —
it scaffolds, configures, health-checks, and observes itself like any other.

### 2.1 The two connections, per platform

| Platform | PRIMARY = `gg.messaging()` (device bus) | NAMED = `gg.messaging_named("site")` |
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
| **Reply back-haul** device → site | replies to rewritten `reply_to` topics | PRIMARY (per-request ephemeral subscription) | `ggcommons/reply-<uuid>` (bridge-minted) | NAMED, to the original site-side reply topic |

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
  the service's local publish path is fixed at QoS 1 (`LOCAL_QOS`, `service.rs:66`); a per-class
  QoS-0 option for `data` is a deferred knob.

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
`header.reply_to = ggcommons/reply-<uuid>` — an **ephemeral topic on the site broker**
(`request_reply.rs:44–52`). Relayed verbatim, the device-side responder would `reply()` onto the
device bus where nobody is subscribed. The bridge therefore proxies the reply path:

```mermaid
sequenceDiagram
  participant Con as console (site broker)
  participant SB as site broker
  participant BR as uns-bridge
  participant DB as device bus
  participant AD as opcua-adapter
  Con->>SB: cmd reload-config, reply_to = R_site (ggcommons/reply-a1)
  SB->>BR: downlink delivery (filter ecv1/gw-01/+/+/cmd/#)
  Note over BR: mint R_dev = ggcommons/reply-7f3<br/>subscribe R_dev on PRIMARY<br/>map R_dev -> (R_site, now+TTL)<br/>rewrite header.reply_to = R_dev, append hop tag
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
- `R_dev` uses the **standard `ggcommons/reply-` prefix** (`request_reply.rs:44`) so it is
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

The load-bearing D9/§9.3 LWT use, now concrete. Declared entirely in config on the **site**
connection (§1.1 example) — the committed `messaging.lwt` shape and provider wiring are reused
verbatim (`config.rs:64–82`, `mqtt.rs:279–297,398–419`):

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
- **Startup cross-check** (cheap, catches the classic misconfig): the bridge compares the
  template-resolved `connections.site.lwt.topic` against its own `gg.uns().topic(UnsClass::State)`
  and logs **WARN on mismatch** (e.g. an unsanitized literal device token). Config remains
  authoritative — the check is advisory.
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
    // --transport MQTT file shape); GREENGRASS = absent (IPC).
    "local": { "host": "localhost", "port": 1883, "clientId": "uns-bridge-{ThingName}" },
    "requestTimeoutSeconds": 30,
    "connections": {
      "site": {
        "local": { "host": "site-broker.dallas.example", "port": 8883,
                   "clientId": "uns-bridge-{ThingName}",
                   "credentials": { "certPath": "…", "keyPath": "…", "caPath": "…" } },
        "lwt": { "topic": "ecv1/{ThingName}/uns-bridge/main/state",
                 "payload": { "status": "UNREACHABLE" }, "qos": 1 },
        "guardReservedClasses": false
      }
    }
  },

  "heartbeat": { "enabled": true, "intervalSecs": 5 },
  "metricEmission": { "target": "messaging" },

  "component": {
    "global": {
      "bridge": {
        "siteConnection": "site",          // which named connection is the uplink
        "maxHops": 4,                      // hop-tag cap (§3.4)
        "reply": { "ttlSecs": 60, "maxPending": 1024 },   // §3.5
        "uplink": { /* §3.6 policy block */ },
        "queue":  { "data": 512, "default": 64 }          // per-class max_messages
      }
    }
  }
}
```

Device identity comes from the standard chain (`-t` ▸ platform env ▸ …, D‑U1) — the bridge adds no
identity config of its own. The component's full name is chosen so the sanitized short name (the
UNS token, D‑U18) is exactly **`uns-bridge`** (e.g. `com.mbreissi.uns-bridge`) — the token the
LWT topic, the console, and this document all assume.

### 2.8 (§3.9) The bridge's own observability + command inbox

Nothing bespoke: heartbeat publishes the bridge's `state` keepalive on the **device bus** via the
library seam (`heartbeat.rs:180–207`); the metric subsystem emits the §3.6 counters; the `cfg`
publisher announces its (redacted) effective config. All of it matches the uplink filters and is
**relayed by the bridge itself** (own-message delivery is inherent to MQTT subscriptions), so the
site broker sees the bridge exactly as it sees any component — plus the LWT that only it sets. The
bridge also subscribes its own command inbox `ecv1/{device}/uns-bridge/+/cmd/#` on the device bus
(the standard Flow-B pattern), so a console `ping`/`describe`/policy verb round-trips through the
same downlink + reply-rewrite path as commands to any other component — the relay needs no
self-special-case.

---

## 3. Where the `uns-bridge` lives (D‑B6)

**Recommendation: a NEW sibling repo `edgecommons/uns-bridge`** — not inside the ggcommons
monorepo.

- **It is the org model.** Components live as flat-named sibling repos pinned to the library by
  git rev (`../CLAUDE.md`): `telemetry-processor` (Rust, the direct precedent), `opcua-adapter`,
  `modbus-adapter`. The monorepo holds libraries, templates, schema, CLI, docs — no components.
  "Tightly lib-coupled" does not distinguish the bridge: telemetry-processor is precisely as
  coupled, and the rev-pin bump train (library `main` first, then one migration PR per component)
  is the established mechanism.
- **Everything downstream keys on the registry**: `registry/components.json` drives
  `ggcommons list-components`, the org profile tables, and `clone.sh` (which auto-clones every
  registry component — verified: it iterates `components[].repo`). The docs site syncs each
  component's `docs/` (`website/scripts/sync-component-docs.mjs`). An in-monorepo bridge would be
  the one component invisible to all of that.
- **Local dev** uses the telemetry-processor pattern: a gitignored `.cargo/config.toml` `paths`
  override pointing at `../ggcommons/libs/rust`, so local builds use the sibling library while CI
  uses the pinned rev.

**Creation checklist** (org actions — the repo does not exist yet):
1. `git init` + GitHub repo `edgecommons/uns-bridge` (scaffold via
   `ggcommons create-component -n com.mbreissi.uns-bridge -l RUST --platforms GREENGRASS,HOST,KUBERNETES`,
   then the bridge-specific code).
2. Registry entry in `registry/components.json`: `name: uns-bridge`, `repo: edgecommons/uns-bridge`,
   `language: RUST`, **`category: "bridge"`** (new category; the profile generator renders
   categories generically), `platforms: [GREENGRASS, HOST, KUBERNETES]`, `library: ggcommons`,
   topics `[edgecommons, aws-iot-greengrass, iiot, uns, mqtt-bridge, edgecommons-bridge]`.
3. `clone.sh` needs **no change** (registry-driven). Reusable CI: `component-ci.yml` from
   `edgecommons/.github`, plus the two-broker compose for the integration job (§6).
4. `Cargo.toml`: `ggcommons = { git = "https://github.com/edgecommons/ggcommons", rev = "<pin>" }`,
   default features + `greengrass` behind a feature for the GG build.

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

A Greengrass component `com.mbreissi.site-broker` whose recipe runs the same compose file via the
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
{allow, all, publish,   ["ecv1/${username}/#", "ggcommons/+"]}.      % own subtree + reply back-haul
{allow, all, subscribe, ["ecv1/${username}/+/+/cmd/#", "ggcommons/+"]}.  % own downlink cmds + (defensive) replies
%% Site consumers (console/historian/MES; CN in a consumer group — separate listener or user prefix):
%%   subscribe the six class wildcards + reply topics; publish cmd + reply topics only.
{allow, {username, {re, "^consumer-"}}, subscribe, ["ecv1/#", "ggcommons/+"]}.
{allow, {username, {re, "^consumer-"}}, publish,   ["ecv1/+/+/+/cmd/#", "ecv1/+/_bcast/main/cmd/#", "ggcommons/+"]}.
{deny, all}.
```

This is what makes `guardReservedClasses: false` (§1.4) safe: a bridge can relay **its own
device's** reserved classes and nothing else. (Reply topics are 2-level — `ggcommons/reply-…` —
hence the `ggcommons/+` grants.)

---

## 5. Local testability (two brokers on one dev box)

The existing infra has **one** EMQX (`ggcommons-emqx`, 1883/8883). The bridge needs a device
broker *and* a site broker:

- **`test-infra/compose.dual.yaml` in the `uns-bridge` repo**: two EMQX services —
  `device` (host ports **1883/8883**, reusing the monorepo's cert layout so it can double as the
  standard broker) and `site` (host ports **1884/8884**, container name `uns-bridge-emqx-site`).
  On a machine already running `ggcommons-emqx`, a device-only override reuses it and starts just
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
       device side observed a *different* `ggcommons/reply-…` topic (the rewrite happened).
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
| D‑B1 | Named-connection config location + shape | **Shared schema**: `messaging.connections` — an object map keyed by name; each value = the existing `Messaging` shape (`local`/`iotCore`/`lwt`) + `requestTimeoutSeconds` + `guardReservedClasses`; name `default` reserved; values template-resolved. Distinct from `component.instances[]` (identity vs transport, D‑U3). Supersedes DESIGN-uns §9.1's pre-D‑U17 "no schema change" note | High | Moderate (schema key, pre-1.0) | no — direction pre-resolved by D‑U17; shape is the lean reading of it |
| D‑B2 | Retrieval API | Config-declared retrieval only: `gg.messaging("<name>")` (Rust `messaging_named(&str)`); **no** imperative `messaging_named(name, cfg)` open; undeclared name fails loud | High | Easy | no |
| D‑B3 | Named-connection lifecycle | Built in `build()` post-config; **background connect** (never fails the build; CONNACK re-subscribe machinery makes it safe, `mqtt.rs:318`); MQTT-only; knobs (`deadline`, guard root) bound like the primary; RAII shutdown via the `GgCommons` map; hot-reload = WARN restart-required | High | Easy | no |
| D‑B4 | Guard vs relay | Per-connection `guardReservedClasses: false` (named connections only, never the primary); guard = misuse prevention, broker ACL (§5.4) = boundary; `ReservedMessaging` seam untouched | High | Easy | **yes** — security-adjacent knob; recommend accept |
| D‑B5 | Language scope of M8 | Schema + contract uniform now (single synced schema makes this automatic); **runtime Rust now**; Java/Python/TS wiring = named fast-follow parity slice P3-L with an interim startup WARN when `connections` is declared | Med-High | Easy | **yes** — confirm the staggered-wiring window honors D‑U17's intent (or pull P3-L earlier) |
| D‑B6 | Where the bridge lives | New sibling repo `edgecommons/uns-bridge` (org model; registry/clone.sh/docs-sync/CI all key on it); monorepo holds no components; `.cargo` paths override for local dev | High | Hard once published | **yes** — repo creation + registry entry are org actions |
| D‑B7 | Relay matrix | Uplink = the six classes (`app` optional, default off) with `+` device; downlink = `cmd` only, **pinned to own `{device}`** (covers `_bcast` via the `+` component position); no cross-device request/reply v1; relay is topic-verbatim | High | Easy | no |
| D‑B8 | Loop protection | Reserved tag key `tags._relay` = JSON array of hop ids (`{device}/uns-bridge`), drop-if-self + `maxHops` (default 4); raw messages covered by uplink/downlink class disjointness; `_`-prefixed tag keys become library-reserved; same-tier broker-to-broker cycles unsupported (documented residual) | Med-High | Easy | no |
| D‑B9 | Reply correlation map | Bridge-minted `ggcommons/reply-` device topics; TTL 60 s (2× the 30 s request default, paired knob), `maxPending` 1024 evict-oldest; expiry unsubscribes + counts; reply relays verbatim (correlation_id preserved) | High | Easy | no |
| D‑B10 | Disconnect durability | Live path drops + counts by default (durability = streaming's job); **exception: bounded `evt` replay buffer (default on, 1000, drop-oldest)**; `state`/`cfg` rehydrate via `republish-*` `_bcast` on reconnect (the §9.3 late-join lever) | Med | Easy | **yes** — scope of the evt buffer (evt-only vs none vs all classes) |
| D‑B11 | LWT | Topic = the bridge's own state topic `ecv1/{device}/uns-bridge/main/state`; payload = bare `{"status":"UNREACHABLE"}` (raw, event-time = delivery-time); QoS 1, no retain; fires on graceful stop too — **intended** (a stopped bridge = an unreachable device); advisory startup topic cross-check | High | Easy | no |
| D‑B12 | Multi-site rooting across the bridge | Relay never rewrites topics; a rooted site broker means components set `topic.includeRoot` end-to-end (D2/D‑U11); bridge-side root injection deferred to an enterprise-tier phase | Med | Moderate | no |
| D‑B13 | Recipe home | `uns-bridge/deploy/site-broker/` (broker+bridge deploy as a pair; docs-site syncs it); EMQX everywhere; ACL file = the durable enforcement | Med-High | Easy | no |
| D‑B14 | Test infra | Dual-EMQX compose in the bridge repo (site broker on 1884/8884; device broker reuses/mirrors `ggcommons-emqx`); relay core is pure-logic over trait fakes for the 90 % gate; e2e list in §6 | High | Easy | no |
| D‑B15 | K8s posture | No bridge inside a cluster (in-cluster broker = aggregation point); boundary bridge = `replicas: 1` + `Recreate` (duplication hazard); CONFIGMAP + Downward-API standard | High | Easy | no |

---

## 7. Phasing — build slices (each independently green: build + tests + gate)

| Slice | Contents | Where |
|---|---|---|
| **P3-1 named connection** | Canonical schema `messaging.connections` (+`namedConnection` def) + `sync-schema.sh`; Rust: typed section, template resolution, `MqttProvider::connect_background`, `DefaultMessagingService` guard flag, `GgCommons::messaging_named` + RAII map + knob binding + hot-reload WARN; unit tests vs fakes + one dual-broker MQTT integration test; Java/Python/TS: the `connections`-declared-but-inert startup WARN (3 tiny diffs) | monorepo |
| **P3-2 bridge core** | Repo scaffold (`edgecommons/uns-bridge`, rev-pinned, `.cargo` override); relay engine over two `MessagingService` handles: six uplink filters + pinned downlink filter, topic-verbatim republish, hop tag (`_relay`, maxHops); unit (fakes) + dual-EMQX relay/loop e2e | uns-bridge |
| **P3-3 reply_to rewrite** | Correlation map + TTL sweep + maxPending eviction + reply back-haul; round-trip + expiry e2e | uns-bridge |
| **P3-4 uplink policy + LWT** | Per-class enable/rate caps/evt buffer; drop-counter metrics; reconnect `republish-*` broadcast (+ the library's Phase-3 `_bcast` listener if not yet landed); `connections.site.lwt` config + startup cross-check; rate-cap/disconnect/LWT e2e | uns-bridge (+ small monorepo bit for the `_bcast` listener) |
| **P3-5 recipes (M2)** | `deploy/site-broker/`: HOST compose, GG DockerApplicationManager recipe, k8s notes + boundary-bridge Deployment, ACL file, TLS notes; docs pages | uns-bridge |
| **P3-6 org integration + validation** | Registry entry (`category: bridge`) → profile regen; docs-site sync; validation matrix: HOST dual-broker on dev box, GREENGRASS on lab-5950x (site broker = dev box), kind boundary case | registry / .github / website |
| **P3-L parity fast-follow** | Java/Python/TS named-connection runtime (keyed registry etc.), replacing the WARN; parity tests | monorepo |

---

## 8. Risks

1. **Staggered M8 wiring (D‑B5)** — three languages accept `messaging.connections` but ignore it
   until P3-L. Mitigated by the startup WARN and the named parity slice; the residual risk is a
   non-Rust component author reading the (uniform) docs and expecting it to work. Call it out in
   the config-schema reference page ("Rust-only until P3-L").
2. **`guardReservedClasses` is a forgery knob if the broker ACL is skipped** — a misconfigured
   site broker (no per-device ACL) plus an opted-out connection lets one device publish another's
   reserved classes. The recipe ships ACL-on by default and the docs say plainly: the ACL, not the
   guard, is the boundary (this is already the D‑U4 posture; the bridge just raises the stakes).
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
