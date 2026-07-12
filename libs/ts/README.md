# edgecommons (TypeScript)

A TypeScript implementation of the Greengrass Commons library — a 4th implementation
alongside Java (canonical), Python, and Rust. It bundles the cross-cutting concerns
of an AWS IoT Greengrass v2 component (configuration, messaging, metrics, heartbeat,
logging, credentials, parameters, telemetry streaming) behind service interfaces so
component authors write only business logic.

It is at **feature parity** with the other libraries: the same config schema, the
same CLI contract, the same subsystem boundaries, the same on-wire message envelope.

## Subsystems

| Area | Module | Notes |
|------|--------|-------|
| Lifecycle | `src/edgecommons.ts` | `EdgeCommonsBuilder` / `EdgeCommons` — parse args, init messaging, load+validate config, init logging/metrics/heartbeat, start the health endpoint, wire hot-reload + SIGTERM/SIGINT. `setReady(bool)` gates `/readyz`; `close()` releases resources (TS has no RAII). |
| CLI contract | `src/cli.ts` | `-c/--config` (FILE/ENV/GG_CONFIG/SHADOW/CONFIG_COMPONENT), `--platform` (GREENGRASS/HOST/KUBERNETES/auto), `--transport` (IPC/MQTT), `-t/--thing`. |
| Config | `src/config/` | Typed model + defaulting accessors, template substitution (sanitized), embedded JSON schema + `ajv` validation, all 5 sources, hot reload. |
| Messaging | `src/messaging/` | Transport/service split: `MessagingProvider` (`StandaloneMqttProvider` dual-broker, `IpcMessagingProvider` Greengrass IPC) + `DefaultMessagingService` (envelope, dispatch, request/reply, strict confirmed publishing). |
| Metrics | `src/metrics/` | `Metric`/`MetricBuilder`, EMF (ms timestamps), targets (log w/ rotation, messaging, cloudwatchcomponent, cloudwatch via optional `@aws-sdk/client-cloudwatch`, **prometheus** pull-based registry served over HTTP via optional `prom-client` — default on KUBERNETES), `MetricEmitter`. |
| Heartbeat | `src/heartbeat.ts` | The UNS `state` keepalive (`ecv1/{device}/{component}/main/state`, `{"status":"RUNNING","uptimeSecs":n}`, best-effort `STOPPED` on shutdown) + the enabled cpu/mem/disk/… measures emitted as the `sys` metric through the metric subsystem; on/5 s/local by default; reacts to config reload. |
| UNS | `src/uns.ts` | `gg.uns()` / `gg.instance(id).uns()` — the unified-namespace topic builder + validator (`ecv1[/{site}]/{device}/{component}/{instance}/{class}[/{channel…}]`), `UnsScope` filters, machine-readable `UnsValidationError` codes, and the reserved-class predicate behind the publish guard (`state|metric|cfg|log` are library-owned). |
| Health | `src/health.ts` | Dependency-free HTTP `GET /livez` (process alive; never checks the broker), `/readyz` + `/startupz` (200 only when messaging-connected, `setReady`, required library gates, and not shutting down); on by default on KUBERNETES, opt-in via `health.enabled` elsewhere. |
| Logging | `src/logging.ts`, `src/log_bus.ts` | Leveled logger with file rotation; reconfigures on reload; per-logger levels via `getLogger(name)`. Optional `logging.publish` sends structured records through `gg.logs()` to the reserved UNS `log` class. |
| Message | `src/message.ts` | The cross-language `Message` envelope + `MessageBuilder`. |
| Credentials | `src/credentials/` | `gg.credentials()` — encrypted local vault + key providers (File/KMS/SecretsManager); opt-in (undefined unless a `credentials` config section is present). |
| Parameters | `src/parameters/` | `gg.parameters()` — offline-first externalized config (env / mountedDir / AWS SSM); opt-in. |
| Streaming | `src/streaming/` | `gg.streams()` — telemetry streaming to Kinesis/Kafka via the shared `edgestreamlog` core (napi-rs native binding); opt-in. |

