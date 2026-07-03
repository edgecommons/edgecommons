# ggcommons UNS — Canonical API Design & Decisions Register (implementation companion)

> **Status: implementation-ready (2026-07-02).** Companion to [`DESIGN-uns.md`](DESIGN-uns.md) (the
> approved design). Java canonical; Python/Rust/TS mirror notes inline. Pre-1.0 hard cut; no
> dual-publish. Shared/layered config is **explicitly out of scope** — each component reads its own
> `hierarchy` + `identity` blocks (shared config is a later co-location optimization).
>
> **Provenance:** produced by a Fable design pass over seven code-grounded reader maps (all four cores +
> schema + components + interop). The **Decisions register** below is the running tracker for later
> review.
>
> **Review status (updated 2026-07-02):** all four flagged decisions **resolved with the user**. **D‑U17
> → uniform config-driven named connection, no divergence** (§2.3); **D‑U18 ✅** (component = short name);
> **D‑U19 → component-inbox + broadcast** (§4.3); **D‑U20 ✅** (heartbeat `targets[]` removed; measures
> keep full sink routing via the metric subsystem). **M11 is pulled into Phase 1.**

**Conformance vocabulary:** *topics are byte-identical* across languages; *envelopes are structurally
identical* (same key set, same values; JSON member order is **not** normative — the four serializers
already differ in member order, and the interop harness already compares structurally). Serializers
SHOULD emit the canonical order below for readability; tests assert structural equality (D‑U22).

---

## Canonical API design

### 1. The top-level `identity` element

#### 1.1 Wire shape

The envelope becomes `{header, identity, tags, body}` (raw messages remain `{raw}` and never carry
identity). Canonical member order: `header`, `identity`, `tags`, `body`; inside `identity`: `hier`,
`path`, `component`, `instance`; inside each `hier` entry: `level`, `value`.

```json
{
  "header":   { "name": "state", "version": "1.0", "timestamp": "…", "uuid": "…", "correlation_id": "…" },
  "identity": {
    "hier": [
      { "level": "site",    "value": "dallas" },
      { "level": "factory", "value": "finishing" },
      { "level": "zone",    "value": "zone-3" },
      { "level": "device",  "value": "gw-01" }
    ],
    "path":      "dallas/finishing/zone-3/gw-01",
    "component": "opcua-adapter",
    "instance":  "main"
  },
  "tags": { "app": "line-ctl" },
  "body": { }
}
```

Rules:
- `hier` is ordered, `minItems 1`; **its last entry is the device**. There is no `device` wire field.
- `path` = the `/`-join of the `hier` values, precomputed by the publisher. On deserialize, a missing
  `path` is recomputed; a present one is taken as-is (publisher is authoritative).
- `component` = the component's **UNS token = sanitized short name** (existing `{ComponentName}`
  semantics: segment after the last `.` — D‑U18). `instance` defaults to `"main"`
  (`MessageIdentity.DEFAULT_INSTANCE`).
- All four keys are present when `identity` is present. `identity` itself is **optional on the wire**: a
  message built without a config-bound builder (the CONFIG_COMPONENT bootstrap request §1.5, or raw
  bridging of external systems) legally omits it.
- **`tags.thing` is removed** — hard cut. `MessageTags` loses the thing field in all four languages:
  Java `MessageTags` drops `thingName` + the `"thing"` special-casing in `fromDict`/`toDict`
  (`MessageTags.java:26,73,87`); Python drops `thing_name` (`message.py:91,108,119`); Rust drops
  `MessageTags.thing_name` + `#[serde(rename="thing")]` (`message.rs:98–103`) and the builder's
  `thing_name()` setter; TS drops `MessageBuilder.withThingName` and the `tags.thing` stamp
  (`message.ts:222–226,255–257`). A stray inbound `thing` key just lands in the generic tag map — no
  legacy shim.

#### 1.2 The in-memory type (Java canonical)

One class serves as both the wire object and the component's resolved identity:

```java
package com.mbreissi.ggcommons.messaging;

public final class MessageIdentity {
    public static final String DEFAULT_INSTANCE = "main";
    public record HierEntry(String level, String value) { }

    private final List<HierEntry> hier;   // immutable, size >= 1; last = device
    private final String path;            // precomputed '/'-join of values
    private final String component;       // UNS component token (sanitized short name)
    private final String instance;        // never null

    public MessageIdentity(List<HierEntry> hier, String component, String instance); // validates + computes path

    public List<HierEntry> getHier();
    public String getPath();
    public String getComponent();
    public String getInstance();
    /** Computed accessor — the last hier entry's value. NOT a wire field. */
    public String getDevice();
    /** Copy with a different per-message instance token (validated). */
    public MessageIdentity withInstance(String instance);

    public JsonObject toDict();                          // canonical order: hier, path, component, instance
    /** Lenient: missing instance -> "main"; missing path -> recomputed; malformed hier -> null + WARN. */
    public static MessageIdentity fromDict(JsonObject src);
}
```

Deserialize leniency is deliberate: a malformed `identity` yields `identity == null` with a warning; the
message still delivers (mirrors the existing lenient envelope handling in all four libs).

#### 1.3 Serialize / deserialize / detection changes (per language)

- **Java** — `Message` gains `MessageIdentity identity` + `getIdentity()`; `toDict()` inserts `identity`
  between `header` and `tags` (`Message.java:65–81`); `MessageBuilder.fromObject` / `Message.build`
  parse `"identity"`; the **envelope-detection predicate** becomes *has any of*
  `header | identity | tags | body` (today `header|tags|body` at `MessageBuilder.java:43`,
  `Message.java:303`).
- **Python** — `Message` dataclass gains `identity: Optional[MessageIdentity] = None` (new
  `ggcommons/messaging/identity.py` dataclass with a `device` property); `to_dict`/`dumps`/`from_object`
  updated the same way (`message.py:131–141,197–217`).
