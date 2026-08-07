# DESIGN — numeric canonicalization at the config boundary

> Status: **accepted** — decision register **D-NC1**…**D-NC5** below. All four languages are
> implemented on `fix/greengrass-numeric-config`. Per-language implementation status is stated
> in §5.

## Problem

Integer-typed configuration fields fail to parse when the configuration is delivered through a
store that round-trips JSON numbers as 64-bit floats. The AWS IoT Greengrass Nucleus is such a
store: it is a Java process, and its configuration store keeps JSON numbers as `Double`, so a
merged `pollIntervalMs: 5000` comes back over IPC as `5000.0`.

Observed on `lab-5950x` (nucleus 2.17.0): a Rust component that installs and runs fine from its
recipe defaults dies at startup the moment its configuration is changed through a Greengrass
config update —

```text
invalid device config: invalid type: floating point `5000.0`, expected u64
```

— because `serde` will not coerce a JSON float into `u64`. The schema gate does not catch it
first: JSON Schema `"type": "integer"` *accepts* an integral float like `5000.0`, so the deploy
succeeds and the component dies afterwards.

Two facts sharpen the account:

- **Core already normalized numbers — but only for its own sections, piecemeal, and with the wrong
  semantics.** Rust carried three private "lenient u64" deserializers (`config/model.rs`,
  `credentials/config.rs`, `parameters/config.rs`), each with the comment *"Greengrass stores
  configuration numbers as doubles"*. That is why the library's own heartbeat/logging/metrics
  survived a Greengrass `--update-config` while the component-owned `component.*` subtree died.
  Their `f as u64` cast **silently truncated** a fractional value and **saturated a negative to
  `0`**.
- **Java had no startup defect but a document-parity defect.** Gson's tree carries the doubles, and
  core's own config classes coerced with `getAsBigDecimal().intValue()`, which tolerates
  `5000.0` — and **silently truncates** `5.5` to `5`. The same logical configuration was therefore
  a different document in Java than in the other languages, the published effective config on the
  UNS `cfg` class differed per store, and a misconfigured value was silently rewritten rather than
  refused.

## Decision register

### D-NC1 — canonicalize JSON numbers once, at the config intake boundary, in core

A pure, idempotent pass walks the raw configuration document and rewrites every JSON number whose
value is integral and inside the shared 64-bit window into an integer JSON number. Everything else
is left byte-identical.

The pass runs at the **configuration intake boundary** of each language's config manager, before
schema validation, before component-supplied candidate validators, and before the snapshot the
component reads. The contract it establishes is:

> Any configuration value a component obtains from the library — the full document, the global
> subtree, an instance subtree, the envelope tags, the published `cfg` document — carries integral
> numbers in integer form, whatever the configuration store did to them.

It is deliberately **whole-document**: the component-owned `component.global` and
`component.instances[]` subtrees are the ones that were never covered, and `tags` is included per
D-NC5.

**Rejected alternatives.**

- *Shared lenient deserializers that component authors annotate their integer fields with.*
  Opt-in, and the observed failure is precisely that nobody opts in — core's own coverage was
  piecemeal after four attempts. It also does nothing for raw-value probes, candidate validators,
  or the published effective configuration.
- *Fixing only the Greengrass IPC value conversion.* Covers one transport, is feature-gated out of
  the normal CI suite, and misses store-shaped numbers arriving through a config component's
  lineage bundles.
- *Repairing the store (write-side / forced `RESET` on deploy).* Cannot fix third-party stores,
  races the Nucleus, and is unnecessary once the reader canonicalizes. The fix is read-side and
  unconditional, so a store already holding `5000.0` parses correctly on the next component start
  or config update — **`RESET` is never needed for this defect**, including for stores written
  before the fix.

**Not canonicalized:** any verbatim byte-match report path, notably the `ComponentConfig` string a
SHADOW source reports back to clear the delta. Canonicalization happens strictly after parse and is
never re-serialized into such a report.

**Component-author impact: none.** No annotation, no code change; the fix reaches a component when
its core dependency is updated and it is rebuilt.

### D-NC2 — the legacy lenient coercions are aligned with the same semantics

The pre-existing lenient coercions in core's own config classes are rewritten on the shared rule:
an integral value in any numeric encoding is accepted; a fractional value, a negative value in a
non-negative field, and a value outside the target type's range are **rejected loudly**.

