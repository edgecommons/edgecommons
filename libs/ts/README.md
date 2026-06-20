# ggcommons (TypeScript)

A TypeScript implementation of the Greengrass Commons library — a 4th implementation
alongside Java (canonical), Python, and Rust. It bundles the cross-cutting concerns
of an AWS IoT Greengrass v2 component (configuration, messaging, metrics, heartbeat,
logging) behind service interfaces so component authors write only business logic.

It is at **feature parity** with the other libraries: the same config schema, the
same CLI contract, the same subsystem boundaries, the same on-wire message envelope.

## Subsystems

| Area | Module | Notes |
|------|--------|-------|
| Lifecycle | `src/ggcommons.ts` | `GGCommonsBuilder` / `GGCommons` — parse args, init messaging, load+validate config, init logging/metrics/heartbeat, wire hot-reload. `close()` releases resources (TS has no RAII). |
| CLI contract | `src/cli.ts` | `-c/--config` (FILE/ENV/GG_CONFIG/SHADOW/CONFIG_COMPONENT), `-m/--mode` (GREENGRASS/STANDALONE), `-t/--thing`. |
| Config | `src/config/` | Typed model + defaulting accessors, template substitution (sanitized), embedded JSON schema + `ajv` validation, all 5 sources, hot reload. |
| Messaging | `src/messaging/` | Transport/service split: `MessagingProvider` (`StandaloneMqttProvider` dual-broker, `IpcMessagingProvider` Greengrass IPC) + `DefaultMessagingService` (envelope, dispatch, request/reply). |
| Metrics | `src/metrics/` | `Metric`/`MetricBuilder`, EMF (ms timestamps), targets (log w/ rotation, messaging, cloudwatchcomponent, cloudwatch via optional `@aws-sdk/client-cloudwatch`), `MetricEmitter`. |
| Heartbeat | `src/heartbeat.ts` | Periodic cpu/mem/disk/threads/files/fds to metric/messaging targets; reacts to config reload. |
| Logging | `src/logging.ts` | Leveled logger with file rotation; reconfigures on reload. |
| Message | `src/message.ts` | The cross-language `Message` envelope + `MessageBuilder`. |

## Quick start

```ts
import { GGCommonsBuilder, MetricBuilder, MessageBuilder, Qos } from "ggcommons";

const gg = await new GGCommonsBuilder("com.example.MyComponent")
  .args(process.argv.slice(2))
  .build();

const cfg = gg.config();
gg.metrics().defineMetric(MetricBuilder.create("ticks").addMeasure("count", "Count", 60).build());

const svc = gg.messaging(); // throws if unavailable
await svc.subscribe(`${cfg.thingName}/cmd`, (topic, msg) => {
  // handle msg.getBody()
}, 16, 1);

await gg.metrics().emitMetric("ticks", { count: 1 });
// on shutdown:
await gg.close();
```

## Build, test

```bash
npm install
npm run build      # tsc -> dist/
npm test           # vitest unit tests (cli, config, message, metrics, messaging, heartbeat)
```

## Runtime modes

- **STANDALONE** — dual-broker MQTT over [`mqtt.js`](https://github.com/mqttjs/MQTT.js):
  a local broker plus an optional AWS IoT Core leg (mutual-TLS). Needs a
  `-m STANDALONE <messaging_config.json>` file (`messaging.local` required,
  `messaging.iotCore` optional). No native build.
- **GREENGRASS** — Greengrass IPC over `aws-iot-device-sdk-v2`'s `greengrasscoreipc`
  client (the **V1** IPC surface — `subscribeToTopic(...).on('message', …)` +
  `.activate()`; the simplified clientV2 is Java/Python-only). Local pub/sub + the
  IoT Core bridge + config (`GG_CONFIG`) + device shadow (`SHADOW`). Requires a
  running nucleus: a deployed component supplies the IPC env (`SVCUID`, the
  domain-socket path) and the recipe must grant `aws.greengrass.ipc.pubsub` (and,
  for the bridge/shadow, `aws.greengrass.ipc.mqttproxy` / `aws.greengrass.ShadowManager`)
  `accessControl`.

## Interoperability — validated

- **Cross-language wire (STANDALONE/MQTT):** joins the shared suite in
  `test-infra/interop/` as the `ts` node. The full matrix is 4×4×2 = **32 combos,
  all passing** (request/reply + raw publish/ingest, every ordered pair across
  python/java/rust/ts, both directions).
- **GREENGRASS IPC, on a live nucleus:** `IpcProvider` deployed as a component on a
  real AWS IoT Greengrass v2 nucleus (`deploy/`, `src/ipc_verify.ts`) confirmed
  connect + request/reply + raw over IPC, **plus cross-language Java→TS** (decoding
  the heartbeat envelope the already-deployed Java ggcommons component publishes over
  IPC). See `deploy/README.md` to reproduce.

## Cross-language parity

Maintained intentionally with the Java/Python/Rust libraries: identical config
schema, CLI flags, subsystem boundaries, message envelope (snake_case header keys,
`thing` tag, `{raw}` for non-envelope payloads), EMF layout, and heartbeat stats
shape. Change public behavior here only alongside the matching change in the mirrors.