- **Rust** — `Message` gains `#[serde(skip_serializing_if = "Option::is_none")] pub identity:
  Option<MessageIdentity>` declared between `header` and `tags` (field order = emit order);
  `MessageIdentity`/`HierEntry` are plain serde structs; `Message::from_json_value` treats `identity`
  as an envelope marker.
- **TS** — `message.ts` gains `export interface HierLevel { level: string; value: string }` and a
  `MessageIdentity` class (`hier/path/component/instance`, `get device()`, `withInstance`, `toObject`,
  `fromObject`); `Message.toObject()` emits it after `header`; `Message.fromObject` adds
  `"identity" in obj` to the envelope predicate (`message.ts:148`).

#### 1.4 Where it is stamped

`MessageBuilder.build()` is the single stamping site (all four languages):

```java
// MessageBuilder (Java) — additions
public MessageBuilder withInstance(String instance);        // validated token; default "main"
public MessageBuilder withIdentity(MessageIdentity id);     // explicit override (tests, vectors, relays)

public Message build() {
    // …existing header/tags/body…
    if (identityOverride != null)      message.identity = identityOverride;
    else if (configService != null)    message.identity =
        configService.getComponentIdentity().withInstance(instance != null ? instance : MessageIdentity.DEFAULT_INSTANCE);
    // no config, no override -> identity stays null (bootstrap/raw cases)
}
```

Mirrors: Python `MessageBuilder.with_instance()` / `with_identity()` stamping in `build()`
(`message_builder.py:84–106`; identity requires config or an explicit override); Rust
`MessageBuilder::instance(&str)` / `identity(MessageIdentity)` with `from_config` capturing
`config.identity()` (`message.rs:322–341`); TS `withInstance()` / `withIdentity()` with `withConfig`
capturing the config identity (`message.ts:232–238`).

#### 1.5 Where it is resolved (config side — NO shared config)

`ConfigManager` resolves identity **once at construction**, from the component's own config, and exposes
`getComponentIdentity()` (instance `"main"`). Resolution algorithm (identical 4 ways, fail-fast):

1. `levels` = top-level `hierarchy.levels` if present, else `["device"]` (zero-config default — the UNS
   works out of the box as `ecv1/{thing}/{comp}/main/{class}`).
2. Level **names** must match `^[A-Za-z0-9_-]+$`, be unique, non-empty (they become Parquet columns in
   Phase 4 — keep them strict).
3. For every level **except the last**, the value comes from the top-level `identity` config object —
   missing ⇒ startup error naming the missing level(s). The **last level's value = the resolved thing
   name** from the existing identity chain (`PlatformResolver.resolveIdentity`, `PlatformResolver.java:460`
   — `-t` ▸ K8s `GGCOMMONS_THING_NAME` ▸ `POD_NAME` ▸ `AWS_IOT_THING_NAME` ▸ default) — D‑U1. A key in
   `identity` config equal to the last level name, or not in `levels[0..n-2]`, is a startup error (typo
   protection the schema cannot express).
4. Every **value** passes through the existing template **sanitizer** (`/ \ + #`, control chars → `_`;
   `..` → `_`). If sanitization changed a value, log WARN and use the sanitized value.
5. `component` = sanitized short name; `path` = join.

**Java init-order note:** `MessagingClient` is constructed *before* `ConfigManager` (`GGCommons.java:145–150`,
because GG_CONFIG/CONFIG_COMPONENT load config over IPC). Consequences, by design: (a) the
CONFIG_COMPONENT bootstrap `get-configuration` request carries **no identity** and instead carries
`{"component": "<short name>"}` in its body; (b) `Uns`, the guard's `includeRoot` flag, and the
request-deadline default are **late-bound** onto the messaging client immediately after `ConfigManager`
construction (§5). Rust/TS/Python build config before or independent of messaging and can bind directly.

---

### 2. `gg.uns()` — topic builder + validator

#### 2.1 Java surface

```java
package com.mbreissi.ggcommons.uns;

/** The closed class set. RESERVED = library-owned publish classes. */
public enum UnsClass {
    STATE("state", /*leaf*/ true),  METRIC("metric", false),
    CFG("cfg", true),               LOG("log", false),
    DATA("data", false),            EVT("evt", false),
    CMD("cmd", false),              APP("app", false);
    public final String token;
    public final boolean leaf;                       // leaf => channel forbidden; else channel REQUIRED
    public static final Set<UnsClass> RESERVED = Set.of(STATE, METRIC, CFG, LOG);
}

/** Wildcard scope for filter(). null field -> '+'. site used only when includeRoot=true. */
public record UnsScope(String site, String device, String component, String instance) {
    public static UnsScope all();
    public static UnsScope device(String device);
    public static UnsScope component(String device, String component);
    public static UnsScope instance(String device, String component, String instance);
}

public final class Uns {
    public static final String ROOT = "ecv1";

    public String topic(UnsClass cls);                                   // leaf classes (state, cfg)
    public String topic(UnsClass cls, String channel);                   // channeled classes
    public String topicFor(MessageIdentity target, UnsClass cls, String channel);
    public String filter(UnsClass cls, UnsScope scope);                  // '/#' appended for channeled classes
    public void   validate(String topic);                                // throws UnsValidationException
    public MessageIdentity identity();                                   // the bound identity
}
// GGCommons: public Uns getUns();   (Python gg.uns(), Rust gg.uns(), TS gg.uns())
```

`gg.getUns()` is bound to the component identity (instance `main`); `gg.instance("kep1").uns()` is bound
to that instance (§3). `topicFor` takes a `MessageIdentity` — typically from a received message's
`getIdentity()` — which is how you address a peer's `cmd` inbox without parsing topics.

#### 2.2 Grammar & normative validation rules

```
[ecv1] [/ {site}]? / {device} / {component} / {instance} / {class} [/ {channel…}]
```