This is a **behavior tightening**, accepted deliberately: a configuration that previously slipped a
`5.5` into one of these fields through a non-schema-validated path now errors instead of silently
becoming `5`. Silently rewriting a user's configuration is a worse defect than the one being fixed.
In the normal pipeline the schema gate rejects these values first (every field concerned is schema
-typed `integer` with `minimum: 1`), so the tightening bites only on direct-construction paths.

### D-NC3 — TypeScript needs no canonicalization pass; the divergence is accepted

JavaScript has a single number type: the decoded `5000.0` **is** `5000` (`JSON.parse('5000.0') === 5000`,
and it re-serializes as `5000`). The distinction the other three languages must repair is
unrepresentable, so the delivered document is already canonical for every integral value and there
is nothing to rewrite. TypeScript therefore runs **no canonicalization pass** — writing one would be
a no-op dressed up as parity. Its deliverable is a contract test pinning the invariant plus this
note.

Verified against the delivery path rather than assumed. Every configuration source parses JSON
(`JSON.parse` for FILE / ENV / CONFIGMAP / SHADOW / CONFIG_COMPONENT), and the Greengrass leg is no
exception: `GreengrassConfigSource.load` calls `IpcMessagingProvider.getConfiguration`, which returns
the IPC SDK's `resp.value`, and that SDK's eventstream deserializer is
`JSON.parse(payload_text)` followed by an identity `deserializeGetConfigurationResponse` — no
per-field conversion, no numeric type to preserve. Nothing on the path can surface a distinct
"float". The protobuf codec agrees: `encodeEcValue` selects `IntValue` on `Number.isInteger`, so an
integral tag value encodes identically whatever the store wrote (D-NC5 holds in TypeScript with no
work).

The inherent limit — JavaScript cannot represent integers beyond 2^53 exactly, and cannot hold the
top of the unsigned 64-bit window at all (`18446744073709551615` parses to `2^64` and is refused
like any other out-of-window value) — is pre-existing and unchanged. It is the same
double-precision ceiling the Greengrass store itself imposes, so TypeScript is not the weakest link
on that path. Documented as a platform limit, not fixed.

