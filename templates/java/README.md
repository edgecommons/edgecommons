# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Java on top of the
`edgecommons` Java library, generated from the EdgeCommons Java component template by the `edgecommons`
CLI. It gives you the library's standard CLI contract, configuration, logging, messaging, metrics,
and heartbeat — so you write only business logic in your component class
(`src/main/java/.../<<COMPONENTNAME>>.java`).

## Run locally (HOST platform, MQTT transport)

```bash
mvn clean package
# Provide an MQTT messaging-config JSON (messaging.local required, messaging.iotCore optional):
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./standalone-messaging.json -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`ecv1/+/+/+/state` to see the component's heartbeat keepalives (the library publishes them
automatically to `ecv1/{device}/{component}/main/state`) and `ecv1/+/+/+/app/#` for the scaffold's
status messages. If you enable the telemetry-streaming subsystem, add
`--enable-native-access=ALL-UNNAMED` (the FFM/Panama binding to `edgestreamlog`).

### The demonstrated monitoring + command surface

Beyond the fully-automatic `state` keepalive and command inbox (`ping` / `reload-config` /
`get-configuration`, live with zero code), `<<COMPONENTNAME>>.java` demonstrates the rest of the
surface an edge-console reads/drives (DESIGN-uns §7/§9), through the **app-usable class
facades** (`docs/platform/DESIGN-class-facades.md`) rather than hand-built topics/bodies:

| Surface | Where | Topic |
|---|---|---|
| Metric (`loopTicks`: `tickCount` counter + `uptimeSecs` gauge) | `gg.getMetrics()` | `ecv1/{device}/{component}/main/metric/loopTicks` (target-dependent; `messaging` target shown) |
| Data signal (`demo-signal`: a sine-wave reading) | `gg.getData().signal("demo-signal").addSample(value).publish()` | `ecv1/{device}/{component}/main/data/demo-signal` |
| Event (`sample-event`, severity + context) | `gg.getEvents().emit(Severity.INFO, "sample-event", message, context)` | `ecv1/{device}/{component}/main/evt/info/sample-event` |
| Custom command verb (`set-greeting`) | `gg.getCommands().register("set-greeting", ...)` | `ecv1/{device}/{component}/main/cmd/set-greeting` |

Subscribe `ecv1/+/+/+/metric/#`, `ecv1/+/+/+/data/#` and `ecv1/+/+/+/evt/#` to see them (metrics
only publish over MQTT when `metricEmission.target` is `messaging`; the default `log` target
writes a local file instead — see `test-configs/<<COMPONENTNAME>>.json`). The `DataFacade` defaults
an omitted sample `quality` to `GOOD` (marked `qualityRaw:"unspecified"` on the wire) — pass an
explicit `Quality` when your source knows a read failed or is stale. The `EventsFacade` derives
the `evt/{severity}/{type}` channel from the body's own severity + type, so the topic and body can
never disagree; use `raiseAlarm`/`clearAlarm` for stateful alarms instead of one-shot `emit`.
Invoke the custom verb with a request/reply tool (e.g. MQTTX) by publishing
`{"header":{"name":"set-greeting","version":"1.0"},"body":{"greeting":"Hi there"}}` to
`ecv1/{device}/{component}/main/cmd/set-greeting`; the next `app` status publish reflects the new
greeting. Replace all four with your own metrics/signals/events/verbs.

### Building against the unreleased library (local-dev only)

This template pins `com.mbreissi.edgecommons:edgecommons` by the `edgecommons.version` Maven property (default:
the CLI's published-pin constant). Until a version is actually released, resolve it from your own
`~/.m2` instead of GitHub Packages:

```bash
# Once, from the edgecommons monorepo checkout (installs whatever version its pom.xml declares):
cd ../core/libs/java && mvn install -DskipTests

# Then build THIS component against that local version:
mvn compile -Dedgecommons.version=<the version mvn install just printed>
```

See the `edgecommons.version` property comment in `pom.xml` for details; the CLI's release-time pin
bump (`_EDGECOMMONS_VERSION` in `cli/edgecommons_cli/commands/create_component.py`) is a separate,
later step.

The component's **UNS identity** is config-driven: the top-level `hierarchy` block declares the
ordered levels (the last is always the resolved thing name, from `-t/--thing` or the platform) and
`identity` supplies a value for every level above it — see `test-configs/<<COMPONENTNAME>>.json`.
Topics are minted via `gg.getUns()`; envelopes built with `.withConfig(...)` carry the identity
automatically.

## Run under Greengrass

On the GREENGRASS platform the component reads its config from the deployment:

```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

## Deploy to Greengrass

Built with **Maven** (a shaded, self-contained JAR) and packaged with the **GDK (Greengrass
Development Kit)** using `gdk-config.json` and `recipe.yaml`:

```bash
mvn clean package
gdk component build
gdk component publish
```

## Deploy to Kubernetes

> The `Dockerfile` and `k8s/` manifests are emitted only when **KUBERNETES** is a selected target
> platform (`--platforms KUBERNETES`).

On the KUBERNETES platform the library auto-detects the environment from the ServiceAccount token,
defaults the config source to `CONFIGMAP` (the mounted `k8s/configmap.yaml`), the transport to
`MQTT` (broker config from that same ConfigMap), and identity from the Downward API — so the
container runs with **no args**.

```bash
# 1. Build the image (multi-stage; needs the published com.mbreissi.edgecommons:edgecommons artifact resolvable).
docker build -t <<COMPONENTNAME>>:latest .

# 2a. Push it to a registry...
docker tag <<COMPONENTNAME>>:latest ghcr.io/<owner>/<<COMPONENTNAME>>:latest
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
# 2b. ...or load it into a local kind cluster instead of pushing:
kind load docker-image <<COMPONENTNAME>>:latest

# 3. Set `image:` in k8s/deployment.yaml to the image you built/pushed, then apply:
kubectl apply -f k8s/
```

Edit `k8s/configmap.yaml` and re-`kubectl apply -f k8s/` to hot-reload the component config in place
(the ConfigMap is mounted as a whole directory so the kubelet `..data` swap is picked up — never
mount it with a `subPath`).

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`).
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS).
- `-t/--thing <name>` — IoT Thing name.

## Layout

| Path | What it is |
|------|-----------|
| `src/main/java/.../<<COMPONENTNAME>>.java` | Your business logic. |
| `pom.xml` | Maven build (shaded JAR for Greengrass deployment). |
| `test-configs/` | Sample component-config files (`<<COMPONENTNAME>>.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
