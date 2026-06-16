# Rust Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in Rust on top of the
[`ggcommons`](../ggcommons-rust-lib) Rust library. It is the Rust counterpart of
`java-component-skeleton` and `python-component-skeleton`, demonstrating the
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

> Messaging in **GREENGRASS** mode (Greengrass IPC) is Phase 2 of the Rust port. On
> a Greengrass core today the component runs heartbeat-only. The messaging features
> above are fully exercised in **STANDALONE** mode against an MQTT broker.

## Run locally (STANDALONE mode)

Start a local broker (see the workspace `CLAUDE.md`), then:

```bash
cargo run -- \
  -m STANDALONE ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

Subscribe to `my-thing/skeleton/data` in an MQTT client to see published messages,
and publish to `my-thing/skeleton/request` (with a `replyTo` header) to exercise
request/reply.

## CLI contract

Same as the Java/Python skeletons:

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
- `-m/--mode <MODE> [path]` — `GREENGRASS (default) | STANDALONE <messaging_config.json>`
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

The recipe declares a `linux` platform and runs the binary in GREENGRASS mode,
reading its configuration from the deployment (`GG_CONFIG`).