1. **Token rule** (identical to the template sanitizer's blacklist, so any sanitized value passes — this
   is the reconciliation): a token is non-empty, contains no `/`, `+`, `#`, `\`, no control characters
   (U+0000–U+001F, U+007F), and no `..` substring. Dots are legal (D5: literal-within-a-level). The
   validator deliberately does **not** impose a stricter whitelist than the sanitizer, or sanitized
   values (thing names with spaces, etc.) would build unpublishable topics.
2. **Depth guard**: total `/` count ≤ 7 (AWS IoT Core's 8-level limit) ⇒ channel ≤ **3 tokens** without
   root, ≤ **2 tokens** with `topic.includeRoot: true`. `topic()` enforces at build time; over-deep
   channel throws, never silently drops at IoT Core.
3. **Length**: total topic ≤ 256 UTF-8 bytes (IoT Core publish limit).
4. **Class rules**: `state`/`cfg` are leaf (channel forbidden); all other classes **require** ≥ 1
   channel token. `cmd` channels are lowercase-hyphenated verbs, optionally family-namespaced
   (`sb/status`).
5. `validate(topic)` accepts only **concrete** topics (rejects `+`/`#`); checks the token rule, root
   literal, minimum 5 tokens (6 rooted), class ∈ enum, leaf-class tail absence, depth, length.
   `filter()` output is correct by construction and not passed through `validate`.
6. **Root** (D‑U11, low priority): `topic.includeRoot` (top-level `topic` config block, default `false`)
   inserts `hier[0].value` after `ecv1` in `topic()`/`topicFor()` and a `site` position in `filter()`.
7. Reply topics (`ggcommons/reply-…`, `MessageHeader.REPLY_MESSAGE_TOPIC_PREFIX`) are **non-UNS** and
   never pass through `uns()`; the guard ignores them because they are not `ecv1/`-rooted (D‑U6).

Error type: Java `UnsValidationException extends IllegalArgumentException` carrying a machine-readable
code — `EMPTY_TOKEN | BAD_CHAR | TRAVERSAL | DEPTH_EXCEEDED | LENGTH_EXCEEDED | CHANNEL_ON_LEAF |
CHANNEL_REQUIRED | BAD_ROOT | BAD_CLASS | WILDCARD_IN_TOPIC`. Codes are pinned in
`uns-test-vectors/topics.json` so all four languages fail identically.

**Mirror notes** — Python: `ggcommons/uns.py` — `class UnsClass(str, Enum)`, `@dataclass(frozen=True)
UnsScope`, `class Uns` with `topic/topic_for/filter/validate/identity`, `UnsValidationError(ValueError)`
with `.code`. Rust: `ggcommons::uns` — `enum UnsClass`, `struct UnsScope` (builder ctors), `struct Uns`,
errors as `GgError::UnsValidation { code, detail }` (new variant); `topic()` returns `Result<String>`.
TS: `src/uns.ts` — `enum UnsClass`, `interface UnsScope` + factory object, `class Uns`, `class
UnsValidationError extends Error { code }`.

#### 2.3 (M8) Named/secondary messaging connection — uniform, config-driven (D‑U17, resolved 2026-07-02)

The `uns-bridge` needs two concurrent connections in one process (device bus + site broker). Rather than a
Rust-only imperative API (the original proposal — a flagged divergence), this is a **uniform,
config-declared library capability in all four languages** (user direction): the bridge declares its
site-broker uplink as a **named messaging connection** in config — conceptually its "external system,"
reusing the same `MessagingProvider`/MQTT stack — and the library provisions + manages it, retrievable by
name:

```
gg.messaging()          // the primary/default connection (unnamed)
gg.messaging("site")    // a config-declared secondary connection — same API surface
```

Both connections get the same reserved-class guard + request-deadline default. Python's static/global
`MessagingClient` becomes a **keyed registry** (default + named). This **eliminates the D‑U17
divergence** — it is config, not a per-language imperative API. It is only needed by the bridge, so it
**lands in Phase 3**; the config shape (a dedicated `messaging.connections[]`-style section vs reusing
`component.instances[]`) is finalized then. Lean: a dedicated named-connections section, kept **distinct**
from the per-message `instance` token and the `gg.instance()` handle (§3 / D‑U3) — those address *message
identity*, this addresses *transport*.

---

### 3. The per-message `instance` seam (finalized)

**Canonical shape: an instance-scoped handle whose only job is to pre-bind the instance token into (a)
the topic builder and (b) the message builder.** The messaging client stays instance-agnostic —
`publish(topic, msg)` already receives both the topic (minted by an instance-bound `Uns`) and the
envelope (stamped by an instance-bound builder). This is precisely why the seam works unchanged over
**Python's static/process-global `MessagingClient`** (`messaging_client.py:19`): no per-instance state
ever touches the global client; the instance travels entirely in the two artifacts passed to it.

```java
public final class GgInstance {
    public String id();
    public Uns uns();                                          // topics minted with this instance token
    public MessageBuilder newMessage(String name, String version);
        // == MessageBuilder.create(name, version).withConfig(configManager).withInstance(id)
    // Components phase adds: telemetry(), events(), commands() here — same handle, no rework.
}

// GGCommons
public GgInstance instance(String instanceId);   // token validated (§2.2); cached per id (ConcurrentHashMap)
```

Rules:
- Component-level messages (everything not built through a handle) default to `instance == "main"`.
- The handle does **not** verify the id against `component.instances[]` — instances may be created
  dynamically; the token-charset validation is the only gate (log DEBUG if the id is not in the
  configured instances list, as a diagnostic aid).
- `MessageBuilder.withInstance` is **public** in all four languages: forging an instance token is no
  worse than forging any envelope field, in-process code is trusted (the broker ACL is the security
  boundary, DESIGN-uns §7.5), and Python/TS have no enforceable privacy anyway. The handle is the
  *convenient* path, not the *privileged* one.

