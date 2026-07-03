# TypeScript Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in TypeScript on top of the
[`ggcommons`](../../libs/ts) TypeScript library. It is the TypeScript counterpart of
the Java, Python, and Rust skeletons, demonstrating the library's standard CLI
contract, configuration, logging, messaging (publish + request/reply), metrics, and
heartbeat — so a component author writes only business logic.

## What it does

Every topic is minted through the **UNS topic builder** (`gg.uns()`), bound to the component's
config-resolved identity — the top-level `hierarchy` + `identity` config blocks; the last
hierarchy level's value is always the resolved thing name (the *device*). With `-t my-thing`
the topics below render as `ecv1/my-thing/TsComponentSkeleton/main/…`.

- **Request/reply** — subscribes to its `cmd` inbox verb
  `ecv1/{device}/TsComponentSkeleton/main/cmd/echo` and replies to each request.
- **Periodic publish** — publishes `ecv1/{device}/TsComponentSkeleton/main/data/seq` every
  `component.global.publish_interval` seconds, emitting a `messages_published` metric per send.
- **IoT Core telemetry** — mirrors each data message to AWS IoT Core on the *same* UNS topic
  (a UNS address is broker-independent), and acks IoT Core commands received on
  `…/cmd/run-demo` to `…/evt/cmd-ack`.
- **Dynamic config** — a config-change listener updates the publish interval live on a hot-reload.
- **Heartbeat** — automatic: the library publishes a `state` keepalive on
  `ecv1/{device}/TsComponentSkeleton/main/state` every `heartbeat.intervalSecs` (default 5 s),
  and emits the enabled CPU/memory measures as the `sys` metric through `metricEmission`.
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

Subscribe to `ecv1/my-thing/TsComponentSkeleton/main/data/seq` in an MQTT client to see
published messages (or `ecv1/my-thing/+/+/state` for the automatic heartbeat keepalives),
and publish to `ecv1/my-thing/TsComponentSkeleton/main/cmd/echo` (with a `reply_to`
header) to exercise request/reply.

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
