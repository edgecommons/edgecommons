# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Python on top of the
`edgecommons` (`edgecommons`) Python library, generated from the EdgeCommons Python component
template by the `edgecommons` CLI. It gives you the library's standard CLI contract, configuration,
logging, messaging, metrics, and heartbeat — so you write only business logic in
[`app/<<COMPONENTNAME>>.py`](app/<<COMPONENTNAME>>.py).

## Run locally (HOST platform, MQTT transport)

```bash
pip install -r requirements.txt
# Provide an MQTT messaging-config JSON (messaging.local required, messaging.northbound optional):
python3 main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json -c FILE test-configs/config_2.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`ecv1/+/+/+/state` to see the component's heartbeats. Topics follow the unified namespace
(`ecv1/{device}/{component}/{instance}/{class}/...`): the component's place in it comes from the
top-level `hierarchy` + `identity` config blocks (see `test-configs/config_1.json`; the last
hierarchy level is always the resolved thing name), and application topics are minted in code via
`gg.uns()` — never hand-written.

### The demonstrated monitoring + command surface

Beyond the fully-automatic `state` keepalive and command inbox (`ping` / `reload-config` /
`get-configuration`, live with zero code), `app/<<COMPONENTNAME>>.py` demonstrates the rest of
the surface an edge-console reads/drives (DESIGN-uns §7/§9), through the **app-usable class
facades** (`docs/platform/DESIGN-class-facades.md`) rather than hand-built topics/bodies:

| Surface | Where | Topic |
|---|---|---|
| Metric (`loopTicks`: `tickCount` counter + `uptimeSecs` gauge) | `gg.get_metrics()` | `ecv1/{device}/{component}/main/metric/loopTicks` (target-dependent; `messaging` target shown) |
| Data signal (`demo-signal`: a sine-wave reading) | `gg.data().publish("demo-signal", value)` | `ecv1/{device}/{component}/main/data/demo-signal` |
| Event (`sample-event`, severity + context) | `gg.events().emit("sample-event", message, context, severity=Severity.INFO)` | `ecv1/{device}/{component}/main/evt/info/sample-event` |
| Custom command verb (`set-greeting`) | `EdgeCommonsBuilder.configure_commands(...)` | `ecv1/{device}/{component}/main/cmd/set-greeting` |

Subscribe `ecv1/+/+/+/metric/#`, `ecv1/+/+/+/data/#` and `ecv1/+/+/+/evt/#` to see them (metrics
only publish over MQTT when `metricEmission.target` is `messaging`; the default `log` target
writes a local file instead). `DataFacade` defaults an omitted sample `quality` to `GOOD` (marked
`qualityRaw:"unspecified"` on the wire) — pass an explicit `Quality` when your source knows a read
failed or is stale. `EventsFacade` derives the `evt/{severity}/{type}` channel from the body's own
severity + type, so the topic and body can never disagree; use `raise_alarm`/`clear_alarm` for
stateful alarms instead of one-shot `emit`. Invoke the custom verb with a request/reply tool
(e.g. MQTTX) by publishing `{"header":{"name":"set-greeting","version":"1.0"},"body":
{"greeting":"Hi there"}}` to `ecv1/{device}/{component}/main/cmd/set-greeting`; the next `app`
status publish reflects the new greeting. Replace all four with your own metrics/signals/events/verbs.

The scaffold selects `initial_ready(False)` before the runtime starts. Its custom handler is installed
through `configure_commands(...)` before the inbox subscription is acknowledged, and it releases the
application readiness gate only after app construction. Readiness additionally requires messaging to be
connected and the command inbox to report `ACTIVE`.

### Building against the unreleased library (local-dev only)

`requirements.txt` names the published `edgecommons` package (a release-time placeholder —
see the comment above it). Until a version is actually released, build against the sibling
monorepo checkout instead:

```bash
pip install -r requirements.txt
bash scripts/link-sibling-lib.sh   # editable-installs ../core/libs/python over the line above
```

See `requirements.txt` and `scripts/link-sibling-lib.sh` for details.

## Run under Greengrass

On the GREENGRASS platform the component reads its config from the deployment:

```bash
python3 main.py --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

## Deploy to Greengrass

Packaged with the **GDK (Greengrass Development Kit)** using `gdk-config.json` and `recipe.yaml`:

```bash
gdk component build
gdk component publish
```

## Deploy to Kubernetes

The Kubernetes artifacts (`Dockerfile`, `k8s/`) exist only when this component was scaffolded
with **KUBERNETES** as a target platform. Build the image from `./Dockerfile`, make it available
to the cluster, point `image:` at it, then apply the manifests:

```bash
# 1. Build the image (requirements.txt resolves the published edgecommons library).
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .

# 2. Make it available to the cluster — push to a registry...
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
#    ...or, for a local kind cluster, load it directly:
# kind load docker-image ghcr.io/<owner>/<<COMPONENTNAME>>:latest

# 3. Set `image:` in k8s/deployment.yaml (replace REPLACE_ME) to that image, then:
kubectl apply -f k8s/
```

With `--platform auto` the library detects KUBERNETES from the ServiceAccount token, reads its
config from the mounted ConfigMap (`CONFIGMAP` source, hot-reloaded on `kubectl apply`), uses the
MQTT transport from that same ConfigMap, and resolves identity from the Downward API — so the
Deployment needs no command-line args.

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`).
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS).
- `-t/--thing <name>` — IoT Thing name.

## Layout

| Path | What it is |
|------|-----------|
| `main.py` | Entry point — builds `EdgeCommons` and starts the app. |
| `app/<<COMPONENTNAME>>.py` | Your business logic. |
| `tests/` | `pytest` tests for the seams the app wires into the library — its command verb and the connectivity it reports. `python -m pytest` — no broker needed. |
| `test-configs/` | Sample component-config files (`config_*.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
