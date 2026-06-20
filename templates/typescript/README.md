# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in TypeScript on
top of the [`ggcommons`](https://example.com/ggcommons-ts-lib) TypeScript library,
generated from `ts-component-template` by the `ggcommons-cli`. It gives you the
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

## Develop & run locally (STANDALONE mode)

Local development uses STANDALONE mode (dual-broker MQTT) — no Greengrass core
needed. Install dependencies, build, start a local MQTT broker, then:

```bash
npm install
npm run build
node dist/main.js \
  -m STANDALONE ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
- `-m/--mode <MODE> [path]` — `GREENGRASS (default) | STANDALONE <messaging_config.json>`
- `-t/--thing <name>` — IoT Thing name

## Build & publish with the GDK (on-device)

The on-device build uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command: bash build.sh`). `build.sh` runs `npm install` + `npm run
build` (tsc) and stages a ZIP artifact (`dist/` + `node_modules/` + `package.json`)
per the GDK contract.

```bash
gdk component build
gdk component publish
```

The recipe declares a `linux` platform and runs the prebuilt JS in GREENGRASS mode
(`node {artifacts:decompressedPath}/<<COMPONENTNAME>>/dist/main.js -c GG_CONFIG`),
reading its configuration from the deployment (`GG_CONFIG`).

## The ggcommons dependency

`package.json` depends on the `ggcommons` library via a `file:` path dependency
(filled in at generation time). When the library is published to an npm registry,
replace that path dependency with the corresponding registry version.