**What TypeScript did need: D-NC2.** The no-op verdict covers the canonicalization pass only.
TypeScript carried the same silent-rewriting defect the other legs removed: `config/model.ts` read
every integer-typed setting through a `Math.trunc` helper — the exact analogue of Rust's `f as u64`
and Java's `getAsBigDecimal().intValue()`, turning `5.5` into `5`, and letting a negative value fall
through a range guard to the schema default. `parameters/config.ts` truncated `refreshIntervalSecs`
the same way. Those reads are now on the shared rule (§5), which also makes the published
user-facing statement ("a fractional value in an integer-typed setting is rejected rather than
rounded or truncated") true in TypeScript. **TypeScript is "no pass needed", not "exempt".**

### D-NC4 — no string or boolean coercion

Strings and booleans are never coerced, however numeric they look. Coercing `"5000"` would corrupt
legitimately-string settings (version literals such as `"1.0"`, identifiers, sizes), and no store
we target stringifies numbers. A string or boolean in an integer-typed field fails downstream
exactly as it does today.

### D-NC5 — `tags` is inside the canonicalized subtree

Configuration `tags` values flow into every message envelope and the protobuf codec types them, so
an unrepaired integral-double tag value would encode as a double on a Greengrass deployment and as
an integer on a host or Kubernetes deployment for the *same logical configuration*. Canonicalizing
the whole document, `tags` included, removes that per-platform skew rather than enshrining it.

Accepted knowingly, including the wider blast radius: a change to emitted wire data brings the
cross-language interop vector and the four-language Greengrass leg into the validation scope.

**Bound on the skew (found during implementation, Java):** the canonical schema types every `tags`
value as a string —

```json
"tags": { "type": "object",
          "patternProperties": { "^[a-zA-Z0-9_-]+$": { "type": "string" } },
          "additionalProperties": false }
```

— so a numeric tag value never survives a schema-validated configuration document, on any store.
The tag-typing skew is therefore reachable only on paths that bypass the schema gate, and D-NC5's
practical effect is to make the intake boundary guarantee it can never appear at all rather than to
change wire bytes on a schema-valid Greengrass deployment. Pinned by
`ConfigManagerNumericCanonicalizationTest.aNumericTagIsRefusedByTheSchemaGate`. Whether this
bound removes the interop/Greengrass legs from scope is a call for the validation plan, not a
change to the decision.

## §3 — exact semantics (normative, all languages)

Applied recursively to every number in objects and arrays; **object keys are never touched**.

> A JSON number is rewritten as an integer if and only if its value is finite, has no fractional
> part, and falls inside the shared 64-bit window:
>
> - `[-2^63, 2^63 - 1]` → a signed 64-bit integer;
> - `(2^63 - 1, 2^64 - 1]` → an unsigned 64-bit integer.
>
> Anything else is left byte-identical. Strings, booleans, and `null` are **never** coerced.

Languages that keep the source text of a parsed number (Java/Gson, via `BigDecimal`) decide
exactness on the **decimal** value, so `5000.00` and `5e3` are recognized without a lossy
intermediate conversion. Languages holding a 64-bit float (Rust/`serde_json`) decide it by exact
round-trip: `f.fract() == 0.0` and the integer candidate converts back to the identical float.
Both rules select the same set of values.

Behavior for an integer-typed field (`u64`, or Java `int`/`long`):

| Value arriving at the field | After the pass | Outcome |
|---|---|---|
| `5000` | `5000` | parses |
| `5000.0` | `5000` | **parses** — the defect, fixed |
| `5000.00`, `5e3` | `5000` | parses |
| `5000.5` | `5000.5` (untouched) | **loud failure** — never truncated to `5000` |
| `-5`, `-5.0` | `-5` | loud failure in a non-negative field — never clamped to `0` or a default |
| `-0.0` | `0` | parses as `0` (IEEE `-0.0 == 0.0`) |
| `1e19` (integral, ≤ 2^64-1) | `10000000000000000000` | parses in an unsigned 64-bit field |
| `1e20` (integral, > 2^64-1) | `1e20` (untouched) | loud failure |
| `2^63` | `9223372036854775808` | parses unsigned; loud failure in a signed 64-bit field |
| value > 2^53 written through a store that uses doubles | whatever double the store kept | precision was destroyed upstream; only exactly-representable integers convert. Documented platform limit, not repairable at this layer |
| `NaN` / `Infinity` | untouched (they have no JSON literal and no integral value) | loud failure |
| `true`, `"5000"` | untouched | loud failure (D-NC4) |

The whole system therefore tells one story: *an integral value in any numeric encoding is accepted;
a fractional value in an integer-typed field is loudly rejected; nothing is ever silently
rewritten.*

## §4 — parity definition

Parity is **identical observable behavior**, not identical code:

1. the configuration document a component receives has integral numbers in integer form in all four
   languages, whatever the store did to them;
2. accept/reject outcomes at the library gates are identical — integral accepted, fractional in an
   integer-typed library field loudly rejected;
3. the published `cfg` document is canonical in all four.

Rejection of fractional values in **component-defined** fields necessarily lives in each component's
own parser (Rust's `serde` gives it for free; Gson/Python/JS components keep their historical
tolerance). That asymmetry predates this change and is out of scope; the library-level contract
above is what parity governs.

## §5 — placement per language

### Java (canonical) — implemented

| Piece | Where |
|---|---|
| The pass and the strict reads | `libs/java/.../config/ConfigNumbers.java` — `canonicalizeNumbers(JsonElement)`, `canonicalized(JsonObject)`, `requireNonNegativeInt(JsonElement, String)` |
| Startup intake | `ConfigManager` constructor — canonicalizes the document before `CandidateValidationRunner.validate` and before `prepareSnapshot` |
| Reload intake | `ConfigManager.tryApplyConfig` — canonicalizes in place of the candidate's `deepCopy()`, i.e. before `ConfigurationValidator.validate`, the component validators, and the snapshot |
| D-NC2 sites | `HeartbeatConfiguration` (`heartbeat.intervalSecs`), `MetricConfiguration` (`metricEmission.targetConfig.intervalSecs`, `metricEmission.targetConfig.port`) — was `getAsBigDecimal().intValue()` |

Implementation notes:

- Rewriting uses `BigDecimal`: Gson's `LazilyParsedNumber` preserves the source text, so exactness
  is decided on the decimal value rather than on double artifacts. Integral and within `long` →
  a `long`-backed `JsonPrimitive`; integral within `(2^63 - 1, 2^64 - 1]` → a `BigInteger`-backed
  `JsonPrimitive`, so the whole unsigned 64-bit window survives.
- Both intake sites use `ConfigNumbers.canonicalized(...)`, which deep-copies first, so the
  provider's (or caller's) document is never mutated behind its back and the committed snapshot is
  isolated from later edits to the source object.
