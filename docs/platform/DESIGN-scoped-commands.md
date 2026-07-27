# DESIGN — Scoped command handlers with declared verb scope

Status: **accepted direction (2026-07-27), targeting the breaking core release 0.5.0. Phase 1
implemented**: all four core libraries (Java canonical, Python, Rust, TypeScript) carry the
`CommandScope` type, the two-form `register`/`registerOutcome` registration surface, pre-dispatch
addressing enforcement, and the `describe` `scope` field, each with its own test suite; the
`ping`/`describe`/`get-configuration`/`status` built-ins register `BOTH`. The generated service/
processor/sink templates, the four cross-language interop nodes, and the example skeletons are
migrated to the new signature. Remaining rollout (templates' nine-verb `protocol-adapter` family,
the shipping component repos' `sb/*` verbs, and the edge-console `describe` scope rendering) follows
per §3 steps 2-4.

Supersedes issue #82's original additive proposal (a fourth `registerScopedOutcome` variant): the
accepted direction is the breaking model — every handler always receives the addressing, and every
verb declares its scope.

## 1. Problem

D-U28 command topics address either the **component** (`ecv1/{d}/{c}/cmd/{verb}`) or one
**instance** (`ecv1/{d}/{c}/{i}/cmd/{verb}`). 0.4.0's `registerScoped` exposes the topic-addressed
instance to **immediate-reply** handlers only. Deferred/outcome handlers (`registerOutcome` — e.g.
camera-adapter `sb/capture`) cannot see it, so SOUTHBOUND §2.2 topic-authoritative routing is
unenforceable for exactly the verbs where mis-targeting is most consequential (camera D-CAM-29).
Accumulating parallel scoped variants per handler kind grows the surface without ever closing the
inconsistency class: a verb can always be registered through a form that is blind to the envelope.

A second, latent problem: under body-only routing, "no instance named" is overloaded — it already
means "the lone configured instance" (optional-iff-one) — so a verb cannot give component-scope
addressing its own semantics ("act on all instances"). Dual-scope verbs are inexpressible.

## 2. The model

### 2.1 Handler signatures (breaking)

Exactly two registration forms remain; **both** always deliver the addressing:

| Form | Handler shape (per language idiom) |
|---|---|
| `register(verb, scope, handler)` | `(request, addressedInstance) -> result` |
| `registerOutcome(verb, scope, handler)` | `(request, outcomeToken, addressedInstance) -> ()` |

`addressedInstance` is `null`/`None`/`Option::None`/`undefined` for a component-scoped delivery,
else the topic's instance token. The 0.4.0 `registerScoped` variant is **removed** (it existed for
one minor release; migration is mechanical). There is no registration form that cannot see the
envelope — the camera class of gap becomes structurally impossible.

### 2.2 Declared verb scope

`scope` is required at registration: **`COMPONENT` | `INSTANCE` | `BOTH`**.

Library enforcement happens **before dispatch** (the handler never runs on an addressing error).
The library owns **addressing only** — extracting the delivery topic's instance token and the
body's `instance` field, and the conflict/rejection rules below. It does **not** know the
component's configuration, so it never itself decides "the lone configured instance" or "this name
doesn't exist" — that split is spelled out in the `INSTANCE` bullet below and pinned as D-SC-4:

- **Universal, every scope:** a topic instance token and a `body.instance` field that are both
  present and different → `BAD_ARGS` ("instance in body conflicts with the addressed instance"),
  checked **before** anything else.
- `COMPONENT`: an instance-addressed delivery (topic token, or a `body.instance` field) →
  automatic `BAD_ARGS` ("`<verb>` is component-scoped"). The handler's `addressedInstance` is
  always `null`/`None`/`undefined`.
- `INSTANCE`: the handler receives `addressedInstance = topic ?? body ?? null`. A `null` value
  reaches the handler — the library does not reject it. It is the component's own default/
  unknown-instance policy (the legacy optional-iff-one default, `NO_SUCH_INSTANCE` for a name it
  does not recognize) that decides what a `null`/unresolved instance means, because that decision
  needs configuration knowledge the library does not have.
- `BOTH`: the same resolution as `INSTANCE` (`topic ?? body ?? null`), but `null` carries its own
  meaning here — "addressed to the whole component" — a distinct, meaningful signal for the first
  time, not an error or a default to be resolved.

`BOTH` has two intended uses, both first-class:

1. **Scope-indifferent** verbs (built-ins `ping`, `describe`, `get-configuration`, `status`):
   identical answer either way; the handler ignores the parameter.
2. **Dual-semantics** verbs: the handler branches — `null` means component-wide behavior (e.g. a
   future `sb/pause` pausing every instance; an aggregating `sb/status`), a token means targeted
   behavior. Reply shapes may differ per scope; the verb's reference doc defines both.

Widening a verb's scope later (`INSTANCE` to `BOTH`) is **additive and non-breaking**: deliveries
that were errors become meaningful; no existing caller changes behavior. Narrowing is a breaking
change to that verb's contract.

### 2.3 Describe / console

Each `describe.commands[]` entry gains `"scope": "component" | "instance" | "both"` — populating
the field the console protocol already declares. The console derives UI from it: instance selector
for `instance`, no selector for `component`, both affordances for `both`. The field participates in
the existing describe digest. Unknown scope values are rejected at registration, never emitted.