**Mirror notes** — Python: `gg.instance("kep1") -> GgInstance` with `.uns()`, `.new_message(name,
version)`; `MessageBuilder.with_instance()`. Rust: `gg.instance("kep1") -> Result<GgInstance>` (validates
token) holding the config snapshot + id; `.uns()`, `.message(name, version) -> MessageBuilder`;
`MessageBuilder::instance(impl Into<String>)`. TS: `gg.instance(id): GgInstance` with `.uns()`,
`.newMessage(name, version)`; `MessageBuilder.withInstance()`.

---

### 4. Reserved-class publish guard + privileged internal-publish seam

#### 4.1 The guard (public surface)

Reserved tokens: `state | metric | cfg | log` (`UnsClass.RESERVED`). The check, identical 4 ways:

```
reject if tokens[0] == "ecv1"
      && ( tokens.length >= 5 && reserved(tokens[4])
        || includeRoot && tokens.length >= 6 && reserved(tokens[5]) )
```

- Class position 4 is always checked (rootless grammar). Position 5 is checked **only when this
  component's `topic.includeRoot` is true** — checking it unconditionally would false-positive on
  legitimate app channels (`ecv1/d/c/i/app/state`). The residual gap (a root-off component hand-forging
  a rooted reserved topic) is accepted: the guard is **misuse prevention, not a security boundary** —
  per-device broker ACLs are the durable enforcement (DESIGN-uns §7.5 pt 3).
- **Guarded methods** (every public path that emits a client-chosen topic): `publish`, `publishRaw`,
  `publishToIoTCore`, `publishToIoTCoreRaw` (D‑U8), `request`, `requestFromIoTCore`, **and `reply` /
  `replyToIoTCore`** — the reply pair matters: a hostile requester could set `header.reply_to` to a
  victim's reserved topic and turn an innocent responder into a forger
  (`GreengrassMessagingProvider.reply:235` publishes straight to `getReplyTo()`).
- `subscribe*` is never guarded (consumers must read reserved classes).
- Non-`ecv1` topics pass untouched: `ggcommons/reply-…` (D‑U6), `cloudwatch/metric/put` (external AWS
  contract, D‑U21), legacy/foreign MQTT bridging.
- Error: Java `ReservedTopicException extends IllegalArgumentException`; Python
  `ReservedTopicError(ValueError)`; Rust `GgError::ReservedTopic(String)`; TS `class ReservedTopicError
  extends Error`. Message names the topic, the class token, and points at `gg.status()/gg.metrics()`
  (or, this phase, the library-owned publishers).

#### 4.2 The privileged seam (per language)

The library's own publishers — heartbeat/state, the `Messaging` metric target, and the new `cfg`
publisher — currently call the **public** `publish()` and would be blocked. Each language gets an
internal path the guard does not apply to:

- **Java** — `ReservedPublisher` (messaging package), via `MessagingClient.reservedPublisher()`. Must be
  public-reachable (Heartbeat/metrics live in other packages), so public but named + documented
  library-internal; wired by `GGCommons.init`. In-process bypass is possible and out of scope.
  ```java
  public final class ReservedPublisher {
      public void publish(String topic, Message msg);                    // no guard
      public void publishRaw(String topic, JsonObject payload);
      public void publishToIoTCore(String topic, Message msg, QOS qos);
  }
  ```
- **Python** — `MessagingClient._publish_reserved(topic, msg)` / `_publish_reserved_raw(...)`
  staticmethods (underscore convention).
- **Rust** — the only language with *real* enforcement: a **crate-private trait**
  ```rust
  pub(crate) trait ReservedMessaging: Send + Sync {
      async fn publish_reserved(&self, topic: &str, msg: &Message) -> Result<()>;
  }
  impl ReservedMessaging for DefaultMessagingService { /* bypasses guard */ }
  ```
  `lib.rs` hands heartbeat/metrics an `Arc<dyn ReservedMessaging>`; test fakes implement both.
- **TS** — `/** @internal */ publishReserved(...)` on `DefaultMessagingService`, with `stripInternal:
  true` in `tsconfig` so it vanishes from published typings (soft enforcement).

`includeRoot` and the guard flag are late-bound in Java/Python (set right after `ConfigManager`;
default `false` before that — nothing publishes rooted topics pre-config).

#### 4.3 Library publisher re-targeting (what actually uses the seam)

Hard-cut topic map, replacing the four legacy sites in this same phase:

| Publisher | Old | New (via internal `uns()` + `ReservedPublisher`) |
|---|---|---|
| heartbeat → **state keepalive** | `ggcommons/{ThingName}/{ComponentName}/heartbeat` (`HeartbeatConfiguration.DEFAULT_TOPIC:53`) | `ecv1/{device}/{component}/main/state`, header `name:"state"`, body `{"status":"RUNNING","uptimeSecs":n}`; best-effort `{"status":"STOPPED"}` on graceful shutdown |
| heartbeat measures | same message | a metric named **`sys`** through the normal metric subsystem each tick (D6) |
| metric `messaging` target | `{ThingName}/{ComponentName}/metric` (`MetricConfiguration:18`) | `ecv1/{device}/{component}/main/metric/{metricName}` (name sanitized as a channel token) |
| **cfg publisher (new)** | — | `ecv1/{device}/{component}/main/cfg` on startup + on config change; body `{"config": <effective, redacted>}`. Redaction v1: `$secret` refs never resolved; `messaging.*.credentials` + any `password`/`pin` key → `"***"` |
| config-get (CONFIG_COMPONENT) | `ggcommons/{ThingName}/config/get/{ComponentName}` (`ConfigComponentProvider.java:22`) | request to `ecv1/{device}/config/main/cmd/get-configuration` — `config` is a **reserved-by-convention logical component name**; requester identified by the envelope (or body `{"component"}` in the pre-config bootstrap §1.5). `cmd` is not reserved — no seam needed |
| config push | `ggcommons/{ThingName}/config/{ComponentName}/updated` (`:23`) | fire-and-forget `cmd`: `ecv1/{device}/{component}/main/cmd/set-config`, body = new config (a `cmd` without `reply_to` is a notification-style command — normative) |
| cloudwatch-component target | `cloudwatch/metric/put` | **unchanged** — external AWS Greengrass component contract (D‑U21) |

