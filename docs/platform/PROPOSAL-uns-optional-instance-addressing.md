# PROPOSAL — UNS optional-instance addressing (resolves D‑CAM‑18)

> **Status: design RATIFIED 2026-07-15 (see §11); implementation not yet started. No code has moved.**
> This document is the agreed design for a change to the canonical UNS **topic grammar**. It defines the
> new grammar, quotes the exact amendments it will make to `UNS-CANONICAL-DESIGN.md`, `DESIGN-uns.md`,
> `SOUTHBOUND.md`, and `camera-adapter/DESIGN.md`, and gives the four-language core implementation plan
> and the validation/migration plan. The three previously-open decisions are now settled: **(1) the rule
> applies to all classes** (retire `main` everywhere), **(2) migration is a single coordinated cutover**
> (feasible while the only deployments are org-controlled test infra), **(3) the wire `identity.instance`
> is omitted when absent.** The canonical docs and the four libraries are updated in follow-up work
> against this agreed design.
>
> Proposed decision id: **D‑U28** (next free id in the D‑U register; confirm against
> `UNS-CANONICAL-DESIGN.md` at merge). Supersedes the addressing half of **M9 / D‑U16** in
> `SOUTHBOUND.md` §2.2 and resolves **camera-adapter `DESIGN.md` §27 Q8**.

## 1. Summary

Make the **instance token optional** in the UNS topic grammar:

- **instance present** → the message/command is **instance-scoped** (addresses one instance);
- **instance absent** → the message/command is **component/global-scoped** (addresses the component as a whole).

This retires the `main` sentinel instance entirely. It dissolves the D‑CAM‑18 dichotomy — the camera
adapter no longer needs to put the camera id **in the message body** (its shipped behaviour), and no
component needs an arbitrary `main` keyword in the path for component-scoped traffic. The topic **is**
the address, which is the point of a UNS.

It costs one cheap invariant (an instance id may not equal a reserved class token) and one small
ergonomic change (a subscriber that wants both scopes binds two subscriptions instead of one). It does
**not** require a variable-length generic topic parser. It **is** a wire-contract change to
already-shipped `main/*` topics, so it carries the full cross-language interop + deployed-IPC gate and a
migration.

## 2. The problem it solves — D‑CAM‑18

`camera-adapter/DESIGN.md` D‑CAM‑18 and §27 Q8 record an unresolved decision: how a southbound command
names which camera it targets. Two incompatible schemes exist today:

- **Scheme A — what the camera adapter shipped.** One component inbox `ecv1/{device}/camera-adapter/main/cmd/#`,
  camera selected by `instance` **in the body**. Uses the `main` sentinel; routes by body inspection.
- **Scheme B — core's approved-but-unshipped target** (`SOUTHBOUND.md` §2.2, mandate **M9**,
  D‑U15/D‑U16). Per-instance topic `ecv1/{device}/{component}/{instance}/cmd/sb/{verb}` with a fixed
  verb family (`sb/status`, `sb/browse`, `sb/read`, confirmed `sb/write`, `sb/subscribe-preview`).

Scheme B is right that the address belongs in the topic, but its verb family is **entirely
single-instance** — it was designed for signal adapters where every command targets one endpoint. The
camera adapter has verbs with **no single instance to name**: component-scoped (`sb/list`,
`sb/discover`), a multi-instance verb (`sb/capture-group`), and fleet-wide verbs (`sb/queue-status`,
`sb/queue-clear`). Per-instance *topic* addressing cannot express any of those. Scheme A works around
that by abandoning topic addressing entirely and inspecting the body.

Neither is satisfactory. The optional-instance grammar is a superset of both: it keeps topic addressing
(Scheme B's strength) **and** expresses component/fleet scope natively (Scheme A's need), with no `main`
sentinel and no body routing.

## 3. The grammar

### 3.1 Before

`UNS-CANONICAL-DESIGN.md` §, current grammar (line ~236):

```
[ecv1] [/ {site}]? / {device} / {component} / {instance} / {class} [/ {channel…}]
```