## Quick start

```ts
import { EdgeCommonsBuilder, MetricBuilder, MessageBuilder, Qos } from "edgecommons";

const gg = await new EdgeCommonsBuilder("com.example.MyComponent")
  .args(process.argv.slice(2))
  .build();

const cfg = gg.config();
gg.metrics().defineMetric(MetricBuilder.create("ticks").addMeasure("count", "Count", 60).build());

const svc = gg.messaging(); // throws if unavailable
await svc.subscribe(`${cfg.thingName}/cmd`, (topic, msg) => {
  // handle msg.getBody()
}, 16, 1);

await gg.metrics().emitMetric("ticks", { count: 1 });

// Optional structured log bus publishing (disabled by default via logging.publish.enabled=false):
await gg.logs().publish({ level: "INFO", logger: "app", message: "started" });

// Opt-in subsystems — undefined unless their config section is present:
const creds = gg.credentials();   // CredentialService | undefined
const params = gg.parameters();   // ParameterService | undefined
const streams = gg.streams();     // StreamService | undefined

// on shutdown:
await gg.close();
```

## Confirmed publishing and prepared app messages

`DefaultMessagingService.publishConfirmed(topic, messageOrBytes, Qos.AtLeastOnce,
timeoutMs)` is the strict publication boundary. It resolves only after the MQTT QoS-1
PUBACK or successful completion of the Greengrass IPC publish operation. A disconnect,
operation failure, timeout, or the bounded 1024-operation capacity rejects with
`PublishConfirmationError`; there is no fallback to best-effort publishing. Exact encoded
bytes are parsed and validated as a complete EdgeCommons envelope before they are sent, and
the same bytes are passed to the transport unchanged.

`AppFacade.prepare(...)` and `prepareCorrelated(requestOrCorrelationId, ...)` build a
`PreparedAppMessage` containing its final topic, envelope, and encoded bytes. Use
`publishConfirmed(prepared, timeoutMs, routing)` when persistence or command acceptance
depends on a transport-confirmed publication. A prepared message is immutable from the
caller's perspective: byte and message accessors return defensive copies.

## Explicit command outcomes and deferred replies

`CommandInbox.registerOutcome(verb, handler)` supports three tagged outcomes from
`CommandOutcomes`: immediate success, immediate coded error, or a deferred reply. Deferred
work follows a two-step durability boundary:

1. Call `inbox.defer(request, lifetimeMs)` before committing the job. This creates an opaque
   provisional token and validates the guarded reply target.
2. After the durable insert commits, call `token.activate()` and return
   `CommandOutcomes.deferred(token)`. If the commit fails, call `token.discard()`.

When durable acceptance must immediately hand work to an async operation, return
`CommandOutcomes.deferredWithContinuation(token, async () => { ... })` instead. The inbox first
validates the exact token in `OPEN` state and only then starts its bounded continuation (maximum
256 in flight). The closure captures and settles the opaque token; it never receives a reply topic.
The original `deferred(token)` form retains its established semantics.

An activated token can settle once with `settleSuccess` or `settleError`. The inbox performs
confirmed reply attempts with bounded retry until confirmation or expiry. The registry holds
at most 1024 entries, lifetime is capped at 1,860,000 ms, open expiry emits the stable
`DEFERRED_REPLY_EXPIRED` diagnostic, and shutdown attempts a `COMPONENT_STOPPING` reply before
cancelling remaining tokens. Deferred state retains only reply metadata and the eventual
reply, not the original request body or application job payload.

## Startup gates and candidate validation

`EdgeCommonsBuilder.initialReady(false)` holds the application readiness gate closed until the
component calls `gg.setReady(true)`. Register side-effect-free candidate checks with
`configurationValidator(name, validator)` and, when needed, set a bounded per-validator deadline
with `configurationValidationTimeout(ms)`. Validators receive the schema-valid candidate, the
redacted prior snapshot (on reload), and `INITIAL` or `RELOAD`; a rejection, timeout, or failure
keeps the precise prior snapshot in place.