**Command addressing — the two config flows + broadcast (D‑U19, resolved 2026-07-02 → component-inbox +
broadcast):**
- **Flow A — config *source* fetch** (a `CONFIG_COMPONENT` client pulls *its own* config): a request to
  `ecv1/{device}/config/main/cmd/get-configuration`; the **config server is the sole subscriber** and
  replies via `reply_to`; the requester self-identifies (envelope, or body `{"component"}` pre-config).
  `config` is a reserved-by-convention **logical component name**.
- **Flow B — console→component commands** (the built-in verbs `get-configuration`, `reload-config`,
  `set-log-level`, `describe`, `sb/*`): addressed to the **target component's own inbox**
  `ecv1/{device}/{component}/{instance}/cmd/{verb}`. A component's single `ecv1/{device}/{me}/+/cmd/#`
  subscription is therefore topic-selective — it never receives another component's commands, no
  body-filtering. `set-config` push (server→component) uses this same inbox.
- **Broadcast** — a reserved pseudo-component token **`_bcast`**: a command to *all* components on a
  device goes to `ecv1/{device}/_bcast/main/cmd/{verb}`, and every component also subscribes to
  `ecv1/{device}/_bcast/main/cmd/#`. This standardizes (and fixes) the malformed
  `ecv1/bcast/cmd/republish-state` in DESIGN-uns §9.3 → `ecv1/{device}/_bcast/main/cmd/republish-state`.
  (Site-wide broadcast across devices = the console publishing per-device from its FleetModel, or a
  `+`-device refinement — deferred to Phase 3.) **Reserved tokens:** logical component `config`;
  pseudo-component `_bcast`; the `_`-prefix is reserved for system pseudo-components. Flow-B verb handlers
  + broadcast land in Phase 3 (facade `commands()`); **Phase 1 implements only Flow A + the `set-config`
  push.**