`{instance}` is **mandatory** and defaults to the literal `main` (`MessageIdentity.DEFAULT_INSTANCE`).
Component-scoped traffic — the heartbeat `main/state`, `main/metric`, `main/cfg`, the command inbox
`main/cmd`, `_bcast/main/cmd`, `config/main/cmd` — all carry the `main` sentinel so that a single `+`
at the instance slot matches both component- and instance-scoped messages.

### 3.2 After

```
[ecv1] [/ {site}]? / {device} / {component} [/ {instance}]? / {class} [/ {channel…}]
```

`{instance}` is **optional**:

- **absent** → component/global scope, e.g. `ecv1/{device}/{component}/state`, `ecv1/{device}/{component}/cmd/sb/{verb}`;
- **present** → instance scope, e.g. `ecv1/{device}/{component}/{instance}/state`, `ecv1/{device}/{component}/{instance}/cmd/sb/{verb}`.

The instance lives in exactly one place conceptually — the envelope **identity element**
`{hier, path, component, instance}` — and the topic is derived from it. So the single model change is:
**`identity.instance` becomes optional (nullable/absent) instead of defaulting to `main`.** When present
the topic emits the slot and the wire identity carries `instance`; when absent the topic omits the slot
and the wire identity omits `instance`.

### 3.3 The one invariant — instance id ∉ reserved class tokens

An instance id may never equal a reserved class token: **`state`, `metric`, `cfg`, `log`, `data`,
`evt`, `cmd`, `app`.** This is a one-line addition to the instance-token validator.

Its precise job is to keep the two scope-subscription templates **disjoint** and any context-free reader
unambiguous. Without it, exactly one pathological topic breaks the model: an instance literally named
`cmd` makes `ecv1/dev/me/cmd/#` (component-scope commands) and `ecv1/dev/me/+/cmd/#` (instance-scope
commands) both match `ecv1/dev/me/cmd/cmd/sb/capture`, so the component would receive it twice with
ambiguous scope. Forbid the collision and the templates are provably disjoint.

### 3.4 Why no variable-length parser is needed

Parsing never happens context-free in this design:

- **Building** is fixed per handle: `gg.uns()` (component handle) emits `.../{class}` with no instance;
  `gg.instance("cam1").uns()` emits `.../cam1/{class}`. The builder knows its context and emits one
  fixed shape.
- **Each subscription is a fixed template.** `ecv1/{device}/{me}/+/cmd/#` — instance always at a known
  slot; `ecv1/{device}/{me}/cmd/#` — never an instance. Each is fixed-length in its own context.
- **Each handler therefore parses in a known shape** — it chose the subscription, so it already knows
  whether an instance token is present and where the class sits.

The only context-free reader is the **reserved-class publish guard** (which rejects a raw publish to a
reserved class). It runs on the publish path, where the class position is known from the handle; for the
`publishRaw`/reserved-publisher case that is handed an arbitrary string, it checks the class at the two
candidate slots (component-scope index and instance-scope index), which the §3.3 invariant makes
unambiguous. That is a localized two-position check, not a grammar rewrite.

## 4. Subscriptions and wildcards

Dropping the `main` sentinel trades one property away deliberately: today a single `+` at the instance
slot uniformly catches both scopes; after this change a component-scoped message is one slot shorter and
`ecv1/+/+/+/{class}` will not match it (`+` is exactly one level).

Consequences, and why they are acceptable:

- A **scope-spanning consumer** binds two fixed templates per class: `ecv1/+/+/+/{class}/#`
  (instance-scoped) and `ecv1/+/+/{class}/#` (component-scoped). The six-wildcard fleet set becomes
  twelve, **or** a fleet consumer collapses to `ecv1/+/+/#` and filters class client-side (it filters
  anyway).
- A **component's own command inbox** binds two templates — `ecv1/{device}/{me}/+/cmd/#` (instance) and
  `ecv1/{device}/{me}/cmd/#` (component/fleet) — routing each to a scope-appropriate handler. This is
  not overhead bolted on: the two scopes already want distinct handlers (a single-instance `sb/capture`
  and a fleet `sb/capture-group` are different validation, admission, and reply paths). Two
  subscriptions binding to two handlers that were always two handlers is free.
