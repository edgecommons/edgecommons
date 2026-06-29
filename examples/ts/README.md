# TypeScript Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in TypeScript on top of the
[`ggcommons`](../../libs/ts) TypeScript library. It is the TypeScript counterpart of
the Java, Python, and Rust skeletons, demonstrating the library's standard CLI
contract, configuration, logging, messaging (publish + request/reply), metrics, and
heartbeat — so a component author writes only business logic.

## What it does

- **Request/reply** — subscribes to `<thing>/skeleton/request` and replies to each request.
- **Periodic publish** — publishes `<thing>/skeleton/data` every
  `component.global.publish_interval` seconds, emitting a `messages_published` metric per send.
- **IoT Core telemetry** — mirrors each data message to AWS IoT Core (`<thing>/skeleton/telemetry`)
  and acks IoT Core commands received on `<thing>/skeleton/cmd`.
- **Dynamic config** — a config-change listener updates the publish interval live on a hot-reload.
- **Heartbeat** — periodic CPU/memory system metrics (configured in `recipe.yaml` / `config.json`).
- **Graceful shutdown** — runs until SIGINT / SIGTERM, unsubscribes, and awaits `gg.close()`.

> The messaging features above work on both the **HOST** platform (MQTT transport,
> against an MQTT broker) and the **GREENGRASS** platform (IPC transport, Greengrass IPC).

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.ts` | Entry point: builds the `ggcommons` runtime from CLI args, runs the app, shuts down. |
| `src/app.ts` | The component logic (request/reply, periodic publish, config listener, metrics). |
| `package.json` | Node manifest. Depends on the `ggcommons` library via a `file:` path dependency. |
| `tsconfig.json` | TypeScript compiler config (emits `dist/`). |
| `recipe.yaml` | Greengrass component recipe (default config + IPC access control). |
| `gdk-config.json` | Greengrass Development Kit config (`build_system: custom` → `build.sh`). |
| `build.sh` | Installs deps, compiles, and stages a ZIP artifact (`dist/` + `node_modules/`) for the GDK. |
| `test-configs/` | Sample `config.json` + `standalone-messaging.json` for local runs. |

## Develop & run locally (HOST platform, MQTT transport)

Install dependencies and build, then start a local MQTT broker (see the workspace
`CLAUDE.md`) and run:

```bash
npm install
npm run build
node dist/main.js \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

Subscribe to `my-thing/skeleton/data` in an MQTT client to see published messages,
and publish to `my-thing/skeleton/request` (with a `reply_to` header) to exercise
request/reply.

## CLI contract

Same as the Java/Python/Rust skeletons:

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Build & publish with the GDK

This component uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command`: `bash build.sh`). `build.sh` runs `npm install` + `npm run
build` (tsc) and stages a ZIP artifact (containing `dist/`, `node_modules/`, and
`package.json`) plus the recipe into `greengrass-build/` per the GDK contract.

```bash
gdk component build
gdk component publish
```

The recipe declares a `linux` platform and runs the prebuilt JS on the GREENGRASS platform
(`node {artifacts:decompressedPath}/ts-component-skeleton/dist/main.js --platform GREENGRASS -c GG_CONFIG`),
reading its configuration from the deployment (`GG_CONFIG`).

## The ggcommons dependency

`package.json` depends on the `ggcommons` library via a `file:` path dependency
(`file:../../libs/ts`). When the library is published to an npm registry, replace
that path dependency with the corresponding registry version.