- The startup path's schema gate runs earlier, in `ConfigManagerFactory.validateConfiguration`,
  and therefore sees the pre-canonical document. This is not observable: JSON Schema already treats
  an integral float as an `integer`, and the pass changes neither type nor magnitude for any
  schema keyword. The reload path canonicalizes before its schema gate.
- **Assertions must be on serialized text.** Gson's `JsonPrimitive.equals` compares numbers by
  their `double` value, so `5000` and `5000.0` compare *equal* as `JsonElement`s — the exact
  distinction this design is about. Tests assert `toString()`.

Rejection messages (the cross-language reference wording):

```text
configuration value '<path>' must be a number, but was <value>
configuration value '<path>' must be a finite number, but was <value>
configuration value '<path>' must be a whole number, but was <value>
configuration value '<path>' must not be negative, but was <value>
configuration value '<path>' is out of range for a 32-bit integer: <value>
```

The rejection is an `IllegalArgumentException`, which reaches the caller as a startup failure at
construction and as a rejected candidate (previous generation retained) on reload.

### Rust — implemented

| Piece | Where |
|---|---|
| The pass and the strict reads | `libs/rust/src/config/canonicalize.rs` — `canonicalize_json_numbers(&mut Value)` (public, re-exported as `edgecommons::config::canonicalize_json_numbers`), plus the crate-internal `de_integral_u64` / `de_integral_opt_u64` / `de_integral_usize` / `de_opt_f64` / `as_integral_u64` |
| Snapshot intake | `config::model::Config::from_value` — canonicalizes before `serde_json::from_value` and before the document is stored, so `raw`, `parsed`, `global()`, `instance()`, `instance_ids()`, and `tags` are all canonical. Covers init, reload, every source, and direct callers (component tests) |
| Pipeline intake | `config::layered::effective_from_source_payload` — the single point `LayeredConfigSource`'s `load`, `watch`, and the `reload-config` re-fetch all pass through, ahead of `config::validation::validate`, the candidate validators, and `Config::from_value`. A `CONFIG_COMPONENT` bundle is canonicalized envelope-first, so `lineageVersion` and every layer fragment are canonical before the merge |
| D-NC2 sites | `config/model.rs` (`logging.publish.maxRecordBytes`, `logging.publish.queue.maxRecords`, `logging.fileLogging.backupCount`, `heartbeat.intervalSecs`, `health.port`, `messaging.requestTimeoutSeconds`, and the `metricEmission.targetConfig` probes), `credentials/config.rs` (`vault.keepVersions`, `vault.cacheTtlSecs`, `central.refreshIntervalSecs`), `parameters/config.rs` (`refreshIntervalSecs`) — the three private "lenient u64" copies are deleted and all three modules now use the one shared implementation |

Implementation notes:

- Exactness is decided by exact round-trip on the `f64`, with an explicit `f < 2^64` bound. The bound
  is load-bearing: `u64::MAX as f64` rounds *up* to exactly `2^64`, so a naive
  `(f as u64) as f64 == f` check would accept `2^64` and silently store `u64::MAX`. A negative uses
  the `i64` round-trip, where `i64::MIN` is exactly representable and needs no such bound.
- A positive value is emitted as an unsigned `Number`. `serde_json` stores a non-negative `i64` in
  the same representation, so `json!(5000)` and the canonicalized `5000.0` compare equal and both
  deserialize into `i64`, `u64`, and `f64` fields.
- Applying the pass at both intake points is deliberate defense in depth: it is idempotent, so the
  second application is a no-op.
- `as_integral_u64` backs the three free-form `metricEmission.targetConfig` accessors, which return
  a plain `u64`/`u16` and have no error channel. Their documented contract is to fall back to the
  schema default; sharing the rule means they no longer truncate `5.5` to `5` or saturate `-5.0` to
  `0` on the way there.