- The cost lands on the **adapter/component developer**, never on a consumer of `data`/`evt`/`state`
  that does not touch the command plane.

## 5. Command addressing after the change — the D‑CAM‑18 resolution

- **Instance-scoped command:** `ecv1/{device}/{component}/{instance}/cmd/sb/{verb}` — e.g. capture on
  one camera, `sb/capture`, `sb/ptz`, `sb/status`.
- **Component/fleet command:** `ecv1/{device}/{component}/cmd/sb/{verb}` — e.g. `sb/capture-group`,
  `sb/queue-status`, `sb/queue-clear`, `sb/list`, `sb/discover`.

No `main`, no body-`instance`. The camera adapter's verbs map onto scope by presence/absence of the
instance token, exactly as their semantics demand.

`SOUTHBOUND.md` §2.2's verb family is **demoted from mandate to convention**: only `sb/status` is
universal (every southbound adapter implements it); `sb/browse` / `sb/read` / `sb/write` /
`sb/subscribe-preview` become **signal-adapter conventions**, and `writes.allow[]` (D‑U16) stays a
convention for adapters that implement `sb/write`. Component authors may add domain verbs (the camera
adapter's `sb/capture`, `sb/capture-group`, `sb/ptz` are legitimate, not deviations). **D‑U15**
(`data/{channel}` naming) is unaffected and must be left alone.

## 6. Exact canonical amendments (to apply on ratification)

### 6.1 `docs/platform/UNS-CANONICAL-DESIGN.md`

- **Grammar line (~236):** replace the mandatory-instance grammar with the optional-instance grammar of
  §3.2 above, and add the §3.3 invariant to the token-validation list (~253).
- **Default instance (~91, ~320):** retire `DEFAULT_INSTANCE = "main"` and the rule "component-level
  messages default to instance == 'main'." Replace with: component-level messages carry **no** instance
  token (absent = component scope).
- **Lenient parse (~111):** change "missing instance → main" to "missing instance → component scope
  (absent)."
- **Topic examples / integration table (~168, ~405–410):** rewrite `ecv1/{device}/{component}/main/{class}`
  forms to `ecv1/{device}/{component}/{class}`: heartbeat `→ ecv1/{device}/{component}/state`, metric
  `→ .../metric/{name}`, cfg `→ .../cfg`, config-get/set and `_bcast` drop `main`
  (`ecv1/{device}/_bcast/cmd/{verb}`, `ecv1/{device}/config/cmd/get-configuration`).
- **Command addressing (~421):** keep `ecv1/{device}/{component}/{instance}/cmd/{verb}` for
  instance-scope and add the component-scope form `ecv1/{device}/{component}/cmd/{verb}`; a component
  binds both `.../{me}/+/cmd/#` and `.../{me}/cmd/#` (§4).
- **Add decision D‑U28** (text in §10 below) and a subscription-model note (§4).

### 6.2 `docs/platform/DESIGN-uns.md`

- **M9 + §1 banner (~20, ~606–621):** record that the **addressing** of the southbound command family is
  superseded by D‑U28 (optional instance); the **capabilities** (browse/read/confirmed-write/preview)
  survive as conventions, not a mandated per-instance topic family. Update the mandate-walk note.
- Update the `southbound/.../control/*` → UNS migration rows to the optional-instance forms.

### 6.3 `docs/SOUTHBOUND.md`

- **§2.2 heading + §1 banner (~14–21, ~186–213):** change "the `cmd/sb/*` family (Phase 5 / M9 — target
  design)" to the optional-instance addressing of §5; demote the verb table from mandate to convention
  (only `sb/status` universal); keep `writes.allow[]` as a convention. Remove the per-instance-topic
  mandate wording; state the current addressing as plain present fact.

### 6.4 `camera-adapter/DESIGN.md`

- **§12.1:** change the command addressing from "`main` inbox plus body `instance`" to the
  optional-instance topic scheme (instance-scope `.../{instance}/cmd/sb/{verb}`; component/fleet
  `.../cmd/sb/{verb}`). Remove the "renegotiation raised in §27" framing.