### 2.4 Interaction with existing surfaces

- **Instance-inbox subscription** (both D-U28 filters) is unchanged; only dispatch changes.
- **`setCommandAvailability`** is orthogonal (a verb can be `INSTANCE`-scoped and `disabled`).
- **`sb/discover`'s** hand-specified "component-scoped router that never calls instance
  resolution" (adapter designs, ADP-6) becomes `scope=COMPONENT` — one word replaces a pattern.
- **Templates**: the generated nine-verb family declares `INSTANCE` (lifecycle + signal verbs),
  and drops its hand-rolled `_resolve` topic/body logic in favor of the library resolution;
  `sb/discover`, where a design adds it, declares `COMPONENT`.

## 3. Migration (0.5.0)

Breaking for every registered handler in all four languages. One coordinated wave:

1. Core libs x4: new signatures + scope enforcement + describe field + tests; remove
   `registerScoped`; migrate built-ins (`BOTH`).
2. Templates x4 + examples + interop nodes: mechanical signature migration; delete local routing.
3. Component repos (opcua, modbus, ethernet-ip, camera, file-replicator, telemetry-processor,
   edge-console gateway, uns-bridge as applicable): signature migration; camera additionally moves
   `sb/capture`/`sb/capture-group` onto the scoped outcome form, closing D-CAM-29; adapters delete
   their §2.2 `scope_body`/`resolve_instance` helpers in favor of the library's.
4. Console: render the `scope` field (selector behavior per §2.3).
5. Validation: per-repo gates + the interop matrix for the describe-shape change + a Dallas
   harness pass; a migration note in each repo's release notes.

## 4. Decision register

- **D-SC-1:** Model D accepted — no handler form may be blind to the addressing; parallel scoped
  variants are removed rather than multiplied.
- **D-SC-2:** Scope is a required, per-verb declaration (`COMPONENT`/`INSTANCE`/`BOTH`) enforced
  by the library before dispatch and advertised in `describe`.
- **D-SC-3:** `BOTH` is first-class with two sanctioned uses (scope-indifferent, dual-semantics);
  `null` addressing at a `BOTH` verb means "the whole component".
- **D-SC-4:** The library owns **addressing** — topic/body instance extraction, conflict-first
  `BAD_ARGS` (checked before anything else), and `COMPONENT`-scope rejection of any instance
  addressing. The legacy optional-iff-one default and `NO_SUCH_INSTANCE` for an unresolved/unknown
  instance need configuration knowledge the library does not have, so they **remain
  component-side**, applied by the handler on a `null`/unknown `addressedInstance`. Adapters delete
  their own topic/body/conflict-detection logic but keep the default-resolution and existence
  check.
- **D-SC-5:** Widening a verb's declared scope is additive; narrowing is a per-verb breaking
  change.
- **D-SC-6:** Ships only in a breaking release (0.5.0) as one coordinated wave; 0.4.x keeps the
  additive surface frozen (no `registerScopedOutcome` stopgap).
- **D-SC-7:** (companion, §6) Every multi-instance component populates the state keepalive's
  `instances[]` entries with the instance **state** from the same single state model that answers
  `sb/status`, using the shared `CONNECTING`/`ONLINE`/`BACKOFF`/`PAUSED` vocabulary. Additive —
  the library API exists since 0.4.0; no core change.
- **D-SC-8:** (companion, §6) edge-console renders that state in the fleet/Instances views and its
  miss-detection treats `PAUSED` as expected-quiet (no staleness alarm for a deliberately paused
  instance). Unknown/absent state falls back to today's connectivity-only rendering.
- **D-SC-9:** (companion, §6) The keepalive-state adoption rides the same 0.5.0 coordinated wave
  (one PR per repo covers both changes), though it is technically additive on 0.4.0 and carries no
  breaking risk of its own.

## 5. Open questions

1. Should `INSTANCE` retain the legacy optional-iff-one body fallback forever, or deprecate it so
   callers must always name the instance (topic or body)? Proposed: retain, revisit at 1.0.
2. Does `BOTH` need per-scope request/response schema advertisement in `describe`, or is prose
   documentation per verb sufficient for Release 1 of this model? Proposed: prose first.
3. `status` built-in currently reports per-instance data at component scope; declare `BOTH` with
   identical output, or give instance-addressed `status` a filtered reply? Proposed: identical
   first, filtered as a later additive refinement.

## 6. Companion wave item — keepalive instance state (accepted)

A deliberately paused instance is indistinguishable on every passive surface from one that has
silently gone stale: the keepalive's `instances[]` reports connectivity only, so fleet views show
"connected" while data has stopped, and console miss-detection may alarm on intentional pauses.
The library constraint that once justified this (the pre-0.4.0 `InstanceConnectivity` carried no
state) is gone.

Accepted resolution (D-SC-7..9): fleet-wide adoption, derived from the **single instance state
model** (never a second bookkeeping path — the same source that answers `sb/status`), rendered by
the console with pause-aware miss-detection. Per-repo register notes that recorded the old
constraint (e.g. modbus D-M8/D-M3) are updated by each repo's wave PR. Consumers MUST ignore an
absent/unknown state — the field is additive on the existing keepalive shape.
