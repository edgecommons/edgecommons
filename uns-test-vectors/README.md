# UNS cross-language conformance vectors

These files pin the **normative edgecommons unified-namespace (UNS) grammar** (see
`docs/platform/UNS-CANONICAL-DESIGN.md` §2.2/§4.1 and `docs/platform/DESIGN-uns.md`):
topic building, topic validation, subscription filters, the reserved-class publish
guard, and the golden canonical message envelopes. The Java reference implementation
generates and verifies them; the Python, Rust, and TypeScript ports **must** pass the
same conformance checks so every language builds **byte-identical topics** and
**structurally identical envelopes** (D-U22).

## Files

| File | What it is |
|------|------------|
| `topics.json` | `build` / `validate` / `filter` / `guard` case groups (inputs + expected outputs or error codes). |
| `envelopes.json` | One golden **full canonical JSON** envelope per UNS class, with pinned `uuid`/`correlation_id`/`timestamp`. |
| `bcast.json` | The `_bcast` **republish** (reconnect-rehydration) contract: the two broadcast command topics, the golden notification envelopes, and the normative listener behavior constants. |
| `commands.json` | The **command-inbox** contract (the minimal `commands()` facade): the own-inbox wildcard, the five built-in verbs' golden request/reply pairs, the unknown-verb error reply, and the normative dispatch behavior. |
| `data.json` | The **`data()`** publish-facade contract (DESIGN-class-facades §2.1): the constructed `SouthboundSignalUpdate` body + defaulting (quality → `GOOD` + `qualityRaw:"unspecified"`, `serverTs` → now, samples wrapper), channel sanitization, the missing-`signal.id` reject, and channel routing. |
| `evt.json` | The **`events()`** publish-facade contract (DESIGN-class-facades §2.2): the `evt/{severity}/{type}` channel **derived from the body**, the four severity tokens, `timestamp` → now, and `raiseAlarm`/`clearAlarm` `alarm`/`active`. |
| `app.json` | The **`app()`** publish-facade contract (DESIGN-class-facades §2.3): body verbatim, header `name` = the caller's name, topic = `app/{channel}` (sanitized). |

The files are UTF-8; some inputs deliberately contain raw C1 control bytes
(U+0085 etc.) — parse them as JSON, do not preprocess.

## topics.json case groups

Every case is `{name, input, expected}`. Failure cases are **single-fault** (D-U26)
so all four languages fail with the identical machine-readable code.

- **build** — input `{hierarchyLevels, identityValues, component, instance,
  includeRoot, class, channel?}` → expected `{topic}` or `{error}`.
  Contract: pair `hierarchyLevels[i]` with `identityValues[<level>]`;
  **`identityValues` and `component` pass through the language's template sanitizer
  first** (`ConfigManager.sanitize` semantics: `/`, `\`, `+`, `#` and ISO control
  characters — including C1 U+0080–U+009F — each become `_`, then any remaining `..`
  becomes `_`). That models the config identity-resolution path and pins the D-U26
  equivalence "sanitized ⇒ valid". **`instance` and `channel` are used verbatim**
  (they are validated tokens, never sanitized). A missing `channel` key means
  "no channel"; an empty `channel` string also means "no channel".
- **validate** — input `{topic, includeRoot}` → `{ok: true}` or `{error}`.
  Validation is includeRoot-sensitive (class position 4 rootless / 5 rooted). Bind
  the validator to an identity with a **multi-level hierarchy** (≥ 2 levels) so the
  `includeRoot` input is the effective root mode — D-U25 makes `includeRoot` a no-op
  for single-level hierarchies.
- **filter** — input `{class, scope{site?, device?, component?, instance?},
  includeRoot}` → `{filter}`. Absent scope fields render as `+`; channeled classes
  get a trailing `/#`; leaf classes (`state`, `cfg`) end at the class token. Same
  multi-level binding note as `validate`.
- **guard** — input `{topic, includeRoot}` → `{reserved: true|false}`. The §4.1
  reserved-class predicate (D-U24): reserved iff `tokens[0] == "ecv1"` and
  `tokens[4]` (always) or `tokens[5]` (only when `includeRoot` is true) is one of
  `state | metric | cfg | log`. Non-`ecv1` topics always pass.

Topics and filters compare **byte-for-byte**; error codes compare **exactly** against
the pinned §2.2 set: `EMPTY_TOKEN, BAD_CHAR, TRAVERSAL, DEPTH_EXCEEDED,
LENGTH_EXCEEDED, CHANNEL_ON_LEAF, CHANNEL_REQUIRED, BAD_ROOT, BAD_CLASS,
WILDCARD_IN_TOPIC`.