- **§27 Q8 + §26 checklist:** mark **resolved** — decided in favour of the optional-instance grammar
  (D‑U28), which supersedes both Scheme A and Scheme B.
- Update the D‑CAM‑18 register row to "Resolved by D‑U28 (optional-instance addressing)."

## 7. Four-language core implementation plan

One concept — *the instance token is optional; absent means component scope* — propagated identically to
all four libraries. **Java is canonical; implement and settle it there first, then mirror.** Grounded in
the current symbols:

### 7.1 Java (`libs/java/`) — canonical

- **`messaging/MessageIdentity.java`** — make `instance` optional. Constructor (`:86`, `:98`) must stop
  defaulting empty/`null` → `DEFAULT_INSTANCE`; preserve absent. `getInstance()` (`:117`) returns an
  `Optional<String>` (or documented-nullable). `toDict()` (`:148`) omits `instance` when absent.
  `fromDict()` (`:206`) keeps a missing `instance` absent (component scope), not `DEFAULT_INSTANCE`.
  Add the §3.3 validation (reject instance ∈ reserved class tokens). Remove/deprecate
  `DEFAULT_INSTANCE`.
- **`messaging/MessageBuilder.java`** — `build()` identity stamping (`:297–298`) stamps the instance
  only when set; no `DEFAULT_INSTANCE` fallback. `withInstance()` (`:245`) unchanged; add a
  component-scope path (default: no instance).
- **`uns/Uns.java`** (`topic`/`topicFor`) — emit the instance slot only when the bound identity carries
  one. `gg.uns()` (component identity) omits it; `gg.instance(id).uns()` includes it.
- **`messaging/ReservedPublisher.java` + `MessagingClient.reservedPublisher()` (`:562`)** — the
  reserved-class guard locates the class token; accept the class at the component-scope slot as well as
  the instance-scope slot (two-position check, safe under §3.3). Extend `ReservedTopicGuardTest`.
- **`commands/CommandInbox.java` (`:427`)** — subscribe **both** `ecv1/{device}/{me}/cmd/#`
  (component/fleet) and `ecv1/{device}/{me}/+/cmd/#` (instance), dispatching to scope-aware handlers.
  `STATUS` (`:464`) stays the universal verb (already per-instance-aware via `instances[]`).
- **`heartbeat/Heartbeat.java`** — publish `state` at component scope (drop `main`); same for the
  `metric` (`metrics/targets/Messaging.java`) and `cfg` (`config/EffectiveConfigPublisher.java`)
  publishers.
- **`uns/RepublishListener.java`** — `_bcast/main/cmd/*` → `_bcast/cmd/*`.
- **`messaging/UnsTestVectors.java`** — regenerate golden topics/envelopes for the optional-instance
  grammar (this is the source the other three languages validate against).

### 7.2 Python (`libs/python/`)

Mirror in `uns.py` (`topic`/`topic_for`), the `MessageIdentity` analog (optional instance, drop the
`main` default and the lenient default in the parser), the `CommandInbox` analog (dual subscribe),
`subscription_handler.py` if the inbox wiring lives there, and the heartbeat/metric/cfg publishers.

### 7.3 Rust (`libs/rust/`)

`uns.rs` (`topic`/`topic_for`; `UnsClass::token` at `:164` already enumerates the reserved set — reuse
it for the §3.3 invariant), `messaging/message.rs` (`instance()` at `:1836`, identity), the
`MessageIdentity` analog (optional instance), `commands.rs` (`CommandInbox`, `OutcomeCommandHandler`/
`FnCommandHandler` — dual subscribe), the reserved guard, and the heartbeat/metric/cfg publishers.

### 7.4 TypeScript (`libs/ts/`)

`message.ts` (`withInstance` at `:779`, identity → optional instance), the `uns` analog, the `commands`
analog (dual subscribe), the reserved guard, and the heartbeat/metric/cfg publishers.

### 7.5 Shared

- **`uns-test-vectors/`** — regenerate from the Java canonical; add component-scope (no-instance) and
  instance-scope cases for every class, plus negative cases for the §3.3 invariant.
