# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in TypeScript on
top of the `ggcommons` TypeScript library, generated from the GGCommons TypeScript
component template by the `ggcommons` CLI. It gives you the
library's standard CLI contract, configuration, logging, messaging, metrics, and
heartbeat — so you write only business logic in [`src/app.ts`](src/app.ts).

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.ts` | Entry point: builds the `ggcommons` runtime from CLI args, runs the app. |
| `src/app.ts` | Your component logic (starts as a minimal app + config-change listener). |
| `package.json` | Node manifest. Depends on the `ggcommons` library via a `file:` path dependency. |
| `tsconfig.json` | TypeScript compiler config (emits `dist/`). |
| `recipe.yaml` | Greengrass component recipe (default config + IPC access control). |
| `gdk-config.json` | Greengrass Development Kit config (`build_system: custom` → `build.sh`). |
| `build.sh` | Installs deps, compiles, and stages a ZIP artifact (`dist/` + `node_modules/`) for the GDK. |
| `test-configs/` | Sample `config.json` + `standalone-messaging.json` for local runs. |

## Develop & run locally (HOST platform, MQTT transport)

Local development runs on the HOST platform with the MQTT transport (dual-broker MQTT) — no
Greengrass core needed. Install dependencies, build, start a local MQTT broker, then:

```bash
npm install
npm run build
node dist/main.js \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

## Topics & identity (UNS)

Components address the bus through the **unified namespace**: every topic is minted by the
identity-bound UNS builder — `gg.uns().topic(UnsClass.Data, "my-channel")` →
`ecv1/{device}/{component}/{instance}/data/my-channel` — never hand-written strings. The
identity comes from the config's top-level `hierarchy` + `identity` blocks (see
`test-configs/config.json`): the **last** hierarchy level's value is always the resolved
thing name (the *device*); every other level's value comes from `identity`. Omitting
`hierarchy` gives the zero-config default `["device"]`. Instance-scoped topics and messages
come from `gg.instance(id).uns()` / `.newMessage(...)`.

The library also publishes an automatic **heartbeat**: a `state` keepalive on
`ecv1/{device}/{component}/main/state` every `heartbeat.intervalSecs` (default 5 s; subscribe
to `ecv1/+/+/+/state` to watch every component's keepalive), with the enabled system measures emitted as the
`sys` metric. The classes `state | metric | cfg | log` are library-owned (reserved) — a direct
publish to them is rejected.

## The demonstrated monitoring + command surface

Beyond the fully-automatic `state` keepalive and command inbox (`ping` / `reload-config` /
`get-configuration`, live with zero code), `src/app.ts` demonstrates the rest of the surface an
edge-console reads/drives (DESIGN-uns §7/§9):

| Surface | Where | Topic |
|---|---|---|
| Metric (`loopTicks`: `tickCount` counter + `uptimeSecs` gauge) | `gg.metrics()` | `ecv1/{device}/{component}/main/metric/loopTicks` (target-dependent; `messaging` target shown) |
| Event (`sample-event`, severity + context) | `gg.uns().topic(UnsClass.Evt, "sample-event")` + `IMessagingService.publish` | `ecv1/{device}/{component}/main/evt/sample-event` |
| Custom command verb (`set-greeting`) | `gg.commands().register("set-greeting", ...)` | `ecv1/{device}/{component}/main/cmd/set-greeting` |

Subscribe `ecv1/+/+/+/metric/#` and `ecv1/+/+/+/evt/#` to see them (metrics only publish over MQTT
when `metricEmission.target` is `messaging`; the default `log` target writes a local file
instead). Invoke the custom verb with a request/reply tool (e.g. MQTTX) by publishing
`{"header":{"name":"set-greeting","version":"1.0"},"body":{"greeting":"Hi there"}}` to
`ecv1/{device}/{component}/main/cmd/set-greeting`; the next `app` status publish reflects the new
greeting. Replace all three with your own metrics/events/verbs.

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Deploy to Greengrass

Packaged with the **GDK (Greengrass Development Kit)** using `gdk-config.json` and `recipe.yaml`.
The on-device build uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command: bash build.sh`). `build.sh` runs `npm install` + `npm run build` (tsc) and
stages a ZIP artifact (`dist/` + `node_modules/` + `package.json`) per the GDK contract.

```bash
gdk component build
gdk component publish
```

The recipe declares a `linux` platform and runs the prebuilt JS on the GREENGRASS platform
(`node {artifacts:decompressedPath}/<<COMPONENTNAME>>/dist/main.js --platform GREENGRASS -c GG_CONFIG`),
reading its configuration from the deployment (`GG_CONFIG`).

## Deploy to Kubernetes

The Kubernetes artifacts (`Dockerfile`, `k8s/`) exist only when this component was scaffolded
with **KUBERNETES** as a target platform. Build the image from `./Dockerfile`, make it available
to the cluster, point `image:` at it, then apply the manifests:

```bash
# 1. Build the image (npm ci resolves the published @edgecommons/ggcommons from GitHub Packages —
#    needs an .npmrc with the registry + a GITHUB_TOKEN at build time).
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

## The ggcommons dependency

`package.json` depends on the `ggcommons` library via a `file:` path dependency
(filled in at generation time, `--dep-source local`, the default). This IS the local-dev
override already: npm resolves the `file:` reference straight from the sibling checkout's
current build output (even an unpublished branch like `feat/unified-namespace`), so unlike an
already-scaffolded component with a published registry pin (see the edge-console's gitignored
`local/ggcommons` workspace stub, generated by `scripts/link-sibling-lib.mjs`, which redirects
its committed `^0.1.1` dependency to the sibling), a freshly generated component needs no extra
override step — just make sure the sibling is built first (`npm run build` in `ggcommons/libs/ts`
before `npm install` here, since a `file:` dependency on a TS package needs its `dist/` present).

Regenerate with `--dep-source registry` once the library publishes a real npm version to switch
to a registry dependency; that pin is a release-time item, not something this template can do
today.