## envelopes.json conformance contract

Each vector is `{name, class, channel?, topic, envelope}` where `envelope` is the
full canonical wire JSON `{header, identity, body}`. Every language must:

1. **Rebuild the envelope** through its message builder with the explicit
   uuid / timestamp / correlation_id setters and the vector's `identity`
   (`envelope.identity` parsed with the lenient wire parser), then assert
   **structural equality** with `envelope` — same key set and values; JSON member
   order is **not** normative (D-U22).
2. **Reproduce `topic` byte-for-byte** from the vector identity + `class` +
   `channel` with `includeRoot=false` (all envelope vectors are rootless).

Notes: the two `state` vectors pin the heartbeat-state body shapes — RUNNING carries
`uptimeSecs`, STOPPED does not (§4.3 / D-U14). The `state`/`cfg` envelope versions
are pinned to `"1.0"`. Bodies of the other classes are representative payloads (the
envelope structure is the contract, not the body schema). No envelope carries `tags`
(built without a config-bound builder) or `reply_to`.

## bcast.json republish contract

Pins the `_bcast` **republish** (reconnect-rehydration) surface — the DESIGN-uns
§9.3-layer-2 / §9.4 late-join lever the `uns-bridge` drives on every site-reconnect
rising edge. The document is `{device, commands[], behavior}`:

- **commands** — exactly two, in order `republish-state`, `republish-cfg`. Each is
  `{name, republishes, topic, input, envelope}`:
  - `topic` is rebuilt **byte-for-byte** from `input`
    (`{device, component: "_bcast", instance: "main", includeRoot: false,
    class: "cmd", channel: <name>}`) through the language's topic builder — the
    reserved `_bcast` pseudo-component pinned to the device, single-level hierarchy,
    so the topic is rootless by D-U25:
    `ecv1/{device}/_bcast/main/cmd/republish-state|republish-cfg`.
  - `envelope` is the golden **notification** the bridge publishes: header
    `{name: <verb>, version: "1.0", timestamp, uuid, correlation_id}`, body `{}` —
    **no `identity`, no `tags`, no `reply_to`** (fire-and-forget). Rebuild through the
    message builder (pinned setters, no identity) and compare **structurally**
    (D-U22).
- **behavior** — the normative republish-listener constants every language implements:
  `jitterWindowMs` (an accepted broadcast re-announces after a uniformly random delay
  in `[0, jitterWindowMs]`), `cooldownMs` (per verb, at most one re-announce per
  cooldown window, measured from the last **accepted** trigger; everything else
  coalesces), `replyTo: false` (never reply). The listener triggers only when the
  envelope `header.name` equals the topic's verb; malformed/foreign payloads are
  ignored, never crash. `republish-state` re-emits the heartbeat `state` keepalive
  (respecting `heartbeat.enabled`); `republish-cfg` re-runs the effective-config
  (`cfg`) publisher. See `docs/platform/DESIGN-uns.md` §9.4.

## commands.json command-inbox contract

Pins the component **command inbox** — the minimal `commands()` facade
(DESIGN-uns §7.3/§9.5, the edge-console slice S2). The document is
`{inbox, verbs[], errors[], behavior}`:

- **inbox** — `{filter, input}`: the own-inbox wildcard every component subscribes
  on its PRIMARY connection at startup, rebuilt **byte-for-byte** from `input`
  (`{device, component, instance: "main", includeRoot: false, class: "cmd"}`)
  through the language's filter builder with every scope token pinned:
  `ecv1/{device}/{component}/main/cmd/#`. Unsubscribed on shutdown, before
  messaging closes. Only the `main`-instance inbox exists in this slice.