- **`test-infra/interop/`** — extend the four language nodes so every language **produces and consumes**
  both a component-scoped and an instance-scoped message/command over the wire.

## 8. Validation (mandatory wire-contract gate)

This changes UNS topic/class behaviour, so per the org rules it is not done until:

1. Per-language unit/coverage green in all four languages (90% line gate each).
2. **`uns-test-vectors/` regenerated** and all four suites pass against them.
3. **Cross-language local-MQTT interop** (`test-infra/interop/`, EMQX): every language acts as producer
   and consumer of both scopes for each affected class and for `cmd` request/reply.
4. **Deployed Greengrass IPC interop on `lab-5950x`**: the four language skeletons exercise both scopes
   over real IPC (the command plane is reachable through IPC, so this is required, not optional).
5. A baseline deployed component regression proving the runtime path (heartbeat/metric/cfg at the new
   topics) did not break.

## 9. Migration off the shipped `main/*` topics — single cutover (ratified)

`main/state` / `main/metric` / `main/cfg` are live (Phases 1–3 merged), but the only deployments are
org-controlled test infra, so migration is a **single coordinated cutover** (§11 decision 2) rather than
a transition window:

1. Land the four-language change (publishers emit the component-scope, no-instance form; the
   `identity.instance` key is omitted when absent) and regenerate `uns-test-vectors/` together.
2. Redeploy all components/skeletons and consumers/`uns-bridge` from the same cut. Old `main/*`
   publishers and new no-instance publishers are never expected to coexist in a live fleet.
3. No wire-version bump (`ecv1` → `ecv2`): the prefix denotes the envelope version, not the topic-shape
   revision, and the cutover replaces the shape atomically.

`main` is not made illegal — an instance genuinely named `main` remains a valid, unambiguous
instance-scoped token (it is not a reserved class token). What is retired is `main`'s use as the
**component-scope sentinel**: component-scope now means the instance token is simply absent.

Note this is independent of the **permanent** two-subscription model in §4: receiving both component-
and instance-scoped commands always requires two subscription templates. The single cutover only removes
any *transitional* need to subscribe the old `main`-form alongside the new form.

## 10. Proposed decision-register entry (D‑U28)

> **D‑U28 — Optional instance token (component vs instance scope). Ratified 2026-07-15.** The UNS
> instance token is optional **for all classes**: present = instance-scoped, absent =
> component/global-scoped. The `main` sentinel instance is retired (an instance genuinely named `main`
> stays a valid instance-scoped token; only its sentinel use is removed). On the wire the
> `identity.instance` key is **omitted when absent**. An instance id may not equal a reserved class token
> (`state`/`metric`/`cfg`/`log`/`data`/`evt`/`cmd`/`app`), which keeps the component-scope and
> instance-scope subscription templates disjoint. A scope-spanning subscriber binds both
> `.../+/{class}/#` and `.../{class}/#`. Migration is a single coordinated cutover across org-controlled
> test infra (no `ecv1`→`ecv2` bump). This supersedes the per-instance-topic **addressing** of the
> southbound command family (M9 / §2.2); the family's capabilities survive as conventions (only
> `sb/status` universal). Resolves camera-adapter §27 Q8 and D‑CAM‑18. D‑U15 (`data/{channel}` naming)
> is unaffected.

## 11. Ratified decisions (2026-07-15)

1. **Scope — all classes.** The optional-instance rule applies to every class (heartbeat/metric/cfg/evt/
   cmd/data/state/log); `main` is retired everywhere. A `cmd`-only version would leave `cmd` omitting the
   instance while `state` still said `main` — the same inconsistency this change exists to remove.
2. **Migration — single coordinated cutover** (§9). Feasible because the only deployments are
   org-controlled test infra; old `main`-form and new no-instance publishers are never expected to
   coexist in a live fleet, so no transition window is needed.
3. **Wire identity — omit `instance` when absent.** The `identity.instance` key is left out of the
   envelope for component-scope (smaller envelope; "absent" is itself the scope signal). Consumers
   reading `identity.instance` treat missing as component scope; this is pinned in the interop vectors.