D-NC2 rejection messages (`serde` custom errors, surfaced by the caller's own wrapping — e.g. a
component's `invalid device config: {e}`):

```text
expected an integer, got the fractional number 5.5
expected a non-negative integer, got -5
expected an integer within the 64-bit range, got 100000000000000000000
expected an integer, got "300"                      (also `true`, `null`, objects, arrays)
expected an integer within this platform's usize range, got <value>
expected a number, got "1.5"                        (the float-typed reader)
```

An unannotated component field keeps `serde_json`'s own wording, which is what the reported failure
showed: `invalid type: floating point \`5000.5\`, expected u64` for a fractional value and
`invalid value: integer \`-5\`, expected u64` for a negative one.

The Greengrass IPC value conversion (`ipc.rs`) is left unchanged; the schema is unchanged.

Rust tests: `config/canonicalize.rs` (the §3 table, nesting, key immutability, idempotency, `-0.0`,
the 2^53 boundary, the unsigned-64-bit window edges including `2^64` staying a float, and every
rejection message); `config/model.rs::greengrass_shaped_config_tests` (a full store-shaped document
through `Config::from_value`, a scaffolded adapter's plain `u64` struct parsing it, the fractional
and negative refusals, canonical `tags`, and poisoned-store recovery); `config/layered.rs` (direct
sources and lineage bundles at pipeline intake); `config/validation.rs` (the schema gate accepts a
doubles-shaped document and still rejects a fractional value in an integer field);
`lib.rs::reload_tests::a_greengrass_shaped_reload_payload_produces_a_canonical_snapshot` (the reload
path end to end, with the candidate validator observing the canonical document);
`messaging/message.rs::config_tags_encode_the_same_ec_value_type_on_every_platform` (D-NC5 at the
protobuf codec); and `tests/config_hot_reload.rs` (the broker-gated full-runtime reload leg).

### Python — implemented

| Piece | Where |
|---|---|
| The pass and the strict reads | `libs/python/edgecommons/config/canonicalize.py` — `canonicalize_json_numbers(document)` (public, re-exported as `edgecommons.config.canonicalize_json_numbers`), plus `require_non_negative_int` / `require_non_negative_integral` / `as_integral_int` |
| Pipeline intake | `ConfigManager._effective_from_source_payload` — the single point `init()`, `configuration_changed`, and the `reload-config` re-fetch all pass through, ahead of `ConfigurationValidator.validate`, the candidate validators, and the snapshot. A `CONFIG_COMPONENT` bundle is canonicalized envelope-first, so every layer fragment is canonical before the deep merge |
| Snapshot intake | `ConfigManager._prepare_snapshot` — canonicalizes the effective document and both retained layers as it copies them, so `get_full_config()`, `get_effective_config()`, `get_global_config()`, `get_instance_config(..)`, the tags, and the typed section models are canonical. Extends the guarantee to `_apply_config`, the legacy seam that bypasses the pipeline |
| D-NC2 sites | `heartbeat_config.py` (`heartbeat.intervalSecs`), `health_config.py` (`health.port`), `metric_config.py` (`metricEmission.targetConfig.port`, `metricEmission.targetConfig.intervalSecs`), `enhanced_logging_config.py` (`logging.fileLogging.backupCount`), `logs.py::_positive_int` (`logging.publish.maxRecordBytes`, `logging.publish.queue.maxRecords`), `credentials/config.py` (`credentials.vault.keepVersions`, `credentials.central.refreshIntervalSecs`), `parameters/config.py` (`parameters.refreshIntervalSecs`), `metrics/targets/cloudwatch.py` (the `buffer.maxDiskBytes` / `buffer.segmentBytes` probes) |

Implementation notes:

- Python is dynamically typed, so the defect does not crash the way it does in Rust — it drifts.
  An integral double flows on as a `float` into the delivered `component.*` subtrees, into
  `isinstance(value, int)` checks in component code, into the published `cfg` document, and into
  the envelope tags. The pass converts a `float` whose `is_integer()` holds and whose value is
  inside the shared window to `int`; everything else is untouched. Python integers are unbounded,
  so the window exists purely for parity of the rule; the comparison against it is exact, because
  Python compares an `int` with a `float` by value.
- `bool` is excluded explicitly from every numeric check — it subclasses `int`, so a naive
  `isinstance(v, (int, float))` would treat `True` as the number 1. Converting floats only is what
  avoids the trap.
- The pass is pure: it deep-copies and rewrites the copy, so a provider's (or a caller's) document
  is never mutated behind its back and the committed snapshot stays isolated from later edits to
  the source object. That copy replaces the `copy.deepcopy` the snapshot builder already did, so
  the isolation guarantee is unchanged.