Heartbeat config reshape (resolves #33 / M11 in this phase — Risks #1): `heartbeat.targets[]` is
**removed** (where the topic drift knobs live); replaced by `heartbeat: { enabled (bool, default true),
intervalSecs (5, min 1), measures {…unchanged}, destination ("local"|"iotcore", default "local") }`. Flips
Rust/TS from effectively-off (empty default targets) and Java (`metric` default) and Python
(`messaging`+legacy topic) all onto **on / 5 s / state-on-local** (D‑U14). Validate on the HOST smoke.

---

### 5. `request()` internal deadline

#### 5.1 Semantics (all languages)

- New config key: **`messaging.requestTimeoutSeconds`** (number, min 0, default **30**; `0` disables) —
  D‑U5. Read from the `messaging` section; explicit per-call override always wins. Java/Python late-bind
  the default right after `ConfigManager` exists (§1.5); until then the built-in 30 applies —
  deliberately, so the CONFIG_COMPONENT bootstrap request gets a deadline instead of hanging forever.
- `request()` arms a **framework-owned timer at send time**. On fire it (1) unsubscribes the ephemeral
  reply topic (the leak being fixed), (2) removes the pending entry, (3) completes the future
  **exceptionally** — even if the caller never awaits/`get()`s.
- **Single idempotent settle path** (the TS `finish()` pattern, `service.ts:170–191`, is the model):
  reply-arrival, deadline, and `cancelRequest` all CAS a per-request `settled` flag; the loser no-ops; a
  straggler reply after settle is logged DEBUG and dropped.
- Consequences (normative): no-arg `get()`/`await` no longer blocks forever; `get(t)` waits `min(t,
  deadline)`; `reply()` and `subscribe()` untouched.
- Overload: `request(topic, msg, timeout)` — zero/None disables the deadline for that call.

#### 5.2 Per-language mechanism

| Lang | API | Timer | Failure signal |
|---|---|---|---|
| **Java** | `request(String, Message)` + `request(String, Message, Duration)` (same for `requestFromIoTCore`) on `MessagingClient` + both providers | one shared lazy 1-thread daemon `ScheduledExecutorService` per provider; `ScheduledFuture` canceled on settle | `future.completeExceptionally(new java.util.concurrent.TimeoutException(...))` (`ReplyFuture extends CompletableFuture<Message>` already carries `replyTopic`) |
| **Python** | `request(topic, msg, timeout_secs: float \| None = None)` (None = default, 0 = off) | `threading.Timer(deadline, _on_deadline)` per request, `.cancel()`ed on settle; settle guarded by a per-entry lock/flag | `Iou` gains `set_error(exc)`; `Iou.get()` **raises** `RequestTimeoutError` (contract change to `iou.py:25–31` — pre-1.0 accepted, flagged) |
| **Rust** | `request(...)` (default) + `request_with_timeout(topic, msg, Option<Duration>)` (no overloading) | restructure `start_request` into a **spawned supervisor task** owning the reply subscription: `tokio::select! { reply = rx => …, _ = sleep(d) => … }`, then unsubscribe + send `Result<Message>` down a `oneshot`; `ReplyFuture` wraps the `oneshot::Receiver` + a cancel handle (Drop still cancels — preserving today's contract, `service.rs:129–137`). Closes Rust's real gap (stored-but-never-polled future) | future resolves `Err(GgError::RequestTimeout { topic, secs })` (new variant) |
| **TS** | already present (`timeoutMs?`, `service.ts:154–160`) — change only the default: `undefined` now resolves to `requestTimeoutSeconds * 1000` (was 0 = off); explicit `0` still disables | existing `setTimeout` + `finish()` | narrow existing `Error` to `class RequestTimeoutError extends Error` for parity |

---

### 6. MQTT LWT provider hook (IPC no-ops; NO retain)

Config-driven only (the bridge is a component with config); minimal shape added to `messaging`:

```jsonc
"messaging": {
  "local":  { },  "iotCore": { },
  "requestTimeoutSeconds": 30,
  "lwt": {
    "topic":   "ecv1/gw-01/uns-bridge/main/state",   // required
    "payload": { "status": "UNREACHABLE" },          // string or object; published VERBATIM
    "qos": 1                                          // 0|1, default 1. NO retain field — hard omit (D9).
  }
}
```

- Applies to the **local-broker connection only** (a bridge's "local" config *is* the site broker on its
  named client). IoT Core LWT deferred; IPC provider logs DEBUG and no-ops.
- Wiring: Java Paho `MqttConnectOptions.setWill(topic, bytes, qos, false)`; Rust rumqttc
  `MqttOptions::set_last_will(LastWill{ retain: false, .. })`; Python paho `client.will_set(...,
  retain=False)`; TS mqtt.js `will: { retain: false }`. All re-register the will on reconnect.
- The will is **registered at CONNECT, not routed through `publish()`** — the guard does not (cannot)
  apply; document this. Broker ACLs govern wills. LWT payload timestamp is connect-time-stale by nature;
  consumers MUST treat it as "event time = delivery time" (the console FleetModel timestamps on receipt).

---

### 7. Facade scope for THIS effort

**In scope now:** `identity`+`hierarchy` resolution + top-level envelope element; `uns()` (+ `UnsScope`,
validation, vectors); the instance handle (`uns()` + `newMessage()` only); the reserved-class guard +
per-language internal seam; the `request()` deadline; MQTT LWT; IoTCore casing normalization; the
library-owned **state / metric / cfg** publishers on UNS topics; the CONFIG_COMPONENT rendezvous remap;
the M8 named client (Rust only); schema changes + `uns-test-vectors` + the interop UNS suite.

**Deferred to the components phase:** `telemetry()`, `status()`, `events()`, `commands()`, `discovery()`
facades; all built-in `cmd` verbs incl. the `republish-state` broadcast listener (late-join lands with
the bridge, Phase 3); the `log`-tail publisher (class reserved+guarded now, publisher later); `uns-bridge`
+ site-broker recipes (M1/M2); streaming enrichment (M15); the southbound command family (M9); D‑U15/16
(Phase 5).

**Schema deltas** (edit `schema/ggcommons-config-schema.json`, then `schema/sync-schema.sh`): ADD
top-level `hierarchy {levels: string[]}`, `identity` (patternProperties `^[A-Za-z0-9_-]+$` → string),
`topic {includeRoot: bool=false}`; `messaging` += `requestTimeoutSeconds`, `lwt`; `heartbeat` −=
`targets` (+`enabled`, +`destination`); `metricEmission.targetConfig` −= `topic` (keep `destination`,
D‑U9). Top-level `additionalProperties:false` then fails every stale config with a precise error —
intended (§10 hard cut).

**Vectors & interop (D‑U12/13/14):** `uns-test-vectors/` mirrors `vault-test-vectors/` — `topics.json`
(cases `{hierarchyLevels, identityValues, component, instance, includeRoot, class, channel} → {topic}` or
`{error: <code §2.2>}`, plus guard cases `{topic, includeRoot} → allowed|reserved`) and `envelopes.json`
(one golden **full canonical JSON** envelope per class with pinned `uuid`/`correlation_id`/`timestamp`;
**Rust and TS builders must gain `uuid()`/`timestamp()` setters** — Java/Python already have them — and
assert structural equality both directions). Generated by a **Java canonical generator test**; per-language
loaders with existence-guarded skips. Interop: each node
(`test-infra/interop/{python_node.py,java_node,rust_node,ts_node}`) gains a `uns-pub` role (fixed identity
via args/env → publishes one `state` + one `data` envelope through the real facade path) and a `uns-guard`
role (attempts a reserved publish, must exit with the documented error); new `test_uns_*` functions in
`test_interop.py` assert byte-identical topics + structurally-identical `identity`, auto-picked-up.

**Casing normalization (D‑U7), exact rename list:** Java `MessagingClient.publishToIotCore →
publishToIoTCore`, `publishToIotCoreRaw → publishToIoTCoreRaw` (provider layer already `IoTCore`); update
the 6+ internal call sites. TS: whole family `Iot → IoT` (`publishToIotCore`, `publishToIotCoreRaw`,
`subscribeToIotCore`, `unsubscribeFromIotCore`, `requestFromIotCore`, `replyToIotCore`,
`cancelRequestFromIotCore`, enum member `Destination.IotCore → Destination.IoTCore`) — the enum's **string
value `"iotcore"` and all config-enum tokens are unchanged**. Python: methods stay snake_case; rename
internal `IotCoreSubscriptionHandler → IoTCoreSubscriptionHandler` (cosmetic). Rust: **keeps
`IotCore`/`_iot_core`** (RFC-430 acronym convention — per-language idiom, not a divergence).

---

## Decisions register

| ID | Decision | Resolution | Conf. | Reversible? | Needs user? |
|---|---|---|---|---|---|
| D‑U1 | `device` source | resolved thing name (`PlatformResolver.resolveIdentity` chain), stamped as last `hier` entry; a `device`-level key in `identity` config is a startup error | High | Easy | no |
| D‑U2 | Identity from each component's OWN config | top-level `hierarchy`+`identity` read directly; resolved once at `ConfigManager` ctor, fail-fast; shared-config is a later optimization, nothing here depends on it | High | Easy | no |
| D‑U3 | Instance seam | `gg.instance(id) → GgInstance{ uns(), newMessage() }`; stamping via public `MessageBuilder.withInstance` (default `main`); works over Python's static client | High | Moderate | no |
| D‑U4 | Privileged internal publish | per-language seams (§4.2): Java `ReservedPublisher`, Python `_publish_reserved`, Rust `pub(crate) trait ReservedMessaging` (real enforcement), TS `@internal`+`stripInternal`. Guard = misuse prevention; broker ACL = boundary | High | Easy | no |
| D‑U5 | Request deadline config | `messaging.requestTimeoutSeconds`, default 30, 0=off; per-call override; late-bound in Java/Python | High | Easy | no |
| D‑U6 | Keep `ggcommons/reply-` prefix | non-UNS, ephemeral; guard keys on `ecv1` root so reply topics are structurally exempt | High | Easy | no |
| D‑U7 | Normalize IoTCore casing now | per-language idiom: Java+TS → `IoTCore`; Rust keeps `IotCore`; Python snake untouched; **all string/config tokens (`"iotcore"`) unchanged** | High | Easy | no |
| D‑U8 | Guard `publishRaw` too | confirmed + extended: `request*` and **`reply*`** also guarded (hostile `reply_to` forgery) | High | Easy | no |
| D‑U9 | Keep `destination`, drop topic overrides | confirmed for `metricEmission.targetConfig`; for heartbeat, subsumed by D‑U20 (`targets[]` removed wholesale, `destination` survives as scalar) | High | Easy | no |
| D‑U10 | Top-level `hierarchy`+`identity` schema props | confirmed; zero-config default `levels=["device"]`; level names strict `^[A-Za-z0-9_-]+$` (future Parquet columns) | High | Easy | no |
| D‑U11 | `topic.includeRoot` (default false) | confirmed, low priority; in `topic()/topicFor()/filter()` + the guard position-5 check; channel budget shrinks to 2 rooted | Med | Easy | no |
| D‑U12 | Vectors generated from Java canonical | confirmed (vault precedent is Rust; UNS is Java-authored — trivial JSON, not crypto) | High | Easy | no |
| D‑U13 | Split `topics.json`+`envelopes.json`; golden = full canonical JSON | confirmed; structural comparison (D‑U22); pinned uuids/timestamps need Rust/TS builder setters | High | Easy | no |
| D‑U14 | Heartbeat #33 → on/5s/local `state` | flips Rust/TS off→on, Java metric→state, Python legacy→state; measures → metric `sys`; graceful `STOPPED` state; validate on HOST smoke. Pulled into Phase 1 (Risks #1) | High | Moderate (fleet-wide behavior) | no (pre-approved; **review the STOPPED addition**) |
| D‑U15 | signalId → sanitized `data/{channel}`, raw id in body | provisional, **Phase 5** — no work now; token rule (§2.2) = the same sanitizer | Med | Easy (deferred) | no |
| D‑U16 | `writes.allow[]` matches stable `signal.id` | provisional, **Phase 5** (adapter-contract change, M9) | Med | Easy (deferred) | no |
| **D‑U17** | M8 named/secondary messaging connection | ✅ **resolved 2026-07-02 → NO divergence.** Reframed (user) from a Rust-only imperative `messaging_named()` into a **uniform, config-declared named connection** in all four langs: the bridge declares its site-broker uplink in config (its "external system," reusing the MQTT provider), retrieved via `gg.messaging("<name>")`; Python's static client → keyed registry. Lands **Phase 3** (with the bridge); config shape finalized then, kept distinct from the per-message `instance`/`gg.instance()` (D‑U3). §2.3 | High | Moderate | resolved |
| **D‑U18** | `identity.component` = **sanitized short name** (existing `{ComponentName}` semantics), not reverse-DNS full name | ✅ **confirmed 2026-07-02** (user agreed). Matches every existing topic site + the design examples; dots legal in-level so full name stays possible later; cross-vendor collision on one device accepted pre-1.0 | High | Hard once fleets deploy | resolved |
| **D‑U19** | Config-command addressing | ✅ **resolved 2026-07-02 → component-inbox + broadcast** (user). **Flow A** (config-source fetch) stays a request to `config/main` (server is sole subscriber, requester self-IDs). **Flow B** (console→component verbs incl. `get-configuration`) → the target component's OWN inbox `ecv1/{device}/{component}/{instance}/cmd/{verb}` (topic-selective, uniform for all verbs, no body-filtering); **broadcast** via reserved `_bcast` (`ecv1/{device}/_bcast/main/cmd/{verb}`). `set-config` push = component inbox. §4.3 | High | Moderate | resolved |
| **D‑U20** | Heartbeat `targets[]` REMOVED → `enabled/intervalSecs/measures/destination`; measures emit metric **`sys`** via normal metric subsystem | ✅ **confirmed 2026-07-02** (user). The measures **keep full sink flexibility** — they now flow through the metric subsystem's own targets (messaging/cloudwatch-component/EMF/local-log), which `heartbeat.targets[]` did not provide for measures; `heartbeat.destination` (local\|iotcore) governs only the lightweight `state` keepalive. Supersedes the letter of D‑U9 for heartbeat | High | Moderate (schema break, intended) | resolved |
| D‑U21 | `cloudwatch/metric/put` unchanged | external AWS Greengrass component contract; non-`ecv1` so guard-exempt | High | Easy | no |
| D‑U22 | Topics byte-identical; envelopes structurally identical (member order not normative) | the four serializers already differ in order + the interop harness compares structurally; byte order would churn all four for zero consumer value | High | Easy | no |
| D‑U23 | Error identities: `UnsValidationException`(+code)/`ReservedTopicException`/`TimeoutException`(Java std)/`RequestTimeoutError`(Py/TS)/`GgError::{UnsValidation,ReservedTopic,RequestTimeout}`(Rust); Python `Iou.get()` now raises on deadline | confirmed; the Python `Iou` contract change is the sharpest edge (pre-1.0 accepted) | High | Easy | no |
| D‑U24 | Guard checks class position 4 always, position 5 only when own `includeRoot=true`; residual rooted-forgery gap accepted (broker ACL closes it) | the alternative (both positions unconditionally) false-positives on legit `app/...` channels | High | Easy | no |

---

## Risks & sequencing notes

1. **M11 (heartbeat parity) moves from Phase 5 into Phase 1** — deliberate change to DESIGN-uns §13. The
   hard cut retires the legacy heartbeat topic in Phase 1, and `heartbeat.targets[]` is the drift-knob
   carrier the schema drops; deferring M11 would break the heartbeat config twice.
2. **Java init order is the Phase-1 landmine** (`GGCommons.java:143–150`: messaging before config).
   Everything identity-dependent on the messaging client is late-bound (guard `includeRoot`, deadline
   default, `Uns` binding); the CONFIG_COMPONENT bootstrap request is *specified* to carry no identity
   (body `{"component"}`). Don't "fix" the init order — GG_CONFIG/CONFIG_COMPONENT need messaging first.
3. **Request-deadline changes startup failure semantics**: a component on CONFIG_COMPONENT with no config
   server now fails its fetch in ~30 s instead of hanging. Correct, but note it in release notes.
4. **`tags.thing` removal has blast radius outside the monorepo**: telemetry-processor's
   envelope-`tags`-as-JSON-column and both adapters read/emit envelopes; components pin the library by
   git rev, so they keep working until their rev bump — but the bump must land **with** their own topic
   migration. Sequence: library `main` first, then one migration PR per component repo (the `tag →
   signal` train is the precedent). `edge-console` design consumes the new identity — no code yet.
5. **Coverage gate (90%, 4 langs)**: new surface is mostly pure logic (validator, guard predicate,
   identity resolution, topic building, settle-CAS) — gate-friendly. Awkward bits: LWT (assert
   connect-options against fakes; live behavior on HOST smoke), the Rust request-supervisor rework
   (extend the existing `FakeProvider` / `testutil.rs`), Python timer races (short deadlines + events,
   not sleeps). Validate Python/Rust coverage on Linux/WSL (Windows undercounts).
6. **The Rust `start_request` restructure is the riskiest refactor** (supervisor task + oneshot changes
   `ReplyFuture` internals while preserving Drop-cleanup, `service.rs:129–140`). Do it as its own commit
   with the existing dual-broker MQTT integration tests green before layering the deadline on top.
7. **Sanitizer/validator coupling is normative**: the `uns()` token rule = *exactly* the sanitizer's
   blacklist (§2.2). If anyone later tightens one, they must tighten both. Pin with vector cases (a thing
   name with a space must build; one with `+` must have been sanitized to `_`).
8. **Reserved names to document:** logical component `config` (Flow A, §4.3/D‑U19), pseudo-component
   `_bcast` (broadcast fan-out, §4.3), the `_`-prefix for system pseudo-components, and the `ecv1` root.
   This supersedes the malformed `ecv1/bcast/cmd/...` in DESIGN-uns §9.3 (now
   `ecv1/{device}/_bcast/main/cmd/...`). Put the reserved-token warning in the identity docs now.
9. **Docs/templates surface is large**: schema-sync copies, CLI templates, examples, website
   config-schema reference + both "Sample Configurations" pages, and the "subscribe `heartbeat/+/+`"
   instructions in CLAUDE.md/README must all move to the six-wildcard set in the same train, or the
   docs-vs-code drift the 2026-06-29 audit fixed re-opens.
10. **Guard is advisory in 3 of 4 languages** (only Rust's `pub(crate)` seam is compiler-enforced). By
    design — say it plainly; land the per-device broker-ACL recipe (Phase 3, M2) as the durable
    enforcement.
11. **Golden-envelope determinism** requires the small builder additions (Rust `uuid()/timestamp()`, TS
    `withUuid()/withTimestamp()`) — trivial public API additions in two languages; keep them in the
    Phase-1 parity checklist.

---

## Build plan — phase checklist

**Phase 1 — grammar / identity / `uns()` / vectors + library publishers + config remap + schema + M11**
(this is the bulk of the "core" work; guard/deadline/LWT/casing from the doc's Phase 2 are folded in
where coupled):
- [ ] **Schema** (`schema/ggcommons-config-schema.json` + `sync-schema.sh` → 5 copies): +`hierarchy`,
  +`identity`, +`topic.includeRoot`, `messaging` += `requestTimeoutSeconds`/`lwt`, `heartbeat` reshape,
  drop drift knobs. Drift gate green.
- [ ] **Java canonical**: `MessageIdentity` + envelope; `MessageBuilder` stamping; `ConfigManager.getComponentIdentity`;
  `Uns` builder/validator; reserved guard + `ReservedPublisher`; `request()` deadline; MQTT LWT; IoTCore
  casing; heartbeat→state (M11) + metric `sys`; metric→UNS; cfg publisher; config-component remap. Unit
  tests → JaCoCo 90%.
- [ ] **`uns-test-vectors/`** (Java generator) + Java loader test.
- [ ] **Mirrors** (parallel): Python, Rust, TS — each vs the vectors; per-lang unit tests → 90% (Linux/WSL
  for Py/Rust).
- [ ] **Interop UNS suite** (`test_interop.py` `test_uns_*`, 4×4) + the `uns-guard` role.
- [ ] **M8 named client** (Rust only).

**Phase 3 — `uns-bridge` (Rust) + site-broker recipes (M1/M2) + late-join** (`republish-state` listener +
minimal `commands()` scaffolding).
**Phase 4 — streaming enrichment (M15).**
**Phase 5 — southbound command family (M9) + D‑U15/D‑U16 + component adoption (opcua/modbus/processor) +
docs/scaffold retirement** (incl. the stale `SouthboundTagUpdate` python-protocol-adapter template).

> **Cross-cutting:** `edge-console` (design only, no code) already consumes the new identity model; its
> DESIGN.md needs no change beyond what already references DESIGN-uns.
