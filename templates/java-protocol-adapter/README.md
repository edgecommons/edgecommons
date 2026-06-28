# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 **protocol-adapter** component (`<<COMPONENTFULLNAME>>`) written in Java on
top of the `ggcommons` Java library. A protocol adapter is a **southbound bridge**: it talks to field
devices/servers over some protocol (OPC UA, Modbus, EtherNet/IP, …) and republishes their values
northbound on the GGCommons messaging bus using the standard **southbound contract**
(see `docs/SOUTHBOUND.md` in the ggcommons monorepo).

The library gives you the standard CLI contract, configuration, logging, messaging, metrics,
heartbeat, and graceful lifecycle — so you write only the **protocol code**, at the `TODO(adapter)`
markers in `src/main/java/.../<<COMPONENTNAME>>.java`.

## What the scaffold already does

- Constructs the runtime via `GGCommonsBuilder`, reads config, and starts **one worker per configured
  device instance** (`component.instances[].id`).
- Publishes each tag update with the standard **`SouthboundTagUpdate`** envelope — `body.device`,
  `body.tag` (canonical `id` + opaque protocol-native `address`), and `body.samples[]` with a
  **normalized `quality`** (`GOOD|BAD|UNCERTAIN`) plus the native `qualityRaw`.
- Defines and emits the standard **`southbound_health`** metric (connection state, poll/publish
  latency, read errors, stale tags).
- Relies on the library's SIGTERM/SIGINT hook for graceful shutdown (no manual hook;
  `main()` blocks on a latch).

It is **protocol-agnostic on purpose** — no protocol SDK is bundled. The placeholder worker emits a
synthetic value so the scaffold runs end-to-end; replace it with your protocol logic.

## What you fill in

1. **Add your protocol SDK** to `pom.xml` (see the `TODO(adapter)` comment there — e.g.
   `org.eclipse.milo:milo-sdk-client:1.1.4` for OPC UA).
2. In `<<COMPONENTNAME>>.java`, at the `TODO(adapter)` markers:
   - `runInstance(...)` — open the connection (with retry/backoff), then subscribe or poll per
     `instance.subscriptions[]`.
   - on each value received, call `publishUpdate(...)` with the tag identity, value, and normalized
     quality.
   - map your native status codes → `GOOD|BAD|UNCERTAIN` (+ `qualityRaw`).
   - `onConfigurationChanged()` — re-apply subscription config on hot reload.

## Config convention (southbound)

Adapter config lives under the **permissive** `component.global` / `component.instances[]` (no schema
change needed). See `test-configs/<<COMPONENTNAME>>.json` for a full example. Shape:

```jsonc
"component": {
  "global":    { "defaults": { "publishIntervalMs": 1000, "samplingRateMs": 500, "queueSize": 100 },
                 "healthThresholds": { "staleTagSecs": 30 } },
  "instances": [ {
    "id": "device-1", "adapter": "<protocol>",
    "connection":  { "endpoint": "..." },
    "publish":     { "topic": "southbound/{site}/{ComponentName}/{InstanceId}/{tagId}", "batchMs": 1000 },
    "write":       { "enabled": false, "topic": "..." },
    "subscriptions":[ { "id": "...", "include": [ { "namespace": 0, "match": "<regex>", "deadband": {"type":"Absolute","value":0.0} } ], "exclude": [] } ]
  } ]
}
```

## Run locally (HOST platform, MQTT transport)

```bash
mvn clean package
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./standalone-messaging.json \
  -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`heartbeat/+/+` for heartbeats and `southbound/#` to see tag updates.

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
