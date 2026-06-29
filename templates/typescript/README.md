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
(filled in at generation time). When the library is published to an npm registry,
replace that path dependency with the corresponding registry version.