- **verbs** — the five built-in verbs, in order `ping`, `describe`, `reload-config`,
  `get-configuration`. Each is `{name, verb, topic, request, reply}`:
  - `topic` is rebuilt byte-for-byte (the **verb is the `cmd` channel**;
    `/`-namespaced verbs are legal for custom registrations).
  - `request` is the golden request envelope: header
    `{name: <verb>, version: "1.0", timestamp, uuid, correlation_id, reply_to}`
    (`header.name` **must equal the topic's verb**; `reply_to` set via the
  language's request path), body = the verb's arguments object (`{}` for all
  five built-ins). The requester's `identity`/`tags` are not part of the
    dispatch contract (a request may carry them; they are ignored).
  - `reply` is the golden reply envelope, published to the request's `reply_to`:
    header `{name: <verb>, version: "1.0", …, correlation_id: <the REQUEST's
    correlation_id>}` (never a `reply_to`), the **responder's** `identity`, and the
    body `{"ok": true, "result": <verb-specific>}` — `ping` →
    `{"status": "RUNNING", "uptimeSecs": n}` (the state keepalive's RUNNING body
    shape; the vector pins 42), `describe` → the descriptor-discovery
    manifest (`schemaVersion`, component identity, command capabilities,
    panel descriptor manifest, and digest), `reload-config` → `{"reloaded": true}`,
    `get-configuration` → `{"config": <redacted effective config>}` (**Flow B** —
    the same redacted snapshot the `cfg` push publishes, as a reply). Envelopes
    compare **structurally** (D-U22); a live reply may additionally carry the
    responder's `tags` (metadata, not normative).
- **errors** — the golden error reply: an unknown (but well-formed) verb with a
  `reply_to` is answered `{"ok": false, "error": {"code": "UNKNOWN_VERB",
  "message": …}}` (the `UNKNOWN_VERB` message text is library-composed and pinned;
  other codes' messages are informative, not normative).
- **behavior** — the normative dispatch rules every language implements:
  `verbIsTopicChannel` (the verb is everything after `cmd/`),
  `headerNameMustEqualVerb`, `fireAndForgetWithoutReplyTo` (no `reply_to` → the
  handler runs, no reply — unknown fire-and-forget verbs are ignored at DEBUG),
  `malformedIgnoredWithoutReply` (missing header / name≠verb / parse anomaly →
  DEBUG ignore, **never** a reply, never a crash), `builtInVerbs` (registered by
  the library; cannot be shadowed or unregistered), `delegatedVerbs`
  (`set-config` is owned by the CONFIG_COMPONENT source's own subscription — the
  inbox always ignores it), and `errorCodes` (the pinned base set: `UNKNOWN_VERB`,
  `HANDLER_ERROR` — a handler threw an uncoded error, `RELOAD_FAILED`,
  `NO_CONFIG`; custom handlers may add codes via the language's coded command
  exception). Handler failures on a fire-and-forget request are logged only.
  See `docs/platform/DESIGN-uns.md` §9.5.

## data.json / evt.json / app.json — the class-facade contracts

Pin the app-usable class publish facades — `data()` / `events()` / `app()`
(DESIGN-class-facades) — which the Python/Rust/TS ports mirror. Every case is
`{name, input, expected}`; `expected` is the LIVE Java facade's output with the
clock pinned at `2026-07-01T12:00:00Z`. Topics compare **byte-for-byte**; bodies
**structurally** (member order not normative, D-U22).

- **data.json** — `input` = `{signalId, signalPath?, signalName?, signalAddress?,
  device?, samples[], override?}`; `expected` = `{topic, route, body}` (plus
  `partitionKey` for a `stream:` route) or `{throws: true}`. The facade constructs
  the `SouthboundSignalUpdate` body and applies the defaulting rules: **`quality`
  omitted → `GOOD`** with **`qualityRaw` → `"unspecified"`** (marking the synthesis);
  a caller-supplied `quality` passes through (and its `qualityRaw` verbatim, else
  absent); **`serverTs` omitted → now**; `sourceTs` is **never** synthesized (absent
  when the source has none); the value-shorthand wraps the single value into a
  one-element `samples` array. The only hard reject (`throws: true`) is a missing/empty
  `signalId`, an empty `samples`, or a sample with no `value`. The channel is the
  sanitized `signalPath` (defaults to `signalId`); each `/`-token passes the config
  template sanitizer, so `data/a+b` → `data/a_b`. `route` is `local` (default) /
  `northbound` / `stream:<name>` from the per-call `override` (config `publish.channel`
  default resolution is Java-unit-tested, not pinned here).
- **evt.json** — `input` = `{kind: emit|raise|clear, severity?, type, message?,
  context?, override?}`; `expected` = `{topic, route, body}`. The channel
  `evt/{severity}/{type}` is **derived from the body's own severity + type** (so topic
  and body can never disagree); the four severity wire tokens are `critical|warning|
  info|debug`; `timestamp` defaults to now; `emit` with no severity defaults to `info`;
  `raiseAlarm`/`clearAlarm` default to `critical` and add `alarm` + `active`
  (`true`/`false`). The `type` is sanitized for the channel token but rides the body
  verbatim.
- **app.json** — `input` = `{name, channel, body, override?}`; `expected` =
  `{topic, route, body}`. The body is passed through **verbatim**, the header `name`
  is the caller's name, and the topic is `app/{channel}` with each `/`-token sanitized.

Generated by the Java canonical generator test (D-U12):
`mvn -f libs/java/pom.xml test -Dtest=UnsTestVectorsGeneratorTest`.
Do not hand-edit; regenerate by deleting the files and re-running the generator test.
