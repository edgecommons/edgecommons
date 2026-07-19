# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 **protocol-adapter** component (`<<COMPONENTFULLNAME>>`) written in Java on
top of the `edgecommons` Java library. A protocol adapter is a **southbound bridge**: it talks to field
devices/servers over some protocol (OPC UA, Modbus, EtherNet/IP, …) and republishes their values
northbound on the EdgeCommons messaging bus using the standard **southbound contract**
(see `docs/SOUTHBOUND.md` in the edgecommons monorepo).

The library gives you the standard CLI contract, configuration, logging, messaging, metrics, heartbeat,
and graceful lifecycle. You write the **protocol code** behind one seam — `Device.java` — and everything
above it (the connection lifecycle, backoff, publishing, health, the command surface) is already wired.

## What the scaffold already does

- **Runs with no hardware.** `Device.java` carries a `DeviceSession` / `DeviceBackend` seam and an
  in-process **simulated backend** (`SimBackend`), so the component connects, reads, publishes, and
  answers commands out of the box. Replace `SimBackend` with your protocol; nothing above the seam
  changes.
- Constructs the runtime via `EdgeCommonsBuilder`, reads config, and starts **one worker per configured
  device instance** (`component.instances[].id`). Each worker owns its device session, connects with
  jittered exponential backoff, polls on the configured interval, and reconnects on a dropped link.
- Publishes each reading with the standard **`SouthboundSignalUpdate`** envelope **via the `data()`
  facade** — `gg.instance(id).data().signal(id).device(...).addSample(value, quality).publish()`. The
  facade builds the body, mints the `ecv1/{device}/{component}/{instance}/data/{signalPath}` topic, and
  stamps identity — you never hand-build the body or the topic. Every sample carries a **normalized
  `quality`** (`GOOD|BAD|UNCERTAIN`) plus the native `qualityRaw`; a failed read is published as `BAD`,
  never dropped.
- Defines and emits **`southbound_health`** — the exact SOUTHBOUND.md §5 set (`connectionState`,
  `publishLatencyMs`, `pollLatencyMs`, `readErrors`, `staleSignals`, `reconnects`), dimensioned by
  `instance` — plus two worked **operational-metric families** (`<<COMPONENTNAME>>Connection`,
  `<<COMPONENTNAME>>Command`) showing the total/interval counter-pair pattern. `Metrics.java` marks where
  to add your protocol's own `Inventory`/`Poll`/`Publish` families.
- Serves the generic **southbound command family** on `gg.getCommands()` and registers three
  **edge-console panels** (`overview`, `signals`, `diagnostics`). See below.
- Reports **per-instance connectivity** through one provider, served on two surfaces (the `state`
  keepalive's `instances[]` and the built-in `status` verb) so a console that watches and one that asks
  can never disagree.
- Heartbeat is **automatic** (the library publishes the `state` keepalive), and shutdown rides the
  library's SIGTERM/SIGINT hook (no manual hook; `main()` blocks on a latch).

## The command surface (`sb/*`)

Registered in `Commands.java`, routed through the worker's `DeviceControl` seam so the command layer
never races the poll loop on the same connection. Instance routing follows D-EIP-13: `body.instance` is
optional when exactly one device is configured, otherwise required.

| Verb | What it does |
|------|--------------|
| `sb/status` | Per-instance link state / paused / endpoint + a counter snapshot. |
| `sb/read` | On-demand read of named signals (`{signals:[{signalId|id|name}]}`). |
| `sb/write` | Batch write (`{writes:[{signalId,value}]}`); the **allow-list is checked before any device I/O**; per-entry confirmation. |
| `sb/signals` | The configured signal inventory (no device round-trip). |
| `sb/browse` | Paged address-space discovery; the default is `BROWSE_UNSUPPORTED` for protocols with no discovery. |
| `sb/pause` / `sb/resume` | Idempotent pause/resume of telemetry production. |
| `reconnect` | Drop the session and re-establish it (one confirmed attempt). |
| `repoll` | Trigger an immediate poll (refused while paused). |

Errors use the standardized codes: `BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`, `WRITE_FAILED`,
`DEVICE_UNAVAILABLE`, `READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`, `BROWSE_FAILED`.

## What you fill in

1. **Add your protocol SDK** to `pom.xml` (see the `TODO(adapter)` comment — e.g.
   `org.eclipse.milo:milo-sdk-client:1.1.4` for OPC UA).
2. In `Device.java`, replace `SimBackend`/`SimSession` with your protocol: implement
   `DeviceBackend.connect(...)`, `DeviceSession.readSignals()` (and `writeSignal`/`browse` where the
   protocol supports them), and map your native status codes to `GOOD|BAD|UNCERTAIN` (+ `qualityRaw`).
   Nothing in `App`, `Commands`, or `Metrics` needs to change.
3. Add your protocol's metric families in `Metrics.java` where the header points, and your device's real
   keys to `config.schema.json`.

## Config convention (southbound)

The **UNS identity** is declared at the top level (`hierarchy` + `identity`); adapter config lives under
`component.global` / `component.instances[]`, validated by `config.schema.json`. See
`test-configs/<<COMPONENTNAME>>.json` for the full example. Shape:

```jsonc
"hierarchy": { "levels": ["site", "device"] },   // last level = the resolved thing name
"identity":  { "site": "site1" },                // a value for every level above the last
"component": {
  "global":    { "defaults": { "pollIntervalMs": 5000 },
                 "healthThresholds": { "staleSignalSecs": 30 } },
  "instances": [ {
    "id": "device-1", "adapter": "sim",          // id = the UNS instance token in data topics
    "connection":    { "endpoint": "sim://device-1" },   // deliberately open: add your protocol's keys
    "pollIntervalMs": 5000,
    "writes":        { "allow": [] }             // signal ids this adapter may write ([] = read-only)
  } ]
}
```

Writes are **allow-listed by stable `signal.id`**. An empty list is read-only — the correct default for
anything touching a control system — and `sb/write` refuses any signal not on the list before it ever
reaches the device.

## Run locally (HOST platform, MQTT transport)

```bash
mvn clean package
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`ecv1/+/+/state` for the heartbeat keepalives and `ecv1/+/+/+/data/#` for the per-device signal updates
(one instance per device). Send commands on `ecv1/{device}/{component}/{instance}/cmd/sb/status` etc.

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
| `src/main/java/.../<<COMPONENTNAME>>.java` | The runtime: wiring, the per-device worker, connectivity, backoff. |
| `src/main/java/.../Device.java` | The `DeviceSession`/`DeviceBackend` seam + the simulated backend — **replace `SimBackend`**. |
| `src/main/java/.../Commands.java` | The `sb/*` command family + the three edge-console panels. |
| `src/main/java/.../Metrics.java` | `southbound_health` + the operational-metric families. |
| `pom.xml` | Maven build (shaded JAR); add your protocol SDK here. |
| `config.schema.json` | The `component.global`/`instances[]` config this adapter understands. |
| `test-configs/` | Sample southbound config (`<<COMPONENTNAME>>.json`) + a HOST messaging config. |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