Use `configureCommands(inbox => { ... })` to install required verbs before the command inbox
submits its wildcard subscription. `CommandInbox.state()` exposes `STARTING`, `ACTIVE`, `FAILED`,
and `STOPPED`; `ACTIVE` means the selected MQTT or Greengrass IPC transport acknowledged the exact
filter. Components using `configureCommands` automatically keep readiness false unless that inbox
is active, and a failed start exposes only a stable `startupError()` code.

## Build, test

```bash
npm install
npm run build      # tsc -> dist/
npm test           # vitest unit tests (cli, config, message, metrics, messaging, heartbeat)
```

## Log bus publishing

`logging.publish` enables the library-owned UNS log publisher. Records publish to
`ecv1/{device}/{component}/main/log/{level}` with envelope header
`{name:"log", version:"1.0"}` and body schema `edgecommons.log.v1`. The `log`
class remains reserved: raw public `messaging.publish(...)` to `.../log/...` is
still rejected, and the publisher uses the internal reserved seam.

`captureNative` captures EdgeCommons `Logger` records. `captureConsole` requests
console capture and, in this SDK, patches `console.*` when enabled; it is disabled
by default. TypeScript cannot universally capture arbitrary third-party logging
libraries unless those libraries write through EdgeCommons `Logger` or `console.*`.

## Runtime model — platform × transport

A component is described by two orthogonal axes: `--platform` (`GREENGRASS | HOST |
KUBERNETES | auto`, default `auto`, which auto-detects from the environment) and
`--transport` (`IPC | MQTT`, default derived from the platform). The legacy single
`-m/--mode` axis has been removed.

- **`--platform HOST`** (transport defaults to `MQTT`) — dual-broker MQTT over
  [`mqtt.js`](https://github.com/mqttjs/MQTT.js): a local broker plus an optional generic
  northbound MQTT leg. Needs a `--transport MQTT <messaging_config.json>` file
  (`messaging.local` required, `messaging.northbound` optional). No native build.
- **`--platform GREENGRASS`** (transport defaults to `IPC`) — Greengrass IPC over
  `aws-iot-device-sdk-v2`'s `greengrasscoreipc` client (the **V1** IPC surface —
  `subscribeToTopic(...).on('message', …)` + `.activate()`; the simplified clientV2 is
  Java/Python-only). Local pub/sub + the IoT Core bridge + config (`GG_CONFIG`) + device
  shadow (`SHADOW`). Requires a running nucleus: a deployed component supplies the IPC
  env (`SVCUID`, the domain-socket path) and the recipe must grant
  `aws.greengrass.ipc.pubsub` (and, for the bridge/shadow,
  `aws.greengrass.ipc.mqttproxy` / `aws.greengrass.ShadowManager`) `accessControl`.
  `IPC` is valid only on `--platform GREENGRASS`.
- **`--platform KUBERNETES`** — declared but not wired until Phase 1.

## Interoperability — validated

- **Cross-language wire (HOST/MQTT):** joins the shared suite in
  `test-infra/interop/` as the `ts` node. The full matrix is 4×4×2 = **32 combos,
  all passing** (request/reply + raw publish/ingest, every ordered pair across
  python/java/rust/ts, both directions).
- **GREENGRASS IPC, on a live nucleus:** `IpcProvider` deployed as a component on a
  real AWS IoT Greengrass v2 nucleus (`deploy/`, `src/ipc_verify.ts`) confirmed
  connect + request/reply + raw over IPC, **plus cross-language Java→TS** (decoding
  the heartbeat envelope the already-deployed Java edgecommons component publishes over
  IPC). See `deploy/README.md` to reproduce.

## Cross-language parity

Maintained intentionally with the Java/Python/Rust libraries: identical config
schema, CLI flags, subsystem boundaries, message envelope (snake_case header keys,
the top-level UNS `identity` element, `{raw}` for non-envelope payloads), EMF
layout, heartbeat stats shape, and byte-identical UNS topics (pinned by the shared
`uns-test-vectors/` conformance suite). Change public behavior here only alongside
the matching change in the mirrors.
