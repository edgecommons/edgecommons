# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 **protocol-adapter** component (`<<COMPONENTFULLNAME>>`) written in Java on
top of the `edgecommons` Java library. A protocol adapter is a **southbound bridge**: it talks to field
devices/servers over some protocol (OPC UA, Modbus, EtherNet/IP, …) and republishes their values
northbound on the EdgeCommons messaging bus using the standard **southbound contract**
(see `docs/SOUTHBOUND.md` in the edgecommons monorepo).

The library gives you the standard CLI contract, configuration, logging, messaging, metrics,
heartbeat, and graceful lifecycle — so you write only the **protocol code**, at the `TODO(adapter)`
markers in `src/main/java/.../<<COMPONENTNAME>>.java`.

## What the scaffold already does

- Constructs the runtime via `EdgeCommonsBuilder`, reads config, and starts **one worker per configured
  device instance** (`component.instances[].id`).
- Publishes each signal update with the standard **`SouthboundSignalUpdate`** envelope — `body.device`,
  `body.signal` (canonical `id` + opaque protocol-native `address`), and `body.samples[]` with a
  **normalized `quality`** (`GOOD|BAD|UNCERTAIN`) plus the native `qualityRaw`.
- Publishes on the **UNS data plane**: each update goes to
  `ecv1/{device}/{component}/{instanceId}/data/{signalPath}`, minted per instance via
  `gg.instance(instanceId).uns().topic(UnsClass.DATA, signalPath)` — never a hand-written topic.
  The envelope's `identity` block is stamped automatically by
  `gg.instance(instanceId).newMessage(...)` from the config-driven identity (top-level `hierarchy`
  + `identity`; the last hierarchy level is always the resolved thing name).
- Defines and emits the standard **`southbound_health`** metric (connection state, poll/publish
  latency, read errors, stale tags). The `messaging` metric target publishes to the UNS metric
  topic (`ecv1/{device}/{component}/main/metric/{metricName}`) automatically — no topic config.
- Heartbeat is **automatic**: the library publishes the `state` keepalive to
  `ecv1/{device}/{component}/main/state` (on / 5 s / local by default; optional
  `heartbeat: {enabled, intervalSecs, measures, destination}` to tune).
- Relies on the library's SIGTERM/SIGINT hook for graceful shutdown (no manual hook;
  `main()` blocks on a latch).
- Starts with `initialReady(false)` and releases the application gate only after all configured
  workers are launched. Connected messaging and an acknowledged `ACTIVE` command inbox remain
  mandatory readiness conditions.

It is **protocol-agnostic on purpose** — no protocol SDK is bundled. The placeholder worker emits a
synthetic value so the scaffold runs end-to-end; replace it with your protocol logic.

## What you fill in

1. **Add your protocol SDK** to `pom.xml` (see the `TODO(adapter)` comment there — e.g.
   `org.eclipse.milo:milo-sdk-client:1.1.4` for OPC UA).
2. In `<<COMPONENTNAME>>.java`, at the `TODO(adapter)` markers:
   - `runInstance(...)` — open the connection (with retry/backoff), then subscribe or poll per
     `instance.subscriptions[]`.
   - on each value received, call `publishUpdate(...)` with the signal identity, value, and normalized
     quality.
   - map your native status codes → `GOOD|BAD|UNCERTAIN` (+ `qualityRaw`).
   - `onConfigurationChanged()` — re-apply subscription config on hot reload.

## Config convention (southbound)

The **UNS identity** is declared at the top level (`hierarchy` + `identity`); adapter config lives
under the **permissive** `component.global` / `component.instances[]` (no schema change needed).
See `test-configs/<<COMPONENTNAME>>.json` for a full example. Shape:

```jsonc
"hierarchy": { "levels": ["site", "device"] },   // last level = the resolved thing name
"identity":  { "site": "site1" },                // a value for every level above the last
"component": {
  "global":    { "defaults": { "publishIntervalMs": 1000, "samplingRateMs": 500, "queueSize": 100 },
                 "healthThresholds": { "staleSignalSecs": 30 } },
  "instances": [ {
    "id": "device-1", "adapter": "<protocol>",   // id = the UNS instance token in data topics
    "connection":  { "endpoint": "..." },
    "publish":     { "batchMs": 1000 },          // NO topic key: data topics are minted via uns()
    "write":       { "enabled": false },         // Phase 5 (M9): reworked to UNS cmd/sb/* verbs
    "subscriptions":[ { "id": "...", "include": [ { "namespace": 0, "match": "<regex>", "deadband": {"type":"Absolute","value":0.0} } ], "exclude": [] } ]
  } ]
}
```

> **Phase 5 (M9) note:** the southbound *command* family (write/read/control toward the device)
> will arrive as UNS `cmd/sb/*` verbs on the component inbox
> (`ecv1/{device}/{component}/{instance}/cmd/sb/write` …). Keep any interim command handlers
> isolated so that retarget stays mechanical.

## Run locally (HOST platform, MQTT transport)

```bash
mvn clean package
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./standalone-messaging.json \
  -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`ecv1/+/+/+/state` for the heartbeat keepalives and `ecv1/+/+/+/data/#` to see signal updates.

## Run under Greengrass

```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

## Deploy

Built with **Maven** (a shaded, self-contained JAR), packaged with the **GDK**:

```bash
mvn clean package
gdk component build
gdk component publish
```

> The `Dockerfile` and `k8s/` manifests are emitted only when **KUBERNETES** is a selected target
> platform (`--platforms KUBERNETES`); see those files for the container/k8s flow.

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default from the platform profile).
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`).
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]` (IPC only valid on GREENGRASS).
- `-t/--thing <name>` — IoT Thing name.

## Layout

| Path | What it is |
|------|-----------|
| `src/main/java/.../<<COMPONENTNAME>>.java` | Your adapter — fill in the `TODO(adapter)` markers. |
| `pom.xml` | Maven build (shaded JAR); add your protocol SDK here. |
| `test-configs/` | Sample southbound config (`<<COMPONENTNAME>>.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