- **Python had a hard failure of its own, not just document drift.** `logs.py::_positive_int` was
  `isinstance(value, int)`-only, so `logging.publish.maxRecordBytes` / `queue.maxRecords` delivered
  as doubles raised and took the whole logging section — and with it startup, or a reload — down.
  It now shares the rule and accepts either encoding.
- The rejections raise `ValueError`, Python's counterpart to Java's `IllegalArgumentException`, and
  reach the caller the same way: a startup failure from `init()`, and a rejected candidate
  (previous generation retained) from `configuration_changed`. The credentials and parameters
  subsystems re-raise the identical message inside their own `CredentialError` / `ParameterError`
  so each module keeps its documented error type.

D-NC2 rejection messages — the canonical Java wording, verbatim:

```text
configuration value '<path>' must be a number, but was <value>
configuration value '<path>' must be a finite number, but was <value>
configuration value '<path>' must be a whole number, but was <value>
configuration value '<path>' must not be negative, but was <value>
configuration value '<path>' is out of range for a 32-bit integer: <value>
configuration value '<path>' is out of range for a 64-bit integer: <value>    (the unsigned reader)
configuration value '<path>' must be positive, but was <value>                (logging.publish sizes)
```

**Bound on the SHADOW report path.** Rust holds the verbatim `ComponentConfig` string a SHADOW
source reports back, so canonicalization cannot reach it. Python has no such verbatim path: it
re-serializes the accepted snapshot (`json.dumps(get_effective_config())`), which already
normalized the desired document's formatting before this change and now also reports integral
numbers in integer form.

Python tests: `libs/python/tests/test_config_numeric_canonicalization.py` — the §3 table, nesting,
key immutability, idempotency, purity, `-0.0`, the 2^53 boundary, the unsigned-window edges
(including 2^64 staying a float), `bool` non-coercion, and every rejection message; then the
pipeline, which reproduces the Greengrass delivery shape with no device (the `Map<String, Object>`
the IPC SDK returns, values as Python `float`) and pushes it through a real `GreengrassConfigManager`
whose only stub is the IPC client: the delivered instance/global subtrees, tags, library sections,
`get_full_config()` and the redacted `cfg` document are canonical, a genuinely fractional value
survives as a `float`, candidate validators and change listeners observe the canonical document, a
lineage bundle is canonical before the merge, and a store poisoned before the fix is read correctly
with no `RESET` — including a further config update applied live against it.

### TypeScript — implemented (no pass, D-NC3; the shared rule at every integer-typed read, D-NC2)

| Piece | Where |
|---|---|
| Canonicalization | **None, deliberately** (D-NC3) — `JSON.parse` has already delivered the canonical value on every source, Greengrass IPC included |
| The strict reads | `libs/ts/src/config/numbers.ts` — `requireNonNegativeInteger(value, path)` for a setting with an error channel and `asNonNegativeInteger(value)` for a probe without one; both re-exported from `config/index.ts`, as Java exports `ConfigNumbers` |
| D-NC2 sites | `config/model.ts` (`logging.fileLogging.backupCount`, `logging.publish.maxRecordBytes`, `logging.publish.queue.maxRecords`, `heartbeat.intervalSecs`, `health.port`, and the `metricEmission.targetConfig` probes `intervalSecs` / `port` / `buffer.maxDiskBytes`), `parameters/config.ts` (`refreshIntervalSecs`), `credentials/config.ts` (`vault.keepVersions`, `central.refreshIntervalSecs`) — the single `Math.trunc`-based `asInt` helper is deleted, not wrapped |

Implementation notes:

- A rejection is an `EdgeCommonsError` of kind `Config` thrown out of `Config.fromValue`, which the
  runtime already treats the way Java does: a startup failure on first load, and a rejected
  candidate with the previous generation retained on hot reload (`edgecommons.ts` catches it and
  logs *"reloaded config could not be parsed; keeping previous"*).
- The accessor split follows **Rust**, not Java: the free-form `metricEmission.targetConfig` reads
  have no error channel and their documented contract is to fall back to the schema default, so they
  use the probe. Sharing the rule is what matters — `6.5` falls back to the default `5`, it never
  becomes `6`. (Java parses those two fields in a constructor and therefore rejects them; that
  asymmetry between Java and Rust predates this work.)
- An absent or `null` setting reads as absent and the caller applies its schema default — Rust's
  `de_integral_opt_u64` shape.
- The window is the shared one, expressed in the only terms JavaScript has: a value at or above
  `2^64` is rejected. There is no separate 2^53 gate, because a value above 2^53 was already
  rounded by `JSON.parse` before any library code ran — see the platform limit under D-NC3.
- `credentials.vault.cacheTtlSecs` has no TypeScript consumer, so there is no read to tighten; the
  Rust field of that name is read by its `SyncEngine`.

Rejection messages (Java's wording, with JavaScript's range term):

```text
configuration value '<path>' must be a number, but was <value>
configuration value '<path>' must be a finite number, but was <value>
configuration value '<path>' must be a whole number, but was <value>
configuration value '<path>' must not be negative, but was <value>
configuration value '<path>' is out of range for a 64-bit integer: <value>
```

TypeScript tests: `libs/ts/test/config_numeric_canonicalization.test.ts` — a store-shaped document
(every number written `x.0`, in the library sections, `component.global`, and `component.instances[]`)
parsed from JSON **text**, because that is what the wire does; it deep-equals *and* re-serializes
byte-identically to its integer form, survives the schema gate, and arrives canonical through
`Config.fromValue` with fractional values (`0.5`, `1.5`) untouched. A strict typed consumer standing
in for a Rust adapter's `serde` struct parses the delivered instance subtree and still refuses a
genuinely fractional one; the published `cfg` document is canonical; the envelope tags of a
store-shaped and a file-shaped document encode to identical protobuf bytes carrying `IntValue`; and
the D-NC2 rejections and probe fallbacks are pinned value by value, including that `6.5` yields the
default and never `6`.

## §6 — schema

**Unchanged.** No new keys, no type changes; `schema/edgecommons-config-schema.json` and its sync
script are not touched by this work. The single-source rule is satisfied by not touching it.

## §7 — validation

| Leg | Scope |
|---|---|
| Per-language unit tests | The §3 table in every language, plus nesting, key immutability, idempotency, `-0.0`, the 2^53 boundary, and the unsigned 64-bit window edges. Java additionally: source-text exactness (`5000.00` → `5000`, `0.1` untouched). Python additionally: `bool` non-coercion. |
| Per-language pipeline tests | A store-shaped document (every number `x.0`) through config-manager intake: the snapshot, the component subtrees, the tags, and the published `cfg` document are canonical; a strict typed consumer parses the delivered document and fails on the raw one. Coverage gates hold. |
| Cross-language interop | Only if the tag-typing skew of D-NC5 is judged reachable — see the bound recorded under D-NC5. |
| Greengrass deployed regression (`lab-5950x`) | Fresh install → RUNNING; `--update-config` with an integer value → RUNNING with the merged value applied (previously BROKEN — the primary proof); recipe redeploy against a doubles-holding store → RUNNING; **poisoned-store recovery**: a store poisoned before the fix, new build deployed with no `RESET` → RUNNING. |
| Rust `greengrass` feature build | WSL/Linux, per the validation matrix. |
| Kubernetes | No ConfigMap-path behavior change (raw JSON is already integral); optional smoke, not a gate. |

Java tests: `libs/java/src/test/java/com/mbreissi/edgecommons/config/ConfigNumbersTest.java` and
`ConfigManagerNumericCanonicalizationTest.java`. The latter reproduces the Greengrass delivery
shape with no device: it builds the `Map<String, Object>` the IPC SDK returns (values as Java
`Double`) and runs it through the same `gson.toJson` → `gson.fromJson` conversion
`GreengrassConfigProvider.loadConfiguration()` performs.
