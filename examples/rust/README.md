# Rust Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in Rust on top of the
[`ggcommons`](../../libs/rust) Rust library. It is the Rust counterpart of
`examples/java` and `examples/python`, demonstrating the
library's standard CLI contract, configuration, logging, messaging
(publish + request/reply), metrics, and heartbeat — so a component author writes
only business logic.

## What it does

- **Request/reply** — subscribes to `<thing>/skeleton/request` and replies to each request.
- **Periodic publish** — publishes `<thing>/skeleton/data` every
  `component.global.publish_interval` seconds, emitting a `messages_published` metric per send.
- **Heartbeat** — periodic CPU/memory system metrics (configured in `recipe.yaml` / `config.json`).
- **Graceful shutdown** — runs until Ctrl-C / SIGTERM, unsubscribes, and drops the
  runtime cleanly (RAII).

> The messaging features above work on both the **HOST** platform (MQTT transport,
> against an MQTT broker) and the **GREENGRASS** platform (IPC transport, Greengrass
> IPC, built with the `greengrass` feature) — both validated against a live Greengrass core.

## Run locally (HOST platform, MQTT transport)

Start a local broker (see the workspace `CLAUDE.md`), then:

```bash
cargo run -- \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

Subscribe to `my-thing/skeleton/data` in an MQTT client to see published messages,
and publish to `my-thing/skeleton/request` (with a `replyTo` header) to exercise
request/reply.

## CLI contract

Same as the Java/Python skeletons:

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Build & publish with the GDK

This component uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command`: `bash build.sh`). `build.sh` compiles the binary and stages
it into `greengrass-build/` per the GDK contract (recipe → `greengrass-build/recipes/`,
artifact → `greengrass-build/artifacts/<name>/<version>/`).

```bash
gdk component build
gdk component publish
```

**Cross-compilation:** Greengrass cores typically run Linux. Build on a Linux host,
or set a Linux target you have a toolchain for:

```bash
GGCOMMONS_TARGET=x86_64-unknown-linux-gnu gdk component build
```

The recipe declares a `linux` platform and runs the binary on the GREENGRASS platform,
reading its configuration from the deployment (`GG_CONFIG`).
